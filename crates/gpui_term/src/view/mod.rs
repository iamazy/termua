use std::{
    cmp,
    collections::VecDeque,
    ops::{Range, RangeInclusive},
    sync::Arc,
    time::Duration,
};

use gpui::{
    Action, AnyElement, App, Bounds, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement, IntoElement, KeyContext, KeyDownEvent, Keystroke, MouseButton,
    MouseDownEvent, ParentElement, Pixels, PromptLevel, ReadGlobal, Render, ScrollWheelEvent,
    Styled, Subscription, Window, div, px,
};
use gpui_common::TermuaIcon;
use gpui_component::{
    ActiveTheme, Icon, IconName, WindowExt,
    menu::{ContextMenu, PopupMenu, PopupMenuItem},
    notification::Notification,
};
use record::{RecordingMenuEntry, recording_context_menu_entry, recording_indicator_label};
use schemars::JsonSchema;
use scrollbar::{SCROLLBAR_WIDTH, ScrollState, ScrollbarPreview, buffer_index_for_line_coord};
use serde::Deserialize;
use smol::Timer;

use crate::{
    Copy, DecreaseFontSize, GridPoint, HoveredWord, IncreaseFontSize, ResetFontSize,
    TerminalContent, TerminalMode,
    element::{ScrollbarPreviewTextElement, TerminalElement},
    point_to_viewport,
    record::render_recording_indicator_label,
    settings::{CursorShape, TerminalBlink, TerminalSettings},
    snippet::{SnippetJump, SnippetJumpDir, SnippetSession, parse_snippet_suffix},
    suggestions::{
        SelectionMove, SuggestionEngine, SuggestionHistoryConfig, SuggestionItem,
        SuggestionStaticConfig, compute_insert_suffix_for_line, extract_cursor_line_prefix,
        extract_cursor_line_suffix, line_is_suggestion_prefix, move_selection_opt,
    },
    terminal::{
        Clear, Event, Paste, ScrollLineDown, ScrollLineUp, ScrollPageDown, ScrollPageUp,
        ScrollToBottom, ScrollToTop, Search, SearchClose, SearchNext, SearchPaste, SearchPrevious,
        SelectAll, ShowCharacterPalette, StartCastRecording, StopCastRecording, Terminal,
        TerminalBounds, ToggleCastRecording, ToggleViMode, UserInput,
    },
    view::search::{SearchState, render_search},
};

pub(crate) mod line_number;
pub(crate) mod record;
pub(crate) mod scrollbar;
pub(crate) mod search;

fn format_scrollbar_preview_line_number(one_based: usize, digits: usize) -> String {
    let digits = digits.max(1);
    format!("{:>width$}\u{00A0}", one_based, width = digits)
}

pub trait ContextMenuProvider: Send + Sync + 'static {
    fn context_menu(
        &self,
        menu: PopupMenu,
        terminal: Entity<Terminal>,
        terminal_view: Entity<TerminalView>,
        window: &mut Window,
        cx: &mut App,
    ) -> PopupMenu;
}

pub struct ImeState {
    pub marked_text: String,
    marked_range_utf16: Option<Range<usize>>,
}

const CURSOR_BLINK_INTERVAL: Duration = Duration::from_millis(500);

/// Sends the specified text directly to the terminal.
#[derive(Clone, Debug, Default, Deserialize, JsonSchema, PartialEq, Action)]
#[action(namespace = terminal)]
pub struct SendText(String);

/// Sends a keystroke sequence to the terminal.
#[derive(Clone, Debug, Default, Deserialize, JsonSchema, PartialEq, Action)]
#[action(namespace = terminal)]
pub struct SendKeystroke(String);

impl SendKeystroke {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

pub struct BlockProperties {
    pub height: u8,
    pub render: Box<dyn Send + Fn(&mut BlockContext) -> AnyElement>,
}

pub struct BlockContext<'a, 'b> {
    pub window: &'a mut Window,
    pub context: &'b mut App,
    pub dimensions: TerminalBounds,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SearchOverlayKeyDown {
    NotOpen,
    /// Return from the key handler without stopping propagation.
    Return,
    /// Stop event propagation and return from the key handler.
    StopAndReturn,
}

#[derive(Clone, Copy, Debug)]
enum SearchOverlayMove {
    Left,
    Right,
    Home,
    End,
}

#[derive(Clone, Copy, Debug)]
enum SearchOverlayDelete {
    Prev,
    Next,
}

struct BlinkingState {
    state: bool,
    terminal_enabled: bool,
    paused: bool,
    epoch: usize,
}

impl Default for BlinkingState {
    fn default() -> Self {
        Self {
            state: true,
            terminal_enabled: false,
            paused: false,
            epoch: 0,
        }
    }
}

#[derive(Debug)]
struct SuggestionsState {
    open: bool,
    items: Vec<SuggestionItem>,
    selected: Option<usize>,
    hovered: Option<usize>,
    epoch: u64,
    prompt_prefix: Option<String>,
    engine: SuggestionEngine,
    static_epoch_seen: u64,
}

impl SuggestionsState {
    fn new(cx: &App) -> Self {
        let settings = TerminalSettings::global(cx);
        let mut engine = SuggestionEngine::new(200, settings.suggestions_max_items);
        if let Some(provider) = cx
            .try_global::<SuggestionHistoryConfig>()
            .and_then(|cfg| cfg.provider.clone())
        {
            for cmd in provider.seed() {
                let _ = engine.history.push(cmd);
            }
        }

        let (static_provider, static_epoch_seen) = cx
            .try_global::<SuggestionStaticConfig>()
            .map(|cfg| (cfg.provider.clone(), cfg.epoch))
            .unwrap_or((None, 0));
        engine.set_static_provider(static_provider);

        Self {
            open: false,
            items: Vec::new(),
            selected: None,
            hovered: None,
            epoch: 0,
            prompt_prefix: None,
            engine,
            static_epoch_seen,
        }
    }

    fn open_with_items(&mut self, items: Vec<SuggestionItem>) {
        self.items = items;
        self.open = !self.items.is_empty();
        self.selected = None;
        self.hovered = None;
    }

    fn close(&mut self) {
        self.open = false;
        self.items.clear();
        self.selected = None;
        self.hovered = None;
    }
}

#[derive(Clone)]
struct PromptContext {
    content: TerminalContent,
    cursor_line_id: Option<i64>,
}

#[derive(Clone, Copy)]
struct TerminalScrollState {
    is_remote_mirror: bool,
    display_offset: usize,
    line_height: Pixels,
}

#[derive(Clone, Copy)]
struct TerminalModeState {
    vi_mode_enabled: bool,
    mode: TerminalMode,
    has_selection: bool,
}

#[derive(Clone, Copy)]
struct ScrollbarPreviewLayoutState {
    view_bounds: Bounds<Pixels>,
    line_height: Pixels,
    cell_width: Pixels,
    total_lines: usize,
}

#[derive(Clone, Copy)]
struct CommandBlockHitLayoutState {
    bounds: Bounds<Pixels>,
    line_height: Pixels,
    display_offset: i32,
    max_row: i32,
    cols: usize,
}

/// A terminal view, maintains the PTY's file handles and communicates with the terminal
pub struct TerminalView {
    pub terminal: Entity<Terminal>,
    pub focus_handle: FocusHandle,
    // Currently using iTerm bell, show bell emoji in tab until input is received
    has_bell: bool,
    cursor_shape: CursorShape,
    blink: BlinkingState,
    pub hover_word: Option<HoveredWord>,
    scroll: ScrollState,
    pub ime_state: Option<ImeState>,
    search: SearchState,
    suggestions: SuggestionsState,
    snippet: Option<SnippetSession>,
    pending_history_commands: VecDeque<String>,
    last_seen_history_block_id: Option<u64>,
    context_menu_enabled: bool,
    context_menu_provider: Option<Arc<dyn ContextMenuProvider>>,
    _subscriptions: Vec<Subscription>,
    _terminal_subscriptions: Vec<Subscription>,
}

impl EventEmitter<Event> for TerminalView {}

impl Focusable for TerminalView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl TerminalView {
    fn prompt_context(&self, cx: &App) -> Option<PromptContext> {
        let terminal = self.terminal.read(cx);
        Some(PromptContext {
            content: terminal.last_content().clone(),
            cursor_line_id: terminal.cursor_line_id(),
        })
    }

    fn terminal_scroll_state(&self, cx: &App) -> TerminalScrollState {
        let terminal = self.terminal.read(cx);
        let content = terminal.last_content();
        TerminalScrollState {
            is_remote_mirror: terminal.is_remote_mirror(),
            display_offset: content.display_offset,
            line_height: content.terminal_bounds.line_height,
        }
    }

    fn terminal_mode_state(&self, cx: &App) -> TerminalModeState {
        let terminal = self.terminal.read(cx);
        let content = terminal.last_content();
        TerminalModeState {
            vi_mode_enabled: terminal.vi_mode_enabled(),
            mode: content.mode,
            has_selection: content.selection.is_some(),
        }
    }

    fn scrollbar_preview_layout_state(&self, cx: &App) -> ScrollbarPreviewLayoutState {
        let terminal = self.terminal.read(cx);
        let terminal_bounds = &terminal.last_content().terminal_bounds;
        ScrollbarPreviewLayoutState {
            view_bounds: terminal_bounds.bounds,
            line_height: terminal_bounds.line_height,
            cell_width: terminal_bounds.cell_width,
            total_lines: terminal.total_lines(),
        }
    }

    fn command_block_hit_layout_state(&self, cx: &App) -> CommandBlockHitLayoutState {
        let content = self.terminal.read(cx).last_content();
        CommandBlockHitLayoutState {
            bounds: content.terminal_bounds.bounds,
            line_height: content.terminal_bounds.line_height,
            display_offset: content.display_offset as i32,
            max_row: content.terminal_bounds.num_lines().saturating_sub(1) as i32,
            cols: content.terminal_bounds.num_columns().max(1),
        }
    }

    fn cast_recording_active(&self, cx: &App) -> bool {
        self.terminal.read(cx).cast_recording_active()
    }

    fn snippet_prompt_is_eligible(&self, prompt: &PromptContext, cx: &App) -> bool {
        let session_line_id = self.snippet.as_ref().and_then(|s| s.cursor_line_id);
        self.suggestions_eligible_for_content(&prompt.content, cx)
            && (session_line_id.is_none() || session_line_id == prompt.cursor_line_id)
    }
}

impl TerminalView {
    pub fn new(terminal: Entity<Terminal>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::new_with_context_menu_provider(terminal, window, cx, false, None)
    }

    pub fn new_with_context_menu(
        terminal: Entity<Terminal>,
        window: &mut Window,
        cx: &mut Context<Self>,
        context_menu_enabled: bool,
    ) -> Self {
        Self::new_with_context_menu_provider(terminal, window, cx, context_menu_enabled, None)
    }

    pub fn new_with_context_menu_provider(
        terminal: Entity<Terminal>,
        window: &mut Window,
        cx: &mut Context<Self>,
        context_menu_enabled: bool,
        context_menu_provider: Option<Arc<dyn ContextMenuProvider>>,
    ) -> Self {
        let terminal_subscriptions = subscribe_for_terminal_events(&terminal, window, cx);

        let focus_handle = cx.focus_handle();
        let focus_in = cx.on_focus_in(&focus_handle, window, |terminal_view, window, cx| {
            terminal_view.focus_in(window, cx);
        });
        let focus_out = cx.on_focus_out(
            &focus_handle,
            window,
            |terminal_view, _event, window, cx| {
                terminal_view.focus_out(window, cx);
            },
        );
        let cursor_shape = TerminalSettings::global(cx)
            .cursor_shape
            .unwrap_or_default();

        Self {
            terminal,
            has_bell: false,
            focus_handle,
            cursor_shape,
            blink: BlinkingState::default(),
            hover_word: None,
            scroll: ScrollState::default(),
            ime_state: None,
            search: SearchState::default(),
            suggestions: SuggestionsState::new(cx),
            snippet: None,
            pending_history_commands: VecDeque::new(),
            last_seen_history_block_id: None,
            context_menu_enabled,
            context_menu_provider,
            _subscriptions: vec![focus_in, focus_out],
            _terminal_subscriptions: terminal_subscriptions,
        }
    }

    pub fn context_menu_enabled(&self) -> bool {
        self.context_menu_enabled
    }

    fn queue_command_for_history(&mut self, command: String, cx: &mut Context<Self>) {
        let command = command.trim().to_string();
        if command.is_empty() {
            return;
        }

        // Only persist history when we can observe command success via OSC 133 command blocks.
        let blocks = self.terminal.read(cx).command_blocks();
        if blocks.is_none() {
            return;
        }

        if self.last_seen_history_block_id.is_none() {
            self.last_seen_history_block_id = blocks
                .as_ref()
                .and_then(|b| b.iter().rev().find(|v| v.ended_at.is_some()).map(|v| v.id))
                .or(Some(0));
        }

        const MAX_PENDING_HISTORY: usize = 32;
        while self.pending_history_commands.len() >= MAX_PENDING_HISTORY {
            self.pending_history_commands.pop_front();
        }
        self.pending_history_commands.push_back(command);
    }

    fn flush_successful_history_from_blocks(
        &mut self,
        blocks: &[crate::command_blocks::CommandBlock],
        cx: &mut Context<Self>,
    ) {
        let Some(last_seen) = self.last_seen_history_block_id.as_mut() else {
            return;
        };

        let successful = crate::suggestions::drain_successful_history_commands(
            &mut self.pending_history_commands,
            last_seen,
            blocks,
        );
        if successful.is_empty() {
            return;
        }

        let provider = cx
            .try_global::<SuggestionHistoryConfig>()
            .and_then(|cfg| cfg.provider.clone());

        for cmd in successful {
            let inserted = self.suggestions.engine.history.push(cmd.clone());
            if inserted && let Some(provider) = provider.as_ref() {
                provider.append(&cmd);
            }
        }
    }

    fn show_toast(
        &mut self,
        level: PromptLevel,
        title: impl Into<String>,
        detail: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        struct TerminalToastNotification;

        let title = title.into();
        cx.emit(Event::Toast {
            level,
            title: title.clone(),
            detail: detail.clone(),
        });
        let note = match (level, detail) {
            (PromptLevel::Info, Some(detail)) => Notification::info(detail).title(title),
            (PromptLevel::Warning, Some(detail)) => Notification::warning(detail).title(title),
            (PromptLevel::Critical, Some(detail)) => Notification::error(detail).title(title),
            (PromptLevel::Info, None) => Notification::info(title),
            (PromptLevel::Warning, None) => Notification::warning(title),
            (PromptLevel::Critical, None) => Notification::error(title),
        }
        .id::<TerminalToastNotification>();

        window.push_notification(note, cx);
    }

    /// Sets the marked (pre-edit) text from the IME.
    pub(crate) fn set_marked_text(
        &mut self,
        text: String,
        range: Option<Range<usize>>,
        cx: &mut Context<Self>,
    ) {
        self.ime_state = Some(ImeState {
            marked_text: text,
            marked_range_utf16: range,
        });
        cx.notify();
    }

    /// Gets the current marked range (UTF-16).
    pub(crate) fn marked_text_range(&self) -> Option<Range<usize>> {
        self.ime_state
            .as_ref()
            .and_then(|state| state.marked_range_utf16.clone())
    }

    /// Clears the marked (pre-edit) text state.
    pub(crate) fn clear_marked_text(&mut self, cx: &mut Context<Self>) {
        if self.ime_state.is_some() {
            self.ime_state = None;
            cx.notify();
        }
    }

