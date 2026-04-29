use std::collections::HashMap;

pub const SHELL_ENV_KEY: &str = "SHELL";
pub const TERMUA_SHELL_ENV_KEY: &str = "TERMUA_SHELL";
pub const TERMUA_BASH_RCFILE_ENV_KEY: &str = "TERMUA_BASH_RCFILE";
pub const TERMUA_PWSH_INIT_ENV_KEY: &str = "TERMUA_PWSH_INIT";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShellKind {
    Bash,
    Zsh,
    Pwsh,
    PowerShell,
    Cmd,
    Other,
}

pub fn pick_shell_program_from_env(env: &HashMap<String, String>) -> Option<&str> {
    env.get(TERMUA_SHELL_ENV_KEY)
        .or_else(|| env.get(SHELL_ENV_KEY))
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
}

pub fn pick_shell_program_from_env_or_else(
    env: &HashMap<String, String>,
    fallback: impl FnOnce() -> Option<String>,
) -> Option<String> {
    pick_shell_program_from_env(env)
        .map(ToString::to_string)
        .or_else(|| {
            fallback()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
}

pub fn shell_kind(program: &str) -> ShellKind {
    let program = program.trim();
    if program.is_empty() {
        return ShellKind::Other;
    }

    let name = std::path::Path::new(program)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(program);

    match name {
        "bash" => ShellKind::Bash,
        "zsh" => ShellKind::Zsh,
        "pwsh" => ShellKind::Pwsh,
        "powershell" => ShellKind::PowerShell,
        "cmd" => ShellKind::Cmd,
        _ => ShellKind::Other,
    }
}

pub fn shell_display_name(program: &str) -> String {
    match shell_kind(program) {
        ShellKind::Bash => "bash".to_string(),
        ShellKind::Zsh => "zsh".to_string(),
        ShellKind::Pwsh | ShellKind::PowerShell => "powershell".to_string(),
        ShellKind::Cmd => "cmd".to_string(),
        ShellKind::Other => std::path::Path::new(program.trim())
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(program.trim())
            .to_string(),
    }
}

fn powershell_integration_args(bypass_execution_policy: bool) -> Vec<String> {
    let mut args = vec!["-NoLogo".to_string(), "-NoExit".to_string()];
    if bypass_execution_policy {
        args.push("-ExecutionPolicy".to_string());
        args.push("Bypass".to_string());
    }
    args.push("-Command".to_string());
    args.push(". \"$env:TERMUA_PWSH_INIT\"".to_string());
    args
}

pub fn shell_integration_args_for_env(program: &str, env: &HashMap<String, String>) -> Vec<String> {
    match shell_kind(program) {
        ShellKind::Bash => env
            .get(TERMUA_BASH_RCFILE_ENV_KEY)
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|rcfile| {
                vec![
                    "--noprofile".to_string(),
                    "--rcfile".to_string(),
                    rcfile.to_string(),
                    "-i".to_string(),
                ]
            })
            .unwrap_or_default(),
        ShellKind::Pwsh => env
            .get(TERMUA_PWSH_INIT_ENV_KEY)
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|_init| powershell_integration_args(cfg!(windows)))
            .unwrap_or_default(),
        ShellKind::Zsh | ShellKind::PowerShell | ShellKind::Cmd | ShellKind::Other => Vec::new(),
    }
}

pub fn shell_program_candidates() -> &'static [&'static str] {
    if cfg!(windows) {
        // Windows: prefer PowerShell 7+ when available, then Windows PowerShell, then cmd.
        &["pwsh", "powershell", "cmd"]
    } else if cfg!(target_os = "macos") {
        // macOS: default user shell is zsh on modern macOS.
        &["zsh", "bash", "pwsh"]
    } else {
        // Linux/*nix: bash is commonly available and expected.
        &["bash", "zsh", "pwsh"]
    }
}

pub fn default_shell_program() -> &'static str {
    if cfg!(windows) {
        "pwsh"
    } else if cfg!(target_os = "macos") {
        "zsh"
    } else {
        "bash"
    }
}

pub fn fallback_shell_program() -> &'static str {
    if cfg!(windows) {
        "powershell"
    } else if cfg!(target_os = "macos") {
        "zsh"
    } else if cfg!(target_os = "linux") {
        "bash"
    } else {
        "sh"
    }
}

