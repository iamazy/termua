use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    rc::Rc,
    time::Duration,
};

use anyhow::Context;
use gpui::{App, AppContext, KeyBinding, KeyBindingContextPredicate, NoAction, Window};
use gpui_common::set_sftp_upload_permit_pool_max_in_app;
use gpui_term::{
    CursorShape, SshBackend, TerminalBlink, TerminalLineHeight, TerminalSettings as TermSettings,
};
use serde::{Deserialize, Serialize};

#[derive(Copy, Clone, Debug)]
struct ThemeSettings {
    mode: ThemeMode,
}

impl gpui::Global for ThemeSettings {}

#[derive(Copy, Clone, Debug)]
pub(crate) struct LanguageSettings {
    pub(crate) language: Language,
}

impl gpui::Global for LanguageSettings {}

#[derive(Copy, Clone, Debug)]
pub(crate) struct SshBackendPreference {
    pub(crate) backend: SshBackend,
}

impl gpui::Global for SshBackendPreference {}

#[derive(Copy, Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeMode {
    #[default]
    System,
    Light,
    Dark,
}

/// Returns the current requested theme mode.
///
/// If no explicit mode was set, defaults to [`ThemeMode::System`].
pub fn theme_mode(cx: &App) -> ThemeMode {
    if cx.has_global::<ThemeSettings>() {
        cx.global::<ThemeSettings>().mode
    } else {
        ThemeMode::System
    }
}

/// Set the application theme mode (Light/Dark/System).
pub fn set_theme_mode(mode: ThemeMode, window: Option<&mut Window>, cx: &mut App) {
    if cx.has_global::<ThemeSettings>() {
        cx.global_mut::<ThemeSettings>().mode = mode;
    } else {
        cx.set_global(ThemeSettings { mode });
    }

    match mode {
        ThemeMode::System => gpui_component::Theme::sync_system_appearance(window, cx),
        ThemeMode::Light => {
            gpui_component::Theme::change(gpui_component::ThemeMode::Light, window, cx)
        }
        ThemeMode::Dark => {
            gpui_component::Theme::change(gpui_component::ThemeMode::Dark, window, cx)
        }
    }

    // Ensure all windows repaint.
    cx.refresh_windows();
}

#[derive(Copy, Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum Language {
    #[default]
    #[serde(rename = "en")]
    English,
    #[serde(rename = "zh-CN")]
    ZhCn,
}

impl Language {
    pub fn locale(self) -> &'static str {
        match self {
            Self::English => "en",
            Self::ZhCn => "zh-CN",
        }
    }
}

/// Set the application language (and active i18n locale).
pub fn set_language(language: Language, cx: &mut App) {
    if cx.has_global::<LanguageSettings>() {
        cx.global_mut::<LanguageSettings>().language = language;
    } else {
        cx.set_global(LanguageSettings { language });
    }

    crate::locale::set_locale(language.locale());

    // Rebuild the OS/app menus so the new locale is reflected immediately.
    // `TermuaAppState` isn't available during early startup before main initializes globals, so
    // we only attempt this when the app state exists.
    if cx.try_global::<crate::TermuaAppState>().is_some() {
        crate::menu::sync_app_menus(cx);
    }

    // Ensure all windows repaint.
    cx.refresh_windows();
}

