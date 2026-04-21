use std::{
    any::Any,
    borrow::Cow,
    collections::HashMap,
    ops::RangeInclusive,
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

use gpui::{
    Bounds, ClipboardItem, Context, EventEmitter, Keystroke, Modifiers, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Point, ReadGlobal, ScrollWheelEvent, Window, actions, px,
};
use serde::{Deserialize, Serialize};

use crate::{GridPoint, IndexedCell, TerminalContent, TerminalSettings, settings::CursorShape};

actions!(
    terminal,
    [
        /// Clears the terminal screen.
        Clear,
        /// Copies selected text to the clipboard.
        Copy,
        /// Pastes from the clipboard.
        Paste,
        /// Shows the character palette for special characters.
        ShowCharacterPalette,
        /// Opens the in-terminal search UI.
        Search,
        /// Jump to the next match in the active search.
        SearchNext,
        /// Jump to the previous match in the active search.
        SearchPrevious,
        /// Close the in-terminal search UI.
        SearchClose,
        /// Paste clipboard contents into the search query.
        SearchPaste,
        /// Scrolls up by one line.
        ScrollLineUp,
        /// Scrolls down by one line.
        ScrollLineDown,
        /// Scrolls up by one page.
        ScrollPageUp,
        /// Scrolls down by one page.
        ScrollPageDown,
        /// Scrolls up by half a page.
        ScrollHalfPageUp,
        /// Scrolls down by half a page.
        ScrollHalfPageDown,
        /// Scrolls to the top of the terminal buffer.
        ScrollToTop,
        /// Scrolls to the bottom of the terminal buffer.
        ScrollToBottom,
        /// Toggles vi mode in the terminal.
        ToggleViMode,
        /// Selects all text in the terminal.
        SelectAll,
        /// Reset font size to the config value.
        ResetFontSize,
        /// Increase font size.
        IncreaseFontSize,
        /// Decrease font size.
        DecreaseFontSize,
        /// Start recording this terminal session to an asciinema `.cast` file.
        StartCastRecording,
        /// Stop recording the current terminal session (if active).
        StopCastRecording,
        /// Toggle asciinema `.cast` recording for this terminal session.
        ToggleCastRecording,
    ]
);

/// Upward flowing events, for changing the title and such.
#[derive(Clone, Debug)]
pub enum Event {
    TitleChanged,
    CloseTerminal,
    Bell,
    Wakeup,
    BlinkChanged(bool),
    SelectionsChanged,
    NewNavigationTarget(Option<String>),
    Open(String),
    /// Informational UI message from the terminal view layer (non-terminal protocol).
    ///
    /// Embedding applications can surface this in their notification/message panel so users can
    /// copy the full text (toasts are often transient and non-selectable).
    Toast {
        level: gpui::PromptLevel,
        title: String,
        detail: Option<String>,
    },
    /// Raw user input (keystrokes / IME committed text / paste) that was actually sent to the PTY.
    ///
    /// Embedding apps can use this to implement features like "broadcast input to visible panes".
    UserInput(UserInput),
    /// SFTP per-file upload progress update.
    ///
    /// Embedding applications can use this to show one progress row per file and provide a
    /// per-file cancel button using the shared `cancel` token.
    SftpUploadFileProgress {
        transfer_id: u64,
        file_index: usize,
        file: String,
        sent: u64,
        total: u64,
        cancel: Arc<AtomicBool>,
    },
    /// SFTP upload finished successfully.
    SftpUploadFinished {
        files: Vec<(String, u64)>,
        total_bytes: u64,
    },
    /// SFTP per-file upload finished successfully.
    SftpUploadFileFinished {
        transfer_id: u64,
        file_index: usize,
        file: String,
        bytes: u64,
    },
    /// SFTP upload cancelled/aborted.
    ///
    /// Currently this is emitted on failures (e.g. remote permission issues).
    SftpUploadCancelled,
    /// SFTP per-file upload cancelled.
    ///
    /// This is emitted when the embedding app toggles the `cancel` token in
    /// `SftpUploadFileProgress`.
    SftpUploadFileCancelled {
        transfer_id: u64,
        file_index: usize,
        file: String,
        sent: u64,
        total: u64,
    },
}

#[derive(Clone, Debug)]
pub enum UserInput {
    Keystroke(Keystroke),
    Text(String),
    Paste(String),
}

/// Policy for shutting down a terminal session.
///
/// This is intended to be called by the embedding app when the terminal UI is being removed
/// (e.g. a tab is closed). Backends should make a best-effort attempt to release OS resources
/// (PTY handles, child processes, background threads) under this policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalShutdownPolicy {
    /// Best-effort graceful shutdown (backend-defined).
    Graceful,
    /// Best-effort graceful shutdown, then force kill after the given duration if the terminal
    /// still hasn't exited.
    GracefulThenKill(Duration),
    /// Immediately force kill (backend-defined).
    Kill,
}

/// Backend interface for the terminal widget.
///
/// The GPUI view and element only speak to this trait. Concrete backends (alacritty_terminal,
/// wezterm_term, ...) live in `crate::backends`.
pub trait TerminalBackend: Send {
    fn backend_name(&self) -> &'static str;

    /// Whether this backend represents a remote-mirrored terminal where the host is authoritative
    /// for scroll/selection state.
    ///
    /// When `true`, view-layer code should avoid mutating view-local scroll/selection state and
    /// instead delegate to backend methods (which may forward input upstream).
    fn is_remote_mirror(&self) -> bool {
        false
    }

    /// Handle backend-specific IO/events (e.g. from a PTY reader task).
    ///
    /// Backends typically downcast `event` to their internal event type.
    fn handle_backend_event(&mut self, _event: Box<dyn Any + Send>, _cx: &mut Context<Terminal>) {}

    fn sync(&mut self, window: &mut Window, cx: &mut Context<Terminal>);

    /// Request that the backend shuts down its underlying terminal session resources.
    ///
    /// Note: this is best-effort. Backends that don't own the underlying process resources may
    /// not be able to honor all policies.
    fn shutdown(&mut self, _policy: TerminalShutdownPolicy, _cx: &mut Context<Terminal>) {}

    fn last_content(&self) -> &TerminalContent;
    fn matches(&self) -> &[RangeInclusive<GridPoint>];
    fn active_match_index(&self) -> Option<usize> {
        None
    }
    fn last_clicked_line(&self) -> Option<i32>;

    fn vi_mode_enabled(&self) -> bool;
    fn mouse_mode(&self, shift: bool) -> bool;
    fn selection_started(&self) -> bool;

    fn has_exited(&self) -> bool {
        false
    }

    /// Clears any active selection.
    ///
    /// This is used to implement common terminal behavior: any user input (typing/paste) cancels
    /// the current selection.
    fn clear_selection(&mut self) {}

    fn set_cursor_shape(&mut self, cursor_shape: CursorShape);
    fn total_lines(&self) -> usize;
    fn viewport_lines(&self) -> usize;

    fn activate_match(&mut self, index: usize);
    fn select_matches(&mut self, matches: &[RangeInclusive<GridPoint>]);
    fn set_search_query(&mut self, _query: Option<String>) {}
    fn select_all(&mut self);
    fn copy(&mut self, keep_selection: Option<bool>, cx: &mut Context<Terminal>);
    fn clear(&mut self);

    fn scroll_line_up(&mut self);
    fn scroll_up_by(&mut self, lines: usize);
    fn scroll_line_down(&mut self);
    fn scroll_down_by(&mut self, lines: usize);
    fn scroll_page_up(&mut self);
    fn scroll_page_down(&mut self);
    fn scroll_to_top(&mut self);
    fn scroll_to_bottom(&mut self);
    fn scrolled_to_top(&self) -> bool;
    fn scrolled_to_bottom(&self) -> bool;

