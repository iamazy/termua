use std::{
    borrow::Cow,
    collections::{BTreeMap, VecDeque},
    io::{Read, Write},
    ops::RangeInclusive,
    sync::Arc,
    thread,
    time::{Duration, Instant, SystemTime},
};

use futures::FutureExt;
use gpui::{
    Bounds, Context, Keystroke, Modifiers, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels,
    Point, ReadGlobal, ScrollWheelEvent, Window, px,
};
use parking_lot::Mutex;
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, PtySystem, native_pty_system};
use smol::channel::{Receiver, Sender};
use wezterm_surface::{CursorShape as WezCursorShape, CursorVisibility};
use wezterm_term::{
    Alert, AlertHandler, Cell as WezCell, CellAttributes, Intensity, PhysRowIndex, Screen,
    StableRowIndex, Terminal, TerminalConfiguration, TerminalSize, Underline, VisibleRowIndex,
    color::{ColorAttribute, ColorPalette},
    input::{
        KeyCode, KeyModifiers, MouseButton as WezMouseButton, MouseEvent as WezMouseEvent,
        MouseEventKind,
    },
};

use crate::{
    Cell, CellFlags, Cursor, CursorRenderShape, Event, GridPoint, NamedColor, PtySource, TermColor,
    TerminalBackend, TerminalBounds, TerminalContent, TerminalMode, TerminalShutdownPolicy,
    TerminalType,
    backends::{self, ssh},
    cast::{CastHeader, CastRecorderSender, CastRecorderState, start_cast_recorder},
    command_blocks::{CommandBlockTracker, OscEvent, OscStreamParser},
    serial::{serial2_char_size, serial2_flow_control, serial2_parity, serial2_stop_bits},
    settings::{CursorShape, TerminalSettings},
    terminal::Terminal as WidgetTerminal,
};

/// Minimal configuration for a wezterm-term terminal instance.
#[derive(Debug)]
struct WeztermConfig {
    scrollback_size: usize,
}

impl TerminalConfiguration for WeztermConfig {
    fn color_palette(&self) -> ColorPalette {
        ColorPalette::default()
    }

    fn scrollback_size(&self) -> usize {
        self.scrollback_size
    }
}

#[derive(Debug)]
enum WezEvent {
    PtyRead(Vec<u8>),
    PtyExit,
    Alert(Alert),
}

#[derive(Clone)]
struct AlertProxy(Sender<WezEvent>);

impl AlertHandler for AlertProxy {
    fn alert(&mut self, alert: Alert) {
        // Alert notifications are best-effort; never block the caller.
        let _ = self.0.try_send(WezEvent::Alert(alert));
    }
}

#[derive(Clone)]
struct SharedWriter {
    inner: Arc<Mutex<Box<dyn Write + Send>>>,
    cast: Arc<Mutex<Option<CastRecorderSender>>>,
}

impl Write for SharedWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = {
            let mut w = self.inner.lock();
            w.write(buf)?
        };

        if n != 0
            && let Some(sender) = self.cast.lock().as_ref()
        {
            sender.input(&buf[..n]);
        }

        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        let mut w = self.inner.lock();
        w.flush()
    }
}

pub struct TerminalBuilder {
    backend: WezTermBackend,
    events_rx: Receiver<WezEvent>,
}

impl TerminalBuilder {
    pub fn new(
        source: PtySource,
        cursor_shape: CursorShape,
        max_scrollback: Option<usize>,
        exit_fn: Option<fn(&mut Context<WidgetTerminal>)>,
    ) -> anyhow::Result<Self> {
        let scrollback_size = max_scrollback
            .unwrap_or(backends::DEFAULT_SCROLLBACK_LINES)
            .min(backends::MAX_SCROLLBACK_LINES);

        let (master, child, sftp) = match source {
            PtySource::Local { env, window_id: _ } => {
                // env.entry("LANG".to_string())
                //     .or_insert_with(|| "en_US.UTF-8".to_string());
                // env.entry("TERM".to_string())
                //     .or_insert_with(|| "xterm-256color".to_string());
                // env.entry("COLORTERM".to_string())
                //     .or_insert_with(|| "truecolor".to_string());

                let pty_system = native_pty_system();
                let pair = pty_system.openpty(PtySize {
                    rows: 24,
                    cols: 80,
                    pixel_width: 0,
                    pixel_height: 0,
                })?;

                // `wezterm_term` uses `portable_pty` directly, which does not provide a
                // cross-platform "default shell" out of the box. We pick a reasonable
                // default per OS and allow users to override it via env (`TERMUA_SHELL`).
                let mut spawn_err: Option<anyhow::Error> = None;
                let mut child: Option<Box<dyn portable_pty::Child + Send>> = None;
                for mut cmd in shell_command_candidates_for_local_env(&env) {
                    for (k, v) in &env {
                        cmd.env(k, v);
                    }

                    match pair.slave.spawn_command(cmd) {
                        Ok(c) => {
                            child = Some(c);
                            break;
                        }
                        Err(e) => spawn_err = Some(e),
                    }
                }

                let child = child.ok_or_else(|| {
                    spawn_err
                        .unwrap_or_else(|| anyhow::anyhow!("failed to spawn any default shell"))
                })?;
                drop(pair.slave);

                (pair.master, child, None)
            }
            PtySource::Ssh { env, opts } => {
                let (pty, child, sftp) = ssh::connect(env, opts)?;
                (
                    Box::new(pty) as Box<dyn MasterPty + Send>,
                    Box::new(child) as Box<dyn portable_pty::Child + Send>,
                    Some(sftp),
                )
            }
            PtySource::Serial { opts } => {
                log::debug!(
                    "gpui_term: opening serial (wezterm backend): port={} baud={} data_bits={} \
                     parity={:?} stop_bits={:?} flow_control={:?}",
                    opts.port,
                    opts.baud,
                    opts.data_bits,
                    opts.parity,
                    opts.stop_bits,
                    opts.flow_control
                );

                let mut pty_system = portable_pty::serial::SerialTty::new(&opts.port);
                pty_system.set_baud_rate(opts.baud);
                pty_system.set_char_size(serial2_char_size(opts.data_bits)?);
                pty_system.set_parity(serial2_parity(opts.parity));
                pty_system.set_stop_bits(serial2_stop_bits(opts.stop_bits));
                pty_system.set_flow_control(serial2_flow_control(opts.flow_control));

                let pair = pty_system.openpty(PtySize {
                    rows: 24,
                    cols: 80,
                    pixel_width: 0,
                    pixel_height: 0,
                })?;

                let child: Box<dyn portable_pty::Child + Send> = pair
                    .slave
                    .spawn_command(CommandBuilder::new_default_prog())?;
                drop(pair.slave);

                (pair.master, child, None)
            }
        };

        let (backend, events_rx) =
            build_backend(master, child, scrollback_size, cursor_shape, exit_fn, sftp)?;
        Ok(Self { backend, events_rx })
    }

    pub fn subscribe(self, cx: &Context<WidgetTerminal>) -> WidgetTerminal {
        let terminal = WidgetTerminal::new(TerminalType::WezTerm, Box::new(self.backend));
        let events_rx = self.events_rx;

        cx.spawn(async move |terminal, cx| {
            let mut batch: Vec<WezEvent> = Vec::with_capacity(64);
            while let Ok(first) = events_rx.recv().await {
                batch.clear();
                batch.push(first);

                let mut timer = cx
                    .background_executor()
                    .timer(Duration::from_millis(4))
                    .fuse();

                loop {
                    futures::select_biased! {
                        e = events_rx.recv().fuse() => {
                            match e {
                                Ok(e) => batch.push(e),
                                Err(_) => break,
                            };
                        }
                        _ = timer => break,
                    }
                }

                terminal.update(cx, |term, cx| {
                    for e in batch.drain(..) {
                        term.dispatch_backend_event(Box::new(e), cx);
                    }
                    cx.notify();
                })?;
            }

            anyhow::Ok(())
        })
        .detach();

        terminal
    }
}

const EVENTS_CHANNEL_CAPACITY: usize = 128;

fn build_backend(
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn portable_pty::Child + Send>,
    scrollback_size: usize,
    default_cursor_shape: CursorShape,
    exit_fn: Option<fn(&mut Context<WidgetTerminal>)>,
    sftp: Option<wezterm_ssh::Sftp>,
) -> anyhow::Result<(WezTermBackend, Receiver<WezEvent>)> {
    let reader = master.try_clone_reader()?;
    let writer = master.take_writer()?;

    let cast_slot: Arc<Mutex<Option<CastRecorderSender>>> = Arc::new(Mutex::new(None));
    let writer = SharedWriter {
        inner: Arc::new(Mutex::new(writer)),
        cast: Arc::clone(&cast_slot),
    };
    let (events_tx, events_rx) = smol::channel::bounded(EVENTS_CHANNEL_CAPACITY);

    let mut term = Terminal::new(
        TerminalSize::default(),
        Arc::new(WeztermConfig { scrollback_size }),
        "termua",
        "0",
        Box::new(writer.clone()),
    );
    // Bubble up terminal notifications (bell/title/etc.) through the same event channel.
    term.set_notification_handler(Box::new(AlertProxy(events_tx.clone())));
    spawn_pty_reader_thread(reader, events_tx.clone());

    // `child` is moved into the wait thread, but we still want to be able to terminate it from
    // the UI thread (e.g. when a tab is closed).
    let child_killer = child.clone_killer();
    spawn_child_wait_thread(child, events_tx.clone());

    Ok((
        WezTermBackend {
            master,
            writer,
            term: Arc::new(Mutex::new(term)),
            child_killer,
            shutdown: ShutdownState::default(),
            pending_ops: VecDeque::with_capacity(16),
            viewport_top_stable: None,
            last_clicked_line: None,
            search: SearchState::default(),
            content: TerminalContent::default(),
            exited: false,
            last_mouse_pos: None,
            selection: SelectionState::default(),
            default_cursor_shape,
            exit_fn,
            scroll_px: px(0.0),
            sftp,
            record: RecordState::new(cast_slot),
            osc: OscStreamParser::new(),
            blocks: CommandBlockTracker::new(200),
        },
        events_rx,
    ))
}

fn current_dir_path_from_term(term: &Terminal) -> Option<String> {
    let url = term.get_current_dir()?;
    if url.scheme() != "file" {
        return None;
    }

    // OSC 7 is `file://<host>/<path>`; we only expose the decoded path.
    urlencoding::decode(url.path()).ok().map(|s| s.into_owned())
}

fn cursor_stable_line(term: &Terminal) -> (i64, usize) {
    let screen = term.screen();
    let cursor = term.cursor_pos();
    let max_y = screen.physical_rows.saturating_sub(1) as VisibleRowIndex;
    let y = cursor.y.clamp(0, max_y);
    let phys = screen.phys_row(y);
    (screen.phys_to_stable_row_index(phys) as i64, cursor.x)
}

fn prev_stable_row(stable: i64) -> i64 {
    if stable > 0 { stable - 1 } else { 0 }
}

fn adjust_osc133_boundary(payload: &str, stable: i64, cursor_x: usize) -> i64 {
    // Heuristic: when the cursor is at column 0, it often means:
    // - For `C`: the user hit enter and the cursor moved to the next line, so the command line
    //   itself is the previous row.
    // - For `D`: the command finished and the cursor is on the next (prompt) line, so the output
    //   ends on the previous row.
    //
    // This approximates Warp-style "command line + output" blocks without requiring prompt
    // markers (OSC 133 A/B).
    let kind = payload.trim_start().as_bytes().first().copied();
    if cursor_x == 0 && matches!(kind, Some(b'A') | Some(b'C') | Some(b'D')) {
        prev_stable_row(stable)
    } else {
        stable
    }
}

fn stable_row_text(screen: &wezterm_term::Screen, stable_row: i64) -> Option<String> {
    let stable_row = StableRowIndex::try_from(stable_row).ok()?;
    let phys_range = screen.stable_range(&(stable_row..stable_row.saturating_add(1)));
    let line = screen.lines_in_phys_range(phys_range).into_iter().next()?;
    Some(line.as_str().trim_end().to_string())
}

fn wrapped_command_span_for_stable_row(
    screen: &wezterm_term::Screen,
    stable_row: i64,
) -> Option<(i64, String)> {
    let stable_row = StableRowIndex::try_from(stable_row).ok()?;
    let phys_range = screen.stable_range(&(stable_row..stable_row.saturating_add(1)));
    let mut start = phys_range.start;
    let end = phys_range.end.checked_sub(1)?;

    while start > 0 {
        let prev = screen
            .lines_in_phys_range(start - 1..start)
            .into_iter()
            .next()?;
        if !prev.last_cell_was_wrapped() {
            break;
        }
        start -= 1;
    }

    let lines = screen.lines_in_phys_range(start..end.saturating_add(1));
    let mut command = String::new();
    for line in lines {
        command.push_str(line.as_str().trim_end_matches(|c: char| c.is_whitespace()));
    }

    let command = command.trim_end().to_string();
    (!command.trim().is_empty()).then(|| (screen.phys_to_stable_row_index(start) as i64, command))
}

