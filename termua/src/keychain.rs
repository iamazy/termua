#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use anyhow::Context;

const TERMUA_SERVICE: &str = "termua";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Secret<'a> {
    SshPassword { session_id: i64 },
    Named { kind: &'a str, key: &'a str },
}

impl<'a> Secret<'a> {
    fn ssh_password(session_id: i64) -> Self {
        Self::SshPassword { session_id }
    }

    fn named(kind: &'a str, key: &'a str) -> Self {
        Self::Named { kind, key }
    }

    #[cfg(any(target_os = "linux", test))]
    fn label(self) -> String {
        match self {
            Self::SshPassword { session_id } => {
                format!("termua ssh password (session {session_id})")
            }
            Self::Named { kind, key } => format!("termua secret ({kind}:{key})"),
        }
    }

    #[cfg(any(target_os = "windows", target_os = "macos", test))]
    fn account_name(self) -> String {
        match self {
            Self::SshPassword { session_id } => format!("ssh_password:{session_id}"),
            Self::Named { kind, key } => format!("{kind}:{key}"),
        }
    }

    #[cfg(any(target_os = "windows", test))]
    fn storage_target(self) -> String {
        format!("{TERMUA_SERVICE}:{}", self.account_name())
    }
}

pub fn store_ssh_password(session_id: i64, password: &str) -> anyhow::Result<()> {
    store_os(Secret::ssh_password(session_id), password)
}

pub fn load_ssh_password(session_id: i64) -> anyhow::Result<Option<String>> {
    load_os(Secret::ssh_password(session_id))
}

pub fn delete_ssh_password(session_id: i64) -> anyhow::Result<()> {
    delete_os(Secret::ssh_password(session_id))
}

pub fn store_zeroclaw_api_key(api_key: &str) -> anyhow::Result<()> {
    store_os(Secret::named("zeroclaw_api_key", "default"), api_key)
}

pub fn load_zeroclaw_api_key() -> anyhow::Result<Option<String>> {
    load_os(Secret::named("zeroclaw_api_key", "default"))
}

pub fn delete_zeroclaw_api_key() -> anyhow::Result<()> {
    delete_os(Secret::named("zeroclaw_api_key", "default"))
}

fn store_os(secret: Secret<'_>, value: &str) -> anyhow::Result<()> {
    #[cfg(target_os = "windows")]
    {
        return windows_backend::store_secret(secret, value);
    }

    #[cfg(target_os = "macos")]
    {
        return macos_backend::store_secret(secret, value);
    }

    #[cfg(target_os = "linux")]
    {
        return linux_backend::store_secret(secret, value);
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        let _ = secret;
        let _ = value;
        anyhow::bail!("system credential manager is not supported on this platform");
    }
}

fn load_os(secret: Secret<'_>) -> anyhow::Result<Option<String>> {
    #[cfg(target_os = "windows")]
    {
        return windows_backend::load_secret(secret);
    }

    #[cfg(target_os = "macos")]
    {
        return macos_backend::load_secret(secret);
    }

    #[cfg(target_os = "linux")]
    {
        return linux_backend::load_secret(secret);
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        let _ = secret;
        Ok(None)
    }
}

fn delete_os(secret: Secret<'_>) -> anyhow::Result<()> {
    #[cfg(target_os = "windows")]
    {
        return windows_backend::delete_secret(secret);
    }

    #[cfg(target_os = "macos")]
    {
        return macos_backend::delete_secret(secret);
    }

    #[cfg(target_os = "linux")]
    {
        return linux_backend::delete_secret(secret);
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        let _ = secret;
        Ok(())
    }
}

#[cfg(target_os = "windows")]
mod windows_backend {
    use std::ptr;

    use windows::{
        Win32::{
            Foundation::{ERROR_NOT_FOUND, GetLastError},
            Security::Credentials::{
                CRED_FLAGS, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW,
                CredDeleteW, CredFree, CredReadW, CredWriteW,
            },
        },
        core::{PCWSTR, PWSTR},
    };

    use super::*;