    fn set_size(&mut self, new_bounds: TerminalBounds);

    fn input(&mut self, input: Cow<'static, [u8]>);
    fn paste(&mut self, text: &str);

    /// Export the last `max_lines` of terminal output (including scrollback), if supported by the
    /// backend.
    ///
    /// The returned text is intended for UI features like assistants and diagnostics; it should
    /// not modify terminal scroll position or selection state.
    fn tail_text(&self, _max_lines: usize) -> Option<String> {
        None
    }

    /// Export terminal text for a specific inclusive line range.
    ///
    /// The meaning of `start_line`/`end_line` is backend-defined, but it should be stable for
    /// the lifetime of the referenced scrollback content (i.e. resilient to physical-index drift
    /// when older scrollback is evicted).
    ///
    /// The returned text is intended for UI features like assistants and diagnostics; it should
    /// not modify terminal scroll position or selection state.
    fn text_for_lines(&self, _start_line: i64, _end_line: i64) -> Option<String> {
        None
    }

    /// Returns the terminal's recent command blocks, if supported by the backend.
    fn command_blocks(&self) -> Option<Vec<crate::command_blocks::CommandBlock>> {
        None
    }

    /// Convert a backend `GridPoint.line` coordinate into a stable row identifier suitable for
    /// matching against command blocks.
    ///
    /// This is a best-effort adapter for UI integrations (e.g. context menus). Backends that
    /// don't have a stable row concept may return `None`.
    fn stable_row_for_grid_line(&self, _line: i32) -> Option<i64> {
        None
    }

    /// Convert a stable row identifier (as used by command blocks) into a backend `GridPoint.line`
    /// coordinate suitable for creating a selection range.
    fn grid_line_for_stable_row(&self, _stable_row: i64) -> Option<i32> {
        None
    }

    /// Set the current selection range.
    ///
    /// Backends should treat this as a UI-only operation and avoid emitting PTY input.
    fn set_selection_range(&mut self, _range: Option<crate::SelectionRange>) {}

    // --- Optional asciinema cast recording integration ---
    fn cast_recording_active(&self) -> bool {
        false
    }

    fn start_cast_recording(&mut self, _opts: crate::CastRecordingOptions) -> gpui::Result<()> {
        Ok(())
    }

    fn stop_cast_recording(&mut self) {}

    fn focus_in(&self);
    fn focus_out(&mut self);
    fn toggle_vi_mode(&mut self);

    fn try_keystroke(&mut self, keystroke: &Keystroke, alt_is_meta: bool) -> bool;
    fn try_modifiers_change(
        &mut self,
        modifiers: &Modifiers,
        window: &Window,
        cx: &mut Context<Terminal>,
    );

    fn mouse_move(&mut self, e: &MouseMoveEvent, cx: &mut Context<Terminal>);
    fn select_word_at_event_position(&mut self, e: &MouseDownEvent);
    fn mouse_drag(
        &mut self,
        e: &MouseMoveEvent,
        region: Bounds<Pixels>,
        cx: &mut Context<Terminal>,
    );
    fn mouse_down(&mut self, e: &MouseDownEvent, cx: &mut Context<Terminal>);
    fn mouse_up(&mut self, e: &MouseUpEvent, cx: &Context<Terminal>);
    fn scroll_wheel(&mut self, e: &ScrollWheelEvent);

    fn get_content(&self) -> String;
    fn last_n_non_empty_lines(&self, n: usize) -> Vec<String>;

    /// Returns up to `count` lines of text starting at `start_line`, where `start_line == 0`
    /// refers to the oldest line in the scrollback buffer.
    ///
    /// Used by the scrollbar/minimap tooltip preview.
    fn preview_lines_from_top(&self, _start_line: usize, _count: usize) -> Vec<String> {
        Vec::new()
    }

    /// Returns a small, renderable slice of the scrollback buffer for the scrollbar/minimap
    /// tooltip preview.
    ///
    /// The returned cells are re-based such that the first returned row has `point.line == 0`.
    /// This makes them directly usable with the terminal renderer (`build_plan`) without needing
    /// to know the backend's internal line coordinate system.
    ///
    /// Returns `(columns, rows, cells)`.
    fn preview_cells_from_top(
        &self,
        _start_line: usize,
        _count: usize,
    ) -> (usize, usize, Vec<IndexedCell>) {
        (0, 0, Vec::new())
    }

    /// Returns the number of "logical" lines in the scrollback, where soft-wrapped rows are
    /// counted as part of the same logical line.
    ///
    /// Default implementation falls back to physical row count.
    fn total_logical_lines(&self) -> usize {
        self.total_lines()
    }

    /// Returns per-row logical line numbers for a physical row range from the top of scrollback.
    ///
    /// The returned vector has length `count` (or shorter if the range extends past the available
    /// scrollback). Entries are `Some(n)` only for the first visual row of a logical line; wrapped
    /// continuation rows return `None`.
    ///
    /// Default implementation numbers every physical row.
    fn logical_line_numbers_from_top(&self, start_line: usize, count: usize) -> Vec<Option<usize>> {
        if count == 0 {
            return Vec::new();
        }
        let total = self.total_lines();
        let start = start_line.min(total);
        let end = start.saturating_add(count).min(total);
        (start..end).map(|i| Some(i.saturating_add(1))).collect()
    }

    /// Returns an identifier for the oldest line currently available in the scrollback.
    ///
    /// This ID is backend-defined but must increase as new lines are appended, and it must be
    /// contiguous for the currently available scrollback (`total_lines()`).
    fn scrollback_top_line_id(&self) -> i64 {
        0
    }

    /// Returns the identifier of the line containing the cursor.
    fn cursor_line_id(&self) -> Option<i64> {
        None
    }

    fn set_env(&mut self, _env: HashMap<String, String>) {}

    /// Returns the current working directory for the terminal session, when known.
    ///
    /// For WezTerm backend this is driven by OSC 7 (`file://...`) emitted by the shell.
    fn current_dir(&self) -> Option<String> {
        None
    }

    // --- Optional SFTP integration. ---
    //
    // This intentionally returns a concrete `wezterm_ssh` type because gpui_term already depends
    // on wezterm_ssh for its SSH PTY implementations.
    fn sftp(&self) -> Option<wezterm_ssh::Sftp> {
        None
    }
}

pub(crate) fn logical_line_numbers_from_wraps(
    total: usize,
    start_line: usize,
    count: usize,
    mut prev_row_wrapped: impl FnMut(usize) -> bool,
) -> Vec<Option<usize>> {
    if count == 0 || total == 0 {
        return Vec::new();
    }

    let start = start_line.min(total);
    let end = start.saturating_add(count).min(total);
    if start == end {
        return Vec::new();
    }

    let mut out = Vec::with_capacity(end - start);
    let mut logical_no: usize = 0;
    let mut was_wrapped = false;
    for row in 0..end {
        let is_start = row == 0 || !was_wrapped;
        if is_start {
            logical_no = logical_no.saturating_add(1);
        }

        if row >= start {
            out.push(if is_start { Some(logical_no) } else { None });
        }

        was_wrapped = prev_row_wrapped(row);
    }

    out
}

/// Terminal widget entrypoint.
///
/// Internally it delegates all behavior to a pluggable backend.
///
/// ```no_run
/// use std::time::Duration;
///
/// use gpui::Context;
/// use gpui_term::{Terminal, TerminalShutdownPolicy};
///
/// fn request_shutdown(term: &mut Terminal, cx: &mut Context<Terminal>) {
///     term.shutdown(
///         TerminalShutdownPolicy::GracefulThenKill(Duration::from_secs(3)),
///         cx,
///     );
/// }
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TerminalType {
    Alacritty,
    WezTerm,
}

pub struct Terminal {
    backend_type: TerminalType,
    inner: Box<dyn TerminalBackend>,
    sftp_upload_active: bool,
    sftp_upload_transfer_id: u64,
}

impl Drop for Terminal {
    fn drop(&mut self) {
        // Ensure cast recording threads flush their writer and exit before the backend is dropped.
        if self.inner.cast_recording_active() {
            self.inner.stop_cast_recording();
        }
    }
}

mod sftp_upload {
    use std::{
        collections::HashSet,
        path::PathBuf,
        sync::{Arc, atomic::AtomicBool},
        time::{Duration, Instant},
    };