#[cfg(any(windows, test))]
fn split_pathext(pathext: &str) -> Vec<String> {
    pathext
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

pub fn program_exists_on_path(program: &str) -> bool {
    if program.trim().is_empty() {
        return false;
    }

    let program = program.trim();

    // If an explicit path is provided, just check it exists.
    let program_path = std::path::Path::new(program);
    if program_path.components().count() > 1 {
        return program_path.is_file();
    }

    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };

    #[cfg(windows)]
    {
        let pathext = std::env::var_os("PATHEXT").unwrap_or_else(|| ".EXE;.CMD;.BAT;.COM".into());
        let pathext = pathext.to_string_lossy();
        let exts = split_pathext(pathext.as_ref());

        for dir in std::env::split_paths(&path) {
            if !dir.is_dir() {
                continue;
            }

            // Try direct match first.
            if dir.join(program).is_file() {
                return true;
            }

            // Try PATHEXT variations.
            for ext in &exts {
                if dir.join(format!("{program}{ext}")).is_file() {
                    return true;
                }
            }
        }

        false
    }

    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;

        for dir in std::env::split_paths(&path) {
            let candidate = dir.join(program);
            if let Ok(meta) = std::fs::metadata(&candidate)
                && meta.is_file()
                && (meta.permissions().mode() & 0o111) != 0
            {
                return true;
            }
        }

        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_shell_program_from_env_prefers_termua_shell() {
        let mut env = HashMap::new();
        env.insert(SHELL_ENV_KEY.to_string(), "bash".to_string());
        env.insert(TERMUA_SHELL_ENV_KEY.to_string(), "fish".to_string());

        assert_eq!(pick_shell_program_from_env(&env), Some("fish"));
    }

    #[test]
    fn pick_shell_program_from_env_or_else_uses_process_fallback() {
        let env = HashMap::new();

        assert_eq!(
            pick_shell_program_from_env_or_else(&env, || Some("/bin/zsh".to_string())),
            Some("/bin/zsh".to_string())
        );
    }

    #[test]
    fn shell_kind_detects_supported_shells() {
        assert_eq!(shell_kind("/bin/bash"), ShellKind::Bash);
        assert_eq!(shell_kind("zsh"), ShellKind::Zsh);
        assert_eq!(shell_kind("pwsh"), ShellKind::Pwsh);
        assert_eq!(shell_kind("powershell"), ShellKind::PowerShell);
        assert_eq!(shell_kind("cmd"), ShellKind::Cmd);
        assert_eq!(shell_kind("unknown"), ShellKind::Other);
    }

    #[test]
    fn shell_display_name_normalizes_supported_shells() {
        assert_eq!(shell_display_name("/bin/bash"), "bash");
        assert_eq!(shell_display_name("pwsh"), "powershell");
        assert_eq!(shell_display_name("powershell"), "powershell");
    }

    #[test]
    fn ui_default_shell_matches_platform_policy() {
        #[cfg(windows)]
        assert_eq!(default_shell_program(), "pwsh");

        #[cfg(target_os = "macos")]
        assert_eq!(default_shell_program(), "zsh");

        #[cfg(all(not(windows), not(target_os = "macos")))]
        assert_eq!(default_shell_program(), "bash");
    }

    #[test]
    fn shell_integration_args_build_for_pwsh() {
        let mut env = HashMap::new();
        env.insert(
            TERMUA_PWSH_INIT_ENV_KEY.to_string(),
            "/tmp/init.ps1".to_string(),
        );
        assert_eq!(
            shell_integration_args_for_env("pwsh", &env),
            powershell_integration_args(cfg!(windows))
        );
    }

    #[test]
    fn shell_integration_args_do_not_build_for_windows_powershell() {
        let mut env = HashMap::new();
        env.insert(
            TERMUA_PWSH_INIT_ENV_KEY.to_string(),
            "/tmp/init.ps1".to_string(),
        );

        assert!(shell_integration_args_for_env("powershell", &env).is_empty());
    }

    #[test]
    fn powershell_integration_args_can_enable_execution_policy_bypass() {
        assert_eq!(
            powershell_integration_args(true),
            vec![
                "-NoLogo".to_string(),
                "-NoExit".to_string(),
                "-ExecutionPolicy".to_string(),
                "Bypass".to_string(),
                "-Command".to_string(),
                ". \"$env:TERMUA_PWSH_INIT\"".to_string(),
            ]
        );
    }

    #[test]
    fn platform_shell_candidates_are_ordered_by_preference() {
        let candidates = shell_program_candidates();

        #[cfg(windows)]
        assert_eq!(candidates.first().copied(), Some("pwsh"));

        #[cfg(target_os = "macos")]
        assert_eq!(candidates.first().copied(), Some("zsh"));

        #[cfg(all(not(windows), not(target_os = "macos")))]
        assert_eq!(candidates.first().copied(), Some("bash"));
    }

    #[test]
    fn split_pathext_ignores_empty_segments() {
        assert_eq!(
            split_pathext(".EXE;.CMD;; ;.BAT;"),
            vec![".EXE".to_string(), ".CMD".to_string(), ".BAT".to_string()]
        );
    }
}
