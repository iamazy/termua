use std::{
    ffi::{CStr, CString},
    mem::size_of,
    os::raw::{c_char, c_int, c_void},
    ptr,
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

#[link(name = "pam")]
unsafe extern "C" {
    fn pam_start(
        service_name: *const c_char,
        user: *const c_char,
        pam_conversation: *const PamConv,
        pamh: *mut PamHandle,
    ) -> c_int;
    fn pam_end(pamh: PamHandle, pam_status: c_int) -> c_int;
    fn pam_authenticate(pamh: PamHandle, flags: c_int) -> c_int;
    fn pam_acct_mgmt(pamh: PamHandle, flags: c_int) -> c_int;
    fn pam_strerror(pamh: PamHandle, errnum: c_int) -> *const c_char;
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

fn pam_error(pamh: PamHandle, code: c_int) -> anyhow::Error {
    unsafe {
        let ptr = pam_strerror(pamh, code);
        if ptr.is_null() {
            return anyhow!("pam error {code}");
        }
        let s = CStr::from_ptr(ptr).to_string_lossy().to_string();
        anyhow!("pam error {code}: {s}")
    }
}

fn pam_authenticate_with(
    service: &CString,
    username: &CString,
    password: &CString,
) -> anyhow::Result<bool> {
    let mut pamh: PamHandle = ptr::null_mut();
    let mut conv = PamConv {
        conv: Some(conv),
        appdata_ptr: password.as_ptr() as *mut c_void,
    };

    let code = unsafe { pam_start(service.as_ptr(), username.as_ptr(), &mut conv, &mut pamh) };
    if code != PAM_SUCCESS {
        return Err(pam_error(pamh, code));
    }

    let code = unsafe { pam_authenticate(pamh, 0) };
    if code != PAM_SUCCESS {
        unsafe { pam_end(pamh, code) };
        if matches!(
            code,
            PAM_AUTH_ERR | PAM_USER_UNKNOWN | PAM_ACCT_EXPIRED | PAM_PERM_DENIED
        ) {
            return Ok(false);
        }
        return Err(pam_error(pamh, code));
    }

    let code = unsafe { pam_acct_mgmt(pamh, 0) };
    unsafe { pam_end(pamh, code) };

    if code == PAM_SUCCESS {
        Ok(true)
    } else if matches!(
        code,
        PAM_AUTH_ERR | PAM_USER_UNKNOWN | PAM_ACCT_EXPIRED | PAM_PERM_DENIED
    ) {
        Ok(false)
    } else {
        Err(pam_error(pamh, code))
    }
}

pub(super) struct PamAuthenticator {
    username: CString,
    login_service: CString,
}

impl PamAuthenticator {
    pub(super) fn new(username: &str) -> anyhow::Result<Self> {
        Ok(Self {
            username: CString::new(username)?,
            login_service: CString::new("login")?,
        })
    }
}

impl Authenticator for PamAuthenticator {
    fn verify_password(&self, password: &str) -> anyhow::Result<bool> {
        let password = CString::new(password)?;
        pam_authenticate_with(&self.login_service, &self.username, &password)
    }
}