    use camino::Utf8PathBuf;
    use gpui::{AsyncApp, WeakEntity};
    use gpui_common::PermitPool;
    use smol::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use wezterm_ssh::{OpenFileType, OpenOptions, Sftp, SftpChannelError, WriteMode};

    use super::Event;

    pub(crate) struct StartParams {
        pub(crate) paths: Vec<PathBuf>,
        pub(crate) sftp: Sftp,
        pub(crate) upload_pool: PermitPool,
        pub(crate) max_concurrency: usize,
        pub(crate) transfer_id: u64,
        pub(crate) remote_dir_hint: Option<String>,
    }

    pub(crate) struct LocalUploadFile {
        pub(crate) path: PathBuf,
        pub(crate) name: String,
        pub(crate) size: u64,
        pub(crate) cancel: Arc<AtomicBool>,
    }

    pub(crate) struct PlannedUploadFile {
        pub(crate) path: PathBuf,
        pub(crate) remote_name: String,
        pub(crate) remote_path: Utf8PathBuf,
        pub(crate) size: u64,
        pub(crate) cancel: Arc<AtomicBool>,
    }

    #[derive(Debug)]
    pub(crate) enum PlanError {
        TooManyCollisions {
            remote_path: Utf8PathBuf,
        },
        MetadataFailed {
            remote_path: Utf8PathBuf,
            err: SftpChannelError,
        },
    }

    #[derive(Clone, Debug)]
    pub(crate) enum UploadMsg {
        Progress {
            file_index: usize,
            file: String,
            sent: u64,
            total: u64,
            cancel: Arc<AtomicBool>,
        },
        Finished {
            file_index: usize,
            file: String,
            bytes: u64,
        },
        Cancelled {
            file_index: usize,
            file: String,
            sent: u64,
            total: u64,
        },
        Failed {
            file_index: usize,
            file: String,
            sent: u64,
            total: u64,
            error: String,
        },
    }

    fn set_upload_active(this: &WeakEntity<super::Terminal>, cx: &mut AsyncApp, active: bool) {
        let _ = this.update(cx, move |this, _cx| {
            this.sftp_upload_active = active;
        });
    }

    fn cancel_upload(this: &WeakEntity<super::Terminal>, cx: &mut AsyncApp, toast: Option<Event>) {
        let _ = this.update(cx, |this, cx| {
            this.sftp_upload_active = false;
            if let Some(toast) = toast {
                cx.emit(toast);
            }
            cx.emit(Event::SftpUploadCancelled);
        });
    }

    fn cancel_upload_with_toast(
        this: &WeakEntity<super::Terminal>,
        cx: &mut AsyncApp,
        level: gpui::PromptLevel,
        title: String,
        detail: Option<String>,
    ) {
        cancel_upload(
            this,
            cx,
            Some(Event::Toast {
                level,
                title,
                detail,
            }),
        );
    }

    fn emit_initial_progress_events(
        this: &WeakEntity<super::Terminal>,
        cx: &mut AsyncApp,
        transfer_id: u64,
        planned: &[PlannedUploadFile],
    ) {
        let _ = this.update(cx, |_, cx| {
            for (file_index, f) in planned.iter().enumerate() {
                cx.emit(Event::SftpUploadFileProgress {
                    transfer_id,
                    file_index,
                    file: f.remote_name.clone(),
                    sent: 0,
                    total: f.size,
                    cancel: Arc::clone(&f.cancel),
                });
            }
        });
    }

    fn try_spawn_workers(
        sftp: &Sftp,
        upload_pool: &PermitPool,
        max_concurrency: usize,
        queue: &mut std::collections::VecDeque<(usize, PlannedUploadFile)>,
        active: &mut usize,
        tx: &smol::channel::Sender<UploadMsg>,
    ) {
        while *active < max_concurrency {
            let Some((file_index, f)) = queue.pop_front() else {
                break;
            };
            spawn_worker(sftp.clone(), upload_pool.clone(), file_index, f, tx.clone());
            *active += 1;
        }
    }

    pub(crate) async fn run_start(
        this: WeakEntity<super::Terminal>,
        cx: &mut AsyncApp,
        params: StartParams,
    ) {
        let StartParams {
            paths,
            sftp,
            upload_pool,
            max_concurrency,
            transfer_id,
            remote_dir_hint,
        } = params;

        set_upload_active(&this, cx, true);
        let Some(mut state) = prepare_upload(
            &this,
            cx,
            &sftp,
            &upload_pool,
            PrepareUploadParams {
                paths,
                transfer_id,
                remote_dir_hint: remote_dir_hint.as_deref(),
                max_concurrency,
            },
        )
        .await
        else {
            return;
        };

        let (files, total_bytes) = run_upload_loop(
            &this,
            cx,
            &sftp,
            &upload_pool,
            max_concurrency,
            transfer_id,
            &mut state,
        )
        .await;

        let _ = this.update(cx, |this, cx| {
            this.sftp_upload_active = false;
            cx.emit(Event::SftpUploadFinished { files, total_bytes });
        });
    }

    struct UploadLoopState {
        tx: smol::channel::Sender<UploadMsg>,
        rx: smol::channel::Receiver<UploadMsg>,
        queue: std::collections::VecDeque<(usize, PlannedUploadFile)>,
        active: usize,
        uploaded: Vec<(usize, String, u64)>,
    }

    impl UploadLoopState {
        fn record_uploaded_file(&mut self, file_index: usize, file: &str, bytes: u64) {
            self.uploaded.push((file_index, file.to_string(), bytes));
        }

        fn finish_active_file(&mut self) {
            self.active = self.active.saturating_sub(1);
        }

        fn cancel_active_file(&mut self) {
            self.active = self.active.saturating_sub(1);
        }
    }

    struct PrepareUploadParams<'a> {
        paths: Vec<PathBuf>,
        transfer_id: u64,
        remote_dir_hint: Option<&'a str>,
        max_concurrency: usize,
    }

