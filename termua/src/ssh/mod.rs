use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
};

use gpui::SharedString;
use gpui_term::{Authentication, SshOptions, TerminalBuilder, TerminalType};

pub(crate) type SshTerminalBuilderFn = Arc<
    dyn Fn(TerminalType, HashMap<String, String>, SshOptions) -> anyhow::Result<TerminalBuilder>
        + Send
        + Sync,
>;

pub(crate) fn ssh_proxy_from_session(session: &crate::store::Session) -> gpui_term::SshProxyMode {
    use crate::store::SshProxyMode as StoreMode;

    let mode = session.ssh_proxy_mode.unwrap_or(StoreMode::Disabled);
    match mode {
        StoreMode::Inherit => gpui_term::SshProxyMode::Inherit,
        StoreMode::Disabled => gpui_term::SshProxyMode::Disabled,
        StoreMode::Command => {
            let command = session.ssh_proxy_command.clone().unwrap_or_default();
            let working_dir = session
                .ssh_proxy_workdir
                .as_deref()
                .map(expand_home_dir_placeholders);

            let mut env = HashMap::new();
            if let Some(vars) = session.ssh_proxy_env.as_deref() {
                for var in vars {
                    env.insert(
                        var.name.clone(),
                        expand_home_dir_placeholders(var.value.as_str()),
                    );
                }
            }

            gpui_term::SshProxyMode::Command(gpui_term::SshProxyCommand {
                command,
                working_dir,
                env,
            })
        }
        StoreMode::JumpServer => {
            let working_dir = session
                .ssh_proxy_workdir
                .as_deref()
                .map(expand_home_dir_placeholders);

            let mut env = HashMap::new();
            if let Some(vars) = session.ssh_proxy_env.as_deref() {
                for var in vars {
                    env.insert(
                        var.name.clone(),
                        expand_home_dir_placeholders(var.value.as_str()),
                    );
                }
            }

            let hops = session
                .ssh_proxy_jump
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|hop| gpui_term::SshJumpHop {
                    host: hop.host.clone(),
                    user: hop.user.clone(),
                    port: hop.port,
                })
                .collect();

            gpui_term::SshProxyMode::JumpServer(gpui_term::SshJumpChain {
                hops,
                working_dir,
                env,
            })
        }
    }
}

pub(crate) fn expand_home_dir_placeholders(raw: &str) -> String {
    let mut out = raw.to_string();

    if out.contains("${HomeDir}") {
        if let Some(home) = home::home_dir() {
            if let Some(home) = home.to_str() {
                out = out.replace("${HomeDir}", home);
            }
        }
    }

    if out == "~" || out.starts_with("~/") {
        if let Some(home) = home::home_dir() {
            if let Some(home) = home.to_str() {
                if out == "~" {
                    out = home.to_string();
                } else {
                    out.replace_range(0..1, home);
                }
            }
        }
    }

    out
}

pub(crate) fn dedupe_tab_label(counts: &mut HashMap<String, usize>, base: &str) -> SharedString {
    let base = base.trim();
    let base = if base.is_empty() { "ssh" } else { base }.to_string();

    let n = counts.entry(base.clone()).or_insert(0);
    *n += 1;

    if *n == 1 {
        base.into()
    } else {
        format!("{} {}", base, *n).into()
    }
}

pub(crate) fn ssh_tab_tooltip(opts: &SshOptions) -> SharedString {
    match &opts.auth {
        Authentication::Password(user, _) => {
            let user = user.trim();
            let user = if user.is_empty() { "root" } else { user };
            format!("{}/{}", user, opts.host.trim()).into()
        }
        Authentication::Config => opts.host.trim().to_string().into(),
    }
}

pub(crate) fn ssh_target_label(opts: &SshOptions) -> String {
    let host = opts.host.trim();
    let port = opts.port.unwrap_or(22);
    match &opts.auth {
        Authentication::Password(user, _) => {
            let user = user.trim();
            let user = if user.is_empty() { "root" } else { user };
            format!("{user}@{host}:{port}")
        }
        Authentication::Config => format!("{host}:{port}"),
    }
}

