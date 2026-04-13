use std::{
    ffi::{CStr, CString},
    mem::size_of,
    os::raw::{c_char, c_int, c_void},
    ptr,
    sync::OnceLock,
};

use anyhow::anyhow;

use super::Authenticator;

// Opaque type.
type PamHandle = *mut c_void;

#[repr(C)]
struct PamMessage {
    msg_style: c_int,
    msg: *const c_char,
}

#[repr(C)]
struct PamResponse {
    resp: *mut c_char,
    resp_retcode: c_int,
}

#[repr(C)]
struct PamConv {
    conv: Option<
        unsafe extern "C" fn(
            num_msg: c_int,
            msg: *mut *const PamMessage,
            resp: *mut *mut PamResponse,
            appdata_ptr: *mut c_void,
        ) -> c_int,
    >,
    appdata_ptr: *mut c_void,
}

const PAM_SUCCESS: c_int = 0;
const PAM_PROMPT_ECHO_OFF: c_int = 1;
const PAM_PROMPT_ECHO_ON: c_int = 2;
const PAM_AUTH_ERR: c_int = 9;
const PAM_USER_UNKNOWN: c_int = 10;
const PAM_ACCT_EXPIRED: c_int = 13;
const PAM_PERM_DENIED: c_int = 6;
const PAM_CRED_INSUFFICIENT: c_int = 8;

