use anyhow::anyhow;
use windows::{
    Win32::{
        Foundation::{CloseHandle, ERROR_LOGON_FAILURE, GetLastError, HANDLE},
        Security::{LOGON32_LOGON_NETWORK, LOGON32_PROVIDER_DEFAULT, LogonUserW},
    },
    core::PCWSTR,
};

use super::Authenticator;

pub(super) struct WindowsAuthenticator {
    username: String,
}

impl WindowsAuthenticator {
    pub(super) fn new(username: &str) -> Self {
        Self {
            username: username.to_string(),
        }
    }
}

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

impl Authenticator for WindowsAuthenticator {
    fn verify_password(&self, password: &str) -> anyhow::Result<bool> {
        let user = to_wide(&self.username);
        let pass = to_wide(password);

        let mut token = HANDLE::default();
        let res = unsafe {
            LogonUserW(
                PCWSTR(user.as_ptr()),
                PCWSTR::null(),
                PCWSTR(pass.as_ptr()),
                LOGON32_LOGON_NETWORK,
                LOGON32_PROVIDER_DEFAULT,
                &mut token,
            )
        };
        let ok = res.is_ok();

        if ok {
            unsafe {
                let _ = CloseHandle(token);
            }
            return Ok(true);
        }

        let err = unsafe { GetLastError() };
        if err == ERROR_LOGON_FAILURE {
            return Ok(false);
        }

        Err(anyhow!("LogonUserW failed: {err:?}"))
    }
}
