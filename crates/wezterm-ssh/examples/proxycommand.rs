//! Demonstrates configuring `ProxyCommand` for wezterm-ssh.
//!
//! Usage:
//!   cargo run -p wezterm-ssh --example proxycommand -- <destination> <proxy_command> [--connect]
//!
//! Examples:
//!   # Typical OpenSSH-style: use local ssh as the transport to the bastion
//!   cargo run -p wezterm-ssh --example proxycommand -- 10.0.0.10 \
//!     \"ssh -W %h:%p bastion_user@bastion.example.com\"
//!
//! Notes:
//! - wezterm-ssh expands `%h/%n/%p/%r` tokens for `proxycommand` when the value is provided via
//!   ssh_config parsing (like in this example).
//! - The proxy command is executed by wezterm-ssh (via `sh -c` on Unix).
//! - Use this only for systems you are authorized to access.

use std::io::{self, Write};

use anyhow::{bail, Context};
use wezterm_ssh::{Config, Session, SessionEvent};

fn usage() -> ! {
    eprintln!(
        "Usage: proxycommand <destination> <proxy_command> [--connect]\n\nExample:\nproxycommand \
         10.0.0.10 \"ssh -W %h:%p bastion_user@bastion.example.com\"\n"
    );
    std::process::exit(2);
}

fn prompt_yes_no(prompt: &str) -> anyhow::Result<bool> {
    let mut stdout = io::stdout();
    stdout.write_all(prompt.as_bytes())?;
    stdout.flush()?;

    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    let line = line.trim();
    Ok(matches!(line, "y" | "Y" | "yes" | "YES"))
}

fn read_line(prompt: &str) -> anyhow::Result<String> {
    let mut stdout = io::stdout();
    stdout.write_all(prompt.as_bytes())?;
    stdout.flush()?;

    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    Ok(line.trim_end_matches(&['\r', '\n'][..]).to_string())
}

fn main() -> anyhow::Result<()> {
    let mut destination: Option<String> = None;
    let mut proxy_command: Option<String> = None;
    let mut connect = false;

    let args = std::env::args().skip(1);
    for arg in args {
        match arg.as_str() {
            "--connect" => connect = true,
            "-h" | "--help" => usage(),
            _ => {
                if destination.is_none() {
                    destination = Some(arg);
                } else if proxy_command.is_none() {
                    proxy_command = Some(arg);
                } else {
                    usage();
                }
            }
        }
    }

    let destination = destination.unwrap_or_else(|| usage());
    let proxy_command = proxy_command.unwrap_or_else(|| usage());

    let mut cfg = Config::new();
    cfg.add_default_config_files();

    // Use config parsing so that `%h/%p/%r` token expansion is applied to ProxyCommand.
    cfg.add_config_string(&format!(
        "Host {dest}\n  ProxyCommand {cmd}\n",
        dest = destination,
        cmd = proxy_command
    ));

    let config = cfg.for_host(&destination);
    println!(
        "proxycommand={}",
        config.get("proxycommand").map(|s| s.as_str()).unwrap_or("")
    );

    if !connect {
        println!("(pass --connect to attempt a connection)");
        return Ok(());
    }

    let (_session, events) = Session::connect(config).context("Session::connect")?;

    smol::block_on(async move {
        while let Ok(event) = events.recv().await {
            match event {
                SessionEvent::Banner(banner) => {
                    if let Some(banner) = banner {
                        eprintln!("{banner}");
                    }
                }
                SessionEvent::HostVerify(verify) => {
                    eprintln!("{}", verify.message);
                    let ok = prompt_yes_no("Trust this host key? [y/N]> ")?;
                    verify.answer(ok).await.context("verify.answer")?;
                    if !ok {
                        bail!("host verification rejected by user");
                    }
                }
                SessionEvent::Authenticate(auth) => {
                    if !auth.username.is_empty() {
                        eprintln!("Authentication for {}", auth.username);
                    }
                    if !auth.instructions.is_empty() {
                        eprintln!("{}", auth.instructions);
                    }
                    let mut answers = Vec::with_capacity(auth.prompts.len());
                    for p in &auth.prompts {
                        let ans = read_line(p.prompt.as_str())?;
                        answers.push(ans);
                    }
                    auth.answer(answers).await.context("auth.answer")?;
                }
                SessionEvent::HostVerificationFailed(failed) => bail!("{failed}"),
                SessionEvent::Error(err) => bail!("{err}"),
                SessionEvent::Authenticated => {
                    println!("authenticated");
                    break;
                }
            }
        }
        Ok(())
    })
}