pub(crate) fn ssh_connect_failure_message(opts: &SshOptions, err: &anyhow::Error) -> String {
    let target = ssh_target_label(opts);
    let reason = {
        let root = err.root_cause().to_string();

        // User-friendly mappings for common SSH failures.
        if root.contains("password auth status: Denied") {
            "Wrong password".to_string()
        } else {
            root
        }
    };

    format!("{target}: {reason}")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SshHostKeyMismatchDetails {
    pub(crate) got_fingerprint: Option<String>,
    pub(crate) known_hosts_path: Option<PathBuf>,
    pub(crate) server_host: Option<String>,
    pub(crate) server_port: Option<u16>,
}

pub(crate) fn parse_ssh_host_key_mismatch(reason: &str) -> Option<SshHostKeyMismatchDetails> {
    if !reason.to_ascii_lowercase().contains("host key mismatch") {
        return None;
    }

    fn between(haystack: &str, start: &str, end: &str) -> Option<String> {
        let s = haystack.split_once(start)?.1;
        let v = s.split_once(end)?.0;
        Some(v.trim().to_string())
    }

    fn parse_server_host_port(server: &str) -> (Option<String>, Option<u16>) {
        let server = server.trim();
        if server.is_empty() {
            return (None, None);
        }

        if let Some((host, port)) = server.rsplit_once(':') {
            if let Ok(port) = port.trim().parse::<u16>() {
                let host = host.trim();
                return ((!host.is_empty()).then(|| host.to_string()), Some(port));
            }
        }

        (Some(server.to_string()), None)
    }

    let (server_host, server_port) = between(reason, "ssh server ", " has changed")
        .or_else(|| between(reason, "ssh server ", ". Got fingerprint"))
        .or_else(|| between(reason, "server ", " has changed"))
        .or_else(|| between(reason, "server ", ". Got fingerprint"))
        .map(|s| parse_server_host_port(s.as_str()))
        .unwrap_or((None, None));

    let got_fingerprint = between(reason, "Got fingerprint ", " instead of");

    let known_hosts_path = between(reason, "Some(\"", "\")").map(PathBuf::from);

    Some(SshHostKeyMismatchDetails {
        got_fingerprint,
        known_hosts_path,
        server_host,
        server_port,
    })
}

pub(crate) fn filter_known_hosts_contents(
    contents: &str,
    host: &str,
    port: u16,
) -> (String, usize) {
    let host = host.trim();
    let mut targets = Vec::with_capacity(2);
    if !host.is_empty() {
        if port == 22 {
            targets.push(host.to_string());
        }
        targets.push(format!("[{host}]:{port}"));
    }

    let mut out = String::with_capacity(contents.len());
    let mut removed = 0usize;

    for line in contents.split_inclusive('\n') {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            out.push_str(line);
            continue;
        }

        let Some(first_field) = trimmed.split_whitespace().next() else {
            out.push_str(line);
            continue;
        };

        let mut should_remove = false;
        for token in first_field.split(',') {
            if targets.iter().any(|t| t == token) {
                should_remove = true;
                break;
            }
        }

        if should_remove {
            removed += 1;
            continue;
        }

        out.push_str(line);
    }

    (out, removed)
}

pub(crate) fn default_known_hosts_path() -> Option<PathBuf> {
    Some(home::home_dir()?.join(".ssh").join("known_hosts"))
}

