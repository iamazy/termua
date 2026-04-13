#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

rust_i18n::i18n!("../locales");

mod app_state;
mod assistant;
mod atomic_write;
mod bootstrap;
mod cast_player;
mod command_history;
mod env;
mod footbar;
mod globals;
mod keychain;
mod locale;
mod lock_screen;
mod logging;
mod menu;
mod notification;
mod panel;
mod right_sidebar;
mod serial;
mod session;
mod settings;
mod sharing;
mod shell_integration;
mod ssh;
mod static_suggestions;
mod theme_manager;
mod window;

pub(crate) use app_state::{PendingCommand, TermuaAppState};
pub(crate) use menu::{
    NewLocalTerminal, OpenNewSession, OpenSftp, PlayCast, ToggleAssistantSidebar,
    ToggleMessagesSidebar, ToggleMultiExec, ToggleSessionsSidebar,
};
pub use session::store;
pub use window::{new_session, settings as config};

use crate::settings::SettingsFile;

fn main() {
    if let Some(result) = try_parse_relay_mode_args() {
        match result {
            Ok(args) => {
                if let Err(err) = termua_relay::server::serve_blocking(
                    args.listen,
                    termua_relay::server::ServerConfig {
                        gate_input: args.gate_input,
                    },
                ) {
                    eprintln!("{err:#}");
                    std::process::exit(1);
                }
                return;
            }
            Err(err) => {
                eprintln!("{err:#}");
                std::process::exit(2);
            }
        }
    }

    match cast_player::try_run_from_env() {
        Ok(true) => return,
        Ok(false) => {}
        Err(err) => {
            eprintln!("{err:#}");
            std::process::exit(1);
        }
    }

    let settings = match settings::load_settings_from_disk() {
        Ok(s) => s,
        Err(err) => {
            eprintln!("failed to load settings.json, using defaults: {err:#}");
            SettingsFile::default()
        }
    };
    logging::init_logging(&settings);

    bootstrap::run(settings);
}

#[derive(Debug, Clone, Copy)]
struct RelayModeArgs {
    listen: std::net::SocketAddr,
    gate_input: bool,
}

fn try_parse_relay_mode_args() -> Option<anyhow::Result<RelayModeArgs>> {
    let mut args = std::env::args_os();
    let _program = args.next();

    let mut run_relay = false;
    let mut listen: Option<std::net::SocketAddr> = None;
    let mut gate_input = true;

    while let Some(arg) = args.next() {
        let Some(arg) = arg.to_str() else {
            continue;
        };

        if arg == "--run-relay" {
            run_relay = true;
            continue;
        }

        if let Some(v) = arg.strip_prefix("--listen=") {
            match v.parse() {
                Ok(v) => listen = Some(v),
                Err(err) => return Some(Err(err.into())),
            }
            continue;
        }
        if arg == "--listen" {
            let Some(v) = args.next().and_then(|v| v.to_str().map(str::to_string)) else {
                return Some(Err(anyhow::anyhow!("--listen requires a value")));
            };
            match v.parse() {
                Ok(v) => listen = Some(v),
                Err(err) => return Some(Err(err.into())),
            }
            continue;
        }

        if let Some(v) = arg.strip_prefix("--gate-input=") {
            match v.parse::<bool>() {
                Ok(v) => gate_input = v,
                Err(err) => return Some(Err(err.into())),
            }
            continue;
        }
        if arg == "--gate-input" {
            let Some(v) = args.next().and_then(|v| v.to_str().map(str::to_string)) else {
                return Some(Err(anyhow::anyhow!("--gate-input requires a value")));
            };
            match v.parse::<bool>() {
                Ok(v) => gate_input = v,
                Err(err) => return Some(Err(err.into())),
            }
            continue;
        }
    }

    if !run_relay {
        return None;
    }

    Some(Ok(RelayModeArgs {
        listen: listen.unwrap_or_else(|| "127.0.0.1:7231".parse().expect("valid default addr")),
        gate_input,
    }))
}
