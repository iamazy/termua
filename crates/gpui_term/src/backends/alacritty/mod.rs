use std::{
    borrow::Cow,
    cmp,
    collections::{BTreeMap, HashMap, VecDeque},
    ops::RangeInclusive,
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};

use alacritty_terminal::{
    Term,
    event::{Event as AlacTermEvent, EventListener, Notify, OnResize, WindowSize},
    event_loop::{EventLoop, Msg, Notifier},
    grid::{Dimensions, Indexed as GridIndexed, Scroll as AlacScroll},
    index::{Column, Line, Point as AlacPoint, Side},
    selection::{Selection, SelectionType},
    sync::FairMutex,
    term::{
        Config, TermMode,
        cell::{Cell as AlacCell, Flags},
    },
    tty::{self, Options},
    vte::ansi::{
        ClearMode, Color as AlacColor, CursorShape as AlacCursorShape,
        CursorStyle as AlacCursorStyle, Handler, NamedColor as AlacNamedColor,
    },
};
use futures::{
    FutureExt, StreamExt,
    channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded},
};
use gpui::{
    Bounds, Context, Keystroke, Modifiers, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels,
    Point, ReadGlobal, ScrollWheelEvent, Window, px,
};
use parking_lot::Mutex;

#[cfg(unix)]
use self::record::RecordingLocalPty;
use self::{
    keys::to_esc_str,
    mouse::{
        alt_scroll, grid_point, grid_point_and_side, mouse_button_report, mouse_moved_report,
        scroll_report,
    },
};
use crate::{
    Cell, CellFlags, Cursor, CursorRenderShape, Event, GridPoint, NamedColor, PtySource,
    SelectionRange as ModelSelectionRange, SerialOptions, SshOptions, TermColor, Terminal,
    TerminalBackend, TerminalBounds, TerminalContent, TerminalMode, TerminalShutdownPolicy,
    TerminalType, backends,
    cast::{CastHeader, CastRecorderSender, CastRecorderState, start_cast_recorder},
    command_blocks::CommandBlockTracker,
    settings::{CursorShape, TerminalSettings},
};

mod keys;
mod mouse;
#[cfg(unix)]
mod record;
mod serial;
pub mod ssh;

fn local_pty_options_for_program_exists(
    env: std::collections::HashMap<String, String>,
    program_exists: impl FnOnce(&str) -> bool,
) -> Options {
    let shell_program = crate::shell::pick_shell_program_from_env(&env);
    let shell = shell_program.filter(|p| program_exists(p)).map(|p| {
        let args = crate::shell::shell_integration_args_for_env(p, &env);

        tty::Shell::new(p.to_string(), args)
    });

    Options {
        shell,
        env,
        ..Options::default()
    }
}

fn prev_stable_row(stable: i64) -> i64 {
    if stable > 0 { stable - 1 } else { 0 }
}

fn adjust_osc133_boundary(payload: &str, stable: i64, cursor_x: usize) -> i64 {
    // Keep consistent with the WezTerm backend heuristic.
    //
    // When the cursor is at column 0, it often means:
    // - For `C`: the user hit enter and the cursor moved to the next line, so the command line
    //   itself is the previous row.
    // - For `D`: the command finished and the cursor is on the next (prompt) line, so the output
    //   ends on the previous row.
    let kind = payload.trim_start().as_bytes().first().copied();
    if cursor_x == 0 && matches!(kind, Some(b'A') | Some(b'C') | Some(b'D')) {
        prev_stable_row(stable)
    } else {
        stable
    }
}

fn stable_row_texts(term: &Term<EventProxy>) -> Vec<(i64, String)> {
    let grid = term.grid();
    let last_col = grid.last_column();

    (grid.topmost_line().0..=grid.bottommost_line().0)
        .map(|line| {
            let stable = grid.stable_row_id_for_line(Line(line));
            let text = term
                .bounds_to_string(
                    AlacPoint::new(Line(line), Column(0)),
                    AlacPoint::new(Line(line), last_col),
                )
                .trim_end()
                .to_string();
            (stable, text)
        })
        .collect()
}

fn wrapped_command_span_for_stable_row(
    term: &Term<EventProxy>,
    stable_row: i64,
) -> Option<(i64, String)> {
    let grid = term.grid();
    let mut line = grid.line_for_stable_row_id(stable_row)?;
    let last_col = grid.last_column();

    while line.0 > grid.topmost_line().0 {
        let prev = Line(line.0 - 1);
        if !grid[prev][last_col].flags.contains(Flags::WRAPLINE) {
            break;
        }
        line = prev;
    }

    let mut command = String::new();
    let end = grid.line_for_stable_row_id(stable_row)?;
    for row in line.0..=end.0 {
        command.push_str(
            term.bounds_to_string(
                AlacPoint::new(Line(row), Column(0)),
                AlacPoint::new(Line(row), last_col),
            )
            .trim_end_matches(|c: char| c.is_whitespace()),
        );
    }

    let command = command.trim_end().to_string();
    (!command.trim().is_empty()).then(|| (grid.stable_row_id_for_line(line), command))
}

fn stable_row_for_grid_line(term: &Term<EventProxy>, line: i32) -> Option<i64> {
    let grid = term.grid();
    let top = grid.topmost_line().0;
    let bottom = grid.bottommost_line().0;
    if line < top || line > bottom {
        return None;
    }
    Some(grid.stable_row_id_for_line(Line(line)))
}

fn selection_from_lines(start_line: i32, end_line: i32, last_col: usize) -> Selection {
    let mut start = AlacPoint::new(Line(start_line), Column(0));
    let mut end = AlacPoint::new(Line(end_line), Column(last_col));
    if start.line.0 > end.line.0 || (start.line.0 == end.line.0 && start.column.0 > end.column.0) {
        std::mem::swap(&mut start, &mut end);
    }

    let mut selection = Selection::new(SelectionType::Simple, start, Side::Left);
    selection.update(end, Side::Right);
    selection
}

fn local_pty_options(env: std::collections::HashMap<String, String>) -> Options {
    local_pty_options_for_program_exists(env, crate::shell::program_exists_on_path)
}

/// A translation struct for Alacritty to communicate with us from their event loop.
#[derive(Clone)]
pub struct EventProxy(pub UnboundedSender<AlacTermEvent>);

impl EventListener for EventProxy {
    fn send_event(&self, event: AlacTermEvent) {
        self.0.unbounded_send(event).ok();
    }
}

#[derive(Clone)]
enum TermOp {
    Resize(TerminalBounds),
    Clear,
    Scroll(AlacScroll),
    SetSelection(Option<(Selection, AlacPoint)>),
    UpdateSelection(Point<Pixels>),
    Copy(Option<bool>),
    ToggleViMode,
}

#[derive(PartialEq, Eq)]
enum SelectionPhase {
    Selecting,
    Ended,
}

pub struct TerminalBuilder {
    backend: AlacrittyBackend,
    events_rx: UnboundedReceiver<AlacTermEvent>,
}

type SharedTermState = (
    Arc<FairMutex<Term<EventProxy>>>,
    Config,
    UnboundedSender<AlacTermEvent>,
    UnboundedReceiver<AlacTermEvent>,
    Arc<Mutex<Option<CastRecorderSender>>>,
);

impl TerminalBuilder {
    fn build_shared_term_state(
        cursor_shape: CursorShape,
        max_scroll_history_lines: Option<usize>,
    ) -> SharedTermState {
        let default_cursor_style = AlacCursorStyle {
            shape: map_cursor_shape(cursor_shape),
            blinking: false,
        };

        let scrolling_history = max_scroll_history_lines
            .unwrap_or(backends::DEFAULT_SCROLLBACK_LINES)
            .min(backends::MAX_SCROLLBACK_LINES);

        let config = Config {
            scrolling_history,
            default_cursor_style,
            ..Config::default()
        };

        let (events_tx, events_rx) = unbounded();

        let term = Term::new(
            config.clone(),
            &TerminalBounds::default(),
            EventProxy(events_tx.clone()),
        );
        let term = Arc::new(FairMutex::new(term));

        let cast_slot: Arc<Mutex<Option<CastRecorderSender>>> = Arc::new(Mutex::new(None));

        (term, config, events_tx, events_rx, cast_slot)
    }