    async fn prepare_upload(
        this: &WeakEntity<super::Terminal>,
        cx: &mut AsyncApp,
        sftp: &Sftp,
        upload_pool: &PermitPool,
        params: PrepareUploadParams<'_>,
    ) -> Option<UploadLoopState> {
        let PrepareUploadParams {
            paths,
            transfer_id,
            remote_dir_hint,
            max_concurrency,
        } = params;

        let local_files = collect_local_files(paths);
        if local_files.is_empty() {
            cancel_upload_with_toast(
                this,
                cx,
                gpui::PromptLevel::Warning,
                "Nothing to upload".to_string(),
                Some("No files were selected.".to_string()),
            );
            return None;
        }

        let remote_dir = match resolve_remote_dir(sftp, remote_dir_hint).await {
            Ok(p) => p,
            Err(err) => {
                cancel_upload_with_toast(
                    this,
                    cx,
                    gpui::PromptLevel::Critical,
                    "Upload failed".to_string(),
                    Some(format!("SFTP: failed to resolve remote directory: {err}")),
                );
                return None;
            }
        };

        // Plan unique remote names up-front (best-effort), to avoid collisions across
        // concurrently executing upload tasks.
        let planned = match plan_files(sftp, &remote_dir, local_files).await {
            Ok(planned) => planned,
            Err(PlanError::TooManyCollisions { remote_path }) => {
                cancel_upload_with_toast(
                    this,
                    cx,
                    gpui::PromptLevel::Critical,
                    "Upload failed".to_string(),
                    Some(format!(
                        "SFTP: refused to overwrite existing file {remote_path}"
                    )),
                );
                return None;
            }
            Err(PlanError::MetadataFailed { remote_path, err }) => {
                cancel_upload_with_toast(
                    this,
                    cx,
                    gpui::PromptLevel::Critical,
                    "Upload failed".to_string(),
                    Some(format!("SFTP metadata failed for {remote_path}: {err}")),
                );
                return None;
            }
        };

        emit_initial_progress_events(this, cx, transfer_id, &planned);

        let (tx, rx) = smol::channel::unbounded::<UploadMsg>();
        let mut queue: std::collections::VecDeque<(usize, PlannedUploadFile)> =
            planned.into_iter().enumerate().collect();
        let mut active: usize = 0;

        try_spawn_workers(
            sftp,
            upload_pool,
            max_concurrency,
            &mut queue,
            &mut active,
            &tx,
        );

        Some(UploadLoopState {
            tx,
            rx,
            queue,
            active,
            uploaded: Vec::new(),
        })
    }

    async fn run_upload_loop(
        this: &WeakEntity<super::Terminal>,
        cx: &mut AsyncApp,
        sftp: &Sftp,
        upload_pool: &PermitPool,
        max_concurrency: usize,
        transfer_id: u64,
        state: &mut UploadLoopState,
    ) -> (Vec<(String, u64)>, u64) {
        while state.active > 0 {
            let msg = match state.rx.recv().await {
                Ok(m) => m,
                Err(_) => break,
            };

            let should_spawn = handle_upload_msg(this, cx, transfer_id, state, msg);
            if should_spawn {
                try_spawn_workers(
                    sftp,
                    upload_pool,
                    max_concurrency,
                    &mut state.queue,
                    &mut state.active,
                    &state.tx,
                );
            }

            if state.active == 0 && state.queue.is_empty() {
                break;
            }
        }

        state.uploaded.sort_by_key(|(file_index, _, _)| *file_index);
        let files: Vec<(String, u64)> = state
            .uploaded
            .drain(..)
            .map(|(_, name, bytes)| (name, bytes))
            .collect();
        let total_bytes: u64 = files.iter().map(|(_, b)| *b).sum();
        (files, total_bytes)
    }

    fn handle_upload_msg(
        this: &WeakEntity<super::Terminal>,
        cx: &mut AsyncApp,
        transfer_id: u64,
        state: &mut UploadLoopState,
        msg: UploadMsg,
    ) -> bool {
        match msg {
            UploadMsg::Progress {
                file_index,
                file,
                sent,
                total,
                cancel,
            } => {
                let _ = this.update(cx, move |_, cx| {
                    cx.emit(Event::SftpUploadFileProgress {
                        transfer_id,
                        file_index,
                        file,
                        sent,
                        total,
                        cancel,
                    });
                });
                false
            }
            UploadMsg::Finished {
                file_index,
                file,
                bytes,
            } => {
                state.record_uploaded_file(file_index, &file, bytes);
                state.finish_active_file();

                let _ = this.update(cx, move |_, cx| {
                    cx.emit(Event::SftpUploadFileFinished {
                        transfer_id,
                        file_index,
                        file,
                        bytes,
                    });
                });
                true
            }
            UploadMsg::Cancelled {
                file_index,
                file,
                sent,
                total,
            } => {
                state.cancel_active_file();

                let _ = this.update(cx, move |_, cx| {
                    cx.emit(Event::SftpUploadFileCancelled {
                        transfer_id,
                        file_index,
                        file,
                        sent,
                        total,
                    });
                });
                true
            }
            UploadMsg::Failed {
                file_index,
                file,
                sent,
                total,
                error,
            } => {
                state.cancel_active_file();

                let _ = this.update(cx, move |_, cx| {
                    cx.emit(Event::Toast {
                        level: gpui::PromptLevel::Critical,
                        title: "Upload failed".to_string(),
                        detail: Some(format!("{file}: {error}")),
                    });
                    cx.emit(Event::SftpUploadFileCancelled {
                        transfer_id,
                        file_index,
                        file,
                        sent,
                        total,
                    });
                });
                true
            }
        }
    }

    pub(crate) fn collect_local_files(paths: Vec<PathBuf>) -> Vec<LocalUploadFile> {
        let mut out: Vec<LocalUploadFile> = Vec::new();
        for path in paths {
            if !path.is_file() {
                continue;
            }
            let name = path
                .file_name()
                .and_then(|s| s.to_str())
                .map(sanitize_filename)
                .unwrap_or_else(|| "upload.bin".to_string());
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            out.push(LocalUploadFile {
                path,
                name,
                size,
                cancel: Arc::new(AtomicBool::new(false)),
            });
        }
        out
    }

    pub(crate) async fn resolve_remote_dir(
        sftp: &Sftp,
        remote_dir_hint: Option<&str>,
    ) -> Result<Utf8PathBuf, SftpChannelError> {
        if let Some(hint) = remote_dir_hint {
            return Ok(Utf8PathBuf::from(hint));
        }
        sftp.canonicalize(".").await
    }

    pub(crate) async fn plan_files(
        sftp: &Sftp,
        remote_dir: &Utf8PathBuf,
        local_files: Vec<LocalUploadFile>,
    ) -> Result<Vec<PlannedUploadFile>, PlanError> {
        let mut reserved_names: HashSet<String> = HashSet::new();
        let mut planned: Vec<PlannedUploadFile> = Vec::with_capacity(local_files.len());

        for file in local_files {
            let LocalUploadFile {
                path,
                name,
                size,
                cancel,
            } = file;

            let base = name.clone();
            let mut suffix: usize = 0;
            let mut remote_name = base.clone();
            loop {
                if reserved_names.contains(&remote_name) {
                    suffix += 1;
                    remote_name = format!("{base}.{suffix}");
                    continue;
                }

                let remote_path = remote_dir.join(&remote_name);
                match sftp.metadata(remote_path.clone()).await {
                    Ok(_) => {
                        suffix += 1;
                        if suffix > 999 {
                            return Err(PlanError::TooManyCollisions { remote_path });
                        }
                        remote_name = format!("{base}.{suffix}");
                        continue;
                    }
                    Err(err) if err.is_not_found() => {
                        reserved_names.insert(remote_name.clone());
                        planned.push(PlannedUploadFile {
                            path,
                            remote_name,
                            remote_path,
                            size,
                            cancel,
                        });
                        break;
                    }
                    Err(err) => {
                        return Err(PlanError::MetadataFailed { remote_path, err });
                    }
                }
            }
        }

        Ok(planned)
    }

