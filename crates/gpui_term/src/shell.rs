use std::collections::HashMap;

pub const SHELL_ENV_KEY: &str = "SHELL";
pub const TERMUA_SHELL_ENV_KEY: &str = "TERMUA_SHELL";
pub const TERMUA_BASH_RCFILE_ENV_KEY: &str = "TERMUA_BASH_RCFILE";
pub const TERMUA_FISH_INIT_ENV_KEY: &str = "TERMUA_FISH_INIT";
pub const TERMUA_NU_CONFIG_ENV_KEY: &str = "TERMUA_NU_CONFIG";
pub const TERMUA_NU_ENV_CONFIG_ENV_KEY: &str = "TERMUA_NU_ENV_CONFIG";
pub const TERMUA_PWSH_INIT_ENV_KEY: &str = "TERMUA_PWSH_INIT";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShellKind {
    Bash,
    Zsh,
    Fish,
    Nu,
    PowerShell,
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
        "fish" => ShellKind::Fish,
        "nu" | "nushell" => ShellKind::Nu,
        "pwsh" | "powershell" => ShellKind::PowerShell,
        _ => ShellKind::Other,
    }
}

pub fn shell_display_name(program: &str) -> String {
    match shell_kind(program) {
        ShellKind::Bash => "bash".to_string(),
        ShellKind::Zsh => "zsh".to_string(),
        ShellKind::Fish => "fish".to_string(),
        ShellKind::Nu => "nushell".to_string(),
        ShellKind::PowerShell => "powershell".to_string(),
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
        ShellKind::Fish => env
            .get(TERMUA_FISH_INIT_ENV_KEY)
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|_init| {
                vec![
                    "--init-command".to_string(),
                    "source \"$TERMUA_FISH_INIT\"".to_string(),
                    "--interactive".to_string(),
                ]
            })
            .unwrap_or_default(),
        ShellKind::Nu => {
            let config = env
                .get(TERMUA_NU_CONFIG_ENV_KEY)
                .map(|s| s.trim())
                .filter(|s| !s.is_empty());
            let env_config = env
                .get(TERMUA_NU_ENV_CONFIG_ENV_KEY)
                .map(|s| s.trim())
                .filter(|s| !s.is_empty());

            match (config, env_config) {
                (Some(config), Some(env_config)) => vec![
                    "--config".to_string(),
                    config.to_string(),
                    "--env-config".to_string(),
                    env_config.to_string(),
                    "--interactive".to_string(),
                ],
                _ => Vec::new(),
            }
        }
        ShellKind::PowerShell => env
            .get(TERMUA_PWSH_INIT_ENV_KEY)
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|_init| powershell_integration_args(cfg!(windows)))
            .unwrap_or_default(),
        ShellKind::Zsh | ShellKind::Other => Vec::new(),
    }
}

fn shell_program_candidates_for_windows() -> &'static [&'static str] {
    &["pwsh", "powershell", "cmd"]
}

