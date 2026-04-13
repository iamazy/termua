mod actions;
mod keybindings;
mod meta;
mod render;
mod state;
mod view;

pub use meta::{SettingMeta, SettingsNavSection, search_settings};
// Used by `keybindings.rs`.
use state::TerminalKeybinding;
pub use state::{SettingsPage, SettingsWindow};

#[cfg(test)]
mod tests;
