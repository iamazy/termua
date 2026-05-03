use std::{collections::VecDeque, ops::Range, sync::Arc, time::Duration};

use gpui::{
    Action, AnyElement, App, Bounds, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement, IntoElement, KeyContext, KeyDownEvent, Keystroke, MouseButton,
    ParentElement, Pixels, PromptLevel, ReadGlobal, Styled, Subscription, Window, div, px,
};
use gpui_common::TermuaIcon;
use gpui_component::{
    ActiveTheme, Icon, IconName, WindowExt,
    menu::{PopupMenu, PopupMenuItem},
    notification::Notification,
};
use record::{RecordingMenuEntry, recording_context_menu_entry, recording_indicator_label};
use schemars::JsonSchema;
use scrolling::{SCROLLBAR_WIDTH, ScrollState, ScrollbarPreview};
use serde::Deserialize;
use smol::Timer;

use crate::{
    Copy, DecreaseFontSize, HoveredWord, IncreaseFontSize, ResetFontSize, TerminalContent,
    TerminalMode,
    element::ScrollbarPreviewTextElement,
    record::render_recording_indicator_label,
    settings::{CursorShape, TerminalBlink, TerminalSettings},
    snippet::{SnippetJump, SnippetJumpDir, SnippetSession},
    suggestions::{
        SuggestionEngine, SuggestionHistoryConfig, SuggestionItem, SuggestionStaticConfig,
    },
    terminal::{
        Clear, Event, Paste, SelectAll, ShowCharacterPalette, StartCastRecording,
        StopCastRecording, Terminal, TerminalBounds, ToggleCastRecording, UserInput,
    },
    view::search::{SearchState, render_search},
};

mod input;
pub(crate) mod line_number;
pub(crate) mod record;
mod render;
pub(crate) mod scrolling;
pub(crate) mod search;
mod suggestions;

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
    use std::{borrow::Cow, ops::RangeInclusive, rc::Rc};

    use gpui::{
        AppContext, Bounds, Context as GpuiContext, Entity, InteractiveElement, Keystroke,
        Modifiers, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels,
        ScrollWheelEvent, Styled, Window, div, point, px, size,
    };
    use gpui_component::Root;

    use super::{TerminalView, format_scrollbar_preview_line_number};
    use crate::{
        Cell, GridPoint, IndexedCell, TerminalBackend, TerminalContent, TerminalShutdownPolicy,
        TerminalType, settings::CursorShape, terminal::TerminalBounds,
    };

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

    use gpui::{AppContext, Entity};

    use super::{
        SuggestionItem, SuggestionsState, TerminalView, scrollbar_preview_tests::PreviewBackend,
    };
    use crate::{GridPoint, TerminalContent};

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
        ops::RangeInclusive,
        rc::Rc,
        sync::{Arc, Mutex},
    };

    use gpui::{
        AppContext, Bounds, Context, Entity, Keystroke, Modifiers, MouseDownEvent, MouseMoveEvent,
        MouseUpEvent, Pixels, ScrollWheelEvent, Window,
    };
    use gpui_component::Root;

    use super::{SuggestionItem, TerminalView};
    use crate::{
        GridPoint, TerminalBackend, TerminalBounds, TerminalContent, TerminalSettings,
        settings::CursorShape,
    };

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
        ops::RangeInclusive,
        sync::{Arc, Mutex},
    };

    use gpui::{
        AppContext, Bounds, Context, Entity, Keystroke, Modifiers, MouseDownEvent, MouseMoveEvent,
        MouseUpEvent, Pixels, ScrollWheelEvent, Window,
    };
    use gpui_component::Root;

    use super::TerminalView;
    use crate::{
        GridPoint, TerminalBackend, TerminalBounds, TerminalContent, TerminalSettings,
        settings::CursorShape, snippet::SnippetSession,
    };

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
    use std::time::Duration;

    use super::no_command_block_detail;
    use crate::command_blocks::CommandBlock;

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
