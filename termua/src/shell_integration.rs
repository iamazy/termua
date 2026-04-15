use std::collections::HashMap;

use gpui_term::shell::{
    ShellKind, TERMUA_BASH_RCFILE_ENV_KEY, TERMUA_FISH_INIT_ENV_KEY, TERMUA_NU_CONFIG_ENV_KEY,
    TERMUA_NU_ENV_CONFIG_ENV_KEY, TERMUA_PWSH_INIT_ENV_KEY, TERMUA_SHELL_ENV_KEY,
    pick_shell_program_from_env_or_else, shell_kind,
};

#[cfg(unix)]
const OSC133_BASH: &str = include_str!("../../assets/shell/termua-osc133.bash");
#[cfg(unix)]
const OSC133_ZSH: &str = include_str!("../../assets/shell/termua-osc133.zsh");
const OSC133_FISH: &str = include_str!("../../assets/shell/termua-osc133.fish");
const OSC133_NU: &str = include_str!("../../assets/shell/termua-osc133.nu");
const NU_ENV_CONFIG: &str = include_str!("../../assets/shell/termua-env.nu");
const OSC133_PWSH: &str = include_str!("../../assets/shell/termua-osc133.ps1");

pub(crate) fn maybe_inject_local_shell_osc133(
    env: HashMap<String, String>,
    terminal_id: usize,
) -> HashMap<String, String> {
    let Some(shell_program) = selected_shell_program_for_env(&env) else {
        return env;
    };

    match shell_kind(&shell_program) {
        ShellKind::Bash => maybe_inject_local_bash_osc133(env, terminal_id),
        ShellKind::Zsh => maybe_inject_local_zsh_osc133(env, terminal_id),
        ShellKind::Fish => maybe_inject_local_fish_osc133(env, terminal_id),
        ShellKind::Nu => maybe_inject_local_nu_osc133(env, terminal_id),
        ShellKind::PowerShell => maybe_inject_local_powershell_osc133(env, terminal_id),
        ShellKind::Other => env,
    }
}

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
                    TERMUA_BASH_RCFILE_ENV_KEY.to_string(),
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

pub(crate) fn maybe_inject_local_fish_osc133(
    env: HashMap<String, String>,
    terminal_id: usize,
) -> HashMap<String, String> {
    let mut env = env;
    let Some(shell_program) = selected_shell_program_for_env(&env) else {
        return env;
    };
    if !is_fish_program(&shell_program) {
        return env;
    }

    match write_fish_init(terminal_id) {
        Ok(init_path) => {
            env.insert("SHELL".to_string(), shell_program.clone());
            env.insert(TERMUA_SHELL_ENV_KEY.to_string(), shell_program);
            env.insert(
                TERMUA_FISH_INIT_ENV_KEY.to_string(),
                init_path.to_string_lossy().to_string(),
            );
        }
        Err(err) => {
            log::warn!("termua: failed to inject OSC133 fish integration: {err:#}");
        }
    }

    env
}

pub(crate) fn maybe_inject_local_nu_osc133(
    env: HashMap<String, String>,
    terminal_id: usize,
) -> HashMap<String, String> {
    let mut env = env;
    let Some(shell_program) = selected_shell_program_for_env(&env) else {
        return env;
    };
    if !is_nu_program(&shell_program) {
        return env;
    }

    match write_nu_config_dir(terminal_id) {
        Ok((config_path, env_config_path)) => {
            env.insert("SHELL".to_string(), shell_program.clone());
            env.insert(TERMUA_SHELL_ENV_KEY.to_string(), shell_program);
            env.insert(
                TERMUA_NU_CONFIG_ENV_KEY.to_string(),
                config_path.to_string_lossy().to_string(),
            );
            env.insert(
                TERMUA_NU_ENV_CONFIG_ENV_KEY.to_string(),
                env_config_path.to_string_lossy().to_string(),
            );
        }
        Err(err) => {
            log::warn!("termua: failed to inject OSC133 nushell integration: {err:#}");
        }
    }

    env
}

pub(crate) fn maybe_inject_local_powershell_osc133(
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
                TERMUA_PWSH_INIT_ENV_KEY.to_string(),
                init_path.to_string_lossy().to_string(),
            );
        }
        Err(err) => {
            log::warn!("termua: failed to inject OSC133 powershell integration: {err:#}");
        }
    }

    env
}

