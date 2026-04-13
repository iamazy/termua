use std::ops::RangeInclusive;

use bitflags::bitflags;

mod backends;
mod builder;
pub mod cast;
pub mod command_blocks;
mod element;
pub mod remote;
mod serial;
mod settings;
pub mod shell;
mod snippet;
mod suggestions;
mod terminal;
mod theme;
mod util;
mod view;

pub use backends::{
    PtySource,
    remote::{RemoteBackend, RemoteBackendEvent},
    ssh::{
        Authentication, SshBackend, SshHostVerificationPrompt, SshHostVerificationPromptGuard,
        SshJumpChain, SshJumpHop, SshOptions, SshProxyCommand, SshProxyMode,
        set_thread_ssh_host_verification_prompt_sender,
    },
};
pub use builder::*;
pub use cast::CastRecordingOptions;
pub use serial::*;
pub use settings::*;
pub use suggestions::{SuggestionHistoryProvider, SuggestionStaticProvider};
pub use terminal::*;
pub use theme::*;
pub use view::*;

pub fn init(cx: &mut gpui::App) {
    gpui_component::init(cx);

    cx.set_global(TerminalSettings::new());
    cx.set_global(cast::CastRecordingConfig::default());
    cx.set_global(suggestions::SuggestionHistoryConfig::default());
    cx.set_global(suggestions::SuggestionStaticConfig::default());

    #[cfg(target_os = "macos")]
    let keys = [
        gpui::KeyBinding::new("cmd-a", SelectAll, Some("Terminal")),
        gpui::KeyBinding::new("cmd-v", Paste, Some("Terminal")),
        gpui::KeyBinding::new("cmd-c", Copy, Some("Terminal")),
        gpui::KeyBinding::new("cmd-k", Clear, Some("Terminal")),
        gpui::KeyBinding::new("cmd-f", terminal::Search, Some("Terminal")),
        gpui::KeyBinding::new("cmd-g", terminal::SearchNext, Some("Terminal")),
        gpui::KeyBinding::new("cmd-shift-g", terminal::SearchPrevious, Some("Terminal")),
        gpui::KeyBinding::new("escape", terminal::SearchClose, Some("Terminal && search")),
        gpui::KeyBinding::new("cmd-v", terminal::SearchPaste, Some("Terminal && search")),
        gpui::KeyBinding::new(
            "tab",
            SendKeystroke::new("tab"),
            Some("Terminal && !search"),
        ),
        gpui::KeyBinding::new(
            "shift-tab",
            SendKeystroke::new("shift-tab"),
            Some("Terminal && !search"),
        ),
        gpui::KeyBinding::new("cmd-+", IncreaseFontSize, Some("Terminal")),
        gpui::KeyBinding::new("cmd-=", IncreaseFontSize, Some("Terminal")),
        gpui::KeyBinding::new("cmd--", DecreaseFontSize, Some("Terminal")),
        gpui::KeyBinding::new("cmd-0", ResetFontSize, Some("Terminal")),
    ];
    #[cfg(not(target_os = "macos"))]
    let keys = [
        gpui::KeyBinding::new("ctrl-shift-a", SelectAll, Some("Terminal")),
        gpui::KeyBinding::new("ctrl-shift-v", Paste, Some("Terminal")),
        gpui::KeyBinding::new("ctrl-shift-c", Copy, Some("Terminal")),
        gpui::KeyBinding::new("ctrl-shift-k", Clear, Some("Terminal")),
        gpui::KeyBinding::new("ctrl-shift-f", terminal::Search, Some("Terminal")),
        gpui::KeyBinding::new("ctrl-g", terminal::SearchNext, Some("Terminal")),
        gpui::KeyBinding::new("ctrl-shift-g", terminal::SearchPrevious, Some("Terminal")),
        gpui::KeyBinding::new("escape", terminal::SearchClose, Some("Terminal && search")),
        gpui::KeyBinding::new(
            "ctrl-shift-v",
            terminal::SearchPaste,
            Some("Terminal && search"),
        ),
        gpui::KeyBinding::new(
            "tab",
            SendKeystroke::new("tab"),
            Some("Terminal && !search"),
        ),
        gpui::KeyBinding::new(
            "shift-tab",
            SendKeystroke::new("shift-tab"),
            Some("Terminal && !search"),
        ),
        gpui::KeyBinding::new("ctrl-+", IncreaseFontSize, Some("Terminal")),
        gpui::KeyBinding::new("ctrl-=", IncreaseFontSize, Some("Terminal")),
        gpui::KeyBinding::new("ctrl--", DecreaseFontSize, Some("Terminal")),
        gpui::KeyBinding::new("ctrl-0", ResetFontSize, Some("Terminal")),
    ];
    cx.bind_keys(keys);
}