    #[allow(clippy::too_many_arguments)]
    fn build_local(
        term: Arc<FairMutex<Term<EventProxy>>>,
        config: Config,
        events_tx: UnboundedSender<AlacTermEvent>,
        events_rx: UnboundedReceiver<AlacTermEvent>,
        cast_slot: Arc<Mutex<Option<CastRecorderSender>>>,
        env: HashMap<String, String>,
        window_id: u64,
    ) -> anyhow::Result<Self> {
        // env.entry("LANG".to_string())
        //     .or_insert_with(|| "en_US.UTF-8".to_string());
        // env.entry("TERM".to_string())
        //     .or_insert_with(|| "xterm-256color".to_string());
        // env.entry("COLORTERM".to_string())
        //     .or_insert_with(|| "truecolor".to_string());

        let pty_options = local_pty_options(env);
        #[cfg(target_os = "windows")]
        let pty = {
            let cast_slot_for_output = Arc::clone(&cast_slot);
            let on_output = Some(Arc::new(move |bytes: &[u8]| {
                if let Some(sender) = cast_slot_for_output.lock().as_ref() {
                    sender.output(bytes);
                }
            }) as alacritty_terminal::tty::windows::IoTap);

            let cast_slot_for_input = Arc::clone(&cast_slot);
            let on_input = Some(Arc::new(move |bytes: &[u8]| {
                if let Some(sender) = cast_slot_for_input.lock().as_ref() {
                    sender.input(bytes);
                }
            }) as alacritty_terminal::tty::windows::IoTap);

            tty::windows::new_with_taps(
                &pty_options,
                TerminalBounds::default().into(),
                window_id,
                on_output,
                on_input,
            )?
        };

        #[cfg(not(target_os = "windows"))]
        let pty = tty::new(&pty_options, TerminalBounds::default().into(), window_id)?;
        #[cfg(unix)]
        let pty = RecordingLocalPty::new(pty, Arc::clone(&cast_slot))?;

        Self::build_with_pty(
            term,
            config,
            events_tx,
            events_rx,
            None,
            cast_slot,
            pty,
            pty_options.drain_on_exit,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn build_ssh(
        term: Arc<FairMutex<Term<EventProxy>>>,
        config: Config,
        events_tx: UnboundedSender<AlacTermEvent>,
        events_rx: UnboundedReceiver<AlacTermEvent>,
        cast_slot: Arc<Mutex<Option<CastRecorderSender>>>,
        env: HashMap<String, String>,
        opts: SshOptions,
    ) -> anyhow::Result<Self> {
        // SSH PTY encapsulates its own remote spawning; no `tty::Options`.
        let (pty, sftp) = ssh::Pty::new(env, opts, Arc::clone(&cast_slot))?;
        Self::build_with_pty(
            term,
            config,
            events_tx,
            events_rx,
            Some(sftp),
            cast_slot,
            pty,
            false,
        )
    }

    fn build_serial(
        term: Arc<FairMutex<Term<EventProxy>>>,
        config: Config,
        events_tx: UnboundedSender<AlacTermEvent>,
        events_rx: UnboundedReceiver<AlacTermEvent>,
        cast_slot: Arc<Mutex<Option<CastRecorderSender>>>,
        opts: SerialOptions,
    ) -> anyhow::Result<Self> {
        let pty = serial::Pty::new(opts, Arc::clone(&cast_slot))?;
        Self::build_with_pty(
            term, config, events_tx, events_rx, None, cast_slot, pty, false,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn build_with_pty<T>(
        term: Arc<FairMutex<Term<EventProxy>>>,
        term_config: Config,
        events_tx: UnboundedSender<AlacTermEvent>,
        events_rx: UnboundedReceiver<AlacTermEvent>,
        sftp: Option<wezterm_ssh::Sftp>,
        cast_slot: Arc<Mutex<Option<CastRecorderSender>>>,
        pty: T,
        drain_on_exit: bool,
    ) -> anyhow::Result<Self>
    where
        T: tty::EventedPty + OnResize + Send + 'static,
    {
        let event_loop = EventLoop::new(
            term.clone(),
            EventProxy(events_tx),
            pty,
            drain_on_exit,
            false,
        )?;

        let pty_tx = event_loop.channel();
        let _io_thread = event_loop.spawn();

        Ok(Self {
            backend: AlacrittyBackend {
                pty_tx: Notifier(pty_tx),
                term,
                term_config,
                pending_ops: VecDeque::with_capacity(16),
                term_mode: TermMode::empty(),
                search: SearchState::default(),
                selection: SelectionState::default(),
                scroll_px: px(0.0),
                exited: false,
                content: TerminalContent::default(),
                sftp,
                record: RecordState::new(cast_slot),
                blocks: CommandBlockTracker::new(200),
            },
            events_rx,
        })
    }

    pub fn new(
        pty_source: PtySource,
        cursor_shape: CursorShape,
        max_scroll_history_lines: Option<usize>,
    ) -> anyhow::Result<Self> {
        let (term, config, events_tx, events_rx, cast_slot) =
            Self::build_shared_term_state(cursor_shape, max_scroll_history_lines);

        match pty_source {
            PtySource::Local { env, window_id } => Self::build_local(
                term, config, events_tx, events_rx, cast_slot, env, window_id,
            ),
            PtySource::Ssh { env, opts } => {
                Self::build_ssh(term, config, events_tx, events_rx, cast_slot, env, opts)
            }
            PtySource::Serial { opts } => {
                Self::build_serial(term, config, events_tx, events_rx, cast_slot, opts)
            }
        }
    }

    pub fn subscribe(self, cx: &Context<Terminal>) -> Terminal {
        let terminal = Terminal::new(TerminalType::Alacritty, Box::new(self.backend));
        let mut events_rx = self.events_rx;

        cx.spawn(async move |terminal, cx| {
            let mut batch: Vec<AlacTermEvent> = Vec::with_capacity(128);

            loop {
                let Some(first) = events_rx.next().await else {
                    break;
                };
                batch.clear();
                batch.push(first);

                let mut timer = cx
                    .background_executor()
                    .timer(Duration::from_millis(4))
                    .fuse();

                loop {
                    futures::select_biased! {
                        _ = timer => break,
                        event = events_rx.next().fuse() => {
                            match event {
                                Some(ev) => {
                                    batch.push(ev);
                                    if batch.len() >= 100 {
                                        break;
                                    }
                                }
                                None => break,
                            }
                        },
                    }
                }

                terminal.update(cx, |this, cx| {
                    for ev in batch.drain(..) {
                        this.dispatch_backend_event(Box::new(ev), cx);
                    }
                    cx.notify();
                })?;

                smol::future::yield_now().await;
            }

            anyhow::Ok(())
        })
        .detach();

        terminal
    }
}

struct SelectionState {
    head: Option<AlacPoint>,
    phase: SelectionPhase,
    command_block_id: Option<u64>,
}

impl Default for SelectionState {
    fn default() -> Self {
        Self {
            head: None,
            phase: SelectionPhase::Ended,
            command_block_id: None,
        }
    }
}

#[derive(Default)]
struct SearchState {
    matches: Vec<RangeInclusive<GridPoint>>,
    active_match: Option<usize>,
    query: Option<String>,
    dirty: bool,
}

struct RecordState {
    slot: Arc<Mutex<Option<CastRecorderSender>>>,
    sender: Option<CastRecorderSender>,
    state: Option<CastRecorderState>,
}

impl RecordState {
    fn new(slot: Arc<Mutex<Option<CastRecorderSender>>>) -> Self {
        Self {
            slot,
            sender: None,
            state: None,
        }
    }
}

pub struct AlacrittyBackend {
    pty_tx: Notifier,
    term: Arc<FairMutex<Term<EventProxy>>>,
    term_config: Config,

    pending_ops: VecDeque<TermOp>,
    term_mode: TermMode,

    search: SearchState,

    selection: SelectionState,
    scroll_px: Pixels,

    exited: bool,

    content: TerminalContent,
    // SFTP support for SSH terminals.
    sftp: Option<wezterm_ssh::Sftp>,

    // Asciinema cast recording.
    record: RecordState,

    blocks: CommandBlockTracker,
}

impl AlacrittyBackend {
    fn write_to_pty(&self, input: impl Into<Cow<'static, [u8]>>) {
        self.pty_tx.notify(input.into());
    }

    fn queue_scroll_delta(&mut self, delta: i32) {
        if delta == 0 {
            return;
        }
        match self.pending_ops.back_mut() {
            Some(TermOp::Scroll(AlacScroll::Delta(prev))) => {
                *prev += delta;
            }
            _ => self
                .pending_ops
                .push_back(TermOp::Scroll(AlacScroll::Delta(delta))),
        }
    }

    fn compute_search_matches(
        term: &Term<EventProxy>,
        query: &str,
    ) -> Vec<RangeInclusive<GridPoint>> {
        let grid = term.grid();
        let cols = grid.columns();
        if cols == 0 {
            return Vec::new();
        }

        let top = grid.topmost_line().0;
        let bottom = grid.bottommost_line().0;
        if top > bottom {
            return Vec::new();
        }

        crate::backends::search::collect_search_matches_for_lines(
            query,
            top..=bottom,
            |line_i32, tokens| {
                // Build a token stream of *visible* glyphs (skip spacer columns for wide
                // characters) so that matching works for CJK/emoji, where
                // consecutive glyphs may be separated by spacer columns in grid
                // space.
                for col in 0..cols {
                    let p = AlacPoint::new(Line(line_i32), Column(col));
                    let cell = &grid[p];
                    if cell
                        .flags
                        .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
                    {
                        continue;
                    }

                    let mut text = String::new();
                    text.push(cell.c);
                    if let Some(zero) = cell.zerowidth() {
                        text.extend(zero.iter());
                    }

                    crate::backends::search::push_search_token(
                        tokens,
                        col,
                        if cell.flags.contains(Flags::WIDE_CHAR) {
                            2
                        } else {
                            1
                        },
                        cols,
                        text,
                    );
                }

                Some(line_i32)
            },
        )
    }

    fn process_alacritty_event(&mut self, event: AlacTermEvent, cx: &mut Context<Terminal>) {
        match event {
            AlacTermEvent::ClipboardStore(_, data) => {
                crate::terminal::write_clipboard(cx, data);
            }
            AlacTermEvent::ClipboardLoad(_, format) => self.write_to_pty(
                match &cx.read_from_clipboard().and_then(|item| item.text()) {
                    Some(text) => format(text),
                    _ => format(""),
                }
                .into_bytes(),
            ),
            AlacTermEvent::PtyWrite(out) => self.write_to_pty(out.into_bytes()),
            AlacTermEvent::TextAreaSizeRequest(format) => {
                self.write_to_pty(format(self.content.terminal_bounds.into()).into_bytes())
            }
            AlacTermEvent::CursorBlinkingChange => {
                let terminal = self.term.lock();
                let blinking = terminal.cursor_style().blinking;
                cx.emit(Event::BlinkChanged(blinking));
            }
            AlacTermEvent::Wakeup => {
                self.search.dirty = true;
            }
            AlacTermEvent::Osc133 {
                payload,
                stable_row,
                cursor_col,
            } => {
                let mut line = adjust_osc133_boundary(&payload, stable_row, cursor_col);
                let command = if payload.trim_start().starts_with('C') {
                    let term = self.term.lock_unfair();
                    match wrapped_command_span_for_stable_row(&term, line) {
                        Some((start_line, command)) => {
                            line = start_line;
                            Some(command)
                        }
                        None => {
                            let grid = term.grid();
                            grid.line_for_stable_row_id(line).map(|row| {
                                term.bounds_to_string(
                                    AlacPoint::new(row, Column(0)),
                                    AlacPoint::new(row, grid.last_column()),
                                )
                                .trim_end()
                                .to_string()
                            })
                        }
                    }
                } else {
                    None
                };
                self.blocks
                    .apply_osc133(&payload, Instant::now(), line, command);
            }
            AlacTermEvent::Bell => cx.emit(Event::Bell),
            AlacTermEvent::Exit => {
                self.exited = true;
                cx.emit(Event::CloseTerminal);
            }
            AlacTermEvent::Title(_title) => {
                cx.emit(Event::TitleChanged);
            }
            _ => {}
        }
    }

    fn refresh_hovered_word(&mut self, window: &Window, cx: &mut Context<Terminal>) {
        self.update_hover_target(window.mouse_position(), cx);
    }

    fn update_hover_target(&mut self, window_pos: Point<Pixels>, cx: &mut Context<Terminal>) {
        let hovered = self.url_from_position(window_pos);

        if let Some(target) =
            backends::update_hovered_word(&mut self.content.last_hovered_word, hovered)
        {
            cx.emit(Event::NewNavigationTarget(target));
        }
    }

    fn url_from_position(
        &self,
        window_pos: Point<Pixels>,
    ) -> Option<(String, RangeInclusive<GridPoint>)> {
        if !self.content.terminal_bounds.bounds.contains(&window_pos) {
            return None;
        }

        let local = window_pos - self.content.terminal_bounds.bounds.origin;
        let (point, _side) = grid_point_and_side(
            local,
            self.content.terminal_bounds,
            self.content.display_offset,
        );

        let term = self.term.lock_unfair();
        let grid = term.grid();

        if point.line < grid.topmost_line() || point.line > grid.bottommost_line() {
            return None;
        }

        let cols = grid.columns();
        if cols == 0 {
            return None;
        }

        let cell_chars = backends::collect_line_chars(
            cols,
            (0..cols).filter_map(|col| {
                let p = AlacPoint::new(point.line, Column(col));
                let cell = &grid[p];
                (!cell.flags.contains(Flags::WIDE_CHAR_SPACER)).then_some((col, cell.c))
            }),
        );

        let hover_col = point.column.0.min(cols - 1);
        backends::hovered_url_from_line_chars(&cell_chars, hover_col, point.line.0)
    }

    fn apply_pending_ops(&mut self, cx: &mut Context<Terminal>) -> bool {
        let mut hover_dirty = false;

        let term_mutex = Arc::clone(&self.term);
        let mut term = term_mutex.lock_unfair();
        while let Some(op) = self.pending_ops.pop_front() {
            self.apply_term_op(op, &mut term, cx, &mut hover_dirty);
        }

        hover_dirty
    }

    fn apply_term_op(
        &mut self,
        op: TermOp,
        term: &mut Term<EventProxy>,
        cx: &mut Context<Terminal>,
        hover_dirty: &mut bool,
    ) {
        match op {
            TermOp::Resize(new_bounds) => self.apply_resize_op(term, new_bounds),
            TermOp::Clear => Self::apply_clear_op(term, cx),
            TermOp::Scroll(scroll) => {
                term.scroll_display(scroll);
                *hover_dirty = true;
            }
            TermOp::SetSelection(selection) => {
                term.selection = selection.as_ref().map(|(sel, _)| sel.clone());
                cx.emit(Event::SelectionsChanged);
            }
            TermOp::UpdateSelection(position) => {
                self.apply_update_selection_op(term, position, cx);
            }
            TermOp::Copy(_keep_selection) => {
                if let Some(txt) = term.selection_to_string() {
                    crate::terminal::write_clipboard(cx, txt);
                }
            }
            TermOp::ToggleViMode => term.toggle_vi_mode(),
        }
    }

    fn apply_resize_op(&mut self, term: &mut Term<EventProxy>, mut new_bounds: TerminalBounds) {
        new_bounds.bounds.size.height = cmp::max(new_bounds.line_height, new_bounds.height());
        new_bounds.bounds.size.width = cmp::max(new_bounds.cell_width, new_bounds.width());

        self.content.terminal_bounds = new_bounds;
        if let Some(sender) = self.record.sender.as_ref() {
            sender.resize(
                new_bounds.num_columns().max(1),
                new_bounds.num_lines().max(1),
            );
        }
        self.pty_tx.0.send(Msg::Resize(new_bounds.into())).ok();
        term.resize(new_bounds);
        let cursor_stable = term
            .grid()
            .stable_row_id_for_line(term.grid().cursor.point.line);
        let lines = stable_row_texts(term);
        self.blocks.remap_after_rewrap(&lines, cursor_stable);
        if let Some(block_id) = self.selection.command_block_id
            && let Some((start_stable, end_stable)) = self.blocks.range_for_block_id(block_id)
            && let Some(start_line) = term.grid().line_for_stable_row_id(start_stable)
            && let Some(end_line) = term.grid().line_for_stable_row_id(end_stable)
        {
            let last_col = new_bounds.num_columns().saturating_sub(1);
            term.selection = Some(selection_from_lines(start_line.0, end_line.0, last_col));
            self.selection.head = None;
            self.selection.phase = SelectionPhase::Ended;
        }
    }

    fn apply_clear_op(term: &mut Term<EventProxy>, cx: &mut Context<Terminal>) {
        // Clear back buffer
        term.clear_screen(ClearMode::Saved);
        let cursor = term.grid().cursor.point;
        // Clear the lines above
        term.grid_mut().reset_region(..cursor.line);
        // Copy the current line up
        let line = term.grid()[cursor.line][..Column(term.grid().columns())]
            .iter()
            .cloned()
            .enumerate()
            .collect::<Vec<(usize, AlacCell)>>();
        for (i, cell) in line {
            term.grid_mut()[Line(0)][Column(i)] = cell;
        }
        // Reset the cursor
        term.grid_mut().cursor.point = AlacPoint::new(Line(0), term.grid_mut().cursor.point.column);
        let new_cursor = term.grid().cursor.point;

        // Clear the lines below the new cursor
        if (new_cursor.line.0 as usize) < term.screen_lines() - 1 {
            term.grid_mut().reset_region((new_cursor.line + 1)..);
        }
        cx.emit(Event::SelectionsChanged);
        cx.emit(Event::Wakeup);
    }

    fn apply_update_selection_op(
        &mut self,
        term: &mut Term<EventProxy>,
        position: Point<Pixels>,
        cx: &mut Context<Terminal>,
    ) {
        if let Some(mut selection) = term.selection.take() {
            let (point, side) = grid_point_and_side(
                position,
                self.content.terminal_bounds,
                term.grid().display_offset(),
            );
            selection.update(point, side);
            term.selection = Some(selection);
            self.selection.head = Some(point);
            self.selection.command_block_id = None;
            cx.emit(Event::SelectionsChanged);
        }
    }

    fn sync_search_matches(&mut self) {
        let search = &mut self.search;
        backends::sync_search_matches(
            &mut search.dirty,
            search.query.as_deref(),
            &mut search.matches,
            &mut search.active_match,
            |q| {
                let term = self.term.lock_unfair();
                let matches = Self::compute_search_matches(&term, q);
                drop(term);
                matches
            },
        );
    }

    fn rebuild_snapshot(&mut self) {
        let term = self.term.lock_unfair();
        let content = term.renderable_content();
        self.term_mode = content.mode;

        let cells: Vec<GridIndexed<AlacCell>> = content
            .display_iter
            .map(|ic| GridIndexed {
                point: ic.point,
                cell: ic.cell.clone(),
            })
            .collect();

        let out = TerminalContent {
            terminal_bounds: self.content.terminal_bounds,
            display_offset: content.display_offset,
            mode: map_mode(content.mode),
            selection_text: content
                .selection
                .is_some()
                .then(|| term.selection_to_string())
                .flatten(),
            selection: content.selection.as_ref().map(|sel| ModelSelectionRange {
                start: map_point(sel.start),
                end: map_point(sel.end),
            }),
            cursor: Cursor {
                shape: map_cursor_render_shape(content.cursor.shape),
                point: map_point(content.cursor.point),
            },
            cursor_char: term.grid()[content.cursor.point].c,
            scrolled_to_top: content.display_offset == term.grid().history_size(),
            scrolled_to_bottom: content.display_offset == 0,
            last_hovered_word: self.content.last_hovered_word.clone(),
            cells: cells
                .into_iter()
                .map(|ic| crate::IndexedCell {
                    point: map_point(ic.point),
                    cell: map_cell(ic.cell),
                })
                .collect(),
        };

        self.content = out;
    }
}

impl TerminalBackend for AlacrittyBackend {
    fn backend_name(&self) -> &'static str {
        "alacritty"
    }

    fn sftp(&self) -> Option<wezterm_ssh::Sftp> {
        self.sftp.clone()
    }

    fn handle_backend_event(
        &mut self,
        event: Box<dyn std::any::Any + Send>,
        cx: &mut Context<Terminal>,
    ) {
        if let Ok(ev) = event.downcast::<AlacTermEvent>() {
            self.process_alacritty_event(*ev, cx);
        }
    }

    fn shutdown(&mut self, _policy: TerminalShutdownPolicy, _cx: &mut Context<Terminal>) {
        // Best-effort immediate shutdown of the Alacritty event loop/PTY.
        log::debug!("gpui_term[alacritty]: shutdown requested; sending Msg::Shutdown");
        let _ = self.pty_tx.0.send(Msg::Shutdown);
    }

    fn sync(&mut self, window: &mut Window, cx: &mut Context<Terminal>) {
        let hover_dirty = self.apply_pending_ops(cx);
        if hover_dirty {
            // Hover state is owned by the backend, so update it outside the term lock.
            self.refresh_hovered_word(window, cx);
        }

        self.sync_search_matches();
        self.rebuild_snapshot();
    }

    fn last_content(&self) -> &TerminalContent {
        &self.content
    }

    fn matches(&self) -> &[RangeInclusive<GridPoint>] {
        &self.search.matches
    }

    fn last_clicked_line(&self) -> Option<i32> {
        None
    }

    fn has_exited(&self) -> bool {
        self.exited
    }

    fn vi_mode_enabled(&self) -> bool {
        false
    }

    fn mouse_mode(&self, shift: bool) -> bool {
        self.term_mode.intersects(TermMode::MOUSE_MODE) && !shift
    }

    fn selection_started(&self) -> bool {
        self.selection.phase == SelectionPhase::Selecting
    }

    fn clear_selection(&mut self) {
        self.selection.head = None;
        self.selection.phase = SelectionPhase::Ended;
        self.selection.command_block_id = None;
        self.pending_ops.push_back(TermOp::SetSelection(None));
    }

    fn set_cursor_shape(&mut self, cursor_shape: CursorShape) {
        self.term_config.default_cursor_style = AlacCursorStyle {
            shape: map_cursor_shape(cursor_shape),
            blinking: false,
        };
        self.term.lock().set_options(self.term_config.clone());
    }

    fn total_lines(&self) -> usize {
        let term = self.term.lock_unfair();
        term.grid().total_lines()
    }

    fn viewport_lines(&self) -> usize {
        let term = self.term.lock_unfair();
        term.grid().screen_lines()
    }

    fn logical_line_numbers_from_top(&self, start_line: usize, count: usize) -> Vec<Option<usize>> {
        if count == 0 {
            return Vec::new();
        }

        let term = self.term.lock_unfair();
        let grid = term.grid();
        let total = grid.total_lines();
        if total == 0 || grid.columns() == 0 {
            return Vec::new();
        }

        let start = start_line.min(total);
        let end = start.saturating_add(count).min(total);
        if start == end {
            return Vec::new();
        }

        let top = grid.topmost_line();
        let last_col = grid.last_column();

        crate::terminal::logical_line_numbers_from_wraps(total, start, count, |row| {
            let line = top + row;
            let cell = &grid[AlacPoint::new(line, last_col)];
            cell.flags.contains(Flags::WRAPLINE)
        })
    }

    fn active_match_index(&self) -> Option<usize> {
        self.search.active_match
    }

    fn activate_match(&mut self, index: usize) {
        if index < self.search.matches.len() {
            self.search.active_match = Some(index);
        } else {
            self.search.active_match = None;
        }
    }

    fn select_matches(&mut self, matches: &[RangeInclusive<GridPoint>]) {
        self.search.matches.clear();
        self.search.matches.extend_from_slice(matches);
        self.search.active_match = (!self.search.matches.is_empty()).then_some(0);
    }

    fn set_search_query(&mut self, query: Option<String>) {
        let q = query.and_then(|s| {
            let trimmed = s.trim().to_string();
            (!trimmed.is_empty()).then_some(trimmed)
        });
        self.search.query = q;
        self.search.dirty = true;

        if let Some(q) = self.search.query.as_deref() {
            let term = self.term.lock_unfair();
            let new_matches = Self::compute_search_matches(&term, q);
            drop(term);
            self.search.matches = new_matches;
            self.search.active_match = (!self.search.matches.is_empty()).then_some(0);
            self.search.dirty = false;
        } else {
            self.search.matches.clear();
            self.search.active_match = None;
            self.search.dirty = false;
        }
    }

    fn select_all(&mut self) {
        let term = self.term.lock_unfair();
        let grid = term.grid();
        let start = AlacPoint::new(grid.topmost_line(), Column(0));
        let end = AlacPoint::new(grid.bottommost_line(), grid.last_column());
        drop(term);
        let mut selection = Selection::new(SelectionType::Simple, start, Side::Left);
        selection.update(end, Side::Right);
        self.pending_ops
            .push_back(TermOp::SetSelection(Some((selection, end))));
    }

    fn copy(&mut self, keep_selection: Option<bool>, _cx: &mut Context<Terminal>) {
        self.pending_ops.push_back(TermOp::Copy(keep_selection));
    }

    fn clear(&mut self) {
        self.pending_ops.push_back(TermOp::Clear)
    }

    fn scroll_line_up(&mut self) {
        self.queue_scroll_delta(1);
    }

    fn scroll_up_by(&mut self, lines: usize) {
        self.queue_scroll_delta(lines as i32);
    }

    fn scroll_line_down(&mut self) {
        self.queue_scroll_delta(-1);
    }

    fn scroll_down_by(&mut self, lines: usize) {
        self.queue_scroll_delta(-(lines as i32));
    }

    fn scroll_page_up(&mut self) {
        self.pending_ops
            .push_back(TermOp::Scroll(AlacScroll::PageUp));
    }

    fn scroll_page_down(&mut self) {
        self.pending_ops
            .push_back(TermOp::Scroll(AlacScroll::PageDown));
    }

    fn scroll_to_top(&mut self) {
        self.pending_ops.push_back(TermOp::Scroll(AlacScroll::Top));
    }

    fn scroll_to_bottom(&mut self) {
        self.pending_ops
            .push_back(TermOp::Scroll(AlacScroll::Bottom));
    }

    fn scrolled_to_top(&self) -> bool {
        self.content.scrolled_to_top
    }

    fn scrolled_to_bottom(&self) -> bool {
        self.content.scrolled_to_bottom
    }

    fn set_size(&mut self, new_bounds: TerminalBounds) {
        if self.content.terminal_bounds != new_bounds {
            self.pending_ops.push_back(TermOp::Resize(new_bounds))
        }
    }

    fn input(&mut self, input: Cow<'static, [u8]>) {
        self.pending_ops
            .push_back(TermOp::Scroll(AlacScroll::Bottom));
        self.pending_ops.push_back(TermOp::SetSelection(None));
        self.write_to_pty(input);
    }

    fn paste(&mut self, text: &str) {
        let paste_text = if self.term_mode.contains(TermMode::BRACKETED_PASTE) {
            format!("{}{}{}", "\x1b[200~", text.replace('\x1b', ""), "\x1b[201~")
        } else {
            text.replace("\r\n", "\r").replace('\n', "\r")
        };
        self.input(paste_text.into_bytes().into());
    }

    fn tail_text(&self, max_lines: usize) -> Option<String> {
        let max_lines = max_lines.max(1);
        let term = self.term.lock_unfair();
        let grid = term.grid();

        let total_lines = grid.total_lines();
        if total_lines == 0 {
            return None;
        }

        let cols = grid.columns();
        if cols == 0 {
            return None;
        }

        let max_lines = max_lines.min(total_lines);
        let bottom = grid.bottommost_line();
        let top = grid.topmost_line();
        let start_line_i32 = (bottom.0 - (max_lines as i32 - 1)).max(top.0);
        let start = AlacPoint::new(Line(start_line_i32), Column(0));
        let end = AlacPoint::new(bottom, grid.last_column());

        Some(term.bounds_to_string(start, end).trim_end().to_string())
    }

    fn text_for_lines(&self, start_line: i64, end_line: i64) -> Option<String> {
        let term = self.term.lock_unfair();
        let grid = term.grid();

        let total_lines = grid.total_lines();
        if total_lines == 0 {
            return None;
        }

        let cols = grid.columns();
        if cols == 0 {
            return None;
        }

        let top = grid.topmost_line().0 as i64;
        let bottom = grid.bottommost_line().0 as i64;

        let mut start = start_line.min(end_line).clamp(top, bottom);
        let mut end = start_line.max(end_line).clamp(top, bottom);
        if start > end {
            std::mem::swap(&mut start, &mut end);
        }

        let start = AlacPoint::new(Line(start as i32), Column(0));
        let end = AlacPoint::new(Line(end as i32), grid.last_column());
        Some(term.bounds_to_string(start, end).trim_end().to_string())
    }

    fn command_blocks(&self) -> Option<Vec<crate::command_blocks::CommandBlock>> {
        Some(self.blocks.blocks())
    }

    fn stable_row_for_grid_line(&self, line: i32) -> Option<i64> {
        let term = self.term.lock_unfair();
        let grid = term.grid();
        let top = grid.topmost_line().0;
        let bottom = grid.bottommost_line().0;
        if line < top || line > bottom {
            return None;
        }
        Some(grid.stable_row_id_for_line(Line(line)))
    }

    fn grid_line_for_stable_row(&self, stable_row: i64) -> Option<i32> {
        let term = self.term.lock_unfair();
        let grid = term.grid();
        grid.line_for_stable_row_id(stable_row).map(|l| l.0)
    }

    fn set_selection_range(&mut self, range: Option<crate::SelectionRange>) {
        self.selection.head = None;
        self.selection.phase = SelectionPhase::Ended;
        self.selection.command_block_id = range.as_ref().and_then(|range| {
            let term = self.term.lock_unfair();
            let start = stable_row_for_grid_line(&term, range.start.line)?;
            let end = stable_row_for_grid_line(&term, range.end.line)?;
            self.blocks.block_id_for_range(start, end)
        });

        let Some(range) = range else {
            self.pending_ops.push_back(TermOp::SetSelection(None));
            return;
        };

        let selection = selection_from_lines(range.start.line, range.end.line, range.end.column);
        self.pending_ops.push_back(TermOp::SetSelection(Some((
            selection,
            AlacPoint::new(Line(range.end.line), Column(range.end.column)),
        ))));
    }

    fn cast_recording_active(&self) -> bool {
        self.record.state.is_some()
    }

    fn start_cast_recording(&mut self, opts: crate::CastRecordingOptions) -> gpui::Result<()> {
        if self.record.state.is_some() {
            self.stop_cast_recording();
        }

        if let Some(parent) = opts.path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let cols = self.content.terminal_bounds.num_columns().max(1);
        let rows = self.content.terminal_bounds.num_lines().max(1);
        let ts = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let header = CastHeader {
            width: cols,
            height: rows,
            timestamp: ts,
            env: BTreeMap::new(),
        };

        let (sender, state) = start_cast_recorder(opts.path, header, opts.include_input)?;
        *self.record.slot.lock() = Some(sender.clone());
        self.record.sender = Some(sender);
        self.record.state = Some(state);
        Ok(())
    }

    fn stop_cast_recording(&mut self) {
        *self.record.slot.lock() = None;
        self.record.sender = None;
        if let Some(state) = self.record.state.take() {
            let _ = state.stop_and_join();
        }
    }

    fn focus_in(&self) {
        if self.term_mode.contains(TermMode::FOCUS_IN_OUT) {
            self.write_to_pty("\x1b[I".as_bytes());
        }
    }

    fn focus_out(&mut self) {
        if self.term_mode.contains(TermMode::FOCUS_IN_OUT) {
            self.write_to_pty("\x1b[O".as_bytes());
        }
    }

    fn toggle_vi_mode(&mut self) {
        self.pending_ops.push_back(TermOp::ToggleViMode);
    }

    fn try_keystroke(&mut self, keystroke: &Keystroke, alt_is_meta: bool) -> bool {
        let esc = to_esc_str(keystroke, &self.term_mode, alt_is_meta);
        if let Some(esc) = esc {
            match esc {
                Cow::Borrowed(string) => self.input(string.as_bytes().into()),
                Cow::Owned(string) => self.input(string.into_bytes().into()),
            };
            true
        } else {
            false
        }
    }

    fn try_modifiers_change(
        &mut self,
        modifiers: &Modifiers,
        window: &Window,
        cx: &mut Context<Terminal>,
    ) {
        if self
            .content
            .terminal_bounds
            .bounds
            .contains(&window.mouse_position())
            && modifiers.secondary()
        {
            self.refresh_hovered_word(window, cx);
        } else if !modifiers.secondary()
            && let Some(target) =
                backends::update_hovered_word(&mut self.content.last_hovered_word, None)
        {
            cx.emit(Event::NewNavigationTarget(target));
        }
        cx.notify();
    }

    fn mouse_move(&mut self, e: &MouseMoveEvent, cx: &mut Context<Terminal>) {
        let position = e.position - self.content.terminal_bounds.bounds.origin;
        if self.mouse_mode(e.modifiers.shift) {
            let (point, _side) = grid_point_and_side(
                position,
                self.content.terminal_bounds,
                self.content.display_offset,
            );
            if let Some(bytes) =
                mouse_moved_report(point, e.pressed_button, e.modifiers, self.term_mode)
            {
                self.pty_tx.notify(bytes);
            }
        } else if e.modifiers.secondary() {
            self.update_hover_target(e.position, cx);
        } else if let Some(target) =
            backends::update_hovered_word(&mut self.content.last_hovered_word, None)
        {
            cx.emit(Event::NewNavigationTarget(target));
        }
    }

    fn select_word_at_event_position(&mut self, e: &MouseDownEvent) {
        let position = e.position - self.content.terminal_bounds.bounds.origin;
        let (point, side) = grid_point_and_side(
            position,
            self.content.terminal_bounds,
            self.content.display_offset,
        );
        // Typical terminal behavior:
        // - Double-click selects a word
        // - Triple-click selects the current line
        let ty = if e.click_count >= 3 {
            SelectionType::Lines
        } else {
            SelectionType::Semantic
        };
        let selection = Selection::new(ty, point, side);
        self.selection.command_block_id = None;
        self.pending_ops
            .push_back(TermOp::SetSelection(Some((selection, point))));
    }

    fn mouse_drag(
        &mut self,
        e: &MouseMoveEvent,
        region: Bounds<Pixels>,
        cx: &mut Context<Terminal>,
    ) {
        let position = e.position - self.content.terminal_bounds.bounds.origin;
        if !self.mouse_mode(e.modifiers.shift) {
            self.selection.phase = SelectionPhase::Selecting;
            self.selection.command_block_id = None;
            self.pending_ops
                .push_back(TermOp::UpdateSelection(position));

            if !self.term_mode.contains(TermMode::ALT_SCREEN)
                && let Some(scroll_lines) = backends::drag_line_delta(
                    e.position,
                    region,
                    self.content.terminal_bounds.line_height,
                )
            {
                self.queue_scroll_delta(scroll_lines);
            }
            cx.notify();
        }
    }

    fn mouse_down(&mut self, e: &MouseDownEvent, cx: &mut Context<Terminal>) {
        if e.button == gpui::MouseButton::Left && e.modifiers.secondary() {
            // Ctrl+click (or the platform's "secondary" modifier) opens a hovered URL.
            self.update_hover_target(e.position, cx);
            if let Some(hovered) = self.content.last_hovered_word.as_ref() {
                cx.emit(Event::Open(hovered.word.clone()));
                return;
            }
        }

        if e.modifiers.secondary() {
            return;
        }

        let position = e.position - self.content.terminal_bounds.bounds.origin;
        let point = grid_point(
            position,
            self.content.terminal_bounds,
            self.content.display_offset,
        );

        if self.mouse_mode(e.modifiers.shift) {
            if let Some(bytes) =
                mouse_button_report(point, e.button, e.modifiers, true, self.term_mode)
            {
                self.pty_tx.notify(bytes);
            }
            return;
        }

        if e.button == gpui::MouseButton::Left {
            if e.click_count >= 2 {
                // Double-click selects a word; triple-click selects the current line.
                self.select_word_at_event_position(e);
                cx.notify();
                return;
            }

            let (point, side) = grid_point_and_side(
                position,
                self.content.terminal_bounds,
                self.content.display_offset,
            );
            let selection = Selection::new(SelectionType::Simple, point, side);
            self.selection.command_block_id = None;
            self.pending_ops
                .push_back(TermOp::SetSelection(Some((selection, point))));
            cx.notify();
        }
    }

    fn mouse_up(&mut self, e: &MouseUpEvent, cx: &Context<Terminal>) {
        let setting = TerminalSettings::global(cx);
        let position = e.position - self.content.terminal_bounds.bounds.origin;

        if self.mouse_mode(e.modifiers.shift) {
            let point = grid_point(
                position,
                self.content.terminal_bounds,
                self.content.display_offset,
            );
            if let Some(bytes) =
                mouse_button_report(point, e.button, e.modifiers, false, self.term_mode)
            {
                self.pty_tx.notify(bytes);
            }
        } else if e.button == gpui::MouseButton::Left && setting.copy_on_select {
            // Copy selection to clipboard.
            // Note: actual clipboard write happens when the op is applied.
            // This keeps ordering consistent with selection updates.
            // The widget-side action `Copy` uses the same path.
            // (Some terminals choose to keep selection; we ignore for now.)
            self.pending_ops.push_back(TermOp::Copy(Some(true)));
        }

        self.selection.phase = SelectionPhase::Ended;
    }

    fn scroll_wheel(&mut self, e: &ScrollWheelEvent) {
        let mouse_mode = self.mouse_mode(e.shift);

        if let Some(scroll_lines) = backends::determine_scroll_lines(
            &mut self.scroll_px,
            e,
            self.content.terminal_bounds.line_height,
            mouse_mode,
            self.content.terminal_bounds.height(),
        ) {
            if mouse_mode {
                let point = grid_point(
                    e.position - self.content.terminal_bounds.bounds.origin,
                    self.content.terminal_bounds,
                    self.content.display_offset,
                );
                if let Some(scrolls) = scroll_report(point, scroll_lines, e, self.term_mode) {
                    for scroll in scrolls {
                        self.pty_tx.notify(scroll);
                    }
                }
            } else if self
                .term_mode
                .contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL)
                && !e.shift
            {
                self.pty_tx.notify(alt_scroll(scroll_lines))
            } else if scroll_lines != 0 {
                self.queue_scroll_delta(scroll_lines);
            }
        }
    }

    fn get_content(&self) -> String {
        let term = self.term.lock_unfair();
        let grid = term.grid();
        let start = AlacPoint::new(grid.topmost_line(), Column(0));
        let end = AlacPoint::new(grid.bottommost_line(), grid.last_column());
        term.bounds_to_string(start, end)
    }

    fn last_n_non_empty_lines(&self, n: usize) -> Vec<String> {
        if n == 0 {
            return Vec::new();
        }

        let term = self.term.lock_unfair();
        let grid = term.grid();
        let cols = grid.columns();
        if cols == 0 {
            return Vec::new();
        }

        let top = grid.topmost_line();
        let last_col = grid.last_column();

        let mut out: Vec<String> = Vec::with_capacity(n);
        let mut line = grid.bottommost_line();
        loop {
            let s = term.bounds_to_string(
                AlacPoint::new(line, Column(0)),
                AlacPoint::new(line, last_col),
            );
            let trimmed = s.trim_end_matches(|c: char| c.is_whitespace()).to_string();
            if !trimmed.is_empty() {
                out.push(trimmed);
                if out.len() >= n {
                    break;
                }
            }

            if line == top {
                break;
            }
            line -= 1usize;
        }

        out.reverse();
        out
    }

    fn preview_lines_from_top(&self, start_line: usize, count: usize) -> Vec<String> {
        if count == 0 {
            return Vec::new();
        }

        let term = self.term.lock_unfair();
        let grid = term.grid();
        let cols = grid.columns();
        if cols == 0 {
            return Vec::new();
        }

        let top = grid.topmost_line();
        let bottom = grid.bottommost_line();
        let last_col = grid.last_column();

        let mut out = Vec::new();
        let mut line = top + start_line;
        for _ in 0..count {
            if line > bottom {
                break;
            }
            let s = term.bounds_to_string(
                AlacPoint::new(line, Column(0)),
                AlacPoint::new(line, last_col),
            );
            out.push(s.trim_end_matches(|c: char| c.is_whitespace()).to_string());
            line += 1usize;
        }
        out
    }

    fn preview_cells_from_top(
        &self,
        start_line: usize,
        count: usize,
    ) -> (usize, usize, Vec<crate::IndexedCell>) {
        if count == 0 {
            return (0, 0, Vec::new());
        }

        let term = self.term.lock_unfair();
        let grid = term.grid();
        let cols = grid.columns();
        if cols == 0 {
            return (0, 0, Vec::new());
        }

        let top = grid.topmost_line();
        let bottom = grid.bottommost_line();

        let mut out: Vec<crate::IndexedCell> = Vec::with_capacity(cols * count);
        let mut rows = 0usize;

        let mut line = top + start_line;
        for r in 0..count {
            if line > bottom {
                break;
            }
            rows = r + 1;
            for c in 0..cols {
                let cell = grid[AlacPoint::new(line, Column(c))].clone();
                out.push(crate::IndexedCell {
                    point: crate::GridPoint::new(r as i32, c),
                    cell: map_cell(cell),
                });
            }
            line += 1usize;
        }

        (cols, rows, out)
    }

    fn scrollback_top_line_id(&self) -> i64 {
        let term = self.term.lock_unfair();
        term.grid().topmost_line().0 as i64
    }

    fn cursor_line_id(&self) -> Option<i64> {
        let term = self.term.lock_unfair();
        Some(term.grid().cursor.point.line.0 as i64)
    }
}

fn map_cursor_shape(shape: CursorShape) -> AlacCursorShape {
    match shape {
        CursorShape::Block => AlacCursorShape::Block,
        CursorShape::Underline => AlacCursorShape::Underline,
        CursorShape::Bar => AlacCursorShape::Beam,
        CursorShape::Hollow => AlacCursorShape::HollowBlock,
    }
}

fn map_cursor_render_shape(shape: AlacCursorShape) -> CursorRenderShape {
    match shape {
        AlacCursorShape::Hidden => CursorRenderShape::Hidden,
        AlacCursorShape::Block => CursorRenderShape::Block,
        AlacCursorShape::Underline => CursorRenderShape::Underline,
        AlacCursorShape::Beam => CursorRenderShape::Bar,
        AlacCursorShape::HollowBlock => CursorRenderShape::Hollow,
    }
}

fn map_point(point: AlacPoint) -> GridPoint {
    GridPoint::new(point.line.0, point.column.0)
}

fn map_mode(mode: TermMode) -> TerminalMode {
    let mut out = TerminalMode::empty();
    if mode.contains(TermMode::ALT_SCREEN) {
        out |= TerminalMode::ALT_SCREEN;
    }
    if mode.contains(TermMode::BRACKETED_PASTE) {
        out |= TerminalMode::BRACKETED_PASTE;
    }
    if mode.contains(TermMode::FOCUS_IN_OUT) {
        out |= TerminalMode::FOCUS_IN_OUT;
    }
    if mode.contains(TermMode::SGR_MOUSE) {
        out |= TerminalMode::SGR_MOUSE;
    }
    if mode.contains(TermMode::UTF8_MOUSE) {
        out |= TerminalMode::UTF8_MOUSE;
    }
    if mode.intersects(TermMode::MOUSE_MODE) {
        out |= TerminalMode::MOUSE_MODE;
    }
    if mode.contains(TermMode::ALTERNATE_SCROLL) {
        out |= TerminalMode::ALTERNATE_SCROLL;
    }
    if mode.contains(TermMode::APP_CURSOR) {
        out |= TerminalMode::APP_CURSOR;
    }
    if mode.contains(TermMode::APP_KEYPAD) {
        out |= TerminalMode::APP_KEYPAD;
    }
    if mode.contains(TermMode::SHOW_CURSOR) {
        out |= TerminalMode::SHOW_CURSOR;
    }
    if mode.contains(TermMode::LINE_WRAP) {
        out |= TerminalMode::LINE_WRAP;
    }
    if mode.contains(TermMode::ORIGIN) {
        out |= TerminalMode::ORIGIN;
    }
    if mode.contains(TermMode::INSERT) {
        out |= TerminalMode::INSERT;
    }
    if mode.contains(TermMode::LINE_FEED_NEW_LINE) {
        out |= TerminalMode::LINE_FEED_NEW_LINE;
    }
    if mode.contains(TermMode::MOUSE_REPORT_CLICK) {
        out |= TerminalMode::MOUSE_REPORT_CLICK;
    }
    if mode.contains(TermMode::MOUSE_DRAG) {
        out |= TerminalMode::MOUSE_DRAG;
    }
    if mode.contains(TermMode::MOUSE_MOTION) {
        out |= TerminalMode::MOUSE_MOTION;
    }
    out
}

fn map_named_color(n: AlacNamedColor) -> NamedColor {
    match n {
        AlacNamedColor::Black => NamedColor::Black,
        AlacNamedColor::Red => NamedColor::Red,
        AlacNamedColor::Green => NamedColor::Green,
        AlacNamedColor::Yellow => NamedColor::Yellow,
        AlacNamedColor::Blue => NamedColor::Blue,
        AlacNamedColor::Magenta => NamedColor::Magenta,
        AlacNamedColor::Cyan => NamedColor::Cyan,
        AlacNamedColor::White => NamedColor::White,
        AlacNamedColor::BrightBlack => NamedColor::BrightBlack,
        AlacNamedColor::BrightRed => NamedColor::BrightRed,
        AlacNamedColor::BrightGreen => NamedColor::BrightGreen,
        AlacNamedColor::BrightYellow => NamedColor::BrightYellow,
        AlacNamedColor::BrightBlue => NamedColor::BrightBlue,
        AlacNamedColor::BrightMagenta => NamedColor::BrightMagenta,
        AlacNamedColor::BrightCyan => NamedColor::BrightCyan,
        AlacNamedColor::BrightWhite => NamedColor::BrightWhite,
        AlacNamedColor::Foreground
        | AlacNamedColor::BrightForeground
        | AlacNamedColor::DimForeground => NamedColor::Foreground,
        AlacNamedColor::Background => NamedColor::Background,
        AlacNamedColor::Cursor => NamedColor::Cursor,
        // Map dim variants to their non-dim counterparts.
        AlacNamedColor::DimBlack => NamedColor::Black,
        AlacNamedColor::DimRed => NamedColor::Red,
        AlacNamedColor::DimGreen => NamedColor::Green,
        AlacNamedColor::DimYellow => NamedColor::Yellow,
        AlacNamedColor::DimBlue => NamedColor::Blue,
        AlacNamedColor::DimMagenta => NamedColor::Magenta,
        AlacNamedColor::DimCyan => NamedColor::Cyan,
        AlacNamedColor::DimWhite => NamedColor::White,
    }
}

fn map_color(c: AlacColor) -> TermColor {
    match c {
        AlacColor::Named(n) => TermColor::Named(map_named_color(n)),
        AlacColor::Indexed(i) => TermColor::Indexed(i),
        AlacColor::Spec(rgb) => TermColor::Rgb(rgb.r, rgb.g, rgb.b),
    }
}

fn map_flags(flags: Flags) -> CellFlags {
    let mut out = CellFlags::empty();
    if flags.contains(Flags::INVERSE) {
        out |= CellFlags::INVERSE;
    }
    if flags.contains(Flags::WRAPLINE) {
        out |= CellFlags::WRAPLINE;
    }
    if flags.contains(Flags::WIDE_CHAR_SPACER) {
        out |= CellFlags::WIDE_CHAR_SPACER;
    }
    if flags.contains(Flags::BOLD) {
        out |= CellFlags::BOLD;
    }
    if flags.contains(Flags::DIM) {
        out |= CellFlags::DIM;
    }
    if flags.contains(Flags::ITALIC) {
        out |= CellFlags::ITALIC;
    }
    if flags.contains(Flags::UNDERLINE) {
        out |= CellFlags::UNDERLINE;
    }
    if flags.contains(Flags::DOUBLE_UNDERLINE) {
        out |= CellFlags::DOUBLE_UNDERLINE;
    }
    if flags.contains(Flags::UNDERCURL) {
        out |= CellFlags::CURLY_UNDERLINE;
    }
    if flags.contains(Flags::DOTTED_UNDERLINE) {
        out |= CellFlags::DOTTED_UNDERLINE;
    }
    if flags.contains(Flags::DASHED_UNDERLINE) {
        out |= CellFlags::DASHED_UNDERLINE;
    }
    if flags.contains(Flags::STRIKEOUT) {
        out |= CellFlags::STRIKEOUT;
    }
    out
}

fn map_cell(cell: AlacCell) -> Cell {
    let mut out = Cell {
        c: cell.c,
        fg: map_color(cell.fg),
        bg: map_color(cell.bg),
        flags: map_flags(cell.flags),
        hyperlink: cell.hyperlink().map(|h| h.uri().to_string()),
        zerowidth: cell.zerowidth().map(|zw| zw.to_vec()).unwrap_or_default(),
    };

    // Alacritty can produce '\0' for empty cells; normalize to ' ' for rendering.
    if out.c == '\0' {
        out.c = ' ';
    }

    out
}

// Implement alacritty's sizing traits for our backend-neutral bounds type.
impl From<TerminalBounds> for WindowSize {
    fn from(val: TerminalBounds) -> Self {
        WindowSize {
            num_lines: val.num_lines() as u16,
            num_cols: val.num_columns() as u16,
            cell_width: f32::from(val.cell_width()) as u16,
            cell_height: f32::from(val.line_height()) as u16,
        }
    }
}

impl alacritty_terminal::grid::Dimensions for TerminalBounds {
    fn total_lines(&self) -> usize {
        self.num_lines()
    }

    fn screen_lines(&self) -> usize {
        self.num_lines()
    }

    fn columns(&self) -> usize {
        self.num_columns()
    }
}

#[cfg(test)]
mod shell_tests {
    use super::*;

    #[test]
    fn local_pty_options_uses_termua_shell_when_available() {
        let mut env = std::collections::HashMap::new();
        env.insert("TERMUA_SHELL".to_string(), "fish".to_string());

        let opts = local_pty_options_for_program_exists(env, |p| p == "fish");
        assert!(opts.shell.is_some());
    }

    #[test]
    fn local_pty_options_uses_shell_integration_args() {
        let mut env = std::collections::HashMap::new();
        env.insert("TERMUA_SHELL".to_string(), "fish".to_string());
        env.insert(
            "TERMUA_FISH_INIT".to_string(),
            "/tmp/termua-test.fish".to_string(),
        );

        let opts = local_pty_options_for_program_exists(env, |p| p == "fish");
        let shell = opts.shell.expect("expected shell");
        let shell_debug = format!("{shell:?}");
        assert!(shell_debug.contains("fish"));
        assert!(shell_debug.contains("--init-command"));
        assert!(shell_debug.contains("source \\\"$TERMUA_FISH_INIT\\\""));
        assert!(shell_debug.contains("--interactive"));
    }

    #[test]
    fn local_pty_options_falls_back_when_shell_not_found() {
        let mut env = std::collections::HashMap::new();
        env.insert("TERMUA_SHELL".to_string(), "fish".to_string());

        let opts = local_pty_options_for_program_exists(env, |_p| false);
        assert!(opts.shell.is_none());
    }
}

#[cfg(test)]
mod command_block_tests {
    #[test]
    fn adjust_osc133_boundary_matches_wezterm_heuristic() {
        assert_eq!(super::adjust_osc133_boundary("C", 10, 0), 9);
        assert_eq!(super::adjust_osc133_boundary("D;0", 10, 0), 9);
        assert_eq!(super::adjust_osc133_boundary("A", 10, 0), 9);
        assert_eq!(super::adjust_osc133_boundary("C", 0, 0), 0);

        assert_eq!(super::adjust_osc133_boundary("C", 10, 5), 10);
        assert_eq!(super::adjust_osc133_boundary("D;0", 10, 5), 10);
        assert_eq!(super::adjust_osc133_boundary("A", 10, 5), 10);
        assert_eq!(super::adjust_osc133_boundary("B", 10, 0), 10);
    }
}

#[cfg(test)]
mod selection_tests {
    use std::{collections::VecDeque, io, sync::Arc};

    use alacritty_terminal::{
        Term,
        event::{OnResize, WindowSize},
        event_loop::EventLoop,
        sync::FairMutex,
        tty::{ChildEvent, EventedPty, EventedReadWrite},
    };
    use gpui::{AppContext, Bounds, Modifiers, MouseButton, MouseDownEvent, point, px, size};
    use parking_lot::Mutex;
    use polling::{Event, PollMode, Poller};

    use super::{
        AlacTermEvent, AlacrittyBackend, EventProxy, RecordState, SearchState, SelectionState,
        TermOp,
    };
    use crate::{
        TerminalBackend, TerminalBounds, TerminalContent, command_blocks::CommandBlockTracker,
    };

    #[derive(Default)]
    struct DummyPty {
        reader: io::Cursor<Vec<u8>>,
        writer: Vec<u8>,
    }

    impl EventedReadWrite for DummyPty {
        type Reader = io::Cursor<Vec<u8>>;
        type Writer = Vec<u8>;

        unsafe fn register(
            &mut self,
            _poller: &Arc<Poller>,
            _event: Event,
            _mode: PollMode,
        ) -> io::Result<()> {
            Ok(())
        }

        fn reregister(
            &mut self,
            _poller: &Arc<Poller>,
            _event: Event,
            _mode: PollMode,
        ) -> io::Result<()> {
            Ok(())
        }

        fn deregister(&mut self, _poller: &Arc<Poller>) -> io::Result<()> {
            Ok(())
        }

        fn reader(&mut self) -> &mut Self::Reader {
            &mut self.reader
        }

        fn writer(&mut self) -> &mut Self::Writer {
            &mut self.writer
        }
    }

    impl EventedPty for DummyPty {
        fn next_child_event(&mut self) -> Option<ChildEvent> {
            None
        }
    }

    impl OnResize for DummyPty {
        fn on_resize(&mut self, _window_size: WindowSize) {}
    }

    fn test_backend() -> AlacrittyBackend {
        let (events_tx, _events_rx) = futures::channel::mpsc::unbounded();
        let term = Term::new(
            alacritty_terminal::term::Config::default(),
            &TerminalBounds::default(),
            EventProxy(events_tx.clone()),
        );
        let term = Arc::new(FairMutex::new(term));

        let event_loop = EventLoop::new(
            Arc::clone(&term),
            EventProxy(events_tx),
            DummyPty::default(),
            false,
            false,
        )
        .expect("event loop");
        let pty_tx = alacritty_terminal::event_loop::Notifier(event_loop.channel());

        let cast_slot: Arc<Mutex<Option<crate::cast::CastRecorderSender>>> =
            Arc::new(Mutex::new(None));

        AlacrittyBackend {
            pty_tx,
            term,
            term_config: alacritty_terminal::term::Config::default(),
            pending_ops: VecDeque::new(),
            term_mode: alacritty_terminal::term::TermMode::empty(),
            search: SearchState::default(),
            selection: SelectionState::default(),
            scroll_px: px(0.0),
            exited: false,
            content: TerminalContent::default(),
            sftp: None,
            record: RecordState::new(cast_slot),
            blocks: CommandBlockTracker::new(200),
        }
    }

    #[gpui::test]
    fn exit_emits_close_terminal_event(cx: &mut gpui::TestAppContext) {
        cx.update(|app| {
            crate::init(app);
        });

        let terminal = cx.update(|app| {
            app.new(|_cx| {
                crate::Terminal::new(crate::TerminalType::Alacritty, Box::new(test_backend()))
            })
        });

        let exited = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let exited_for_sub = exited.clone();
        let _sub = cx.update(|app| {
            app.subscribe(&terminal, move |_, event: &crate::Event, _| {
                if matches!(event, crate::Event::CloseTerminal) {
                    exited_for_sub.store(true, std::sync::atomic::Ordering::Relaxed);
                }
            })
        });

        cx.update(|app| {
            terminal.update(app, |terminal, cx| {
                terminal.dispatch_backend_event(Box::new(AlacTermEvent::Exit), cx);
            });
        });
        cx.run_until_parked();

        assert!(
            exited.load(std::sync::atomic::Ordering::Relaxed),
            "expected alacritty exit to emit CloseTerminal"
        );
    }

    #[test]
    fn triple_click_selects_current_line() {
        let (events_tx, _events_rx) = futures::channel::mpsc::unbounded();
        let term = Term::new(
            alacritty_terminal::term::Config::default(),
            &TerminalBounds::default(),
            EventProxy(events_tx.clone()),
        );
        let term = Arc::new(FairMutex::new(term));

        // Create a Notifier channel without running the event loop.
        let event_loop = EventLoop::new(
            Arc::clone(&term),
            EventProxy(events_tx),
            DummyPty::default(),
            false,
            false,
        )
        .expect("event loop");
        let pty_tx = alacritty_terminal::event_loop::Notifier(event_loop.channel());

        let cast_slot: Arc<Mutex<Option<crate::cast::CastRecorderSender>>> =
            Arc::new(Mutex::new(None));

        let mut backend = AlacrittyBackend {
            pty_tx,
            term,
            term_config: alacritty_terminal::term::Config::default(),
            pending_ops: VecDeque::new(),
            term_mode: alacritty_terminal::term::TermMode::empty(),
            search: SearchState::default(),
            selection: SelectionState::default(),
            scroll_px: px(0.0),
            exited: false,
            content: TerminalContent::default(),
            sftp: None,
            record: RecordState::new(cast_slot),
            blocks: CommandBlockTracker::new(200),
        };

        let bounds = Bounds::new(point(px(0.0), px(0.0)), size(px(100.0), px(30.0)));
        backend.content.terminal_bounds = TerminalBounds::new(px(10.0), px(10.0), bounds);

        let e = MouseDownEvent {
            button: MouseButton::Left,
            position: point(px(5.0 * 10.0 + 1.0), px(1.0 * 10.0 + 1.0)),
            modifiers: Modifiers::default(),
            click_count: 3,
            first_mouse: false,
        };

        backend.select_word_at_event_position(&e);

        match backend.pending_ops.back() {
            Some(TermOp::SetSelection(Some((selection, _)))) => {
                assert_eq!(
                    selection.ty,
                    alacritty_terminal::selection::SelectionType::Lines
                );
            }
            _ => panic!("expected SetSelection op"),
        }
    }
}
