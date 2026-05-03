use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use camino::Utf8PathBuf;
use gpui::{
    App, AppContext, Bounds, Context, Div, Entity, EventEmitter, ExternalPaths, FocusHandle,
    Focusable, Image, ImageSource, InteractiveElement, IntoElement, MouseButton, MouseDownEvent,
    ParentElement, PathPromptOptions, PromptLevel, Render, SharedString, Stateful, Styled,
    StyledImage, Subscription, Window, canvas, div, img, prelude::FluentBuilder, px,
};
use gpui_common::TermuaIcon;
use gpui_component::{
    ActiveTheme, Icon, IconName, Sizable, Size as UiSize, WindowExt,
    breadcrumb::{Breadcrumb, BreadcrumbItem},
    button::{Button, ButtonVariant, ButtonVariants},
    dialog::DialogButtonProps,
    input::{Input, InputEvent, InputState},
    menu::{ContextMenuExt, DropdownMenu, PopupMenu, PopupMenuItem},
    notification::Notification,
    scroll::ScrollableElement,
    table::{Column, DataTable, TableDelegate, TableEvent, TableState},
    text::TextView,
};
use gpui_transfer::{
    AUTO_DISMISS_AFTER, TransferCenterState, TransferKind, TransferProgress, TransferStatus,
    TransferTask,
};
use smol::{
    Timer,
    io::{AsyncReadExt, AsyncWriteExt},
};
use time::OffsetDateTime;
use wezterm_ssh::{FilePermissions, FileType, Metadata, OpenFileType, OpenOptions, WriteMode};

use super::{
    Delete, Download, Entry, EntryKind, NewFolder, Refresh, Rename, SortColumn, SortDirection,
    SortSpec, TreeState, Upload, VisibleRow,
};
use crate::{
    preview::{PreviewGate, gate_preview},
    state::{breadcrumbs_for_path, display_name_for_dir},
};

mod file_icons;
mod format;
mod path;
mod preview;
mod render;
mod table;

use format::{default_download_dir, entry_from_meta, format_modified, format_size};
use path::{apply_hidden_filter, join_remote, parent_dir};
use preview::{PreviewContent, PreviewPane, PreviewTarget};

#[derive(Clone, Debug)]
enum Transfer {
    Upload {
        name: String,
        sent: u64,
        total: u64,
    },
    Download {
        name: String,
        received: u64,
        total: Option<u64>,
    },
    Finished {
        title: String,
    },
}

#[derive(Clone, Debug)]
struct TransferEntry {
    transfer: Transfer,
    cancel: Option<Arc<AtomicBool>>,
    detail: Option<SharedString>,
    group_id: Option<String>,
    group_total: Option<usize>,
}

#[derive(Clone, Debug)]
struct PendingToast {
    level: PromptLevel,
    title: String,
    detail: Option<String>,
}

#[derive(Clone, Debug)]
enum SftpOpKind {
    NewFolder { parent: String },
    Rename { target: String, parent: String },
}

#[derive(Clone)]
struct SftpOp {
    kind: SftpOpKind,
    input: Entity<InputState>,
}

static NEXT_TRANSFER_EPOCH: AtomicUsize = AtomicUsize::new(1);