    fn to_utf16_nul(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn store_secret(secret: Secret<'_>, value: &str) -> anyhow::Result<()> {
        let account = secret.account_name();
        let target = to_utf16_nul(&secret.storage_target());
        let user = to_utf16_nul(TERMUA_SERVICE);
        let blob = value.as_bytes();

        let mut cred = build_credential(&target, &user, blob);
        unsafe { CredWriteW(&mut cred, 0) }.with_context(|| {
            format!(
                "CredWriteW failed for secret {account} ({})",
                unsafe { GetLastError() }.0
            )
        })?;
        Ok(())
    }

    fn build_credential(target: &[u16], user: &[u16], blob: &[u8]) -> CREDENTIALW {
        // SAFETY: The Windows API consumes the pointer only for the duration of the call.
        CREDENTIALW {
            Flags: CRED_FLAGS(0),
            Type: CRED_TYPE_GENERIC,
            TargetName: PWSTR(target.as_ptr() as *mut _),
            Comment: PWSTR::null(),
            CredentialBlobSize: blob.len() as u32,
            CredentialBlob: blob.as_ptr() as *mut u8,
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            AttributeCount: 0,
            Attributes: ptr::null_mut(),
            TargetAlias: PWSTR::null(),
            UserName: PWSTR(user.as_ptr() as *mut _),
            ..Default::default()
        }
    }

    pub fn load_secret(secret: Secret<'_>) -> anyhow::Result<Option<String>> {
        let account = secret.account_name();
        let target = to_utf16_nul(&secret.storage_target());
        let mut out: *mut CREDENTIALW = ptr::null_mut();

        if let Err(_err) = unsafe {
            CredReadW(
                PCWSTR(target.as_ptr()),
                CRED_TYPE_GENERIC,
                Some(0),
                &mut out,
            )
        } {
            let err = unsafe { GetLastError() };
            if err == ERROR_NOT_FOUND {
                return Ok(None);
            }
            anyhow::bail!("CredReadW failed ({})", err.0);
        }

        let cred = LoadedCredential(out);
        let blob = cred.blob();
        let s = String::from_utf8(blob.to_vec())
            .with_context(|| format!("credential blob utf-8 for secret {account}"))?;
        Ok(Some(s))
    }

    pub fn delete_secret(secret: Secret<'_>) -> anyhow::Result<()> {
        let target = to_utf16_nul(&secret.storage_target());
        if let Err(_err) =
            unsafe { CredDeleteW(PCWSTR(target.as_ptr()), CRED_TYPE_GENERIC, Some(0)) }
        {
            let err = unsafe { GetLastError() };
            if err == ERROR_NOT_FOUND {
                return Ok(());
            }
            anyhow::bail!("CredDeleteW failed ({})", err.0);
        }

        Ok(())
    }

    struct LoadedCredential(*mut CREDENTIALW);

    impl LoadedCredential {
        fn blob(&self) -> &[u8] {
            let cred = unsafe { &*self.0 };
            unsafe {
                std::slice::from_raw_parts(cred.CredentialBlob, cred.CredentialBlobSize as usize)
            }
        }
    }

    impl Drop for LoadedCredential {
        fn drop(&mut self) {
            unsafe { CredFree(self.0 as *const _) };
        }
    }
}

#[cfg(target_os = "macos")]
mod macos_backend {
    use std::{ffi::c_void, ptr};

    use super::*;

    type OSStatus = i32;
    type SecKeychainItemRef = *mut c_void;

    #[link(name = "Security", kind = "framework")]
    unsafe extern "C" {
        fn SecKeychainAddGenericPassword(
            keychain: *const c_void,
            service_name_length: u32,
            service_name: *const u8,
            account_name_length: u32,
            account_name: *const u8,
            password_length: u32,
            password_data: *const c_void,
            item_ref: *mut SecKeychainItemRef,
        ) -> OSStatus;

        fn SecKeychainFindGenericPassword(
            keychain: *const c_void,
            service_name_length: u32,
            service_name: *const u8,
            account_name_length: u32,
            account_name: *const u8,
            password_length: *mut u32,
            password_data: *mut *mut c_void,
            item_ref: *mut SecKeychainItemRef,
        ) -> OSStatus;

        fn SecKeychainItemModifyAttributesAndData(
            item_ref: SecKeychainItemRef,
            attr_list: *const c_void,
            length: u32,
            data: *const c_void,
        ) -> OSStatus;

        fn SecKeychainItemDelete(item_ref: SecKeychainItemRef) -> OSStatus;
        fn SecKeychainItemFreeContent(attr_list: *const c_void, data: *mut c_void) -> OSStatus;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRelease(cf: *const c_void);
    }