pub fn set_suggestion_history_provider(
    cx: &mut gpui::App,
    provider: Option<std::sync::Arc<dyn SuggestionHistoryProvider>>,
) {
    cx.global_mut::<suggestions::SuggestionHistoryConfig>()
        .provider = provider;
}

pub fn set_suggestion_static_provider(
    cx: &mut gpui::App,
    provider: Option<std::sync::Arc<dyn SuggestionStaticProvider>>,
) {
    let cfg = cx.global_mut::<suggestions::SuggestionStaticConfig>();
    cfg.provider = provider;
    cfg.epoch = cfg.epoch.wrapping_add(1);
}

pub fn set_cast_recording_default_dir(cx: &mut gpui::App, dir: Option<std::path::PathBuf>) {
    cx.global_mut::<cast::CastRecordingConfig>().default_dir = dir;
}

pub fn set_cast_recording_path_provider(
    cx: &mut gpui::App,
    provider: Option<std::sync::Arc<dyn Send + Sync + Fn() -> Option<std::path::PathBuf>>>,
) {
    cx.global_mut::<cast::CastRecordingConfig>().request_path = provider;
}

pub fn set_cast_recording_include_input_by_default(cx: &mut gpui::App, enabled: bool) {
    cx.global_mut::<cast::CastRecordingConfig>()
        .include_input_by_default = enabled;
}

/// A point in the terminal grid.
///
/// `line` may be negative when referencing scrollback positions (backend-defined).
#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct GridPoint {
    pub line: i32,
    pub column: usize,
}