fn selected_shell_program_for_env(env: &HashMap<String, String>) -> Option<String> {
    pick_shell_program_from_env_or_else(env, || std::env::var("SHELL").ok())
}

#[cfg(unix)]
fn is_bash_program(program: &str) -> bool {
    matches!(shell_kind(program), ShellKind::Bash)
}

#[cfg(unix)]
fn is_zsh_program(program: &str) -> bool {
    matches!(shell_kind(program), ShellKind::Zsh)
}

fn is_fish_program(program: &str) -> bool {
    matches!(shell_kind(program), ShellKind::Fish)
}

fn is_nu_program(program: &str) -> bool {
    matches!(shell_kind(program), ShellKind::Nu)
}

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

#[cfg(unix)]
fn set_private_dir_permissions(path: &std::path::Path) -> anyhow::Result<()> {
    use std::{fs, os::unix::fs::PermissionsExt as _};
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_private_dir_permissions(_path: &std::path::Path) -> anyhow::Result<()> {
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

#[cfg(unix)]
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

#[cfg(unix)]
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

fn write_fish_init(terminal_id: usize) -> anyhow::Result<std::path::PathBuf> {
    use std::fs;

    let init_path =
        unique_shell_path(&format!("termua-fish-init-{terminal_id}"))?.with_extension("fish");
    fs::write(&init_path, OSC133_FISH)?;
    set_private_file_permissions(&init_path)?;
    Ok(init_path)
}

fn write_powershell_init(terminal_id: usize) -> anyhow::Result<std::path::PathBuf> {
    use std::fs;

    let init_path =
        unique_shell_path(&format!("termua-pwsh-init-{terminal_id}"))?.with_extension("ps1");
    fs::write(&init_path, OSC133_PWSH)?;
    set_private_file_permissions(&init_path)?;
    Ok(init_path)
}

fn render_nu_config(orig_config_path: Option<&std::path::Path>) -> String {
    OSC133_NU.replace(
        "__TERMUA_ORIG_CONFIG__",
        &nushell_source_literal(orig_config_path),
    )
}

fn render_nu_env_config(orig_env_path: Option<&std::path::Path>) -> String {
    NU_ENV_CONFIG.replace(
        "__TERMUA_ORIG_ENV__",
        &nushell_source_literal(orig_env_path),
    )
}

fn nushell_source_literal(path: Option<&std::path::Path>) -> String {
    path.map(|path| format!("{:?}", path.to_string_lossy()))
        .unwrap_or_else(|| "null".to_string())
}

fn existing_nushell_user_config_file(file_name: &str) -> Option<std::path::PathBuf> {
    let base_config_dir = std::env::var_os("XDG_CONFIG_HOME")
        .or_else(|| std::env::var_os("APPDATA"))
        .or_else(|| {
            std::env::var_os("HOME").map(|home| {
                std::path::PathBuf::from(home)
                    .join(".config")
                    .into_os_string()
            })
        })
        .map(std::path::PathBuf::from)?;
    let path = base_config_dir.join("nushell").join(file_name);
    path.exists().then_some(path)
}

fn write_nu_config_dir(
    terminal_id: usize,
) -> anyhow::Result<(std::path::PathBuf, std::path::PathBuf)> {
    use std::fs;

    let config_dir = unique_shell_path(&format!("termua-nu-config-{terminal_id}"))?;
    fs::create_dir_all(&config_dir)?;
    set_private_dir_permissions(&config_dir)?;

    let env_config_path = config_dir.join("env.nu");
    let config_path = config_dir.join("config.nu");

    let orig_env_path = existing_nushell_user_config_file("env.nu");
    let orig_config_path = existing_nushell_user_config_file("config.nu");
    let env_config = render_nu_env_config(orig_env_path.as_deref());
    let config = render_nu_config(orig_config_path.as_deref());

    fs::write(&env_config_path, env_config)?;
    fs::write(&config_path, config)?;
    set_private_file_permissions(&env_config_path)?;
    set_private_file_permissions(&config_path)?;

    Ok((config_path, env_config_path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_bash_program_by_basename() {
        assert!(is_bash_program("bash"));
        assert!(is_bash_program("/bin/bash"));
        assert!(!is_bash_program("zsh"));
    }

    #[test]
    fn detects_zsh_program_by_basename() {
        assert!(is_zsh_program("zsh"));
        assert!(is_zsh_program("/bin/zsh"));
        assert!(!is_zsh_program("bash"));
    }

    #[test]
    fn detects_fish_program_by_basename() {
        assert!(is_fish_program("fish"));
        assert!(is_fish_program("/usr/bin/fish"));
        assert!(!is_fish_program("bash"));
    }

    #[test]
    fn detects_nu_program_by_basename() {
        assert!(is_nu_program("nu"));
        assert!(is_nu_program("/usr/bin/nu"));
        assert!(!is_nu_program("bash"));
    }

    #[test]
    fn detects_pwsh_program_by_basename() {
        assert!(is_pwsh_program("pwsh"));
        assert!(is_pwsh_program("/snap/bin/pwsh"));
        assert!(!is_pwsh_program("powershell"));
        assert!(!is_pwsh_program("bash"));
    }

    #[cfg(unix)]
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

    #[cfg(unix)]
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

    #[cfg(unix)]
    #[test]
    fn fish_injection_writes_init_and_sets_env() {
        let init = write_fish_init(7).expect("write fish init");
        assert!(init.exists(), "fish init should exist");

        let mut env = HashMap::new();
        env.insert("SHELL".to_string(), "fish".to_string());

        let env = maybe_inject_local_shell_osc133(env, 7);
        assert_eq!(env.get("TERMUA_SHELL").map(String::as_str), Some("fish"));
        let init_path = env
            .get("TERMUA_FISH_INIT")
            .expect("expected TERMUA_FISH_INIT to be set");
        assert!(
            std::path::Path::new(init_path).exists(),
            "fish init should exist"
        );
    }

    #[cfg(unix)]
    #[test]
    fn nu_injection_writes_configs_and_sets_env() {
        let (config, env_config) = write_nu_config_dir(7).expect("write nu config dir");
        assert!(config.exists(), "nu config should exist");
        assert!(env_config.exists(), "nu env config should exist");

        let mut env = HashMap::new();
        env.insert("SHELL".to_string(), "nu".to_string());

        let env = maybe_inject_local_shell_osc133(env, 7);
        assert_eq!(env.get("TERMUA_SHELL").map(String::as_str), Some("nu"));
        assert!(
            std::path::Path::new(
                env.get("TERMUA_NU_CONFIG")
                    .expect("expected TERMUA_NU_CONFIG to be set")
            )
            .exists(),
            "nu config should exist"
        );
        assert!(
            std::path::Path::new(
                env.get("TERMUA_NU_ENV_CONFIG")
                    .expect("expected TERMUA_NU_ENV_CONFIG to be set")
            )
            .exists(),
            "nu env config should exist"
        );
    }

    #[test]
    fn renders_nu_config_with_const_source_path_and_non_deprecated_get_flag() {
        let config = render_nu_config(None);

        assert!(config.contains("hooks.pre_prompt"));
        assert!(config.contains("hooks.pre_execution"));
        assert!(config.contains("133;A"));
        assert!(config.contains("133;B"));
        assert!(config.contains("133;C"));
        assert!(config.contains("133;D;($__termua_exit)"));
        assert!(config.contains("const __termua_orig_config = null"));
        assert!(config.contains("source $__termua_orig_config"));
        assert!(config.contains("get -o hooks.pre_prompt"));
        assert!(config.contains("get -o hooks.pre_execution"));
        assert!(!config.contains("let __termua_orig_config"));
        assert!(!config.contains("get -i"));
    }

    #[test]
    fn renders_nu_env_config_with_const_source_env_path() {
        let env_config = render_nu_env_config(None);

        assert!(env_config.contains("const __termua_orig_env = null"));
        assert!(env_config.contains("source-env $__termua_orig_env"));
        assert!(!env_config.contains("let __termua_orig_env"));
    }

    #[test]
    fn powershell_injection_writes_init_and_sets_env() {
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

    #[cfg(unix)]
    #[test]
    fn zsh_osc133_script_avoids_readonly_status_parameter() {
        assert!(
            !OSC133_ZSH.contains("local status="),
            "zsh reserves `status` as a readonly special parameter"
        );
        assert!(OSC133_ZSH.contains("local exit_status=$?"));
    }

    #[cfg(unix)]
    #[test]
    fn osc133_shell_scripts_emit_prompt_markers() {
        for script in [OSC133_BASH, OSC133_ZSH, OSC133_FISH, OSC133_NU, OSC133_PWSH] {
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
