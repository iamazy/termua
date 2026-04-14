use rust_i18n::t;

use super::SettingsPage;

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum SettingsNavSection {
    Appearance,
    Terminal,
    Recording,
    Logging,
    Assistant,
    Security,
}

#[derive(Clone, Debug)]
pub struct SettingMeta {
    pub id: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub keywords: &'static [&'static str],
    pub section: SettingsNavSection,
    pub page: SettingsPage,
}

static ALL_SETTINGS_META: &[SettingMeta] = &[
    SettingMeta {
        id: "lock_screen.enabled",
        title: "Enabled",
        description: "Enable Termua's lock screen feature.",
        keywords: &["lock", "lock_screen", "security", "privacy", "enabled"],
        section: SettingsNavSection::Security,
        page: SettingsPage::SecurityLockScreen,
    },
    SettingMeta {
        id: "lock_screen.timeout_secs",
        title: "Lock after",
        description: "Automatically lock after being idle for the selected duration.",
        keywords: &["lock", "timeout", "idle", "security", "minutes", "never"],
        section: SettingsNavSection::Security,
        page: SettingsPage::SecurityLockScreen,
    },
    SettingMeta {
        id: "appearance.theme",
        title: "Theme",
        description: "Choose System/Light/Dark theme mode.",
        keywords: &["appearance", "theme", "light", "dark", "system"],
        section: SettingsNavSection::Appearance,
        page: SettingsPage::AppearanceTheme,
    },
    SettingMeta {
        id: "appearance.light_theme",
        title: "Light theme",
        description: "Choose the light theme colors.",
        keywords: &["appearance", "theme", "light", "colors"],
        section: SettingsNavSection::Appearance,
        page: SettingsPage::AppearanceTheme,
    },
    SettingMeta {
        id: "appearance.dark_theme",
        title: "Dark theme",
        description: "Choose the dark theme colors.",
        keywords: &["appearance", "theme", "dark", "colors"],
        section: SettingsNavSection::Appearance,
        page: SettingsPage::AppearanceTheme,
    },
    SettingMeta {
        id: "appearance.language",
        title: "Language",
        description: "Choose the application UI language.",
        keywords: &[
            "appearance",
            "language",
            "lang",
            "locale",
            "中文",
            "english",
        ],
        section: SettingsNavSection::Appearance,
        page: SettingsPage::AppearanceLanguage,
    },
    SettingMeta {
        id: "terminal.default_backend",
        title: "Default backend",
        description: "Choose the default terminal backend.",
        keywords: &["terminal", "backend", "default", "alacritty", "wezterm"],
        section: SettingsNavSection::Terminal,
        page: SettingsPage::Terminal,
    },
    SettingMeta {
        id: "terminal.ssh_backend",
        title: "SSH Backend",
        description: "Used for SSH terminals and SFTP.",
        keywords: &["terminal", "ssh", "backend", "ssh2", "libssh", "sftp"],
        section: SettingsNavSection::Terminal,
        page: SettingsPage::Terminal,
    },
    SettingMeta {
        id: "terminal.font_family",
        title: "Font family",
        description: "Font family for terminal text.",
        keywords: &["terminal", "font", "family", "typeface"],
        section: SettingsNavSection::Terminal,
        page: SettingsPage::TerminalFont,
    },
    SettingMeta {
        id: "terminal.font_size",
        title: "Font size",
        description: "Font size in pixels.",
        keywords: &["terminal", "font", "size"],
        section: SettingsNavSection::Terminal,
        page: SettingsPage::TerminalFont,
    },
    SettingMeta {
        id: "terminal.ligatures",
        title: "Ligatures",
        description: "Enable font ligatures for terminal text.",
        keywords: &["terminal", "font", "ligatures", "calt", "programming"],
        section: SettingsNavSection::Terminal,
        page: SettingsPage::TerminalFont,
    },
    SettingMeta {
        id: "terminal.cursor_shape",
        title: "Cursor shape",
        description: "Shape used to render the cursor.",
        keywords: &[
            "terminal",
            "cursor",
            "shape",
            "block",
            "underline",
            "bar",
            "hollow",
        ],
        section: SettingsNavSection::Terminal,
        page: SettingsPage::TerminalCursor,
    },
    SettingMeta {
        id: "terminal.blinking",
        title: "Cursor blinking",
        description: "Controls whether the cursor blinks.",
        keywords: &["terminal", "cursor", "blink", "blinking"],
        section: SettingsNavSection::Terminal,
        page: SettingsPage::TerminalCursor,
    },
    SettingMeta {
        id: "terminal.show_scrollbar",
        title: "Show scrollbar",
        description: "Whether to render the scrollbar.",
        keywords: &["terminal", "rendering", "scrollbar", "scroll"],
        section: SettingsNavSection::Terminal,
        page: SettingsPage::TerminalRendering,
    },
    SettingMeta {
        id: "terminal.show_line_numbers",
        title: "Show line numbers",
        description: "Whether to render line numbers.",
        keywords: &["terminal", "rendering", "line", "numbers"],
        section: SettingsNavSection::Terminal,
        page: SettingsPage::TerminalRendering,
    },
    SettingMeta {
        id: "terminal.option_as_meta",
        title: "Option as Meta",
        description: "Treat Option/Alt as Meta for terminal input.",
        keywords: &["terminal", "behavior", "option", "alt", "meta"],
        section: SettingsNavSection::Terminal,
        page: SettingsPage::TerminalBehavior,
    },
    SettingMeta {
        id: "terminal.copy_on_select",
        title: "Copy on select",
        description: "Copy selection to clipboard when selecting text.",
        keywords: &[
            "terminal",
            "behavior",
            "copy",
            "select",
            "selection",
            "clipboard",
        ],
        section: SettingsNavSection::Terminal,
        page: SettingsPage::TerminalBehavior,
    },
    SettingMeta {
        id: "terminal.sftp_upload_max_concurrency",
        title: "SFTP upload concurrency",
        description: "Maximum number of files to upload concurrently via SFTP.",
        keywords: &[
            "terminal",
            "sftp",
            "upload",
            "concurrency",
            "parallel",
            "transfer",
        ],
        section: SettingsNavSection::Terminal,
        page: SettingsPage::TerminalSftp,
    },
    SettingMeta {
        id: "terminal.suggestions_enabled",
        title: "Suggestions",
        description: "Show inline command suggestions in shell-like contexts.",
        keywords: &[
            "terminal",
            "suggestions",
            "autocomplete",
            "completion",
            "history",
        ],
        section: SettingsNavSection::Terminal,
        page: SettingsPage::TerminalSuggestions,
    },
    SettingMeta {
        id: "terminal.suggestions_max_items",
        title: "Suggestions max items",
        description: "Maximum number of suggestions to show.",
        keywords: &[
            "terminal",
            "suggestions",
            "autocomplete",
            "completion",
            "max",
        ],
        section: SettingsNavSection::Terminal,
        page: SettingsPage::TerminalSuggestions,
    },
    SettingMeta {
        id: "terminal.suggestions_json_dir",
        title: "Suggestions JSON dir",
        description: "Load suggestions from JSON files in the suggestions.d directory.",
        keywords: &[
            "terminal",
            "suggestions",
            "json",
            "dir",
            "directory",
            "reload",
        ],
        section: SettingsNavSection::Terminal,
        page: SettingsPage::TerminalSuggestions,
    },
    SettingMeta {
        id: "sharing.enabled",
        title: "Sharing enabled",
        description: "Enable terminal sharing via a relay server.",
        keywords: &[
            "terminal",
            "sharing",
            "share",
            "screen",
            "relay",
            "websocket",
            "enabled",
        ],
        section: SettingsNavSection::Terminal,
        page: SettingsPage::TerminalSharing,
    },
    SettingMeta {
        id: "sharing.relay_url",
        title: "Relay URL",
        description: "WebSocket URL of the relay server.",
        keywords: &["terminal", "sharing", "relay", "url", "ws", "wss", "server"],
        section: SettingsNavSection::Terminal,
        page: SettingsPage::TerminalSharing,
    },
    SettingMeta {
        id: "sharing.local_relay",
        title: "Local relay",
        description: "Start/stop a local relay process (test only).",
        keywords: &[
            "terminal", "sharing", "relay", "local", "test", "process", "ws", "server",
        ],
        section: SettingsNavSection::Terminal,
        page: SettingsPage::TerminalSharing,
    },
    SettingMeta {
        id: "terminal.keybindings.copy",
        title: "Copy keybinding",
        description: "Keybinding for Copy action in the terminal.",
        keywords: &["terminal", "keybinding", "shortcut", "copy", "clipboard"],
        section: SettingsNavSection::Terminal,
        page: SettingsPage::TerminalKeyBindings,
    },
    SettingMeta {
        id: "terminal.keybindings.paste",
        title: "Paste keybinding",
        description: "Keybinding for Paste action in the terminal.",
        keywords: &["terminal", "keybinding", "shortcut", "paste", "clipboard"],
        section: SettingsNavSection::Terminal,
        page: SettingsPage::TerminalKeyBindings,
    },
    SettingMeta {
        id: "terminal.keybindings.select_all",
        title: "Select all keybinding",
        description: "Keybinding for Select All action in the terminal.",
        keywords: &["terminal", "keybinding", "shortcut", "select", "all"],
        section: SettingsNavSection::Terminal,
        page: SettingsPage::TerminalKeyBindings,
    },
    SettingMeta {
        id: "terminal.keybindings.clear",
        title: "Clear keybinding",
        description: "Keybinding for Clear action in the terminal.",
        keywords: &["terminal", "keybinding", "shortcut", "clear"],
        section: SettingsNavSection::Terminal,
        page: SettingsPage::TerminalKeyBindings,
    },
    SettingMeta {
        id: "terminal.keybindings.search",
        title: "Search keybinding",
        description: "Keybinding to open terminal search.",
        keywords: &["terminal", "keybinding", "shortcut", "search", "find"],
        section: SettingsNavSection::Terminal,
        page: SettingsPage::TerminalKeyBindings,
    },
    SettingMeta {
        id: "terminal.keybindings.search_next",
        title: "Search next keybinding",
        description: "Keybinding for searching next match in terminal search.",
        keywords: &["terminal", "keybinding", "shortcut", "search", "next"],
        section: SettingsNavSection::Terminal,
        page: SettingsPage::TerminalKeyBindings,
    },
    SettingMeta {
        id: "terminal.keybindings.search_previous",
        title: "Search previous keybinding",
        description: "Keybinding for searching previous match in terminal search.",
        keywords: &[
            "terminal",
            "keybinding",
            "shortcut",
            "search",
            "previous",
            "prev",
        ],
        section: SettingsNavSection::Terminal,
        page: SettingsPage::TerminalKeyBindings,
    },
    SettingMeta {
        id: "terminal.keybindings.increase_font_size",
        title: "Increase font size keybinding",
        description: "Keybinding to increase terminal font size.",
        keywords: &[
            "terminal",
            "keybinding",
            "shortcut",
            "font",
            "increase",
            "zoom",
        ],
        section: SettingsNavSection::Terminal,
        page: SettingsPage::TerminalKeyBindings,
    },
    SettingMeta {
        id: "terminal.keybindings.decrease_font_size",
        title: "Decrease font size keybinding",
        description: "Keybinding to decrease terminal font size.",
        keywords: &[
            "terminal",
            "keybinding",
            "shortcut",
            "font",
            "decrease",
            "zoom",
        ],
        section: SettingsNavSection::Terminal,
        page: SettingsPage::TerminalKeyBindings,
    },
    SettingMeta {
        id: "terminal.keybindings.reset_font_size",
        title: "Reset font size keybinding",
        description: "Keybinding to reset terminal font size.",
        keywords: &[
            "terminal",
            "keybinding",
            "shortcut",
            "font",
            "reset",
            "zoom",
        ],
        section: SettingsNavSection::Terminal,
        page: SettingsPage::TerminalKeyBindings,
    },
    SettingMeta {
        id: "recording.include_input_by_default",
        title: "Include input by default",
        description: "Whether to include input events when recording casts.",
        keywords: &["recording", "cast", "input"],
        section: SettingsNavSection::Recording,
        page: SettingsPage::RecordingCast,
    },
    SettingMeta {
        id: "recording.playback_speed",
        title: "Playback speed",
        description: "Default playback speed used when opening cast files.",
        keywords: &["recording", "cast", "playback", "speed"],
        section: SettingsNavSection::Recording,
        page: SettingsPage::RecordingCast,
    },
    SettingMeta {
        id: "logging.level",
        title: "Log level",
        description: "Verbosity of logs (requires restart).",
        keywords: &["logging", "log", "level", "verbosity", "debug", "trace"],
        section: SettingsNavSection::Logging,
        page: SettingsPage::Logging,
    },
    SettingMeta {
        id: "logging.path",
        title: "Log path",
        description: "Optional log file path (requires restart).",
        keywords: &["logging", "log", "path", "file", "output"],
        section: SettingsNavSection::Logging,
        page: SettingsPage::Logging,
    },
    SettingMeta {
        id: "assistant.enabled",
        title: "Enabled",
        description: "Enable the embedded assistant (ZeroClaw).",
        keywords: &["assistant", "ai", "llm", "enabled", "zeroclaw"],
        section: SettingsNavSection::Assistant,
        page: SettingsPage::Assistant,
    },
    SettingMeta {
        id: "assistant.status",
        title: "ZeroClaw status",
        description: "Checks whether the local ZeroClaw service is running.",
        keywords: &["assistant", "zeroclaw", "status", "health", "alive"],
        section: SettingsNavSection::Assistant,
        page: SettingsPage::Assistant,
    },
    SettingMeta {
        id: "assistant.provider",
        title: "Provider",
        description: "LLM provider name (zeroclaw provider id). Empty = use zeroclaw default.",
        keywords: &["assistant", "ai", "llm", "provider", "zeroclaw"],
        section: SettingsNavSection::Assistant,
        page: SettingsPage::Assistant,
    },
    SettingMeta {
        id: "assistant.model",
        title: "Model",
        description: "Model name. Empty = use zeroclaw default.",
        keywords: &["assistant", "ai", "llm", "model", "zeroclaw"],
        section: SettingsNavSection::Assistant,
        page: SettingsPage::Assistant,
    },
    SettingMeta {
        id: "assistant.temperature",
        title: "Temperature",
        description: "Optional temperature override. Empty = use zeroclaw default.",
        keywords: &["assistant", "ai", "llm", "temperature", "sampling"],
        section: SettingsNavSection::Assistant,
        page: SettingsPage::Assistant,
    },
    SettingMeta {
        id: "assistant.api_url",
        title: "API URL",
        description: "Optional custom base URL. Empty = provider default.",
        keywords: &["assistant", "api", "url", "endpoint", "base_url"],
        section: SettingsNavSection::Assistant,
        page: SettingsPage::Assistant,
    },
    SettingMeta {
        id: "assistant.api_path",
        title: "API path",
        description: "Optional OpenAI-compatible API path suffix. Empty = provider default.",
        keywords: &["assistant", "api", "path", "openai", "compatible"],
        section: SettingsNavSection::Assistant,
        page: SettingsPage::Assistant,
    },
    SettingMeta {
        id: "assistant.provider_timeout_secs",
        title: "Timeout (seconds)",
        description: "Optional provider request timeout in seconds. Empty = provider default.",
        keywords: &["assistant", "timeout", "seconds", "request"],
        section: SettingsNavSection::Assistant,
        page: SettingsPage::Assistant,
    },
    SettingMeta {
        id: "assistant.extra_headers",
        title: "Extra headers",
        description: "Optional extra HTTP headers for provider API requests.",
        keywords: &["assistant", "headers", "http", "request"],
        section: SettingsNavSection::Assistant,
        page: SettingsPage::Assistant,
    },
    SettingMeta {
        id: "assistant.api_key",
        title: "API key",
        description: "Stored in the OS keychain (not in settings.json).",
        keywords: &["assistant", "api", "key", "token", "secret", "keychain"],
        section: SettingsNavSection::Assistant,
        page: SettingsPage::Assistant,
    },
];

