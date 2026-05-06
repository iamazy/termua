#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

rust_i18n::i18n!("../locales");

mod app_state;
mod assistant;
mod atomic_write;
mod bootstrap;
mod cast_player;
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
mod ssh;
mod static_suggestions;
mod theme_manager;
mod window;

pub(crate) use app_state::{PendingCommand, SerialParams, SshParams, TermuaAppState};
pub(crate) use menu::{
    NewLocalTerminal, OpenNewSession, OpenSftp, PlayCast, ToggleAssistantSidebar,
    ToggleMessagesSidebar, ToggleMultiExec, ToggleSessionsSidebar,
};
pub use session::store;
pub use window::{new_session, settings as config};

use crate::settings::SettingsFile;

fn main() {
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