fn stable_row_texts(screen: &wezterm_term::Screen) -> Vec<(i64, String)> {
    let total = screen.scrollback_rows();
    screen
        .lines_in_phys_range(0..total)
        .into_iter()
        .enumerate()
        .map(|(phys, line)| {
            (
                screen.phys_to_stable_row_index(phys) as i64,
                line.as_str().trim_end().to_string(),
            )
        })
        .collect()
}

#[derive(Clone)]
enum TermOp {
    Resize(TerminalBounds),
    Clear,
    Scroll(i32),
    Copy(Option<bool>),
}

#[derive(Default)]
struct SelectionState {
    range: Option<crate::SelectionRange>,
    selecting: bool,
    anchor: Option<GridPoint>,
    command_block_id: Option<u64>,
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

#[derive(Clone, Copy, Debug, Default)]
struct ShutdownState {
    requested_at: Option<Instant>,
    kill_after: Option<Duration>,
    kill_sent: bool,
}

fn poll_shutdown(
    shutdown: &mut ShutdownState,
    now: Instant,
    exited: bool,
    killer: &mut dyn ChildKiller,
) {
    if exited || shutdown.kill_sent {
        return;
    }

    let (Some(requested_at), Some(kill_after)) = (shutdown.requested_at, shutdown.kill_after)
    else {
        return;
    };

    if now.duration_since(requested_at) >= kill_after {
        // Best-effort; the wait thread will emit `PtyExit` when the child actually terminates.
        log::debug!(
            "gpui_term[wezterm]: shutdown kill-after elapsed ({kill_after:?}); sending kill()"
        );
        let _ = killer.kill();
        shutdown.kill_sent = true;
    }
}

pub struct WezTermBackend {
    master: Box<dyn MasterPty + Send>,
    writer: SharedWriter,
    term: Arc<Mutex<Terminal>>,
    child_killer: Box<dyn ChildKiller + Send + Sync>,
    shutdown: ShutdownState,

    pending_ops: VecDeque<TermOp>,
    /// When `None`, follow the live view at the bottom of the scrollback.
    /// When `Some`, this is the stable row index of the top line of the viewport.
    viewport_top_stable: Option<StableRowIndex>,
    last_clicked_line: Option<i32>,
    search: SearchState,
    content: TerminalContent,
    exited: bool,
    last_mouse_pos: Option<Point<Pixels>>,
    selection: SelectionState,
    default_cursor_shape: CursorShape,
    exit_fn: Option<fn(&mut Context<WidgetTerminal>)>,
    scroll_px: Pixels,

    // SFTP support for SSH terminals.
    sftp: Option<wezterm_ssh::Sftp>,

    // Asciinema cast recording.
    record: RecordState,

    osc: OscStreamParser,
    blocks: CommandBlockTracker,
}

impl WezTermBackend {
    fn map_modifiers(modifiers: &Modifiers) -> KeyModifiers {
        let mut mods = KeyModifiers::default();
        if modifiers.shift {
            mods |= KeyModifiers::SHIFT;
        }
        if modifiers.control {
            mods |= KeyModifiers::CTRL;
        }
        if modifiers.alt {
            mods |= KeyModifiers::ALT;
        }
        if modifiers.platform {
            mods |= KeyModifiers::SUPER;
        }
        mods
    }

    fn term_mouse_grabbed(&self) -> bool {
        self.term.lock().is_mouse_grabbed()
    }

    fn compute_viewport_plan(&self, screen: &Screen) -> ViewportPlan {
        compute_viewport_plan(screen, self.viewport_top_stable)
    }

    fn compute_search_matches(term: &Terminal, query: &str) -> Vec<RangeInclusive<GridPoint>> {
        let screen = term.screen();
        let cols = screen.physical_cols.max(1);
        let rows = screen.physical_rows.max(1);
        let total = screen.scrollback_rows();
        if total == 0 {
            return Vec::new();
        }

        // Stable index of the topmost line we still have in the scrollback.
        let top_stable = screen.phys_to_stable_row_index(0);
        // Stable index of the top line of the live viewport.
        let base_start = total.saturating_sub(rows);
        let base_stable = screen.phys_to_stable_row_index(base_start);

        crate::backends::search::collect_search_matches_for_lines(
            query,
            screen.lines_in_phys_range(0..total).into_iter().enumerate(),
            |(phys_idx, line), tokens| {
                let stable_row = top_stable.saturating_add(phys_idx as StableRowIndex);
                let line_coord_i64 = stable_row as i64 - base_stable as i64;
                if line_coord_i64 < i64::from(i32::MIN) || line_coord_i64 > i64::from(i32::MAX) {
                    return None;
                }
                let line_coord = line_coord_i64 as i32;

                // Build a token stream of visible glyphs so wide characters (CJK, emoji) match
                // correctly even though they occupy multiple terminal columns.
                for cellref in line.visible_cells() {
                    let idx = cellref.cell_index();
                    if idx >= cols {
                        continue;
                    }

                    crate::backends::search::push_search_token(
                        tokens,
                        idx,
                        cellref.width(),
                        cols,
                        cellref.str(),
                    );
                }

                Some(line_coord)
            },
        )
    }

    fn queue_scroll_delta(&mut self, delta: i32) {
        if delta == 0 {
            return;
        }
        match self.pending_ops.back_mut() {
            Some(TermOp::Scroll(prev)) => *prev += delta,
            _ => self.pending_ops.push_back(TermOp::Scroll(delta)),
        }
    }

    fn clamp_pos_to_terminal_bounds(&self, pos: Point<Pixels>) -> Option<Point<Pixels>> {
        let bounds = self.content.terminal_bounds.bounds;
        if bounds.size.width <= Pixels::ZERO || bounds.size.height <= Pixels::ZERO {
            return None;
        }

        let mut pos = pos;
        let max_x = bounds.origin.x + bounds.size.width;
        let max_y = bounds.origin.y + bounds.size.height;
        if pos.x < bounds.origin.x {
            pos.x = bounds.origin.x;
        } else if pos.x >= max_x {
            // Keep position inside bounds even when clamping.
            pos.x = max_x - px(0.01);
        }
        if pos.y < bounds.origin.y {
            pos.y = bounds.origin.y;
        } else if pos.y >= max_y {
            pos.y = max_y - px(0.01);
        }

        Some(pos)
    }

    fn cell_coords_from_window_pos(&self, pos: Point<Pixels>) -> Option<(usize, i64)> {
        if !self.content.terminal_bounds.bounds.contains(&pos) {
            return None;
        }

        // Don't report mouse events while scrolled back; typical terminals require being at bottom.
        if self.content.display_offset != 0 {
            return None;
        }

        let local = pos - self.content.terminal_bounds.bounds.origin;
        let col = (local.x / self.content.terminal_bounds.cell_width()) as isize;
        let row = (local.y / self.content.terminal_bounds.line_height()) as isize;
        if col < 0 || row < 0 {
            return None;
        }

        Some((col as usize, row as i64))
    }

    fn selection_point_from_window_pos(&self, pos: Point<Pixels>) -> Option<GridPoint> {
        if !self.content.terminal_bounds.bounds.contains(&pos) {
            return None;
        }
        self.selection_point_from_window_pos_clamped(pos)
    }

    fn selection_point_from_window_pos_clamped(&self, pos: Point<Pixels>) -> Option<GridPoint> {
        let bounds = self.content.terminal_bounds.bounds;
        let pos = self.clamp_pos_to_terminal_bounds(pos)?;
        let local = pos - bounds.origin;
        let col = (local.x / self.content.terminal_bounds.cell_width()) as usize;
        let row = (local.y / self.content.terminal_bounds.line_height()) as usize;

        let term = self.term.lock();
        let screen = term.screen();
        let plan = self.compute_viewport_plan(screen);

        let row = row.min(plan.rows.saturating_sub(1));
        let col = col.min(plan.cols.saturating_sub(1));

        // Map to an absolute-ish coordinate space anchored at the live-view base stable row.
        let stable_row = plan
            .viewport_top_stable
            .saturating_add(row as StableRowIndex);
        let line = stable_row as i64 - plan.base_stable as i64;
        let line = line.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
        Some(GridPoint::new(line, col))
    }

    fn normalize_selection(a: GridPoint, b: GridPoint) -> crate::SelectionRange {
        if a <= b {
            crate::SelectionRange { start: a, end: b }
        } else {
            crate::SelectionRange { start: b, end: a }
        }
    }

    fn stable_row_for_grid_line_on_screen(screen: &wezterm_term::Screen, line: i32) -> Option<i64> {
        let total = screen.scrollback_rows();
        if total == 0 {
            return None;
        }

        let rows = screen.physical_rows.max(1);
        let base_start = total.saturating_sub(rows);
        let base_stable = screen.phys_to_stable_row_index(base_start);

        let stable = if line >= 0 {
            base_stable.saturating_add(line as StableRowIndex)
        } else {
            base_stable.saturating_sub((-line) as StableRowIndex)
        };

        Some(stable as i64)
    }

    fn grid_line_for_stable_row_on_screen(
        screen: &wezterm_term::Screen,
        stable_row: i64,
    ) -> Option<i32> {
        let total = screen.scrollback_rows();
        if total == 0 {
            return None;
        }

        let rows = screen.physical_rows.max(1);
        let base_start = total.saturating_sub(rows);
        let base_stable = screen.phys_to_stable_row_index(base_start) as i64;

        let line = stable_row.saturating_sub(base_stable);
        Some(line.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32)
    }

    fn command_block_id_for_selection_range(
        &self,
        selection: &crate::SelectionRange,
    ) -> Option<u64> {
        let term = self.term.lock();
        let screen = term.screen();
        let start = Self::stable_row_for_grid_line_on_screen(screen, selection.start.line)?;
        let end = Self::stable_row_for_grid_line_on_screen(screen, selection.end.line)?;
        self.blocks.block_id_for_range(start, end)
    }

    fn remapped_selected_command_block_selection_range(
        &self,
        screen: &wezterm_term::Screen,
        cols: usize,
    ) -> Option<crate::SelectionRange> {
        let block_id = self.selection.command_block_id?;
        let (start_stable, end_stable) = self.blocks.range_for_block_id(block_id)?;
        let start_line = Self::grid_line_for_stable_row_on_screen(screen, start_stable);
        let end_line = Self::grid_line_for_stable_row_on_screen(screen, end_stable);

        let (Some(start_line), Some(end_line)) = (start_line, end_line) else {
            return None;
        };

        let last_col = cols.saturating_sub(1);
        Some(crate::SelectionRange {
            start: GridPoint::new(start_line, 0),
            end: GridPoint::new(end_line, last_col),
        })
    }

    fn select_line_at_event_position(&mut self, e: &MouseDownEvent) {
        if self.mouse_mode(e.modifiers.shift) || e.modifiers.secondary() {
            return;
        }

        let Some(p) = self.selection_point_from_window_pos(e.position) else {
            return;
        };

        let cols = self.content.terminal_bounds.num_columns().max(1);
        let rows = self.content.terminal_bounds.num_lines().max(1);
        let last_col = cols.saturating_sub(1);

        let row_i32 = p.line + self.content.display_offset as i32;
        if row_i32 < 0 {
            return;
        }
        let mut start_row = row_i32 as usize;
        let mut end_row = start_row;
        if start_row >= rows {
            return;
        }

        let wraps_to_next =
            |cells: &[crate::IndexedCell], row: usize, last_col: usize, cols: usize| {
                let idx = row.saturating_mul(cols).saturating_add(last_col);
                cells
                    .get(idx)
                    .is_some_and(|c| c.cell.flags.contains(crate::CellFlags::WRAPLINE))
            };

        // Expand left across wrapped lines.
        while start_row > 0 && wraps_to_next(&self.content.cells, start_row - 1, last_col, cols) {
            start_row -= 1;
        }
        // Expand right across wrapped lines.
        while end_row + 1 < rows && wraps_to_next(&self.content.cells, end_row, last_col, cols) {
            end_row += 1;
        }

        let start_line = start_row as i32 - self.content.display_offset as i32;
        let end_line = end_row as i32 - self.content.display_offset as i32;

        self.selection.range = Some(crate::SelectionRange {
            start: GridPoint::new(start_line, 0),
            end: GridPoint::new(end_line, last_col),
        });
        self.selection.selecting = false;
        self.selection.anchor = None;
        self.selection.command_block_id = None;
    }