    /// Commits (sends) the given text to the PTY. Called by InputHandler::replace_text_in_range.
    pub(crate) fn commit_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if text.is_empty() {
            return;
        }

        let text = text.to_string();
        let mut deleted_chars = 0usize;

        if self.snippet.is_some() {
            let eligible = self
                .prompt_context(cx)
                .is_some_and(|prompt| self.snippet_prompt_is_eligible(&prompt, cx));

            if !eligible {
                self.snippet = None;
            } else if let Some(session) = self.snippet.as_mut() {
                if session.selected {
                    deleted_chars = session.replace_active_placeholder(&text);
                    session.selected = false;
                } else {
                    let before = session.inserted_len_chars;
                    session.insert_into_active_placeholder(&text);
                    if session.inserted_len_chars == before {
                        // Cursor moved away from the active placeholder; cancel snippet mode to
                        // avoid local placeholder state drifting from the remote line editor.
                        self.snippet = None;
                    }
                }
            }
        }

        self.snap_to_bottom_on_input(cx);
        let emit_text = text.clone();
        let alt_is_meta = TerminalSettings::global(cx).option_as_meta;
        let backspace = Keystroke::parse("backspace").unwrap();
        self.terminal.update(cx, move |term, _| {
            for _ in 0..deleted_chars {
                term.try_keystroke(&backspace, alt_is_meta);
            }
            term.input(text.into_bytes());
        });
        cx.emit(Event::UserInput(UserInput::Text(emit_text)));
    }

    pub(crate) fn terminal_bounds(&self, cx: &App) -> TerminalBounds {
        self.terminal.read(cx).last_content().terminal_bounds
    }

    pub fn entity(&self) -> &Entity<Terminal> {
        &self.terminal
    }

    pub fn has_bell(&self) -> bool {
        self.has_bell
    }

    pub fn clear_bell(&mut self, cx: &mut Context<TerminalView>) {
        self.has_bell = false;
        cx.emit(Event::Wakeup);
    }

    fn show_character_palette(
        &mut self,
        _: &ShowCharacterPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .terminal
            .read(cx)
            .last_content()
            .mode
            .contains(TerminalMode::ALT_SCREEN)
        {
            self.terminal.update(cx, |term, cx| {
                term.try_keystroke(
                    &Keystroke::parse("ctrl-cmd-space").unwrap(),
                    TerminalSettings::global(cx).option_as_meta,
                )
            });
        } else {
            window.show_character_palette();
        }
    }

    fn start_cast_recording(
        &mut self,
        _: &StartCastRecording,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let cfg = crate::cast::CastRecordingConfig::global(cx);
        let include_input = cfg.include_input_by_default;
        let path = crate::cast::default_cast_path(cfg);

        let result = self.terminal.update(cx, |term, _| {
            term.start_cast_recording(crate::CastRecordingOptions {
                path: path.clone(),
                include_input,
            })
        });

        match result {
            Ok(()) => self.show_toast(
                PromptLevel::Info,
                "Recording started",
                Some(path.display().to_string()),
                window,
                cx,
            ),
            Err(err) => self.show_toast(
                PromptLevel::Warning,
                "Recording failed",
                Some(err.to_string()),
                window,
                cx,
            ),
        }
    }

    fn stop_cast_recording(
        &mut self,
        _: &StopCastRecording,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let active = self.cast_recording_active(cx);
        if !active {
            self.show_toast(PromptLevel::Info, "Not recording", None, window, cx);
            return;
        }

        self.terminal.update(cx, |term, _| {
            term.stop_cast_recording();
        });
        self.show_toast(PromptLevel::Info, "Recording stopped", None, window, cx);
    }

    fn toggle_cast_recording(
        &mut self,
        _: &ToggleCastRecording,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let recording_active = self.cast_recording_active(cx);
        if recording_active {
            self.stop_cast_recording(&StopCastRecording, window, cx);
        } else {
            self.start_cast_recording(&StartCastRecording, window, cx);
        }
    }

    fn open_search(&mut self, _: &Search, window: &mut Window, cx: &mut Context<Self>) {
        self.search.search_open = true;
        self.search.search_panel_dragging = false;
        self.search.search_panel_drag_start_mouse = None;
        self.search.search_panel_drag_start_pos = None;
        self.search.search_expected_commit = None;
        self.search.search.end();

        // Default position: near the top, centered within the current window.
        let viewport = window.viewport_size();
        let panel_w = px(520.0)
            .min((viewport.width - px(24.0)).max(Pixels::ZERO))
            .max(px(320.0).min(viewport.width.max(Pixels::ZERO)));
        let keep = px(32.0);
        if !self.search.search_panel_pos_initialized {
            let x = (viewport.width - panel_w).max(Pixels::ZERO) / 2.0;
            self.search.search_panel_pos = gpui::point(x, px(72.0));
            self.search.search_panel_pos_initialized = true;
        } else {
            // If the window size changed since the last open, keep the panel reachable.
            let mut pos = self.search.search_panel_pos;
            pos.x = pos
                .x
                .clamp((Pixels::ZERO - panel_w) + keep, viewport.width - keep);
            pos.y = pos.y.clamp(px(0.0), viewport.height - keep);
            self.search.search_panel_pos = pos;
        }

        window.focus(&self.focus_handle, cx);
        cx.notify();
    }

    fn close_search(&mut self, _: &SearchClose, window: &mut Window, cx: &mut Context<Self>) {
        self.search.search_open = false;
        self.clear_scrollbar_preview(cx);
        self.search.search_epoch = self.search.search_epoch.wrapping_add(1);
        self.search.search.clear();
        self.search.search_ime_state = None;
        self.search.search_expected_commit = None;
        self.search.search_panel_dragging = false;
        self.search.search_panel_drag_start_mouse = None;
        self.search.search_panel_drag_start_pos = None;

        window.focus(&self.focus_handle, cx);

        self.terminal.update(cx, |term, _| {
            term.set_search_query(None);
            term.select_matches(&[]);
        });
        cx.notify();
    }

    fn search_next(&mut self, _: &SearchNext, _window: &mut Window, cx: &mut Context<Self>) {
        self.jump_search(true, cx);
    }

    fn search_previous(
        &mut self,
        _: &SearchPrevious,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.jump_search(false, cx);
    }

    fn jump_search(&mut self, forward: bool, cx: &mut Context<Self>) {
        self.terminal.update(cx, |term, _| {
            let matches_len = term.matches().len();
            if matches_len == 0 {
                return;
            }

            let cur = term.active_match_index().unwrap_or(0) % matches_len;
            let next = if forward {
                (cur + 1) % matches_len
            } else {
                cur.checked_sub(1).unwrap_or(matches_len - 1)
            };
            term.activate_match(next);
            term.jump_to_match(next);
        });
        cx.notify();
    }

    fn search_paste(&mut self, _: &SearchPaste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.search.search.insert(&text);
            self.schedule_search_update(cx);
        }
    }