impl GridPoint {
    pub const fn new(line: i32, column: usize) -> Self {
        Self { line, column }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SelectionRange {
    pub start: GridPoint,
    pub end: GridPoint,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum CursorRenderShape {
    Hidden,
    Block,
    Underline,
    Bar,
    Hollow,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct Cursor {
    pub shape: CursorRenderShape,
    pub point: GridPoint,
}

bitflags! {
    /// A backend-neutral subset of terminal modes used by the GPUI widget.
    #[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
    pub struct TerminalMode: u32 {
        const ALT_SCREEN        = 1 << 0;
        const BRACKETED_PASTE   = 1 << 1;
        const FOCUS_IN_OUT      = 1 << 2;

        const SGR_MOUSE         = 1 << 3;
        const UTF8_MOUSE        = 1 << 4;
        const MOUSE_MODE        = 1 << 5;
        const ALTERNATE_SCROLL  = 1 << 6;

        const APP_CURSOR        = 1 << 7;
        const APP_KEYPAD        = 1 << 8;
        const SHOW_CURSOR       = 1 << 9;
        const LINE_WRAP         = 1 << 10;

        const ORIGIN            = 1 << 11;
        const INSERT            = 1 << 12;
        const LINE_FEED_NEW_LINE= 1 << 13;

        const MOUSE_REPORT_CLICK= 1 << 14;
        const MOUSE_DRAG        = 1 << 15;
        const MOUSE_MOTION      = 1 << 16;
    }
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash, serde::Serialize, serde::Deserialize)]
pub enum NamedColor {
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    White,

    BrightBlack,
    BrightRed,
    BrightGreen,
    BrightYellow,
    BrightBlue,
    BrightMagenta,
    BrightCyan,
    BrightWhite,

    Foreground,
    Background,
    Cursor,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub enum TermColor {
    Named(NamedColor),
    Indexed(u8),
    Rgb(u8, u8, u8),
}

bitflags! {
    #[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
    pub struct CellFlags: u32 {
        const INVERSE          = 1 << 0;
        const WRAPLINE         = 1 << 1;
        const WIDE_CHAR_SPACER = 1 << 2;

        const BOLD             = 1 << 3;
        const DIM              = 1 << 4;
        const ITALIC           = 1 << 5;
        const UNDERLINE        = 1 << 6;
        const DOUBLE_UNDERLINE = 1 << 7;
        const CURLY_UNDERLINE  = 1 << 8;
        const DOTTED_UNDERLINE = 1 << 9;
        const DASHED_UNDERLINE = 1 << 10;
        const STRIKEOUT        = 1 << 11;
    }
}

#[derive(Clone, Debug)]
pub struct Cell {
    pub c: char,
    pub fg: TermColor,
    pub bg: TermColor,
    pub flags: CellFlags,
    pub hyperlink: Option<String>,
    pub zerowidth: Vec<char>,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            c: ' ',
            fg: TermColor::Named(NamedColor::Foreground),
            bg: TermColor::Named(NamedColor::Background),
            flags: CellFlags::empty(),
            hyperlink: None,
            zerowidth: Vec::new(),
        }
    }
}

impl Cell {
    #[inline]
    pub fn hyperlink(&self) -> Option<&str> {
        self.hyperlink.as_deref()
    }

    #[inline]
    pub fn zerowidth(&self) -> Option<&[char]> {
        (!self.zerowidth.is_empty()).then_some(self.zerowidth.as_slice())
    }
}

#[derive(Debug, Clone)]
pub struct IndexedCell {
    pub point: GridPoint,
    pub cell: Cell,
}

impl std::ops::Deref for IndexedCell {
    type Target = Cell;

    #[inline]
    fn deref(&self) -> &Cell {
        &self.cell
    }
}

#[derive(Clone)]
pub struct TerminalContent {
    pub cells: Vec<IndexedCell>,
    pub mode: TerminalMode,
    pub display_offset: usize,
    pub selection_text: Option<String>,
    pub selection: Option<SelectionRange>,
    pub cursor: Cursor,
    pub cursor_char: char,
    pub terminal_bounds: crate::terminal::TerminalBounds,
    pub last_hovered_word: Option<HoveredWord>,
    pub scrolled_to_top: bool,
    pub scrolled_to_bottom: bool,
}

impl Default for TerminalContent {
    fn default() -> Self {
        Self {
            cells: Vec::new(),
            mode: TerminalMode::empty(),
            display_offset: 0,
            selection_text: None,
            selection: None,
            cursor: Cursor {
                shape: CursorRenderShape::Hidden,
                point: GridPoint::new(0, 0),
            },
            cursor_char: ' ',
            terminal_bounds: Default::default(),
            last_hovered_word: None,
            scrolled_to_top: false,
            scrolled_to_bottom: true,
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct HoveredWord {
    pub word: String,
    pub word_match: RangeInclusive<GridPoint>,
    pub id: usize,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, Default)]
pub struct ViewportPoint {
    pub line: usize,
    pub column: usize,
}

/// Convert a backend grid point into viewport-relative coordinates.
#[inline]
pub fn point_to_viewport(display_offset: usize, point: GridPoint) -> Option<ViewportPoint> {
    let line = point.line + display_offset as i32;
    if line < 0 {
        return None;
    }
    Some(ViewportPoint {
        line: line as usize,
        column: point.column,
    })
}