/// Ensures [`LanguageSettings`] exists and that the active i18n locale matches it.
///
/// If no explicit language was set yet, initializes it to `default_language`.
pub(crate) fn ensure_language_state_with_default<C>(
    default_language: Language,
    cx: &mut C,
) -> Language
where
    C: AppContext + std::borrow::BorrowMut<App>,
{
    let language = {
        let app = cx.borrow_mut();
        if app.has_global::<LanguageSettings>() {
            app.global::<LanguageSettings>().language
        } else {
            app.set_global(LanguageSettings {
                language: default_language,
            });
            default_language
        }
    };

    crate::locale::set_locale(language.locale());
    language
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AppearanceSettings {
    pub theme: ThemeMode,
    pub language: Language,
    /// Name of the selected light theme config (from ThemeRegistry). None = registry default.
    pub light_theme: Option<String>,
    /// Name of the selected dark theme config (from ThemeRegistry). None = registry default.
    pub dark_theme: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct RecordingSettings {
    pub include_input_by_default: bool,
    pub playback_speed: f64,
}

impl gpui::Global for RecordingSettings {}

impl RecordingSettings {
    pub fn playback_speed_or_default(&self) -> f64 {
        if self.playback_speed.is_finite() && self.playback_speed > 0.0 {
            self.playback_speed
        } else {
            1.0
        }
    }
}

impl Default for RecordingSettings {
    fn default() -> Self {
        Self {
            include_input_by_default: false,
            playback_speed: 1.0,
        }
    }
}

#[derive(Copy, Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    /// Follow `TERMUA_LOG`/`RUST_LOG` if present; otherwise use the logger's default.
    #[default]
    Default,
    Error,
    Warn,
    Info,
    Debug,
    Trace,
    Off,
}

impl LogLevel {
    pub fn to_level_filter(self) -> log::LevelFilter {
        match self {
            Self::Default => log::LevelFilter::Error,
            Self::Error => log::LevelFilter::Error,
            Self::Warn => log::LevelFilter::Warn,
            Self::Info => log::LevelFilter::Info,
            Self::Debug => log::LevelFilter::Debug,
            Self::Trace => log::LevelFilter::Trace,
            Self::Off => log::LevelFilter::Off,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LoggingSettings {
    pub level: LogLevel,
    /// Optional log file path. If relative, it's resolved relative to the directory containing
    /// `settings.json`.
    pub path: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct UiSettings {
    /// Persisted navigation state for SettingsWindow.
    ///
    /// Stored as the nav tree item id (e.g. `"nav.page.terminal.font"`), rather than an enum, to
    /// avoid coupling this settings file format to the settings window module.
    pub last_settings_page: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SharingSettings {
    /// Whether the terminal sharing feature is enabled.
    pub enabled: bool,
    /// Optional relay websocket URL (e.g. "ws://127.0.0.1:7231/ws").
    pub relay_url: Option<String>,
}

impl gpui::Global for SharingSettings {}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct AssistantSettings {
    pub enabled: bool,
    /// LLM provider name as understood by zeroclaw (e.g. "openai", "anthropic", "openrouter",
    /// "codex", ...). None means "use zeroclaw config defaults".
    pub provider: Option<String>,
    /// Model name for the selected provider (e.g. "gpt-4o-mini"). None means "use zeroclaw
    /// config defaults".
    pub model: Option<String>,
    /// Optional temperature override.
    pub temperature: Option<f64>,
    /// Optional custom base URL for the selected provider (OpenAI-compatible providers, Ollama,
    /// etc). None means "use provider defaults".
    pub api_url: Option<String>,
    /// Optional API path override for OpenAI-compatible providers (e.g. "/chat/completions").
    pub api_path: Option<String>,
    /// Optional timeout override in seconds for provider API requests.
    pub provider_timeout_secs: Option<u64>,
    /// Extra HTTP headers to include in provider API requests.
    #[serde(default)]
    pub extra_headers: HashMap<String, String>,
}

impl gpui::Global for AssistantSettings {}

impl Default for AssistantSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            provider: None,
            model: None,
            temperature: None,
            api_url: None,
            api_path: None,
            provider_timeout_secs: None,
            extra_headers: HashMap::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct LockScreenSettings {
    /// Whether Termua's lock screen feature is enabled.
    pub enabled: bool,
    /// Idle timeout in seconds before locking.
    ///
    /// `0` means "never auto-lock".
    pub timeout_secs: u64,
}

impl gpui::Global for LockScreenSettings {}

impl Default for LockScreenSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            timeout_secs: 5 * 60,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TerminalKeyBindings {
    pub copy: Option<String>,
    pub paste: Option<String>,
    pub select_all: Option<String>,
    pub clear: Option<String>,
    pub search: Option<String>,
    pub search_next: Option<String>,
    pub search_previous: Option<String>,
    pub increase_font_size: Option<String>,
    pub decrease_font_size: Option<String>,
    pub reset_font_size: Option<String>,
}

#[derive(Copy, Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalBackend {
    #[default]
    Alacritty,
    Wezterm,
}

#[derive(Clone, Debug)]
pub struct TerminalSettings {
    pub settings: TermSettings,
    pub default_backend: TerminalBackend,
    pub ssh_backend: SshBackend,
}

impl std::ops::Deref for TerminalSettings {
    type Target = TermSettings;

    fn deref(&self) -> &Self::Target {
        &self.settings
    }
}

impl std::ops::DerefMut for TerminalSettings {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.settings
    }
}

impl Default for TerminalSettings {
    fn default() -> Self {
        let mut settings = TermSettings::default();
        settings.font_features = set_calt_font_feature(&settings.font_features, true);

        Self {
            settings,
            default_backend: TerminalBackend::default(),
            ssh_backend: SshBackend::default(),
        }
    }
}

impl Serialize for TerminalSettings {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut value = serde_json::to_value(&self.settings).map_err(serde::ser::Error::custom)?;
        let map = value.as_object_mut().ok_or_else(|| {
            serde::ser::Error::custom("terminal settings should serialize as an object")
        })?;

        map.insert(
            "default_backend".into(),
            serde_json::to_value(self.default_backend).map_err(serde::ser::Error::custom)?,
        );
        map.insert(
            "ssh_backend".into(),
            serde_json::to_value(self.ssh_backend).map_err(serde::ser::Error::custom)?,
        );
        map.insert(
            "ligatures".into(),
            serde_json::Value::Bool(self.ligatures_enabled()),
        );

        let extra_font_features = font_features_without_calt(&self.settings.font_features);
        if extra_font_features.tag_value_list().is_empty() {
            map.remove("font_features");
        } else {
            map.insert(
                "font_features".into(),
                serde_json::to_value(extra_font_features).map_err(serde::ser::Error::custom)?,
            );
        }

        value.serialize(serializer)
    }
}

impl TerminalSettings {
    pub fn ligatures_enabled(&self) -> bool {
        self.settings.font_features.is_calt_enabled() == Some(true)
    }

    pub fn set_ligatures_enabled(&mut self, enabled: bool) {
        self.settings.font_features = set_calt_font_feature(&self.settings.font_features, enabled);
    }
}

fn set_calt_font_feature(font_features: &gpui::FontFeatures, enabled: bool) -> gpui::FontFeatures {
    let mut features = font_features.tag_value_list().to_vec();

    if let Some((_, value)) = features.iter_mut().find(|(tag, _)| tag == "calt") {
        *value = u32::from(enabled);
    } else {
        features.push(("calt".into(), u32::from(enabled)));
    }

    gpui::FontFeatures(std::sync::Arc::new(features))
}

fn font_features_without_calt(font_features: &gpui::FontFeatures) -> gpui::FontFeatures {
    let features = font_features
        .tag_value_list()
        .iter()
        .filter(|(tag, _)| tag != "calt")
        .cloned()
        .collect::<Vec<_>>();

    gpui::FontFeatures(std::sync::Arc::new(features))
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SettingsFile {
    pub appearance: AppearanceSettings,
    pub terminal: TerminalSettings,
    #[serde(default)]
    pub terminal_keybindings: TerminalKeyBindings,
    pub recording: RecordingSettings,
    #[serde(default)]
    pub logging: LoggingSettings,
    #[serde(default)]
    pub ui: UiSettings,
    #[serde(default)]
    pub sharing: SharingSettings,
    #[serde(default)]
    pub assistant: AssistantSettings,
    #[serde(default)]
    pub lock_screen: LockScreenSettings,
}

#[derive(Clone, Debug, Default)]
struct AppliedTerminalKeybindings {
    copy: Option<String>,
    paste: Option<String>,
    select_all: Option<String>,
    clear: Option<String>,
    search: Option<String>,
    search_next: Option<String>,
    search_previous: Option<String>,
    increase_font_size: Option<String>,
    decrease_font_size: Option<String>,
    reset_font_size: Option<String>,
}

impl gpui::Global for AppliedTerminalKeybindings {}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct SettingsFilePatch {
    appearance: AppearanceSettings,
    recording: RecordingSettings,
    logging: LoggingSettings,
    ui: UiSettings,
    #[serde(default)]
    sharing: SharingSettings,
    assistant: AssistantSettings,
    lock_screen: LockScreenSettings,
    terminal: TerminalSettingsPatch,
    terminal_keybindings: TerminalKeyBindings,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct TerminalSettingsPatch {
    default_backend: Option<TerminalBackend>,
    ssh_backend: Option<SshBackend>,
    ligatures: Option<bool>,
    font_size: Option<gpui::Pixels>,
    font_family: Option<gpui::SharedString>,
    font_fallbacks: Option<Option<gpui::FontFallbacks>>,
    font_features: Option<gpui::FontFeatures>,
    font_weight: Option<gpui::FontWeight>,
    line_height: Option<TerminalLineHeight>,
    env: Option<HashMap<String, String>>,
    cursor_shape: Option<Option<CursorShape>>,
    blinking: Option<TerminalBlink>,
    option_as_meta: Option<bool>,
    copy_on_select: Option<bool>,
    sftp_upload_max_concurrency: Option<usize>,
    minimum_contrast: Option<f32>,
    show_scrollbar: Option<bool>,
    show_line_numbers: Option<bool>,
    suggestions_enabled: Option<bool>,
    suggestions_max_items: Option<usize>,
}

impl TerminalSettingsPatch {
    fn apply_to(&self, settings: &mut TermSettings) {
        if let Some(v) = self.font_size {
            settings.font_size = v;
        }
        if let Some(v) = self.font_family.clone() {
            settings.font_family = v;
        }
        if let Some(v) = self.font_fallbacks.clone() {
            settings.font_fallbacks = v;
        }
        if let Some(v) = self.font_features.clone() {
            settings.font_features = v;
        }
        if let Some(v) = self.ligatures {
            settings.font_features = set_calt_font_feature(&settings.font_features, v);
        }
        if let Some(v) = self.font_weight {
            settings.font_weight = v;
        }
        if let Some(v) = self.line_height.clone() {
            settings.line_height = v;
        }
        if let Some(v) = self.env.clone() {
            settings.env = v;
        }
        if let Some(v) = self.cursor_shape {
            settings.cursor_shape = v;
        }
        if let Some(v) = self.blinking {
            settings.blinking = v;
        }
        if let Some(v) = self.option_as_meta {
            settings.option_as_meta = v;
        }
        if let Some(v) = self.copy_on_select {
            settings.copy_on_select = v;
        }
        if let Some(v) = self.sftp_upload_max_concurrency {
            // Keep a bounded, sane range even if users hand-edit settings.json.
            settings.sftp_upload_max_concurrency = v.clamp(2, 15);
        }
        if let Some(v) = self.minimum_contrast {
            settings.minimum_contrast = v;
        }
        if let Some(v) = self.show_scrollbar {
            settings.show_scrollbar = v;
        }
        if let Some(v) = self.show_line_numbers {
            settings.show_line_numbers = v;
        }
        if let Some(v) = self.suggestions_enabled {
            settings.suggestions_enabled = v;
        }
        if let Some(v) = self.suggestions_max_items {
            settings.suggestions_max_items = v;
        }
    }
}

impl<'de> Deserialize<'de> for TerminalSettings {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let patch = TerminalSettingsPatch::deserialize(deserializer)?;
        let mut terminal = TerminalSettings::default();
        patch.apply_to(&mut terminal.settings);
        if let Some(default_backend) = patch.default_backend {
            terminal.default_backend = default_backend;
        }
        if let Some(ssh_backend) = patch.ssh_backend {
            terminal.ssh_backend = ssh_backend;
        }
        Ok(terminal)
    }
}

impl SettingsFile {
    pub fn load_from_str_lenient(json: &str) -> anyhow::Result<Self> {
        let patch: SettingsFilePatch =
            serde_json_lenient::from_str_lenient(json).context("parse settings.json")?;

        let mut terminal = TerminalSettings::default();
        patch.terminal.apply_to(&mut terminal.settings);
        let terminal = TerminalSettings {
            settings: terminal.settings,
            default_backend: patch.terminal.default_backend.unwrap_or_default(),
            ssh_backend: patch.terminal.ssh_backend.unwrap_or_default(),
        };

        Ok(Self {
            appearance: patch.appearance,
            terminal,
            terminal_keybindings: patch.terminal_keybindings,
            recording: patch.recording,
            logging: patch.logging,
            ui: patch.ui,
            sharing: patch.sharing,
            assistant: patch.assistant,
            lock_screen: patch.lock_screen,
        })
    }

    pub fn load_from_path(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref();
        match fs::read_to_string(path) {
            Ok(s) => Self::load_from_str_lenient(&s),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(err) => Err(err).with_context(|| format!("read settings file {path:?}")),
        }
    }

    pub fn save_to_path(&self, path: impl AsRef<Path>) -> anyhow::Result<()> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create settings directory {parent:?}"))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
                    .with_context(|| format!("chmod settings directory {parent:?}"))?;
            }
        }

        let json = self.to_json_pretty()?;
        crate::atomic_write::write_string(path, &json)
            .with_context(|| format!("write settings file {path:?}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
                .with_context(|| format!("chmod settings file {path:?}"))?;
        }
        Ok(())
    }

    pub fn to_json_pretty(&self) -> anyhow::Result<String> {
        serde_json::to_string_pretty(self).context("serialize settings.json")
    }

    pub fn apply_to_app(&self, window: Option<&mut Window>, cx: &mut App) {
        if cx.has_global::<TermSettings>() {
            *cx.global_mut::<TermSettings>() = self.terminal.settings.clone();
        } else {
            cx.set_global(self.terminal.settings.clone());
        }

        if cx.has_global::<SshBackendPreference>() {
            cx.global_mut::<SshBackendPreference>().backend = self.terminal.ssh_backend;
        } else {
            cx.set_global(SshBackendPreference {
                backend: self.terminal.ssh_backend,
            });
        }

        // Apply immediately to in-flight and queued transfers by resizing the global permit pool.
        set_sftp_upload_permit_pool_max_in_app(
            cx,
            self.terminal.settings.sftp_upload_max_concurrency,
        );

        if cx.has_global::<gpui_term::cast::CastRecordingConfig>() {
            cx.global_mut::<gpui_term::cast::CastRecordingConfig>()
                .include_input_by_default = self.recording.include_input_by_default;
        }

        if cx.has_global::<RecordingSettings>() {
            *cx.global_mut::<RecordingSettings>() = self.recording.clone();
        } else {
            cx.set_global(self.recording.clone());
        }

        if cx.has_global::<SharingSettings>() {
            *cx.global_mut::<SharingSettings>() = self.sharing.clone();
        } else {
            cx.set_global(self.sharing.clone());
        }

        self.apply_assistant_settings(cx);
        self.apply_lock_screen_settings(cx);

        // Apply selected concrete theme configs (if present) before changing mode, so
        // `Theme::change` uses the updated light/dark configs.
        crate::theme_manager::apply_selected_themes(
            self.appearance.light_theme.as_deref(),
            self.appearance.dark_theme.as_deref(),
            cx,
        );

        set_language(self.appearance.language, cx);
        set_theme_mode(self.appearance.theme, window, cx);
    }

    pub fn apply_assistant_settings(&self, cx: &mut App) {
        if cx.has_global::<AssistantSettings>() {
            *cx.global_mut::<AssistantSettings>() = self.assistant.clone();
        } else {
            cx.set_global(self.assistant.clone());
        }
    }

    pub fn apply_lock_screen_settings(&self, cx: &mut App) {
        if cx.has_global::<LockScreenSettings>() {
            *cx.global_mut::<LockScreenSettings>() = self.lock_screen.clone();
        } else {
            cx.set_global(self.lock_screen.clone());
        }

        if cx.has_global::<crate::lock_screen::LockState>() {
            let enabled = self.lock_screen.enabled;
            let timeout = Duration::from_secs(self.lock_screen.timeout_secs);
            cx.global_mut::<crate::lock_screen::LockState>()
                .set_user_enabled(enabled);
            cx.global_mut::<crate::lock_screen::LockState>()
                .set_idle_timeout(timeout);
        }
    }

    /// Apply terminal keybinding overrides.
    ///
    /// Implementation detail: GPUI key bindings are append-only. We implement updates by adding
    /// higher-precedence bindings:
    /// - When an override is set: bind `NoAction` for the default keystrokes, then bind the
    ///   override keystroke(s) for the action.
    /// - When an override is cleared: re-bind the default keystrokes for the action, so they take
    ///   precedence over any previously-added `NoAction` bindings.
    ///
    /// Defaults are scoped to the `Terminal` context so overrides don't affect other UI.
    pub fn apply_terminal_keybindings(&self, cx: &mut App) {
        if !cx.has_global::<AppliedTerminalKeybindings>() {
            cx.set_global(AppliedTerminalKeybindings::default());
        }

        let kb = &self.terminal_keybindings;
        let mut state = cx.global::<AppliedTerminalKeybindings>().clone();

        apply_terminal_binding::<gpui_term::Copy>(
            cx,
            &mut state.copy,
            DEFAULT_COPY,
            kb.copy.as_deref(),
        );
        apply_terminal_binding::<gpui_term::Paste>(
            cx,
            &mut state.paste,
            DEFAULT_PASTE,
            kb.paste.as_deref(),
        );
        apply_terminal_binding::<gpui_term::SelectAll>(
            cx,
            &mut state.select_all,
            DEFAULT_SELECT_ALL,
            kb.select_all.as_deref(),
        );
        apply_terminal_binding::<gpui_term::Clear>(
            cx,
            &mut state.clear,
            DEFAULT_CLEAR,
            kb.clear.as_deref(),
        );
        apply_terminal_binding::<gpui_term::Search>(
            cx,
            &mut state.search,
            DEFAULT_SEARCH,
            kb.search.as_deref(),
        );
        apply_terminal_binding::<gpui_term::SearchNext>(
            cx,
            &mut state.search_next,
            DEFAULT_SEARCH_NEXT,
            kb.search_next.as_deref(),
        );
        apply_terminal_binding::<gpui_term::SearchPrevious>(
            cx,
            &mut state.search_previous,
            DEFAULT_SEARCH_PREV,
            kb.search_previous.as_deref(),
        );
        apply_terminal_binding::<gpui_term::IncreaseFontSize>(
            cx,
            &mut state.increase_font_size,
            DEFAULT_FONT_INC,
            kb.increase_font_size.as_deref(),
        );
        apply_terminal_binding::<gpui_term::DecreaseFontSize>(
            cx,
            &mut state.decrease_font_size,
            DEFAULT_FONT_DEC,
            kb.decrease_font_size.as_deref(),
        );
        apply_terminal_binding::<gpui_term::ResetFontSize>(
            cx,
            &mut state.reset_font_size,
            DEFAULT_FONT_RESET,
            kb.reset_font_size.as_deref(),
        );

        *cx.global_mut::<AppliedTerminalKeybindings>() = state;
    }
}

#[cfg(target_os = "macos")]
const DEFAULT_SELECT_ALL: &[&str] = &["cmd-a"];
#[cfg(not(target_os = "macos"))]
const DEFAULT_SELECT_ALL: &[&str] = &["ctrl-shift-a"];

#[cfg(target_os = "macos")]
const DEFAULT_PASTE: &[&str] = &["cmd-v"];
#[cfg(not(target_os = "macos"))]
const DEFAULT_PASTE: &[&str] = &["ctrl-shift-v"];

#[cfg(target_os = "macos")]
const DEFAULT_COPY: &[&str] = &["cmd-c"];
#[cfg(not(target_os = "macos"))]
const DEFAULT_COPY: &[&str] = &["ctrl-shift-c"];

#[cfg(target_os = "macos")]
const DEFAULT_CLEAR: &[&str] = &["cmd-k"];
#[cfg(not(target_os = "macos"))]
const DEFAULT_CLEAR: &[&str] = &["ctrl-shift-k"];

#[cfg(target_os = "macos")]
const DEFAULT_SEARCH: &[&str] = &["cmd-f"];
#[cfg(not(target_os = "macos"))]
const DEFAULT_SEARCH: &[&str] = &["ctrl-shift-f"];

#[cfg(target_os = "macos")]
const DEFAULT_SEARCH_NEXT: &[&str] = &["cmd-g"];
#[cfg(not(target_os = "macos"))]
const DEFAULT_SEARCH_NEXT: &[&str] = &["ctrl-g"];

#[cfg(target_os = "macos")]
const DEFAULT_SEARCH_PREV: &[&str] = &["cmd-shift-g"];
#[cfg(not(target_os = "macos"))]
const DEFAULT_SEARCH_PREV: &[&str] = &["ctrl-shift-g"];

#[cfg(target_os = "macos")]
const DEFAULT_FONT_INC: &[&str] = &["cmd-+", "cmd-="];
#[cfg(not(target_os = "macos"))]
const DEFAULT_FONT_INC: &[&str] = &["ctrl-+", "ctrl-="];

#[cfg(target_os = "macos")]
const DEFAULT_FONT_DEC: &[&str] = &["cmd--"];
#[cfg(not(target_os = "macos"))]
const DEFAULT_FONT_DEC: &[&str] = &["ctrl--"];

#[cfg(target_os = "macos")]
const DEFAULT_FONT_RESET: &[&str] = &["cmd-0"];
#[cfg(not(target_os = "macos"))]
const DEFAULT_FONT_RESET: &[&str] = &["ctrl-0"];

fn load_user_keybinding(
    keystrokes: &str,
    action: Box<dyn gpui::Action>,
    context: Option<&str>,
) -> Option<KeyBinding> {
    let keystrokes = keystrokes.trim();
    if keystrokes.is_empty() {
        return None;
    }

    let context_predicate = context.and_then(|context| {
        KeyBindingContextPredicate::parse(context)
            .ok()
            .map(|p| Rc::new(p))
    });

    match KeyBinding::load(
        keystrokes,
        action,
        context_predicate,
        false,
        None,
        &gpui::DummyKeyboardMapper,
    ) {
        Ok(binding) => Some(binding),
        Err(err) => {
            log::warn!("invalid keybinding string {keystrokes:?}: {err:#}");
            None
        }
    }
}

fn apply_terminal_binding<A: gpui::Action + Default + 'static>(
    cx: &mut App,
    previous_override: &mut Option<String>,
    default_keystrokes: &[&str],
    override_keystrokes: Option<&str>,
) {
    let next_override = override_keystrokes
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    // If nothing changed, avoid unbounded keymap growth.
    if previous_override.as_deref() == next_override.as_deref() {
        return;
    }

    // If the user entered a non-empty override but it doesn't parse, keep the previous binding.
    if let Some(candidate) = next_override.as_deref() {
        if load_user_keybinding(candidate, Box::new(A::default()), Some("Terminal")).is_none() {
            return;
        }
    }

    // Disable the previously-applied override if we're changing or clearing it.
    if let Some(prev) = previous_override.as_deref() {
        cx.bind_keys([KeyBinding::new(prev, NoAction {}, Some("Terminal"))]);
    }

    if let Some(override_keys) = next_override.as_deref() {
        // Disable defaults, then apply override.
        for &k in default_keystrokes {
            cx.bind_keys([KeyBinding::new(k, NoAction {}, Some("Terminal"))]);
        }
        if let Some(binding) =
            load_user_keybinding(override_keys, Box::new(A::default()), Some("Terminal"))
        {
            cx.bind_keys([binding]);
        }
    } else {
        // Re-apply defaults at higher precedence to undo any prior NoAction shadowing.
        for &k in default_keystrokes {
            cx.bind_keys([KeyBinding::new(k, A::default(), Some("Terminal"))]);
        }
    }

    *previous_override = next_override;
}

pub fn settings_json_path() -> PathBuf {
    #[cfg(test)]
    if let Some(path) = SETTINGS_JSON_PATH_OVERRIDE.with(|slot| slot.borrow().clone()) {
        return path;
    }

    #[cfg(target_os = "windows")]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            return PathBuf::from(appdata).join("termua").join("settings.json");
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Some(home) = home::home_dir() {
            return home
                .join("Library")
                .join("Application Support")
                .join("termua")
                .join("settings.json");
        }
    }

    // Linux and fallback for unknown platforms.
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        return PathBuf::from(xdg).join("termua").join("settings.json");
    }

    home::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config")
        .join("termua")
        .join("settings.json")
}

