use std::collections::HashMap;

pub const SHELL_ENV_KEY: &str = "SHELL";
pub const TERMUA_SHELL_ENV_KEY: &str = "TERMUA_SHELL";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShellKind {
    Bash,
    Zsh,
    Fish,
    Nu,
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
    let name = normalized_shell_program_name(program);
    if name.is_empty() {
        return ShellKind::Other;
    }

    match name.as_str() {
        "bash" => ShellKind::Bash,
        "zsh" => ShellKind::Zsh,
        "fish" => ShellKind::Fish,
        "nu" => ShellKind::Nu,
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
        ShellKind::Fish => "fish".to_string(),
        ShellKind::Nu => "nushell".to_string(),
        ShellKind::Pwsh | ShellKind::PowerShell => "powershell".to_string(),
        ShellKind::Cmd => "cmd".to_string(),
        ShellKind::Other => std::path::Path::new(program.trim())
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(program.trim())
            .to_string(),
    }
}

pub fn shell_program_candidates() -> &'static [&'static str] {
    if cfg!(windows) {
        // Windows: prefer PowerShell 7+, Windows PowerShell, and cmd; also detect NuShell.
        &["pwsh", "powershell", "cmd", "nu"]
    } else if cfg!(target_os = "macos") {
        // macOS: default user shell is zsh on modern macOS.
        &["zsh", "bash", "fish", "nu", "pwsh", "powershell"]
    } else {
        // Linux/*nix: bash is commonly available and expected.
        &["bash", "zsh", "fish", "nu", "pwsh", "powershell"]
    }
}

fn detect_shell_programs(
    process_shell: Option<&str>,
    exists: impl Fn(&str) -> bool,
) -> Vec<String> {
    detect_shell_programs_from_candidates(process_shell, shell_program_candidates(), exists)
}

fn normalized_shell_program_name(program: &str) -> String {
    let name = program
        .trim()
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default();
    let name = name.to_ascii_lowercase();
    name.strip_suffix(".exe").unwrap_or(&name).to_string()
}

fn detect_shell_programs_from_candidates(
    process_shell: Option<&str>,
    candidates: &[&str],
    exists: impl Fn(&str) -> bool,
) -> Vec<String> {
    let process_shell = process_shell
        .map(str::trim)
        .filter(|shell| !shell.is_empty());
    let process_shell_name = process_shell.map(normalized_shell_program_name);
    let process_shell_available = process_shell.is_some_and(&exists);
    let candidate_states = candidates
        .iter()
        .map(|candidate| {
            let normalized_name = normalized_shell_program_name(candidate);
            let duplicates_process_shell = process_shell_available
                && process_shell_name.as_deref() == Some(normalized_name.as_str());
            let available = !duplicates_process_shell && exists(candidate);
            (*candidate, normalized_name, available)
        })
        .collect::<Vec<_>>();
    let pwsh_available = (process_shell_available && process_shell_name.as_deref() == Some("pwsh"))
        || candidate_states
            .iter()
            .any(|(_, name, available)| *available && name == "pwsh");

    let mut programs = Vec::new();
    if let Some(shell) = process_shell.filter(|_| {
        process_shell_available
            && !(pwsh_available && process_shell_name.as_deref() == Some("powershell"))
    }) {
        programs.push(shell.to_string());
    }

    for (candidate, candidate_name, available) in candidate_states {
        if (process_shell_available
            && process_shell_name.as_deref() == Some(candidate_name.as_str()))
            || (pwsh_available && candidate_name == "powershell")
            || !available
        {
            continue;
        }
        programs.push(candidate.to_string());
    }

    if programs.is_empty() {
        programs.push(default_shell_program().to_string());
    }
    programs
}