    pub(crate) fn commit_search_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if text.is_empty() {
            return;
        }
        if self.search.search_expected_commit.as_deref() == Some(text) {
            self.search.search_expected_commit = None;
            return;
        }
        self.search.search_expected_commit = None;
        self.search.search_ime_state = None;
        self.search.search.insert(text);
        self.schedule_search_update(cx);
    }

    pub(crate) fn set_search_marked_text(
        &mut self,
        text: String,
        range: Option<Range<usize>>,
        cx: &mut Context<Self>,
    ) {
        self.search.search_ime_state = Some(ImeState {
            marked_text: text,
            marked_range_utf16: range,
        });
        cx.notify();
    }

    pub(crate) fn clear_search_marked_text(&mut self, cx: &mut Context<Self>) {
        self.search.search_ime_state = None;
        cx.notify();
    }

    pub(crate) fn is_search_open(&self) -> bool {
        self.search.search_open
    }

    pub(crate) fn suggestions_snapshot(&self) -> Option<(Vec<SuggestionItem>, Option<usize>)> {
        self.suggestions.open.then(|| {
            let highlighted = self
                .suggestions
                .hovered
                .or(self.suggestions.selected)
                .and_then(|highlighted| {
                    let last = self.suggestions.items.len().saturating_sub(1);
                    (!self.suggestions.items.is_empty()).then_some(highlighted.min(last))
                });
            (self.suggestions.items.clone(), highlighted)
        })
    }

    pub(crate) fn close_suggestions(&mut self, cx: &mut Context<Self>) {
        if !self.suggestions.open {
            return;
        }
        self.suggestions.close();
        cx.notify();
    }

    pub(crate) fn snippet_snapshot_for_content(
        &self,
        content: &TerminalContent,
        cursor_line_id: Option<i64>,
        cx: &App,
    ) -> Option<SnippetSession> {
        let snippet = self.snippet.clone()?;
        if !self.suggestions_eligible_for_content(content, cx) {
            return None;
        }

        if let Some(expected) = snippet.cursor_line_id
            && cursor_line_id != Some(expected)
        {
            return None;
        }

        Some(snippet)
    }

    pub(crate) fn set_suggestions_hovered(
        &mut self,
        hovered: Option<usize>,
        cx: &mut Context<Self>,
    ) {
        if !self.suggestions.open {
            if self.suggestions.hovered.take().is_some() {
                cx.notify();
            }
            return;
        }

        let hovered = hovered.and_then(|idx| {
            let last = self.suggestions.items.len().saturating_sub(1);
            (!self.suggestions.items.is_empty()).then_some(idx.min(last))
        });

        if self.suggestions.hovered != hovered {
            self.suggestions.hovered = hovered;
            cx.notify();
        }
    }

    pub(crate) fn search_marked_text_range(&self) -> Option<Range<usize>> {
        self.search
            .search_ime_state
            .as_ref()
            .and_then(|state| state.marked_range_utf16.clone())
    }

    pub(crate) fn search_panel_pos(&self) -> gpui::Point<Pixels> {
        self.search.search_panel_pos
    }

    fn suggestions_eligible_for_content(&self, content: &TerminalContent, cx: &App) -> bool {
        TerminalSettings::global(cx).suggestions_enabled
            && content.display_offset == 0
            && !content.mode.contains(TerminalMode::ALT_SCREEN)
            && content.selection.is_none()
            && self.scroll.block_below_cursor.is_none()
    }

    fn schedule_suggestions_update(&mut self, cx: &mut Context<Self>) {
        let epoch = self.suggestions.epoch.wrapping_add(1);
        self.suggestions.epoch = epoch;
        cx.spawn(async move |this, cx| {
            Timer::after(Duration::from_millis(200)).await;
            let _ = this.update(cx, |this, cx| {
                if this.suggestions.epoch != epoch {
                    return;
                }

                let Some(prompt) = this.prompt_context(cx) else {
                    return;
                };
                if !this.suggestions_eligible_for_content(&prompt.content, cx) {
                    this.suggestions.prompt_prefix = None;
                    this.suggestions.close();
                    cx.notify();
                    return;
                }

                let Some(prompt_prefix) = this.suggestions.prompt_prefix.clone() else {
                    this.suggestions.close();
                    cx.notify();
                    return;
                };

                let line_prefix = extract_cursor_line_prefix(&prompt.content);
                let input_prefix = line_prefix.strip_prefix(&prompt_prefix).unwrap_or("");

                this.suggestions.engine.max_items =
                    TerminalSettings::global(cx).suggestions_max_items;

                if let Some(cfg) = cx.try_global::<SuggestionStaticConfig>()
                    && cfg.epoch != this.suggestions.static_epoch_seen
                {
                    this.suggestions.static_epoch_seen = cfg.epoch;
                    this.suggestions
                        .engine
                        .set_static_provider(cfg.provider.clone());
                }

                let items = this.suggestions.engine.suggest(input_prefix);
                this.suggestions.open_with_items(items);
                cx.notify();
            });
        })
        .detach();
    }

    fn accept_selected_suggestion(
        &mut self,
        content: &TerminalContent,
        cursor_line_id: Option<i64>,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.suggestions.open {
            return false;
        }

        let Some(selected) = self.suggestions.selected else {
            return false;
        };
        self.accept_suggestion_at_index(selected, content, cursor_line_id, cx)
    }

    pub(crate) fn accept_suggestion_at_index(
        &mut self,
        index: usize,
        content: &TerminalContent,
        cursor_line_id: Option<i64>,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.suggestions.open {
            return false;
        }

        let Some(item) = self.suggestions.items.get(index).cloned() else {
            return false;
        };

        let line_prefix = extract_cursor_line_prefix(content);
        let Some((input_prefix, suffix_template)) = compute_insert_suffix_for_line(
            &line_prefix,
            self.suggestions.prompt_prefix.as_deref(),
            &item.full_text,
        ) else {
            return false;
        };

        let line_suffix = extract_cursor_line_suffix(content);
        if !line_suffix.trim().is_empty() {
            let combined_line = format!("{input_prefix}{line_suffix}");
            if line_is_suggestion_prefix(&combined_line, &item.full_text) {
                return false;
            }
        }

        let mut suffix_rendered = suffix_template.clone();
        let mut snippet_session: Option<SnippetSession> = None;
        let mut initial_move_left = 0usize;

        if let Some(snippet) = parse_snippet_suffix(&suffix_template) {
            suffix_rendered = snippet.rendered;

            // `$0` is a cursor position, not an editable placeholder. Avoid entering snippet mode
            // if there are no non-zero tabstops.
            if snippet.tabstops.iter().any(|t| t.index != 0) {
                let mut session = SnippetSession::new(suffix_rendered.clone(), snippet.tabstops);
                session.cursor_line_id = cursor_line_id;
                session.start_point = content.cursor.point;
                session.active = 0;

                let target_end = session
                    .tabstops
                    .first()
                    .map(|t| t.range_chars.end)
                    .unwrap_or(session.inserted_len_chars);

                initial_move_left = session.inserted_len_chars.saturating_sub(target_end);
                session.cursor_offset_chars = target_end;
                session.selected = true;
                snippet_session = Some(session);
            }
        }

        self.snap_to_bottom_on_input(cx);
        let alt_is_meta = TerminalSettings::global(cx).option_as_meta;
        let left = Keystroke::parse("left").unwrap();

        let suffix = suffix_rendered.into_bytes();
        self.terminal.update(cx, move |term, _| {
            term.input(suffix);
            for _ in 0..initial_move_left {
                term.try_keystroke(&left, alt_is_meta);
            }
        });
        self.snippet = snippet_session;
        self.suggestions.close();
        true
    }

    fn schedule_search_update(&mut self, cx: &mut Context<Self>) {
        let epoch = self.search.search_epoch.wrapping_add(1);
        self.search.search_epoch = epoch;
        cx.spawn(async move |this, cx| {
            Timer::after(Duration::from_millis(150)).await;
            let _ = this.update(cx, |this, cx| {
                if !this.search.search_open || this.search.search_epoch != epoch {
                    return;
                }

                // Only treat all-whitespace as empty; otherwise keep the query exactly as typed.
                let q = this.search.search.text().to_string();
                this.terminal.update(cx, |term, _| {
                    if q.chars().all(|c| c.is_whitespace()) {
                        term.set_search_query(None);
                    } else {
                        term.set_search_query(Some(q));
                        if !term.matches().is_empty() {
                            term.activate_match(0);
                        }
                    }
                });
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn search_cursor_utf16(&self) -> usize {
        self.search.search.cursor_utf16()
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.terminal.update(cx, |term, _| term.select_all());
        cx.notify();
    }

    fn clear(&mut self, _: &Clear, _: &mut Window, cx: &mut Context<Self>) {
        self.terminal.update(cx, |term, _| term.clear());
        cx.notify();
    }

    fn reset_font_size(&mut self, _: &ResetFontSize, _: &mut Window, cx: &mut Context<Self>) {
        self.terminal.update(cx, |_, cx| {
            cx.global_mut::<TerminalSettings>().font_size = px(15.);
        });
        cx.notify();
    }

    fn increase_font_size(&mut self, _: &IncreaseFontSize, _: &mut Window, cx: &mut Context<Self>) {
        self.terminal.update(cx, |_, cx| {
            let font_size = cx.global::<TerminalSettings>().font_size;
            if font_size >= px(100.) {
                return;
            }
            cx.global_mut::<TerminalSettings>().font_size += px(1.);
        });
        cx.notify();
    }

    fn decrease_font_size(&mut self, _: &DecreaseFontSize, _: &mut Window, cx: &mut Context<Self>) {
        self.terminal.update(cx, |_, cx| {
            let font_size = cx.global::<TerminalSettings>().font_size;
            if font_size <= px(5.) {
                return;
            }
            cx.global_mut::<TerminalSettings>().font_size -= px(1.);
        });
        cx.notify();
    }

    fn max_scroll_top(&self, cx: &App) -> Pixels {
        let terminal = self.terminal.read(cx);

        let Some(block) = self.scroll.block_below_cursor.as_ref() else {
            return Pixels::ZERO;
        };

        let content = terminal.last_content();
        let line_height = content.terminal_bounds.line_height;
        let viewport_lines = terminal.viewport_lines();
        let cursor =
            point_to_viewport(content.display_offset, content.cursor.point).unwrap_or_default();
        let max_scroll_top_in_lines =
            (block.height as usize).saturating_sub(viewport_lines.saturating_sub(cursor.line + 1));

        max_scroll_top_in_lines as f32 * line_height
    }

    /// Zed-like behavior: if the user is looking at history (terminal scrollback) or has scrolled
    /// away from the live block content, any input that is forwarded to the PTY should snap the
    /// view back to the bottom.
    fn snap_to_bottom_on_input(&mut self, cx: &mut Context<Self>) {
        let TerminalScrollState {
            display_offset,
            line_height,
            ..
        } = self.terminal_scroll_state(cx);

        // History scrolling: ensure we immediately return to the live viewport on any PTY input.
        // (Some input paths, like `try_keystroke`, don't go through backend `input()`.)
        if display_offset != 0 {
            self.terminal.update(cx, |term, _| term.scroll_to_bottom());
            self.scroll.scroll_top = Pixels::ZERO;
            self.scroll.stick_to_bottom = true;
            cx.notify();
            return;
        }

        // Block-below-cursor extra scroll space (e.g. prompt block). Only applies in live view.
        if self.scroll.block_below_cursor.is_some() {
            let max = self.max_scroll_top(cx);
            let at_bottom = self.scroll.scroll_top + line_height / 2.0 >= max;

            if !at_bottom {
                self.scroll.scroll_top = max;
            }
        }

        // Typing implies "follow output" unless the user scrolls away again.
        self.scroll.stick_to_bottom = true;
        cx.notify();
    }

    pub(crate) fn set_mouse_left_down_in_terminal(&mut self, down: bool) {
        self.scroll.mouse_left_down_in_terminal = down;
    }

    pub(crate) fn mouse_left_down_in_terminal(&self) -> bool {
        self.scroll.mouse_left_down_in_terminal
    }

    pub(crate) fn scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Scroll-wheel usage should reveal the overlay scrollbar briefly, even if the pointer
        // isn't within the scrollbar lane.
        //
        // Note: This is kept view-local so we can avoid allocating dedicated layout space.
        // The element decides whether to paint the scrollbar based on this flag.
        // (Timer-based auto-hide happens in `reveal_scrollbar_for_scroll`.)
        //
        // We don't require focus checks here; callers already gate on focus.
        self.reveal_scrollbar_for_scroll(window, cx);

        let TerminalScrollState {
            is_remote_mirror,
            display_offset,
            line_height,
        } = self.terminal_scroll_state(cx);

        if is_remote_mirror {
            self.terminal.update(cx, |term, _| term.scroll_wheel(event));
            return;
        }

        if self.scroll.block_below_cursor.is_some() && display_offset == 0 {
            let y_delta = event.delta.pixel_delta(line_height).y;
            if y_delta < Pixels::ZERO || self.scroll.scroll_top > Pixels::ZERO {
                let max = self.max_scroll_top(cx);
                self.scroll.scroll_top = cmp::max(
                    Pixels::ZERO,
                    cmp::min(self.scroll.scroll_top - y_delta, max),
                );
                self.scroll.stick_to_bottom = self.scroll.scroll_top + line_height / 2.0 >= max;
                cx.notify();
                return;
            }
        }
        // Scrolling the terminal history should never keep a block-scroll offset.
        self.scroll.scroll_top = Pixels::ZERO;
        self.scroll.stick_to_bottom = false;
        self.terminal.update(cx, |term, _| term.scroll_wheel(event));
    }

    pub(crate) fn scrollbar_dragging(&self) -> bool {
        self.scroll.scrollbar_dragging
    }

    pub(crate) fn scrollbar_hovered(&self) -> bool {
        self.scroll.scrollbar_hovered
    }

    pub(crate) fn scrollbar_revealed(&self) -> bool {
        self.scroll.scrollbar_revealed
    }

    pub(crate) fn set_scrollbar_hovered(&mut self, hovered: bool, cx: &mut Context<Self>) {
        if self.scroll.scrollbar_hovered != hovered {
            self.scroll.scrollbar_hovered = hovered;
            cx.notify();
        }
    }

    fn reveal_scrollbar_for_scroll(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if !TerminalSettings::global(cx).show_scrollbar {
            return;
        }
        self.scroll.scrollbar_revealed = true;
        self.scroll.scrollbar_reveal_epoch = self.scroll.scrollbar_reveal_epoch.wrapping_add(1);
        let epoch = self.scroll.scrollbar_reveal_epoch;
        cx.notify();

        // Auto-hide after a short delay. If the user is hovering/dragging at that time, the
        // scrollbar stays visible due to those signals (not this temporary reveal flag).
        cx.spawn(async move |this, cx| {
            Timer::after(Duration::from_millis(900)).await;
            let _ = this.update(cx, |this, cx| {
                if this.scroll.scrollbar_reveal_epoch != epoch {
                    return;
                }
                if this.scroll.scrollbar_revealed {
                    this.scroll.scrollbar_revealed = false;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(crate) fn scroll_top(&self) -> Pixels {
        self.scroll.scroll_top
    }

    pub(crate) fn scrollbar_virtual_offset(&self) -> Option<usize> {
        self.scroll.scrollbar_virtual_offset
    }

    pub(crate) fn scrollbar_drag_origin(&self) -> Option<(Pixels, usize)> {
        Some((
            self.scroll.scrollbar_drag_start_y?,
            self.scroll.scrollbar_drag_start_offset?,
        ))
    }

    pub(crate) fn set_scrollbar_drag_origin(&mut self, mouse_y: Pixels, offset: usize) {
        self.scroll.scrollbar_drag_start_y = Some(mouse_y);
        self.scroll.scrollbar_drag_start_offset = Some(offset);
    }

    pub(crate) fn begin_scrollbar_drag(&mut self, mouse_y: Pixels, cx: &mut Context<Self>) {
        self.scroll.scrollbar_dragging = true;
        let current = {
            let terminal = self.terminal.read(cx);
            terminal.last_content().display_offset
        };
        self.scroll.scrollbar_virtual_offset = Some(current);
        self.scroll.scrollbar_last_target_offset = Some(current);
        self.set_scrollbar_drag_origin(mouse_y, current);
    }

    pub(crate) fn end_scrollbar_drag(&mut self) {
        self.scroll.scrollbar_dragging = false;
        self.scroll.scrollbar_last_target_offset = None;
        self.scroll.scrollbar_virtual_offset = None;
        self.scroll.scrollbar_drag_start_y = None;
        self.scroll.scrollbar_drag_start_offset = None;
    }

    pub(crate) fn scrollbar_preview(&self) -> Option<&ScrollbarPreview> {
        self.scroll.scrollbar_preview.as_ref()
    }

    pub(crate) fn clear_scrollbar_preview(&mut self, cx: &mut Context<Self>) {
        if self.scroll.scrollbar_preview.take().is_some() {
            cx.notify();
        }
    }

    pub(crate) fn set_scrollbar_preview_for_match(
        &mut self,
        match_index: usize,
        anchor: gpui::Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let Some((start, cols, rows, cells, match_range)) = (|| {
            let terminal = self.terminal.read(cx);
            let matches = terminal.matches();
            if match_index >= matches.len() {
                return None;
            }

            let total_lines = terminal.total_lines();
            let viewport_lines = terminal.viewport_lines();
            let match_range = matches[match_index].clone();

            let start_line_coord = match_range.start().line;
            let end_line_coord = match_range.end().line;
            let start_line_from_top =
                buffer_index_for_line_coord(total_lines, viewport_lines, start_line_coord);
            let end_line_from_top =
                buffer_index_for_line_coord(total_lines, viewport_lines, end_line_coord);

            let context_above = 3usize;
            let total = 7usize;
            let start = start_line_from_top.saturating_sub(context_above);

            let (cols, rows, cells) = terminal.preview_cells_from_top(start, total);
            if rows == 0 || cells.is_empty() {
                return None;
            }

            // Convert the match range into preview-local coordinates (preview starts at line 0).
            let local_start_line = start_line_from_top.saturating_sub(start);
            let local_end_line = end_line_from_top.saturating_sub(start);
            let local_range = RangeInclusive::new(
                GridPoint::new(local_start_line as i32, match_range.start().column),
                GridPoint::new(local_end_line as i32, match_range.end().column),
            );

            Some((start, cols, rows, cells, local_range))
        })() else {
            self.clear_scrollbar_preview(cx);
            return;
        };

        // Avoid re-fetching preview text while the pointer moves within the same marker.
        if let Some(prev) = self.scroll.scrollbar_preview.as_mut()
            && prev.match_index == match_index
            && prev.start_line_from_top == start
        {
            prev.anchor = anchor;
            cx.notify();
            return;
        }

        self.scroll.scrollbar_preview = Some(ScrollbarPreview {
            match_index,
            anchor,
            start_line_from_top: start,
            cols,
            rows,
            cells,
            match_range,
        });
        cx.notify();
    }

    pub(crate) fn apply_scrollbar_target_offset(
        &mut self,
        target_offset: usize,
        cx: &mut Context<Self>,
    ) {
        if self.scroll.scrollbar_last_target_offset == Some(target_offset) {
            return;
        }
        self.scroll.scrollbar_last_target_offset = Some(target_offset);
        let current = self.scroll.scrollbar_virtual_offset.unwrap_or_else(|| {
            let terminal = self.terminal.read(cx);
            terminal.last_content().display_offset
        });
        self.scroll_to_display_offset_from_current(current, target_offset, cx);
        self.scroll.scrollbar_virtual_offset = Some(target_offset);
    }

    fn scroll_to_display_offset_from_current(
        &mut self,
        current_offset: usize,
        target_offset: usize,
        cx: &mut Context<Self>,
    ) {
        let (is_remote_mirror, max_offset) = {
            let terminal = self.terminal.read(cx);
            let total_lines = terminal.total_lines();
            let viewport_lines = terminal.viewport_lines();
            (
                terminal.is_remote_mirror(),
                total_lines.saturating_sub(viewport_lines),
            )
        };

        let current_offset = current_offset.min(max_offset);
        let target_offset = target_offset.min(max_offset);
        if target_offset == current_offset {
            if !is_remote_mirror {
                self.scroll.scroll_top = Pixels::ZERO;
                self.scroll.stick_to_bottom = target_offset == 0;
            }
            return;
        }

        if !is_remote_mirror {
            // Scrolling the terminal history should never keep the extra "block below cursor"
            // scroll.
            self.scroll.scroll_top = Pixels::ZERO;
        }

        self.terminal.update(cx, |term, _| {
            if target_offset == 0 {
                term.scroll_to_bottom();
            } else if target_offset == max_offset {
                term.scroll_to_top();
            } else if target_offset > current_offset {
                term.scroll_up_by(target_offset - current_offset);
            } else {
                term.scroll_down_by(current_offset - target_offset);
            }
        });

        if !is_remote_mirror {
            self.scroll.stick_to_bottom = target_offset == 0;
            cx.notify();
        }
    }

    // `scroll_to_display_offset_from_current` is the only implementation we need right now.

    fn scroll_line_up(&mut self, _: &ScrollLineUp, window: &mut Window, cx: &mut Context<Self>) {
        self.reveal_scrollbar_for_scroll(window, cx);
        let TerminalScrollState {
            is_remote_mirror,
            display_offset,
            line_height,
        } = self.terminal_scroll_state(cx);
        if is_remote_mirror {
            self.terminal.update(cx, |term, _| term.scroll_line_up());
            return;
        }
        if self.scroll.block_below_cursor.is_some()
            && display_offset == 0
            && self.scroll.scroll_top > Pixels::ZERO
        {
            self.scroll.scroll_top = cmp::max(self.scroll.scroll_top - line_height, Pixels::ZERO);
            let max = self.max_scroll_top(cx);
            self.scroll.stick_to_bottom = self.scroll.scroll_top + line_height / 2.0 >= max;
            return;
        }

        self.terminal.update(cx, |term, _| term.scroll_line_up());
        // Terminal scrollback and block scrolling are mutually exclusive.
        self.scroll.scroll_top = Pixels::ZERO;
        self.scroll.stick_to_bottom = false;
        cx.notify();
    }

    fn scroll_line_down(
        &mut self,
        _: &ScrollLineDown,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reveal_scrollbar_for_scroll(window, cx);
        let TerminalScrollState {
            is_remote_mirror,
            display_offset,
            line_height,
        } = self.terminal_scroll_state(cx);
        if is_remote_mirror {
            self.terminal.update(cx, |term, _| term.scroll_line_down());
            return;
        }
        if self.scroll.block_below_cursor.is_some() && display_offset == 0 {
            let max_scroll_top = self.max_scroll_top(cx);
            if self.scroll.scroll_top < max_scroll_top {
                self.scroll.scroll_top =
                    cmp::min(self.scroll.scroll_top + line_height, max_scroll_top);
            }
            self.scroll.stick_to_bottom =
                self.scroll.scroll_top + line_height / 2.0 >= max_scroll_top;
            return;
        }

        self.terminal.update(cx, |term, _| term.scroll_line_down());
        // Terminal scrollback and block scrolling are mutually exclusive.
        self.scroll.scroll_top = Pixels::ZERO;
        self.scroll.stick_to_bottom = false;
        cx.notify();
    }

    fn scroll_page_up(&mut self, _: &ScrollPageUp, window: &mut Window, cx: &mut Context<Self>) {
        self.reveal_scrollbar_for_scroll(window, cx);
        let (is_remote_mirror, line_height, viewport_lines) = {
            let terminal = self.terminal.read(cx);
            (
                terminal.is_remote_mirror(),
                terminal.last_content().terminal_bounds.line_height(),
                terminal.viewport_lines(),
            )
        };
        if is_remote_mirror {
            self.terminal.update(cx, |term, _| term.scroll_page_up());
            return;
        }
        if self.scroll.scroll_top == Pixels::ZERO {
            self.terminal.update(cx, |term, _| term.scroll_page_up());
            self.scroll.scroll_top = Pixels::ZERO;
            self.scroll.stick_to_bottom = false;
        } else {
            let visible_block_lines = (self.scroll.scroll_top / line_height) as usize;
            let visible_content_lines = viewport_lines - visible_block_lines;

            if visible_block_lines >= viewport_lines {
                self.scroll.scroll_top =
                    ((visible_block_lines - viewport_lines) as f32) * line_height;
            } else {
                self.scroll.scroll_top = px(0.);
                self.terminal
                    .update(cx, |term, _| term.scroll_up_by(visible_content_lines));
            }
            self.scroll.stick_to_bottom =
                self.scroll.scroll_top + line_height / 2.0 >= self.max_scroll_top(cx);
        }
        cx.notify();
    }

    fn scroll_page_down(
        &mut self,
        _: &ScrollPageDown,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reveal_scrollbar_for_scroll(window, cx);
        let TerminalScrollState {
            is_remote_mirror,
            display_offset,
            ..
        } = self.terminal_scroll_state(cx);
        if is_remote_mirror {
            self.terminal.update(cx, |term, _| term.scroll_page_down());
            return;
        }
        self.terminal.update(cx, |term, _| term.scroll_page_down());
        // Scrolling the terminal history should not apply block scrolling offsets.
        // `scroll_top` is only meaningful while we're at the live view.
        if self.scroll.block_below_cursor.is_some() && display_offset == 0 {
            self.scroll.scroll_top = self.max_scroll_top(cx);
            self.scroll.stick_to_bottom = true;
        } else {
            self.scroll.scroll_top = Pixels::ZERO;
            self.scroll.stick_to_bottom = false;
        }
        cx.notify();
    }

    fn scroll_to_top(&mut self, _: &ScrollToTop, window: &mut Window, cx: &mut Context<Self>) {
        self.reveal_scrollbar_for_scroll(window, cx);
        let is_remote_mirror = self.terminal_scroll_state(cx).is_remote_mirror;
        if is_remote_mirror {
            self.terminal.update(cx, |term, _| term.scroll_to_top());
            return;
        }
        self.terminal.update(cx, |term, _| term.scroll_to_top());
        self.scroll.scroll_top = Pixels::ZERO;
        self.scroll.stick_to_bottom = false;
        cx.notify();
    }

    fn scroll_to_bottom(
        &mut self,
        _: &ScrollToBottom,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reveal_scrollbar_for_scroll(window, cx);
        let is_remote_mirror = self.terminal_scroll_state(cx).is_remote_mirror;
        if is_remote_mirror {
            self.terminal.update(cx, |term, _| term.scroll_to_bottom());
            return;
        }
        self.terminal.update(cx, |term, _| term.scroll_to_bottom());
        if self.scroll.block_below_cursor.is_some() {
            self.scroll.scroll_top = self.max_scroll_top(cx);
        } else {
            self.scroll.scroll_top = Pixels::ZERO;
        }
        self.scroll.stick_to_bottom = true;
        cx.notify();
    }

    fn toggle_vi_mode(&mut self, _: &ToggleViMode, _: &mut Window, cx: &mut Context<Self>) {
        self.terminal.update(cx, |term, _| term.toggle_vi_mode());
        cx.notify();
    }

    pub fn should_show_cursor(&self, focused: bool, cx: &mut Context<Self>) -> bool {
        // Don't blink the cursor when not focused, blinking is disabled, or paused
        if !focused
            || self.blink.paused
            || self
                .terminal
                .read(cx)
                .last_content()
                .mode
                .contains(TerminalMode::ALT_SCREEN)
        {
            return true;
        }

        match TerminalSettings::global(cx).blinking {
            // If the user requested to never blink, don't blink it.
            TerminalBlink::Off => true,
            // If the terminal is controlling it, check terminal mode
            TerminalBlink::TerminalControlled => !self.blink.terminal_enabled || self.blink.state,
            TerminalBlink::On => self.blink.state,
        }
    }

    fn blink_cursors(&mut self, epoch: usize, cx: &mut Context<Self>) {
        if epoch == self.blink.epoch && !self.blink.paused {
            self.blink.state = !self.blink.state;
            cx.notify();

            let epoch = self.next_blink_epoch();
            cx.spawn(async move |this, cx| {
                Timer::after(CURSOR_BLINK_INTERVAL).await;
                let _ = this.update(cx, |this, cx| this.blink_cursors(epoch, cx));
            })
            .detach();
        }
    }

    pub fn pause_cursor_blinking(&mut self, cx: &mut Context<Self>) {
        self.blink.paused = true;
        self.blink.state = true;
        cx.notify();

        let epoch = self.next_blink_epoch();
        cx.spawn(async move |this, cx| {
            Timer::after(CURSOR_BLINK_INTERVAL).await;
            let _ = this.update(cx, |this, cx| this.resume_cursor_blinking(epoch, cx));
        })
        .detach();
    }

    pub fn terminal(&self) -> &Entity<Terminal> {
        &self.terminal
    }

    pub fn clear_block_below_cursor(&mut self, cx: &mut Context<Self>) {
        self.scroll.block_below_cursor = None;
        cx.notify();
    }

    fn next_blink_epoch(&mut self) -> usize {
        self.blink.epoch = self.blink.epoch.wrapping_add(1);
        self.blink.epoch
    }

    fn resume_cursor_blinking(&mut self, epoch: usize, cx: &mut Context<Self>) {
        if epoch == self.blink.epoch {
            self.blink.paused = false;
            self.blink_cursors(epoch, cx);
        }
    }

    /// Attempt to paste the clipboard into the terminal
    fn copy(&mut self, _: &crate::terminal::Copy, _: &mut Window, cx: &mut Context<Self>) {
        self.terminal.update(cx, |term, cx| term.copy(None, cx));
        cx.notify();
    }

    /// Attempt to paste the clipboard into the terminal
    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(clipboard_string) = cx.read_from_clipboard().and_then(|item| item.text()) {
            // Treat an empty clipboard as a no-op. This avoids surprising behavior and (on some
            // platforms) helps prevent "paste" from leaving the terminal without focus.
            if !clipboard_string.is_empty() {
                self.snap_to_bottom_on_input(cx);
                let emit_text = clipboard_string.clone();

                let mut deleted_chars = 0usize;
                if self.snippet.is_some() {
                    let eligible = self
                        .prompt_context(cx)
                        .is_some_and(|prompt| self.snippet_prompt_is_eligible(&prompt, cx));

                    if !eligible {
                        self.snippet = None;
                    } else if let Some(session) = self.snippet.as_mut() {
                        if session.selected {
                            deleted_chars = session.replace_active_placeholder(&clipboard_string);
                            session.selected = false;
                        } else {
                            session.insert_into_active_placeholder(&clipboard_string);
                        }
                    }
                }

                let alt_is_meta = TerminalSettings::global(cx).option_as_meta;
                let backspace = Keystroke::parse("backspace").unwrap();
                self.terminal.update(cx, move |terminal, _cx| {
                    for _ in 0..deleted_chars {
                        terminal.try_keystroke(&backspace, alt_is_meta);
                    }
                    terminal.paste(&clipboard_string);
                });
                cx.emit(Event::UserInput(UserInput::Paste(emit_text)));
            }
        }

        // If paste was invoked from a context menu, the menu may steal focus. Always restore
        // focus to the terminal view so the user can keep typing.
        window.focus(&self.focus_handle, cx);
    }

    fn send_text(&mut self, text: &SendText, _: &mut Window, cx: &mut Context<Self>) {
        self.clear_bell(cx);
        self.snap_to_bottom_on_input(cx);
        self.terminal.update(cx, |term, _| {
            term.input(text.0.to_string().into_bytes());
        });
    }

    fn send_keystroke(&mut self, text: &SendKeystroke, _: &mut Window, cx: &mut Context<Self>) {
        if let Ok(keystroke) = Keystroke::parse(&text.0) {
            self.clear_bell(cx);

            if keystroke.key == "tab" && self.snippet.is_some() {
                let eligible = self
                    .prompt_context(cx)
                    .is_some_and(|prompt| self.snippet_prompt_is_eligible(&prompt, cx));

                if !eligible {
                    self.snippet = None;
                } else if let Some(session) = self.snippet.as_mut() {
                    let dir = if keystroke.modifiers.shift {
                        SnippetJumpDir::Prev
                    } else {
                        SnippetJumpDir::Next
                    };

                    let jump = session.jump(dir);
                    let (delta, exit) = match jump {
                        SnippetJump::Noop => (0isize, false),
                        SnippetJump::Move(delta) => (delta, false),
                        SnippetJump::Exit(delta) => (delta, true),
                    };

                    let alt_is_meta = TerminalSettings::global(cx).option_as_meta;
                    let left = Keystroke::parse("left").unwrap();
                    let right = Keystroke::parse("right").unwrap();
                    self.terminal.update(cx, move |term, _| {
                        if delta < 0 {
                            for _ in 0..((-delta) as usize) {
                                term.try_keystroke(&left, alt_is_meta);
                            }
                        } else {
                            for _ in 0..(delta as usize) {
                                term.try_keystroke(&right, alt_is_meta);
                            }
                        }
                    });

                    if exit {
                        self.snippet = None;
                    }
                    cx.notify();
                    return;
                }
            }

            if keystroke.key == "tab"
                && TerminalSettings::global(cx).suggestions_enabled
                && self.suggestions.open
                && let Some(prompt) = self.prompt_context(cx)
                && self.suggestions_eligible_for_content(&prompt.content, cx)
                && self.accept_selected_suggestion(&prompt.content, prompt.cursor_line_id, cx)
            {
                return;
            }

            let (processed, vi_mode_enabled) = self.terminal.update(cx, |term, cx| {
                let processed =
                    term.try_keystroke(&keystroke, TerminalSettings::global(cx).option_as_meta);
                let vi_mode_enabled = term.vi_mode_enabled();
                if processed && vi_mode_enabled {
                    cx.notify();
                }
                (processed, vi_mode_enabled)
            });

            if processed {
                // Avoid yanking the viewport while the user is actively navigating in terminal
                // vi-mode. If the keystroke exits vi-mode, `vi_mode_enabled()` will be false here
                // and we will snap as expected.
                if !vi_mode_enabled {
                    self.snap_to_bottom_on_input(cx);
                }
            }
        }
    }

    fn dispatch_context(&self, cx: &App) -> KeyContext {
        let mut dispatch_context = KeyContext::new_with_defaults();
        dispatch_context.add("Terminal");

        if self.search.search_open {
            dispatch_context.add("search");
        }

        let TerminalModeState {
            vi_mode_enabled,
            mode,
            has_selection,
        } = self.terminal_mode_state(cx);

        if vi_mode_enabled {
            dispatch_context.add("vi_mode");
        }

        dispatch_context.set(
            "screen",
            if mode.contains(TerminalMode::ALT_SCREEN) {
                "alt"
            } else {
                "normal"
            },
        );

        if mode.contains(TerminalMode::APP_CURSOR) {
            dispatch_context.add("DECCKM");
        }
        if mode.contains(TerminalMode::APP_KEYPAD) {
            dispatch_context.add("DECPAM");
        } else {
            dispatch_context.add("DECPNM");
        }
        if mode.contains(TerminalMode::SHOW_CURSOR) {
            dispatch_context.add("DECTCEM");
        }
        if mode.contains(TerminalMode::LINE_WRAP) {
            dispatch_context.add("DECAWM");
        }
        if mode.contains(TerminalMode::ORIGIN) {
            dispatch_context.add("DECOM");
        }
        if mode.contains(TerminalMode::INSERT) {
            dispatch_context.add("IRM");
        }
        // LNM is apparently the name for this. https://vt100.net/docs/vt510-rm/LNM.html
        if mode.contains(TerminalMode::LINE_FEED_NEW_LINE) {
            dispatch_context.add("LNM");
        }
        if mode.contains(TerminalMode::FOCUS_IN_OUT) {
            dispatch_context.add("report_focus");
        }
        if mode.contains(TerminalMode::ALTERNATE_SCROLL) {
            dispatch_context.add("alternate_scroll");
        }
        if mode.contains(TerminalMode::BRACKETED_PASTE) {
            dispatch_context.add("bracketed_paste");
        }
        if mode.intersects(TerminalMode::MOUSE_MODE) {
            dispatch_context.add("any_mouse_reporting");
        }
        {
            let mouse_reporting = if mode.contains(TerminalMode::MOUSE_REPORT_CLICK) {
                "click"
            } else if mode.contains(TerminalMode::MOUSE_DRAG) {
                "drag"
            } else if mode.contains(TerminalMode::MOUSE_MOTION) {
                "motion"
            } else {
                "off"
            };
            dispatch_context.set("mouse_reporting", mouse_reporting);
        }
        {
            let format = if mode.contains(TerminalMode::SGR_MOUSE) {
                "sgr"
            } else if mode.contains(TerminalMode::UTF8_MOUSE) {
                "utf8"
            } else {
                "normal"
            };
            dispatch_context.set("mouse_format", format);
        };

        if has_selection {
            dispatch_context.add("selection");
        }

        dispatch_context
    }
}

impl TerminalView {
    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.clear_bell(cx);
        self.pause_cursor_blinking(cx);

        if self.handle_search_overlay_key_down_for_terminal_key_down(event, window, cx) {
            return;
        }

        if self.handle_snippet_key_down(event, cx) {
            return;
        }

        if self.handle_suggestions_key_down(event, cx) {
            return;
        }

        self.forward_keystroke_to_terminal(event, cx);
    }

    fn handle_search_overlay_key_down_for_terminal_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        match self.handle_search_overlay_key_down(event, window, cx) {
            SearchOverlayKeyDown::NotOpen => false,
            SearchOverlayKeyDown::Return => true,
            SearchOverlayKeyDown::StopAndReturn => {
                // Search is a overlay; don't forward keystrokes to the terminal.
                cx.stop_propagation();
                true
            }
        }
    }

    fn handle_snippet_key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        if self.snippet.is_none() {
            return false;
        }

        let eligible = self
            .prompt_context(cx)
            .is_some_and(|prompt| self.snippet_prompt_is_eligible(&prompt, cx));

        if !eligible {
            self.snippet = None;
            return false;
        }

        // Any newline ends the snippet session.
        if event.keystroke.key.as_str() == "enter" {
            self.snippet = None;
            return false;
        }

        let is_plain_text = event.keystroke.key_char.as_ref().is_some_and(|ch| {
            !ch.is_empty()
                && !event.keystroke.is_ime_in_progress()
                && !event.keystroke.modifiers.control
                && !event.keystroke.modifiers.platform
                && !event.keystroke.modifiers.function
                && !event.keystroke.modifiers.alt
        });
        if is_plain_text
            && !matches!(event.keystroke.key.as_str(), "tab" | "escape")
            && let Some(ch) = event.keystroke.key_char.as_deref()
            && !ch.is_empty()
        {
            // Snippet placeholder "selection" is local UI state; terminal line editors do not
            // support replacing highlighted ranges. Treat character input as an explicit
            // commit so we can delete/replace the active placeholder and keep placeholder
            // highlight ranges in sync.
            self.commit_text(ch, cx);
            cx.notify();
            cx.stop_propagation();
            return true;
        }

        let Some(session) = self.snippet.as_mut() else {
            return false;
        };

        if event.keystroke.key.as_str() == "backspace" {
            if session.selected {
                let deleted_chars = session.delete_active_placeholder();
                session.selected = false;

                if deleted_chars > 0 {
                    let alt_is_meta = TerminalSettings::global(cx).option_as_meta;
                    let backspace = Keystroke::parse("backspace").unwrap();
                    self.terminal.update(cx, move |term, _| {
                        for _ in 0..deleted_chars {
                            term.try_keystroke(&backspace, alt_is_meta);
                        }
                    });
                    cx.notify();
                    cx.stop_propagation();
                    return true;
                }
            } else {
                let deleted = session.backspace_one_in_active_placeholder();
                if deleted {
                    let alt_is_meta = TerminalSettings::global(cx).option_as_meta;
                    let backspace = Keystroke::parse("backspace").unwrap();
                    self.terminal.update(cx, move |term, _| {
                        term.try_keystroke(&backspace, alt_is_meta);
                    });
                    cx.notify();
                    cx.stop_propagation();
                    return true;
                }
            }
            return false;
        }

        // Any unexpected navigation/editing key cancels snippet mode to avoid
        // desync with the remote line editor state.
        let key = event.keystroke.key.as_str();
        let cancel = event.keystroke.modifiers.control
            || event.keystroke.modifiers.platform
            || event.keystroke.modifiers.function
            || event.keystroke.modifiers.alt
            || matches!(
                key,
                "left"
                    | "right"
                    | "up"
                    | "down"
                    | "home"
                    | "end"
                    | "pageup"
                    | "pagedown"
                    | "delete"
            );
        if cancel {
            self.snippet = None;
        }

        false
    }

    fn handle_suggestions_key_down(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        // Suggestions are intentionally conservative: remote/SSH sessions have no shell
        // integration, so we only show/accept append-only hints in shell-like contexts.
        if !TerminalSettings::global(cx).suggestions_enabled || self.snippet.is_some() {
            return false;
        }

        let Some(prompt) = self.prompt_context(cx) else {
            return false;
        };
        if !self.suggestions_eligible_for_content(&prompt.content, cx) {
            self.suggestions.prompt_prefix = None;
            self.suggestions.close();
            return false;
        }

        match event.keystroke.key.as_str() {
            "escape" if self.suggestions.open => {
                self.suggestions.close();
                cx.notify();
                cx.stop_propagation();
                true
            }
            "up" if self.suggestions.open => {
                self.suggestions.selected = move_selection_opt(
                    self.suggestions.selected,
                    self.suggestions.items.len(),
                    SelectionMove::Up,
                );
                cx.notify();
                cx.stop_propagation();
                true
            }
            "down" if self.suggestions.open => {
                self.suggestions.selected = move_selection_opt(
                    self.suggestions.selected,
                    self.suggestions.items.len(),
                    SelectionMove::Down,
                );
                cx.notify();
                cx.stop_propagation();
                true
            }
            "enter" => {
                if self.accept_selected_suggestion(&prompt.content, prompt.cursor_line_id, cx) {
                    cx.stop_propagation();
                    return true;
                }

                if let Some(prompt_prefix) = self.suggestions.prompt_prefix.take() {
                    let line_prefix = extract_cursor_line_prefix(&prompt.content);
                    let input = line_prefix
                        .strip_prefix(&prompt_prefix)
                        .unwrap_or("")
                        .trim()
                        .to_string();
                    if !input.is_empty() {
                        self.queue_command_for_history(input, cx);
                    }
                }
                self.suggestions.close();
                false
            }
            "right" => {
                if self.accept_selected_suggestion(&prompt.content, prompt.cursor_line_id, cx) {
                    cx.stop_propagation();
                    return true;
                }
                false
            }
            "backspace" => {
                if self.suggestions.prompt_prefix.is_none() {
                    self.suggestions.prompt_prefix =
                        Some(extract_cursor_line_prefix(&prompt.content));
                }
                self.schedule_suggestions_update(cx);
                false
            }
            _ => {
                let is_plain_text = event.keystroke.key_char.as_ref().is_some_and(|ch| {
                    !ch.is_empty()
                        && !event.keystroke.is_ime_in_progress()
                        && !event.keystroke.modifiers.control
                        && !event.keystroke.modifiers.platform
                        && !event.keystroke.modifiers.function
                        && !event.keystroke.modifiers.alt
                });

                if is_plain_text {
                    if self.suggestions.prompt_prefix.is_none() {
                        self.suggestions.prompt_prefix =
                            Some(extract_cursor_line_prefix(&prompt.content));
                    }
                    self.schedule_suggestions_update(cx);
                } else if self.suggestions.open {
                    self.suggestions.close();
                }
                false
            }
        }
    }

    fn forward_keystroke_to_terminal(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let (handled, vi_mode_enabled) = self.terminal.update(cx, |term, cx| {
            let handled = term.try_keystroke(
                &event.keystroke,
                TerminalSettings::global(cx).option_as_meta,
            );
            (handled, term.vi_mode_enabled())
        });

        if handled {
            cx.stop_propagation();
            // In terminal vi-mode, keystrokes are usually for scrollback/navigation, so don't
            // force the view back to the bottom.
            if !vi_mode_enabled {
                self.snap_to_bottom_on_input(cx);
            }
            cx.emit(Event::UserInput(UserInput::Keystroke(
                event.keystroke.clone(),
            )));
        }
    }

    fn focus_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.terminal.update(cx, |terminal, _| {
            terminal.set_cursor_shape(self.cursor_shape);
            terminal.focus_in();
        });
        self.blink_cursors(self.blink.epoch, cx);
        window.invalidate_character_coordinates();
        cx.notify();
    }

    fn focus_out(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.terminal.update(cx, |terminal, _| {
            terminal.focus_out();
            terminal.set_cursor_shape(CursorShape::Hollow);
        });
        self.suggestions.close();
        cx.notify();
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let terminal_handle = self.terminal.clone();
        let terminal_view_handle = cx.entity();

        self.sync_scroll_for_render(cx);
        let focused = self.focus_handle.is_focused(window);

        let mut root = self.terminal_view_root_base(cx);
        root = self.terminal_view_root_mouse_handlers(root, cx);
        root = root.child(self.terminal_view_inner_wrapper(
            terminal_handle,
            terminal_view_handle.clone(),
            focused,
            cx,
        ));
        root = root.children(self.collect_overlay_elements(window, cx));

        if !self.context_menu_enabled {
            return root.into_any_element();
        }

        let context_menu_enabled = self.context_menu_enabled;
        let action_context = self.focus_handle.clone();
        let menu_terminal_handle = self.terminal.clone();
        let terminal_view = terminal_view_handle.clone();
        let context_menu_provider = self.context_menu_provider.clone();

        ContextMenu::new("terminal-view-context-menu", root)
            .menu(move |menu, window, cx| {
                Self::build_terminal_context_menu(
                    context_menu_enabled,
                    action_context.clone(),
                    menu_terminal_handle.clone(),
                    terminal_view.clone(),
                    context_menu_provider.clone(),
                    menu,
                    window,
                    cx,
                )
            })
            .into_any_element()
    }
}

impl TerminalView {
    fn terminal_view_root_base(&mut self, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        div()
            .id("terminal-view")
            .size_full()
            .relative()
            .track_focus(&self.focus_handle(cx))
            .key_context(self.dispatch_context(cx))
            .on_action(cx.listener(TerminalView::send_text))
            .on_action(cx.listener(TerminalView::send_keystroke))
            .on_action(cx.listener(TerminalView::open_search))
            .on_action(cx.listener(TerminalView::search_next))
            .on_action(cx.listener(TerminalView::search_previous))
            .on_action(cx.listener(TerminalView::close_search))
            .on_action(cx.listener(TerminalView::search_paste))
            .on_action(cx.listener(TerminalView::copy))
            .on_action(cx.listener(TerminalView::paste))
            .on_action(cx.listener(TerminalView::clear))
            .on_action(cx.listener(TerminalView::reset_font_size))
            .on_action(cx.listener(TerminalView::increase_font_size))
            .on_action(cx.listener(TerminalView::decrease_font_size))
            .on_action(cx.listener(TerminalView::scroll_line_up))
            .on_action(cx.listener(TerminalView::scroll_line_down))
            .on_action(cx.listener(TerminalView::scroll_page_up))
            .on_action(cx.listener(TerminalView::scroll_page_down))
            .on_action(cx.listener(TerminalView::scroll_to_top))
            .on_action(cx.listener(TerminalView::scroll_to_bottom))
            .on_action(cx.listener(TerminalView::toggle_vi_mode))
            .on_action(cx.listener(TerminalView::show_character_palette))
            .on_action(cx.listener(TerminalView::select_all))
            .on_action(cx.listener(TerminalView::start_cast_recording))
            .on_action(cx.listener(TerminalView::stop_cast_recording))
            .on_action(cx.listener(TerminalView::toggle_cast_recording))
            .on_key_down(cx.listener(Self::key_down))
    }

    fn terminal_view_root_mouse_handlers(
        &mut self,
        root: gpui::Stateful<gpui::Div>,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let root = root.on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, event: &MouseDownEvent, window, cx| {
                // Treat the left gutter (line numbers/padding) as UI chrome: allow selecting
                // command blocks there rather than starting a terminal selection.
                let content_bounds = {
                    let terminal = this.terminal.read(cx);
                    terminal.last_content().terminal_bounds.bounds
                };
                if event.position.x < content_bounds.origin.x {
                    this.close_suggestions(cx);
                    this.select_command_block_at_y(
                        event.position.y,
                        event.modifiers.shift,
                        window,
                        cx,
                    );
                    cx.stop_propagation();
                }
            }),
        );

        if !self.context_menu_enabled {
            return root;
        }

        root.on_mouse_down(
            MouseButton::Right,
            cx.listener(|this, event: &MouseDownEvent, window, cx| {
                this.close_suggestions(cx);

                // We treat the left gutter (outside the terminal content bounds) as "UI
                // chrome", not part of the terminal application. Allow the
                // context menu there even when the terminal is in mouse
                // mode (e.g. vim/tmux).
                let (content_bounds, mouse_mode_enabled, has_selection) = {
                    let terminal = this.terminal.read(cx);
                    (
                        terminal.last_content().terminal_bounds.bounds,
                        terminal.mouse_mode(event.modifiers.shift),
                        terminal.last_content().selection.is_some(),
                    )
                };
                let clicked_in_gutter = event.position.x < content_bounds.origin.x;
                if clicked_in_gutter {
                    // Pre-select the block (if any) so context-menu actions apply.
                    this.select_command_block_at_y(
                        event.position.y,
                        event.modifiers.shift,
                        window,
                        cx,
                    );
                    return;
                }

                // When the terminal is in mouse mode (e.g. vim/tmux), don't open the context
                // menu; let the application handle right clicks.
                if mouse_mode_enabled {
                    cx.stop_propagation();
                    return;
                }

                if !has_selection {
                    this.terminal.update(cx, |terminal, _| {
                        terminal.select_word_at_event_position(event);
                    });
                    window.refresh();
                }
            }),
        )
    }

    fn terminal_view_inner_wrapper(
        &mut self,
        terminal_handle: Entity<Terminal>,
        terminal_view_handle: Entity<TerminalView>,
        focused: bool,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        // NOTE: Keep a wrapper div around `TerminalElement`; without it the terminal
        // element can interfere with overlay UI (context menu, etc).
        div()
            .id("terminal-view-inner")
            .size_full()
            .relative()
            .child(TerminalElement::new(
                terminal_handle,
                terminal_view_handle,
                self.focus_handle.clone(),
                focused,
                self.should_show_cursor(focused, cx),
                self.scroll.block_below_cursor.clone(),
            ))
    }

    fn handle_search_overlay_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> SearchOverlayKeyDown {
        if !self.search.search_open {
            return SearchOverlayKeyDown::NotOpen;
        }

        match event.keystroke.key.as_str() {
            "escape" => {
                self.close_search(&SearchClose, window, cx);
            }
            "enter" => {
                self.jump_search(!event.keystroke.modifiers.shift, cx);
            }
            "left" => self.search_overlay_move_cursor(SearchOverlayMove::Left, event, cx),
            "right" => self.search_overlay_move_cursor(SearchOverlayMove::Right, event, cx),
            "home" => self.search_overlay_move_cursor(SearchOverlayMove::Home, event, cx),
            "end" => self.search_overlay_move_cursor(SearchOverlayMove::End, event, cx),
            "backspace" => self.search_overlay_delete(SearchOverlayDelete::Prev, event, cx),
            "delete" => self.search_overlay_delete(SearchOverlayDelete::Next, event, cx),
            // Common terminal muscle memory: cmd/ctrl+a moves to start of the input.
            "a" if event.keystroke.modifiers.secondary() => {
                self.search.search.home();
                cx.notify();
            }
            // Support cmd/ctrl+v paste into the query even if keybinding dispatch is skipped.
            "v" if event.keystroke.modifiers.secondary() => {
                self.search_paste(&SearchPaste, window, cx);
            }
            // Support cmd/ctrl+g next/prev even if keybinding dispatch is skipped.
            "g" if event.keystroke.modifiers.secondary() => {
                self.jump_search(!event.keystroke.modifiers.shift, cx);
            }
            _ => {
                // Text input (including IME) is handled via the InputHandler installed by the
                // terminal element while the search is open.
                //
                // However, on some platforms plain latin text does *not* go through the IME
                // callbacks; in that case we fall back to `key_char`.
                if self.search_overlay_is_composing(event) {
                    // IME is actively composing; don't insert raw keystrokes.
                    return SearchOverlayKeyDown::Return;
                }

                if event.keystroke.modifiers.control
                    || event.keystroke.modifiers.platform
                    || event.keystroke.modifiers.function
                    || event.keystroke.modifiers.alt
                {
                    return SearchOverlayKeyDown::Return;
                }

                if let Some(ch) = event.keystroke.key_char.as_ref()
                    && !ch.is_empty()
                {
                    self.search.search_expected_commit = Some(ch.clone());
                    self.search.search.insert(ch);
                    self.schedule_search_update(cx);
                }
            }
        }

        SearchOverlayKeyDown::StopAndReturn
    }

    fn search_overlay_is_composing(&self, event: &KeyDownEvent) -> bool {
        self.search_overlay_has_marked_text() || event.keystroke.is_ime_in_progress()
    }

    fn search_overlay_has_marked_text(&self) -> bool {
        self.search
            .search_ime_state
            .as_ref()
            .is_some_and(|ime| !ime.marked_text.is_empty())
    }

    fn search_overlay_move_cursor(
        &mut self,
        movement: SearchOverlayMove,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) {
        if self.search_overlay_is_composing(event) {
            // Let the IME handle caret movement inside an active composition.
            return;
        }

        match movement {
            SearchOverlayMove::Left => self.search.search.move_left(),
            SearchOverlayMove::Right => self.search.search.move_right(),
            SearchOverlayMove::Home => self.search.search.home(),
            SearchOverlayMove::End => self.search.search.end(),
        }
        cx.notify();
    }

    fn search_overlay_delete(
        &mut self,
        delete: SearchOverlayDelete,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) {
        let should_ignore = match delete {
            SearchOverlayDelete::Prev => self.search_overlay_has_marked_text(),
            SearchOverlayDelete::Next => self.search_overlay_is_composing(event),
        };
        if should_ignore {
            // Let the platform IME drive composition edits via the InputHandler.
            return;
        }

        let changed = match delete {
            SearchOverlayDelete::Prev => self.search.search.delete_prev(),
            SearchOverlayDelete::Next => self.search.search.delete_next(),
        };
        if changed {
            self.schedule_search_update(cx);
        }
        cx.notify();
    }

    fn sync_scroll_for_render(&mut self, cx: &mut Context<Self>) {
        // Keep the view pinned to the last line while we're following the live view.
        // This needs to happen with the latest `last_content()` (post-sync), so we do it here.
        let display_offset = self.terminal_scroll_state(cx).display_offset;
        if display_offset != 0 {
            self.scroll.scroll_top = Pixels::ZERO;
        } else if self.scroll.stick_to_bottom && self.scroll.block_below_cursor.is_some() {
            self.scroll.scroll_top = self.max_scroll_top(cx);
        }
    }

    fn collect_overlay_elements(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut out: Vec<AnyElement> = Vec::new();
        if let Some(search) = render_search(self, window, cx) {
            out.push(search);
        }
        if let Some(preview) = self.render_scrollbar_preview_overlay(cx) {
            out.push(preview);
        }
        if let Some(rec) = self.render_recording_indicator_overlay(cx) {
            out.push(rec);
        }
        out
    }

    fn render_scrollbar_preview_overlay(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let preview = self.scrollbar_preview().cloned()?;
        let terminal_settings = TerminalSettings::global(cx);
        if !terminal_settings.show_scrollbar {
            return None;
        }

        let ScrollbarPreview {
            anchor,
            start_line_from_top,
            cols,
            rows,
            cells,
            match_range,
            ..
        } = preview;

        let theme = cx.theme();
        let ScrollbarPreviewLayoutState {
            view_bounds,
            line_height,
            cell_width,
            total_lines,
        } = self.scrollbar_preview_layout_state(cx);

        let content_h = line_height * (rows.max(1) as f32) + px(16.0);
        // `preview.anchor` is stored in window coordinates; convert it to this view's
        // local coordinate space so absolute positioning works correctly when the
        // terminal view is embedded (i.e. not at window origin).
        let anchor_y = anchor.y - view_bounds.origin.y;
        let y = scrollbar_preview_overlay_top(anchor_y, view_bounds.size.height, content_h);

        // Use the theme's popover color so the preview reads as an overlay panel,
        // clearly distinct from the terminal background.
        let panel_bg = theme.popover;
        let panel_border = theme.border.opacity(0.9);
        let line_no_fg = theme.foreground.opacity(0.40);
        let line_no_digits = total_lines.max(1).to_string().len();

        let mut body = div()
            .id("terminal-scrollbar-preview")
            .debug_selector(|| "terminal-scrollbar-preview".to_string())
            .absolute()
            .left_0()
            .top(y)
            // Match the terminal's horizontal span, excluding the scrollbar lane so we
            // don't steal hover from the markers.
            .right(SCROLLBAR_WIDTH)
            .bg(panel_bg)
            .border_1()
            .border_color(panel_border)
            .rounded_md()
            .shadow_lg()
            .py(px(8.0))
            .px(px(10.0))
            .font_family(terminal_settings.font_family.clone())
            .text_size(terminal_settings.font_size)
            .font_weight(terminal_settings.font_weight)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| {
                    cx.stop_propagation();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|_, _, _, cx| {
                    cx.stop_propagation();
                }),
            )
            .flex_col();

        let line_numbers = render_scrollbar_preview_line_numbers(
            rows,
            start_line_from_top,
            line_height,
            line_no_fg,
            line_no_digits,
        );

        let text = div()
            .h(line_height * (rows.max(1) as f32))
            .flex_1()
            .overflow_hidden()
            .child(ScrollbarPreviewTextElement::new(
                cells,
                cell_width,
                line_height,
                cols,
                Some(match_range),
            ));

        body = body.child(
            div()
                .flex()
                .items_start()
                .overflow_hidden()
                .child(line_numbers)
                .child(text),
        );

        Some(body.into_any_element())
    }

    fn render_recording_indicator_overlay(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let recording_active = self.cast_recording_active(cx);
        let label = recording_indicator_label(recording_active)?;
        let theme = cx.theme();
        Some(render_recording_indicator_label(theme, label))
    }

    #[allow(clippy::too_many_arguments)]
    fn build_terminal_context_menu(
        context_menu_enabled: bool,
        action_context: FocusHandle,
        menu_terminal_handle: Entity<Terminal>,
        terminal_view: Entity<TerminalView>,
        context_menu_provider: Option<Arc<dyn ContextMenuProvider>>,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut App,
    ) -> PopupMenu {
        if !context_menu_enabled {
            return menu;
        }

        let menu = menu.action_context(action_context);

        if let Some(provider) = context_menu_provider.as_ref() {
            return provider.context_menu(menu, menu_terminal_handle, terminal_view, window, cx);
        }

        let recording_active = menu_terminal_handle.read(cx).cast_recording_active();
        let menu = match recording_context_menu_entry(recording_active) {
            RecordingMenuEntry::Item { checked } => {
                let icon_color = if checked {
                    cx.theme().danger
                } else {
                    cx.theme().muted_foreground
                };
                let icon = Icon::default()
                    .path(TermuaIcon::Record)
                    .text_color(icon_color);

                menu.item(
                    PopupMenuItem::new("Recording")
                        .icon(icon)
                        .checked(checked)
                        .action(Box::new(ToggleCastRecording)),
                )
                .separator()
            }
        };

        menu.menu_with_icon("Copy", IconName::Copy, Box::new(Copy))
            .menu("Paste", Box::new(Paste))
            .separator()
            .menu("SelectAll", Box::new(SelectAll))
            .separator()
            .menu("Clear", Box::new(Clear))
    }

    fn select_command_block_at_y(
        &mut self,
        y: Pixels,
        _shift: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle, cx);
        let debug_toasts = cfg!(debug_assertions);

        let CommandBlockHitLayoutState {
            bounds,
            line_height,
            display_offset,
            max_row,
            cols,
        } = self.command_block_hit_layout_state(cx);

        if line_height <= px(0.0) || bounds.size.height <= px(0.0) {
            return;
        }

        let rel_y = y - bounds.origin.y;
        if rel_y < px(0.0) || rel_y >= bounds.size.height {
            return;
        }

        let mut row = (rel_y / line_height).floor() as i32;
        row = row.clamp(0, max_row);

        let grid_line = row.saturating_sub(display_offset);

        let Some((stable, blocks, block, start_line, end_line)) = (|| {
            let terminal = self.terminal.read(cx);
            let stable = terminal.stable_row_for_grid_line(grid_line)?;
            let blocks = terminal.command_blocks()?;
            let block = crate::command_blocks::block_at_stable_row(&blocks, stable).cloned();
            let (start_line, end_line) = match block.as_ref() {
                Some(block) => {
                    let end_stable = block.output_end_line.unwrap_or(stable);
                    (
                        terminal.grid_line_for_stable_row(block.output_start_line)?,
                        terminal.grid_line_for_stable_row(end_stable)?,
                    )
                }
                None => (0, 0),
            };
            Some((stable, blocks, block, start_line, end_line))
        })() else {
            if debug_toasts {
                self.show_toast(
                    PromptLevel::Info,
                    "Command blocks unavailable",
                    Some("This terminal backend doesn't expose command blocks.".to_string()),
                    window,
                    cx,
                );
            }
            return;
        };

        let Some(_block) = block else {
            self.terminal.update(cx, |terminal, _| {
                terminal.set_selection_range(None);
            });
            window.refresh();
            if debug_toasts {
                self.show_toast(
                    PromptLevel::Info,
                    "No command block here",
                    Some(no_command_block_detail(&blocks, stable)),
                    window,
                    cx,
                );
            }
            return;
        };

        let last_col = cols.saturating_sub(1);

        self.terminal.update(cx, |terminal, _cx| {
            terminal.set_selection_range(Some(crate::SelectionRange {
                start: crate::GridPoint::new(start_line, 0),
                end: crate::GridPoint::new(end_line, last_col),
            }));
        });
        window.refresh();
        cx.notify();
    }
}

