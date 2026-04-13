use std::{collections::HashSet, sync::Arc};

use gpui::{
    AnyElement, App, ClickEvent, Context, CursorStyle, InteractiveElement, IntoElement,
    MouseButton, ParentElement, Render, Styled, Window, WindowControlArea, div,
    prelude::FluentBuilder, px,
};
use gpui_component::{
    ActiveTheme, Disableable, IconName, Sizable, StyledExt, TitleBar, WindowExt,
    button::{Button, ButtonVariants},
    h_flex,
    input::Input,
    menu::{DropdownMenu, PopupMenuItem},
    select::Select,
    switch::Switch,
    tree::{TreeEntry, tree},
    v_flex,
};
use rust_i18n::t;

struct TerminalKeybindingFieldState {
    clear_enabled: bool,
    display_text: String,
    text_color: gpui::Hsla,
    border_color: gpui::Hsla,
}

use super::{
    SettingsWindow,
    keybindings::{
        is_modifier_only_key, keybinding_clear_button_enabled, normalize_keybinding_value,
    },
    state::AssistantModelSelectItem,
};
use crate::{
    notification,
    settings::{Language, LogLevel, ThemeMode},
};

#[derive(Clone, Copy, Debug)]
enum ThemeDropdownKind {
    Light,
    Dark,
}

pub(super) fn next_stepped_value(current: f32, delta: f32, step: f32, min: f32, max: f32) -> f32 {
    (current + delta * step).clamp(min, max)
}

macro_rules! settings_supported_id_matches {
    ($id:expr) => {
        matches!(
            $id,
            "lock_screen.enabled"
                | "lock_screen.timeout_secs"
                | "appearance.theme"
                | "appearance.language"
                | "appearance.light_theme"
                | "appearance.dark_theme"
                | "terminal.default_backend"
                | "terminal.ssh_backend"
                | "terminal.font_family"
                | "terminal.keybindings.copy"
                | "terminal.keybindings.paste"
                | "terminal.keybindings.select_all"
                | "terminal.keybindings.clear"
                | "terminal.keybindings.search"
                | "terminal.keybindings.search_next"
                | "terminal.keybindings.search_previous"
                | "terminal.keybindings.increase_font_size"
                | "terminal.keybindings.decrease_font_size"
                | "terminal.keybindings.reset_font_size"
                | "terminal.font_size"
                | "terminal.ligatures"
                | "terminal.cursor_shape"
                | "terminal.blinking"
                | "terminal.show_scrollbar"
                | "terminal.show_line_numbers"
                | "terminal.option_as_meta"
                | "terminal.copy_on_select"
                | "terminal.sftp_upload_max_concurrency"
                | "terminal.suggestions_enabled"
                | "terminal.suggestions_max_items"
                | "terminal.suggestions_json_dir"
                | "sharing.enabled"
                | "sharing.relay_url"
                | "sharing.local_relay"
                | "recording.include_input_by_default"
                | "recording.playback_speed"
                | "logging.level"
                | "logging.path"
                | "assistant.provider"
                | "assistant.enabled"
                | "assistant.status"
                | "assistant.model"
                | "assistant.temperature"
                | "assistant.api_url"
                | "assistant.api_path"
                | "assistant.provider_timeout_secs"
                | "assistant.extra_headers"
                | "assistant.api_key"
        )
    };
}

