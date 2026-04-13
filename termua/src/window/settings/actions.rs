use std::time::Duration;

use gpui::{App, Context, FocusHandle, Window};
use gpui_component::input::InputState;
use rust_i18n::t;

use super::{
    SettingsWindow,
    state::{
        TerminalKeybinding, build_nav_tree_items, find_tree_item_by_id, nav_tree_item_id_for_page,
        page_for_nav_tree_item_id,
    },
};
use crate::settings::save_settings_to_disk;

impl SettingsWindow {
    fn sync_input_placeholders(
        window: &mut Window,
        cx: &mut Context<Self>,
        placeholders: &[(gpui::Entity<InputState>, String)],
    ) {
        for (input, placeholder) in placeholders {
            Self::set_input_placeholder(input, placeholder.clone(), window, cx);
        }
    }

    fn set_input_placeholder(
        input: &gpui::Entity<InputState>,
        placeholder: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        input.update(cx, |state, cx| {
            state.set_placeholder(placeholder, window, cx);
        });
    }

    fn sync_assistant_placeholders(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        Self::sync_input_placeholders(
            window,
            cx,
            &[
                (
                    self.assistant_temperature_input.clone(),
                    t!("Settings.Assistant.TemperaturePlaceholder").to_string(),
                ),
                (
                    self.assistant_api_url_input.clone(),
                    t!("Settings.Assistant.ApiUrlPlaceholder").to_string(),
                ),
                (
                    self.assistant_api_path_input.clone(),
                    t!("Settings.Assistant.ApiPathPlaceholder").to_string(),
                ),
                (
                    self.assistant_provider_timeout_input.clone(),
                    t!("Settings.Assistant.TimeoutPlaceholder").to_string(),
                ),
                (
                    self.assistant_extra_headers_input.clone(),
                    t!("Settings.Assistant.ExtraHeadersPlaceholder").to_string(),
                ),
                (
                    self.assistant_api_key_input.clone(),
                    t!("Settings.Assistant.ApiKeyPlaceholder").to_string(),
                ),
            ],
        );
    }

    pub(super) fn sync_localized_strings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.set_window_title(t!("Settings.WindowTitle").as_ref());

        self.lock_overlay.sync_localized_placeholders(window, cx);

        Self::sync_input_placeholders(
            window,
            cx,
            &[
                (
                    self.search_input.clone(),
                    t!("Settings.Search.Placeholder").to_string(),
                ),
                (
                    self.logging_path_input.clone(),
                    t!("Settings.Logging.PathPlaceholder").to_string(),
                ),
                (
                    self.sharing_relay_url_input.clone(),
                    t!("Settings.Sharing.RelayUrlPlaceholder").to_string(),
                ),
            ],
        );
        self.sync_assistant_placeholders(window, cx);

        let nav_tree_items = build_nav_tree_items();
        self.nav_tree_items = nav_tree_items.clone();

