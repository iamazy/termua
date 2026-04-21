use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use crate::store::SessionEnvVar;

pub(crate) const CAST_PLAYER_ENV_MODE: &str = "TERMUA_CAST_PLAYER";
pub(crate) const CAST_PLAYER_ENV_PATH: &str = "TERMUA_CAST_PLAYER_PATH";
pub(crate) const CAST_PLAYER_ENV_SPEED: &str = "TERMUA_CAST_PLAYER_SPEED";

pub(crate) fn cast_player_child_env(
    cast_path: &Path,
    playback_speed: f64,
) -> HashMap<String, String> {
    let mut env = HashMap::new();
    env.insert(CAST_PLAYER_ENV_MODE.to_string(), "1".to_string());
    env.insert(
        CAST_PLAYER_ENV_PATH.to_string(),
        cast_path.to_string_lossy().to_string(),
    );
    if playback_speed.is_finite() && playback_speed > 0.0 {
        env.insert(
            CAST_PLAYER_ENV_SPEED.to_string(),
            playback_speed.to_string(),
        );
    }
    let termua_path = termua_executable_path().unwrap_or_else(|| PathBuf::from("termua"));
    env.insert(
        "TERMUA_SHELL".to_string(),
        termua_path.to_string_lossy().to_string(),
    );
    env
}

pub(crate) fn build_terminal_env(
    shell_program: &str,
    term: &str,
    colorterm: Option<&str>,
    charset: &str,
    env_vars: &[SessionEnvVar],
) -> HashMap<String, String> {
    let mut env = HashMap::new();

    let shell_program = shell_program.trim();
    if !shell_program.is_empty() {
        env.insert("SHELL".to_string(), shell_program.to_string());
        // gpui_term's WezTerm backend uses `TERMUA_SHELL` to pick the program to spawn.
        env.insert("TERMUA_SHELL".to_string(), shell_program.to_string());
    }

    let term = term.trim();
    if !term.is_empty() {
        env.insert("TERM".to_string(), term.to_string());
    }

    if let Some(colorterm) = colorterm.map(str::trim).filter(|value| !value.is_empty()) {
        env.insert("COLORTERM".to_string(), colorterm.to_string());
    }

    let charset = charset.trim().to_ascii_uppercase();
    if charset.contains("UTF-8") || charset.contains("UTF8") {
        env.insert("LANG".to_string(), "en_US.UTF-8".to_string());
        env.insert("LC_CTYPE".to_string(), "en_US.UTF-8".to_string());
    } else if charset.contains("ASCII") {
        env.insert("LANG".to_string(), "C".to_string());
        env.insert("LC_CTYPE".to_string(), "C".to_string());
    }

    for var in env_vars {
        let name = var.name.trim();
        if name.is_empty() {
            continue;
        }
        env.insert(name.to_string(), var.value.clone());
    }

    env
}

fn termua_executable_path() -> Option<PathBuf> {
    std::env::current_exe().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cast_player_env_sets_mode_and_path() {
        let env = cast_player_child_env(Path::new("/tmp/demo.cast"), 1.5);
        assert_eq!(env.get(CAST_PLAYER_ENV_MODE).map(String::as_str), Some("1"));
        assert_eq!(
            env.get(CAST_PLAYER_ENV_PATH).map(String::as_str),
            Some("/tmp/demo.cast")
        );
        assert_eq!(
            env.get(CAST_PLAYER_ENV_SPEED).map(String::as_str),
            Some("1.5")
        );
        assert!(
            env.get("TERMUA_SHELL")
                .is_some_and(|v| !v.trim().is_empty()),
            "cast player terminal env should include TERMUA_SHELL to select the program to spawn"
        );
    }

    #[test]
    fn build_terminal_env_merges_colorterm_and_explicit_env_overrides() {
        let env = build_terminal_env(
            "/bin/zsh",
            "xterm-256color",
            Some("truecolor"),
            "UTF-8",
            &[
                SessionEnvVar {
                    name: "TERM".to_string(),
                    value: "screen-256color".to_string(),
                },
                SessionEnvVar {
                    name: "COLORTERM".to_string(),
                    value: "24bit".to_string(),
                },
                SessionEnvVar {
                    name: "CUSTOM_FLAG".to_string(),
                    value: "1".to_string(),
                },
            ],
        );

        assert_eq!(env.get("SHELL"), Some(&"/bin/zsh".to_string()));
        assert_eq!(env.get("TERMUA_SHELL"), Some(&"/bin/zsh".to_string()));
        assert_eq!(env.get("TERM"), Some(&"screen-256color".to_string()));
        assert_eq!(env.get("COLORTERM"), Some(&"24bit".to_string()));
        assert_eq!(env.get("CUSTOM_FLAG"), Some(&"1".to_string()));
        assert_eq!(env.get("LANG"), Some(&"en_US.UTF-8".to_string()));
        assert_eq!(env.get("LC_CTYPE"), Some(&"en_US.UTF-8".to_string()));
    }
}