    fn selection_to_string(&self, selection: &crate::SelectionRange) -> String {
        let selection = crate::SelectionRange {
            start: selection.start.min(selection.end),
            end: selection.start.max(selection.end),
        };

        let term = self.term.lock();
        let screen = term.screen();
        let plan = self.compute_viewport_plan(screen);

        let rows = plan.rows.max(1);
        let cols = plan.cols.max(1);
        let max_stable = plan
            .base_stable
            .saturating_add(rows.saturating_sub(1) as StableRowIndex);

        // Note: We intentionally use saturating math here so "too-large" selection ranges clamp
        // to the available scrollback rather than returning an empty string.
        let stable_from_line = |line: i32| -> StableRowIndex {
            if line >= 0 {
                plan.base_stable.saturating_add(line as StableRowIndex)
            } else {
                plan.base_stable.saturating_sub((-line) as StableRowIndex)
            }
        };

        let mut start_stable = stable_from_line(selection.start.line);
        let mut end_stable = stable_from_line(selection.end.line);
        if start_stable > end_stable {
            std::mem::swap(&mut start_stable, &mut end_stable);
        }

        start_stable = start_stable.max(plan.top_stable);
        end_stable = end_stable.min(max_stable);

        let mut out = String::new();
        for stable_row in start_stable..=end_stable {
            let phys_range = screen.stable_range(&(stable_row..stable_row.saturating_add(1)));
            let Some(line) = screen.lines_in_phys_range(phys_range).into_iter().next() else {
                continue;
            };

            let line_i64 = stable_row as i64 - plan.base_stable as i64;
            let line_i32 = line_i64.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;

            let mut col_start = 0usize;
            let mut col_end = cols.saturating_sub(1);
            if line_i32 == selection.start.line {
                col_start = selection.start.column.min(col_end);
            }
            if line_i32 == selection.end.line {
                col_end = selection.end.column.min(col_end);
            }

            let mut cols_text: Vec<Option<String>> = vec![None; cols];
            for cellref in line.visible_cells() {
                let idx = cellref.cell_index();
                if idx < cols {
                    cols_text[idx] = Some(cellref.str().to_string());
                }
            }

            let mut line_buf = String::new();
            for cell in cols_text.iter().take(col_end + 1).skip(col_start) {
                if let Some(s) = cell.as_ref() {
                    line_buf.push_str(s);
                } else {
                    line_buf.push(' ');
                }
            }

            // Trim trailing whitespace to better match typical terminal selection behavior.
            let trimmed = line_buf.trim_end_matches(|c: char| c.is_whitespace());
            out.push_str(trimmed);

            if stable_row != end_stable {
                out.push('\n');
            }
        }

        out
    }

    fn send_mouse_event(
        &self,
        kind: MouseEventKind,
        button: WezMouseButton,
        pos: Point<Pixels>,
        modifiers: &Modifiers,
    ) {
        let Some((x, y)) = self.cell_coords_from_window_pos(pos) else {
            return;
        };

        let mut term = self.term.lock();

        let _ = term.mouse_event(WezMouseEvent {
            kind,
            x,
            y,
            x_pixel_offset: 0,
            y_pixel_offset: 0,
            button,
            modifiers: Self::map_modifiers(modifiers),
        });
    }

    fn write_to_pty(&self, bytes: &[u8]) {
        let mut w = self.writer.clone();
        let _ = w.write_all(bytes);
        let _ = w.flush();
    }

    fn url_from_position(
        &self,
        window_pos: Point<Pixels>,
    ) -> Option<(String, RangeInclusive<GridPoint>)> {
        if !self.content.terminal_bounds.bounds.contains(&window_pos) {
            return None;
        }

        let local = window_pos - self.content.terminal_bounds.bounds.origin;
        let col = (local.x / self.content.terminal_bounds.cell_width()) as usize;
        let row = (local.y / self.content.terminal_bounds.line_height()) as usize;

        let term = self.term.lock();
        let screen = term.screen();
        let plan = self.compute_viewport_plan(screen);
        let cols = plan.cols;
        let rows = plan.rows;

        let row = row.min(rows.saturating_sub(1));
        let col = col.min(cols.saturating_sub(1));

        let stable_row = plan
            .viewport_top_stable
            .saturating_add(row as StableRowIndex);
        let phys_range = screen.stable_range(&(stable_row..stable_row.saturating_add(1)));
        let line = screen.lines_in_phys_range(phys_range).into_iter().next()?;
        let chars = backends::collect_line_chars(
            cols,
            line.visible_cells().map(|cellref| {
                let idx = cellref.cell_index();
                let ch = cellref.str().chars().next().unwrap_or(' ');
                (idx, ch)
            }),
        );

        let line_coord = row as i32 - plan.display_offset as i32;
        backends::hovered_url_from_line_chars(&chars, col, line_coord)
    }

    fn update_hover_target(&mut self, pos: Point<Pixels>, cx: &mut Context<WidgetTerminal>) {
        let hovered = self.url_from_position(pos);

        if let Some(target) =
            backends::update_hovered_word(&mut self.content.last_hovered_word, hovered)
        {
            cx.emit(Event::NewNavigationTarget(target));
        }
    }

    fn snapshot_cursor_shape(
        default_cursor_shape: CursorShape,
        cursor_shape: WezCursorShape,
        cursor_visible: bool,
        display_offset: usize,
    ) -> CursorRenderShape {
        let cursor_shape = match cursor_shape {
            WezCursorShape::Default => match default_cursor_shape {
                CursorShape::Block => CursorRenderShape::Block,
                CursorShape::Underline => CursorRenderShape::Underline,
                CursorShape::Bar => CursorRenderShape::Bar,
                CursorShape::Hollow => CursorRenderShape::Hollow,
            },
            WezCursorShape::BlinkingBlock | WezCursorShape::SteadyBlock => CursorRenderShape::Block,
            WezCursorShape::BlinkingUnderline | WezCursorShape::SteadyUnderline => {
                CursorRenderShape::Underline
            }
            WezCursorShape::BlinkingBar | WezCursorShape::SteadyBar => CursorRenderShape::Bar,
        };

        if display_offset == 0 && cursor_visible {
            cursor_shape
        } else {
            CursorRenderShape::Hidden
        }
    }

    fn snapshot_terminal_mode(term: &Terminal) -> TerminalMode {
        let mut mode = TerminalMode::empty();
        if term.is_alt_screen_active() {
            mode |= TerminalMode::ALT_SCREEN;
            mode |= TerminalMode::ALTERNATE_SCROLL;
        }
        if term.bracketed_paste_enabled() {
            mode |= TerminalMode::BRACKETED_PASTE;
        }
        if term.is_mouse_grabbed() {
            mode |= TerminalMode::MOUSE_MODE;
        }
        mode
    }

    fn snapshot_cells_and_cursor_char(
        screen: &Screen,
        plan: &ViewportPlan,
        cols: usize,
        rows: usize,
        cursor_row: Option<usize>,
        cursor_col: usize,
        cursor_hidden: bool,
    ) -> (Vec<crate::IndexedCell>, char) {
        let lines = screen.lines_in_phys_range(plan.phys_range.clone());

        let mut cursor_char = ' ';
        let mut cells = Vec::with_capacity(cols * rows);
        for (r, line) in lines.iter().enumerate() {
            let row_wrapped = line.last_cell_was_wrapped();
            let mut row_cells: Vec<Option<(WezCell, usize)>> = vec![None; cols];
            for cellref in line.visible_cells() {
                let idx = cellref.cell_index();
                if idx < cols {
                    row_cells[idx] = Some((cellref.as_cell(), cellref.width()));
                }
            }

            let mut skip = 0usize;
            // When synthesizing spacer cells for wide glyphs, preserve the original cell style
            // so background runs (e.g. bracketed-paste highlight/inverse) cover the full width.
            let mut wide_spacer_style: Option<(TermColor, TermColor, CellFlags)> = None;
            for (c, slot) in row_cells.iter().enumerate() {
                let point = GridPoint::new(r as i32 - plan.display_offset as i32, c);

                if skip > 0 {
                    skip -= 1;
                    let (fg, bg, flags) = wide_spacer_style.unwrap_or((
                        TermColor::Named(NamedColor::Foreground),
                        TermColor::Named(NamedColor::Background),
                        CellFlags::empty(),
                    ));
                    let mut flags = flags | CellFlags::WIDE_CHAR_SPACER;
                    if row_wrapped && c + 1 == cols {
                        flags |= CellFlags::WRAPLINE;
                    }
                    cells.push(crate::IndexedCell {
                        point,
                        cell: Cell {
                            c: ' ',
                            fg,
                            bg,
                            flags,
                            hyperlink: None,
                            zerowidth: Vec::new(),
                        },
                    });
                    if skip == 0 {
                        wide_spacer_style = None;
                    }
                    continue;
                }

                let (wcell, width) = slot.clone().unwrap_or_else(|| (WezCell::blank(), 1));
                let mut mapped = map_cell(&wcell);
                if row_wrapped && c + 1 == cols {
                    mapped.flags |= CellFlags::WRAPLINE;
                }
                if width > 1 {
                    skip = width.saturating_sub(1);
                    wide_spacer_style = Some((mapped.fg, mapped.bg, mapped.flags));
                }

                if !cursor_hidden && cursor_row == Some(r) && cursor_col == c {
                    cursor_char = mapped.c;
                }

                cells.push(crate::IndexedCell {
                    point,
                    cell: mapped,
                });
            }
        }

        Self::pad_snapshot_cells(&mut cells, cols, rows, plan.display_offset);
        (cells, cursor_char)
    }

    fn pad_snapshot_cells(
        cells: &mut Vec<crate::IndexedCell>,
        cols: usize,
        rows: usize,
        display_offset: usize,
    ) {
        while cells.len() < cols * rows {
            let idx = cells.len();
            let r = idx / cols;
            let c = idx % cols;
            cells.push(crate::IndexedCell {
                point: GridPoint::new(r as i32 - display_offset as i32, c),
                cell: Cell {
                    c: ' ',
                    fg: TermColor::Named(NamedColor::Foreground),
                    bg: TermColor::Named(NamedColor::Background),
                    flags: CellFlags::empty(),
                    hyperlink: None,
                    zerowidth: Vec::new(),
                },
            });
        }
    }

    fn snapshot(&mut self) {
        let term = self.term.lock();
        let screen = term.screen();
        let plan = self.compute_viewport_plan(screen);
        // Keep the stored scroll position normalized to what we actually rendered.
        self.viewport_top_stable = plan.normalized_top_stable;
        let cols = plan.cols;
        let rows = plan.rows;

        let cursor = term.cursor_pos();
        let cursor_visible = cursor.visibility == CursorVisibility::Visible;
        let cursor_shape = Self::snapshot_cursor_shape(
            self.default_cursor_shape,
            cursor.shape,
            cursor_visible,
            plan.display_offset,
        );

        let cursor_row: Option<usize> = cursor.y.try_into().ok();
        let cursor_col: usize = cursor.x;

        let mut out = TerminalContent {
            terminal_bounds: self.content.terminal_bounds,
            display_offset: plan.display_offset,
            mode: Self::snapshot_terminal_mode(&term),
            selection: self.selection.range.clone(),
            scrolled_to_bottom: plan.display_offset == 0,
            scrolled_to_top: plan.scrolled_to_top,
            cursor: Cursor {
                shape: cursor_shape,
                point: GridPoint::new(cursor.y as i32 - plan.display_offset as i32, cursor.x),
            },
            cursor_char: ' ',
            last_hovered_word: self.content.last_hovered_word.clone(),
            ..TerminalContent::default()
        };

        let cursor_hidden = out.cursor.shape == CursorRenderShape::Hidden;
        let (cells, cursor_char) = Self::snapshot_cells_and_cursor_char(
            screen,
            &plan,
            cols,
            rows,
            cursor_row,
            cursor_col,
            cursor_hidden,
        );
        out.cells = cells;
        out.cursor_char = cursor_char;
        self.content = out;
    }

    fn sync_search_matches(&mut self) {
        let search = &mut self.search;
        backends::sync_search_matches(
            &mut search.dirty,
            search.query.as_deref(),
            &mut search.matches,
            &mut search.active_match,
            |q| {
                let term = self.term.lock();
                let matches = Self::compute_search_matches(&term, q);
                drop(term);
                matches
            },
        );
    }