        self.nav_tree_state.update(cx, |tree, cx| {
            tree.set_items(nav_tree_items, cx);
        });
        self.sync_nav_tree_selection(cx);
    }

    pub(super) fn select_page_by_nav_id(&mut self, nav_id: &str, cx: &mut Context<Self>) -> bool {
        let Some(page) = page_for_nav_tree_item_id(nav_id) else {
            return false;
        };

        self.selected_page = page;
        self.settings.ui.last_settings_page = Some(nav_id.to_string());
        self.sync_nav_tree_selection(cx);
        self.save_later(cx);
        cx.notify();
        true
    }

    pub(super) fn unlock_from_overlay(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.lock_overlay.unlock_with_password(window, cx);
    }

    pub(super) fn apply_and_save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let prev_language = self.current_language;
        self.settings.apply_to_app(Some(window), cx);
        let next_language = self.settings.appearance.language;
        if prev_language != next_language {
            self.current_language = next_language;
            self.sync_localized_strings(window, cx);
        }
        self.save_later(cx);
        cx.notify();
        window.refresh();
    }

    pub(super) fn save_only(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.save_later(cx);
        cx.notify();
        window.refresh();
    }

    pub(super) fn apply_terminal_keybindings(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.settings.apply_terminal_keybindings(cx);
        self.save_only(window, cx);
    }

    pub(super) fn save_later(&mut self, cx: &mut Context<Self>) {
        self.save_epoch = self.save_epoch.wrapping_add(1);
        let epoch = self.save_epoch;

        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(300))
                .await;

            let _ = this.update(cx, |this, _cx| {
                if this.save_epoch != epoch {
                    return;
                }
                if let Err(err) = save_settings_to_disk(&this.settings) {
                    log::warn!("failed to save settings.json: {err:#}");
                }
            });
        })
        .detach();
    }

    pub(super) fn search_query(&self, cx: &App) -> String {
        self.search_input.read(cx).value().to_string()
    }

    pub(super) fn sync_nav_tree_selection(&mut self, cx: &mut Context<Self>) {
        let id = nav_tree_item_id_for_page(self.selected_page);
        let Some(item) = find_tree_item_by_id(&self.nav_tree_items, id) else {
            return;
        };

        self.nav_tree_state.update(cx, |tree, cx| {
            tree.set_selected_item(Some(item), cx);
        });
    }

    pub(super) fn terminal_keybinding_default_label(id: &'static str) -> &'static str {
        TerminalKeybinding::from_id(id)
            .map(|k| k.default_label())
            .unwrap_or("")
    }

    pub(super) fn terminal_keybinding_focus_handle(&self, id: &'static str) -> &FocusHandle {
        let Some(k) = TerminalKeybinding::from_id(id) else {
            return &self.focus_handle;
        };
        &self.terminal_keybinding_focus[k.index()]
    }

    pub(super) fn terminal_keybinding_value(&self, id: &'static str) -> Option<&String> {
        let k = TerminalKeybinding::from_id(id)?;
        match k {
            TerminalKeybinding::Copy => self.settings.terminal_keybindings.copy.as_ref(),
            TerminalKeybinding::Paste => self.settings.terminal_keybindings.paste.as_ref(),
            TerminalKeybinding::SelectAll => self.settings.terminal_keybindings.select_all.as_ref(),
            TerminalKeybinding::Clear => self.settings.terminal_keybindings.clear.as_ref(),
            TerminalKeybinding::Search => self.settings.terminal_keybindings.search.as_ref(),
            TerminalKeybinding::SearchNext => {
                self.settings.terminal_keybindings.search_next.as_ref()
            }
            TerminalKeybinding::SearchPrevious => {
                self.settings.terminal_keybindings.search_previous.as_ref()
            }
            TerminalKeybinding::IncreaseFontSize => self
                .settings
                .terminal_keybindings
                .increase_font_size
                .as_ref(),
            TerminalKeybinding::DecreaseFontSize => self
                .settings
                .terminal_keybindings
                .decrease_font_size
                .as_ref(),
            TerminalKeybinding::ResetFontSize => {
                self.settings.terminal_keybindings.reset_font_size.as_ref()
            }
        }
    }

    pub(super) fn set_terminal_keybinding_value(&mut self, id: &'static str, v: Option<String>) {
        let Some(k) = TerminalKeybinding::from_id(id) else {
            return;
        };
        match k {
            TerminalKeybinding::Copy => self.settings.terminal_keybindings.copy = v,
            TerminalKeybinding::Paste => self.settings.terminal_keybindings.paste = v,
            TerminalKeybinding::SelectAll => self.settings.terminal_keybindings.select_all = v,
            TerminalKeybinding::Clear => self.settings.terminal_keybindings.clear = v,
            TerminalKeybinding::Search => self.settings.terminal_keybindings.search = v,
            TerminalKeybinding::SearchNext => self.settings.terminal_keybindings.search_next = v,
            TerminalKeybinding::SearchPrevious => {
                self.settings.terminal_keybindings.search_previous = v
            }
            TerminalKeybinding::IncreaseFontSize => {
                self.settings.terminal_keybindings.increase_font_size = v
            }
            TerminalKeybinding::DecreaseFontSize => {
                self.settings.terminal_keybindings.decrease_font_size = v
            }
            TerminalKeybinding::ResetFontSize => {
                self.settings.terminal_keybindings.reset_font_size = v
            }
        }
    }
}