#[cfg(test)]
fn format_clock(d: Duration) -> String {
    let secs = d.as_secs();
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 {
        format!("{h:02}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

fn no_command_block_detail(blocks: &[crate::command_blocks::CommandBlock], stable: i64) -> String {
    if blocks.is_empty() {
        "No OSC 133 blocks detected yet. Ensure this tab is a local bash or zsh shell with TERMUA \
         OSC 133 integration active."
            .to_string()
    } else {
        match blocks.last() {
            Some(last) => format!(
                "stable_row={stable} blocks={} last_block.start={} last_block.end={:?}",
                blocks.len(),
                last.output_start_line,
                last.output_end_line
            ),
            None => format!("stable_row={stable} blocks=0"),
        }
    }
}

fn subscribe_for_terminal_events(
    terminal: &Entity<Terminal>,
    window: &mut Window,
    cx: &mut Context<TerminalView>,
) -> Vec<Subscription> {
    let terminal_subscription = cx.observe(terminal, |terminal_view, terminal, cx| {
        if let Some(blocks) = terminal.read(cx).command_blocks() {
            terminal_view.flush_successful_history_from_blocks(&blocks, cx);
        }
        cx.notify();
    });

    let terminal_events_subscription = cx.subscribe_in(
        terminal,
        window,
        move |terminal_view, terminal, event, window, cx| {
            handle_terminal_event(terminal_view, terminal, event, window, cx);
        },
    );
    vec![terminal_subscription, terminal_events_subscription]
}

fn handle_terminal_event(
    terminal_view: &mut TerminalView,
    terminal: &Entity<Terminal>,
    event: &Event,
    window: &mut Window,
    cx: &mut Context<TerminalView>,
) {
    match event {
        Event::Wakeup => {
            cx.notify();
            cx.emit(Event::Wakeup);
        }
        Event::Bell => {
            terminal_view.has_bell = true;
            cx.emit(Event::Wakeup);
        }
        Event::BlinkChanged(blinking) => {
            if matches!(
                TerminalSettings::global(cx).blinking,
                TerminalBlink::TerminalControlled
            ) {
                terminal_view.blink.terminal_enabled = *blinking;
            }
        }
        Event::NewNavigationTarget(url_opt) => {
            let hovered_word = {
                let terminal = terminal.read(cx);
                terminal.last_content().last_hovered_word.clone()
            };
            match url_opt.as_ref().zip(hovered_word.as_ref()) {
                Some((_url, hovered_word)) => {
                    if Some(hovered_word) != terminal_view.hover_word.as_ref() {
                        terminal_view.hover_word = Some(hovered_word.clone());
                        cx.notify();
                    }
                }
                None => {
                    terminal_view.hover_word = None;
                    cx.notify();
                }
            }
        }
        Event::Open(url) => cx.open_url(url),
        Event::SelectionsChanged => {
            window.invalidate_character_coordinates();
        }
        _ => {}
    }
}

fn scrollbar_preview_overlay_top(
    anchor_y: Pixels,
    view_height: Pixels,
    content_h: Pixels,
) -> Pixels {
    let mut y = anchor_y - content_h / 2.0;
    let top_pad = px(12.0);
    // Keep extra breathing room at the bottom so the preview doesn't get
    // obscured by footer bars/overlays (e.g. transfer UI).
    let bottom_pad = px(56.0);
    y = y.clamp(top_pad, (view_height - content_h - bottom_pad).max(top_pad));
    y
}

fn render_scrollbar_preview_line_numbers(
    rows: usize,
    start_line_from_top: usize,
    line_height: Pixels,
    line_no_fg: gpui::Hsla,
    line_no_digits: usize,
) -> gpui::Div {
    let mut line_numbers = div().flex_col().flex_shrink_0();
    for i in 0..rows {
        let line_no = start_line_from_top.saturating_add(i).saturating_add(1);
        line_numbers = line_numbers.child(
            div()
                .h(line_height)
                .whitespace_nowrap()
                .overflow_hidden()
                .text_color(line_no_fg)
                .child(format_scrollbar_preview_line_number(
                    line_no,
                    line_no_digits,
                )),
        );
    }
    line_numbers
}

#[cfg(test)]
mod scrollbar_preview_tests {
    use std::rc::Rc;

    use super::*;

    #[test]
    fn format_scrollbar_preview_line_number_right_aligns() {
        assert_eq!(format_scrollbar_preview_line_number(3, 1), "3\u{00A0}");
        assert_eq!(format_scrollbar_preview_line_number(3, 4), "   3\u{00A0}");
    }

    #[test]
    fn format_scrollbar_preview_line_number_uses_trailing_space() {
        // The preview renderer now uses real terminal cells (with fixed-width positioning), so
        // we no longer need to preserve spaces via NBSP substitution.
        assert_eq!(format_scrollbar_preview_line_number(1, 1), "1\u{00A0}");
    }

    use std::{borrow::Cow, ops::RangeInclusive};

    use gpui::{
        AppContext, Bounds, Context as GpuiContext, Keystroke, Modifiers, MouseDownEvent,
        MouseMoveEvent, MouseUpEvent, Pixels, ScrollWheelEvent, Window, point, px, size,
    };
    use gpui_component::Root;

    use crate::{
        Cell, GridPoint, IndexedCell, TerminalBackend, TerminalContent, TerminalShutdownPolicy,
        TerminalType,
    };

    pub(super) struct PreviewBackend {
        content: TerminalContent,
        matches: Vec<RangeInclusive<GridPoint>>,
        total_lines: usize,
        viewport_lines: usize,
        preview_cols: usize,
        preview_rows: usize,
        preview_cells: Vec<IndexedCell>,
    }

    impl PreviewBackend {
        pub(super) fn new() -> Self {
            // Give the renderer something to work with; actual bounds will be set via `set_size`.
            let content = TerminalContent::default();

            // Make a single match close to the bottom of the buffer.
            // With total_lines=100 and viewport_lines=20, line_coord=19 maps to buffer index 99.
            let matches = vec![RangeInclusive::new(
                GridPoint::new(19, 0),
                GridPoint::new(19, 1),
            )];

            let preview_cols = 24;
            let preview_rows = 7;
            let mut preview_cells = Vec::new();
            for row in 0..preview_rows {
                for col in 0..preview_cols {
                    preview_cells.push(IndexedCell {
                        point: GridPoint::new(row as i32, col),
                        cell: Cell {
                            c: if col == 0 { '>' } else { 'x' },
                            ..Default::default()
                        },
                    });
                }
            }

            Self {
                content,
                matches,
                total_lines: 100,
                viewport_lines: 20,
                preview_cols,
                preview_rows,
                preview_cells,
            }
        }
    }

    impl TerminalBackend for PreviewBackend {
        fn backend_name(&self) -> &'static str {
            "preview-test"
        }

        fn sync(&mut self, _window: &mut Window, _cx: &mut GpuiContext<crate::Terminal>) {}

        fn shutdown(
            &mut self,
            _policy: TerminalShutdownPolicy,
            _cx: &mut GpuiContext<crate::Terminal>,
        ) {
        }

        fn last_content(&self) -> &TerminalContent {
            &self.content
        }

        fn matches(&self) -> &[RangeInclusive<GridPoint>] {
            &self.matches
        }

        fn last_clicked_line(&self) -> Option<i32> {
            None
        }

        fn vi_mode_enabled(&self) -> bool {
            false
        }

        fn mouse_mode(&self, _shift: bool) -> bool {
            false
        }

        fn selection_started(&self) -> bool {
            false
        }

        fn set_cursor_shape(&mut self, _cursor_shape: CursorShape) {}

        fn total_lines(&self) -> usize {
            self.total_lines
        }

        fn viewport_lines(&self) -> usize {
            self.viewport_lines
        }

        fn activate_match(&mut self, _index: usize) {}

        fn select_matches(&mut self, _matches: &[RangeInclusive<GridPoint>]) {}

        fn select_all(&mut self) {}

        fn copy(&mut self, _keep_selection: Option<bool>, _cx: &mut GpuiContext<crate::Terminal>) {}

        fn clear(&mut self) {}

        fn scroll_line_up(&mut self) {}
        fn scroll_up_by(&mut self, _lines: usize) {}
        fn scroll_line_down(&mut self) {}
        fn scroll_down_by(&mut self, _lines: usize) {}
        fn scroll_page_up(&mut self) {}
        fn scroll_page_down(&mut self) {}
        fn scroll_to_top(&mut self) {}
        fn scroll_to_bottom(&mut self) {}

        fn scrolled_to_top(&self) -> bool {
            true
        }

        fn scrolled_to_bottom(&self) -> bool {
            true
        }

        fn set_size(&mut self, new_bounds: TerminalBounds) {
            self.content.terminal_bounds = new_bounds;
        }

        fn input(&mut self, _input: Cow<'static, [u8]>) {}

        fn paste(&mut self, _text: &str) {}

        fn focus_in(&self) {}

        fn focus_out(&mut self) {}

        fn toggle_vi_mode(&mut self) {}

        fn try_keystroke(&mut self, _keystroke: &Keystroke, _alt_is_meta: bool) -> bool {
            false
        }

        fn try_modifiers_change(
            &mut self,
            _modifiers: &Modifiers,
            _window: &Window,
            _cx: &mut GpuiContext<crate::Terminal>,
        ) {
        }

        fn mouse_move(&mut self, _e: &MouseMoveEvent, _cx: &mut GpuiContext<crate::Terminal>) {}

        fn select_word_at_event_position(&mut self, _e: &MouseDownEvent) {}

        fn mouse_drag(
            &mut self,
            _e: &MouseMoveEvent,
            _region: Bounds<Pixels>,
            _cx: &mut GpuiContext<crate::Terminal>,
        ) {
        }

        fn mouse_down(&mut self, _e: &MouseDownEvent, _cx: &mut GpuiContext<crate::Terminal>) {}

        fn mouse_up(&mut self, _e: &MouseUpEvent, _cx: &GpuiContext<crate::Terminal>) {}

        fn scroll_wheel(&mut self, _e: &ScrollWheelEvent) {}

        fn get_content(&self) -> String {
            String::new()
        }

        fn last_n_non_empty_lines(&self, _n: usize) -> Vec<String> {
            Vec::new()
        }

        fn preview_cells_from_top(
            &self,
            _start_line: usize,
            _count: usize,
        ) -> (usize, usize, Vec<IndexedCell>) {
            (
                self.preview_cols,
                self.preview_rows,
                self.preview_cells.clone(),
            )
        }
    }

    #[gpui::test]
    fn scrollbar_preview_is_not_obscured_by_footer_bar(cx: &mut gpui::TestAppContext) {
        cx.update(|app| {
            crate::init(app);
        });

        let view_slot: Rc<std::cell::RefCell<Option<Entity<TerminalView>>>> =
            Rc::new(std::cell::RefCell::new(None));
        let view_slot_for_window = view_slot.clone();

        let (root, v) = cx.add_window_view(|window, cx| {
            let terminal = cx.new(|_| {
                crate::Terminal::new(TerminalType::WezTerm, Box::new(PreviewBackend::new()))
            });
            let terminal_view = cx.new(|cx| TerminalView::new(terminal, window, cx));
            *view_slot_for_window.borrow_mut() = Some(terminal_view.clone());

            terminal_view.update(cx, |this, cx| {
                // Anchor the preview near the bottom of the window so the default clamp behavior
                // would overlap a bottom footer bar overlay.
                this.set_scrollbar_preview_for_match(0, point(px(0.0), px(690.0)), cx);
            });

            Root::new(terminal_view, window, cx)
        });

        v.draw(
            point(px(0.0), px(0.0)),
            size(
                gpui::AvailableSpace::Definite(px(900.0)),
                gpui::AvailableSpace::Definite(px(700.0)),
            ),
            move |_, _| {
                div().size_full().relative().child(root).child(
                    // Simulate a bottom "footer bar" overlay that can obscure the preview
                    // tooltip when a search marker is near the bottom of the scrollbar.
                    div()
                        .debug_selector(|| "test-footerbar".to_string())
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .h(px(48.0)),
                )
            },
        );

        v.run_until_parked();

        let view = view_slot
            .borrow()
            .clone()
            .expect("expected terminal view to be captured");
        let preview_set = v.read_entity(&view, |this, _app| this.scrollbar_preview().is_some());
        assert!(preview_set, "expected scrollbar preview state to be set");

        let preview_bounds = v
            .debug_bounds("terminal-scrollbar-preview")
            .expect("scrollbar preview should exist");
        let footer_bounds = v
            .debug_bounds("test-footerbar")
            .expect("test footer bar should exist");

        let preview_bottom = preview_bounds.origin.y + preview_bounds.size.height;
        assert!(
            preview_bottom <= footer_bounds.origin.y,
            "expected preview bottom ({preview_bottom:?}) to be above footer bar top ({:?})",
            footer_bounds.origin.y
        );
    }
}

#[cfg(test)]
mod suggestion_selection_tests {
    use std::rc::Rc;

    use gpui::AppContext;

    use super::{scrollbar_preview_tests::PreviewBackend, *};

    #[gpui::test]
    fn suggestions_open_has_no_default_selection(cx: &mut gpui::TestAppContext) {
        cx.update(|app| {
            crate::init(app);
        });

        let mut state = cx.update(|app| SuggestionsState::new(app));
        assert_eq!(state.selected, None);

        state.open_with_items(vec![SuggestionItem {
            full_text: "ls -al".to_string(),
            score: 0,
            description: None,
        }]);

        assert!(state.open);
        assert_eq!(state.selected, None);
    }

    #[gpui::test]
    fn enter_does_not_accept_suggestion_when_unselected(cx: &mut gpui::TestAppContext) {
        use std::cell::RefCell;

        use gpui_component::Root;

        cx.update(|app| {
            crate::init(app);
        });

        let view_slot: Rc<RefCell<Option<Entity<TerminalView>>>> = Rc::new(RefCell::new(None));
        let view_slot_for_window = view_slot.clone();

        let (_root, v) = cx.add_window_view(|window, cx| {
            let terminal = cx.new(|_| {
                crate::Terminal::new(
                    crate::TerminalType::WezTerm,
                    Box::new(PreviewBackend::new()),
                )
            });
            let terminal_view = cx.new(|cx| TerminalView::new(terminal, window, cx));
            *view_slot_for_window.borrow_mut() = Some(terminal_view.clone());
            Root::new(terminal_view, window, cx)
        });

        let view = view_slot
            .borrow()
            .clone()
            .expect("expected terminal view to be captured");

        let accepted = v.update(|_window, cx| {
            view.update(cx, |this, cx| {
                this.suggestions.open = true;
                this.suggestions.items = vec![SuggestionItem {
                    full_text: "ls -al".to_string(),
                    score: 0,
                    description: None,
                }];
                this.suggestions.selected = None;
                this.suggestions.hovered = Some(0);
                this.suggestions.prompt_prefix = Some("".to_string());

                let mut content = TerminalContent::default();
                content.cursor.point = GridPoint::new(0, 2);
                content.cells = vec![
                    crate::IndexedCell {
                        point: GridPoint::new(0, 0),
                        cell: crate::Cell {
                            c: 'l',
                            ..Default::default()
                        },
                    },
                    crate::IndexedCell {
                        point: GridPoint::new(0, 1),
                        cell: crate::Cell {
                            c: 's',
                            ..Default::default()
                        },
                    },
                ];

                this.accept_selected_suggestion(&content, None, cx)
            })
        });

        assert!(!accepted, "should not accept without a selection");
        let still_open = v.read_entity(&view, |this, _app| this.suggestions.open);
        assert!(
            still_open,
            "accept should not close suggestions when unselected"
        );
    }

    #[gpui::test]
    fn accept_suggestion_at_index_works_without_selection(cx: &mut gpui::TestAppContext) {
        use std::cell::RefCell;

        use gpui_component::Root;

        cx.update(|app| {
            crate::init(app);
        });

        let view_slot: Rc<RefCell<Option<Entity<TerminalView>>>> = Rc::new(RefCell::new(None));
        let view_slot_for_window = view_slot.clone();

        let (_root, v) = cx.add_window_view(|window, cx| {
            let terminal = cx.new(|_| {
                crate::Terminal::new(
                    crate::TerminalType::WezTerm,
                    Box::new(PreviewBackend::new()),
                )
            });
            let terminal_view = cx.new(|cx| TerminalView::new(terminal, window, cx));
            *view_slot_for_window.borrow_mut() = Some(terminal_view.clone());
            Root::new(terminal_view, window, cx)
        });

        let view = view_slot
            .borrow()
            .clone()
            .expect("expected terminal view to be captured");

        let accepted = v.update(|_window, cx| {
            view.update(cx, |this, cx| {
                this.suggestions.open = true;
                this.suggestions.items = vec![SuggestionItem {
                    full_text: "ls -al".to_string(),
                    score: 0,
                    description: None,
                }];
                this.suggestions.selected = None;
                this.suggestions.hovered = None;
                this.suggestions.prompt_prefix = Some("".to_string());

                let mut content = TerminalContent::default();
                content.cursor.point = GridPoint::new(0, 2);
                content.cells = vec![
                    crate::IndexedCell {
                        point: GridPoint::new(0, 0),
                        cell: crate::Cell {
                            c: 'l',
                            ..Default::default()
                        },
                    },
                    crate::IndexedCell {
                        point: GridPoint::new(0, 1),
                        cell: crate::Cell {
                            c: 's',
                            ..Default::default()
                        },
                    },
                ];

                this.accept_suggestion_at_index(0, &content, None, cx)
            })
        });

        assert!(accepted, "expected accept by index to succeed");
        let still_open = v.read_entity(&view, |this, _app| this.suggestions.open);
        assert!(!still_open, "accept should close suggestions");
    }

    #[gpui::test]
    fn focus_out_closes_suggestions(cx: &mut gpui::TestAppContext) {
        use std::cell::RefCell;

        use gpui_component::Root;

        cx.update(|app| {
            crate::init(app);
        });

        let view_slot: Rc<RefCell<Option<Entity<TerminalView>>>> = Rc::new(RefCell::new(None));
        let view_slot_for_window = view_slot.clone();

        let (_root, v) = cx.add_window_view(|window, cx| {
            let terminal = cx.new(|_| {
                crate::Terminal::new(
                    crate::TerminalType::WezTerm,
                    Box::new(PreviewBackend::new()),
                )
            });
            let terminal_view = cx.new(|cx| TerminalView::new(terminal, window, cx));
            *view_slot_for_window.borrow_mut() = Some(terminal_view.clone());
            Root::new(terminal_view, window, cx)
        });

        let view = view_slot
            .borrow()
            .clone()
            .expect("expected terminal view to be captured");

        v.update(|window, cx| {
            view.update(cx, |this, cx| {
                this.suggestions.open = true;
                this.suggestions.items = vec![SuggestionItem {
                    full_text: "ls -al".to_string(),
                    score: 0,
                    description: None,
                }];
                this.suggestions.selected = None;
                this.suggestions.hovered = None;
                this.suggestions.prompt_prefix = Some("".to_string());

                this.focus_out(window, cx);
            });
        });

        let still_open = v.read_entity(&view, |this, _app| this.suggestions.open);
        assert!(!still_open, "focus out should close suggestions");
    }
}

#[cfg(test)]
mod suggestion_acceptance_shell_agnostic_tests {
    use std::{
        borrow::Cow,
        cell::RefCell,
        rc::Rc,
        sync::{Arc, Mutex},
    };

    use gpui::{AppContext, Bounds, Modifiers, MouseMoveEvent, MouseUpEvent, Pixels};
    use gpui_component::Root;

    use super::*;
    use crate::TerminalBackend;

    #[derive(Clone, Debug, Eq, PartialEq)]
    enum BackendEvent {
        Input(Vec<u8>),
        Keystroke(String),
    }

    struct RecordingBackend {
        content: TerminalContent,
        log: Arc<Mutex<Vec<BackendEvent>>>,
    }

    impl RecordingBackend {
        fn new(log: Arc<Mutex<Vec<BackendEvent>>>) -> Self {
            Self {
                content: TerminalContent::default(),
                log,
            }
        }
    }

    impl TerminalBackend for RecordingBackend {
        fn backend_name(&self) -> &'static str {
            "recording-test"
        }

        fn sync(&mut self, _window: &mut Window, _cx: &mut Context<crate::Terminal>) {}

        fn last_content(&self) -> &TerminalContent {
            &self.content
        }

        fn matches(&self) -> &[RangeInclusive<GridPoint>] {
            &[]
        }

        fn last_clicked_line(&self) -> Option<i32> {
            None
        }

        fn vi_mode_enabled(&self) -> bool {
            false
        }

        fn mouse_mode(&self, _shift: bool) -> bool {
            false
        }

        fn selection_started(&self) -> bool {
            false
        }

        fn set_cursor_shape(&mut self, _cursor_shape: CursorShape) {}

        fn total_lines(&self) -> usize {
            0
        }

        fn viewport_lines(&self) -> usize {
            0
        }

        fn activate_match(&mut self, _index: usize) {}

        fn select_matches(&mut self, _matches: &[RangeInclusive<GridPoint>]) {}

        fn select_all(&mut self) {}

        fn copy(&mut self, _keep_selection: Option<bool>, _cx: &mut Context<crate::Terminal>) {}

        fn clear(&mut self) {}

        fn scroll_line_up(&mut self) {}

        fn scroll_up_by(&mut self, _lines: usize) {}

        fn scroll_line_down(&mut self) {}

        fn scroll_down_by(&mut self, _lines: usize) {}

        fn scroll_page_up(&mut self) {}

        fn scroll_page_down(&mut self) {}

        fn scroll_to_top(&mut self) {}

        fn scroll_to_bottom(&mut self) {}

        fn scrolled_to_top(&self) -> bool {
            true
        }

        fn scrolled_to_bottom(&self) -> bool {
            true
        }

        fn set_size(&mut self, new_bounds: TerminalBounds) {
            self.content.terminal_bounds = new_bounds;
        }

        fn input(&mut self, input: Cow<'static, [u8]>) {
            self.log
                .lock()
                .expect("log lock poisoned")
                .push(BackendEvent::Input(input.to_vec()));
        }

        fn paste(&mut self, text: &str) {
            self.log
                .lock()
                .expect("log lock poisoned")
                .push(BackendEvent::Input(text.as_bytes().to_vec()));
        }

        fn focus_in(&self) {}

        fn focus_out(&mut self) {}

        fn toggle_vi_mode(&mut self) {}

        fn try_keystroke(&mut self, keystroke: &Keystroke, _alt_is_meta: bool) -> bool {
            self.log
                .lock()
                .expect("log lock poisoned")
                .push(BackendEvent::Keystroke(keystroke.key.clone()));
            true
        }

        fn try_modifiers_change(
            &mut self,
            _modifiers: &Modifiers,
            _window: &Window,
            _cx: &mut Context<crate::Terminal>,
        ) {
        }

        fn mouse_move(&mut self, _e: &MouseMoveEvent, _cx: &mut Context<crate::Terminal>) {}

        fn select_word_at_event_position(&mut self, _e: &MouseDownEvent) {}

        fn mouse_drag(
            &mut self,
            _e: &MouseMoveEvent,
            _region: Bounds<Pixels>,
            _cx: &mut Context<crate::Terminal>,
        ) {
        }

        fn mouse_down(&mut self, _e: &MouseDownEvent, _cx: &mut Context<crate::Terminal>) {}

        fn mouse_up(&mut self, _e: &MouseUpEvent, _cx: &Context<crate::Terminal>) {}

        fn scroll_wheel(&mut self, _e: &ScrollWheelEvent) {}

        fn get_content(&self) -> String {
            String::new()
        }

        fn last_n_non_empty_lines(&self, _n: usize) -> Vec<String> {
            Vec::new()
        }
    }

    fn line_content(text: &str, cursor_col: usize) -> TerminalContent {
        let mut content = TerminalContent::default();
        content.cursor.point = GridPoint::new(0, cursor_col);
        content.cells = text
            .chars()
            .enumerate()
            .map(|(column, ch)| crate::IndexedCell {
                point: GridPoint::new(0, column),
                cell: crate::Cell {
                    c: ch,
                    ..Default::default()
                },
            })
            .collect();
        content
    }

    #[gpui::test]
    fn accept_selected_suggestion_allows_shell_prompt_decorations_after_cursor(
        cx: &mut gpui::TestAppContext,
    ) {
        let log = Arc::new(Mutex::new(Vec::<BackendEvent>::new()));

        cx.update(|app| {
            crate::init(app);
            app.global_mut::<TerminalSettings>().suggestions_enabled = true;
        });

        let view_slot: Rc<RefCell<Option<Entity<TerminalView>>>> = Rc::new(RefCell::new(None));
        let view_slot_for_window = view_slot.clone();
        let log_for_backend = Arc::clone(&log);

        let (_root, v) = cx.add_window_view(|window, cx| {
            let terminal = cx.new(|_| {
                crate::Terminal::new(
                    crate::TerminalType::WezTerm,
                    Box::new(RecordingBackend::new(Arc::clone(&log_for_backend))),
                )
            });
            let terminal_view = cx.new(|cx| TerminalView::new(terminal, window, cx));
            *view_slot_for_window.borrow_mut() = Some(terminal_view.clone());
            Root::new(terminal_view, window, cx)
        });

        let view = view_slot
            .borrow()
            .clone()
            .expect("expected terminal view to be captured");

        let accepted = v.update(|_window, cx| {
            view.update(cx, |this, cx| {
                this.suggestions.open = true;
                this.suggestions.items = vec![SuggestionItem {
                    full_text: "git status".to_string(),
                    score: 0,
                    description: None,
                }];
                this.suggestions.selected = Some(0);
                this.suggestions.prompt_prefix = Some("$ ".to_string());

                let content = line_content("$ git st      user@host", 8);
                this.accept_selected_suggestion(&content, None, cx)
            })
        });

        assert!(
            accepted,
            "expected right-side prompt text to not block acceptance"
        );
        assert_eq!(
            log.lock().expect("log lock poisoned").as_slice(),
            &[BackendEvent::Input(b"atus".to_vec())]
        );
        let still_open = v.read_entity(&view, |this, _app| this.suggestions.open);
        assert!(!still_open, "accept should close suggestions");
    }

    #[gpui::test]
    fn accept_selected_suggestion_falls_back_when_prompt_prefix_changes(
        cx: &mut gpui::TestAppContext,
    ) {
        let log = Arc::new(Mutex::new(Vec::<BackendEvent>::new()));

        cx.update(|app| {
            crate::init(app);
            app.global_mut::<TerminalSettings>().suggestions_enabled = true;
        });

        let view_slot: Rc<RefCell<Option<Entity<TerminalView>>>> = Rc::new(RefCell::new(None));
        let view_slot_for_window = view_slot.clone();
        let log_for_backend = Arc::clone(&log);

        let (_root, v) = cx.add_window_view(|window, cx| {
            let terminal = cx.new(|_| {
                crate::Terminal::new(
                    crate::TerminalType::WezTerm,
                    Box::new(RecordingBackend::new(Arc::clone(&log_for_backend))),
                )
            });
            let terminal_view = cx.new(|cx| TerminalView::new(terminal, window, cx));
            *view_slot_for_window.borrow_mut() = Some(terminal_view.clone());
            Root::new(terminal_view, window, cx)
        });

        let accepted = v.update(|_window, cx| {
            view_slot
                .borrow()
                .clone()
                .expect("expected terminal view to be captured")
                .update(cx, |this, cx| {
                    this.suggestions.open = true;
                    this.suggestions.items = vec![SuggestionItem {
                        full_text: "git status".to_string(),
                        score: 0,
                        description: None,
                    }];
                    this.suggestions.selected = Some(0);
                    this.suggestions.prompt_prefix = Some("old> ".to_string());

                    let content = line_content("new> git st", 11);
                    this.accept_selected_suggestion(&content, None, cx)
                })
        });

        assert!(
            accepted,
            "expected shell prompt redraws to not block acceptance"
        );
        assert_eq!(
            log.lock().expect("log lock poisoned").as_slice(),
            &[BackendEvent::Input(b"atus".to_vec())]
        );
    }
}

#[cfg(test)]
mod prompt_context_tests {
    use std::{cell::RefCell, rc::Rc};

    use gpui::AppContext;
    use gpui_component::Root;

    use super::{scrollbar_preview_tests::PreviewBackend, *};

    #[gpui::test]
    fn prompt_context_snapshots_content_and_cursor_line_id(cx: &mut gpui::TestAppContext) {
        cx.update(|app| {
            crate::init(app);
            app.global_mut::<TerminalSettings>().suggestions_enabled = true;
        });

        let view_slot: Rc<RefCell<Option<Entity<TerminalView>>>> = Rc::new(RefCell::new(None));
        let view_slot_for_window = view_slot.clone();

        let (_root, window_cx) = cx.add_window_view(|window, cx| {
            let terminal = cx.new(|_| {
                crate::Terminal::new(
                    crate::TerminalType::WezTerm,
                    Box::new(PreviewBackend::new()),
                )
            });
            let terminal_view = cx.new(|cx| TerminalView::new(terminal, window, cx));
            *view_slot_for_window.borrow_mut() = Some(terminal_view.clone());
            Root::new(terminal_view, window, cx)
        });

        let view = view_slot
            .borrow()
            .clone()
            .expect("expected terminal view to be captured");

        window_cx.update(|_window, cx| {
            view.update(cx, |this, cx| {
                this.suggestions.prompt_prefix = Some("$ ".to_string());
                let ctx = this.prompt_context(cx).expect("expected prompt context");
                assert!(this.suggestions_eligible_for_content(&ctx.content, cx));
                assert_eq!(ctx.cursor_line_id, None);
            });
        });
    }
}

#[cfg(test)]
mod snippet_placeholder_key_down_tests {
    use std::{
        borrow::Cow,
        sync::{Arc, Mutex},
    };

    use gpui::{AppContext, Bounds, Modifiers, MouseMoveEvent, MouseUpEvent, Pixels};
    use gpui_component::Root;

    use super::*;
    use crate::TerminalBackend;

    #[derive(Clone, Debug, Eq, PartialEq)]
    enum BackendEvent {
        Input(Vec<u8>),
        Keystroke(String),
    }

    struct RecordingBackend {
        content: TerminalContent,
        log: Arc<Mutex<Vec<BackendEvent>>>,
    }

    impl RecordingBackend {
        fn new(log: Arc<Mutex<Vec<BackendEvent>>>) -> Self {
            Self {
                content: TerminalContent::default(),
                log,
            }
        }
    }

    impl TerminalBackend for RecordingBackend {
        fn backend_name(&self) -> &'static str {
            "recording-test"
        }

        fn sync(&mut self, _window: &mut Window, _cx: &mut Context<crate::Terminal>) {}

        fn last_content(&self) -> &TerminalContent {
            &self.content
        }

        fn matches(&self) -> &[RangeInclusive<GridPoint>] {
            &[]
        }

        fn last_clicked_line(&self) -> Option<i32> {
            None
        }

        fn vi_mode_enabled(&self) -> bool {
            false
        }

        fn mouse_mode(&self, _shift: bool) -> bool {
            false
        }

        fn selection_started(&self) -> bool {
            false
        }

        fn set_cursor_shape(&mut self, _cursor_shape: CursorShape) {}

        fn total_lines(&self) -> usize {
            0
        }

        fn viewport_lines(&self) -> usize {
            0
        }

        fn activate_match(&mut self, _index: usize) {}

        fn select_matches(&mut self, _matches: &[RangeInclusive<GridPoint>]) {}

        fn select_all(&mut self) {}

        fn copy(&mut self, _keep_selection: Option<bool>, _cx: &mut Context<crate::Terminal>) {}

        fn clear(&mut self) {}

        fn scroll_line_up(&mut self) {}

        fn scroll_up_by(&mut self, _lines: usize) {}

        fn scroll_line_down(&mut self) {}

        fn scroll_down_by(&mut self, _lines: usize) {}

        fn scroll_page_up(&mut self) {}

        fn scroll_page_down(&mut self) {}

        fn scroll_to_top(&mut self) {}

        fn scroll_to_bottom(&mut self) {}

        fn scrolled_to_top(&self) -> bool {
            true
        }

        fn scrolled_to_bottom(&self) -> bool {
            true
        }

        fn set_size(&mut self, new_bounds: TerminalBounds) {
            self.content.terminal_bounds = new_bounds;
        }

        fn input(&mut self, input: Cow<'static, [u8]>) {
            self.log
                .lock()
                .expect("log lock poisoned")
                .push(BackendEvent::Input(input.to_vec()));
        }

        fn paste(&mut self, text: &str) {
            self.log
                .lock()
                .expect("log lock poisoned")
                .push(BackendEvent::Input(text.as_bytes().to_vec()));
        }

        fn focus_in(&self) {}

        fn focus_out(&mut self) {}

        fn toggle_vi_mode(&mut self) {}

        fn try_keystroke(&mut self, keystroke: &Keystroke, _alt_is_meta: bool) -> bool {
            self.log
                .lock()
                .expect("log lock poisoned")
                .push(BackendEvent::Keystroke(keystroke.key.clone()));
            true
        }

        fn try_modifiers_change(
            &mut self,
            _modifiers: &Modifiers,
            _window: &Window,
            _cx: &mut Context<crate::Terminal>,
        ) {
        }

        fn mouse_move(&mut self, _e: &MouseMoveEvent, _cx: &mut Context<crate::Terminal>) {}

        fn select_word_at_event_position(&mut self, _e: &MouseDownEvent) {}

        fn mouse_drag(
            &mut self,
            _e: &MouseMoveEvent,
            _region: Bounds<Pixels>,
            _cx: &mut Context<crate::Terminal>,
        ) {
        }

        fn mouse_down(&mut self, _e: &MouseDownEvent, _cx: &mut Context<crate::Terminal>) {}

        fn mouse_up(&mut self, _e: &MouseUpEvent, _cx: &Context<crate::Terminal>) {}

        fn scroll_wheel(&mut self, _e: &ScrollWheelEvent) {}

        fn get_content(&self) -> String {
            String::new()
        }

        fn last_n_non_empty_lines(&self, _n: usize) -> Vec<String> {
            Vec::new()
        }
    }

    #[gpui::test]
    fn typing_replaces_selected_snippet_placeholder(cx: &mut gpui::TestAppContext) {
        use std::{cell::RefCell, rc::Rc};

        cx.update(|app| {
            crate::init(app);
            app.global_mut::<TerminalSettings>().suggestions_enabled = true;
        });

        let log = Arc::new(Mutex::new(Vec::<BackendEvent>::new()));

        let view_slot: Rc<RefCell<Option<Entity<TerminalView>>>> = Rc::new(RefCell::new(None));
        let view_slot_for_window = view_slot.clone();
        let log_for_backend = Arc::clone(&log);

        let (_root, window_cx) = cx.add_window_view(|window, cx| {
            let terminal = cx.new(|_| {
                crate::Terminal::new(
                    crate::TerminalType::WezTerm,
                    Box::new(RecordingBackend::new(Arc::clone(&log_for_backend))),
                )
            });
            let terminal_view = cx.new(|cx| TerminalView::new(terminal, window, cx));
            *view_slot_for_window.borrow_mut() = Some(terminal_view.clone());
            Root::new(terminal_view, window, cx)
        });

        let view = view_slot
            .borrow()
            .clone()
            .expect("expected terminal view to be captured");

        window_cx.update(|window, cx| {
            view.update(cx, |this, cx| {
                let mut session = SnippetSession::new(
                    "body".to_string(),
                    vec![crate::snippet::TabStop {
                        index: 1,
                        range_chars: 0..4,
                    }],
                );
                session.start_point = this.terminal.read(cx).last_content().cursor.point;
                session.active = 0;
                session.cursor_offset_chars = 4;
                session.selected = true;
                this.snippet = Some(session);
            });

            let focus = view.read(cx).focus_handle.clone();
            window.focus(&focus, cx);
        });

        window_cx.simulate_keystrokes("x");

        let rendered = window_cx.read_entity(&view, |this, _app| {
            this.snippet
                .as_ref()
                .expect("expected snippet to remain active")
                .rendered
                .clone()
        });
        assert_eq!(rendered, "x");

        let log = log.lock().expect("log lock poisoned").clone();
        assert_eq!(
            log,
            vec![
                BackendEvent::Keystroke("backspace".to_string()),
                BackendEvent::Keystroke("backspace".to_string()),
                BackendEvent::Keystroke("backspace".to_string()),
                BackendEvent::Keystroke("backspace".to_string()),
                BackendEvent::Input(b"x".to_vec()),
            ],
            "expected snippet placeholder to be replaced via backspaces + input"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{format_clock, no_command_block_detail};
    use crate::command_blocks::CommandBlock;

    #[test]
    fn format_clock_displays_mm_ss_or_hh_mm_ss() {
        assert_eq!(format_clock(std::time::Duration::from_secs(0)), "00:00");
        assert_eq!(format_clock(std::time::Duration::from_secs(65)), "01:05");
        assert_eq!(
            format_clock(std::time::Duration::from_secs(3661)),
            "01:01:01"
        );
    }

    #[test]
    fn no_command_block_detail_mentions_supported_shells() {
        let detail = no_command_block_detail(&[], 42);
        assert!(detail.contains("bash or zsh"));
        assert!(detail.contains("OSC 133"));
    }

    #[test]
    fn no_command_block_detail_reports_last_block_context() {
        let blocks = vec![CommandBlock {
            id: 1,
            started_at: std::time::Instant::now(),
            ended_at: None,
            exit_code: None,
            command: None,
            output_start_line: 10,
            output_end_line: Some(20),
        }];

        let detail = no_command_block_detail(&blocks, 42);
        assert!(detail.contains("stable_row=42"));
        assert!(detail.contains("last_block.start=10"));
        assert!(detail.contains("last_block.end=Some(20)"));
    }

    #[test]
    fn command_block_debug_toasts_are_gated_by_debug_assertions() {
        let src = include_str!("mod.rs");
        let gate = "let debug_toasts = cfg!(debug_assertions);";
        assert!(
            src.contains(gate),
            "expected command block selection to gate debug toasts on debug assertions"
        );
    }
}