pub(crate) fn remove_known_host_entry(
    known_hosts_path: &Path,
    host: &str,
    port: u16,
) -> anyhow::Result<String> {
    let host = host.trim();
    if host.is_empty() {
        return Err(anyhow::anyhow!("missing host"));
    }

    let mut targets = Vec::with_capacity(2);
    targets.push(format!("[{host}]:{port}"));
    if port == 22 {
        targets.push(host.to_string());
    }

    // Prefer `ssh-keygen -R` because it can remove hashed entries.
    let mut output_summary = String::new();
    for target in targets.iter() {
        let output = match Command::new("ssh-keygen")
            .arg("-R")
            .arg(target)
            .arg("-f")
            .arg(known_hosts_path)
            .output()
        {
            Ok(output) => output,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                // Fallback: best-effort edit for non-hashed entries.
                let contents = std::fs::read_to_string(known_hosts_path)?;
                let (updated, removed) = filter_known_hosts_contents(&contents, host, port);
                if removed > 0 {
                    crate::atomic_write::write_string(known_hosts_path, &updated)?;
                }
                return Ok(format!(
                    "updated {} (removed {removed} entr{})",
                    known_hosts_path.display(),
                    if removed == 1 { "y" } else { "ies" }
                ));
            }
            Err(err) => return Err(err.into()),
        };

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        if !output.status.success() {
            return Err(anyhow::anyhow!(
                "ssh-keygen -R {target} failed: {}",
                stderr.trim()
            ));
        }

        // `ssh-keygen` is fairly chatty; keep a short summary for notifications.
        if !stdout.trim().is_empty() {
            output_summary.push_str(stdout.trim());
            output_summary.push('\n');
        }
        if !stderr.trim().is_empty() {
            output_summary.push_str(stderr.trim());
            output_summary.push('\n');
        }
    }

    Ok(output_summary.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_tab_label_is_deduped_by_appending_incrementing_numbers() {
        let mut counts = HashMap::new();
        assert_eq!(dedupe_tab_label(&mut counts, "prod").as_ref(), "prod");
        assert_eq!(dedupe_tab_label(&mut counts, "prod").as_ref(), "prod 2");
        assert_eq!(dedupe_tab_label(&mut counts, "prod").as_ref(), "prod 3");
        assert_eq!(dedupe_tab_label(&mut counts, "staging").as_ref(), "staging");
        assert_eq!(
            dedupe_tab_label(&mut counts, "staging").as_ref(),
            "staging 2"
        );
    }

    #[test]
    fn ssh_tab_tooltip_shows_user_and_host_for_password_auth() {
        let opts = SshOptions {
            host: "example.com".to_string(),
            port: Some(22),
            auth: Authentication::Password("alice".to_string(), "pw".to_string()),
            proxy: gpui_term::SshProxyMode::Inherit,
            backend: gpui_term::SshBackend::default(),
            tcp_nodelay: false,
            tcp_keepalive: false,
        };

        assert_eq!(ssh_tab_tooltip(&opts).as_ref(), "alice/example.com");
    }

    #[test]
    fn ssh_tab_tooltip_defaults_user_to_root_when_empty() {
        let opts = SshOptions {
            host: "example.com".to_string(),
            port: Some(22),
            auth: Authentication::Password("".to_string(), "pw".to_string()),
            proxy: gpui_term::SshProxyMode::Inherit,
            backend: gpui_term::SshBackend::default(),
            tcp_nodelay: false,
            tcp_keepalive: false,
        };

        assert_eq!(ssh_tab_tooltip(&opts).as_ref(), "root/example.com");
    }

    #[test]
    fn ssh_tab_tooltip_shows_host_only_for_config_auth() {
        let opts = SshOptions {
            host: "example.com".to_string(),
            port: Some(22),
            auth: Authentication::Config,
            proxy: gpui_term::SshProxyMode::Inherit,
            backend: gpui_term::SshBackend::default(),
            tcp_nodelay: false,
            tcp_keepalive: false,
        };

        assert_eq!(ssh_tab_tooltip(&opts).as_ref(), "example.com");
    }

    #[test]
    fn ssh_connect_failure_message_mentions_wrong_password_when_denied() {
        let opts = SshOptions {
            host: "example.com".to_string(),
            port: Some(22),
            auth: Authentication::Password("alice".to_string(), "super-secret".to_string()),
            proxy: gpui_term::SshProxyMode::Inherit,
            backend: gpui_term::SshBackend::default(),
            tcp_nodelay: false,
            tcp_keepalive: false,
        };

        let err = anyhow::anyhow!("ssh login error: password auth status: Denied");
        let message = ssh_connect_failure_message(&opts, &err);
        assert!(
            message.contains("Wrong password"),
            "expected failure message to map denial to user-friendly phrasing, got {message:?}"
        );
    }

    #[test]
    fn parse_ssh_host_key_mismatch_extracts_fingerprint_and_known_hosts_path() {
        let reason = concat!(
            "host verification failed: host key mismatch for ssh server 127.0.0.1:22. ",
            "Got fingerprint SHA256:P2yrg3Yviu/doplfeJo5IggYUMVuuF2vFNEOceH2qtM ",
            "instead of the expected value from your known hosts file ",
            "Some(\"/home/iamazy/.ssh/known_hosts\")"
        );

        let details = parse_ssh_host_key_mismatch(reason).expect("expected mismatch details");
        assert_eq!(
            details.got_fingerprint.as_deref(),
            Some("SHA256:P2yrg3Yviu/doplfeJo5IggYUMVuuF2vFNEOceH2qtM")
        );
        assert_eq!(
            details
                .known_hosts_path
                .as_ref()
                .map(|p| p.to_string_lossy().to_string()),
            Some("/home/iamazy/.ssh/known_hosts".to_string())
        );
        assert_eq!(details.server_host.as_deref(), Some("127.0.0.1"));
        assert_eq!(details.server_port, Some(22));
    }

    #[test]
    fn filter_known_hosts_contents_removes_matching_host_entries() {
        let contents = concat!(
            "# comment\n",
            "127.0.0.1 ssh-ed25519 AAAA\n",
            "[127.0.0.1]:2222 ssh-ed25519 BBBB\n",
            "example.com ssh-ed25519 CCCC\n"
        );

        let (out_22, removed_22) = filter_known_hosts_contents(contents, "127.0.0.1", 22);
        assert_eq!(removed_22, 1, "out={out_22:?}");
        assert!(!out_22.contains("127.0.0.1 ssh-ed25519 AAAA"));
        assert!(out_22.contains("[127.0.0.1]:2222 ssh-ed25519 BBBB"));
        assert!(out_22.contains("example.com ssh-ed25519 CCCC"));

        let (out_2222, removed_2222) = filter_known_hosts_contents(contents, "127.0.0.1", 2222);
        assert_eq!(removed_2222, 1, "out={out_2222:?}");
        assert!(out_2222.contains("127.0.0.1 ssh-ed25519 AAAA"));
        assert!(!out_2222.contains("[127.0.0.1]:2222 ssh-ed25519 BBBB"));
        assert!(out_2222.contains("example.com ssh-ed25519 CCCC"));
    }
}