pub fn shell_program_candidates() -> &'static [&'static str] {
    if cfg!(windows) {
        // Windows: prefer PowerShell 7+ when available, then Windows PowerShell, then cmd.
        shell_program_candidates_for_windows()
    } else if cfg!(target_os = "macos") {
        // macOS: default user shell is zsh on modern macOS.
        &["zsh", "bash", "fish", "nu", "pwsh", "sh"]
    } else {
        // Linux/*nix: bash is commonly available and expected.
        &["bash", "zsh", "fish", "nu", "pwsh", "sh"]
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

pub fn shell_program_items_for_program_exists(
    mut program_exists: impl FnMut(&str) -> bool,
) -> Vec<String> {
    shell_program_items_for_candidates(
        shell_program_candidates(),
        fallback_shell_program(),
        &mut program_exists,
    )
}

fn shell_program_items_for_candidates(
    candidates: &[&str],
    fallback: &str,
    mut program_exists: impl FnMut(&str) -> bool,
) -> Vec<String> {
    let mut items = Vec::new();

    let pwsh_exists = candidates.contains(&"pwsh") && program_exists("pwsh");
    for candidate in candidates {
        if *candidate == "pwsh" {
            if pwsh_exists {
                items.push((*candidate).to_string());
            }
        } else if *candidate == "powershell" && pwsh_exists {
            continue;
        } else if program_exists(candidate) {
            items.push((*candidate).to_string());
        }
    }

    if items.is_empty() {
        items.push(fallback.to_string());
    }

    items
}

pub fn shell_program_items() -> Vec<String> {
    shell_program_items_for_program_exists(program_exists_on_path)
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
        assert_eq!(shell_kind("fish"), ShellKind::Fish);
        assert_eq!(shell_kind("nu"), ShellKind::Nu);
        assert_eq!(shell_kind("pwsh"), ShellKind::PowerShell);
        assert_eq!(shell_kind("powershell"), ShellKind::PowerShell);
        assert_eq!(shell_kind("unknown"), ShellKind::Other);
    }

    #[test]
    fn shell_display_name_normalizes_supported_shells() {
        assert_eq!(shell_display_name("/bin/bash"), "bash");
        assert_eq!(shell_display_name("nu"), "nushell");
        assert_eq!(shell_display_name("pwsh"), "powershell");
        assert_eq!(shell_display_name("powershell"), "powershell");
        assert_eq!(shell_display_name("/opt/bin/fish"), "fish");
        assert_eq!(shell_display_name("xonsh"), "xonsh");
    }

    #[test]
    fn shell_integration_args_build_for_fish() {
        let mut env = HashMap::new();
        env.insert(
            TERMUA_FISH_INIT_ENV_KEY.to_string(),
            "/tmp/it doesn't matter.fish".to_string(),
        );
        assert_eq!(
            shell_integration_args_for_env("fish", &env),
            vec![
                "--init-command".to_string(),
                "source \"$TERMUA_FISH_INIT\"".to_string(),
                "--interactive".to_string(),
            ]
        );
    }

    #[test]
    fn shell_integration_args_build_for_nu() {
        let mut env = HashMap::new();
        env.insert(
            TERMUA_NU_CONFIG_ENV_KEY.to_string(),
            "/tmp/config.nu".to_string(),
        );
        env.insert(
            TERMUA_NU_ENV_CONFIG_ENV_KEY.to_string(),
            "/tmp/env.nu".to_string(),
        );
        assert_eq!(
            shell_integration_args_for_env("nu", &env),
            vec![
                "--config".to_string(),
                "/tmp/config.nu".to_string(),
                "--env-config".to_string(),
                "/tmp/env.nu".to_string(),
                "--interactive".to_string(),
            ]
        );
    }

    #[test]
    fn shell_integration_args_build_for_powershell() {
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
    fn shell_program_items_for_program_exists_filters_candidates() {
        let candidates = shell_program_candidates();
        let keep_a = candidates.first().copied().unwrap();
        let keep_b = candidates.last().copied().unwrap();

        let items = shell_program_items_for_program_exists(|name| name == keep_a || name == keep_b);
        assert_eq!(items, vec![keep_a.to_string(), keep_b.to_string()]);
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
    fn windows_shell_candidates_prefer_pwsh_over_powershell() {
        assert_eq!(
            shell_program_candidates_for_windows(),
            &["pwsh", "powershell", "cmd"]
        );
    }

    #[test]
    fn windows_shell_program_items_hide_powershell_when_pwsh_exists() {
        let items = shell_program_items_for_candidates(
            shell_program_candidates_for_windows(),
            fallback_shell_program(),
            |name| matches!(name, "pwsh" | "powershell" | "cmd"),
        );

        assert_eq!(items, vec!["pwsh".to_string(), "cmd".to_string()]);
    }

    #[test]
    fn windows_shell_program_items_use_powershell_when_pwsh_is_missing() {
        let items = shell_program_items_for_candidates(
            shell_program_candidates_for_windows(),
            fallback_shell_program(),
            |name| matches!(name, "powershell" | "cmd"),
        );

        assert_eq!(items, vec!["powershell".to_string(), "cmd".to_string()]);
    }

    #[test]
    fn split_pathext_ignores_empty_segments() {
        assert_eq!(
            split_pathext(".EXE;.CMD;; ;.BAT;"),
            vec![".EXE".to_string(), ".CMD".to_string(), ".BAT".to_string()]
        );
    }
}
