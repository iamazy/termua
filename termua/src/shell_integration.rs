use std::collections::HashMap;

use gpui_term::shell::{
    ShellKind, TERMUA_SHELL_ENV_KEY, pick_shell_program_from_env_or_else, shell_kind,
};

#[cfg(target_os = "linux")]
const OSC133_BASH: &str = include_str!("../../assets/shell/termua-osc133.bash");
#[cfg(target_os = "macos")]
const OSC133_ZSH: &str = include_str!("../../assets/shell/termua-osc133.zsh");
#[cfg(any(windows, test))]
const OSC133_PWSH: &str = include_str!("../../assets/shell/termua-osc133.ps1");

pub(crate) fn maybe_inject_local_shell_osc133(
    env: HashMap<String, String>,
    terminal_id: usize,
) -> HashMap<String, String> {
    let Some(shell_program) = selected_shell_program_for_env(&env) else {
        return env;
    };

    match shell_kind(&shell_program) {
        #[cfg(target_os = "linux")]
        ShellKind::Bash => maybe_inject_local_bash_osc133(env, terminal_id),
        #[cfg(target_os = "macos")]
        ShellKind::Zsh => maybe_inject_local_zsh_osc133(env, terminal_id),
        #[cfg(windows)]
        ShellKind::Pwsh => maybe_inject_local_pwsh_osc133(env, terminal_id),
        ShellKind::Other => env,
        _ => env,
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn maybe_inject_local_bash_osc133(
    env: HashMap<String, String>,
    terminal_id: usize,
) -> HashMap<String, String> {
    #[cfg(not(unix))]
    {
        let _ = terminal_id;
        return env;
    }

    #[cfg(unix)]
    {
        let mut env = env;
        let Some(shell_program) = selected_shell_program_for_env(&env) else {
            return env;
        };
        if !is_bash_program(&shell_program) {
            return env;
        }

        match write_bash_rcfile(terminal_id) {
            Ok(rcfile_path) => {
                env.insert("SHELL".to_string(), shell_program.clone());
                env.insert(TERMUA_SHELL_ENV_KEY.to_string(), shell_program);
                env.insert(
                    gpui_term::shell::TERMUA_BASH_RCFILE_ENV_KEY.to_string(),
                    rcfile_path.to_string_lossy().to_string(),
                );
            }
            Err(err) => {
                log::warn!("termua: failed to inject OSC133 bash integration: {err:#}");
            }
        }

        env
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn maybe_inject_local_zsh_osc133(
    env: HashMap<String, String>,
    terminal_id: usize,
) -> HashMap<String, String> {
    #[cfg(not(unix))]
    {
        let _ = terminal_id;
        return env;
    }

    #[cfg(unix)]
    {
        let mut env = env;
        let Some(shell_program) = selected_shell_program_for_env(&env) else {
            return env;
        };
        if !is_zsh_program(&shell_program) {
            return env;
        }

        let orig_zdotdir = env
            .get("ZDOTDIR")
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(ToString::to_string);

        match write_zsh_dotdir(terminal_id) {
            Ok(zdotdir) => {
                env.insert("SHELL".to_string(), shell_program.clone());
                env.insert(TERMUA_SHELL_ENV_KEY.to_string(), shell_program);
                if let Some(orig) = orig_zdotdir {
                    env.insert("TERMUA_ORIG_ZDOTDIR".to_string(), orig);
                }
                env.insert("ZDOTDIR".to_string(), zdotdir.to_string_lossy().to_string());
            }
            Err(err) => {
                log::warn!("termua: failed to inject OSC133 zsh integration: {err:#}");
            }
        }

        env
    }
}

#[cfg(windows)]
pub(crate) fn maybe_inject_local_pwsh_osc133(
    env: HashMap<String, String>,
    terminal_id: usize,
) -> HashMap<String, String> {
    let mut env = env;
    let Some(shell_program) = selected_shell_program_for_env(&env) else {
        return env;
    };
    if !is_pwsh_program(&shell_program) {
        return env;
    }

    match write_powershell_init(terminal_id) {
        Ok(init_path) => {
            env.insert("SHELL".to_string(), shell_program.clone());
            env.insert(TERMUA_SHELL_ENV_KEY.to_string(), shell_program);
            env.insert(
                gpui_term::shell::TERMUA_PWSH_INIT_ENV_KEY.to_string(),
                init_path.to_string_lossy().to_string(),
            );
        }
        Err(err) => {
            log::warn!("termua: failed to inject OSC133 pwsh integration: {err:#}");
        }
    }

    env
}

fn selected_shell_program_for_env(env: &HashMap<String, String>) -> Option<String> {
    pick_shell_program_from_env_or_else(env, || std::env::var("SHELL").ok())
}

#[cfg(target_os = "linux")]
fn is_bash_program(program: &str) -> bool {
    matches!(shell_kind(program), ShellKind::Bash)
}

#[cfg(target_os = "macos")]
fn is_zsh_program(program: &str) -> bool {
    matches!(shell_kind(program), ShellKind::Zsh)
}

#[cfg(any(windows, test))]
fn is_pwsh_program(program: &str) -> bool {
    let program = program.trim();
    if program.is_empty() {
        return false;
    }

    std::path::Path::new(program)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(program)
        == "pwsh"
}

fn shell_runtime_dir() -> anyhow::Result<std::path::PathBuf> {
    use std::{fs, path::PathBuf};

    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
    let mut dir = runtime_dir
        .unwrap_or_else(std::env::temp_dir)
        .join("termua-shell");

    let ensure_dir_writable = |dir: &PathBuf| -> bool {
        if fs::create_dir_all(dir).is_err() {
            return false;
        }

        let probe = dir.join(".termua-write-probe");
        match fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&probe)
        {
            Ok(_) => {
                let _ = fs::remove_file(&probe);
                true
            }
            Err(_) => false,
        }
    };

    if !ensure_dir_writable(&dir) {
        let fallback = std::env::temp_dir().join("termua-shell");
        fs::create_dir_all(&fallback)?;
        dir = fallback;
    }

    Ok(dir)
}

#[cfg(unix)]
fn set_private_file_permissions(path: &std::path::Path) -> anyhow::Result<()> {
    use std::{fs, os::unix::fs::PermissionsExt as _};
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_private_file_permissions(_path: &std::path::Path) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(target_os = "macos")]
fn set_private_dir_permissions(path: &std::path::Path) -> anyhow::Result<()> {
    use std::{fs, os::unix::fs::PermissionsExt as _};
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

fn unique_shell_path(name: &str) -> anyhow::Result<std::path::PathBuf> {
    use std::time::{SystemTime, UNIX_EPOCH};

    let dir = shell_runtime_dir()?;
    let pid = std::process::id();
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    Ok(dir.join(format!("{name}-{pid}-{ts}")))
}

#[cfg(target_os = "linux")]
fn write_bash_rcfile(terminal_id: usize) -> anyhow::Result<std::path::PathBuf> {
    use std::fs;

    let rc_path =
        unique_shell_path(&format!("termua-bash-rc-{terminal_id}"))?.with_extension("bashrc");

    let mut rc = String::new();
    rc.push_str("if [ -f /etc/bash.bashrc ]; then . /etc/bash.bashrc; fi\n");
    rc.push_str("if [ -f /etc/bashrc ]; then . /etc/bashrc; fi\n");
    rc.push_str("if [ -f \"$HOME/.bashrc\" ]; then . \"$HOME/.bashrc\"; fi\n");
    rc.push_str("\n# --- termua osc133 integration ---\n");
    rc.push_str(OSC133_BASH);
    rc.push('\n');

    fs::write(&rc_path, rc)?;
    set_private_file_permissions(&rc_path)?;
    Ok(rc_path)
}

#[cfg(target_os = "macos")]
fn write_zsh_dotdir(terminal_id: usize) -> anyhow::Result<std::path::PathBuf> {
    use std::fs;

    let zdotdir = unique_shell_path(&format!("termua-zsh-dotdir-{terminal_id}"))?;
    fs::create_dir_all(&zdotdir)?;
    set_private_dir_permissions(&zdotdir)?;

    // Important: When we set `ZDOTDIR`, zsh will look for `.zshenv` and `.zshrc` *only* in that
    // directory (not `$HOME`), so we must source the user's real dotfiles best-effort.
    let mut zshenv = String::new();
    zshenv.push_str("# termua injected .zshenv\n");
    zshenv.push_str(
        "if [ -n \"${TERMUA_ORIG_ZDOTDIR-}\" ] && [ -f \"$TERMUA_ORIG_ZDOTDIR/.zshenv\" ]; then . \
         \"$TERMUA_ORIG_ZDOTDIR/.zshenv\"; fi\n",
    );
    zshenv.push_str(
        "if [ -z \"${TERMUA_ORIG_ZDOTDIR-}\" ] && [ -f \"$HOME/.zshenv\" ]; then . \
         \"$HOME/.zshenv\"; fi\n",
    );

    let mut zshrc = String::new();
    zshrc.push_str("# termua injected .zshrc\n");
    zshrc.push_str("if [ -f /etc/zsh/zshrc ]; then . /etc/zsh/zshrc; fi\n");
    zshrc.push_str("if [ -f /etc/zshrc ]; then . /etc/zshrc; fi\n");
    zshrc.push_str(
        "if [ -n \"${TERMUA_ORIG_ZDOTDIR-}\" ] && [ -f \"$TERMUA_ORIG_ZDOTDIR/.zshrc\" ]; then . \
         \"$TERMUA_ORIG_ZDOTDIR/.zshrc\"; fi\n",
    );
    zshrc.push_str(
        "if [ -z \"${TERMUA_ORIG_ZDOTDIR-}\" ] && [ -f \"$HOME/.zshrc\" ]; then . \
         \"$HOME/.zshrc\"; fi\n",
    );
    zshrc.push_str("\n# --- termua osc133 integration ---\n");
    zshrc.push_str(OSC133_ZSH);
    zshrc.push('\n');

    let zshenv_path = zdotdir.join(".zshenv");
    let zshrc_path = zdotdir.join(".zshrc");
    fs::write(&zshenv_path, zshenv)?;
    fs::write(&zshrc_path, zshrc)?;
    set_private_file_permissions(&zshenv_path)?;
    set_private_file_permissions(&zshrc_path)?;

    Ok(zdotdir)
}

#[cfg(windows)]
fn write_powershell_init(terminal_id: usize) -> anyhow::Result<std::path::PathBuf> {
    use std::fs;

    let init_path =
        unique_shell_path(&format!("termua-pwsh-init-{terminal_id}"))?.with_extension("ps1");
    fs::write(&init_path, OSC133_PWSH)?;
    set_private_file_permissions(&init_path)?;
    Ok(init_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn detects_bash_program_by_basename() {
        assert!(is_bash_program("bash"));
        assert!(is_bash_program("/bin/bash"));
        assert!(!is_bash_program("zsh"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn detects_zsh_program_by_basename() {
        assert!(is_zsh_program("zsh"));
        assert!(is_zsh_program("/bin/zsh"));
        assert!(!is_zsh_program("bash"));
    }

    #[test]
    fn detects_pwsh_program_by_basename() {
        assert!(is_pwsh_program("pwsh"));
        assert!(is_pwsh_program("/snap/bin/pwsh"));
        assert!(!is_pwsh_program("powershell"));
        assert!(!is_pwsh_program("bash"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn injection_writes_rcfile_and_sets_env() {
        // First, ensure the underlying filesystem write succeeds (helps provide
        // a useful failure message if the environment is restricted).
        let rcfile = write_bash_rcfile(7).expect("write rcfile");
        assert!(rcfile.exists(), "rcfile should exist");

        let mut env = HashMap::new();
        env.insert("SHELL".to_string(), "bash".to_string());

        let env = maybe_inject_local_bash_osc133(env, 7);
        assert_eq!(env.get("TERMUA_SHELL").map(String::as_str), Some("bash"));
        let rcfile_path = env
            .get("TERMUA_BASH_RCFILE")
            .expect("expected TERMUA_BASH_RCFILE to be set");
        assert!(
            std::path::Path::new(rcfile_path).exists(),
            "rcfile should exist"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn zsh_injection_writes_dotdir_and_sets_env() {
        let dotdir = write_zsh_dotdir(7).expect("write dotdir");
        assert!(dotdir.exists(), "dotdir should exist");
        assert!(dotdir.join(".zshrc").exists(), ".zshrc should exist");

        let mut env = HashMap::new();
        env.insert("SHELL".to_string(), "zsh".to_string());

        let env = maybe_inject_local_zsh_osc133(env, 7);
        assert_eq!(env.get("TERMUA_SHELL").map(String::as_str), Some("zsh"));
        let zdotdir = env.get("ZDOTDIR").expect("expected ZDOTDIR to be set");
        assert!(
            std::path::Path::new(zdotdir).join(".zshrc").exists(),
            ".zshrc should exist"
        );
    }

    #[cfg(windows)]
    #[test]
    fn pwsh_injection_writes_init_and_sets_env() {
        let init = write_powershell_init(7).expect("write powershell init");
        assert!(init.exists(), "powershell init should exist");

        let mut env = HashMap::new();
        env.insert("SHELL".to_string(), "pwsh".to_string());

        let env = maybe_inject_local_shell_osc133(env, 7);
        assert_eq!(env.get("TERMUA_SHELL").map(String::as_str), Some("pwsh"));
        let init_path = env
            .get("TERMUA_PWSH_INIT")
            .expect("expected TERMUA_PWSH_INIT to be set");
        assert!(
            std::path::Path::new(init_path).exists(),
            "powershell init should exist"
        );
    }

    #[test]
    fn windows_powershell_does_not_inject_pwsh_init() {
        let mut env = HashMap::new();
        env.insert("SHELL".to_string(), "powershell".to_string());

        let env = maybe_inject_local_shell_osc133(env, 7);
        assert_eq!(env.get("TERMUA_SHELL").map(String::as_str), None);
        assert_eq!(env.get("TERMUA_PWSH_INIT").map(String::as_str), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_only_integrates_bash() {
        let mut bash_env = HashMap::new();
        bash_env.insert("SHELL".to_string(), "bash".to_string());
        let bash_env = maybe_inject_local_shell_osc133(bash_env, 7);
        assert_eq!(
            bash_env.get("TERMUA_SHELL").map(String::as_str),
            Some("bash")
        );
        assert!(bash_env.contains_key("TERMUA_BASH_RCFILE"));

        let mut zsh_env = HashMap::new();
        zsh_env.insert("SHELL".to_string(), "zsh".to_string());
        let zsh_env = maybe_inject_local_shell_osc133(zsh_env, 7);
        assert_eq!(zsh_env.get("TERMUA_SHELL").map(String::as_str), None);
        assert_eq!(zsh_env.get("ZDOTDIR").map(String::as_str), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_only_integrates_zsh() {
        let mut zsh_env = HashMap::new();
        zsh_env.insert("SHELL".to_string(), "zsh".to_string());
        let zsh_env = maybe_inject_local_shell_osc133(zsh_env, 7);
        assert_eq!(zsh_env.get("TERMUA_SHELL").map(String::as_str), Some("zsh"));
        assert!(zsh_env.contains_key("ZDOTDIR"));

        let mut bash_env = HashMap::new();
        bash_env.insert("SHELL".to_string(), "bash".to_string());
        let bash_env = maybe_inject_local_shell_osc133(bash_env, 7);
        assert_eq!(bash_env.get("TERMUA_SHELL").map(String::as_str), None);
        assert_eq!(bash_env.get("TERMUA_BASH_RCFILE").map(String::as_str), None);
    }

    #[cfg(windows)]
    #[test]
    fn windows_only_integrates_pwsh() {
        let mut pwsh_env = HashMap::new();
        pwsh_env.insert("SHELL".to_string(), "pwsh".to_string());
        let pwsh_env = maybe_inject_local_shell_osc133(pwsh_env, 7);
        assert_eq!(
            pwsh_env.get("TERMUA_SHELL").map(String::as_str),
            Some("pwsh")
        );
        assert!(pwsh_env.contains_key("TERMUA_PWSH_INIT"));

        let mut powershell_env = HashMap::new();
        powershell_env.insert("SHELL".to_string(), "powershell".to_string());
        let powershell_env = maybe_inject_local_shell_osc133(powershell_env, 7);
        assert_eq!(powershell_env.get("TERMUA_SHELL").map(String::as_str), None);
        assert_eq!(
            powershell_env.get("TERMUA_PWSH_INIT").map(String::as_str),
            None
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn zsh_osc133_script_avoids_readonly_status_parameter() {
        assert!(
            !OSC133_ZSH.contains("local status="),
            "zsh reserves `status` as a readonly special parameter"
        );
        assert!(OSC133_ZSH.contains("local exit_status=$?"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn osc133_shell_scripts_emit_prompt_markers() {
        for script in [OSC133_BASH, OSC133_PWSH] {
            assert!(
                script.contains("133;A") || script.contains("\"A\""),
                "expected script to emit prompt start marker"
            );
            assert!(
                script.contains("133;B") || script.contains("\"B\""),
                "expected script to emit prompt end marker"
            );
            assert!(
                script.contains("133;C") || script.contains("\"C\""),
                "expected script to emit command start marker"
            );
            assert!(
                script.contains("133;D") || script.contains("\"D;"),
                "expected script to emit command end marker"
            );
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn osc133_shell_scripts_emit_prompt_markers() {
        for script in [OSC133_ZSH, OSC133_PWSH] {
            assert!(
                script.contains("133;A") || script.contains("\"A\""),
                "expected script to emit prompt start marker"
            );
            assert!(
                script.contains("133;B") || script.contains("\"B\""),
                "expected script to emit prompt end marker"
            );
            assert!(
                script.contains("133;C") || script.contains("\"C\""),
                "expected script to emit command start marker"
            );
            assert!(
                script.contains("133;D") || script.contains("\"D;"),
                "expected script to emit command end marker"
            );
        }
    }
}
