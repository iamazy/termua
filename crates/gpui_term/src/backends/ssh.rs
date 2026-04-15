use std::{cell::RefCell, collections::HashMap, time::Duration};

use anyhow::anyhow;
use futures::FutureExt;
use log::{debug, error, trace};
use smol::{
    Timer,
    channel::{Receiver, Sender, bounded},
};
use wezterm_ssh::{
    Config, ConfigMap, PtySize, Session, SessionEvent, Sftp, SshChildProcess, SshPty,
};

#[derive(Copy, Clone, Debug, Default, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SshBackend {
    /// Use `ssh2` (libssh2).
    #[default]
    Ssh2,
    /// Use `libssh` (libssh-rs).
    Libssh,
}

impl SshBackend {
    pub fn as_wezterm_config_value(self) -> &'static str {
        match self {
            Self::Ssh2 => "ssh2",
            Self::Libssh => "libssh",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SshOptions {
    pub host: String,
    pub port: Option<u16>,
    pub auth: Authentication,
    pub proxy: SshProxyMode,
    pub backend: SshBackend,
    pub tcp_nodelay: bool,
    pub tcp_keepalive: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Authentication {
    Password(String, String),
    Config,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SshProxyMode {
    /// Follow ssh config/defaults (don't override `ProxyCommand`).
    #[default]
    Inherit,
    /// Force-disable proxying (equivalent to `ProxyCommand none`).
    Disabled,
    /// Use an explicit `ProxyCommand` for this session (overrides ssh config).
    Command(SshProxyCommand),
    /// Connect via one or more intermediate SSH servers (ProxyJump-like).
    JumpServer(SshJumpChain),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshProxyCommand {
    pub command: String,
    pub working_dir: Option<String>,
    pub env: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshJumpChain {
    pub hops: Vec<SshJumpHop>,
    pub working_dir: Option<String>,
    pub env: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshJumpHop {
    pub host: String,
    pub user: Option<String>,
    pub port: Option<u16>,
}

const DEFAULT_SERVER_ALIVE_INTERVAL_SECS: u64 = 60;
const AUTHENTICATION_TIMEOUT_SECS: u64 = 20;
const HOST_VERIFICATION_TIMEOUT_SECS: u64 = 120;
const MAX_PASSWORD_PROMPTS: usize = 3;

/// A request to confirm trusting an unknown SSH host key.
///
/// Embedding apps can provide a UI for this by installing a per-thread prompt sender via
/// [`SshHostVerificationPromptGuard`].
#[derive(Debug)]
pub struct SshHostVerificationPrompt {
    pub message: String,
    pub reply: Sender<bool>,
}

thread_local! {
    static HOST_VERIFY_PROMPT_SENDER: RefCell<Option<Sender<SshHostVerificationPrompt>>> = const { RefCell::new(None) };
}

/// A guard that installs a per-thread SSH host verification prompt sender.
///
/// This is used to route interactive trust prompts (unknown host keys) from background SSH
/// handshake threads back to the embedding UI. Dropping the guard restores the previous sender.
pub struct SshHostVerificationPromptGuard {
    prev: Option<Sender<SshHostVerificationPrompt>>,
}

impl Drop for SshHostVerificationPromptGuard {
    fn drop(&mut self) {
        HOST_VERIFY_PROMPT_SENDER.with(|slot| {
            *slot.borrow_mut() = self.prev.take();
        });
    }
}

pub fn set_thread_ssh_host_verification_prompt_sender(
    sender: Option<Sender<SshHostVerificationPrompt>>,
) -> SshHostVerificationPromptGuard {
    let prev = HOST_VERIFY_PROMPT_SENDER.with(|slot| slot.replace(sender));
    SshHostVerificationPromptGuard { prev }
}

async fn prompt_trust_unknown_host(message: String) -> Option<bool> {
    let sender = HOST_VERIFY_PROMPT_SENDER.with(|slot| slot.borrow().clone())?;

    let (reply, confirm) = bounded(1);
    if sender
        .send(SshHostVerificationPrompt { message, reply })
        .await
        .is_err()
    {
        return None;
    }

    let mut timeout = Timer::after(Duration::from_secs(HOST_VERIFICATION_TIMEOUT_SECS)).fuse();
    let recv = confirm.recv().fuse();
    futures::pin_mut!(recv);

    futures::select_biased! {
        decision = recv => decision.ok(),
        _ = timeout => None,
    }
}

fn prompt_is_password_like(prompt: &str) -> bool {
    let p = prompt.to_ascii_lowercase();
    p.contains("password") || p.contains("passphrase")
}

fn prompt_is_otp_like(prompt: &str) -> bool {
    // Best-effort heuristics for common MFA/OTP prompts. We don't support interactive prompting
    // in the UI yet, so this is only used to improve error messages.
    let p = prompt.to_ascii_lowercase();
    p.contains("verification code")
        || p.contains("verification-code")
        || p.contains("one-time")
        || p.contains("one time")
        || p.contains("otp")
        || p.contains("totp")
        || p.contains("2fa")
        || p.contains("two-factor")
        || p.contains("two factor")
        || p.contains("auth code")
        || p.contains("auth-code")
        || p.contains("token")
        || p.contains("security code")
}

fn spawn_session_event_drain(events: Receiver<SessionEvent>) {
    std::thread::spawn(move || {
        while let Ok(event) = smol::block_on(events.recv()) {
            match event {
                SessionEvent::HostVerify(verify) => {
                    // We are draining because the session is being abandoned; be conservative and
                    // decline trust prompts so we don't accidentally persist a host key.
                    let _ = smol::block_on(verify.answer(false));
                }
                SessionEvent::Authenticate(auth) => {
                    let answers: Vec<String> = auth.prompts.iter().map(|_| String::new()).collect();
                    let _ = smol::block_on(auth.answer(answers));
                }
                _ => {}
            }
        }
    });
}

fn ensure_default_keepalive(config: &mut ConfigMap) {
    // `wezterm_ssh` follows OpenSSH semantics: `ServerAliveInterval 0` disables keepalive.
    // We only set a default value if the user/config didn't specify anything.
    config
        .entry("serveraliveinterval".to_string())
        .or_insert_with(|| DEFAULT_SERVER_ALIVE_INTERVAL_SECS.to_string());
}

#[cfg(unix)]
fn sh_single_quote(s: &str) -> String {
    // `'` in a single-quoted string is represented as: '"'"'
    let escaped = s.replace('\'', "'\"'\"'");
    format!("'{escaped}'")
}

fn expand_proxy_command_tokens(
    template: &str,
    hostname: &str,
    original_host: &str,
    port: u16,
    user: &str,
) -> String {
    // Match wezterm-ssh's `proxycommand` token set: %h %n %p %r and `%%` for literal.
    template
        .replace("%h", hostname)
        .replace("%n", original_host)
        .replace("%p", &port.to_string())
        .replace("%r", user)
        .replace("%%", "%")
}

#[cfg(unix)]
fn wrap_proxy_command_unix(
    command: &str,
    working_dir: Option<&str>,
    env: &HashMap<String, String>,
) -> String {
    let mut script = String::new();

    if let Some(dir) = working_dir
        && !dir.trim().is_empty()
    {
        script.push_str("cd -- ");
        script.push_str(&sh_single_quote(dir.trim()));
        script.push_str(" || exit 1; ");
    }

    for (k, v) in env.iter() {
        let k = k.trim();
        if k.is_empty() {
            continue;
        }
        // Conservative: avoid producing obviously broken shell for invalid names.
        let valid = k.bytes().enumerate().all(|(i, b)| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'_' => true,
            b'0'..=b'9' => i != 0,
            _ => false,
        });
        if !valid {
            continue;
        }

        script.push_str("export ");
        script.push_str(k);
        script.push('=');
        script.push_str(&sh_single_quote(v));
        script.push_str("; ");
    }

    script.push_str("exec ");
    script.push_str(command);

    format!("sh -lc {}", sh_single_quote(&script))
}

fn apply_proxy_settings(opts: &SshOptions, config: &mut ConfigMap) {
    let hostname = config
        .get("hostname")
        .map(String::as_str)
        .unwrap_or(opts.host.as_str());
    let user = config
        .get("user")
        .map(String::as_str)
        .unwrap_or("unknown-user");
    let port: u16 = config
        .get("port")
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(22);

    match &opts.proxy {
        SshProxyMode::Inherit => {}
        SshProxyMode::Disabled => {
            config.insert("proxycommand".to_string(), "none".to_string());
        }
        SshProxyMode::Command(settings) => {
            #[allow(unused_mut)]
            let mut cmd = expand_proxy_command_tokens(
                settings.command.as_str(),
                hostname,
                opts.host.as_str(),
                port,
                user,
            );

            #[cfg(unix)]
            {
                if settings.working_dir.as_deref().is_some() || !settings.env.is_empty() {
                    cmd = wrap_proxy_command_unix(
                        cmd.as_str(),
                        settings.working_dir.as_deref(),
                        &settings.env,
                    );
                }
            }

            config.insert("proxycommand".to_string(), cmd);
        }
        SshProxyMode::JumpServer(jump) => {
            let hops: Vec<&SshJumpHop> = jump
                .hops
                .iter()
                .filter(|h| !h.host.trim().is_empty())
                .collect();
            if hops.is_empty() {
                config.insert("proxyjump".to_string(), "none".to_string());
                return;
            }

            let chain = hops
                .iter()
                .filter_map(|h| {
                    let host = h.host.trim();
                    if host.is_empty() {
                        return None;
                    }
                    let mut out = String::new();
                    if let Some(user) = h.user.as_deref()
                        && !user.trim().is_empty()
                    {
                        out.push_str(user.trim());
                        out.push('@');
                    }
                    out.push_str(host);
                    if let Some(port) = h.port {
                        out.push(':');
                        out.push_str(&port.to_string());
                    }
                    Some(out)
                })
                .collect::<Vec<_>>()
                .join(",");

            config.insert("proxyjump".to_string(), chain);
        }
    }
}

fn build_config_for_opts(
    opts: &SshOptions,
    load_default_config_files: bool,
) -> (Option<String>, ConfigMap) {
    let mut cfg = Config::new();
    if load_default_config_files {
        cfg.add_default_config_files();
    }

    let mut config = cfg.for_host(opts.host.as_str());

    let auth_password = match &opts.auth {
        Authentication::Password(user, password) => {
            if let Some(port) = opts.port {
                config.insert("port".to_string(), port.to_string());
            }
            config.insert("user".to_string(), user.clone());
            Some(password.clone())
        }
        Authentication::Config => {
            if let Some(port) = opts.port {
                config.insert("port".to_string(), port.to_string());
            }
            None
        }
    };

    ensure_default_keepalive(&mut config);
    apply_proxy_settings(opts, &mut config);

    config.insert(
        "wezterm_ssh_backend".to_string(),
        opts.backend.as_wezterm_config_value().to_string(),
    );
    config.insert(
        "wezterm_ssh_tcp_nodelay".to_string(),
        if opts.tcp_nodelay {
            "true".to_string()
        } else {
            "false".to_string()
        },
    );
    config.insert(
        "wezterm_ssh_tcp_keepalive".to_string(),
        if opts.tcp_keepalive {
            "true".to_string()
        } else {
            "false".to_string()
        },
    );

    (auth_password, config)
}

pub fn connect(
    env: HashMap<String, String>,
    opts: SshOptions,
) -> anyhow::Result<(SshPty, SshChildProcess, Sftp)> {
    smol::block_on(connect_async(env, opts))
}

async fn connect_async(
    env: HashMap<String, String>,
    opts: SshOptions,
) -> anyhow::Result<(SshPty, SshChildProcess, Sftp)> {
    let (auth_password, config) = build_config_for_opts(&opts, true);
    log_connect_attempt(&opts, &config);

    let (session, events) = Session::connect(config)?;
    let (session, _events) =
        authenticate_session(session, events, auth_password.as_deref()).await?;

    // // NOTE: Avoid forcing locale facets like `LC_COLLATE=C`. Some userlands (e.g. uutils
    // // coreutils `ls`) may treat that as a "C locale" signal and shell-escape UTF-8
    // // filenames (showing `$'\\NNN'` byte sequences) instead of rendering Unicode.
    // //
    // // Prefer inheriting the local process locale, and let the remote host decide if it
    // // can't apply it (it may ignore env requests depending on sshd config).
    // let mut env = HashMap::new();
    // for key in ["LC_ALL", "LC_CTYPE", "LANG"] {
    //     if let Ok(val) = std::env::var(key)
    //         && !val.is_empty()
    //     {
    //         env.insert(key.to_string(), val);
    //     }
    // }

    let env = if env.is_empty() { None } else { Some(env) };

    let (pty, child) = session
        .request_pty("xterm-256color", PtySize::default(), None, env)
        .await?;
    let sftp = session.sftp();
    Ok((pty, child, sftp))
}

fn log_connect_attempt(opts: &SshOptions, config: &ConfigMap) {
    let ssh_backend = config
        .get("wezterm_ssh_backend")
        .map(String::as_str)
        .unwrap_or("<unset>");
    debug!(
        "ssh connect: {}:{} (backend={})",
        opts.host.as_str(),
        config.get("port").map(String::as_str).unwrap_or("22"),
        ssh_backend
    );
}

fn abort_auth<T>(
    session: Session,
    events: Receiver<SessionEvent>,
    err: anyhow::Error,
) -> anyhow::Result<T> {
    // Do not drop the `events` receiver while the session thread might still be running; keep it
    // alive and best-effort answer prompts to avoid panics inside the wezterm-ssh dependency.
    drop(session);
    spawn_session_event_drain(events);
    Err(err)
}

struct AuthPromptPlan {
    answers: Vec<String>,
    used_password: bool,
    saw_otp_prompt: bool,
    prompts_summary: String,
}

fn build_auth_prompt_plan(
    prompts: &[wezterm_ssh::AuthenticationPrompt],
    password: &str,
) -> AuthPromptPlan {
    let mut used_password = false;
    let mut saw_otp_prompt = false;

    let mut answers: Vec<String> = Vec::with_capacity(prompts.len());
    for prompt in prompts.iter() {
        if prompt_is_password_like(&prompt.prompt) {
            answers.push(password.to_string());
            used_password = true;
        } else {
            if !prompt.echo && prompt_is_otp_like(&prompt.prompt) {
                saw_otp_prompt = true;
            }
            answers.push(String::new());
        }
    }

    let prompts_summary = prompts
        .iter()
        .map(|p| p.prompt.trim())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" | ");

    AuthPromptPlan {
        answers,
        used_password,
        saw_otp_prompt,
        prompts_summary,
    }
}

async fn recv_event_or_timeout(
    events: &Receiver<SessionEvent>,
    mut auth_timeout: &mut futures::future::Fuse<Timer>,
) -> anyhow::Result<Option<SessionEvent>> {
    let recv = events.recv().fuse();
    futures::pin_mut!(recv);
    let event = futures::select_biased! {
        event = recv => Some(event),
        _ = auth_timeout => None,
    };

    let Some(event) = event else {
        return Ok(None);
    };

    event
        .map(Some)
        .map_err(|_| anyhow!("ssh login error: connection closed during authentication"))
}

async fn authenticate_session(
    session: Session,
    events: Receiver<SessionEvent>,
    auth_password: Option<&str>,
) -> anyhow::Result<(Session, Receiver<SessionEvent>)> {
    let session = session;
    let events = events;

    let mut auth_timeout = Timer::after(Duration::from_secs(AUTHENTICATION_TIMEOUT_SECS)).fuse();
    let mut password_prompt_count: usize = 0;

    loop {
        let event = recv_event_or_timeout(&events, &mut auth_timeout).await?;
        let Some(event) = event else {
            return abort_auth(
                session,
                events,
                anyhow!("ssh login error: authentication timed out"),
            );
        };

        // Treat timeout as inactivity-based: any received event resets the timer.
        auth_timeout = Timer::after(Duration::from_secs(AUTHENTICATION_TIMEOUT_SECS)).fuse();

        match event {
            SessionEvent::Banner(banner) => {
                if let Some(banner) = banner {
                    trace!("{banner}");
                }
            }
            SessionEvent::HostVerify(verify) => {
                let trusted = prompt_trust_unknown_host(verify.message.clone())
                    .await
                    .unwrap_or(false);

                // Always answer to unblock the session thread; on decline we also abort this
                // connection attempt.
                let _ = verify.answer(trusted).await;

                if !trusted {
                    return abort_auth(
                        session,
                        events,
                        anyhow!("ssh login error: host key not trusted"),
                    );
                }
            }
            SessionEvent::Authenticate(auth) => {
                let Some(password) = auth_password else {
                    // No interactive prompting in the UI at the moment; fail fast, but make sure
                    // to answer this prompt (and drain subsequent events) so we don't trigger
                    // panics inside wezterm-ssh.
                    let answers: Vec<String> = auth.prompts.iter().map(|_| String::new()).collect();
                    let _ = auth.answer(answers).await;
                    return abort_auth(
                        session,
                        events,
                        anyhow!("ssh login error: interactive authentication required"),
                    );
                };

                let plan = build_auth_prompt_plan(&auth.prompts, password);
                if !plan.used_password {
                    let _ = auth.answer(plan.answers).await;
                    let err = if plan.saw_otp_prompt {
                        anyhow!(
                            "ssh login error: unsupported authentication prompt (OTP/2FA \
                             required): {}",
                            plan.prompts_summary
                        )
                    } else {
                        anyhow!(
                            "ssh login error: unsupported authentication prompt: {}",
                            plan.prompts_summary
                        )
                    };
                    return abort_auth(session, events, err);
                }

                password_prompt_count += 1;
                if password_prompt_count > MAX_PASSWORD_PROMPTS {
                    let _ = auth.answer(plan.answers).await;
                    // The server keeps re-prompting for a password; treat as a denial.
                    return abort_auth(
                        session,
                        events,
                        anyhow!("ssh login error: password auth status: Denied"),
                    );
                }

                if let Err(err) = auth.answer(plan.answers).await {
                    return abort_auth(session, events, anyhow!("ssh login error: {err:#}"));
                }
            }
            SessionEvent::HostVerificationFailed(failed) => {
                error!("host verification failed: {failed}");
                return Err(anyhow!("host verification failed: {failed}"));
            }
            SessionEvent::Error(err) => {
                error!("ssh login error: {err}");
                return Err(anyhow!("ssh login error: {err}"));
            }
            SessionEvent::Authenticated => break,
        }
    }

    Ok((session, events))
}

#[cfg(test)]
fn build_config_for_test(opts: &SshOptions) -> ConfigMap {
    // Keep tests deterministic: don't read user ssh config from disk.
    build_config_for_opts(opts, false).1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_keepalive_is_applied_when_absent() {
        let mut config = ConfigMap::new();
        ensure_default_keepalive(&mut config);
        assert_eq!(
            config.get("serveraliveinterval").map(String::as_str),
            Some("60")
        );
    }

    #[test]
    fn default_keepalive_does_not_override_user_value() {
        let mut config = ConfigMap::new();
        config.insert("serveraliveinterval".to_string(), "0".to_string());
        ensure_default_keepalive(&mut config);
        assert_eq!(
            config.get("serveraliveinterval").map(String::as_str),
            Some("0")
        );
    }

    #[test]
    fn prompt_password_like_detection() {
        assert!(prompt_is_password_like("Password:"));
        assert!(prompt_is_password_like("Password for root@example.com: "));
        assert!(prompt_is_password_like(
            "Enter passphrase for key '/home/me/.ssh/id_ed25519': "
        ));
        assert!(!prompt_is_password_like("Verification code: "));
    }

    #[test]
    fn prompt_otp_like_detection() {
        assert!(prompt_is_otp_like("Verification code: "));
        assert!(prompt_is_otp_like("One-time password: "));
        assert!(prompt_is_otp_like("TOTP: "));
        assert!(!prompt_is_otp_like("Password: "));
    }

    #[test]
    fn proxy_disabled_sets_proxycommand_none() {
        let opts = SshOptions {
            host: "example.com".to_string(),
            port: Some(2222),
            auth: Authentication::Password("alice".to_string(), "pw".to_string()),
            proxy: SshProxyMode::Disabled,
            backend: SshBackend::default(),
            tcp_nodelay: false,
            tcp_keepalive: false,
        };

        let config = build_config_for_test(&opts);
        assert_eq!(config.get("proxycommand").map(String::as_str), Some("none"));
    }

    #[test]
    fn proxy_command_expands_common_tokens() {
        let opts = SshOptions {
            host: "server-alias".to_string(),
            port: Some(2200),
            auth: Authentication::Password("bob".to_string(), "pw".to_string()),
            proxy: SshProxyMode::Command(SshProxyCommand {
                command: "nc -x 127.0.0.1:1080 %h %p %r %n".to_string(),
                working_dir: None,
                env: HashMap::new(),
            }),
            backend: SshBackend::default(),
            tcp_nodelay: false,
            tcp_keepalive: false,
        };

        let config = build_config_for_test(&opts);
        let cmd = config.get("proxycommand").cloned().unwrap_or_default();
        assert!(cmd.contains("server-alias"));
        assert!(cmd.contains("2200"));
        assert!(cmd.contains("bob"));
    }

    #[cfg(unix)]
    #[test]
    fn proxy_command_wraps_with_cwd_and_env_on_unix() {
        let mut env = HashMap::new();
        env.insert(
            "HTTP_PROXY".to_string(),
            "http://127.0.0.1:3128".to_string(),
        );

        let opts = SshOptions {
            host: "example.com".to_string(),
            port: Some(22),
            auth: Authentication::Password("root".to_string(), "pw".to_string()),
            proxy: SshProxyMode::Command(SshProxyCommand {
                command: "nc %h %p".to_string(),
                working_dir: Some("/tmp".to_string()),
                env,
            }),
            backend: SshBackend::default(),
            tcp_nodelay: false,
            tcp_keepalive: false,
        };

        let config = build_config_for_test(&opts);
        let cmd = config.get("proxycommand").cloned().unwrap_or_default();
        assert!(cmd.starts_with("sh -lc "));
        assert!(cmd.contains("cd --"));
        assert!(cmd.contains("HTTP_PROXY="));
        assert!(cmd.contains("exec nc"));
    }

    #[test]
    fn jump_proxy_one_hop_sets_proxyjump() {
        let opts = SshOptions {
            host: "example.com".to_string(),
            port: Some(22),
            auth: Authentication::Password("root".to_string(), "pw".to_string()),
            proxy: SshProxyMode::JumpServer(SshJumpChain {
                hops: vec![SshJumpHop {
                    host: "jump1".to_string(),
                    user: Some("alice".to_string()),
                    port: Some(2222),
                }],
                working_dir: None,
                env: HashMap::new(),
            }),
            backend: SshBackend::default(),
            tcp_nodelay: false,
            tcp_keepalive: false,
        };

        let config = build_config_for_test(&opts);
        assert_eq!(
            config.get("proxyjump").map(String::as_str),
            Some("alice@jump1:2222")
        );
    }

    #[test]
    fn jump_proxy_multi_hop_sets_proxyjump_chain() {
        let opts = SshOptions {
            host: "example.com".to_string(),
            port: Some(22),
            auth: Authentication::Password("root".to_string(), "pw".to_string()),
            proxy: SshProxyMode::JumpServer(SshJumpChain {
                hops: vec![
                    SshJumpHop {
                        host: "jump1".to_string(),
                        user: Some("a".to_string()),
                        port: Some(22),
                    },
                    SshJumpHop {
                        host: "jump2".to_string(),
                        user: None,
                        port: Some(2201),
                    },
                    SshJumpHop {
                        host: "jump3".to_string(),
                        user: Some("c".to_string()),
                        port: None,
                    },
                ],
                working_dir: None,
                env: HashMap::new(),
            }),
            backend: SshBackend::default(),
            tcp_nodelay: false,
            tcp_keepalive: false,
        };

        let config = build_config_for_test(&opts);
        assert_eq!(
            config.get("proxyjump").map(String::as_str),
            Some("a@jump1:22,jump2:2201,c@jump3")
        );
    }

    #[test]
    fn tcp_socket_options_are_written_into_config() {
        let opts = SshOptions {
            host: "example.com".to_string(),
            port: Some(22),
            auth: Authentication::Password("root".to_string(), "pw".to_string()),
            proxy: SshProxyMode::Disabled,
            backend: SshBackend::default(),
            tcp_nodelay: true,
            tcp_keepalive: true,
        };

        let config = build_config_for_test(&opts);
        assert_eq!(
            config.get("wezterm_ssh_tcp_nodelay").map(String::as_str),
            Some("true")
        );
        assert_eq!(
            config.get("wezterm_ssh_tcp_keepalive").map(String::as_str),
            Some("true")
        );
    }

    #[test]
    fn ssh_backend_is_written_into_config() {
        let opts = SshOptions {
            host: "example.com".to_string(),
            port: Some(22),
            auth: Authentication::Config,
            proxy: SshProxyMode::Inherit,
            backend: SshBackend::Libssh,
            tcp_nodelay: false,
            tcp_keepalive: false,
        };

        let config = build_config_for_test(&opts);
        assert_eq!(
            config.get("wezterm_ssh_backend").map(String::as_str),
            Some("libssh")
        );
    }
}
