use std::{
    cmp::Ordering,
    collections::{BTreeMap, HashSet},
    rc::Rc,
};

use gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable, Hsla,
    InteractiveElement, IntoElement, ParentElement, Render, Styled as _, Subscription, Window, div,
    px,
};
use gpui_component::{
    ActiveTheme as _, Sizable as _,
    color_picker::{ColorPicker, ColorPickerEvent, ColorPickerState},
    h_flex,
    input::{Input, InputEvent, InputState},
    switch::Switch,
    v_flex,
};
use rust_i18n::t;

pub struct ThemeEditor {
    focus_handle: FocusHandle,

    edited_config: gpui_component::ThemeConfig,

    name_input: Entity<InputState>,
    color_values: BTreeMap<String, String>,
    color_rows: Vec<ColorRow>,
    suppressed_input_events: HashSet<String>,
    _subscriptions: Vec<Subscription>,
}

struct ColorRow {
    id: String,
    label: String,
    key: String,
    input: Entity<InputState>,
    picker: Entity<ColorPickerState>,
}

impl EventEmitter<()> for ThemeEditor {}

impl Focusable for ThemeEditor {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn new_input(
    window: &mut Window,
    cx: &mut Context<ThemeEditor>,
    placeholder: String,
) -> Entity<InputState> {
    cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
}

fn set_input_value(
    input: &Entity<InputState>,
    value: &str,
    window: &mut Window,
    cx: &mut Context<ThemeEditor>,
) {
    let value = value.to_string();
    input.update(cx, move |state, cx| state.set_value(&value, window, cx));
}

fn new_color_input(
    value: &str,
    window: &mut Window,
    cx: &mut Context<ThemeEditor>,
) -> Entity<InputState> {
    let input = cx.new(|cx| InputState::new(window, cx));
    set_input_value(&input, value, window, cx);
    input
}

impl ThemeEditor {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();

        let base_config = if cx.theme().mode.is_dark() {
            (*gpui_component::Theme::global(cx).dark_theme).clone()
        } else {
            (*gpui_component::Theme::global(cx).light_theme).clone()
        };

        let mut edited_config = base_config;

        let name_input = new_input(window, cx, t!("ThemeEditor.NamePlaceholder").to_string());

        let mut color_values = initial_color_values(&edited_config, cx);
        // Ensure a minimal set is always present.
        ensure_minimal_colors(&mut color_values, cx);

        // Sync edited_config.colors to the map so future clones are consistent.
        edited_config.colors = colors_from_map(&color_values);

        let mut color_rows = Vec::new();
        for (key, hex) in color_values.iter() {
            let (id, label) = color_id_and_label(key);

            let input = new_color_input(hex, window, cx);

            let picker = cx.new(|cx| {
                let default_color = <Hsla as gpui_component::Colorize>::parse_hex(hex)
                    .unwrap_or_else(|_| cx.theme().background);
                ColorPickerState::new(window, cx).default_value(default_color)
            });

            color_rows.push(ColorRow {
                id,
                label,
                key: key.clone(),
                input,
                picker,
            });
        }

        // Keep ordering stable (and nice) by sorting by key.
        const PINNED_KEYS: [&str; 5] = [
            "background",
            "foreground",
            "muted.background",
            "muted.foreground",
            "border",
        ];
        color_rows.sort_by(|a, b| {
            let a_pos = PINNED_KEYS.iter().position(|k| *k == a.key);
            let b_pos = PINNED_KEYS.iter().position(|k| *k == b.key);
            match (a_pos, b_pos) {
                (Some(a_pos), Some(b_pos)) => a_pos.cmp(&b_pos),
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => a.key.cmp(&b.key),
            }
        });

        let mut subscriptions = Vec::new();
        for row in &color_rows {
            let key_for_input = row.key.clone();
            let key_for_picker = row.key.clone();
            let input_entity = row.input.clone();
            let picker_entity = row.picker.clone();

            subscriptions.push(cx.subscribe_in(
                &row.input,
                window,
                move |this, input, ev: &InputEvent, window, cx| {
                    if !matches!(ev, InputEvent::Change | InputEvent::PressEnter { .. }) {
                        return;
                    }
                    if this.suppressed_input_events.remove(&key_for_input) {
                        return;
                    }

                    let value = input.read(cx).value().trim().to_string();
                    let Some(canonical) = canonical_hex8(&value) else {
                        return;
                    };

                    // Keep picker in sync.
                    if let Ok(color) = <Hsla as gpui_component::Colorize>::parse_hex(&canonical) {
                        picker_entity.update(cx, |picker, cx| picker.set_value(color, window, cx));
                    }

                    this.color_values.insert(key_for_input.clone(), canonical);
                    this.apply_preview(window, cx);
                },
            ));

            subscriptions.push(cx.subscribe_in(
                &row.picker,
                window,
                move |this, _picker, ev: &ColorPickerEvent, window, cx| {
                    let ColorPickerEvent::Change(Some(color)) = ev else {
                        return;
                    };

                    let hex = hsla_to_hex8(*color);
                    this.color_values
                        .insert(key_for_picker.clone(), hex.clone());

                    // Keep input in sync (avoid feedback loop).
                    this.suppressed_input_events.insert(key_for_picker.clone());
                    input_entity.update(cx, |state, cx| state.set_value(&hex, window, cx));

                    this.apply_preview(window, cx);
                },
            ));
        }