    pub(crate) fn spawn_worker(
        sftp: Sftp,
        upload_pool: PermitPool,
        file_index: usize,
        file: PlannedUploadFile,
        tx: smol::channel::Sender<UploadMsg>,
    ) {
        smol::spawn(async move {
            upload_one_file(sftp, upload_pool, file_index, file, tx).await;
        })
        .detach();
    }

    async fn close_remote_best_effort(remote: &mut (impl smol::io::AsyncWrite + Unpin)) {
        let _ = remote.flush().await;
        let _ = remote.close().await;
    }

    async fn send_progress(
        tx: &smol::channel::Sender<UploadMsg>,
        file_index: usize,
        file: String,
        sent: u64,
        total: u64,
        cancel: Arc<AtomicBool>,
    ) {
        let _ = tx
            .send(UploadMsg::Progress {
                file_index,
                file,
                sent,
                total,
                cancel,
            })
            .await;
    }

    async fn send_cancelled(
        tx: &smol::channel::Sender<UploadMsg>,
        file_index: usize,
        file: String,
        sent: u64,
        total: u64,
    ) {
        let _ = tx
            .send(UploadMsg::Cancelled {
                file_index,
                file,
                sent,
                total,
            })
            .await;
    }

    async fn send_failed(
        tx: &smol::channel::Sender<UploadMsg>,
        file_index: usize,
        file: String,
        sent: u64,
        total: u64,
        error: String,
    ) {
        let _ = tx
            .send(UploadMsg::Failed {
                file_index,
                file,
                sent,
                total,
                error,
            })
            .await;
    }

    async fn send_finished(
        tx: &smol::channel::Sender<UploadMsg>,
        file_index: usize,
        file: String,
        sent: u64,
    ) {
        let _ = tx
            .send(UploadMsg::Finished {
                file_index,
                file,
                bytes: sent,
            })
            .await;
    }

    enum CopyOutcome {
        Finished { sent: u64 },
        Cancelled { sent: u64 },
        Failed { sent: u64, error: String },
    }

    struct CopyParams<'a> {
        tx: &'a smol::channel::Sender<UploadMsg>,
        file_index: usize,
        remote_name: &'a str,
        total: u64,
        path_display: &'a str,
        cancel: &'a Arc<AtomicBool>,
    }

    async fn copy_local_to_remote(
        local: &mut smol::fs::File,
        remote: &mut (impl smol::io::AsyncWrite + Unpin),
        params: CopyParams<'_>,
    ) -> CopyOutcome {
        use std::sync::atomic::Ordering;

        let CopyParams {
            tx,
            file_index,
            remote_name,
            total,
            path_display,
            cancel,
        } = params;

        let mut sent: u64 = 0;
        let mut last_emit_at = Instant::now();
        let mut buf = vec![0u8; 128 * 1024];

        loop {
            if cancel.load(Ordering::Relaxed) {
                return CopyOutcome::Cancelled { sent };
            }

            let n = match local.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => n,
                Err(err) => {
                    return CopyOutcome::Failed {
                        sent,
                        error: format!("Failed to read local file {path_display}: {err}"),
                    };
                }
            };

            if let Err(err) = remote.write_all(&buf[..n]).await {
                return CopyOutcome::Failed {
                    sent,
                    error: format!("SFTP write failed: {err}"),
                };
            }

            sent = sent.saturating_add(n as u64);

            let now = Instant::now();
            if now.duration_since(last_emit_at) >= Duration::from_millis(200) {
                last_emit_at = now;
                send_progress(
                    tx,
                    file_index,
                    remote_name.to_string(),
                    sent.min(total),
                    total,
                    Arc::clone(cancel),
                )
                .await;
            }
        }

        CopyOutcome::Finished { sent }
    }

    struct FinalizeParams<'a> {
        tx: &'a smol::channel::Sender<UploadMsg>,
        file_index: usize,
        remote_name: String,
        total: u64,
        cancel: &'a Arc<AtomicBool>,
    }

    async fn finalize_success(
        remote: &mut (impl smol::io::AsyncWrite + Unpin),
        params: FinalizeParams<'_>,
        sent: u64,
    ) {
        let FinalizeParams {
            tx,
            file_index,
            remote_name,
            total,
            cancel,
        } = params;

        close_remote_best_effort(remote).await;
        send_progress(
            tx,
            file_index,
            remote_name.clone(),
            sent.min(total),
            total,
            Arc::clone(cancel),
        )
        .await;
        send_finished(tx, file_index, remote_name, sent).await;
    }

    async fn finalize_cancelled(
        remote: &mut (impl smol::io::AsyncWrite + Unpin),
        sftp: &Sftp,
        remote_path: Utf8PathBuf,
        params: FinalizeParams<'_>,
        sent: u64,
    ) {
        let FinalizeParams {
            tx,
            file_index,
            remote_name,
            total,
            cancel,
        } = params;

        close_remote_best_effort(remote).await;
        let _ = sftp.remove_file(remote_path).await;
        send_progress(
            tx,
            file_index,
            remote_name.clone(),
            sent.min(total),
            total,
            Arc::clone(cancel),
        )
        .await;
        send_cancelled(tx, file_index, remote_name, sent.min(total), total).await;
    }

    async fn finalize_failed(
        remote: &mut (impl smol::io::AsyncWrite + Unpin),
        params: FinalizeParams<'_>,
        sent: u64,
        error: String,
    ) {
        let FinalizeParams {
            tx,
            file_index,
            remote_name,
            total,
            cancel: _,
        } = params;

        close_remote_best_effort(remote).await;
        send_failed(tx, file_index, remote_name, sent.min(total), total, error).await;
    }

    async fn upload_one_file(
        sftp: Sftp,
        upload_pool: PermitPool,
        file_index: usize,
        file: PlannedUploadFile,
        tx: smol::channel::Sender<UploadMsg>,
    ) {
        let PlannedUploadFile {
            path,
            remote_name,
            remote_path,
            size,
            cancel,
        } = file;

        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            send_cancelled(&tx, file_index, remote_name, 0, size).await;
            return;
        }

        let _permit = upload_pool.acquire().await;

        let mut remote = match sftp
            .open_with_mode(
                remote_path.clone(),
                OpenOptions {
                    read: false,
                    write: Some(WriteMode::Write),
                    mode: 0o644,
                    ty: OpenFileType::File,
                },
            )
            .await
        {
            Ok(fh) => fh,
            Err(err) => {
                send_failed(
                    &tx,
                    file_index,
                    remote_name,
                    0,
                    size,
                    format!("SFTP open failed: {err}"),
                )
                .await;
                return;
            }
        };

        let mut local = match smol::fs::File::open(&path).await {
            Ok(fh) => fh,
            Err(err) => {
                send_failed(
                    &tx,
                    file_index,
                    remote_name,
                    0,
                    size,
                    format!("Failed to open local file {}: {err}", path.display()),
                )
                .await;
                return;
            }
        };

        let path_display = path.display().to_string();
        let outcome = copy_local_to_remote(
            &mut local,
            &mut remote,
            CopyParams {
                tx: &tx,
                file_index,
                remote_name: &remote_name,
                total: size,
                path_display: &path_display,
                cancel: &cancel,
            },
        )
        .await;

        let params = FinalizeParams {
            tx: &tx,
            file_index,
            remote_name,
            total: size,
            cancel: &cancel,
        };
        match outcome {
            CopyOutcome::Finished { sent } => finalize_success(&mut remote, params, sent).await,
            CopyOutcome::Cancelled { sent } => {
                finalize_cancelled(&mut remote, &sftp, remote_path, params, sent).await;
            }
            CopyOutcome::Failed { sent, error } => {
                finalize_failed(&mut remote, params, sent, error).await;
            }
        };
    }

    fn sanitize_filename(name: &str) -> String {
        let trimmed = name.trim_matches(|c: char| c.is_whitespace() || c == '\u{0}');
        let base = trimmed.rsplit(['/', '\\']).next().unwrap_or(trimmed).trim();
        let mut out = String::with_capacity(base.len());
        for ch in base.chars() {
            match ch {
                '/' | '\\' | '\0' => {}
                _ => out.push(ch),
            }
        }
        let out = out.trim().to_string();
        if out.is_empty() {
            "upload.bin".to_string()
        } else {
            out
        }
    }
}

