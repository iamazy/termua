#[cfg(unix)]
fn should_include_dev_name(name: &str) -> bool {
    // Linux
    if name.starts_with("ttyUSB")
        || name.starts_with("ttyACM")
        || name.starts_with("ttyS")
        || name.starts_with("ttyAMA")
        || name.starts_with("ttyTHS")
        || name.starts_with("ttyO")
        || name.starts_with("ttyGS")
        || name.starts_with("rfcomm")
        || name == "serial0"
        || name == "serial1"
    {
        return true;
    }

    // macOS
    if name.starts_with("cu.") || name.starts_with("tty.") {
        return true;
    }

    false
}

pub(crate) fn open_failure_hint(port: &str, err: &anyhow::Error) -> Option<String> {
    if !err_is_permission_denied(err) {
        return None;
    }

    let mut hint = String::new();
    hint.push_str("Hint:\n");

    #[cfg(target_os = "linux")]
    {
        hint.push_str(&format!(
            "- Linux: make sure your user can access `{port}` (usually `dialout`, sometimes \
             `uucp`).\n  Try: `sudo usermod -aG dialout $USER` then log out/in.\n"
        ));
        hint.push_str(&format!("- Check permissions: `ls -l {port}`\n"));
    }

    #[cfg(target_os = "macos")]
    {
        let _ = port;
        hint.push_str(
            "- macOS: prefer `/dev/cu.*` devices; make sure no other app is using the port.\n",
        );
    }

    #[cfg(windows)]
    {
        let _ = port;
        hint.push_str(
            "- Windows: make sure the COM port isn't already in use; try running Termua as \
             Administrator.\n",
        );
    }

    Some(hint.trim().to_string())
}

fn err_is_permission_denied(err: &anyhow::Error) -> bool {
    fn is_permission_denied_str(s: &str) -> bool {
        let s = s.to_ascii_lowercase();
        s.contains("permission denied")
            || s.contains("access is denied")
            || s.contains("os error 13")
            || s.contains("os error 5")
    }

    err.chain().any(|cause| {
        #[allow(clippy::match_like_matches_macro)]
        {
            // Prefer structured detection when possible.
            if let Some(io) = cause.downcast_ref::<std::io::Error>() {
                return io.kind() == std::io::ErrorKind::PermissionDenied;
            }
        }

        is_permission_denied_str(&cause.to_string())
    })
}

#[cfg(unix)]
pub(crate) fn list_ports() -> Vec<String> {
    let mut stable_aliases = Vec::new();
    let mut direct_devices = Vec::new();
    let mut ptys = Vec::new();

    let Ok(entries) = std::fs::read_dir("/dev") else {
        return Vec::new();
    };

    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if should_include_dev_name(name) {
            direct_devices.push(format!("/dev/{name}"));
        }
    }

    // Stable aliases (usually Linux): `/dev/serial/by-id/*` and `/dev/serial/by-path/*`.
    //
    // These are often easier to identify and remain stable across reboots compared to
    // `/dev/ttyUSB0` indices.
    {
        use std::os::unix::fs::FileTypeExt;

        fn extend_from_dir(out: &mut Vec<String>, dir: &str) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };

            for entry in entries.flatten() {
                let path = entry.path();
                let Ok(meta) = std::fs::metadata(&path) else {
                    continue;
                };
                if !meta.file_type().is_char_device() {
                    continue;
                }
                out.push(path.to_string_lossy().to_string());
            }
        }

        extend_from_dir(&mut stable_aliases, "/dev/serial/by-id");
        extend_from_dir(&mut stable_aliases, "/dev/serial/by-path");
    }

    // PTYs (usually used for testing/bridging with tools like `socat`).
    if let Ok(entries) = std::fs::read_dir("/dev/pts") {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if name == "ptmx" {
                continue;
            }
            if !name.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }

            ptys.push(format!("/dev/pts/{name}"));
        }
    }

    // Keep results stable and user-friendly:
    // 1) stable aliases (by-id/by-path)
    // 2) direct /dev entries (ttyUSB*, cu.*, ...)
    // 3) PTYs (/dev/pts/*)
    stable_aliases.sort();
    stable_aliases.dedup();
    direct_devices.sort();
    direct_devices.dedup();
    ptys.sort();
    ptys.dedup();

    let mut out = Vec::with_capacity(stable_aliases.len() + direct_devices.len() + ptys.len());
    out.extend(stable_aliases);
    out.extend(direct_devices);
    out.extend(ptys);
    out
}

#[cfg(windows)]
pub(crate) fn list_ports() -> Vec<String> {
    match windows_list_ports_from_registry() {
        Ok(ports) if !ports.is_empty() => ports,
        _ => (1..=64).map(|n| format!("COM{n}")).collect(),
    }
}

#[cfg(windows)]
fn windows_list_ports_from_registry() -> anyhow::Result<Vec<String>> {
    use windows::{
        Win32::{
            Foundation::ERROR_NO_MORE_ITEMS,
            System::Registry::{HKEY, HKEY_LOCAL_MACHINE, KEY_READ, RegEnumValueW, RegOpenKeyExW},
        },
        core::PWSTR,
    };

    let subkey = windows::core::w!("HARDWARE\\DEVICEMAP\\SERIALCOMM");

    let mut hkey = HKEY::default();
    let status = unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, subkey, Some(0), KEY_READ, &mut hkey) };
    status.ok()?;
    let _guard = WindowsRegKeyGuard(hkey);

    let mut out = Vec::new();

    let mut index = 0u32;
    loop {
        // Name
        let mut name_buf = [0u16; 256];
        let mut name_len = name_buf.len() as u32;

        // Value (COMx)
        let mut value_buf = [0u16; 256];
        let mut value_len = (value_buf.len() * 2) as u32;

        let mut value_type = 0u32;
        let status = unsafe {
            RegEnumValueW(
                hkey,
                index,
                Some(PWSTR(name_buf.as_mut_ptr())),
                &mut name_len,
                None,
                Some((&mut value_type) as *mut u32),
                Some(value_buf.as_mut_ptr() as *mut u8),
                Some((&mut value_len) as *mut u32),
            )
        };

        if status == ERROR_NO_MORE_ITEMS {
            break;
        }

        status.ok()?;

        // Only keep string values (REG_SZ). For now, just ignore the value type since
        // SERIALCOMM is expected to be string values.
        let _ = value_type;

        // `value_len` is bytes.
        let value_chars = (value_len as usize / 2).min(value_buf.len());
        let value = String::from_utf16_lossy(&value_buf[..value_chars]);
        let value = value.trim_matches('\0').trim().to_string();
        if !value.is_empty() {
            out.push(value);
        }

        index += 1;
    }

    out.sort();
    out.dedup();
    Ok(out)
}

#[cfg(windows)]
struct WindowsRegKeyGuard(windows::Win32::System::Registry::HKEY);

#[cfg(windows)]
impl Drop for WindowsRegKeyGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::System::Registry::RegCloseKey(self.0);
        }
    }
}