impl SettingMeta {
    pub fn all() -> &'static [Self] {
        ALL_SETTINGS_META
    }

    pub fn localized_title(&self) -> String {
        let key = format!("Settings.Meta.{}.Title", self.id);
        let localized = t!(key.as_str()).to_string();
        if localized == key {
            self.title.to_string()
        } else {
            localized
        }
    }

    pub fn localized_description(&self) -> String {
        let key = format!("Settings.Meta.{}.Description", self.id);
        let localized = t!(key.as_str()).to_string();
        if localized == key {
            self.description.to_string()
        } else {
            localized
        }
    }
}

fn contains_ascii_case_insensitive(haystack: &str, query: &[u8]) -> bool {
    if query.is_empty() {
        return true;
    }

    let bytes = haystack.as_bytes();
    if query.len() > bytes.len() {
        return false;
    }

    for start in 0..=bytes.len().saturating_sub(query.len()) {
        let mut ok = true;
        for (ix, &q) in query.iter().enumerate() {
            let h = bytes[start + ix].to_ascii_lowercase();
            if h != q {
                ok = false;
                break;
            }
        }
        if ok {
            return true;
        }
    }
    false
}

fn cmp_ascii_case_insensitive(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;

    let ab = a.as_bytes();
    let bb = b.as_bytes();
    for (&x, &y) in ab.iter().zip(bb.iter()) {
        let x = x.to_ascii_lowercase();
        let y = y.to_ascii_lowercase();
        match x.cmp(&y) {
            Ordering::Equal => {}
            ord => return ord,
        }
    }
    ab.len().cmp(&bb.len())
}

pub fn search_settings<'a>(entries: &'a [SettingMeta], query: &str) -> Vec<&'a SettingMeta> {
    let q = query.trim();
    if q.is_empty() {
        return Vec::new();
    }
    let q_lc = q.to_ascii_lowercase();
    let q_lc = q_lc.as_bytes();

    let mut results: Vec<(&SettingMeta, String)> = Vec::new();
    for entry in entries {
        let title = entry.localized_title();
        let description = entry.localized_description();
        if contains_ascii_case_insensitive(entry.id, q_lc)
            || contains_ascii_case_insensitive(&title, q_lc)
            || contains_ascii_case_insensitive(&description, q_lc)
            || entry
                .keywords
                .iter()
                .copied()
                .any(|k| contains_ascii_case_insensitive(k, q_lc))
        {
            results.push((entry, title));
        }
    }

    results.sort_by(|(a, a_title), (b, b_title)| {
        cmp_ascii_case_insensitive(a_title, b_title).then_with(|| a.id.cmp(b.id))
    });

    results.into_iter().map(|(entry, _)| entry).collect()
}