    fn refresh_selection_text(&mut self) -> Option<String> {
        let selection_text = self.selection.range.as_ref().and_then(|sel| {
            let txt = self.selection_to_string(sel);
            (!txt.is_empty()).then_some(txt)
        });
        self.content.selection_text = selection_text.clone();
        selection_text
    }
}

#[derive(Clone, Debug)]
struct ViewportPlan {
    cols: usize,
    rows: usize,
    top_stable: StableRowIndex,
    base_stable: StableRowIndex,
    viewport_top_stable: StableRowIndex,
    /// The normalized scroll state to persist back into the backend.
    normalized_top_stable: Option<StableRowIndex>,
    display_offset: usize,
    phys_range: std::ops::Range<PhysRowIndex>,
    scrolled_to_top: bool,
}

fn compute_viewport_plan(screen: &Screen, requested_top: Option<StableRowIndex>) -> ViewportPlan {
    let cols = screen.physical_cols.max(1);
    let rows = screen.physical_rows.max(1);

    // Stable index of the topmost line we still have in the scrollback.
    let top_stable = screen.phys_to_stable_row_index(0);
    // Stable index of the first visible row when following the live view.
    let base_phys = screen.phys_row(0);
    let base_stable = screen.phys_to_stable_row_index(base_phys);

    let mut viewport_top_stable = requested_top.unwrap_or(base_stable);
    if viewport_top_stable < top_stable {
        viewport_top_stable = top_stable;
    }
    if viewport_top_stable >= base_stable {
        viewport_top_stable = base_stable;
    }

    let normalized_top_stable = if viewport_top_stable >= base_stable {
        None
    } else {
        Some(viewport_top_stable)
    };

    let display_offset = (base_stable - viewport_top_stable).max(0) as usize;
    let end_stable = viewport_top_stable.saturating_add(rows as StableRowIndex);
    let stable_range = viewport_top_stable..end_stable;
    let phys_range = screen.stable_range(&stable_range);

    ViewportPlan {
        cols,
        rows,
        top_stable,
        base_stable,
        viewport_top_stable,
        normalized_top_stable,
        display_offset,
        phys_range,
        scrolled_to_top: viewport_top_stable == top_stable,
    }
}

fn srgb_f32_to_u8(v: f32) -> u8 {
    // SrgbaTuple is in 0.0..=1.0; clamp and map to 0..=255.
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn map_color(attr: ColorAttribute, is_bg: bool) -> TermColor {
    match attr {
        ColorAttribute::Default => TermColor::Named(if is_bg {
            NamedColor::Background
        } else {
            NamedColor::Foreground
        }),
        ColorAttribute::PaletteIndex(idx) => TermColor::Indexed(idx),
        ColorAttribute::TrueColorWithPaletteFallback(rgb, _)
        | ColorAttribute::TrueColorWithDefaultFallback(rgb) => TermColor::Rgb(
            srgb_f32_to_u8(rgb.0),
            srgb_f32_to_u8(rgb.1),
            srgb_f32_to_u8(rgb.2),
        ),
    }
}

fn map_flags(attrs: &CellAttributes) -> CellFlags {
    let mut flags = CellFlags::empty();
    if matches!(attrs.intensity(), Intensity::Bold) {
        flags |= CellFlags::BOLD;
    }
    if attrs.italic() {
        flags |= CellFlags::ITALIC;
    }
    if attrs.underline() != Underline::None {
        flags |= CellFlags::UNDERLINE;
    }
    if attrs.strikethrough() {
        flags |= CellFlags::STRIKEOUT;
    }
    if attrs.reverse() {
        flags |= CellFlags::INVERSE;
    }
    flags
}

fn map_cell(cell: &WezCell) -> Cell {
    let attrs = cell.attrs();
    let mut out = Cell {
        c: cell.str().chars().next().unwrap_or(' '),
        fg: map_color(attrs.foreground(), false),
        bg: map_color(attrs.background(), true),
        flags: map_flags(attrs),
        hyperlink: attrs.hyperlink().map(|h| h.uri().to_string()),
        zerowidth: Vec::new(),
    };

    // Capture additional grapheme content as zerowidth so shaping can render it.
    let mut chars = cell.str().chars();
    let _ = chars.next();
    let rest: Vec<char> = chars.collect();
    if !rest.is_empty() {
        out.zerowidth = rest;
    }

    out
}

fn spawn_pty_reader_thread(mut reader: Box<dyn Read + Send>, tx: Sender<WezEvent>) {
    thread::spawn(move || {
        let mut buf = [0u8; 16 * 1024];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => {
                    let _ = tx.send_blocking(WezEvent::PtyExit);
                    return;
                }
                Ok(n) => {
                    let _ = tx.send_blocking(WezEvent::PtyRead(buf[..n].to_vec()));
                }
                Err(_) => {
                    let _ = tx.send_blocking(WezEvent::PtyExit);
                    return;
                }
            }
        }
    });
}

fn spawn_child_wait_thread(mut child: Box<dyn portable_pty::Child + Send>, tx: Sender<WezEvent>) {
    thread::spawn(move || {
        let _ = child.wait();
        let _ = tx.send_blocking(WezEvent::PtyExit);
    });
}

fn default_shell_command_candidates() -> Vec<CommandBuilder> {
    #[cfg(windows)]
    {
        let mut candidates = Vec::with_capacity(4);

        // Git Bash / MSYS2 often set `SHELL` to a working path on Windows.
        if let Ok(shell) = std::env::var("SHELL") {
            candidates.push(CommandBuilder::new(shell));
        }

        // Prefer PowerShell (Core if present) before falling back to cmd.exe.
        candidates.push(CommandBuilder::new("pwsh"));
        candidates.push(CommandBuilder::new("powershell"));

        let comspec = std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string());
        candidates.push(CommandBuilder::new(comspec));

        return candidates;
    }

    #[cfg(not(windows))]
    {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
        vec![CommandBuilder::new(shell)]
    }
}