pub fn available_shell_programs() -> Vec<String> {
    let process_shell = std::env::var(SHELL_ENV_KEY).ok();
    detect_shell_programs(process_shell.as_deref(), program_exists_on_path)
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
        assert_eq!(shell_kind("pwsh"), ShellKind::Pwsh);
        assert_eq!(shell_kind("powershell"), ShellKind::PowerShell);
        assert_eq!(shell_kind("cmd"), ShellKind::Cmd);
        assert_eq!(shell_kind("unknown"), ShellKind::Other);
    }

    #[test]
    fn shell_kind_normalizes_executable_suffix_case_and_windows_paths() {
        assert_eq!(shell_kind("/opt/nushell/bin/nu.exe"), ShellKind::Nu);
        assert_eq!(shell_kind("PWSh.EXE"), ShellKind::Pwsh);
        assert_eq!(
            shell_kind(r"C:\Program Files\PowerShell\7\pwsh.exe"),
            ShellKind::Pwsh
        );
    }

    #[test]
    fn shell_display_name_normalizes_supported_shells() {
        assert_eq!(shell_display_name("/bin/bash"), "bash");
        assert_eq!(shell_display_name("fish"), "fish");
        assert_eq!(shell_display_name("nu"), "nushell");
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
    fn platform_shell_candidates_are_ordered_by_preference() {
        let candidates = shell_program_candidates();

        assert!(candidates.contains(&"nu"));
        assert!(candidates.contains(&"pwsh"));
        assert!(candidates.contains(&"powershell"));

        #[cfg(windows)]
        assert_eq!(candidates.first().copied(), Some("pwsh"));

        #[cfg(target_os = "macos")]
        assert_eq!(candidates.first().copied(), Some("zsh"));

        #[cfg(all(not(windows), not(target_os = "macos")))]
        assert_eq!(candidates.first().copied(), Some("bash"));
    }

    #[test]
    fn detected_shell_programs_filters_unavailable_candidates() {
        let detected =
            detect_shell_programs_from_candidates(None, &["zsh", "bash", "nu"], |program| {
                matches!(program, "zsh" | "nu")
            });

        assert_eq!(detected, vec!["zsh".to_string(), "nu".to_string()]);
    }

    #[test]
    fn detected_shell_programs_prefers_executable_process_shell() {
        let detected = detect_shell_programs(Some("/opt/homebrew/bin/fish"), |program| {
            matches!(program, "/opt/homebrew/bin/fish" | "bash" | "fish")
        });

        assert_eq!(
            detected.first().map(String::as_str),
            Some("/opt/homebrew/bin/fish")
        );
        assert_eq!(
            detected
                .iter()
                .filter(|program| program.ends_with("fish"))
                .count(),
            1
        );
    }

    #[test]
    fn detected_shell_programs_prefers_power_shell_7_when_both_versions_exist() {
        let detected =
            detect_shell_programs_from_candidates(None, &["pwsh", "powershell", "nu"], |_| true);

        assert_eq!(detected, vec!["pwsh".to_string(), "nu".to_string()]);
    }

    #[test]
    fn detected_shell_programs_does_not_probe_a_candidate_more_than_once() {
        use std::{cell::RefCell, collections::HashMap};

        let probe_counts = RefCell::new(HashMap::<String, usize>::new());
        let detected =
            detect_shell_programs_from_candidates(None, &["pwsh", "powershell", "nu"], |program| {
                *probe_counts
                    .borrow_mut()
                    .entry(program.to_string())
                    .or_default() += 1;
                true
            });

        assert_eq!(detected, vec!["pwsh".to_string(), "nu".to_string()]);
        assert!(
            probe_counts.borrow().values().all(|count| *count == 1),
            "each PATH candidate should be probed at most once: {:?}",
            probe_counts.borrow()
        );
    }

    #[test]
    fn detected_shell_programs_keeps_power_shell_5_when_version_7_is_missing() {
        let detected =
            detect_shell_programs_from_candidates(None, &["pwsh", "powershell"], |program| {
                program == "powershell"
            });

        assert_eq!(detected, vec!["powershell".to_string()]);
    }

    #[test]
    fn detected_shell_programs_drops_power_shell_5_process_shell_when_version_7_exists() {
        let detected = detect_shell_programs_from_candidates(
            Some("/opt/microsoft/powershell"),
            &["pwsh", "powershell"],
            |program| matches!(program, "/opt/microsoft/powershell" | "pwsh" | "powershell"),
        );

        assert_eq!(detected, vec!["pwsh".to_string()]);
    }

    #[test]
    fn detected_shell_programs_falls_back_when_none_are_available() {
        assert_eq!(
            detect_shell_programs(Some("/missing/shell"), |_| false),
            vec![default_shell_program().to_string()]
        );
    }

    #[test]
    fn split_pathext_ignores_empty_segments() {
        assert_eq!(
            split_pathext(".EXE;.CMD;; ;.BAT;"),
            vec![".EXE".to_string(), ".CMD".to_string(), ".BAT".to_string()]
        );
    }
}
