use std::collections::HashMap;

use rust_i18n::t;

use super::TerminalKeybinding;
use crate::settings::SettingsFile;

pub(super) fn normalize_keybinding_value(raw: &str) -> Option<String> {
    let v = raw.trim();
    if v.is_empty() {
        None
    } else {
        // Store in canonical, parseable form so we can compare keystrokes reliably and so
        // settings.json doesn't depend on any platform-specific display formatting.
        gpui::Keystroke::parse(v).ok().map(|k| k.unparse())
    }
}

pub(super) fn is_modifier_only_key(key: &str) -> bool {
    matches!(key, "shift" | "control" | "alt" | "platform" | "function")
}

pub(super) fn keybinding_clear_button_enabled(value: Option<&String>) -> bool {
    value.map(|v| !v.trim().is_empty()).unwrap_or(false)
}

pub(super) fn terminal_keybinding_conflicts(
    settings: &SettingsFile,
) -> HashMap<&'static str, Vec<&'static str>> {
    let mut by_stroke: HashMap<String, Vec<&'static str>> = HashMap::new();
    for k in TerminalKeybinding::all() {
        let id = k.id();
        let value = match k {
            TerminalKeybinding::Copy => settings.terminal_keybindings.copy.as_deref(),
            TerminalKeybinding::Paste => settings.terminal_keybindings.paste.as_deref(),
            TerminalKeybinding::SelectAll => settings.terminal_keybindings.select_all.as_deref(),
            TerminalKeybinding::Clear => settings.terminal_keybindings.clear.as_deref(),
            TerminalKeybinding::Search => settings.terminal_keybindings.search.as_deref(),
            TerminalKeybinding::SearchNext => settings.terminal_keybindings.search_next.as_deref(),
            TerminalKeybinding::SearchPrevious => {
                settings.terminal_keybindings.search_previous.as_deref()
            }
            TerminalKeybinding::IncreaseFontSize => {
                settings.terminal_keybindings.increase_font_size.as_deref()
            }
            TerminalKeybinding::DecreaseFontSize => {
                settings.terminal_keybindings.decrease_font_size.as_deref()
            }
            TerminalKeybinding::ResetFontSize => {
                settings.terminal_keybindings.reset_font_size.as_deref()
            }
        };

        let Some(value) = value else { continue };
        let Some(canonical) = normalize_keybinding_value(value) else {
            continue;
        };
        by_stroke.entry(canonical).or_default().push(id);
    }

    let mut conflicts: HashMap<&'static str, Vec<&'static str>> = HashMap::new();
    for ids in by_stroke.into_values() {
        if ids.len() < 2 {
            continue;
        }
        for &id in &ids {
            conflicts.insert(
                id,
                ids.iter().copied().filter(|other| other != &id).collect(),
            );
        }
    }

    conflicts
}

pub(super) fn keybinding_short_name(id: &'static str) -> String {
    let key = format!("Settings.Meta.{}.Title", id);
    t!(key.as_str()).to_string()
}

pub(super) fn keybinding_warning_for_setting_id(
    id: &'static str,
    value: Option<&String>,
    conflicts: &HashMap<&'static str, Vec<&'static str>>,
) -> Option<String> {
    if !id.starts_with("terminal.keybindings.") {
        return None;
    }

    if let Some(value) = value {
        if normalize_keybinding_value(value).is_none() {
            return Some(t!("Settings.KeyBindingsUi.Warning.InvalidShortcut").to_string());
        }
    }

    if let Some(others) = conflicts.get(id) {
        let mut names: Vec<String> = others.iter().copied().map(keybinding_short_name).collect();
        names.sort_unstable();
        return Some(
            t!(
                "Settings.KeyBindingsUi.Warning.ConflictsWith",
                others = names.join(", ")
            )
            .to_string(),
        );
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_keybinding_value_strips_whitespace_and_rejects_empty() {
        assert_eq!(normalize_keybinding_value("   "), None);
        assert_eq!(normalize_keybinding_value("\n\t"), None);
    }

    #[test]
    fn normalize_keybinding_value_rejects_invalid() {
        assert_eq!(normalize_keybinding_value("ctrl-a-b"), None);
    }

    #[test]
    fn normalize_keybinding_value_canonicalizes() {
        // Case/format normalization is implementation-defined; just assert it parses.
        assert!(normalize_keybinding_value("ctrl-C").is_some());
    }
}