fn shell_command_candidates_for_local_env(
    env: &std::collections::HashMap<String, String>,
) -> Vec<CommandBuilder> {
    let mut candidates = Vec::new();

    if let Some(shell) = crate::shell::pick_shell_program_from_env(env) {
        let args = crate::shell::shell_integration_args_for_env(shell, env);
        if args.is_empty() {
            candidates.push(CommandBuilder::new(shell));
        } else {
            let mut cmd = CommandBuilder::new(shell);
            cmd.args(args);
            candidates.push(cmd);
        }
    }

    for cmd in default_shell_command_candidates() {
        let Some(argv0) = cmd.get_argv().first() else {
            continue;
        };
        if candidates
            .first()
            .and_then(|c| c.get_argv().first())
            .is_some_and(|first| first == argv0)
        {
            continue;
        }
        candidates.push(cmd);
    }

    candidates
}
impl TerminalBackend for WezTermBackend {
    fn backend_name(&self) -> &'static str {
        "wezterm"
    }

    fn sftp(&self) -> Option<wezterm_ssh::Sftp> {
        self.sftp.clone()
    }

    fn current_dir(&self) -> Option<String> {
        let term = self.term.lock();
        current_dir_path_from_term(&term)
    }

    fn handle_backend_event(
        &mut self,
        event: Box<dyn std::any::Any + Send>,
        cx: &mut Context<WidgetTerminal>,
    ) {
        if let Ok(event) = event.downcast::<WezEvent>() {
            match *event {
                WezEvent::PtyRead(bytes) => {
                    if let Some(sender) = self.record.sender.as_ref() {
                        sender.output(&bytes);
                    }

                    let mut term = self.term.lock();
                    let completions = self.osc.push_with_offsets(&bytes);

                    let mut prev = 0usize;
                    for (end, ev) in completions {
                        let end = end.min(bytes.len());
                        if end > prev {
                            term.advance_bytes(&bytes[prev..end]);
                        }

                        let (cursor_line, cursor_x) = cursor_stable_line(&term);
                        let OscEvent::Osc133(payload) = ev;
                        let mut line = adjust_osc133_boundary(&payload, cursor_line, cursor_x);
                        let command = if payload.trim_start().starts_with('C') {
                            match wrapped_command_span_for_stable_row(term.screen(), line) {
                                Some((start_line, command)) => {
                                    line = start_line;
                                    Some(command)
                                }
                                None => stable_row_text(term.screen(), line),
                            }
                        } else {
                            None
                        };
                        self.blocks
                            .apply_osc133(&payload, Instant::now(), line, command);
                        prev = end;
                    }

                    if prev < bytes.len() {
                        term.advance_bytes(&bytes[prev..]);
                    }
                    self.search.dirty = true;
                    cx.emit(Event::Wakeup);
                }
                WezEvent::PtyExit => {
                    if self.exited {
                        return;
                    }
                    self.exited = true;
                    // Ensure any active cast recording flushes and closes on PTY exit, even if the
                    // terminal view remains open to show the final buffer.
                    self.stop_cast_recording();
                    if let Some(f) = self.exit_fn.as_ref() {
                        f(cx);
                    } else {
                        cx.emit(Event::CloseTerminal);
                    }
                }
                WezEvent::Alert(alert) => match alert {
                    Alert::Bell => cx.emit(Event::Bell),
                    Alert::WindowTitleChanged(_)
                    | Alert::TabTitleChanged(_)
                    | Alert::IconTitleChanged(_) => cx.emit(Event::TitleChanged),
                    _ => {}
                },
            }
        }
    }

    fn command_blocks(&self) -> Option<Vec<crate::command_blocks::CommandBlock>> {
        Some(self.blocks.blocks())
    }

    fn stable_row_for_grid_line(&self, line: i32) -> Option<i64> {
        let term = self.term.lock();
        Self::stable_row_for_grid_line_on_screen(term.screen(), line)
    }

    fn grid_line_for_stable_row(&self, stable_row: i64) -> Option<i32> {
        let term = self.term.lock();
        Self::grid_line_for_stable_row_on_screen(term.screen(), stable_row)
    }

    fn set_selection_range(&mut self, range: Option<crate::SelectionRange>) {
        self.selection.command_block_id = range
            .as_ref()
            .and_then(|range| self.command_block_id_for_selection_range(range));
        self.selection.range = range;
        self.selection.selecting = false;
        self.selection.anchor = None;
    }

    fn shutdown(&mut self, policy: TerminalShutdownPolicy, _cx: &mut Context<WidgetTerminal>) {
        if self.exited {
            return;
        }

        log::debug!("gpui_term[wezterm]: shutdown requested: policy={policy:?}");

        // Best-effort graceful exit: close the PTY writer to deliver EOF to the slave side.
        {
            // Drop the real PTY writer. `portable_pty` documents that dropping the master writer
            // sends EOF to the slave end.
            let mut inner = self.writer.inner.lock();
            *inner = Box::new(std::io::sink());
        }

        let now = Instant::now();
        self.shutdown.requested_at.get_or_insert(now);

        let requested_kill_after = match policy {
            TerminalShutdownPolicy::Graceful => None,
            TerminalShutdownPolicy::GracefulThenKill(d) => Some(d),
            TerminalShutdownPolicy::Kill => Some(Duration::ZERO),
        };

        if let Some(new_kill_after) = requested_kill_after {
            self.shutdown.kill_after = Some(match self.shutdown.kill_after {
                Some(existing) => existing.min(new_kill_after),
                None => new_kill_after,
            });
        }

        // For `Kill` we do the kill immediately (and also leave the shutdown timer in place so
        // future sync ticks don't repeat it).
        if matches!(policy, TerminalShutdownPolicy::Kill) && !self.shutdown.kill_sent {
            let _ = self.child_killer.kill();
            self.shutdown.kill_sent = true;
        }
    }

    fn sync(&mut self, _window: &mut Window, cx: &mut Context<WidgetTerminal>) {
        poll_shutdown(
            &mut self.shutdown,
            Instant::now(),
            self.exited,
            self.child_killer.as_mut(),
        );

        let mut copy_request: Option<Option<bool>> = None;
        while let Some(op) = self.pending_ops.pop_front() {
            match op {
                TermOp::Resize(bounds) => {
                    self.content.terminal_bounds = bounds;
                    let rows = bounds.num_lines().max(1);
                    let cols = bounds.num_columns().max(1);
                    if let Some(sender) = self.record.sender.as_ref() {
                        sender.resize(cols, rows);
                    }
                    let pixel_width_u16 =
                        u32::from(bounds.width().ceil()).min(u16::MAX as u32) as u16;
                    let pixel_height_u16 =
                        u32::from(bounds.height().ceil()).min(u16::MAX as u32) as u16;
                    let _ = self.master.resize(PtySize {
                        rows: rows as u16,
                        cols: cols as u16,
                        pixel_width: pixel_width_u16,
                        pixel_height: pixel_height_u16,
                    });

                    let mut term = self.term.lock();
                    term.resize(TerminalSize {
                        rows,
                        cols,
                        pixel_width: usize::from(bounds.width().ceil()),
                        pixel_height: usize::from(bounds.height().ceil()),
                        dpi: 0,
                    });
                    let cursor_stable = cursor_stable_line(&term).0;
                    let lines = stable_row_texts(term.screen());
                    self.blocks.remap_after_rewrap(&lines, cursor_stable);
                    let remapped_selection =
                        self.remapped_selected_command_block_selection_range(term.screen(), cols);
                    drop(term);
                    if let Some(range) = remapped_selection {
                        self.selection.range = Some(range);
                        self.selection.selecting = false;
                        self.selection.anchor = None;
                    }
                    self.search.dirty = true;
                }
                TermOp::Clear => {
                    let mut term = self.term.lock();
                    term.erase_scrollback_and_viewport();
                    self.viewport_top_stable = None;
                    self.search.dirty = true;
                    cx.emit(Event::Wakeup);
                }
                TermOp::Scroll(delta) => {
                    let term = self.term.lock();
                    let plan = self.compute_viewport_plan(term.screen());
                    let mut top = plan.viewport_top_stable;
                    if delta > 0 {
                        top = top.saturating_sub(delta as StableRowIndex);
                    } else {
                        top = top.saturating_add((-delta) as StableRowIndex);
                    }

                    if top < plan.top_stable {
                        top = plan.top_stable;
                    }
                    if top >= plan.base_stable {
                        self.viewport_top_stable = None;
                    } else {
                        self.viewport_top_stable = Some(top);
                    }
                }
                TermOp::Copy(keep) => {
                    copy_request = Some(keep);
                }
            }
        }

        self.sync_search_matches();

        self.snapshot();

        let selection_text = self.refresh_selection_text();

        if copy_request.is_some()
            && let Some(txt) = selection_text
        {
            crate::terminal::write_clipboard(cx, txt);
        }
    }

    fn last_content(&self) -> &TerminalContent {
        &self.content
    }

    fn matches(&self) -> &[RangeInclusive<GridPoint>] {
        &self.search.matches
    }

    fn active_match_index(&self) -> Option<usize> {
        self.search.active_match
    }

    fn last_clicked_line(&self) -> Option<i32> {
        self.last_clicked_line
    }

    fn vi_mode_enabled(&self) -> bool {
        false
    }

    fn mouse_mode(&self, shift: bool) -> bool {
        self.term_mouse_grabbed() && !shift
    }

    fn selection_started(&self) -> bool {
        self.selection.selecting
    }

    fn clear_selection(&mut self) {
        self.selection.selecting = false;
        self.selection.anchor = None;
        self.selection.range = None;
        self.selection.command_block_id = None;
    }

    fn set_cursor_shape(&mut self, cursor_shape: CursorShape) {
        self.default_cursor_shape = cursor_shape;
    }

    fn total_lines(&self) -> usize {
        self.term.lock().screen().scrollback_rows()
    }

    fn viewport_lines(&self) -> usize {
        self.term.lock().screen().physical_rows
    }

    fn logical_line_numbers_from_top(&self, start_line: usize, count: usize) -> Vec<Option<usize>> {
        if count == 0 {
            return Vec::new();
        }

        let term = self.term.lock();
        let screen = term.screen();
        let total = screen.scrollback_rows();
        if total == 0 {
            return Vec::new();
        }

        let start = start_line.min(total);
        let end = start.saturating_add(count).min(total);
        if start == end {
            return Vec::new();
        }

        // WezTerm exposes wrap information per physical line via `last_cell_was_wrapped()`.
        // A physical row is a continuation if the previous row was wrapped.
        let lines = screen.lines_in_phys_range(0..end);
        crate::terminal::logical_line_numbers_from_wraps(total, start, count, |row| {
            lines
                .get(row)
                .map(|line| line.last_cell_was_wrapped())
                .unwrap_or(false)
        })
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
            let term = self.term.lock();
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
        let rows = self.content.terminal_bounds.num_lines().max(1);
        let last_col = self.content.terminal_bounds.last_column();
        let top = -(self.content.display_offset as i32);
        let bottom = top + rows.saturating_sub(1) as i32;

        self.selection.range = Some(crate::SelectionRange {
            start: GridPoint::new(top, 0),
            end: GridPoint::new(bottom, last_col),
        });
        self.selection.selecting = false;
        self.selection.anchor = None;
        self.selection.command_block_id = None;
    }

    fn copy(&mut self, keep_selection: Option<bool>, _cx: &mut Context<WidgetTerminal>) {
        self.pending_ops.push_back(TermOp::Copy(keep_selection));
    }

    fn clear(&mut self) {
        self.pending_ops.push_back(TermOp::Clear);
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
        let page = self.content.terminal_bounds.num_lines().max(1);
        self.queue_scroll_delta(page.saturating_sub(1) as i32);
    }

    fn scroll_page_down(&mut self) {
        let page = self.content.terminal_bounds.num_lines().max(1);
        self.queue_scroll_delta(-(page.saturating_sub(1) as i32));
    }

    fn scroll_to_top(&mut self) {
        // Use an intentionally-low stable index; it will be clamped to the actual
        // top of the scrollback during snapshotting.
        self.viewport_top_stable = Some(0);
    }

    fn scroll_to_bottom(&mut self) {
        self.viewport_top_stable = None;
    }

    fn scrolled_to_top(&self) -> bool {
        self.content.scrolled_to_top
    }

    fn scrolled_to_bottom(&self) -> bool {
        self.content.scrolled_to_bottom
    }

    fn set_size(&mut self, new_bounds: TerminalBounds) {
        // Avoid re-sending redundant resize operations every layout tick; it is expensive and it
        // also interferes with search state (a resize marks search as dirty and can reset the
        // active match selection).
        if self.content.terminal_bounds != new_bounds {
            self.pending_ops.push_back(TermOp::Resize(new_bounds));
        }
    }

    fn input(&mut self, input: Cow<'static, [u8]>) {
        // Typing while scrolled back should snap back to the live view.
        self.viewport_top_stable = None;
        self.selection.selecting = false;
        self.selection.anchor = None;
        self.selection.range = None;
        self.write_to_pty(&input);
    }

    fn paste(&mut self, text: &str) {
        let mut term = self.term.lock();
        let _ = term.send_paste(text);
    }

    fn tail_text(&self, max_lines: usize) -> Option<String> {
        let max_lines = max_lines.max(1);
        let term = self.term.lock();
        let screen = term.screen();

        let total = screen.scrollback_rows();
        if total == 0 {
            return None;
        }

        let start = total.saturating_sub(max_lines);
        let lines = screen.lines_in_phys_range(start..total);

        let mut out = String::new();
        for (ix, line) in lines.iter().enumerate() {
            if ix > 0 {
                out.push('\n');
            }
            out.push_str(line.as_str().trim_end());
        }
        Some(out)
    }

    fn text_for_lines(&self, start_line: i64, end_line: i64) -> Option<String> {
        let term = self.term.lock();
        let screen = term.screen();

        let total = screen.scrollback_rows();
        if total == 0 {
            return None;
        }

        let top_stable = screen.phys_to_stable_row_index(0);
        let bottom_stable = screen.phys_to_stable_row_index(total.saturating_sub(1));

        let start_line = usize::try_from(start_line).ok()?;
        let end_line = usize::try_from(end_line).ok()?;
        let mut start = start_line.min(end_line) as StableRowIndex;
        let mut end = start_line.max(end_line) as StableRowIndex;

        if start < top_stable {
            start = top_stable;
        }
        if end > bottom_stable {
            end = bottom_stable;
        }
        if start > end {
            return None;
        }

        let end_exclusive = end.saturating_add(1);
        let phys_range = screen.stable_range(&(start..end_exclusive));
        let lines = screen.lines_in_phys_range(phys_range.start..phys_range.end);

        let mut out = String::new();
        for (ix, line) in lines.iter().enumerate() {
            if ix > 0 {
                out.push('\n');
            }
            out.push_str(line.as_str().trim_end());
        }
        Some(out)
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
        let mut term = self.term.lock();
        term.focus_changed(true);
    }

    fn focus_out(&mut self) {
        let mut term = self.term.lock();
        term.focus_changed(false);
    }
    fn toggle_vi_mode(&mut self) {}

    fn try_keystroke(&mut self, keystroke: &Keystroke, _alt_is_meta: bool) -> bool {
        if let Some((key, mods)) = map_keystroke(keystroke) {
            let mut term = self.term.lock();
            let _ = term.key_down(key, mods);
            return true;
        }
        false
    }

    fn try_modifiers_change(
        &mut self,
        modifiers: &Modifiers,
        window: &Window,
        cx: &mut Context<WidgetTerminal>,
    ) {
        if modifiers.secondary() {
            self.update_hover_target(window.mouse_position(), cx);
        } else if let Some(target) =
            backends::update_hovered_word(&mut self.content.last_hovered_word, None)
        {
            cx.emit(Event::NewNavigationTarget(target));
        }
    }

    fn mouse_move(&mut self, e: &MouseMoveEvent, cx: &mut Context<WidgetTerminal>) {
        self.last_mouse_pos = Some(e.position);
        if e.modifiers.secondary() {
            self.update_hover_target(e.position, cx);
        } else if let Some(target) =
            backends::update_hovered_word(&mut self.content.last_hovered_word, None)
        {
            cx.emit(Event::NewNavigationTarget(target));
        }

        if self.mouse_mode(e.modifiers.shift) && !e.modifiers.secondary() {
            self.send_mouse_event(
                MouseEventKind::Move,
                WezMouseButton::None,
                e.position,
                &e.modifiers,
            );
        }
    }

    fn select_word_at_event_position(&mut self, e: &MouseDownEvent) {
        if self.mouse_mode(e.modifiers.shift) || e.modifiers.secondary() {
            return;
        }

        if e.click_count >= 3 {
            self.select_line_at_event_position(e);
            return;
        }

        let Some(p) = self.selection_point_from_window_pos(e.position) else {
            return;
        };

        let cols = self.content.terminal_bounds.num_columns().max(1);
        let rows = self.content.terminal_bounds.num_lines().max(1);
        let viewport_row_i32 = p.line + self.content.display_offset as i32;
        if viewport_row_i32 < 0 {
            return;
        }
        let viewport_row = viewport_row_i32 as usize;
        if viewport_row >= rows {
            return;
        }

        let mut line_chars: Vec<Option<char>> = vec![None; cols];
        let base = viewport_row * cols;
        let end = (base + cols).min(self.content.cells.len());
        for (col, ic) in self.content.cells[base..end].iter().enumerate() {
            if ic.cell.flags.contains(CellFlags::WIDE_CHAR_SPACER) {
                continue;
            }
            line_chars[col] = Some(ic.cell.c);
        }

        let col = p.column.min(cols.saturating_sub(1));
        let is_sep = |c: Option<char>| matches!(c, Some(ch) if ch.is_whitespace());
        if is_sep(line_chars[col]) {
            return;
        }

        let mut start = col;
        while start > 0 && !is_sep(line_chars[start - 1]) {
            start -= 1;
        }
        let mut end = col;
        while end + 1 < cols && !is_sep(line_chars[end + 1]) {
            end += 1;
        }

        self.selection.range = Some(crate::SelectionRange {
            start: GridPoint::new(p.line, start),
            end: GridPoint::new(p.line, end),
        });
        self.selection.selecting = false;
        self.selection.anchor = None;
        self.selection.command_block_id = None;
    }

    fn mouse_drag(
        &mut self,
        e: &MouseMoveEvent,
        region: Bounds<Pixels>,
        cx: &mut Context<WidgetTerminal>,
    ) {
        self.last_mouse_pos = Some(e.position);
        if self.selection.selecting
            && !self.mouse_mode(e.modifiers.shift)
            && !e.modifiers.secondary()
        {
            // While selecting, keep updating the selection even when the cursor leaves the
            // terminal bounds, and scroll when dragging beyond the top/bottom edges.
            let term = self.term.lock();
            let screen = term.screen();

            // Apply drag-scroll first so the selection point calculation matches the updated
            // viewport.
            if let Some(delta) = backends::drag_line_delta(
                e.position,
                region,
                self.content.terminal_bounds.line_height(),
            ) {
                let plan = self.compute_viewport_plan(screen);
                let mut top = plan.viewport_top_stable;
                if delta > 0 {
                    top = top.saturating_sub(delta as StableRowIndex);
                } else {
                    top = top.saturating_add((-delta) as StableRowIndex);
                }

                if top < plan.top_stable {
                    top = plan.top_stable;
                }
                if top >= plan.base_stable {
                    self.viewport_top_stable = None;
                } else {
                    self.viewport_top_stable = Some(top);
                }
            }

            let plan = self.compute_viewport_plan(screen);
            if let Some(anchor) = self.selection.anchor {
                let Some(pos) = self.clamp_pos_to_terminal_bounds(e.position) else {
                    return;
                };

                let bounds = self.content.terminal_bounds.bounds;
                let local = pos - bounds.origin;
                let col = (local.x / self.content.terminal_bounds.cell_width()) as usize;
                let row = (local.y / self.content.terminal_bounds.line_height()) as usize;
                let row = row.min(plan.rows.saturating_sub(1));
                let col = col.min(plan.cols.saturating_sub(1));

                let stable_row = plan
                    .viewport_top_stable
                    .saturating_add(row as StableRowIndex);
                let line = stable_row as i64 - plan.base_stable as i64;
                let line = line.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
                let p = GridPoint::new(line, col);

                self.selection.range = Some(Self::normalize_selection(anchor, p));
                self.selection.command_block_id = None;
                cx.emit(Event::SelectionsChanged);
            }
            return;
        }
        if self.mouse_mode(e.modifiers.shift) && !e.modifiers.secondary() {
            self.send_mouse_event(
                MouseEventKind::Move,
                WezMouseButton::None,
                e.position,
                &e.modifiers,
            );
        }
    }

    fn mouse_down(&mut self, e: &MouseDownEvent, cx: &mut Context<WidgetTerminal>) {
        self.last_mouse_pos = Some(e.position);
        if e.button == gpui::MouseButton::Left && e.modifiers.secondary() {
            self.update_hover_target(e.position, cx);
            if let Some(hovered) = self.content.last_hovered_word.as_ref() {
                cx.emit(Event::Open(hovered.word.clone()));
                return;
            }
        }

        if e.modifiers.secondary() {
            return;
        }

        if !self.mouse_mode(e.modifiers.shift) {
            if e.button == gpui::MouseButton::Left && e.click_count >= 2 {
                // Double-click selects a word, matching typical terminal behavior.
                // Triple-click selects the current line.
                self.select_word_at_event_position(e);
                cx.emit(Event::SelectionsChanged);
                return;
            }
            if let (gpui::MouseButton::Left, Some(p)) =
                (e.button, self.selection_point_from_window_pos(e.position))
            {
                self.selection.selecting = true;
                self.selection.anchor = Some(p);
                self.selection.range = Some(crate::SelectionRange { start: p, end: p });
                self.selection.command_block_id = None;
                cx.emit(Event::SelectionsChanged);
            }
            return;
        }

        let button = match e.button {
            gpui::MouseButton::Left => WezMouseButton::Left,
            gpui::MouseButton::Middle => WezMouseButton::Middle,
            gpui::MouseButton::Right => WezMouseButton::Right,
            _ => WezMouseButton::None,
        };

        self.send_mouse_event(MouseEventKind::Press, button, e.position, &e.modifiers);
    }

    fn mouse_up(&mut self, e: &MouseUpEvent, cx: &Context<WidgetTerminal>) {
        self.last_mouse_pos = Some(e.position);
        if self.selection.selecting && e.button == gpui::MouseButton::Left {
            self.selection.selecting = false;
            self.selection.anchor = None;

            if TerminalSettings::global(cx).copy_on_select && self.selection.range.is_some() {
                self.pending_ops.push_back(TermOp::Copy(Some(true)));
            }
            return;
        }

        if !self.mouse_mode(e.modifiers.shift) || e.modifiers.secondary() {
            return;
        }

        let button = match e.button {
            gpui::MouseButton::Left => WezMouseButton::Left,
            gpui::MouseButton::Middle => WezMouseButton::Middle,
            gpui::MouseButton::Right => WezMouseButton::Right,
            _ => WezMouseButton::None,
        };
        self.send_mouse_event(MouseEventKind::Release, button, e.position, &e.modifiers);
    }

    fn scroll_wheel(&mut self, e: &ScrollWheelEvent) {
        let mouse_mode = self.mouse_mode(e.modifiers.shift);

        let Some(scroll_lines) = backends::determine_scroll_lines(
            &mut self.scroll_px,
            e,
            self.content.terminal_bounds.line_height(),
            mouse_mode,
            self.content.terminal_bounds.height(),
        ) else {
            return;
        };
        if scroll_lines == 0 {
            return;
        }

        // WezTerm's terminal model will translate wheel events in the alternate screen into
        // cursor keys when mouse reporting is disabled (xterm-style alternateScroll).
        let alt_screen_active = self.term.lock().is_alt_screen_active();

        let forward_to_term = self.content.display_offset == 0
            && !e.modifiers.secondary()
            && (mouse_mode || (alt_screen_active && !e.modifiers.shift));

        if forward_to_term {
            let pos = self
                .last_mouse_pos
                .unwrap_or(self.content.terminal_bounds.bounds.center());
            let steps = scroll_lines.unsigned_abs() as usize;
            let button = if scroll_lines > 0 {
                WezMouseButton::WheelUp(1)
            } else {
                WezMouseButton::WheelDown(1)
            };
            for _ in 0..steps {
                self.send_mouse_event(MouseEventKind::Press, button, pos, &e.modifiers);
            }
        } else if !mouse_mode {
            self.queue_scroll_delta(scroll_lines);
        }
    }

    fn get_content(&self) -> String {
        let term = self.term.lock();
        let screen = term.screen();
        let total = screen.scrollback_rows();
        if total == 0 {
            return String::new();
        }

        let lines = screen.lines_in_phys_range(0..total);
        let mut out = String::new();
        for (i, line) in lines.iter().enumerate() {
            let s = line.as_str();
            let s = s.trim_end_matches(|c: char| c.is_whitespace());
            out.push_str(s);
            if i + 1 < lines.len() {
                out.push('\n');
            }
        }
        out
    }

    fn last_n_non_empty_lines(&self, n: usize) -> Vec<String> {
        if n == 0 {
            return Vec::new();
        }

        let term = self.term.lock();
        let screen = term.screen();
        let total = screen.scrollback_rows();
        if total == 0 {
            return Vec::new();
        }

        let mut out: Vec<String> = Vec::with_capacity(n);
        let mut end = total;
        while end > 0 && out.len() < n {
            // Fetch in chunks to avoid cloning the full scrollback when `total` is large.
            let start = end.saturating_sub(64);
            let chunk = screen.lines_in_phys_range(start..end);
            for line in chunk.into_iter().rev() {
                let s = line
                    .as_str()
                    .trim_end_matches(|c: char| c.is_whitespace())
                    .to_string();
                if s.is_empty() {
                    continue;
                }
                out.push(s);
                if out.len() >= n {
                    break;
                }
            }
            end = start;
        }

        out.reverse();
        out
    }

    fn preview_lines_from_top(&self, start_line: usize, count: usize) -> Vec<String> {
        if count == 0 {
            return Vec::new();
        }
        let term = self.term.lock();
        let screen = term.screen();
        let total = screen.scrollback_rows();
        if total == 0 {
            return Vec::new();
        }

        let start = start_line.min(total);
        let end = start.saturating_add(count).min(total);
        screen
            .lines_in_phys_range(start..end)
            .into_iter()
            .map(|line| {
                line.as_str()
                    .trim_end_matches(|c: char| c.is_whitespace())
                    .to_string()
            })
            .collect()
    }

    fn preview_cells_from_top(
        &self,
        start_line: usize,
        count: usize,
    ) -> (usize, usize, Vec<crate::IndexedCell>) {
        if count == 0 {
            return (0, 0, Vec::new());
        }

        let term = self.term.lock();
        let screen = term.screen();
        let total = screen.scrollback_rows();
        if total == 0 {
            return (screen.physical_cols.max(1), 0, Vec::new());
        }

        let cols = screen.physical_cols.max(1);
        let start = start_line.min(total);
        let end = start.saturating_add(count).min(total);
        let lines = screen.lines_in_phys_range(start..end);

        let mut cells = Vec::with_capacity(cols * lines.len().max(1));
        for (r, line) in lines.iter().enumerate() {
            let row_wrapped = line.last_cell_was_wrapped();
            let mut row_cells: Vec<Option<(WezCell, usize)>> = vec![None; cols];
            for cellref in line.visible_cells() {
                let idx = cellref.cell_index();
                if idx < cols {
                    row_cells[idx] = Some((cellref.as_cell(), cellref.width()));
                }
            }

            let mut skip = 0usize;
            let mut wide_spacer_style: Option<(TermColor, TermColor, CellFlags)> = None;
            for (c, slot) in row_cells.iter().enumerate() {
                let point = GridPoint::new(r as i32, c);

                if skip > 0 {
                    skip -= 1;
                    let (fg, bg, flags) = wide_spacer_style.unwrap_or((
                        TermColor::Named(NamedColor::Foreground),
                        TermColor::Named(NamedColor::Background),
                        CellFlags::empty(),
                    ));
                    let mut flags = flags | CellFlags::WIDE_CHAR_SPACER;
                    if row_wrapped && c + 1 == cols {
                        flags |= CellFlags::WRAPLINE;
                    }
                    cells.push(crate::IndexedCell {
                        point,
                        cell: Cell {
                            c: ' ',
                            fg,
                            bg,
                            flags,
                            hyperlink: None,
                            zerowidth: Vec::new(),
                        },
                    });
                    if skip == 0 {
                        wide_spacer_style = None;
                    }
                    continue;
                }

                let (wcell, width) = slot.clone().unwrap_or_else(|| (WezCell::blank(), 1));
                let mut mapped = map_cell(&wcell);
                if row_wrapped && c + 1 == cols {
                    mapped.flags |= CellFlags::WRAPLINE;
                }
                if width > 1 {
                    skip = width.saturating_sub(1);
                    wide_spacer_style = Some((mapped.fg, mapped.bg, mapped.flags));
                }

                cells.push(crate::IndexedCell {
                    point,
                    cell: mapped,
                });
            }
        }

        (cols, lines.len(), cells)
    }

    fn scrollback_top_line_id(&self) -> i64 {
        let term = self.term.lock();
        let screen = term.screen();
        screen.phys_to_stable_row_index(0) as i64
    }

    fn cursor_line_id(&self) -> Option<i64> {
        let term = self.term.lock();
        let screen = term.screen();
        let cursor = term.cursor_pos();
        let phys = screen.phys_row(cursor.y);
        Some(screen.phys_to_stable_row_index(phys) as i64)
    }
}