    const ERR_SEC_ITEM_NOT_FOUND: OSStatus = -25300;
    const ERR_SEC_DUPLICATE_ITEM: OSStatus = -25299;

    fn as_bytes(s: &str) -> (*const u8, u32) {
        (s.as_ptr(), s.len() as u32)
    }

    struct FoundSecret {
        item: SecKeychainItemRef,
        pw_len: u32,
        pw_ptr: *mut c_void,
    }

    impl FoundSecret {
        fn bytes(&self) -> &[u8] {
            unsafe { std::slice::from_raw_parts(self.pw_ptr as *const u8, self.pw_len as usize) }
        }
    }

    impl Drop for FoundSecret {
        fn drop(&mut self) {
            if !self.pw_ptr.is_null() {
                let _ = unsafe { SecKeychainItemFreeContent(ptr::null(), self.pw_ptr) };
            }

            if !self.item.is_null() {
                unsafe { CFRelease(self.item) };
            }
        }
    }

    fn find_secret(account: &str) -> anyhow::Result<Option<FoundSecret>> {
        let (svc_ptr, svc_len) = as_bytes(TERMUA_SERVICE);
        let (acct_ptr, acct_len) = as_bytes(&account);

        let mut pw_len: u32 = 0;
        let mut pw_ptr: *mut c_void = ptr::null_mut();
        let mut item: SecKeychainItemRef = ptr::null_mut();
        let status = unsafe {
            SecKeychainFindGenericPassword(
                ptr::null(),
                svc_len,
                svc_ptr,
                acct_len,
                acct_ptr,
                &mut pw_len,
                &mut pw_ptr,
                &mut item,
            )
        };
        if status == ERR_SEC_ITEM_NOT_FOUND {
            return Ok(None);
        }
        if status != 0 {
            anyhow::bail!("SecKeychainFindGenericPassword failed ({status})");
        }

        Ok(Some(FoundSecret {
            item,
            pw_len,
            pw_ptr,
        }))
    }

    pub fn store_secret(secret: Secret<'_>, value: &str) -> anyhow::Result<()> {
        let account = secret.account_name();
        let (svc_ptr, svc_len) = as_bytes(TERMUA_SERVICE);
        let (acct_ptr, acct_len) = as_bytes(&account);

        let mut item: SecKeychainItemRef = ptr::null_mut();
        let mut status = unsafe {
            SecKeychainAddGenericPassword(
                ptr::null(),
                svc_len,
                svc_ptr,
                acct_len,
                acct_ptr,
                value.len() as u32,
                value.as_ptr() as *const c_void,
                &mut item,
            )
        };

        if status == ERR_SEC_DUPLICATE_ITEM {
            let found = find_secret(&account)?
                .with_context(|| format!("missing existing keychain item for secret {account}"))?;
            status = unsafe {
                SecKeychainItemModifyAttributesAndData(
                    found.item,
                    ptr::null(),
                    value.len() as u32,
                    value.as_ptr() as *const c_void,
                )
            };
        }

        if status != 0 {
            anyhow::bail!("SecKeychainAddGenericPassword failed ({status})");
        }

        if !item.is_null() {
            unsafe { CFRelease(item) };
        }
        Ok(())
    }

    pub fn load_secret(secret: Secret<'_>) -> anyhow::Result<Option<String>> {
        let account = secret.account_name();
        let Some(found) = find_secret(&account)? else {
            return Ok(None);
        };

        let value = String::from_utf8(found.bytes().to_vec()).context("password utf-8")?;
        Ok(Some(value))
    }