impl Terminal {
    /// Construct a terminal widget from a concrete backend implementation.
    ///
    /// `backend_type` is stored to allow higher layers to later rebuild/switch backend
    /// implementations while preserving the surrounding widget plumbing.
    pub fn new(backend_type: TerminalType, inner: Box<dyn TerminalBackend>) -> Self {
        Self {
            backend_type,
            inner,
            sftp_upload_active: false,
            sftp_upload_transfer_id: 0,
        }
    }

    pub fn backend_type(&self) -> TerminalType {
        self.backend_type
    }

    pub fn replace_backend(&mut self, backend_type: TerminalType, inner: Box<dyn TerminalBackend>) {
        self.backend_type = backend_type;
        self.inner = inner;
        self.sftp_upload_active = false;
        self.sftp_upload_transfer_id = 0;
    }

    pub fn sftp(&self) -> Option<wezterm_ssh::Sftp> {
        self.inner.sftp()
    }

    pub fn backend_name(&self) -> &'static str {
        self.inner.backend_name()
    }

    pub fn is_remote_mirror(&self) -> bool {
        self.inner.is_remote_mirror()
    }

    pub fn dispatch_backend_event(&mut self, event: Box<dyn Any + Send>, cx: &mut Context<Self>) {
        self.inner.handle_backend_event(event, cx);
    }

    pub fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.inner.sync(window, cx);
    }

    pub fn shutdown(&mut self, policy: TerminalShutdownPolicy, cx: &mut Context<Self>) {
        self.inner.shutdown(policy, cx);
    }

    pub fn last_content(&self) -> &TerminalContent {
        self.inner.last_content()
    }

    pub fn has_exited(&self) -> bool {
        self.inner.has_exited()
    }

    pub fn matches(&self) -> &[RangeInclusive<GridPoint>] {
        self.inner.matches()
    }

    pub fn active_match_index(&self) -> Option<usize> {
        self.inner.active_match_index()
    }

    pub fn last_line(&self) -> Option<i32> {
        self.inner.last_clicked_line()
    }

    pub fn vi_mode_enabled(&self) -> bool {
        self.inner.vi_mode_enabled()
    }

    pub fn mouse_mode(&self, shift: bool) -> bool {
        self.inner.mouse_mode(shift)
    }

    pub fn selection_started(&self) -> bool {
        self.inner.selection_started()
    }

    pub fn tail_text(&self, max_lines: usize) -> Option<String> {
        self.inner.tail_text(max_lines)
    }

    pub fn text_for_lines(&self, start_line: i64, end_line: i64) -> Option<String> {
        self.inner.text_for_lines(start_line, end_line)
    }

    pub fn command_blocks(&self) -> Option<Vec<crate::command_blocks::CommandBlock>> {
        self.inner.command_blocks()
    }

    pub fn stable_row_for_grid_line(&self, line: i32) -> Option<i64> {
        self.inner.stable_row_for_grid_line(line)
    }

    pub fn grid_line_for_stable_row(&self, stable_row: i64) -> Option<i32> {
        self.inner.grid_line_for_stable_row(stable_row)
    }

    pub fn set_selection_range(&mut self, range: Option<crate::SelectionRange>) {
        self.inner.set_selection_range(range);
    }

    pub fn set_cursor_shape(&mut self, cursor_shape: CursorShape) {
        self.inner.set_cursor_shape(cursor_shape)
    }

    pub fn total_lines(&self) -> usize {
        self.inner.total_lines()
    }

    pub fn viewport_lines(&self) -> usize {
        self.inner.viewport_lines()
    }

    pub fn total_logical_lines(&self) -> usize {
        self.inner.total_logical_lines()
    }

    pub fn logical_line_numbers_from_top(
        &self,
        start_line: usize,
        count: usize,
    ) -> Vec<Option<usize>> {
        self.inner.logical_line_numbers_from_top(start_line, count)
    }

    pub fn activate_match(&mut self, index: usize) {
        self.inner.activate_match(index)
    }

    pub fn select_matches(&mut self, matches: &[RangeInclusive<GridPoint>]) {
        self.inner.select_matches(matches)
    }

    pub fn set_search_query(&mut self, query: Option<String>) {
        self.inner.set_search_query(query)
    }

    /// Scroll the viewport to bring the given match index into view.
    ///
    /// Uses the existing scroll APIs so the behavior is backend-agnostic.
    pub fn jump_to_match(&mut self, index: usize) {
        let Some(range) = self.matches().get(index) else {
            return;
        };

        let total_lines = self.total_lines();
        let viewport_lines = self.viewport_lines().max(1);
        let max_offset = total_lines.saturating_sub(viewport_lines);
        if total_lines == 0 || max_offset == 0 {
            return;
        }

        // `GridPoint.line` is in a coordinate system where the top of the live viewport is 0.
        // Convert to an absolute row index from the top of scrollback.
        let row_from_top_i64 = max_offset as i64 + range.start().line as i64;
        if row_from_top_i64 < 0 {
            return;
        }
        let row_from_top = (row_from_top_i64 as usize).min(total_lines.saturating_sub(1));

        // Choose a viewport top so the match is comfortably visible (roughly centered).
        let half = viewport_lines / 2;
        let viewport_top_idx = row_from_top.saturating_sub(half).min(max_offset);
        let target_display_offset = max_offset.saturating_sub(viewport_top_idx);

        self.scroll_to_bottom();
        self.scroll_up_by(target_display_offset);
    }

    pub fn select_all(&mut self) {
        self.inner.select_all()
    }

    pub fn copy(&mut self, keep_selection: Option<bool>, cx: &mut Context<Self>) {
        self.inner.copy(keep_selection, cx)
    }

    pub fn clear(&mut self) {
        self.inner.clear()
    }

    pub fn scroll_line_up(&mut self) {
        self.inner.scroll_line_up()
    }

    pub fn scroll_up_by(&mut self, lines: usize) {
        self.inner.scroll_up_by(lines)
    }

    pub fn scroll_line_down(&mut self) {
        self.inner.scroll_line_down()
    }

    pub fn scroll_down_by(&mut self, lines: usize) {
        self.inner.scroll_down_by(lines)
    }

    pub fn scroll_page_up(&mut self) {
        self.inner.scroll_page_up()
    }

    pub fn scroll_page_down(&mut self) {
        self.inner.scroll_page_down()
    }

    pub fn scroll_to_top(&mut self) {
        self.inner.scroll_to_top()
    }

    pub fn scroll_to_bottom(&mut self) {
        self.inner.scroll_to_bottom()
    }

    pub fn set_size(&mut self, new_bounds: TerminalBounds) {
        self.inner.set_size(new_bounds)
    }

    pub fn input(&mut self, input: impl Into<Cow<'static, [u8]>>) {
        self.clear_selection_for_user_input();
        self.inner.input(input.into())
    }

    pub fn paste(&mut self, text: &str) {
        self.clear_selection_for_user_input();
        self.inner.paste(text)
    }

    pub fn cast_recording_active(&self) -> bool {
        self.inner.cast_recording_active()
    }

    pub fn start_cast_recording(&mut self, opts: crate::CastRecordingOptions) -> gpui::Result<()> {
        self.inner.start_cast_recording(opts)
    }

    pub fn stop_cast_recording(&mut self) {
        self.inner.stop_cast_recording()
    }

    pub fn current_dir(&self) -> Option<String> {
        self.inner.current_dir()
    }

    pub fn sftp_upload_is_active(&self) -> bool {
        self.sftp_upload_active
    }

    /// Starts uploading local files to the remote current directory via SFTP.
    ///
    /// For non-SSH terminals, `self.sftp()` returns `None` and this is a no-op.
    pub fn start_sftp_upload(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        if self.sftp_upload_active {
            cx.emit(Event::Toast {
                level: gpui::PromptLevel::Warning,
                title: "Transfer in progress".to_string(),
                detail: Some("Wait for the current upload to finish.".to_string()),
            });
            return;
        }

        let max_concurrency = TerminalSettings::global(cx)
            .sftp_upload_max_concurrency
            .clamp(1, 15);

        let upload_pool = gpui_common::set_sftp_upload_permit_pool_max(cx, max_concurrency);

        let Some(sftp) = self.sftp() else {
            return;
        };

        self.sftp_upload_transfer_id = self.sftp_upload_transfer_id.wrapping_add(1);
        let transfer_id = self.sftp_upload_transfer_id;

        let params = sftp_upload::StartParams {
            paths,
            sftp,
            upload_pool,
            max_concurrency,
            transfer_id,
            remote_dir_hint: self.current_dir(),
        };

        // Note: do *not* assume `paths` is non-empty or all are files.
        cx.spawn(async move |this, cx| sftp_upload::run_start(this, cx, params).await)
            .detach();
    }

    pub fn focus_in(&self) {
        self.inner.focus_in()
    }

    pub fn focus_out(&mut self) {
        self.inner.focus_out()
    }

    pub fn toggle_vi_mode(&mut self) {
        self.inner.toggle_vi_mode()
    }

    pub fn try_keystroke(&mut self, keystroke: &Keystroke, alt_is_meta: bool) -> bool {
        self.clear_selection_for_user_input();
        if let Some(text) = plain_text_for_keystroke(keystroke) {
            self.inner.input(Cow::Owned(text.into_bytes()));
            true
        } else {
            self.inner.try_keystroke(keystroke, alt_is_meta)
        }
    }

    pub fn try_modifiers_change(
        &mut self,
        modifiers: &Modifiers,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        self.inner.try_modifiers_change(modifiers, window, cx)
    }

    pub fn mouse_move(&mut self, e: &MouseMoveEvent, cx: &mut Context<Self>) {
        self.inner.mouse_move(e, cx)
    }

    pub fn select_word_at_event_position(&mut self, e: &MouseDownEvent) {
        self.inner.select_word_at_event_position(e)
    }

    pub fn mouse_drag(
        &mut self,
        e: &MouseMoveEvent,
        region: Bounds<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.inner.mouse_drag(e, region, cx)
    }

    pub fn mouse_down(&mut self, e: &MouseDownEvent, cx: &mut Context<Self>) {
        self.inner.mouse_down(e, cx)
    }

    pub fn mouse_up(&mut self, e: &MouseUpEvent, cx: &Context<Self>) {
        self.inner.mouse_up(e, cx)
    }

    pub fn scroll_wheel(&mut self, e: &ScrollWheelEvent) {
        self.inner.scroll_wheel(e)
    }

    pub fn get_content(&self) -> String {
        self.inner.get_content()
    }

    pub fn last_n_non_empty_lines(&self, n: usize) -> Vec<String> {
        self.inner.last_n_non_empty_lines(n)
    }

    pub fn preview_lines_from_top(&self, start_line: usize, count: usize) -> Vec<String> {
        self.inner.preview_lines_from_top(start_line, count)
    }

    pub fn preview_cells_from_top(
        &self,
        start_line: usize,
        count: usize,
    ) -> (usize, usize, Vec<IndexedCell>) {
        self.inner.preview_cells_from_top(start_line, count)
    }

    pub fn scrollback_top_line_id(&self) -> i64 {
        self.inner.scrollback_top_line_id()
    }

    pub fn cursor_line_id(&self) -> Option<i64> {
        self.inner.cursor_line_id()
    }

    fn clear_selection_for_user_input(&mut self) {
        if self.inner.last_content().selection.is_some() {
            self.inner.clear_selection();
        }
    }
}