        Self {
            focus_handle,
            edited_config,
            name_input,
            color_values,
            color_rows,
            suppressed_input_events: HashSet::default(),
            _subscriptions: subscriptions,
        }
    }

    fn apply_preview(&mut self, _window: &mut Window, cx: &mut App) {
        self.edited_config.colors = colors_from_map(&self.color_values);
        let config = Rc::new(self.edited_config.clone());
        let mode = config.mode;
        gpui_component::Theme::global_mut(cx).apply_config(&config);
        gpui_component::Theme::change(mode, None, cx);
        cx.refresh_windows();
    }

    pub fn save_payload(&self, cx: &App) -> ThemeSavePayload {
        let name = self.name_input.read(cx).value().trim().to_string();
        let theme_name = if name.is_empty() {
            "Custom Theme".to_string()
        } else {
            name
        };

        let mut theme = self.edited_config.clone();
        theme.name = theme_name.clone().into();
        theme.is_default = false;

        let mode = theme.mode;
        let set = gpui_component::ThemeSet {
            name: "Custom".into(),
            author: None,
            url: None,
            themes: vec![theme],
        };

        ThemeSavePayload {
            theme_name,
            mode,
            set,
        }
    }
}

fn initial_color_values(
    config: &gpui_component::ThemeConfig,
    cx: &App,
) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Ok(value) = serde_json::to_value(&config.colors) else {
        return out;
    };
    let Some(obj) = value.as_object() else {
        return out;
    };
    for (k, v) in obj {
        let Some(s) = v.as_str() else {
            continue;
        };
        if let Some(hex) = canonical_hex8(s) {
            out.insert(k.clone(), hex);
        }
    }

    // If the theme config is sparse, still show a usable editor.
    if out.is_empty() {
        out.insert(
            "background".to_string(),
            hsla_to_hex8(cx.theme().background),
        );
        out.insert(
            "foreground".to_string(),
            hsla_to_hex8(cx.theme().foreground),
        );
    }

    out
}

fn ensure_minimal_colors(map: &mut BTreeMap<String, String>, cx: &App) {
    map.entry("background".to_string())
        .or_insert_with(|| hsla_to_hex8(cx.theme().background));
    map.entry("foreground".to_string())
        .or_insert_with(|| hsla_to_hex8(cx.theme().foreground));
    map.entry("muted.background".to_string())
        .or_insert_with(|| hsla_to_hex8(cx.theme().muted));
    map.entry("muted.foreground".to_string())
        .or_insert_with(|| hsla_to_hex8(cx.theme().muted_foreground));
    map.entry("border".to_string())
        .or_insert_with(|| hsla_to_hex8(cx.theme().border));
}

fn colors_from_map(map: &BTreeMap<String, String>) -> gpui_component::ThemeConfigColors {
    let mut obj = serde_json::Map::new();
    for (k, v) in map {
        obj.insert(k.clone(), serde_json::Value::String(v.clone()));
    }
    serde_json::from_value(serde_json::Value::Object(obj)).unwrap_or_default()
}

fn canonical_hex8(s: &str) -> Option<String> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let Ok(color) = <Hsla as gpui_component::Colorize>::parse_hex(s) else {
        return None;
    };
    Some(hsla_to_hex8(color))
}