fn next_transfer_epoch() -> usize {
    NEXT_TRANSFER_EPOCH.fetch_add(1, Ordering::Relaxed)
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum ContextMenuAction {
    Refresh,
    Upload,
    Download,
    NewFolder,
    Rename,
    Delete,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum ContextMenu {
    Action(ContextMenuAction),
    Separator,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum ContextMenuTarget {
    Background,
    Single(EntryKind),
    Multi { has_file: bool, has_dir: bool },
}

fn delete_selected_item_title(name: Option<&str>) -> String {
    match name {
        Some(name) if !name.trim().is_empty() => format!("Delete \"{name}\"?"),
        _ => "Delete selected item?".to_string(),
    }
}

fn sftp_context_menu(target: ContextMenuTarget) -> Vec<ContextMenu> {
    use ContextMenu as S;
    use ContextMenuAction as A;

    let mut out = vec![S::Action(A::Refresh), S::Separator];

    match target {
        ContextMenuTarget::Background => {
            out.extend([S::Action(A::Upload), S::Separator, S::Action(A::NewFolder)]);
        }
        ContextMenuTarget::Single(EntryKind::Dir) => {
            out.extend([
                S::Action(A::Upload),
                S::Separator,
                S::Action(A::NewFolder),
                S::Action(A::Rename),
                S::Separator,
                S::Action(A::Delete),
            ]);
        }
        ContextMenuTarget::Single(_) => {
            out.extend([
                S::Action(A::Download),
                S::Separator,
                S::Action(A::Rename),
                S::Separator,
                S::Action(A::Delete),
            ]);
        }
        ContextMenuTarget::Multi { .. } => {
            out.push(S::Action(A::Delete));
        }
    }

    out
}

fn sftp_table_columns() -> Vec<Column> {
    // Keys are stable identifiers used by Table; keep them short and ASCII.
    vec![
        Column::new("name", "Name").width(px(360.0)).resizable(true),
        Column::new("size", "Size")
            .width(px(120.0))
            .text_right()
            .resizable(true),
        Column::new("modified", "Modified")
            .width(px(180.0))
            .resizable(true),
        Column::new("perms", "Perms")
            .width(px(120.0))
            .resizable(true),
    ]
}

fn sftp_dir_icon_path(expanded: bool) -> TermuaIcon {
    if expanded {
        TermuaIcon::FolderOpenBlue
    } else {
        TermuaIcon::FolderClosedBlue
    }
}

fn accept_external_file_drop_paths(paths: &[PathBuf]) -> bool {
    !paths.is_empty() && paths.iter().all(|p| p.is_file())
}

fn folder_drop_ring_shadow(border: gpui::Hsla) -> Vec<gpui::BoxShadow> {
    vec![gpui::BoxShadow {
        color: border.opacity(0.25),
        offset: gpui::point(px(0.0), px(0.0)),
        blur_radius: px(0.0),
        spread_radius: px(2.0),
    }]
}

fn table_row_ix_from_mouse_y(
    body_y: gpui::Pixels,
    scroll_offset_y: gpui::Pixels,
    row_h: gpui::Pixels,
) -> Option<usize> {
    // `scroll_offset_y` is a content translation. When scrolled down, it is typically negative.
    // Convert viewport-relative y into content-relative y by subtracting the translation.
    if body_y < px(0.0) {
        return None;
    }
    let y = body_y - scroll_offset_y;
    if y < px(0.0) {
        return None;
    }
    Some((y.as_f32() / row_h.as_f32()).floor() as usize)
}

struct SftpTable {
    sftp: Option<wezterm_ssh::Sftp>,
    tree: Option<TreeState>,
    loading: HashSet<String>,

    show_hidden: bool,

    selected_ids: HashSet<String>,
    selection_anchor_id: Option<String>,

    columns: Vec<Column>,
    sort: SortSpec,
    visible: Vec<VisibleRow>,

    context_row: Option<usize>,
    pending_toast: Option<PendingToast>,
    pending_toast_epoch: usize,

    transfers: HashMap<usize, TransferEntry>,

    op: Option<SftpOp>,
}

pub struct SftpView {
    table: Entity<TableState<SftpTable>>,
    path_input: Entity<InputState>,
    path_editing: bool,
    table_bounds: Bounds<gpui::Pixels>,
    last_row_activate: Option<(usize, Instant)>,
    preview: PreviewPane,
    preview_epoch: usize,
    show_preview: bool,
    last_pushed_notification_epoch: usize,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<()> for SftpView {}

impl Focusable for SftpView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn new_input<T>(
    window: &mut Window,
    cx: &mut Context<T>,
    placeholder: impl Into<String>,
) -> Entity<InputState> {
    let placeholder = placeholder.into();
    cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
}

fn new_configured_input<T, F>(
    window: &mut Window,
    cx: &mut Context<T>,
    placeholder: impl Into<String>,
    configure: F,
) -> Entity<InputState>
where
    F: FnOnce(InputState) -> InputState,
{
    let placeholder = placeholder.into();
    cx.new(|cx| configure(InputState::new(window, cx).placeholder(placeholder)))
}

impl SftpView {
    pub fn new(sftp: wezterm_ssh::Sftp, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let table = cx.new(|cx| {
            TableState::new(SftpTable::new(sftp), window, cx)
                .sortable(false)
                .col_selectable(false)
                .col_movable(false)
        });
        let path_input = new_input(window, cx, "Path");

        // Bootstrap the root listing.
        table.update(cx, |state, cx| {
            state.delegate_mut().bootstrap_root(cx);
        });

        let path_sub = Self::subscribe_path_input(&path_input, &table, window, cx);
        let table_events = Self::subscribe_table_events(&table, window, cx);

        Self {
            table,
            path_input,
            path_editing: false,
            table_bounds: Bounds::default(),
            last_row_activate: None,
            preview: PreviewPane {
                target: None,
                content: PreviewContent::Empty,
            },
            preview_epoch: 0,
            show_preview: false,
            last_pushed_notification_epoch: 0,
            focus_handle,
            _subscriptions: vec![path_sub, table_events],
        }
    }

    fn subscribe_path_input(
        path_input: &Entity<InputState>,
        table: &Entity<TableState<SftpTable>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Subscription {
        let table = table.clone();
        cx.subscribe_in(path_input, window, move |this, input, ev, window, cx| {
            this.handle_path_input_event(&table, input, ev, window, cx);
        })
    }

    fn handle_path_input_event(
        &mut self,
        table: &Entity<TableState<SftpTable>>,
        input: &Entity<InputState>,
        ev: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match ev {
            InputEvent::PressEnter { .. } => {
                let path = input.read(cx).value().to_string();
                let path = path.trim();
                if !path.is_empty() {
                    let path = path.to_string();
                    table.update(cx, |state, cx| {
                        state.delegate_mut().cd(path, cx);
                    });
                }

                self.close_preview(cx);
                self.path_editing = false;
                window.focus(&self.focus_handle(cx), cx);
                cx.notify();
            }
            InputEvent::Blur => {
                if self.path_editing {
                    self.path_editing = false;
                    cx.notify();
                }
            }
            _ => {}
        }
    }

    fn subscribe_table_events(
        table: &Entity<TableState<SftpTable>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Subscription {
        let table_entity = table.clone();
        let table_for_events = table_entity.clone();
        cx.subscribe_in(
            &table_entity,
            window,
            move |this, _table_state, ev, _window, cx| {
                this.handle_table_event(&table_for_events, ev, cx);
            },
        )
    }

    fn handle_table_event(
        &mut self,
        table: &Entity<TableState<SftpTable>>,
        ev: &TableEvent,
        cx: &mut Context<Self>,
    ) {
        // Prefer Table's native double-click events, but also provide a fallback path:
        // on some platforms/backends, click_count can be unreliable, so we treat a fast
        // repeated SelectRow on the same row as an "activate".
        let mut activate_row: Option<usize> = None;

        match ev {
            TableEvent::DoubleClickedRow(row_ix) => activate_row = Some(*row_ix),
            TableEvent::DoubleClickedCell(row_ix, _col_ix) => activate_row = Some(*row_ix),
            TableEvent::SelectRow(row_ix) => {
                self.handle_table_row_selected(table, *row_ix, &mut activate_row, cx);
            }
            TableEvent::ClearSelection => self.close_preview(cx),
            _ => {}
        }

        if let Some(row_ix) = activate_row {
            let dir_id = table
                .read(cx)
                .delegate()
                .row(row_ix)
                .and_then(|row| (row.kind == EntryKind::Dir).then(|| row.id.clone()));
            if let Some(dir_id) = dir_id {
                self.close_preview(cx);
                table.update(cx, |state, cx| state.delegate_mut().cd(dir_id, cx));
                cx.stop_propagation();
            }
        }
    }

    fn handle_table_row_selected(
        &mut self,
        table: &Entity<TableState<SftpTable>>,
        row_ix: usize,
        activate_row: &mut Option<usize>,
        cx: &mut Context<Self>,
    ) {
        let target = table
            .read(cx)
            .delegate()
            .row(row_ix)
            .map(|row| PreviewTarget {
                id: row.id.clone(),
                name: row.name.clone(),
                kind: row.kind,
                size: row.size,
            });

        // Clicking a previewable file opens the preview pane. Selecting anything else closes it.
        let gate = target
            .as_ref()
            .map(|t| gate_preview(true, &t.name, t.kind, t.size))
            .unwrap_or(PreviewGate::Hidden);
        match gate {
            PreviewGate::Allowed { .. } => {
                self.show_preview = true;
                self.request_preview(target, cx);
            }
            PreviewGate::Hidden | PreviewGate::TooLarge { .. } => self.close_preview(cx),
        }

        let now = Instant::now();
        // Use a short threshold similar to typical OS double-click timing.
        let threshold = Duration::from_millis(450);
        let is_fast_repeat = self
            .last_row_activate
            .map(|(prev_ix, prev_at)| prev_ix == row_ix && now.duration_since(prev_at) <= threshold)
            .unwrap_or(false);
        self.last_row_activate = Some((row_ix, now));
        if is_fast_repeat {
            *activate_row = Some(row_ix);
        }
    }

    pub fn disconnect(&mut self, cx: &mut Context<Self>) {
        self.table
            .update(cx, |state, cx| state.delegate_mut().disconnect(cx));
    }

    fn on_refresh(&mut self, _: &Refresh, _window: &mut Window, cx: &mut Context<Self>) {
        let table = self.table.clone();
        table.update(cx, |state, cx| {
            let target = state.selected_row();
            state.delegate_mut().refresh_at(target, cx);
        });
    }

    fn on_upload(&mut self, _: &Upload, window: &mut Window, cx: &mut Context<Self>) {
        let table = self.table.clone();
        table.update(cx, |state, cx| {
            let target = state
                .delegate_mut()
                .context_row
                .take()
                .or(state.selected_row());
            state.delegate_mut().upload(target, window, cx)
        });
    }

    fn on_download(&mut self, _: &Download, window: &mut Window, cx: &mut Context<Self>) {
        let table = self.table.clone();
        table.update(cx, |state, cx| {
            let target = state
                .delegate_mut()
                .context_row
                .take()
                .or(state.selected_row());
            state.delegate_mut().download(target, window, cx)
        });
    }

    fn on_new_folder(&mut self, _: &NewFolder, window: &mut Window, cx: &mut Context<Self>) {
        let table = self.table.clone();
        table.update(cx, |state, cx| {
            let target = state
                .delegate_mut()
                .context_row
                .take()
                .or(state.selected_row());
            state.delegate_mut().open_new_folder(target, window, cx)
        });
    }

    fn on_rename(&mut self, _: &Rename, window: &mut Window, cx: &mut Context<Self>) {
        let table = self.table.clone();
        table.update(cx, |state, cx| {
            let target = state
                .delegate_mut()
                .context_row
                .take()
                .or(state.selected_row());
            state.delegate_mut().open_rename(target, window, cx)
        });
    }

    fn on_delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        let table = self.table.clone();
        window.open_alert_dialog(cx, move |dialog, _window, _cx| {
            let table = table.clone();
            let (targets_count, selected_name) = {
                let state = table.read(_cx);
                let target_row = state.delegate().context_row.or(state.selected_row());
                let ids = state.delegate().delete_target_ids(target_row);
                let name = if ids.len() == 1 {
                    state
                        .delegate()
                        .visible
                        .iter()
                        .find(|row| row.id == ids[0])
                        .map(|row| row.name.as_str())
                } else {
                    None
                };
                (ids.len(), name)
            };
            let title = if targets_count <= 1 {
                delete_selected_item_title(selected_name)
            } else {
                format!("Delete {targets_count} items?")
            };
            dialog
                .title(title)
                .confirm()
                .button_props(
                    DialogButtonProps::default()
                        .ok_text("Delete")
                        .ok_variant(ButtonVariant::Danger)
                        .cancel_text("Cancel"),
                )
                .on_ok(move |_e, window, cx| {
                    table.update(cx, |state, cx| {
                        let target_row = state
                            .delegate_mut()
                            .context_row
                            .take()
                            .or(state.selected_row());
                        let ids = state.delegate().delete_target_ids(target_row);
                        state.delegate_mut().delete_selected_ids(ids, window, cx);
                    });
                    true
                })
        });
    }

    fn close(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let table = self.table.clone();
        table.update(cx, |state, cx| state.delegate_mut().close(cx));
    }

    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let table = self.table.clone();
        table.update(cx, |state, cx| state.delegate_mut().confirm(window, cx));
    }
}

#[cfg(test)]
mod tests;