#[link(name = "dl")]
unsafe extern "C" {
    fn dlopen(filename: *const c_char, flag: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

#[derive(Clone, Copy)]
pub(super) struct PamApi {
    // Hold the `dlopen` handle so the symbols remain valid for the lifetime of this process.
    #[allow(dead_code)]
    handle: *mut c_void,
    pam_start: unsafe extern "C" fn(
        service_name: *const c_char,
        user: *const c_char,
        pam_conversation: *const PamConv,
        pamh: *mut PamHandle,
    ) -> c_int,
    pam_end: unsafe extern "C" fn(pamh: PamHandle, pam_status: c_int) -> c_int,
    pam_authenticate: unsafe extern "C" fn(pamh: PamHandle, flags: c_int) -> c_int,
    pam_acct_mgmt: unsafe extern "C" fn(pamh: PamHandle, flags: c_int) -> c_int,
    pam_strerror: unsafe extern "C" fn(pamh: PamHandle, errnum: c_int) -> *const c_char,
}

unsafe impl Send for PamApi {}
unsafe impl Sync for PamApi {}

impl PamApi {
    pub(super) fn load_from_names(names: &[&str]) -> Option<Self> {
        for name in names {
            let Ok(name) = CString::new(*name) else {
                continue;
            };
            let handle = unsafe { dlopen(name.as_ptr(), libc::RTLD_NOW) };
            if handle.is_null() {
                continue;
            }

            unsafe fn load_sym<T>(handle: *mut c_void, name: &CStr) -> Option<T> {
                let sym = unsafe { dlsym(handle, name.as_ptr()) };
                if sym.is_null() {
                    return None;
                }
                Some(unsafe { std::mem::transmute_copy(&sym) })
            }

            fn c(name: &'static [u8]) -> &'static CStr {
                CStr::from_bytes_with_nul(name).expect("valid CStr literal")
            }

            type PamStartFn = unsafe extern "C" fn(
                service_name: *const c_char,
                user: *const c_char,
                pam_conversation: *const PamConv,
                pamh: *mut PamHandle,
            ) -> c_int;
            type PamEndFn = unsafe extern "C" fn(pamh: PamHandle, pam_status: c_int) -> c_int;
            type PamAuthenticateFn = unsafe extern "C" fn(pamh: PamHandle, flags: c_int) -> c_int;
            type PamAcctMgmtFn = unsafe extern "C" fn(pamh: PamHandle, flags: c_int) -> c_int;
            type PamStrerrorFn =
                unsafe extern "C" fn(pamh: PamHandle, errnum: c_int) -> *const c_char;

            let pam_start: PamStartFn = unsafe { load_sym(handle, c(b"pam_start\0"))? };
            let pam_end: PamEndFn = unsafe { load_sym(handle, c(b"pam_end\0"))? };
            let pam_authenticate: PamAuthenticateFn =
                unsafe { load_sym(handle, c(b"pam_authenticate\0"))? };
            let pam_acct_mgmt: PamAcctMgmtFn = unsafe { load_sym(handle, c(b"pam_acct_mgmt\0"))? };
            let pam_strerror: PamStrerrorFn = unsafe { load_sym(handle, c(b"pam_strerror\0"))? };

            return Some(Self {
                handle,
                pam_start,
                pam_end,
                pam_authenticate,
                pam_acct_mgmt,
                pam_strerror,
            });
        }

        None
    }
}

fn pam_api() -> Option<&'static PamApi> {
    static API: OnceLock<Option<PamApi>> = OnceLock::new();
    API.get_or_init(|| PamApi::load_from_names(&["libpam.so.0", "libpam.so"]))
        .as_ref()
}

unsafe extern "C" fn conv(
    num_msg: c_int,
    msg: *mut *const PamMessage,
    resp: *mut *mut PamResponse,
    appdata_ptr: *mut c_void,
) -> c_int {
    if num_msg <= 0 || resp.is_null() {
        return PAM_PERM_DENIED;
    }

    let password = appdata_ptr as *const c_char;
    if password.is_null() {
        return PAM_PERM_DENIED;
    }

    let replies =
        unsafe { libc::calloc(num_msg as usize, size_of::<PamResponse>()) } as *mut PamResponse;
    if replies.is_null() {
        return PAM_CRED_INSUFFICIENT;
    }

    for i in 0..num_msg as isize {
        let m = unsafe { *msg.offset(i) };
        if m.is_null() {
            continue;
        }

        let style = unsafe { (*m).msg_style };
        if style == PAM_PROMPT_ECHO_OFF || style == PAM_PROMPT_ECHO_ON {
            let dup = unsafe { libc::strdup(password) };
            if dup.is_null() {
                unsafe { libc::free(replies as *mut c_void) };
                return PAM_CRED_INSUFFICIENT;
            }
            unsafe {
                (*replies.offset(i)).resp = dup;
                (*replies.offset(i)).resp_retcode = 0;
            }
        } else {
            unsafe {
                (*replies.offset(i)).resp = ptr::null_mut();
                (*replies.offset(i)).resp_retcode = 0;
            }
        }
    }

    unsafe { *resp = replies };
    PAM_SUCCESS
}

fn pam_error(api: &PamApi, pamh: PamHandle, code: c_int) -> anyhow::Error {
    unsafe {
        let ptr = (api.pam_strerror)(pamh, code);
        if ptr.is_null() {
            return anyhow!("pam error {code}");
        }
        let s = CStr::from_ptr(ptr).to_string_lossy().to_string();
        anyhow!("pam error {code}: {s}")
    }
}

fn pam_authenticate_with(
    api: &PamApi,
    service: &CString,
    username: &CString,
    password: &CString,
) -> anyhow::Result<bool> {
    let mut pamh: PamHandle = ptr::null_mut();
    let mut conv = PamConv {
        conv: Some(conv),
        appdata_ptr: password.as_ptr() as *mut c_void,
    };

    let code =
        unsafe { (api.pam_start)(service.as_ptr(), username.as_ptr(), &mut conv, &mut pamh) };
    if code != PAM_SUCCESS {
        return Err(pam_error(api, pamh, code));
    }

    let code = unsafe { (api.pam_authenticate)(pamh, 0) };
    if code != PAM_SUCCESS {
        unsafe { (api.pam_end)(pamh, code) };
        if matches!(
            code,
            PAM_AUTH_ERR | PAM_USER_UNKNOWN | PAM_ACCT_EXPIRED | PAM_PERM_DENIED
        ) {
            return Ok(false);
        }
        return Err(pam_error(api, pamh, code));
    }

    let code = unsafe { (api.pam_acct_mgmt)(pamh, 0) };
    unsafe { (api.pam_end)(pamh, code) };

    if code == PAM_SUCCESS {
        Ok(true)
    } else if matches!(
        code,
        PAM_AUTH_ERR | PAM_USER_UNKNOWN | PAM_ACCT_EXPIRED | PAM_PERM_DENIED
    ) {
        Ok(false)
    } else {
        Err(pam_error(api, pamh, code))
    }
}

fn choose_default_service() -> &'static str {
    let prefer = ["system-auth", "common-auth", "login"];
    for svc in prefer {
        let p = std::path::Path::new("/etc/pam.d").join(svc);
        if std::fs::metadata(p).is_ok() {
            return svc;
        }
    }
    "login"
}

pub(super) struct PamAuthenticator {
    api: &'static PamApi,
    username: CString,
    service: CString,
}

impl PamAuthenticator {
    pub(super) fn new(username: &str) -> anyhow::Result<Self> {
        let Some(api) = pam_api() else {
            return Err(anyhow!("libpam not available"));
        };

        let service = std::env::var("TERMUA_PAM_SERVICE")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| choose_default_service().to_string());

        Ok(Self {
            api,
            username: CString::new(username)?,
            service: CString::new(service)?,
        })
    }
}

impl Authenticator for PamAuthenticator {
    fn verify_password(&self, password: &str) -> anyhow::Result<bool> {
        let password = CString::new(password)?;
        pam_authenticate_with(self.api, &self.service, &self.username, &password)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn pam_loader_returns_none_for_missing_library() {
        assert!(super::PamApi::load_from_names(&["libpam-definitely-not-present.so"]).is_none());
    }
}