fn map_keystroke(keystroke: &Keystroke) -> Option<(KeyCode, KeyModifiers)> {
    let mut mods = KeyModifiers::default();
    if keystroke.modifiers.shift {
        mods |= KeyModifiers::SHIFT;
    }
    if keystroke.modifiers.control {
        mods |= KeyModifiers::CTRL;
    }
    if keystroke.modifiers.alt {
        mods |= KeyModifiers::ALT;
    }
    if keystroke.modifiers.platform {
        mods |= KeyModifiers::SUPER;
    }

    let key = match keystroke.key.as_ref() {
        "enter" => KeyCode::Enter,
        "escape" => KeyCode::Escape,
        "backspace" => KeyCode::Backspace,
        "tab" => KeyCode::Tab,
        "left" => KeyCode::LeftArrow,
        "right" => KeyCode::RightArrow,
        "up" => KeyCode::UpArrow,
        "down" => KeyCode::DownArrow,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" => KeyCode::PageUp,
        "pagedown" => KeyCode::PageDown,
        "delete" => KeyCode::Delete,
        "insert" => KeyCode::Insert,
        k if k.len() == 1 => {
            let ch = k.chars().next()?;
            KeyCode::Char(ch)
        }
        _ => return None,
    };

    Some((key, mods))
}

#[cfg(test)]
mod backpressure_tests {
    use std::{io::Read, sync::mpsc, time::Duration};

    use smol::channel;

    use super::{WezEvent, spawn_pty_reader_thread};

    struct NotifyingReader {
        remaining: usize,
        done: mpsc::Sender<()>,
    }

    impl Read for NotifyingReader {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.remaining == 0 {
                let _ = self.done.send(());
                return Ok(0);
            }

            self.remaining -= 1;
            if !buf.is_empty() {
                buf[0] = 0;
            }
            Ok(1)
        }
    }

    #[test]
    fn pty_reader_thread_should_not_run_ahead_of_event_consumer() {
        // This test encodes the desired behavior: if the UI isn't consuming backend events,
        // the PTY reader must eventually apply backpressure rather than buffering unboundedly.
        //
        // The current implementation uses an unbounded channel, so it will reach EOF quickly
        // and this test should fail until we add backpressure.
        let (done_tx, done_rx) = mpsc::channel();
        let reader = NotifyingReader {
            remaining: 2_000,
            done: done_tx,
        };

        // Capacity 1 ensures backpressure immediately if the receiver doesn't drain.
        let (tx, _rx) = channel::bounded::<WezEvent>(1);
        spawn_pty_reader_thread(Box::new(reader), tx);

        // If we get EOF quickly, we're buffering unboundedly.
        assert!(
            done_rx.recv_timeout(Duration::from_millis(200)).is_err(),
            "PTY reader reached EOF without backpressure; backend event queue is effectively \
             unbounded"
        );
    }
}