impl SettingsWindow {
    fn check_zeroclaw_status(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.assistant_service_in_flight {
            return;
        }

        self.assistant_service_in_flight = true;
        self.assistant_service_error = None;

        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                let endpoint = termua_zeroclaw::Client::gateway_endpoint_blocking()?;
                let ok = termua_zeroclaw::Client::gateway_health_blocking(&endpoint)?;
                Ok::<_, anyhow::Error>((endpoint, ok))
            })
            .await;

            let _ = this.update(cx, |this, cx| {
                this.assistant_service_in_flight = false;
                match result {
                    Ok((endpoint, ok)) => {
                        this.assistant_gateway_endpoint = Some(endpoint);
                        this.assistant_service_alive = Some(ok);
                        this.assistant_service_error = None;
                    }
                    Err(err) => {
                        this.assistant_service_alive = Some(false);
                        this.assistant_service_error = Some(format!("{err:#}").into());
                    }
                }
                cx.notify();
            });
        })
        .detach();

        window.refresh();
    }

    fn ensure_zeroclaw_running(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.assistant_service_in_flight {
            return;
        }

        self.assistant_service_in_flight = true;
        self.assistant_service_error = None;

        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                let endpoint = termua_zeroclaw::Client::gateway_endpoint_blocking()?;
                if termua_zeroclaw::Client::gateway_health_blocking(&endpoint).unwrap_or(false) {
                    return Ok::<_, anyhow::Error>((None, endpoint, true));
                }

                let handle = termua_zeroclaw::Client::gateway_start_background_blocking()?;
                let handle_endpoint = handle.endpoint.clone();

                for _ in 0..10 {
                    if termua_zeroclaw::Client::gateway_health_blocking(&handle_endpoint)
                        .unwrap_or(false)
                    {
                        return Ok((Some(handle), handle_endpoint, true));
                    }
                    std::thread::sleep(std::time::Duration::from_millis(200));
                }

                Ok((Some(handle), handle_endpoint, false))
            })
            .await;

            let _ = this.update(cx, |this, cx| {
                this.assistant_service_in_flight = false;
                match result {
                    Ok((handle, endpoint, ok)) => {
                        this.assistant_gateway_endpoint = Some(endpoint);
                        this.assistant_service_alive = Some(ok);
                        this.assistant_service_error = None;
                        if let Some(handle) = handle {
                            this.assistant_gateway_handle = Some(handle);
                        }
                    }
                    Err(err) => {
                        this.assistant_service_alive = Some(false);
                        this.assistant_service_error = Some(format!("{err:#}").into());
                    }
                }
                cx.notify();
            });
        })
        .detach();

        window.refresh();
    }

    fn shutdown_zeroclaw(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.assistant_service_in_flight {
            return;
        }

        self.assistant_service_in_flight = true;
        self.assistant_service_error = None;

        let endpoint = self.assistant_gateway_endpoint.clone();
        let handle = self.assistant_gateway_handle.take();

        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                let endpoint =
                    endpoint.unwrap_or(termua_zeroclaw::Client::gateway_endpoint_blocking()?);

                let mut errors = Vec::<anyhow::Error>::new();

                if let Err(err) = termua_zeroclaw::Client::gateway_shutdown_blocking(&endpoint) {
                    errors.push(err);
                }

                // Stop the supervising daemon too, otherwise it can restart the gateway.
                if let Err(err) = termua_zeroclaw::Client::stop_daemon_blocking() {
                    errors.push(err);
                }

                let alive =
                    termua_zeroclaw::Client::gateway_health_blocking(&endpoint).unwrap_or(false);

                Ok::<_, anyhow::Error>((endpoint, alive, errors))
            })
            .await;

            let _ = this.update(cx, |this, cx| {
                this.assistant_service_in_flight = false;
                match result {
                    Ok((endpoint, alive, errors)) => {
                        this.assistant_gateway_endpoint = Some(endpoint);
                        this.assistant_service_alive = Some(alive);

                        if errors.is_empty() {
                            this.assistant_service_error = None;
                        } else {
                            this.assistant_service_error = Some(
                                errors
                                    .into_iter()
                                    .map(|e| format!("{e:#}"))
                                    .collect::<Vec<_>>()
                                    .join("\n\n")
                                    .into(),
                            );
                        }
                    }
                    Err(err) => {
                        this.assistant_service_error = Some(format!("{err:#}").into());
                    }
                }

                if let Some(handle) = handle {
                    // Join the gateway thread if we spawned it.
                    handle.join();
                }

                cx.notify();
            });
        })
        .detach();

        window.refresh();
    }

    fn reload_static_suggestions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.static_suggestions_reload_in_flight {
            return;
        }

        self.static_suggestions_reload_in_flight = true;
        let background = cx.background_executor().clone();

        cx.spawn_in(window, async move |this, window| {
            let task = background.spawn(async move {
                crate::static_suggestions::StaticSuggestionsDb::load_default()
            });
            let result = task.await;

            let _ = this.update_in(window, |this, window, cx| {
                this.static_suggestions_reload_in_flight = false;
                match result {
                    Ok(db) => {
                        gpui_term::set_suggestion_static_provider(cx, Some(Arc::new(db)));
                        notification::notify(
                            notification::MessageKind::Info,
                            "Reloaded suggestions.",
                            window,
                            cx,
                        );
                    }
                    Err(err) => {
                        notification::notify(
                            notification::MessageKind::Error,
                            format!("Reload suggestions failed: {err:#}"),
                            window,
                            cx,
                        );
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn fetch_model_list_if_needed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.assistant_model_fetch_in_flight {
            return;
        }

        // Only fetch when (a) no cached list yet, or (b) model is currently unset.
        let model_empty = self
            .settings
            .assistant
            .model
            .as_deref()
            .map(str::trim)
            .unwrap_or("")
            .is_empty();
        if !self.assistant_model_candidates.is_empty() && !model_empty {
            return;
        }

        self.assistant_model_fetch_in_flight = true;
        self.assistant_model_fetch_error = None;

        let assistant_settings = self.settings.assistant.clone();
        let window_handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let result = smol::unblock(move || {
                let api_key = crate::keychain::load_zeroclaw_api_key().ok().flatten();
                let opts = termua_zeroclaw::ClientOptions {
                    provider: assistant_settings.provider,
                    model: None,
                    api_key,
                    api_url: assistant_settings.api_url,
                    api_path: assistant_settings.api_path,
                    temperature: assistant_settings.temperature,
                    provider_timeout_secs: assistant_settings.provider_timeout_secs,
                    extra_headers: assistant_settings.extra_headers,
                };
                termua_zeroclaw::Client::list_models_blocking_with_options(opts)
            })
            .await;

            let _ = window_handle.update(cx, move |_, window, cx| {
                let _ = this.update(cx, |this, cx| {
                    this.assistant_model_fetch_in_flight = false;
                    match result {
                        Ok(models) => {
                            this.assistant_model_candidates =
                                models.into_iter().map(gpui::SharedString::from).collect();
                            this.assistant_model_fetch_error = None;
                        }
                        Err(err) => {
                            this.assistant_model_candidates.clear();
                            this.assistant_model_fetch_error = Some(format!("{err:#}").into());
                        }
                    }

                    // Keep the model dropdown in sync with cached candidates.
                    let mut items = Vec::with_capacity(this.assistant_model_candidates.len() + 2);
                    items.push(AssistantModelSelectItem::default_item());

                    let mut seen = HashSet::<String>::new();
                    for model in this.assistant_model_candidates.iter().cloned() {
                        let model_s = model.to_string();
                        if model_s.trim().is_empty() || !seen.insert(model_s.clone()) {
                            continue;
                        }
                        items.push(AssistantModelSelectItem::for_model(model));
                    }

                    // Ensure the currently-selected model remains present even if the provider
                    // doesn't report it.
                    if let Some(model) = this
                        .settings
                        .assistant
                        .model
                        .as_deref()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                    {
                        if seen.insert(model.to_string()) {
                            items.push(AssistantModelSelectItem::for_model(
                                model.to_string().into(),
                            ));
                        }
                    }

                    let selected_value: gpui::SharedString = this
                        .settings
                        .assistant
                        .model
                        .as_deref()
                        .map(str::trim)
                        .unwrap_or("")
                        .to_string()
                        .into();

                    this.assistant_model_select.update(cx, |select, cx| {
                        use gpui_component::select::SearchableVec;

                        select.set_items(SearchableVec::new(items), window, cx);
                        select.set_selected_value(&selected_value, window, cx);
                    });

                    cx.notify();
                });
            });
        })
        .detach();

        window.refresh();
    }

    fn on_select_logging_level(
        this: &mut SettingsWindow,
        level: LogLevel,
        window: &mut Window,
        cx: &mut Context<SettingsWindow>,
    ) {
        this.settings.logging.level = level;
        this.save_only(window, cx);
    }

    fn on_select_terminal_blink(
        this: &mut SettingsWindow,
        v: gpui_term::TerminalBlink,
        window: &mut Window,
        cx: &mut Context<SettingsWindow>,
    ) {
        this.settings.terminal.blinking = v;
        this.apply_and_save(window, cx);
    }

    fn cursor_shape_label_and_glyph(shape: gpui_term::CursorShape) -> (&'static str, &'static str) {
        match shape {
            // Match shapes to their visual equivalents.
            gpui_term::CursorShape::Block => ("Block", "█"),
            gpui_term::CursorShape::Underline => ("Underline", "_"),
            gpui_term::CursorShape::Bar => ("Bar", "⎸"),
            gpui_term::CursorShape::Hollow => ("Hollow", "▯"),
        }
    }

    fn on_select_terminal_cursor_shape(
        this: &mut SettingsWindow,
        shape: gpui_term::CursorShape,
        window: &mut Window,
        cx: &mut Context<SettingsWindow>,
    ) {
        this.settings.terminal.cursor_shape = Some(shape);
        this.apply_and_save(window, cx);
    }

    fn render_checked_dropdown_element_items<V: Copy + PartialEq + 'static>(
        &self,
        button: Button,
        current: V,
        items: &'static [(&'static str, V)],
        render_item: fn(&'static str, V, &mut Window, &mut App) -> AnyElement,
        on_select: fn(&mut SettingsWindow, V, &mut Window, &mut Context<SettingsWindow>),
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let this = cx.entity();
        button
            .dropdown_menu(move |menu, _window, _cx| {
                let mut menu = menu;
                for &(item_label, value) in items {
                    menu = menu.item(
                        PopupMenuItem::element(move |window, cx| {
                            render_item(item_label, value, window, cx)
                        })
                        .checked(current == value)
                        .on_click({
                            let this = this.clone();
                            move |_, window, cx| {
                                this.update(cx, |this, cx| {
                                    on_select(this, value, window, cx);
                                });
                            }
                        }),
                    );
                }
                menu
            })
            .into_any_element()
    }

    fn render_terminal_cursor_shape_dropdown_item(
        label: &'static str,
        shape: gpui_term::CursorShape,
        _window: &mut Window,
        _cx: &mut App,
    ) -> AnyElement {
        let (_, glyph) = Self::cursor_shape_label_and_glyph(shape);
        let selector = format!("termua-settings-terminal-cursor-shape-option-{label}-{glyph}");
        div()
            .debug_selector(move || selector)
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(div().w(px(16.)).text_center().child(glyph))
                    .child(div().child(label)),
            )
            .into_any_element()
    }

    fn render_terminal_font_size_stepper(
        &self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let this = cx.entity();

        let font_size = f32::from(&self.settings.terminal.font_size);
        let step = 1.0;
        let min_size = 6.0;
        let max_size = 64.0;

        let adjust = |delta: f32| {
            let this = this.clone();
            move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                this.update(cx, |this, cx| {
                    let current = f32::from(&this.settings.terminal.font_size);
                    let next = next_stepped_value(current, delta, step, min_size, max_size);
                    this.settings.terminal.font_size = px(next);
                    this.apply_and_save(window, cx);
                });
            }
        };

        h_flex()
            .items_center()
            .gap_1()
            .child(
                Button::new("settings-font-size-dec")
                    .label("-")
                    .on_click(adjust(-1.0)),
            )
            .child(
                div()
                    .w(px(56.))
                    .text_center()
                    .text_sm()
                    .child(format!("{font_size:.0}")),
            )
            .child(
                Button::new("settings-font-size-inc")
                    .label("+")
                    .on_click(adjust(1.0)),
            )
            .into_any_element()
    }

    fn render_checked_dropdown<V: Copy + PartialEq + 'static>(
        &self,
        button_id: &'static str,
        label: impl Into<gpui::SharedString>,
        current: V,
        items: &'static [(&'static str, V)],
        on_select: fn(&mut SettingsWindow, V, &mut Window, &mut Context<SettingsWindow>),
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let this = cx.entity();
        Button::new(button_id)
            .label(label)
            .dropdown_menu(move |menu, _window, _cx| {
                let mut menu = menu;
                for &(item_label, value) in items {
                    menu = menu.item(
                        PopupMenuItem::new(item_label)
                            .checked(current == value)
                            .on_click({
                                let this = this.clone();
                                move |_, window, cx| {
                                    this.update(cx, |this, cx| {
                                        on_select(this, value, window, cx);
                                    });
                                }
                            }),
                    );
                }
                menu
            })
            .into_any_element()
    }

    fn render_bool_switch<F>(
        &self,
        switch_id: &'static str,
        checked: bool,
        on_toggle: F,
        cx: &mut Context<Self>,
    ) -> AnyElement
    where
        F: Fn(&mut SettingsWindow, bool, &mut Window, &mut Context<SettingsWindow>)
            + Copy
            + 'static,
    {
        let this = cx.entity();
        Switch::new(switch_id)
            .checked(checked)
            .on_click(move |checked, window, cx| {
                let checked = *checked;
                this.update(cx, |this, cx| {
                    on_toggle(this, checked, window, cx);
                });
            })
            .into_any_element()
    }

    fn default_theme_name(kind: ThemeDropdownKind, cx: &App) -> String {
        let registry = gpui_component::ThemeRegistry::global(cx);
        match kind {
            ThemeDropdownKind::Light => registry.default_light_theme().name.to_string(),
            ThemeDropdownKind::Dark => registry.default_dark_theme().name.to_string(),
        }
    }

    fn render_theme_dropdown(
        &self,
        kind: ThemeDropdownKind,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let this = cx.entity();
        let current = match kind {
            ThemeDropdownKind::Light => self.settings.appearance.light_theme.clone(),
            ThemeDropdownKind::Dark => self.settings.appearance.dark_theme.clone(),
        };

        let default_name = Self::default_theme_name(kind, cx);
        let is_default_selected =
            current.is_none() || current.as_deref() == Some(default_name.as_str());
        let label = if is_default_selected {
            format!("{default_name}(Default)")
        } else {
            current
                .as_deref()
                .unwrap_or(default_name.as_str())
                .to_string()
        };

        let button_id = match kind {
            ThemeDropdownKind::Light => "settings-light-theme-dropdown",
            ThemeDropdownKind::Dark => "settings-dark-theme-dropdown",
        };

        let kind_is_dark = matches!(kind, ThemeDropdownKind::Dark);

        Button::new(button_id)
            .label(label)
            .dropdown_menu(move |menu, _window, cx| {
                let default_name = Self::default_theme_name(kind, cx);
                let theme_names = gpui_component::ThemeRegistry::global(cx)
                    .sorted_themes()
                    .into_iter()
                    .filter(|t| t.mode.is_dark() == kind_is_dark)
                    .map(|t| t.name.clone())
                    .filter(|name| name.as_ref() != default_name.as_str())
                    .collect::<Vec<_>>();

                let mut menu = menu.item(
                    PopupMenuItem::new(format!("{default_name}(Default)"))
                        .checked(
                            current.is_none() || current.as_deref() == Some(default_name.as_str()),
                        )
                        .on_click({
                            let this = this.clone();
                            move |_, window, cx| {
                                this.update(cx, |this, cx| {
                                    match kind {
                                        ThemeDropdownKind::Light => {
                                            this.settings.appearance.light_theme = None;
                                        }
                                        ThemeDropdownKind::Dark => {
                                            this.settings.appearance.dark_theme = None;
                                        }
                                    }
                                    this.apply_and_save(window, cx);
                                });
                            }
                        }),
                );

                for name in theme_names {
                    let checked = current.as_deref() == Some(name.as_ref());
                    menu = menu.item(PopupMenuItem::new(name.clone()).checked(checked).on_click({
                        let this = this.clone();
                        move |_, window, cx| {
                            let value = name.to_string();
                            this.update(cx, |this, cx| {
                                match kind {
                                    ThemeDropdownKind::Light => {
                                        this.settings.appearance.light_theme = Some(value);
                                    }
                                    ThemeDropdownKind::Dark => {
                                        this.settings.appearance.dark_theme = Some(value);
                                    }
                                }
                                this.apply_and_save(window, cx);
                            });
                        }
                    }));
                }

                menu
            })
            .into_any_element()
    }

    fn render_left_pane(&self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();

        v_flex()
            .id("termua-settings-left-pane")
            .w(px(280.))
            .min_h_0()
            .border_r_1()
            .border_color(cx.theme().border.opacity(0.6))
            .child(div().p_2().child(Input::new(&self.search_input)))
            .child(div().flex_1().min_h_0().child(tree(
                &self.nav_tree_state,
                move |ix, entry: &TreeEntry, selected, _window, _cx| {
                    let item = entry.item();
                    let item_id = item.id.clone();
                    let is_folder = entry.is_folder();

                    let mut row = crate::window::nav_tree::nav_tree_row(ix, entry, selected);

                    if is_folder {
                        row = row.font_medium();
                    }

                    let entity = entity.clone();
                    row = row.on_click(move |_, window, cx| {
                        let selected = entity.update(cx, |this, cx| {
                            this.select_page_by_nav_id(item_id.as_ref(), cx)
                        });
                        if selected {
                            window.refresh();
                        }
                    });

                    row
                },
            )))
    }

    pub(super) fn render_setting_row_with_warning(
        &self,
        title: impl IntoElement,
        description: impl IntoElement,
        control: impl IntoElement,
        warning: Option<String>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let mut row = v_flex()
            .gap_1()
            .py(px(10.))
            .border_b_1()
            .border_color(cx.theme().border.opacity(0.6))
            .child(
                h_flex()
                    .items_center()
                    .gap_4()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .text_color(cx.theme().foreground)
                            .whitespace_normal()
                            .child(title),
                    )
                    .child(div().flex_shrink_0().child(control)),
            )
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .whitespace_normal()
                    .child(description),
            );

        if let Some(warning) = warning {
            row = row.child(
                div()
                    .w_full()
                    .min_w_0()
                    .text_xs()
                    .text_color(cx.theme().warning)
                    .whitespace_normal()
                    .child(warning),
            );
        }

        row
    }

    fn render_terminal_keybinding(
        &self,
        id: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let focus_handle = self.terminal_keybinding_focus_handle(id).clone();
        let focused = focus_handle.is_focused(window);

        let field_state = self.terminal_keybinding_field_state(id, focused, cx);

        let focus_for_click = focus_handle.clone();
        let settings_focus_for_escape = self.focus_handle.clone();

        div()
            .id(format!("termua-settings-keybinding-field-{id}"))
            .w(px(300.))
            .h(px(28.))
            .px_2()
            .rounded_md()
            .border_1()
            .border_color(field_state.border_color)
            .bg(cx.theme().background.opacity(0.35))
            .text_sm()
            .text_color(field_state.text_color)
            .whitespace_normal()
            .track_focus(&focus_handle)
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |_, _ev: &gpui::MouseDownEvent, window, cx| {
                    // Focusing a handle can update entity/window state; defer to avoid re-entrant
                    // updates (gpui "already being updated" panic).
                    let focus_for_click = focus_for_click.clone();
                    window.defer(cx, move |window, cx| {
                        focus_for_click.focus(window, cx);
                    });
                    cx.stop_propagation();
                }),
            )
            .on_key_down(
                cx.listener(move |this, ev: &gpui::KeyDownEvent, window, cx| {
                    this.handle_terminal_keybinding_key_down(
                        id,
                        ev,
                        settings_focus_for_escape.clone(),
                        window,
                        cx,
                    );
                }),
            )
            .child(
                h_flex()
                    .items_center()
                    .w_full()
                    .gap_2()
                    .child(div().flex_1().min_w_0().child(field_state.display_text))
                    .child(
                        Button::new(format!("termua-settings-keybinding-clear-{id}"))
                            .icon(IconName::Close)
                            .xsmall()
                            .ghost()
                            .tab_stop(false)
                            .disabled(!field_state.clear_enabled)
                            .tooltip(t!("Settings.Button.Clear").to_string())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.clear_terminal_keybinding_and_refocus(id, window, cx);
                                cx.stop_propagation();
                            })),
                    ),
            )
            .into_any_element()
    }

    fn terminal_keybinding_field_state(
        &self,
        id: &'static str,
        focused: bool,
        cx: &Context<Self>,
    ) -> TerminalKeybindingFieldState {
        let clear_enabled = keybinding_clear_button_enabled(self.terminal_keybinding_value(id));
        let raw_value = self
            .terminal_keybinding_value(id)
            .map(String::as_str)
            .unwrap_or("");
        let display_value =
            normalize_keybinding_value(raw_value).unwrap_or_else(|| raw_value.to_string());

        let default_label = Self::terminal_keybinding_default_label(id);
        let display_text = if focused {
            if display_value.is_empty() {
                t!("Settings.KeyBindingsUi.PressKeys").to_string()
            } else {
                display_value
            }
        } else if display_value.is_empty() {
            t!("Settings.KeyBindingsUi.DefaultLabel", label = default_label).to_string()
        } else {
            display_value
        };

        let text_color = if self.terminal_keybinding_value(id).is_some() {
            cx.theme().foreground
        } else {
            cx.theme().muted_foreground
        };
        let border_color = if focused {
            cx.theme().accent
        } else {
            cx.theme().border.opacity(0.6)
        };

        TerminalKeybindingFieldState {
            clear_enabled,
            display_text,
            text_color,
            border_color,
        }
    }

    fn handle_terminal_keybinding_key_down(
        &mut self,
        id: &'static str,
        ev: &gpui::KeyDownEvent,
        settings_focus_for_escape: gpui::FocusHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if ev.is_held {
            return;
        }

        match ev.keystroke.key.as_str() {
            "escape" => {
                window.defer(cx, move |window, cx| {
                    settings_focus_for_escape.focus(window, cx)
                });
                cx.stop_propagation();
                return;
            }
            "backspace" | "delete" => {
                self.set_terminal_keybinding_value(id, None);
                self.apply_terminal_keybindings(window, cx);
                cx.stop_propagation();
                return;
            }
            key if is_modifier_only_key(key) => return,
            _ => {}
        }

        let captured = ev.keystroke.unparse();
        let Some(value) = normalize_keybinding_value(&captured) else {
            return;
        };

        self.set_terminal_keybinding_value(id, Some(value));
        self.apply_terminal_keybindings(window, cx);
        cx.stop_propagation();
    }

    fn clear_terminal_keybinding_and_refocus(
        &mut self,
        id: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_terminal_keybinding_value(id, None);
        self.apply_terminal_keybindings(window, cx);
        let focus = self.terminal_keybinding_focus_handle(id).clone();
        window.defer(cx, move |window, cx| focus.focus(window, cx));
    }

    fn render_lock_screen_enabled_control(&self, cx: &mut Context<Self>) -> AnyElement {
        let supported = cx
            .global::<crate::lock_screen::LockState>()
            .locking_supported();
        let this = cx.entity();
        div()
            .debug_selector(|| "termua-settings-lock-screen-enabled-switch".to_string())
            .child(
                Switch::new("termua-settings-lock-screen-enabled")
                    .checked(self.settings.lock_screen.enabled)
                    .disabled(!supported)
                    .on_click(move |checked, window, cx| {
                        let checked = *checked;
                        this.update(cx, |this, cx| {
                            this.settings.lock_screen.enabled = checked;
                            this.apply_and_save(window, cx);
                        });
                    }),
            )
            .into_any_element()
    }

    fn render_lock_screen_timeout_control(&self, cx: &mut Context<Self>) -> AnyElement {
        let supported = cx
            .global::<crate::lock_screen::LockState>()
            .locking_supported();
        let enabled = self.settings.lock_screen.enabled && supported;
        let current = self.settings.lock_screen.timeout_secs;
        let this = cx.entity();

        const ITEMS: &[(&str, u64)] = &[
            ("Never", 0),
            ("1 minute", 60),
            ("5 minutes", 5 * 60),
            ("10 minutes", 10 * 60),
            ("15 minutes", 15 * 60),
            ("30 minutes", 30 * 60),
            ("1 hour", 60 * 60),
        ];

        let label = ITEMS
            .iter()
            .find(|(_, v)| *v == current)
            .map(|(l, _)| *l)
            .unwrap_or("5 minutes");

        div()
            .debug_selector(|| "termua-settings-lock-screen-timeout-dropdown".to_string())
            .child(
                Button::new("termua-settings-lock-screen-timeout")
                    .label(label)
                    .disabled(!enabled)
                    .dropdown_menu(move |menu, _window, _cx| {
                        let mut menu = menu;
                        for &(item_label, value) in ITEMS {
                            menu = menu.item(
                                PopupMenuItem::new(item_label)
                                    .checked(current == value)
                                    .on_click({
                                        let this = this.clone();
                                        move |_, window, cx| {
                                            this.update(cx, |this, cx| {
                                                this.settings.lock_screen.timeout_secs = value;
                                                this.apply_and_save(window, cx);
                                            });
                                        }
                                    }),
                            );
                        }
                        menu
                    }),
            )
            .into_any_element()
    }

    fn render_appearance_theme_control(&self, cx: &mut Context<Self>) -> AnyElement {
        let current = self.settings.appearance.theme;
        let label = match current {
            ThemeMode::System => t!("Settings.ThemeMode.System").to_string(),
            ThemeMode::Light => t!("Settings.ThemeMode.Light").to_string(),
            ThemeMode::Dark => t!("Settings.ThemeMode.Dark").to_string(),
        };
        let this = cx.entity();

        Button::new("settings-theme-dropdown")
            .label(label)
            .dropdown_menu(move |menu, _window, _cx| {
                let mut menu = menu;
                for (item_label, value) in [
                    ("Settings.ThemeMode.System", ThemeMode::System),
                    ("Settings.ThemeMode.Light", ThemeMode::Light),
                    ("Settings.ThemeMode.Dark", ThemeMode::Dark),
                ] {
                    menu = menu.item(
                        PopupMenuItem::new(t!(item_label).to_string())
                            .checked(current == value)
                            .on_click({
                                let this = this.clone();
                                move |_, window, cx| {
                                    this.update(cx, |this, cx| {
                                        this.settings.appearance.theme = value;
                                        this.apply_and_save(window, cx);
                                    });
                                }
                            }),
                    );
                }
                menu
            })
            .into_any_element()
    }

    fn render_appearance_language_control(&self, cx: &mut Context<Self>) -> AnyElement {
        let current = self.settings.appearance.language;
        let label = match current {
            Language::English => t!("Settings.Language.English").to_string(),
            Language::ZhCn => t!("Settings.Language.ZhCn").to_string(),
        };
        let this = cx.entity();

        Button::new("settings-language-dropdown")
            .label(label)
            .dropdown_menu(move |menu, _window, _cx| {
                let mut menu = menu;
                for (item_label, value) in [
                    ("Settings.Language.English", Language::English),
                    ("Settings.Language.ZhCn", Language::ZhCn),
                ] {
                    menu = menu.item(
                        PopupMenuItem::new(t!(item_label).to_string())
                            .checked(current == value)
                            .on_click({
                                let this = this.clone();
                                move |_, window, cx| {
                                    this.update(cx, |this, cx| {
                                        this.settings.appearance.language = value;
                                        this.apply_and_save(window, cx);
                                    });
                                }
                            }),
                    );
                }
                menu
            })
            .into_any_element()
    }

    fn render_terminal_sftp_upload_max_concurrency_control(
        &self,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let current = self.settings.terminal.sftp_upload_max_concurrency;
        let this = cx.entity();

        const ITEMS: &[usize] = &[2, 5, 10, 15];
        let label = current.to_string();

        div()
            .debug_selector(|| "termua-settings-terminal-sftp-upload-max-concurrency".to_string())
            .child(
                Button::new("termua-settings-terminal-sftp-upload-max-concurrency-button")
                    .label(label)
                    .dropdown_menu(move |menu, _window, _cx| {
                        let mut menu = menu;
                        for &value in ITEMS {
                            menu = menu.item(
                                PopupMenuItem::new(value.to_string())
                                    .checked(current == value)
                                    .on_click({
                                        let this = this.clone();
                                        move |_, window, cx| {
                                            this.update(cx, |this, cx| {
                                                this.settings
                                                    .terminal
                                                    .sftp_upload_max_concurrency = value;
                                                this.apply_and_save(window, cx);
                                            });
                                        }
                                    }),
                            );
                        }
                        menu
                    }),
            )
            .into_any_element()
    }

    fn render_terminal_suggestions_max_items_control(&self, cx: &mut Context<Self>) -> AnyElement {
        let enabled = self.settings.terminal.suggestions_enabled;
        let current = self.settings.terminal.suggestions_max_items;
        let this = cx.entity();

        const ITEMS: &[usize] = &[4, 6, 8, 12, 16];
        let label = current.to_string();

        div()
            .debug_selector(|| "termua-settings-terminal-suggestions-max-items".to_string())
            .child(
                Button::new("termua-settings-terminal-suggestions-max-items-button")
                    .label(label)
                    .disabled(!enabled)
                    .dropdown_menu(move |menu, _window, _cx| {
                        let mut menu = menu;
                        for &value in ITEMS {
                            menu = menu.item(
                                PopupMenuItem::new(value.to_string())
                                    .checked(current == value)
                                    .on_click({
                                        let this = this.clone();
                                        move |_, window, cx| {
                                            this.update(cx, |this, cx| {
                                                this.settings.terminal.suggestions_max_items =
                                                    value;
                                                this.apply_and_save(window, cx);
                                            });
                                        }
                                    }),
                            );
                        }
                        menu
                    }),
            )
            .into_any_element()
    }

    fn render_terminal_suggestions_json_dir_control(&self, cx: &mut Context<Self>) -> AnyElement {
        let this = cx.entity();
        let in_flight = self.static_suggestions_reload_in_flight;

        h_flex()
            .gap_2()
            .items_center()
            .child(
                Button::new("termua-settings-static-suggestions-reload")
                    .debug_selector(|| "termua-settings-static-suggestions-reload".to_string())
                    .label(t!("Settings.Button.Reload").to_string())
                    .disabled(in_flight)
                    .on_click(move |_ev, window, cx| {
                        this.update(cx, |this, cx| {
                            this.reload_static_suggestions(window, cx);
                        });
                    }),
            )
            .into_any_element()
    }

    fn render_logging_level_control(&self, cx: &mut Context<Self>) -> AnyElement {
        let current = self.settings.logging.level;
        let label = match current {
            LogLevel::Default => "Default",
            LogLevel::Error => "Error",
            LogLevel::Warn => "Warn",
            LogLevel::Info => "Info",
            LogLevel::Debug => "Debug",
            LogLevel::Trace => "Trace",
            LogLevel::Off => "Off",
        };

        const ITEMS: &[(&str, LogLevel)] = &[
            ("Default", LogLevel::Default),
            ("Error", LogLevel::Error),
            ("Warn", LogLevel::Warn),
            ("Info", LogLevel::Info),
            ("Debug", LogLevel::Debug),
            ("Trace", LogLevel::Trace),
            ("Off", LogLevel::Off),
        ];

        self.render_checked_dropdown(
            "settings-logging-level",
            label,
            current,
            ITEMS,
            Self::on_select_logging_level,
            cx,
        )
    }

    fn render_assistant_enabled_control(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .debug_selector(|| "termua-settings-assistant-enabled-switch".to_string())
            .child(self.render_bool_switch(
                "termua-settings-assistant-enabled",
                self.settings.assistant.enabled,
                |this, checked, window, cx| {
                    let settings_entity = cx.entity();

                    this.settings.assistant.enabled = checked;
                    this.settings.apply_assistant_settings(cx);
                    this.save_only(window, cx);

                    if checked {
                        // If enabled and zeroclaw isn't running, pull it up.
                        this.assistant_service_bootstrap_done = true;
                        this.ensure_zeroclaw_running(window, cx);
                    } else {
                        this.assistant_service_bootstrap_done = false;
                        // Ask whether to also shut down the zeroclaw service.
                        let Some(Some(root)) = window.root::<gpui_component::Root>() else {
                            return;
                        };

                        root.update(cx, |root, cx| {
                            root.open_dialog(
                                move |dialog, _window, _cx| {
                                    use gpui_component::dialog::DialogButtonProps;

                                    dialog
                                        .confirm()
                                        .title(
                                            t!("Settings.Assistant.DisableZeroClawTitle")
                                                .to_string(),
                                        )
                                        .child(
                                            div().child(
                                                t!("Settings.Assistant.DisableZeroClawPrompt")
                                                    .to_string(),
                                            ),
                                        )
                                        .button_props(
                                            DialogButtonProps::default()
                                                .ok_text(
                                                    t!("Settings.Assistant.\
                                                        DisableZeroClawStopDaemon")
                                                    .to_string(),
                                                )
                                                .cancel_text(
                                                    t!("Settings.Assistant.\
                                                        DisableZeroClawKeepRunning")
                                                    .to_string(),
                                                ),
                                        )
                                        .on_ok({
                                            let settings_entity = settings_entity.clone();
                                            move |_ev, window, cx| {
                                                settings_entity.update(cx, |this, cx| {
                                                    this.shutdown_zeroclaw(window, cx);
                                                });
                                                true
                                            }
                                        })
                                },
                                window,
                                cx,
                            );
                        });
                    }

                    cx.notify();
                },
                cx,
            ))
            .into_any_element()
    }

    fn render_assistant_status_control(&self, cx: &mut Context<Self>) -> AnyElement {
        let in_flight = self.assistant_service_in_flight;
        let alive = self.assistant_service_alive;
        let this = cx.entity();

        let (label, color) = match alive {
            Some(true) => ("Running", cx.theme().success),
            Some(false) => ("Not running", cx.theme().danger),
            None => ("Unknown", cx.theme().muted_foreground),
        };

        v_flex()
            .w(px(240.))
            .gap_1()
            .debug_selector(|| "termua-settings-assistant-status-indicator".to_string())
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(div().w(px(10.)).h(px(10.)).rounded_full().bg(color))
                    .child(div().text_sm().child(label))
                    .child(div().flex_1())
                    .child(
                        Button::new("termua-settings-assistant-status-check")
                            .label(if in_flight { "Checking..." } else { "Check" })
                            .small()
                            .disabled(in_flight)
                            .on_click({
                                move |_, window, cx| {
                                    this.update(cx, |this, cx| {
                                        this.check_zeroclaw_status(window, cx);
                                    });
                                }
                            }),
                    ),
            )
            .into_any_element()
    }

    fn render_assistant_api_key_control(&self, cx: &mut Context<Self>) -> AnyElement {
        let this = cx.entity();
        h_flex()
            .gap_1()
            .items_center()
            .child(
                div()
                    .w(px(320.))
                    .child(Input::new(&self.assistant_api_key_input).cleanable(true)),
            )
            .child(
                Button::new("termua-settings-assistant-api-key-save")
                    .label(t!("Settings.Button.Save").to_string())
                    .small()
                    .on_click({
                        let this = this.clone();
                        move |_, window, cx| {
                            this.update(cx, |this, cx| {
                                let value =
                                    this.assistant_api_key_input.read(cx).value().to_string();
                                let value = value.trim().to_string();
                                if value.is_empty() {
                                    return;
                                }

                                match crate::keychain::store_zeroclaw_api_key(&value) {
                                    Ok(()) => {
                                        this.assistant_api_key_input.update(cx, |state, cx| {
                                            state.set_value("", window, cx);
                                        });
                                    }
                                    Err(err) => {
                                        log::warn!("failed to store assistant api key: {err:#}");
                                    }
                                }
                            });
                        }
                    }),
            )
            .child(
                Button::new("termua-settings-assistant-api-key-clear")
                    .label(t!("Settings.Button.Clear").to_string())
                    .small()
                    .ghost()
                    .on_click({
                        move |_, window, cx| {
                            this.update(cx, |this, cx| {
                                if let Err(err) = crate::keychain::delete_zeroclaw_api_key() {
                                    log::warn!("failed to delete assistant api key: {err:#}");
                                }
                                this.assistant_api_key_input.update(cx, |state, cx| {
                                    state.set_value("", window, cx);
                                });
                            });
                        }
                    }),
            )
            .into_any_element()
    }

    pub(super) fn render_control_for_setting(
        &self,
        id: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if let Some(control) = self.render_control_for_lock_screen(id, cx) {
            return control;
        }
        if let Some(control) = self.render_control_for_appearance(id, window, cx) {
            return control;
        }
        if let Some(control) = self.render_control_for_terminal(id, window, cx) {
            return control;
        }
        if let Some(control) = self.render_control_for_sharing(id, window, cx) {
            return control;
        }
        if let Some(control) = self.render_control_for_recording(id, window, cx) {
            return control;
        }
        if let Some(control) = self.render_control_for_logging(id, window, cx) {
            return control;
        }
        if let Some(control) = self.render_control_for_assistant(id, window, cx) {
            return control;
        }

        debug_assert!(
            !settings_supported_id_matches!(id),
            "supported setting id missing render_control_for_setting arm: {id}"
        );
        div()
            .child(t!("Settings.Unsupported").to_string())
            .into_any_element()
    }

    fn render_control_for_lock_screen(
        &self,
        id: &'static str,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        match id {
            "lock_screen.enabled" => Some(self.render_lock_screen_enabled_control(cx)),
            "lock_screen.timeout_secs" => Some(self.render_lock_screen_timeout_control(cx)),
            _ => None,
        }
    }

    fn render_control_for_appearance(
        &self,
        id: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        match id {
            "appearance.theme" => Some(self.render_appearance_theme_control(cx)),
            "appearance.language" => Some(self.render_appearance_language_control(cx)),
            "appearance.light_theme" => {
                Some(self.render_theme_dropdown(ThemeDropdownKind::Light, window, cx))
            }
            "appearance.dark_theme" => {
                Some(self.render_theme_dropdown(ThemeDropdownKind::Dark, window, cx))
            }
            _ => None,
        }
    }

    fn render_control_for_terminal(
        &self,
        id: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if matches!(
            id,
            "terminal.keybindings.copy"
                | "terminal.keybindings.paste"
                | "terminal.keybindings.select_all"
                | "terminal.keybindings.clear"
                | "terminal.keybindings.search"
                | "terminal.keybindings.search_next"
                | "terminal.keybindings.search_previous"
                | "terminal.keybindings.increase_font_size"
                | "terminal.keybindings.decrease_font_size"
                | "terminal.keybindings.reset_font_size"
        ) {
            return Some(self.render_terminal_keybinding(id, window, cx));
        }

        match id {
            "terminal.default_backend" => Some(
                div()
                    .w(px(240.))
                    .debug_selector(|| {
                        "termua-settings-terminal-default-backend-select".to_string()
                    })
                    .child(Select::new(&self.terminal_default_backend_select))
                    .into_any_element(),
            ),
            "terminal.ssh_backend" => Some(
                div()
                    .w(px(240.))
                    .debug_selector(|| "termua-settings-terminal-ssh-backend-select".to_string())
                    .child(Select::new(&self.terminal_ssh_backend_select))
                    .into_any_element(),
            ),
            "terminal.font_family" => Some(
                div()
                    .w(px(240.))
                    .debug_selector(|| "termua-settings-terminal-font-family-select".to_string())
                    .child(Select::new(&self.font_family_select))
                    .into_any_element(),
            ),
            "terminal.font_size" => Some(self.render_terminal_font_size_stepper(window, cx)),
            "terminal.ligatures" => Some(
                div()
                    .debug_selector(|| "termua-settings-terminal-ligatures-switch".to_string())
                    .child(self.render_bool_switch(
                        "settings-terminal-ligatures",
                        self.settings.terminal.ligatures_enabled(),
                        |this, checked, window, cx| {
                            this.settings.terminal.set_ligatures_enabled(checked);
                            this.apply_and_save(window, cx);
                        },
                        cx,
                    ))
                    .into_any_element(),
            ),
            "terminal.cursor_shape" => Some(self.render_terminal_cursor_shape_control(cx)),
            "terminal.blinking" => Some(self.render_terminal_blinking_control(cx)),
            "terminal.show_scrollbar" => Some(self.render_bool_switch(
                "settings-show-scrollbar",
                self.settings.terminal.show_scrollbar,
                |this, checked, window, cx| {
                    this.settings.terminal.show_scrollbar = checked;
                    this.apply_and_save(window, cx);
                },
                cx,
            )),
            "terminal.show_line_numbers" => Some(self.render_bool_switch(
                "settings-show-line-numbers",
                self.settings.terminal.show_line_numbers,
                |this, checked, window, cx| {
                    this.settings.terminal.show_line_numbers = checked;
                    this.apply_and_save(window, cx);
                },
                cx,
            )),
            "terminal.option_as_meta" => Some(self.render_bool_switch(
                "settings-option-as-meta",
                self.settings.terminal.option_as_meta,
                |this, checked, window, cx| {
                    this.settings.terminal.option_as_meta = checked;
                    this.apply_and_save(window, cx);
                },
                cx,
            )),
            "terminal.copy_on_select" => Some(self.render_bool_switch(
                "settings-copy-on-select",
                self.settings.terminal.copy_on_select,
                |this, checked, window, cx| {
                    this.settings.terminal.copy_on_select = checked;
                    this.apply_and_save(window, cx);
                },
                cx,
            )),
            "terminal.sftp_upload_max_concurrency" => {
                Some(self.render_terminal_sftp_upload_max_concurrency_control(cx))
            }
            "terminal.suggestions_enabled" => Some(self.render_bool_switch(
                "settings-suggestions-enabled",
                self.settings.terminal.suggestions_enabled,
                |this, checked, window, cx| {
                    this.settings.terminal.suggestions_enabled = checked;
                    this.apply_and_save(window, cx);
                },
                cx,
            )),
            "terminal.suggestions_max_items" => {
                Some(self.render_terminal_suggestions_max_items_control(cx))
            }
            "terminal.suggestions_json_dir" => {
                Some(self.render_terminal_suggestions_json_dir_control(cx))
            }
            _ => None,
        }
    }

    fn render_control_for_sharing(
        &self,
        id: &'static str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        match id {
            "sharing.enabled" => Some(self.render_bool_switch(
                "settings-sharing-enabled",
                self.settings.sharing.enabled,
                |this, checked, window, cx| {
                    this.settings.sharing.enabled = checked;
                    this.apply_and_save(window, cx);
                },
                cx,
            )),
            "sharing.relay_url" => Some(
                div()
                    .w(px(420.))
                    .child(
                        Input::new(&self.sharing_relay_url_input)
                            .cleanable(true)
                            .disabled(!self.settings.sharing.enabled),
                    )
                    .into_any_element(),
            ),
            "sharing.local_relay" => {
                let this = cx.entity();
                let running = crate::sharing::local_relay_running(cx);
                let state_id = "settings-sharing-local-relay";

                Some(
                    Switch::new(state_id)
                        .checked(running)
                        .on_click(move |checked, window, cx| {
                            let want_running = *checked;
                            this.update(cx, |this, cx| {
                                let relay_url =
                                    this.settings.sharing.relay_url.clone().unwrap_or_else(|| {
                                        crate::sharing::DEFAULT_RELAY_URL.to_string()
                                    });

                                if want_running {
                                    match crate::sharing::local_relay_listen_addr_from_ws_url(
                                        &relay_url,
                                    ) {
                                        Ok(listen) => {
                                            if let Err(err) =
                                                crate::sharing::start_local_relay(&listen, cx)
                                            {
                                                notification::notify_deferred(
                                                    notification::MessageKind::Error,
                                                    format!("Start local relay failed: {err:#}"),
                                                    window,
                                                    cx,
                                                );
                                            }
                                        }
                                        Err(err) => {
                                            notification::notify_deferred(
                                                notification::MessageKind::Warning,
                                                format!(
                                                    "Relay URL is not usable for local relay: \
                                                     {err:#}"
                                                ),
                                                window,
                                                cx,
                                            );
                                        }
                                    }
                                } else {
                                    crate::sharing::stop_local_relay(cx);
                                }

                                cx.notify();
                            });
                            window.refresh();
                        })
                        .into_any_element(),
                )
            }
            _ => None,
        }
    }

    fn render_terminal_cursor_shape_control(&self, cx: &mut Context<Self>) -> AnyElement {
        let cursor_shape = self.settings.terminal.cursor_shape.unwrap_or_default();
        let (cursor_label, cursor_glyph) = Self::cursor_shape_label_and_glyph(cursor_shape);
        let button_label = format!("{cursor_glyph} {cursor_label}");
        let button_selector =
            format!("termua-settings-terminal-cursor-shape-button-{cursor_label}-{cursor_glyph}");

        const ITEMS: &[(&str, gpui_term::CursorShape)] = &[
            ("Block", gpui_term::CursorShape::Block),
            ("Underline", gpui_term::CursorShape::Underline),
            ("Bar", gpui_term::CursorShape::Bar),
            ("Hollow", gpui_term::CursorShape::Hollow),
        ];

        self.render_checked_dropdown_element_items(
            Button::new("settings-cursor-shape")
                .debug_selector(move || button_selector)
                .label(button_label),
            cursor_shape,
            ITEMS,
            Self::render_terminal_cursor_shape_dropdown_item,
            Self::on_select_terminal_cursor_shape,
            cx,
        )
    }

    fn render_terminal_blinking_control(&self, cx: &mut Context<Self>) -> AnyElement {
        let blinking = self.settings.terminal.blinking;
        let blink_label = match blinking {
            gpui_term::TerminalBlink::Off => "Off",
            gpui_term::TerminalBlink::TerminalControlled => "Terminal Controlled",
            gpui_term::TerminalBlink::On => "On",
        };

        const ITEMS: &[(&str, gpui_term::TerminalBlink)] = &[
            ("On", gpui_term::TerminalBlink::On),
            (
                "Terminal Controlled",
                gpui_term::TerminalBlink::TerminalControlled,
            ),
            ("Off", gpui_term::TerminalBlink::Off),
        ];

        self.render_checked_dropdown(
            "settings-cursor-blink",
            blink_label,
            blinking,
            ITEMS,
            Self::on_select_terminal_blink,
            cx,
        )
    }

    fn render_control_for_recording(
        &self,
        id: &'static str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        match id {
            "recording.include_input_by_default" => Some(self.render_bool_switch(
                "settings-recording-include-input",
                self.settings.recording.include_input_by_default,
                |this, checked, window, cx| {
                    this.settings.recording.include_input_by_default = checked;
                    this.apply_and_save(window, cx);
                },
                cx,
            )),
            "recording.playback_speed" => Some(
                div()
                    .w(px(120.))
                    .debug_selector(|| {
                        "termua-settings-recording-playback-speed-select".to_string()
                    })
                    .child(Select::new(&self.recording_playback_speed_select))
                    .into_any_element(),
            ),
            _ => None,
        }
    }

    fn render_control_for_logging(
        &self,
        id: &'static str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        match id {
            "logging.level" => Some(self.render_logging_level_control(cx)),
            "logging.path" => Some(
                div()
                    .w(px(420.))
                    .child(Input::new(&self.logging_path_input).cleanable(true))
                    .into_any_element(),
            ),
            _ => None,
        }
    }

    fn render_control_for_assistant(
        &self,
        id: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        match id {
            "assistant.provider" => Some(
                div()
                    .w(px(240.))
                    .debug_selector(|| "termua-settings-assistant-provider-select".to_string())
                    .child(Select::new(&self.assistant_provider_select))
                    .into_any_element(),
            ),
            "assistant.enabled" => Some(self.render_assistant_enabled_control(cx)),
            "assistant.status" => Some(self.render_assistant_status_control(cx)),
            "assistant.model" => Some(self.render_assistant_model_control(window, cx)),
            "assistant.temperature" => Some(
                div()
                    .w(px(240.))
                    .child(Input::new(&self.assistant_temperature_input).cleanable(true))
                    .into_any_element(),
            ),
            "assistant.api_url" => Some(
                div()
                    .w(px(420.))
                    .child(Input::new(&self.assistant_api_url_input).cleanable(true))
                    .into_any_element(),
            ),
            "assistant.api_path" => Some(
                div()
                    .w(px(420.))
                    .child(Input::new(&self.assistant_api_path_input).cleanable(true))
                    .into_any_element(),
            ),
            "assistant.provider_timeout_secs" => Some(
                div()
                    .w(px(240.))
                    .child(Input::new(&self.assistant_provider_timeout_input).cleanable(true))
                    .into_any_element(),
            ),
            "assistant.extra_headers" => Some(
                div()
                    .w(px(420.))
                    .child(Input::new(&self.assistant_extra_headers_input).cleanable(true))
                    .into_any_element(),
            ),
            "assistant.api_key" => Some(self.render_assistant_api_key_control(cx)),
            _ => None,
        }
    }

    fn render_assistant_model_control(
        &self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .w(px(240.))
            .debug_selector(|| "termua-settings-assistant-model-dropdown-button".to_string())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    // Lazy fetch: only when model is unset or we have no cached list yet.
                    this.fetch_model_list_if_needed(window, cx);
                }),
            )
            .child(Select::new(&self.assistant_model_select))
            .into_any_element()
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // One-shot status bootstrap for the Assistant page: when the user opens the Assistant
        // settings page and the assistant is enabled, refresh liveness once without requiring
        // a manual "Check". Avoid spawning background tasks in tests (gpui test scheduler).
        #[cfg(not(test))]
        if self.settings.assistant.enabled
            && self.selected_page == super::SettingsPage::Assistant
            && !self.assistant_service_bootstrap_done
            && !self.assistant_service_in_flight
        {
            self.assistant_service_bootstrap_done = true;
            let this = cx.entity();
            window.defer(cx, move |window, cx| {
                this.update(cx, |this, cx| {
                    this.ensure_zeroclaw_running(window, cx);
                });
            });
        }

        let has_root = window.root::<gpui_component::Root>().flatten().is_some();
        let drag_overlay = (has_root && window.has_active_sheet(cx)).then(|| {
            let left_inset = if cfg!(target_os = "macos") {
                // Leave room for macOS traffic-light window controls.
                px(84.)
            } else {
                px(0.)
            };
            let right_inset = if cfg!(target_os = "macos") {
                px(0.)
            } else {
                // Leave room for right-side window controls.
                px(140.)
            };

            div()
                .absolute()
                .top_0()
                .left(left_inset)
                .right(right_inset)
                .h(gpui_component::TITLE_BAR_HEIGHT)
                .debug_selector(|| "termua-settings-sheet-drag-overlay".to_string())
                .when(cfg!(windows), |this| {
                    // On Windows, `start_window_move()` doesn't reliably work when a sheet is
                    // active; marking a region as draggable is more reliable.
                    this.window_control_area(WindowControlArea::Drag)
                })
                .when(!cfg!(windows), |this| {
                    // On Linux (client decorations), the overlay sits above the titlebar, so we
                    // manually start the window move.
                    this.cursor(CursorStyle::OpenHand).on_mouse_down(
                        MouseButton::Left,
                        |_ev, window, cx| {
                            window.prevent_default();
                            cx.stop_propagation();

                            // `start_window_move` is unimplemented in gpui's test window.
                            #[cfg(not(test))]
                            window.start_window_move();
                        },
                    )
                })
        });

        let lock_overlay = self
            .lock_overlay
            .render_overlay_if_locked(Self::unlock_from_overlay, cx);

        v_flex()
            .id("termua-settings-window")
            .size_full()
            .bg(cx.theme().background)
            .relative()
            // Treat any interaction in this window as activity for the lock timer.
            .on_any_mouse_down(|_ev, _window, cx| {
                cx.global::<crate::lock_screen::LockState>()
                    .report_activity();
            })
            .on_mouse_move(|_ev, _window, cx| {
                cx.global::<crate::lock_screen::LockState>()
                    .report_activity();
            })
            .on_key_down(cx.listener(|_this, ev: &gpui::KeyDownEvent, _window, cx| {
                if ev.is_held {
                    return;
                }
                cx.global::<crate::lock_screen::LockState>()
                    .report_activity();
            }))
            // Keep a normal titlebar, but only the main window shows the in-window menubar.
            .child(
                TitleBar::new().child(
                    h_flex()
                        .id("termua-settings-titlebar-left")
                        .items_center()
                        .gap_x_1()
                        .child(
                            div()
                                // Used by tests (noop in release builds).
                                .debug_selector(|| "termua-settings-titlebar-icon".to_string())
                                .child(gpui_component::Icon::new(IconName::Settings).small()),
                        ),
                ),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    // `h_flex()` defaults to `items_center()`, which causes panes to size to their
                    // content height (breaking scroll because the scroll container can grow).
                    // We need the panes to stretch to the available height.
                    .items_stretch()
                    .child(self.render_left_pane(window, cx))
                    .child(self.render_right_pane(window, cx)),
            )
            .children(gpui_component::Root::render_sheet_layer(window, cx))
            .children(gpui_component::Root::render_dialog_layer(window, cx))
            .when_some(drag_overlay, |this, overlay| this.child(overlay))
            .children(gpui_component::Root::render_notification_layer(window, cx))
            .when_some(lock_overlay, |this, overlay| this.child(overlay))
    }
}

impl SettingsWindow {
    #[cfg(test)]
    pub(super) fn supports_setting_id(id: &'static str) -> bool {
        settings_supported_id_matches!(id)
    }
}