pub fn settings_dir_path() -> PathBuf {
    settings_json_path()
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn load_settings_from_disk() -> anyhow::Result<SettingsFile> {
    SettingsFile::load_from_path(settings_json_path())
}

pub fn save_settings_to_disk(settings: &SettingsFile) -> anyhow::Result<()> {
    settings.save_to_path(settings_json_path())
}

// --- Test-only helpers (kept at the bottom for readability) ---

#[cfg(test)]
thread_local! {
    static SETTINGS_JSON_PATH_OVERRIDE: std::cell::RefCell<Option<PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub struct SettingsJsonPathOverrideGuard {
    prev: Option<PathBuf>,
}

#[cfg(test)]
impl Drop for SettingsJsonPathOverrideGuard {
    fn drop(&mut self) {
        let prev = self.prev.take();
        SETTINGS_JSON_PATH_OVERRIDE.with(|slot| *slot.borrow_mut() = prev);
    }
}

#[cfg(test)]
pub fn override_settings_json_path(path: PathBuf) -> SettingsJsonPathOverrideGuard {
    let prev = SETTINGS_JSON_PATH_OVERRIDE.with(|slot| slot.borrow_mut().replace(path));
    SettingsJsonPathOverrideGuard { prev }
}

#[cfg(test)]
mod tests {
    use gpui::px;
    use serde_json::Value;

    use super::*;

    #[gpui::test]
    fn theme_mode_tracks_requested_mode(cx: &mut gpui::TestAppContext) {
        let mut app = cx.app.borrow_mut();
        gpui_component::init(&mut app);

        set_theme_mode(ThemeMode::Dark, None, &mut app);
        assert_eq!(theme_mode(&app), ThemeMode::Dark);

        set_theme_mode(ThemeMode::Light, None, &mut app);
        assert_eq!(theme_mode(&app), ThemeMode::Light);

        set_theme_mode(ThemeMode::System, None, &mut app);
        assert_eq!(theme_mode(&app), ThemeMode::System);
    }

    #[test]
    fn settings_dir_path_uses_parent_of_settings_json() {
        let settings_path = std::env::temp_dir()
            .join("termua-settings-dir-test")
            .join("settings.json");
        let _guard = override_settings_json_path(settings_path.clone());

        assert_eq!(settings_dir_path(), settings_path.parent().unwrap());
    }

    #[test]
    fn parsing_empty_object_yields_defaults() {
        let settings = SettingsFile::load_from_str_lenient("{}").unwrap();
        assert_eq!(settings.terminal.copy_on_select, true);
        assert_eq!(settings.terminal.option_as_meta, false);
        assert_eq!(settings.terminal.ligatures_enabled(), true);
    }

    #[test]
    fn lenient_parsing_allows_trailing_commas() {
        let settings = SettingsFile::load_from_str_lenient(
            r#"
            {
              "terminal": {
                "copy_on_select": false,
              },
            }
            "#,
        )
        .unwrap();
        assert_eq!(settings.terminal.copy_on_select, false);
    }

    #[test]
    fn parsing_empty_object_defaults_sharing_disabled() {
        let settings = SettingsFile::load_from_str_lenient("{}").unwrap();
        assert_eq!(settings.sharing.enabled, false);
    }

    #[test]
    fn parsing_sharing_relay_url_roundtrips() {
        let settings = SettingsFile::load_from_str_lenient(
            r#"
            {
              "sharing": {
                "enabled": true,
                "relay_url": "wss://example.com/ws"
              }
            }
            "#,
        )
        .unwrap();

        assert_eq!(settings.sharing.enabled, true);
        assert_eq!(
            settings.sharing.relay_url.as_deref(),
            Some("wss://example.com/ws")
        );
    }

    #[test]
    fn parsing_partial_terminal_settings_merges_with_defaults() {
        let settings = SettingsFile::load_from_str_lenient(
            r#"
            {
              "terminal": {
                "copy_on_select": false
              }
            }
            "#,
        )
        .unwrap();

        assert_eq!(settings.terminal.copy_on_select, false);
        // Ensure other defaults remain intact.
        assert_eq!(settings.terminal.option_as_meta, false);
    }

    #[test]
    fn to_json_pretty_contains_terminal_top_level_key() {
        let json = SettingsFile::default().to_json_pretty().unwrap();
        assert!(json.contains("\"terminal\""));
    }

    #[test]
    fn to_json_pretty_contains_logging_top_level_key() {
        let json = SettingsFile::default().to_json_pretty().unwrap();
        assert!(json.contains("\"logging\""));
    }

    #[test]
    fn to_json_pretty_contains_assistant_top_level_key() {
        let json = SettingsFile::default().to_json_pretty().unwrap();
        assert!(json.contains("\"assistant\""));
    }

    #[test]
    fn parsing_language_setting_roundtrips() {
        let settings = SettingsFile::load_from_str_lenient(
            r#"
            {
              "appearance": {
                "language": "zh-CN"
              }
            }
            "#,
        )
        .unwrap();

        assert_eq!(settings.appearance.language, Language::ZhCn);

        let json = settings.to_json_pretty().unwrap();
        assert!(json.contains("\"language\": \"zh-CN\""));
    }

    #[test]
    fn terminal_default_backend_defaults_to_alacritty_in_json() {
        let json = SettingsFile::default().to_json_pretty().unwrap();
        let v: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["terminal"]["default_backend"], "alacritty");
    }

    #[test]
    fn terminal_ssh_backend_defaults_to_ssh2_in_json() {
        let json = SettingsFile::default().to_json_pretty().unwrap();
        let v: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["terminal"]["ssh_backend"], "ssh2");
    }

    #[test]
    fn logging_settings_are_leniently_parsed_and_roundtripped() {
        let settings = SettingsFile::load_from_str_lenient(
            r#"
            {
              "logging": {
                "level": "debug",
                "path": "termua.log"
              }
            }
            "#,
        )
        .unwrap();

        let json = settings.to_json_pretty().unwrap();
        let v: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["logging"]["level"], "debug");
        assert_eq!(v["logging"]["path"], "termua.log");
    }

    #[test]
    fn terminal_default_backend_is_leniently_parsed_and_roundtripped() {
        let settings = SettingsFile::load_from_str_lenient(
            r#"
            {
              "terminal": {
                "default_backend": "wezterm"
              }
            }
            "#,
        )
        .unwrap();

        let json = settings.to_json_pretty().unwrap();
        let v: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["terminal"]["default_backend"], "wezterm");
    }

    #[test]
    fn terminal_ssh_backend_is_leniently_parsed_and_roundtripped() {
        let settings = SettingsFile::load_from_str_lenient(
            r#"
            {
              "terminal": {
                "ssh_backend": "libssh"
              }
            }
            "#,
        )
        .unwrap();

        let json = settings.to_json_pretty().unwrap();
        let v: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["terminal"]["ssh_backend"], "libssh");
    }

    #[test]
    fn terminal_suggestions_settings_roundtrip() {
        let settings = SettingsFile::load_from_str_lenient(
            r#"
            {
              "terminal": {
                "suggestions_enabled": true,
                "suggestions_max_items": 12
              }
            }
            "#,
        )
        .unwrap();

        let json = settings.to_json_pretty().unwrap();
        let v: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["terminal"]["suggestions_enabled"], true);
        assert_eq!(v["terminal"]["suggestions_max_items"], 12);
    }

    #[test]
    fn terminal_ligatures_bool_setting_is_leniently_parsed_and_roundtripped() {
        let settings = SettingsFile::load_from_str_lenient(
            r#"
            {
              "terminal": {
                "ligatures": false
              }
            }
            "#,
        )
        .unwrap();

        assert!(!settings.terminal.ligatures_enabled());

        let json = settings.to_json_pretty().unwrap();
        let v: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["terminal"]["ligatures"], false);
        assert!(
            v["terminal"].get("font_features").is_none(),
            "expected default calt state to serialize via terminal.ligatures only"
        );
    }

    #[test]
    fn terminal_ligatures_helpers_toggle_calt_feature() {
        let mut settings = TerminalSettings::default();

        assert!(settings.ligatures_enabled());
        assert_eq!(settings.font_features.is_calt_enabled(), Some(true));

        settings.set_ligatures_enabled(false);
        assert!(!settings.ligatures_enabled());
        assert_eq!(settings.font_features.is_calt_enabled(), Some(false));

        settings.set_ligatures_enabled(true);
        assert!(settings.ligatures_enabled());
        assert_eq!(settings.font_features.is_calt_enabled(), Some(true));
    }

    #[test]
    fn terminal_ligatures_helpers_preserve_other_font_features() {
        let mut settings = TerminalSettings::default();
        settings.font_features = gpui::FontFeatures(std::sync::Arc::new(vec![
            ("ss01".into(), 1),
            ("calt".into(), 0),
        ]));

        settings.set_ligatures_enabled(true);

        assert_eq!(
            settings.font_features.tag_value_list(),
            &[("ss01".into(), 1), ("calt".into(), 1)]
        );
    }

    #[test]
    fn terminal_ligatures_roundtrip_preserves_non_calt_font_features() {
        let mut settings = SettingsFile::default();
        settings.terminal.font_features = gpui::FontFeatures(std::sync::Arc::new(vec![
            ("ss01".into(), 1),
            ("calt".into(), 0),
        ]));

        let json = settings.to_json_pretty().unwrap();
        let v: Value = serde_json::from_str(&json).unwrap();

        assert_eq!(v["terminal"]["ligatures"], false);
        assert_eq!(v["terminal"]["font_features"]["ss01"], 1);
        assert!(
            v["terminal"]["font_features"].get("calt").is_none(),
            "expected calt to be represented by terminal.ligatures instead of font_features"
        );
    }

    #[test]
    fn terminal_sftp_upload_max_concurrency_is_clamped() {
        let settings = SettingsFile::load_from_str_lenient(
            r#"
            {
              "terminal": {
                "sftp_upload_max_concurrency": 1
              }
            }
            "#,
        )
        .unwrap();
        assert_eq!(settings.terminal.sftp_upload_max_concurrency, 2);

        let settings = SettingsFile::load_from_str_lenient(
            r#"
            {
              "terminal": {
                "sftp_upload_max_concurrency": 999
              }
            }
            "#,
        )
        .unwrap();
        assert_eq!(settings.terminal.sftp_upload_max_concurrency, 15);
    }

    #[gpui::test]
    fn apply_to_app_resizes_global_sftp_permit_pool(cx: &mut gpui::TestAppContext) {
        let mut settings = SettingsFile::default();
        settings.terminal.settings.sftp_upload_max_concurrency = 2;

        let mut app = cx.app.borrow_mut();
        gpui_term::init(&mut app);
        settings.apply_to_app(None, &mut app);

        let pool = gpui_common::sftp_upload_permit_pool_in_app(&mut app);
        assert_eq!(pool.max(), 2);
    }

    #[test]
    fn default_settings_json_is_serializable() {
        // This test is the "red" for Task 2 (TerminalSettings must implement Serialize).
        let _ = serde_json::to_string(&SettingsFile::default()).unwrap();
    }

    #[gpui::test]
    fn apply_updates_runtime_globals(cx: &mut gpui::TestAppContext) {
        let mut app = cx.app.borrow_mut();
        gpui_term::init(&mut app);

        let mut settings = SettingsFile::default();
        settings.appearance.theme = ThemeMode::Dark;
        settings.recording.include_input_by_default = true;
        settings.recording.playback_speed = 1.5;
        settings.terminal.font_size = px(20.0);

        settings.apply_to_app(None, &mut app);

        assert_eq!(app.global::<TermSettings>().font_size, px(20.0));
        assert_eq!(theme_mode(&app), ThemeMode::Dark);
        assert_eq!(
            app.global::<gpui_term::cast::CastRecordingConfig>()
                .include_input_by_default,
            true
        );
        assert_eq!(app.global::<RecordingSettings>().playback_speed, 1.5);
    }

    #[gpui::test]
    fn apply_to_app_applies_selected_dark_theme(cx: &mut gpui::TestAppContext) {
        use gpui_component::{ActiveTheme, Colorize};

        let themes_dir =
            std::env::temp_dir().join(format!("termua-settings-theme-test-{}", std::process::id()));

        std::fs::create_dir_all(&themes_dir).unwrap();
        std::fs::write(
            themes_dir.join("test-theme.json"),
            r##"
{
  "name": "Test Themes",
  "themes": [
    {
      "name": "My Dark",
      "mode": "dark",
      "colors": {
        "background": "#000000ff",
        "foreground": "#dce0e5ff"
      }
    }
  ]
}
"##,
        )
        .unwrap();

        {
            let mut app = cx.app.borrow_mut();
            menubar::init(&mut app);
            gpui_term::init(&mut app);

            gpui_component::ThemeRegistry::watch_dir(themes_dir, &mut app, |_cx| {}).unwrap();
        }

        cx.run_until_parked();

        let mut app = cx.app.borrow_mut();
        let mut settings = SettingsFile::default();
        settings.appearance.theme = ThemeMode::Dark;
        settings.appearance.dark_theme = Some("My Dark".to_string());

        settings.apply_to_app(None, &mut app);
        assert_eq!(app.theme().background.to_hex(), "#000000");
    }

    #[test]
    fn load_user_keybinding_treats_whitespace_as_none() {
        assert!(load_user_keybinding("   ", Box::new(NoAction {}), Some("Terminal")).is_none());
        assert!(load_user_keybinding("\n\t", Box::new(NoAction {}), Some("Terminal")).is_none());
    }

    #[test]
    fn load_user_keybinding_rejects_invalid_strings_without_panicking() {
        assert!(
            load_user_keybinding("ctrl-a-b", Box::new(NoAction {}), Some("Terminal")).is_none()
        );
    }

    #[cfg(unix)]
    #[test]
    fn save_to_path_replaces_existing_file_atomically_and_preserves_mode() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "termua-settings-atomic-save-test-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        let path = dir.join("settings.json");
        std::fs::write(&path, "old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();

        let ino_before = std::fs::metadata(&path).unwrap().ino();
        SettingsFile::default().save_to_path(&path).unwrap();

        let meta_after = std::fs::metadata(&path).unwrap();
        let ino_after = meta_after.ino();
        let mode_after = meta_after.permissions().mode() & 0o777;

        assert_ne!(
            ino_before, ino_after,
            "atomic save should replace the file (rename) instead of truncating it in place"
        );
        assert_eq!(
            mode_after, 0o600,
            "atomic save should preserve existing file permissions"
        );

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn save_to_path_creates_private_file_on_unix() {
        use std::os::unix::fs::PermissionsExt;

        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "termua-settings-private-save-test-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        let path = dir.join("settings.json");
        let _ = std::fs::remove_file(&path);

        SettingsFile::default().save_to_path(&path).unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "settings.json should be 0600 on Unix");

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
