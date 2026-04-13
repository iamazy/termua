use std::collections::HashMap;

use gpui::{
    AbsoluteLength, FontFallbacks, FontFeatures, FontWeight, Global, Pixels, SharedString, px,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TerminalSettings {
    pub font_size: Pixels,
    pub font_family: SharedString,
    pub font_fallbacks: Option<FontFallbacks>,
    pub font_features: FontFeatures,
    pub font_weight: FontWeight,
    pub line_height: TerminalLineHeight,
    pub env: HashMap<String, String>,
    pub cursor_shape: Option<CursorShape>,
    pub blinking: TerminalBlink,
    pub option_as_meta: bool,
    pub copy_on_select: bool,
    /// Maximum number of concurrent SFTP uploads.
    #[serde(default = "default_sftp_upload_max_concurrency")]
    pub sftp_upload_max_concurrency: usize,
    pub minimum_contrast: f32,
    /// Whether to render the scrollbar.
    #[serde(default = "default_true")]
    pub show_scrollbar: bool,
    /// Whether to render line numbers to the left of the terminal content.
    #[serde(default)]
    pub show_line_numbers: bool,
    /// Whether to show inline command suggestions in shell-like contexts.
    #[serde(default)]
    pub suggestions_enabled: bool,
    /// Maximum number of suggestions to show.
    #[serde(default = "default_suggestions_max_items")]
    pub suggestions_max_items: usize,
}

impl TerminalSettings {
    pub fn new() -> Self {
        Self {
            font_size: px(15.),
            // Prefer a portable default. Users can override via settings.
            font_family: SharedString::new_static(".ZedMono"),
            font_fallbacks: None,
            font_features: FontFeatures::disable_ligatures(),
            font_weight: FontWeight::NORMAL,
            line_height: TerminalLineHeight::Comfortable,
            env: Default::default(),
            cursor_shape: None,
            blinking: TerminalBlink::On,
            option_as_meta: false,
            copy_on_select: true,
            sftp_upload_max_concurrency: default_sftp_upload_max_concurrency(),
            minimum_contrast: 45.,
            show_scrollbar: true,
            show_line_numbers: true,
            suggestions_enabled: false,
            suggestions_max_items: default_suggestions_max_items(),
        }
    }
}

impl Default for TerminalSettings {
    fn default() -> Self {
        Self::new()
    }
}

impl Global for TerminalSettings {}

fn default_true() -> bool {
    true
}

fn default_suggestions_max_items() -> usize {
    8
}

fn default_sftp_upload_max_concurrency() -> usize {
    5
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum TerminalLineHeight {
    /// Use a line height that's comfortable for reading, 1.618
    #[default]
    Comfortable,
    /// Use a standard line height, 1.3. This option is useful for TUIs,
    /// particularly if they use box characters
    Standard,
    /// Use a custom line height.
    Custom(f32),
}

impl TerminalLineHeight {
    pub fn value(&self) -> AbsoluteLength {
        let value = match self {
            TerminalLineHeight::Comfortable => 1.618,
            TerminalLineHeight::Standard => 1.3,
            TerminalLineHeight::Custom(line_height) => f32::max(*line_height, 1.),
        };
        px(value).into()
    }
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TerminalBlink {
    /// Never blink the cursor, ignoring the terminal mode.
    Off,
    /// Default the cursor blink to off, but allow the terminal to
    /// set blinking.
    TerminalControlled,
    /// Always blink the cursor, ignoring the terminal mode.
    On,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CursorShape {
    /// Cursor is a block like `█`.
    #[default]
    Block,
    /// Cursor is an underscore like `_`.
    Underline,
    /// Cursor is a vertical bar like `⎸`.
    Bar,
    /// Cursor is a hollow box like `▯`.
    Hollow,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_settings_defaults_include_sftp_upload_concurrency() {
        assert_eq!(TerminalSettings::default().sftp_upload_max_concurrency, 5);
    }
}