impl EventEmitter<Event> for Terminal {}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalBounds {
    pub cell_width: Pixels,
    pub line_height: Pixels,
    pub bounds: Bounds<Pixels>,
}

impl TerminalBounds {
    pub fn new(line_height: Pixels, cell_width: Pixels, bounds: Bounds<Pixels>) -> Self {
        Self {
            cell_width,
            line_height,
            bounds,
        }
    }

    pub fn num_lines(&self) -> usize {
        (self.bounds.size.height / self.line_height).floor() as usize
    }

    pub fn num_columns(&self) -> usize {
        (self.bounds.size.width / self.cell_width).floor() as usize
    }

    pub fn last_column(&self) -> usize {
        self.num_columns().saturating_sub(1)
    }

    pub fn height(&self) -> Pixels {
        self.bounds.size.height
    }

    pub fn width(&self) -> Pixels {
        self.bounds.size.width
    }

    pub fn cell_width(&self) -> Pixels {
        self.cell_width
    }

    pub fn line_height(&self) -> Pixels {
        self.line_height
    }
}

impl Default for TerminalBounds {
    fn default() -> Self {
        Self::new(
            px(5.0),
            px(5.0),
            Bounds {
                origin: Point::default(),
                size: gpui::Size {
                    width: px(500.0),
                    height: px(30.0),
                },
            },
        )
    }
}

/// Convenience for backends to copy selection text to the system clipboard.
pub(crate) fn write_clipboard(cx: &mut Context<Terminal>, text: String) {
    cx.write_to_clipboard(ClipboardItem::new_string(text));
}

fn plain_text_for_keystroke(keystroke: &Keystroke) -> Option<String> {
    if keystroke.is_ime_in_progress() {
        return None;
    }
    if keystroke.modifiers.control
        || keystroke.modifiers.platform
        || keystroke.modifiers.function
        || keystroke.modifiers.alt
    {
        return None;
    }
    if matches!(keystroke.key.as_str(), "tab" | "escape" | "enter") {
        return None;
    }
    keystroke
        .key_char
        .as_deref()
        .map(str::to_string)
        .filter(|v| !v.is_empty())
}