fn hsla_to_hex8(color: Hsla) -> String {
    let rgb = color.to_rgb();
    let to_u8 = |f: f32| (f.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!(
        "#{:02X}{:02X}{:02X}{:02X}",
        to_u8(rgb.r),
        to_u8(rgb.g),
        to_u8(rgb.b),
        to_u8(rgb.a)
    )
}

fn color_id_and_label(key: &str) -> (String, String) {
    // Special-cases to keep existing test selectors stable.
    if key == "muted.background" {
        return ("muted".to_string(), "muted".to_string());
    }
    if key == "muted.foreground" {
        return (
            "muted_foreground".to_string(),
            "muted.foreground".to_string(),
        );
    }
    let id = key
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect::<String>();
    (id, key.to_string())
}

pub struct ThemeSavePayload {
    pub theme_name: String,
    pub mode: gpui_component::ThemeMode,
    pub set: gpui_component::ThemeSet,
}

fn unique_theme_path(
    themes_dir: &std::path::Path,
    theme_name: &str,
    mode: gpui_component::ThemeMode,
) -> std::path::PathBuf {
    let mut stem = sanitize_file_stem(theme_name);
    if mode.is_dark() {
        stem.push_str("-dark");
    } else {
        stem.push_str("-light");
    }

    let mut n = 0usize;
    loop {
        let file_name = if n == 0 {
            format!("{stem}.json")
        } else {
            format!("{stem}-{n}.json")
        };
        let path = themes_dir.join(file_name);
        if !path.exists() {
            return path;
        }
        n += 1;
    }
}

fn sanitize_file_stem(s: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in s.chars() {
        let ch = ch.to_ascii_lowercase();
        let ok = ch.is_ascii_alphanumeric();
        if ok {
            out.push(ch);
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "custom-theme".to_string()
    } else {
        out
    }
}

pub fn write_theme_set(themes_dir: &std::path::Path, payload: &ThemeSavePayload) {
    if let Err(err) = std::fs::create_dir_all(themes_dir) {
        log::warn!("failed to create themes dir: {err:#}");
        return;
    }

    let path = unique_theme_path(themes_dir, &payload.theme_name, payload.mode);
    let json = match serde_json::to_string_pretty(&payload.set) {
        Ok(json) => json,
        Err(err) => {
            log::warn!("failed to serialize theme file: {err:#}");
            return;
        }
    };
    if let Err(err) = std::fs::write(&path, json) {
        log::warn!("failed to write theme file: {err:#}");
    }
}

impl Render for ThemeEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        let mut rows = Vec::new();
        for row in &self.color_rows {
            let id = row.id.clone();
            let label = row.label.clone();

            rows.push(
                h_flex()
                    .items_center()
                    .gap_2()
                    .h(px(32.))
                    .debug_selector({
                        let id = id.clone();
                        move || format!("termua-theme-editor-color-{id}")
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .child(label),
                    )
                    .child(
                        div()
                            .w(px(140.))
                            .flex_shrink_0()
                            .h(px(32.))
                            .debug_selector({
                                let id = id.clone();
                                move || format!("termua-theme-editor-color-{id}-input")
                            })
                            .on_any_mouse_down({
                                let input = row.input.clone();
                                move |_, window, cx| {
                                    input.update(cx, |state, cx| state.focus(window, cx));
                                }
                            })
                            .child(Input::new(&row.input)),
                    )
                    .child(
                        div()
                            .debug_selector({
                                let id = id.clone();
                                move || format!("termua-theme-editor-color-{id}-swatch")
                            })
                            .child(ColorPicker::new(&row.picker).xsmall()),
                    )
                    .into_any_element(),
            );
        }

        v_flex()
            .gap_2()
            .debug_selector(|| "termua-theme-editor-sheet".to_string())
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .w_full()
                            .h(px(32.))
                            .debug_selector(|| "termua-theme-editor-name-input".to_string())
                            .child(Input::new(&self.name_input)),
                    )
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(div().w(px(200.)).child(t!("ThemeEditor.Mode").to_string()))
                            .child(
                                div()
                                    .debug_selector(|| {
                                        "termua-theme-editor-mode-switch".to_string()
                                    })
                                    .child(
                                        Switch::new("termua-theme-editor-mode-switch")
                                            .small()
                                            .checked(self.edited_config.mode.is_dark())
                                            .label(t!("ThemeEditor.Dark").to_string())
                                            .on_click(cx.listener(|this, checked, window, cx| {
                                                this.edited_config.mode = if *checked {
                                                    gpui_component::ThemeMode::Dark
                                                } else {
                                                    gpui_component::ThemeMode::Light
                                                };
                                                this.apply_preview(window, cx);
                                            })),
                                    ),
                            ),
                    ),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(t!("ThemeEditor.Hint").to_string()),
            )
            .children(rows)
    }
}

// UI behavior is covered by `termua/src/window/settings.rs` gpui tests, which render the editor
// inside `gpui_component::Root` (required for sheets).