#[cfg(test)]
mod shutdown_tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use super::{ChildKiller, Duration, Instant, ShutdownState, poll_shutdown};

    #[derive(Clone, Debug, Default)]
    struct FakeKiller {
        kills: Arc<AtomicUsize>,
    }

    impl FakeKiller {
        fn kill_count(&self) -> usize {
            self.kills.load(Ordering::SeqCst)
        }
    }

    impl ChildKiller for FakeKiller {
        fn kill(&mut self) -> std::io::Result<()> {
            self.kills.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
            Box::new(self.clone())
        }
    }

    #[test]
    fn poll_shutdown_kills_after_timeout_once() {
        let mut killer = FakeKiller::default();
        let mut shutdown = ShutdownState {
            requested_at: Some(Instant::now() - Duration::from_secs(10)),
            kill_after: Some(Duration::from_secs(3)),
            kill_sent: false,
        };

        poll_shutdown(&mut shutdown, Instant::now(), false, &mut killer);
        assert_eq!(killer.kill_count(), 1);
        assert!(shutdown.kill_sent);

        // Subsequent polls should not re-kill.
        poll_shutdown(&mut shutdown, Instant::now(), false, &mut killer);
        assert_eq!(killer.kill_count(), 1);
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::VecDeque, io::Write, panic, sync::Arc};

    use gpui::{
        AppContext, Bounds, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, point, px, size,
    };
    use portable_pty::{ChildKiller, MasterPty, PtySize};
    use wezterm_term::{Terminal, TerminalConfiguration, TerminalSize};

    use super::{
        Instant, Mutex, SharedWriter, WezEvent, WezTermBackend, compute_viewport_plan,
        cursor_stable_line, default_shell_command_candidates, stable_row_texts,
        wrapped_command_span_for_stable_row,
    };
    use crate::{
        CastRecordingOptions, TerminalContent, TerminalType,
        backends::{
            self,
            wezterm::{WeztermConfig, current_dir_path_from_term},
        },
        terminal::{Terminal as WidgetTerminal, TerminalBackend},
    };

    #[derive(Debug)]
    struct TestConfig {
        scrollback: usize,
    }

    impl TerminalConfiguration for TestConfig {
        fn color_palette(&self) -> wezterm_term::color::ColorPalette {
            wezterm_term::color::ColorPalette::default()
        }

        fn scrollback_size(&self) -> usize {
            self.scrollback
        }
    }

    #[test]
    fn shared_writer_should_not_become_unusable_after_panic_while_holding_lock() {
        // If the inner writer panics while `SharedWriter` holds the mutex, std::sync::Mutex would
        // poison and subsequent `lock().unwrap()` calls would panic. We want the writer to remain
        // usable after the panic (parking_lot::Mutex has no poisoning).
        #[derive(Default)]
        struct OncePanicWriter {
            panicked: bool,
        }

        impl Write for OncePanicWriter {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                if !self.panicked {
                    self.panicked = true;
                    panic!("boom");
                }
                Ok(buf.len())
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let writer = SharedWriter {
            inner: Arc::new(Mutex::new(Box::new(OncePanicWriter::default()))),
            cast: Arc::new(Mutex::new(None)),
        };

        assert!(
            panic::catch_unwind(panic::AssertUnwindSafe({
                let mut w = writer.clone();
                move || {
                    let _ = w.write_all(b"x");
                }
            }))
            .is_err()
        );

        // The second write should not panic; the inner writer now succeeds.
        assert!(
            panic::catch_unwind(panic::AssertUnwindSafe({
                let mut w = writer.clone();
                move || {
                    w.write_all(b"y").unwrap();
                }
            }))
            .is_ok()
        );
    }

    fn line_text_at_phys(screen: &wezterm_term::Screen, phys_row: usize) -> String {
        let line = screen
            .lines_in_phys_range(phys_row..phys_row + 1)
            .into_iter()
            .next()
            .expect("expected a line");
        line.as_str().trim_end().to_string()
    }

    #[test]
    fn text_for_lines_exports_requested_stable_range() {
        let cast_slot = Arc::new(Mutex::new(None));
        let writer = SharedWriter {
            inner: Arc::new(Mutex::new(
                Box::new(std::io::sink()) as Box<dyn Write + Send>
            )),
            cast: Arc::clone(&cast_slot),
        };

        // Keep scrollback at 0 so stable mapping is deterministic in this test.
        let cfg = Arc::new(TestConfig { scrollback: 0 });
        let mut wezterm_term = Terminal::new(
            TerminalSize {
                rows: 3,
                cols: 10,
                pixel_width: 0,
                pixel_height: 0,
                dpi: 0,
            },
            cfg,
            "termua",
            "0",
            Box::new(writer.clone()),
        );

        wezterm_term.advance_bytes(b"one\r\ntwo\r\nthree");
        let screen = wezterm_term.screen();
        assert_eq!(line_text_at_phys(screen, 0), "one");
        assert_eq!(line_text_at_phys(screen, 1), "two");
        assert_eq!(line_text_at_phys(screen, 2), "three");

        let backend = WezTermBackend {
            master: Box::new(DummyMasterPty),
            writer,
            term: Arc::new(Mutex::new(wezterm_term)),
            child_killer: Box::new(DummyChildKiller),
            shutdown: super::ShutdownState::default(),
            pending_ops: VecDeque::new(),
            viewport_top_stable: None,
            last_clicked_line: None,
            search: super::SearchState::default(),
            content: TerminalContent::default(),
            exited: false,
            last_mouse_pos: None,
            selection: super::SelectionState::default(),
            default_cursor_shape: crate::CursorShape::default(),
            exit_fn: None,
            scroll_px: px(0.0),
            sftp: None,
            record: super::RecordState::new(cast_slot),
            osc: crate::command_blocks::OscStreamParser::new(),
            blocks: crate::command_blocks::CommandBlockTracker::new(200),
        };

        let term = backend.term.lock();
        let screen = term.screen();
        let s0 = screen.phys_to_stable_row_index(0) as i64;
        let s1 = screen.phys_to_stable_row_index(1) as i64;
        let s2 = screen.phys_to_stable_row_index(2) as i64;
        drop(term);

        assert_eq!(
            <WezTermBackend as TerminalBackend>::text_for_lines(&backend, s0, s1).as_deref(),
            Some("one\ntwo")
        );
        assert_eq!(
            <WezTermBackend as TerminalBackend>::text_for_lines(&backend, s1, s1).as_deref(),
            Some("two")
        );
        assert_eq!(
            <WezTermBackend as TerminalBackend>::text_for_lines(&backend, s1, s2).as_deref(),
            Some("two\nthree")
        );
    }

    fn test_backend(rows: usize, cols: usize, scrollback: usize) -> WezTermBackend {
        let cast_slot = Arc::new(Mutex::new(None));
        let writer = SharedWriter {
            inner: Arc::new(Mutex::new(
                Box::new(std::io::sink()) as Box<dyn Write + Send>
            )),
            cast: Arc::clone(&cast_slot),
        };
        let cfg = Arc::new(TestConfig { scrollback });
        let wezterm_term = Terminal::new(
            TerminalSize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
                dpi: 0,
            },
            cfg,
            "termua",
            "0",
            Box::new(writer.clone()),
        );

        WezTermBackend {
            master: Box::new(DummyMasterPty),
            writer,
            term: Arc::new(Mutex::new(wezterm_term)),
            child_killer: Box::new(DummyChildKiller),
            shutdown: super::ShutdownState::default(),
            pending_ops: VecDeque::new(),
            viewport_top_stable: None,
            last_clicked_line: None,
            search: super::SearchState::default(),
            content: TerminalContent::default(),
            exited: false,
            last_mouse_pos: None,
            selection: super::SelectionState::default(),
            default_cursor_shape: crate::CursorShape::default(),
            exit_fn: None,
            scroll_px: px(0.0),
            sftp: None,
            record: super::RecordState::new(cast_slot),
            osc: crate::command_blocks::OscStreamParser::new(),
            blocks: crate::command_blocks::CommandBlockTracker::new(200),
        }
    }

    #[test]
    fn selected_command_block_selection_tracks_resize_rewrap() {
        let mut backend = test_backend(5, 20, 100);
        let bounds = Bounds::new(point(px(0.0), px(0.0)), size(px(200.0), px(50.0)));
        backend.content.terminal_bounds = crate::TerminalBounds::new(px(10.0), px(10.0), bounds);

        {
            let mut term = backend.term.lock();
            term.advance_bytes(b"$ echo 123456\r\nout\r\n% ");
        }

        let (start_stable, end_stable, start_line, end_line) = {
            let term = backend.term.lock();
            let screen = term.screen();
            let start_stable = screen.phys_to_stable_row_index(0) as i64;
            let end_stable = screen.phys_to_stable_row_index(1) as i64;
            (
                start_stable,
                end_stable,
                WezTermBackend::grid_line_for_stable_row_on_screen(screen, start_stable)
                    .expect("start line"),
                WezTermBackend::grid_line_for_stable_row_on_screen(screen, end_stable)
                    .expect("end line"),
            )
        };

        backend.blocks.apply_osc133(
            "C",
            Instant::now(),
            start_stable,
            Some("$ echo 123456".to_string()),
        );
        backend
            .blocks
            .apply_osc133("D;0", Instant::now(), end_stable, None);

        <WezTermBackend as TerminalBackend>::set_selection_range(
            &mut backend,
            Some(crate::SelectionRange {
                start: crate::GridPoint::new(start_line, 0),
                end: crate::GridPoint::new(end_line, 19),
            }),
        );
        let block_id = backend
            .selection
            .command_block_id
            .expect("selection should remember selected command block");

        {
            let mut term = backend.term.lock();
            term.resize(TerminalSize {
                rows: 5,
                cols: 8,
                pixel_width: 0,
                pixel_height: 0,
                dpi: 0,
            });
            let cursor_stable = cursor_stable_line(&term).0;
            let lines = stable_row_texts(term.screen());
            backend.blocks.remap_after_rewrap(&lines, cursor_stable);
            let remapped_selection =
                backend.remapped_selected_command_block_selection_range(term.screen(), 8);
            drop(term);
            if let Some(range) = remapped_selection {
                backend.selection.range = Some(range);
                backend.selection.selecting = false;
                backend.selection.anchor = None;
            }

            let (expected_start, expected_end) = backend
                .blocks
                .range_for_block_id(block_id)
                .expect("block should still exist after resize");
            let selection = backend
                .selection
                .range
                .clone()
                .expect("selection should remain active");
            let term = backend.term.lock();
            let selected_start = WezTermBackend::stable_row_for_grid_line_on_screen(
                term.screen(),
                selection.start.line,
            )
            .expect("selected start stable row");
            let selected_end = WezTermBackend::stable_row_for_grid_line_on_screen(
                term.screen(),
                selection.end.line,
            )
            .expect("selected end stable row");

            assert_eq!(
                (selected_start, selected_end),
                (expected_start, expected_end)
            );
            assert_eq!(selection.start.column, 0);
            assert_eq!(selection.end.column, 7);
        }
    }

    #[test]
    fn wrapped_command_span_starts_at_first_visual_row() {
        let backend = test_backend(5, 8, 100);
        {
            let mut term = backend.term.lock();
            term.advance_bytes(b"$ 111111111111");
        }

        let term = backend.term.lock();
        let screen = term.screen();
        assert_eq!(line_text_at_phys(screen, 0), "$ 111111");
        assert_eq!(line_text_at_phys(screen, 1), "111111");

        let last_command_row = screen.phys_to_stable_row_index(1) as i64;
        let (start_stable, command) =
            wrapped_command_span_for_stable_row(screen, last_command_row).expect("wrapped span");

        assert_eq!(start_stable, screen.phys_to_stable_row_index(0) as i64);
        assert_eq!(command, "$ 111111111111");
    }

    #[test]
    fn stable_viewport_survives_scrollback_eviction() {
        let rows = 3usize;
        let cols = 10usize;
        let scrollback = 5usize;

        let cfg = Arc::new(TestConfig { scrollback });
        let mut term = Terminal::new(
            TerminalSize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
                dpi: 0,
            },
            cfg,
            "termua",
            "0",
            Box::new(Vec::<u8>::new()),
        );

        // Fill past scrollback capacity so the ring is hot and future output causes eviction.
        for i in 0..50 {
            let s = format!("L{i:02}\r\n");
            term.advance_bytes(s.as_bytes());
        }

        let screen_before = term.screen();
        let top_stable_before = screen_before.phys_to_stable_row_index(0);

        // Simulate the old physical-index viewport: scroll up 1 line from bottom.
        let base_start = screen_before.scrollback_rows().saturating_sub(rows);
        let viewport_start_phys = base_start.saturating_sub(1);
        let line_before_phys = line_text_at_phys(screen_before, viewport_start_phys);

        // Stable row index for that same viewport start.
        let base_stable = screen_before.phys_to_stable_row_index(base_start);
        let viewport_top_stable = base_stable - 1;
        let plan_before = compute_viewport_plan(screen_before, Some(viewport_top_stable));
        let line_before_stable = line_text_at_phys(screen_before, plan_before.phys_range.start);

        assert_eq!(line_before_phys, line_before_stable);

        // One more line should cause eviction (pop_front) and thus change phys<->stable mapping.
        term.advance_bytes(b"ZZ\r\n");

        let screen_after = term.screen();
        let top_stable_after = screen_after.phys_to_stable_row_index(0);
        assert!(
            top_stable_after > top_stable_before,
            "expected scrollback eviction to advance stable row index offset"
        );

        // Old physical indexing points at the wrong logical line after eviction.
        let line_after_phys = line_text_at_phys(screen_after, viewport_start_phys);
        assert_ne!(
            line_after_phys, line_before_phys,
            "physical row indexing should drift when scrollback evicts"
        );

        // Stable indexing yields the same logical content.
        let plan_after = compute_viewport_plan(screen_after, Some(viewport_top_stable));
        let line_after_stable = line_text_at_phys(screen_after, plan_after.phys_range.start);
        assert_eq!(line_after_stable, line_before_stable);
    }

    #[derive(Debug, Default)]
    struct DummyMasterPty;

    impl MasterPty for DummyMasterPty {
        fn resize(&self, _size: PtySize) -> Result<(), anyhow::Error> {
            Ok(())
        }

        fn get_size(&self) -> Result<PtySize, anyhow::Error> {
            Ok(PtySize::default())
        }

        fn try_clone_reader(&self) -> Result<Box<dyn std::io::Read + Send>, anyhow::Error> {
            Ok(Box::new(std::io::empty()))
        }

        fn take_writer(&self) -> Result<Box<dyn std::io::Write + Send>, anyhow::Error> {
            Ok(Box::new(std::io::sink()))
        }

        #[cfg(unix)]
        fn process_group_leader(&self) -> Option<libc::pid_t> {
            None
        }

        #[cfg(unix)]
        fn as_raw_fd(&self) -> Option<std::os::unix::io::RawFd> {
            None
        }

        #[cfg(unix)]
        fn tty_name(&self) -> Option<std::path::PathBuf> {
            None
        }
    }

    #[derive(Clone, Debug, Default)]
    struct DummyChildKiller;

    impl ChildKiller for DummyChildKiller {
        fn kill(&mut self) -> std::io::Result<()> {
            Ok(())
        }

        fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
            Box::new(Self)
        }
    }

    #[test]
    fn triple_click_selects_current_line() {
        let cast_slot = Arc::new(Mutex::new(None));
        let writer = SharedWriter {
            inner: Arc::new(Mutex::new(
                Box::new(std::io::sink()) as Box<dyn std::io::Write + Send>
            )),
            cast: Arc::clone(&cast_slot),
        };

        // Keep scrollback at 0 so viewport coordinate mapping is stable and predictable.
        let cfg = Arc::new(TestConfig { scrollback: 0 });
        let wezterm_term = Terminal::new(
            TerminalSize {
                rows: 3,
                cols: 10,
                pixel_width: 0,
                pixel_height: 0,
                dpi: 0,
            },
            cfg,
            "termua",
            "0",
            Box::new(writer.clone()),
        );

        let mut backend = WezTermBackend {
            master: Box::new(DummyMasterPty),
            writer,
            term: Arc::new(Mutex::new(wezterm_term)),
            child_killer: Box::new(DummyChildKiller),
            shutdown: super::ShutdownState::default(),
            pending_ops: VecDeque::new(),
            viewport_top_stable: None,
            last_clicked_line: None,
            search: super::SearchState::default(),
            content: TerminalContent::default(),
            exited: false,
            last_mouse_pos: None,
            selection: super::SelectionState::default(),
            default_cursor_shape: crate::CursorShape::default(),
            exit_fn: None,
            scroll_px: px(0.0),
            sftp: None,
            record: super::RecordState::new(cast_slot),
            osc: crate::command_blocks::OscStreamParser::new(),
            blocks: crate::command_blocks::CommandBlockTracker::new(200),
        };

        // 3 rows x 10 cols.
        let bounds = Bounds::new(point(px(0.0), px(0.0)), size(px(100.0), px(30.0)));
        backend.content.terminal_bounds = crate::TerminalBounds::new(px(10.0), px(10.0), bounds);

        // Fill visible cells; put a couple of words on row 1 so word-selection differs from line.
        let cols = backend.content.terminal_bounds.num_columns();
        let rows = backend.content.terminal_bounds.num_lines();
        assert_eq!((rows, cols), (3, 10));

        backend.content.cells = (0..rows * cols)
            .map(|i| {
                let row = i / cols;
                let col = i % cols;
                let mut cell = crate::Cell::default();
                if row == 1 {
                    // "abc def   " (10 cols).
                    let s = b"abc def   ";
                    cell.c = s[col] as char;
                }
                crate::IndexedCell {
                    point: crate::GridPoint::new(row as i32, col),
                    cell,
                }
            })
            .collect();

        // Click on the 'e' in "def" (row 1, col 5).
        let e = MouseDownEvent {
            button: MouseButton::Left,
            position: point(px(5.0 * 10.0 + 1.0), px(1.0 * 10.0 + 1.0)),
            modifiers: Modifiers::default(),
            click_count: 3,
            first_mouse: false,
        };

        backend.select_word_at_event_position(&e);
        assert_eq!(
            backend.selection.range,
            Some(crate::SelectionRange {
                start: crate::GridPoint::new(1, 0),
                end: crate::GridPoint::new(1, 9),
            })
        );
    }

    #[gpui::test]
    fn pty_exit_stops_cast_recording(cx: &mut gpui::TestAppContext) {
        let cast_slot = Arc::new(Mutex::new(None));
        let writer = SharedWriter {
            inner: Arc::new(Mutex::new(
                Box::new(std::io::sink()) as Box<dyn std::io::Write + Send>
            )),
            cast: Arc::clone(&cast_slot),
        };

        let cfg = Arc::new(TestConfig { scrollback: 0 });
        let wezterm_term = Terminal::new(
            TerminalSize::default(),
            cfg,
            "termua",
            "0",
            Box::new(writer.clone()),
        );

        let backend = WezTermBackend {
            master: Box::new(DummyMasterPty),
            writer,
            term: Arc::new(Mutex::new(wezterm_term)),
            child_killer: Box::new(DummyChildKiller),
            shutdown: super::ShutdownState::default(),
            pending_ops: VecDeque::new(),
            viewport_top_stable: None,
            last_clicked_line: None,
            search: super::SearchState::default(),
            content: TerminalContent::default(),
            exited: false,
            last_mouse_pos: None,
            selection: super::SelectionState::default(),
            default_cursor_shape: crate::CursorShape::default(),
            exit_fn: None,
            scroll_px: px(0.0),
            sftp: None,
            record: super::RecordState::new(cast_slot),
            osc: crate::command_blocks::OscStreamParser::new(),
            blocks: crate::command_blocks::CommandBlockTracker::new(200),
        };

        let terminal = cx.new(|_| WidgetTerminal::new(TerminalType::WezTerm, Box::new(backend)));

        terminal.update(cx, |term, cx| {
            let path = std::env::temp_dir().join(format!(
                "termua-test-pty-exit-{}-{}.cast",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ));

            term.start_cast_recording(CastRecordingOptions {
                path,
                include_input: false,
            })
            .unwrap();
            assert!(term.cast_recording_active());

            term.dispatch_backend_event(Box::new(WezEvent::PtyExit), cx);
            assert!(!term.cast_recording_active());
        });
    }

    #[test]
    fn drag_line_delta_detects_edges_and_clamps() {
        let region = Bounds::new(point(px(0.0), px(100.0)), size(px(200.0), px(200.0)));
        let line_height = px(10.0);

        let mk = |y: f32| MouseMoveEvent {
            position: point(px(10.0), px(y)),
            pressed_button: Some(MouseButton::Left),
            modifiers: Modifiers::default(),
        };

        assert!(backends::drag_line_delta(mk(150.0).position, region, line_height).is_none());
        assert!(backends::drag_line_delta(mk(99.0).position, region, line_height).unwrap() > 0);
        assert!(backends::drag_line_delta(mk(301.0).position, region, line_height).unwrap() < 0);

        // Very far away should clamp to a small step to keep scrolling controllable.
        assert_eq!(
            backends::drag_line_delta(mk(-10_000.0).position, region, line_height),
            Some(3)
        );
        assert_eq!(
            backends::drag_line_delta(mk(10_000.0).position, region, line_height),
            Some(-3)
        );
    }

    #[test]
    fn __subprocess_dump_default_shell_command_candidates() {
        let candidates = default_shell_command_candidates();
        let argv0 = candidates
            .first()
            .and_then(|c| c.get_argv().first())
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();

        println!(
            "TERMUA_TEST_DEFAULT_SHELL_CANDIDATES_LEN={}",
            candidates.len()
        );
        println!("TERMUA_TEST_DEFAULT_SHELL_CANDIDATES_ARGV0={argv0}");
    }

    #[test]
    fn default_shell_candidates_do_not_consult_parent_termua_shell_env() {
        use std::process::Command;

        let exe = std::env::current_exe().expect("current test executable path should exist");
        let output = Command::new(exe)
            .env("TERMUA_SHELL", "termua-test-termua-shell")
            .env("SHELL", "termua-test-shell")
            .arg("__subprocess_dump_default_shell_command_candidates")
            .arg("--nocapture")
            .output()
            .expect("spawn test subprocess");

        assert!(
            output.status.success(),
            "subprocess failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = String::from_utf8_lossy(&output.stdout);
        let argv0 = stdout
            .lines()
            .find_map(|line| line.strip_prefix("TERMUA_TEST_DEFAULT_SHELL_CANDIDATES_ARGV0="))
            .unwrap_or_default();

        assert_ne!(
            argv0, "termua-test-termua-shell",
            "default_shell_command_candidates should not consult parent TERMUA_SHELL"
        );
    }

    #[test]
    fn local_shell_candidates_prefers_env_termua_shell_override() {
        let mut env = std::collections::HashMap::new();
        env.insert("TERMUA_SHELL".to_string(), "termua-env-shell".to_string());

        let candidates = super::shell_command_candidates_for_local_env(&env);
        assert_eq!(
            candidates[0].get_argv()[0].to_string_lossy(),
            "termua-env-shell"
        );
    }

    #[test]
    fn local_shell_candidates_uses_bash_rcfile_when_configured() {
        let mut env = std::collections::HashMap::new();
        env.insert("TERMUA_SHELL".to_string(), "/bin/bash".to_string());
        env.insert(
            "TERMUA_BASH_RCFILE".to_string(),
            "/tmp/termua-test.bashrc".to_string(),
        );

        let candidates = super::shell_command_candidates_for_local_env(&env);
        let argv: Vec<String> = candidates[0]
            .get_argv()
            .iter()
            .map(|s| s.to_string_lossy().to_string())
            .collect();

        assert_eq!(
            argv,
            vec![
                "/bin/bash",
                "--noprofile",
                "--rcfile",
                "/tmp/termua-test.bashrc",
                "-i"
            ]
        );
    }

    #[test]
    fn local_shell_candidates_uses_fish_init_when_configured() {
        let mut env = std::collections::HashMap::new();
        env.insert("TERMUA_SHELL".to_string(), "fish".to_string());
        env.insert(
            "TERMUA_FISH_INIT".to_string(),
            "/tmp/termua-test.fish".to_string(),
        );

        let candidates = super::shell_command_candidates_for_local_env(&env);
        let argv: Vec<String> = candidates[0]
            .get_argv()
            .iter()
            .map(|s| s.to_string_lossy().to_string())
            .collect();

        assert_eq!(
            argv,
            vec![
                "fish",
                "--init-command",
                "source \"$TERMUA_FISH_INIT\"",
                "--interactive"
            ]
        );
    }

    #[test]
    fn local_shell_candidates_uses_nu_configs_when_configured() {
        let mut env = std::collections::HashMap::new();
        env.insert("TERMUA_SHELL".to_string(), "nu".to_string());
        env.insert(
            "TERMUA_NU_CONFIG".to_string(),
            "/tmp/termua-config.nu".to_string(),
        );
        env.insert(
            "TERMUA_NU_ENV_CONFIG".to_string(),
            "/tmp/termua-env.nu".to_string(),
        );

        let candidates = super::shell_command_candidates_for_local_env(&env);
        let argv: Vec<String> = candidates[0]
            .get_argv()
            .iter()
            .map(|s| s.to_string_lossy().to_string())
            .collect();

        assert_eq!(
            argv,
            vec![
                "nu",
                "--config",
                "/tmp/termua-config.nu",
                "--env-config",
                "/tmp/termua-env.nu",
                "--interactive"
            ]
        );
    }

    #[test]
    fn local_shell_candidates_uses_powershell_init_when_configured() {
        let mut env = std::collections::HashMap::new();
        env.insert("TERMUA_SHELL".to_string(), "pwsh".to_string());
        env.insert(
            "TERMUA_PWSH_INIT".to_string(),
            "/tmp/termua-init.ps1".to_string(),
        );

        let candidates = super::shell_command_candidates_for_local_env(&env);
        let argv: Vec<String> = candidates[0]
            .get_argv()
            .iter()
            .map(|s| s.to_string_lossy().to_string())
            .collect();

        let mut expected = vec!["pwsh", "-NoLogo", "-NoExit"];
        if cfg!(windows) {
            expected.extend(["-ExecutionPolicy", "Bypass"]);
        }
        expected.extend(["-Command", ". \"$env:TERMUA_PWSH_INIT\""]);

        assert_eq!(
            argv,
            expected
                .into_iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        );
    }

    #[cfg(windows)]
    #[test]
    fn default_shell_candidates_includes_cmd_fallback_on_windows() {
        // We don't assert that `pwsh`/`powershell` exist; only that we always
        // provide a final fallback.
        let candidates = default_shell_command_candidates();
        assert!(!candidates.is_empty());
        assert!(!candidates.last().unwrap().get_argv().is_empty());
    }

    #[test]
    fn pty_source_supports_ssh_variant() {
        // Compile-time smoke test: the backend exposes a public SSH PTY source variant
        // so callsites can build a WezTerm terminal over an SSH session.
        let _ = crate::PtySource::Ssh {
            env: std::collections::HashMap::new(),
            opts: super::ssh::SshOptions {
                host: "127.0.0.1".to_string(),
                port: None,
                auth: super::ssh::Authentication::Config,
                proxy: super::ssh::SshProxyMode::Inherit,
                backend: super::ssh::SshBackend::default(),
                tcp_nodelay: false,
                tcp_keepalive: false,
            },
        };
    }

    #[test]
    fn osc7_current_dir_is_exposed_as_path() {
        let mut term = Terminal::new(
            TerminalSize::default(),
            Arc::new(WeztermConfig {
                scrollback_size: 1000,
            }),
            "termua",
            "0",
            Box::new(std::io::sink()),
        );

        term.advance_bytes(b"\x1b]7;file://localhost/home/user%20name\x07");
        assert_eq!(
            current_dir_path_from_term(&term).as_deref(),
            Some("/home/user name")
        );
    }

    #[test]
    fn osc133_boundary_adjusts_for_column_zero() {
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