#[cfg(test)]
mod cast_recording_tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use super::*;

    struct FakeBackend {
        active: bool,
        stop_calls: Arc<AtomicUsize>,
    }

    impl TerminalBackend for FakeBackend {
        fn backend_name(&self) -> &'static str {
            "fake"
        }

        fn sync(&mut self, _window: &mut Window, _cx: &mut Context<Terminal>) {}

        fn last_content(&self) -> &TerminalContent {
            static CONTENT: std::sync::OnceLock<TerminalContent> = std::sync::OnceLock::new();
            CONTENT.get_or_init(TerminalContent::default)
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

        fn copy(&mut self, _keep_selection: Option<bool>, _cx: &mut Context<Terminal>) {}

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

        fn set_size(&mut self, _new_bounds: TerminalBounds) {}

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
            _cx: &mut Context<Terminal>,
        ) {
        }

        fn mouse_move(&mut self, _e: &MouseMoveEvent, _cx: &mut Context<Terminal>) {}

        fn select_word_at_event_position(&mut self, _e: &MouseDownEvent) {}

        fn mouse_drag(
            &mut self,
            _e: &MouseMoveEvent,
            _region: Bounds<Pixels>,
            _cx: &mut Context<Terminal>,
        ) {
        }

        fn mouse_down(&mut self, _e: &MouseDownEvent, _cx: &mut Context<Terminal>) {}

        fn mouse_up(&mut self, _e: &MouseUpEvent, _cx: &Context<Terminal>) {}

        fn scroll_wheel(&mut self, _e: &ScrollWheelEvent) {}

        fn get_content(&self) -> String {
            String::new()
        }

        fn last_n_non_empty_lines(&self, _n: usize) -> Vec<String> {
            Vec::new()
        }

        fn cast_recording_active(&self) -> bool {
            self.active
        }

        fn start_cast_recording(&mut self, _opts: crate::CastRecordingOptions) -> gpui::Result<()> {
            self.active = true;
            Ok(())
        }

        fn stop_cast_recording(&mut self) {
            self.active = false;
            self.stop_calls.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn terminal_forwards_cast_recording_calls_to_backend() {
        let stop_calls = Arc::new(AtomicUsize::new(0));
        let mut terminal = Terminal::new(
            TerminalType::WezTerm,
            Box::new(FakeBackend {
                active: false,
                stop_calls: Arc::clone(&stop_calls),
            }),
        );
        assert!(!terminal.cast_recording_active());

        terminal
            .start_cast_recording(crate::CastRecordingOptions {
                path: std::path::PathBuf::from("/tmp/test.cast"),
                include_input: false,
            })
            .unwrap();
        assert!(terminal.cast_recording_active());

        terminal.stop_cast_recording();
        assert!(!terminal.cast_recording_active());
        assert_eq!(stop_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn terminal_drop_stops_cast_recording() {
        let stop_calls = Arc::new(AtomicUsize::new(0));
        {
            let _terminal = Terminal::new(
                TerminalType::WezTerm,
                Box::new(FakeBackend {
                    active: true,
                    stop_calls: Arc::clone(&stop_calls),
                }),
            );
        }

        assert_eq!(stop_calls.load(Ordering::SeqCst), 1);
    }
}

#[cfg(test)]
mod keystroke_text_input_tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    struct CapturingBackend {
        input_calls: Arc<Mutex<Vec<Vec<u8>>>>,
        keystroke_calls: Arc<Mutex<Vec<String>>>,
    }

    impl TerminalBackend for CapturingBackend {
        fn backend_name(&self) -> &'static str {
            "capture"
        }

        fn sync(&mut self, _window: &mut Window, _cx: &mut Context<Terminal>) {}

        fn last_content(&self) -> &TerminalContent {
            static CONTENT: std::sync::OnceLock<TerminalContent> = std::sync::OnceLock::new();
            CONTENT.get_or_init(TerminalContent::default)
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
        fn copy(&mut self, _keep_selection: Option<bool>, _cx: &mut Context<Terminal>) {}
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
        fn set_size(&mut self, _new_bounds: TerminalBounds) {}

        fn input(&mut self, input: Cow<'static, [u8]>) {
            self.input_calls.lock().unwrap().push(input.into_owned());
        }

        fn paste(&mut self, _text: &str) {}

        fn focus_in(&self) {}
        fn focus_out(&mut self) {}
        fn toggle_vi_mode(&mut self) {}

        fn try_keystroke(&mut self, keystroke: &Keystroke, _alt_is_meta: bool) -> bool {
            self.keystroke_calls
                .lock()
                .unwrap()
                .push(keystroke.unparse());
            true
        }

        fn try_modifiers_change(
            &mut self,
            _modifiers: &Modifiers,
            _window: &Window,
            _cx: &mut Context<Terminal>,
        ) {
        }

        fn mouse_move(&mut self, _e: &MouseMoveEvent, _cx: &mut Context<Terminal>) {}
        fn select_word_at_event_position(&mut self, _e: &MouseDownEvent) {}
        fn mouse_drag(
            &mut self,
            _e: &MouseMoveEvent,
            _region: Bounds<Pixels>,
            _cx: &mut Context<Terminal>,
        ) {
        }
        fn mouse_down(&mut self, _e: &MouseDownEvent, _cx: &mut Context<Terminal>) {}
        fn mouse_up(&mut self, _e: &MouseUpEvent, _cx: &Context<Terminal>) {}
        fn scroll_wheel(&mut self, _e: &ScrollWheelEvent) {}
        fn get_content(&self) -> String {
            String::new()
        }
        fn last_n_non_empty_lines(&self, _n: usize) -> Vec<String> {
            Vec::new()
        }
    }

    #[test]
    fn terminal_try_keystroke_uses_key_char_for_plain_text() {
        let input_calls = Arc::new(Mutex::new(Vec::new()));
        let keystroke_calls = Arc::new(Mutex::new(Vec::new()));

        let mut terminal = Terminal::new(
            TerminalType::WezTerm,
            Box::new(CapturingBackend {
                input_calls: Arc::clone(&input_calls),
                keystroke_calls: Arc::clone(&keystroke_calls),
            }),
        );

        // Simulate CapsLock: key is printed label ("a"), but typed character is uppercase.
        let k = Keystroke {
            modifiers: Modifiers::none(),
            key: "a".to_string(),
            key_char: Some("A".to_string()),
        };
        assert!(terminal.try_keystroke(&k, false));

        assert_eq!(input_calls.lock().unwrap().as_slice(), &[b"A".to_vec()]);
        assert!(keystroke_calls.lock().unwrap().is_empty());
    }

    #[test]
    fn terminal_try_keystroke_forwards_ctrl_to_backend() {
        let input_calls = Arc::new(Mutex::new(Vec::new()));
        let keystroke_calls = Arc::new(Mutex::new(Vec::new()));

        let mut terminal = Terminal::new(
            TerminalType::WezTerm,
            Box::new(CapturingBackend {
                input_calls: Arc::clone(&input_calls),
                keystroke_calls: Arc::clone(&keystroke_calls),
            }),
        );

        let k = Keystroke {
            modifiers: Modifiers {
                control: true,
                ..Modifiers::none()
            },
            key: "c".to_string(),
            key_char: None,
        };
        assert!(terminal.try_keystroke(&k, false));

        assert!(input_calls.lock().unwrap().is_empty());
        assert_eq!(
            keystroke_calls.lock().unwrap().as_slice(),
            &["ctrl-c".to_string()]
        );
    }

    #[test]
    fn terminal_try_keystroke_does_not_convert_enter_to_text() {
        let input_calls = Arc::new(Mutex::new(Vec::new()));
        let keystroke_calls = Arc::new(Mutex::new(Vec::new()));

        let mut terminal = Terminal::new(
            TerminalType::WezTerm,
            Box::new(CapturingBackend {
                input_calls: Arc::clone(&input_calls),
                keystroke_calls: Arc::clone(&keystroke_calls),
            }),
        );

        let k = Keystroke {
            modifiers: Modifiers::none(),
            key: "enter".to_string(),
            key_char: Some("\n".to_string()),
        };
        assert!(terminal.try_keystroke(&k, false));

        assert!(input_calls.lock().unwrap().is_empty());
        assert_eq!(
            keystroke_calls.lock().unwrap().as_slice(),
            &["enter".to_string()]
        );
    }
}