    pub fn delete_secret(secret: Secret<'_>) -> anyhow::Result<()> {
        let account = secret.account_name();
        let Some(found) = find_secret(&account)? else {
            return Ok(());
        };

        let del = unsafe { SecKeychainItemDelete(found.item) };
        if del != 0 {
            anyhow::bail!("SecKeychainItemDelete failed ({del})");
        }
        Ok(())
    }
}

#[cfg(target_os = "linux")]
mod linux_backend {
    use std::collections::HashMap;

    use secret_service::{EncryptionType, blocking::SecretService};

    use super::*;

    fn with_attrs<T>(secret: Secret<'_>, f: impl FnOnce(HashMap<&'static str, &str>) -> T) -> T {
        match secret {
            Secret::SshPassword { session_id } => {
                let session_id = session_id.to_string();
                f(HashMap::from([
                    ("app", TERMUA_SERVICE),
                    ("kind", "ssh_password"),
                    ("session_id", session_id.as_str()),
                ]))
            }
            Secret::Named { kind, key } => f(HashMap::from([
                ("app", TERMUA_SERVICE),
                ("kind", kind),
                ("key", key),
            ])),
        }
    }

    fn label(secret: Secret<'_>) -> String {
        secret.label()
    }

    pub fn store_secret(secret: Secret<'_>, value: &str) -> anyhow::Result<()> {
        let ss = SecretService::connect(EncryptionType::Dh).context("connect secret service")?;
        let collection = ss
            .get_any_collection()
            .context("get secret service collection")?;

        if collection.is_locked().unwrap_or(false) {
            let _ = collection.unlock();
        }

        with_attrs(secret, |attrs| {
            collection
                .create_item(&label(secret), attrs, value.as_bytes(), true, "text/plain")
                .context("create secret service item")
        })?;
        Ok(())
    }

    pub fn load_secret(secret: Secret<'_>) -> anyhow::Result<Option<String>> {
        let ss = SecretService::connect(EncryptionType::Dh).context("connect secret service")?;
        let items = with_attrs(secret, |attrs| {
            ss.search_items(attrs)
                .context("search secret service items")
        })?;

        let item = items.unlocked.first().or_else(|| items.locked.first());
        let Some(item) = item else {
            return Ok(None);
        };

        if item.is_locked().unwrap_or(false) {
            let _ = item.unlock();
        }

        let secret = item.get_secret().context("read secret service item")?;
        let value = String::from_utf8(secret).context("secret service value utf-8")?;
        Ok(Some(value))
    }

    pub fn delete_secret(secret: Secret<'_>) -> anyhow::Result<()> {
        let ss = SecretService::connect(EncryptionType::Dh).context("connect secret service")?;
        let items = with_attrs(secret, |attrs| {
            ss.search_items(attrs)
                .context("search secret service items")
        })?;

        for item in items.unlocked.into_iter().chain(items.locked) {
            if item.is_locked().unwrap_or(false) {
                let _ = item.unlock();
            }
            let _ = item.delete();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::Secret;

    #[test]
    fn ssh_secret_descriptor_builds_expected_account_name() {
        let secret = Secret::ssh_password(42);

        assert_eq!(secret.account_name(), "ssh_password:42");
    }

    #[test]
    fn named_secret_descriptor_builds_expected_account_name() {
        let secret = Secret::named("zeroclaw_api_key", "default");

        assert_eq!(secret.account_name(), "zeroclaw_api_key:default");
    }

    #[test]
    fn ssh_secret_descriptor_builds_expected_storage_target_and_label() {
        let secret = Secret::ssh_password(42);

        assert_eq!(secret.storage_target(), "termua:ssh_password:42");
        assert_eq!(secret.label(), "termua ssh password (session 42)");
    }

    #[test]
    fn named_secret_descriptor_builds_expected_storage_target_and_label() {
        let secret = Secret::named("zeroclaw_api_key", "default");

        assert_eq!(secret.storage_target(), "termua:zeroclaw_api_key:default");
        assert_eq!(secret.label(), "termua secret (zeroclaw_api_key:default)");
    }
}
