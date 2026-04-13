//! TermuaWindow behavior and event handling.

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use gpui::{
    App, AppContext, Context, FocusHandle, Focusable, InteractiveElement, IntoElement,
    ParentElement, ReadGlobal, SharedString, Styled, Window, div, px,
};
use gpui_common::{TermuaIcon, format_bytes};
use gpui_component::{
    Icon,
    button::{Button, ButtonVariants},
    h_flex, v_flex,
};
use gpui_dock::{DockPlacement, PanelView};
use gpui_term::{
    Authentication, CursorShape, Event as TerminalEvent, PtySource, RemoteBackendEvent,
    SerialFlowControl, SerialOptions, SerialParity, SerialStopBits, SshOptions, TerminalBuilder,
    TerminalSettings, TerminalType, TerminalView, UserInput as TerminalUserInput,
    remote::{RemoteFrame, RemoteInputEvent, RemoteSnapshot, RemoteTerminalContent},
};
use gpui_transfer::{
    AUTO_DISMISS_AFTER, TransferCenterState, TransferKind, TransferProgress, TransferStatus,
    TransferTask,
};
use rust_i18n::t;
use smol::Timer;

use super::TermuaWindow;
use crate::{
    NewLocalTerminal, OpenSftp, PendingCommand, PlayCast, TermuaAppState,
    env::{build_local_terminal_env, cast_player_child_env},
    lock_screen, notification,
    panel::{PanelKind, SshErrorPanel, TerminalPanel, terminal_panel_tab_name},
    sharing::{
        ClientToRelay as RelayClientToRelay, HostShare, RelaySharingState,
        RelayToClient as RelayRelayToClient, ReleaseControl, RequestControl, RevokeControl,
        StartSharing, StopSharing, ViewerShare, compose_share_key, connect_relay, gen_join_key,
        gen_room_id, parse_share_key,
    },
    ssh::{
        SshHostKeyMismatchDetails, dedupe_tab_label, default_known_hosts_path,
        parse_ssh_host_key_mismatch, remove_known_host_entry, ssh_connect_failure_message,
        ssh_proxy_from_session, ssh_tab_tooltip, ssh_target_label,
    },
};

impl TermuaWindow {
    fn has_open_tabs(&self, cx: &App) -> bool {
        self.dock_area
            .read(cx)
            .visible_tab_panels(cx)
            .into_iter()
            .any(|tab_panel| tab_panel.read(cx).active_panel(cx).is_some())
    }

    fn open_quit_confirm_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Some(_)) = window.root::<gpui_component::Root>() else {
            cx.quit();
            return;
        };

        window.defer(cx, move |window, app| {
            gpui_component::Root::update(window, app, |root, window, cx| {
                root.open_dialog(
                    move |dialog, _window, _app| {
                        dialog
                            .title(t!("MainWindow.QuitConfirm.Title").to_string())
                            .child(
                                div()
                                    .debug_selector(|| "termua-quit-confirm-body".to_string())
                                    .child(t!("MainWindow.QuitConfirm.Body").to_string()),
                            )
                            .button_props(
                                gpui_component::dialog::DialogButtonProps::default()
                                    .ok_text(t!("MainWindow.QuitConfirm.Button.Quit").to_string())
                                    .cancel_text(
                                        t!("MainWindow.QuitConfirm.Button.Cancel").to_string(),
                                    ),
                            )
                            .on_ok(|_, _window, app| {
                                app.quit();
                                true
                            })
                            .confirm()
                    },
                    window,
                    cx,
                );
            });
        });
    }

    pub(crate) fn request_quit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.has_open_tabs(cx) {
            self.open_quit_confirm_dialog(window, cx);
        } else {
            cx.quit();
        }
    }

    fn sftp_upload_panel_prefix(panel_id: usize) -> String {
        format!("sftp-upload-{panel_id}-")
    }

    fn sftp_upload_group_id(panel_id: usize, transfer_id: u64) -> String {
        format!("sftp-upload-{panel_id}-{transfer_id}")
    }

    fn sftp_upload_task_id(panel_id: usize, transfer_id: u64, file_index: usize) -> String {
        format!(
            "{}-{file_index}",
            Self::sftp_upload_group_id(panel_id, transfer_id)
        )
    }

    pub(crate) fn subscribe_terminal_events_for_messages(
        &mut self,
        terminal: gpui::Entity<gpui_term::Terminal>,
        panel_id: usize,
        tab_label: gpui::SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let sub = cx.subscribe_in(
            &terminal,
            window,
            move |_this, _terminal, event, window, cx| {
                Self::handle_terminal_event_for_messages(panel_id, &tab_label, event, window, cx);
            },
        );
        self._subscriptions.push(sub);
    }

    fn handle_terminal_event_for_messages(
        panel_id: usize,
        tab_label: &SharedString,
        event: &TerminalEvent,
        window: &mut Window,
        cx: &mut Context<TermuaWindow>,
    ) {
        if Self::handle_terminal_event_toast(event, window, cx) {
            return;
        }
        let _ = Self::handle_terminal_event_sftp_upload(panel_id, tab_label, event, window, cx);
    }

    fn handle_terminal_event_toast(
        event: &TerminalEvent,
        window: &mut Window,
        cx: &mut Context<TermuaWindow>,
    ) -> bool {
        match event {
            TerminalEvent::Toast {
                level,
                title,
                detail,
            } => {
                let kind = match level {
                    gpui::PromptLevel::Info => crate::notification::MessageKind::Info,
                    gpui::PromptLevel::Warning => crate::notification::MessageKind::Warning,
                    gpui::PromptLevel::Critical => crate::notification::MessageKind::Error,
                };
                let message = match detail.as_deref() {
                    Some(detail) if !detail.trim().is_empty() => format!("{title}\n{detail}"),
                    _ => title.clone(),
                };
                crate::notification::notify(kind, message, window, cx);
                true
            }
            _ => false,
        }
    }

    fn upsert_transfer(task: TransferTask, cx: &mut Context<TermuaWindow>) {
        if cx.try_global::<TransferCenterState>().is_none() {
            return;
        }
        cx.global_mut::<TransferCenterState>().upsert(task);
    }

    fn remove_transfer(id: &str, cx: &mut Context<TermuaWindow>) {
        if cx.try_global::<TransferCenterState>().is_none() {
            return;
        }
        cx.global_mut::<TransferCenterState>().remove(id);
    }

    fn sftp_upload_progress(sent: u64, total: u64) -> TransferProgress {
        if total > 0 {
            TransferProgress::Determinate((sent as f32 / total as f32).clamp(0.0, 1.0))
        } else {
            TransferProgress::Determinate(1.0)
        }
    }

    fn build_sftp_upload_task(
        panel_id: usize,
        transfer_id: u64,
        file_index: usize,
        file: &str,
        status: TransferStatus,
        sent: u64,
        total: u64,
        cancel: Option<&Arc<std::sync::atomic::AtomicBool>>,
    ) -> (String, TransferTask) {
        let group_id = Self::sftp_upload_group_id(panel_id, transfer_id);
        let id = Self::sftp_upload_task_id(panel_id, transfer_id, file_index);
        let task = TransferTask::new(id.clone(), SharedString::from(file.to_string()))
            .with_group(group_id, Some(file_index.saturating_add(1)))
            .with_kind(TransferKind::Upload)
            .with_status(status)
            .with_progress(Self::sftp_upload_progress(sent, total))
            .with_bytes(Some(sent), Some(total).filter(|t| *t > 0));

        let task = if let Some(cancel) = cancel {
            task.with_cancel_token(Arc::clone(cancel))
        } else {
            task
        };

        (id, task)
    }

    fn handle_terminal_event_sftp_upload(
        panel_id: usize,
        tab_label: &SharedString,
        event: &TerminalEvent,
        window: &mut Window,
        cx: &mut Context<TermuaWindow>,
    ) -> bool {
        match event {
            TerminalEvent::SftpUploadFileProgress {
                transfer_id,
                file_index,
                file,
                sent,
                total,
                cancel,
            } => {
                Self::handle_sftp_upload_file_progress(
                    panel_id,
                    transfer_id,
                    file_index,
                    file,
                    sent,
                    total,
                    cancel,
                    cx,
                );
                true
            }
            TerminalEvent::SftpUploadFinished { files, total_bytes } => {
                Self::handle_sftp_upload_finished(tab_label, files.len(), *total_bytes, window, cx);
                true
            }
            TerminalEvent::SftpUploadFileFinished {
                transfer_id,
                file_index,
                file,
                bytes,
            } => {
                Self::handle_sftp_upload_file_finished(
                    panel_id,
                    transfer_id,
                    file_index,
                    file,
                    *bytes,
                    cx,
                );
                true
            }
            TerminalEvent::SftpUploadCancelled => {
                Self::handle_sftp_upload_cancelled(panel_id, tab_label, window, cx);
                true
            }
            TerminalEvent::SftpUploadFileCancelled {
                transfer_id,
                file_index,
                file,
                sent,
                total,
            } => {
                Self::handle_sftp_upload_file_cancelled(
                    panel_id,
                    transfer_id,
                    file_index,
                    file,
                    sent,
                    total,
                    cx,
                );
                true
            }
            _ => false,
        }
    }

    fn handle_sftp_upload_file_progress(
        panel_id: usize,
        transfer_id: &u64,
        file_index: &usize,
        file: &str,
        sent: &u64,
        total: &u64,
        cancel: &Arc<std::sync::atomic::AtomicBool>,
        cx: &mut Context<TermuaWindow>,
    ) {
        let (_id, task) = Self::build_sftp_upload_task(
            panel_id,
            *transfer_id,
            *file_index,
            file,
            TransferStatus::InProgress,
            *sent,
            *total,
            Some(cancel),
        );
        Self::upsert_transfer(task, cx);
    }

    fn handle_sftp_upload_finished(
        tab_label: &SharedString,
        file_count: usize,
        total_bytes: u64,
        window: &mut Window,
        cx: &mut Context<TermuaWindow>,
    ) {
        notification::notify(
            notification::MessageKind::Success,
            Self::sftp_upload_finished_message(tab_label, file_count, total_bytes),
            window,
            cx,
        );
    }

    fn handle_sftp_upload_file_finished(
        panel_id: usize,
        transfer_id: &u64,
        file_index: &usize,
        file: &str,
        bytes: u64,
        cx: &mut Context<TermuaWindow>,
    ) {
        let (id, task) = Self::build_sftp_upload_task(
            panel_id,
            *transfer_id,
            *file_index,
            file,
            TransferStatus::Finished,
            bytes,
            bytes,
            None,
        );
        Self::upsert_transfer(task, cx);
        Self::schedule_transfer_auto_dismiss(id, cx);
    }

    fn handle_sftp_upload_cancelled(
        panel_id: usize,
        tab_label: &SharedString,
        window: &mut Window,
        cx: &mut Context<TermuaWindow>,
    ) {
        notification::notify(
            notification::MessageKind::Warning,
            Self::sftp_upload_cancelled_message(tab_label),
            window,
            cx,
        );

        if cx.try_global::<TransferCenterState>().is_some() {
            cx.global_mut::<TransferCenterState>()
                .remove_groups_with_prefix(Self::sftp_upload_panel_prefix(panel_id).as_str());
        }
    }

    fn sftp_upload_count_label(file_count: usize) -> String {
        match file_count {
            1 => "1 file".to_string(),
            n => format!("{n} files"),
        }
    }

    fn sftp_upload_finished_message(
        tab_label: &SharedString,
        file_count: usize,
        total_bytes: u64,
    ) -> String {
        format!(
            "[{}] Upload via SFTP complete: {}, {}",
            tab_label.as_ref(),
            Self::sftp_upload_count_label(file_count),
            format_bytes(total_bytes)
        )
    }

    fn sftp_upload_cancelled_message(tab_label: &SharedString) -> String {
        format!("[{}] Upload via SFTP cancelled", tab_label.as_ref())
    }

    fn sharing_started_message(room_id: &str, join_key: &str) -> String {
        format!(
            "Sharing started\nShare Key: {}",
            compose_share_key(room_id, join_key)
        )
    }

    fn handle_sftp_upload_file_cancelled(
        panel_id: usize,
        transfer_id: &u64,
        file_index: &usize,
        file: &str,
        sent: &u64,
        total: &u64,
        cx: &mut Context<TermuaWindow>,
    ) {
        let (id, task) = Self::build_sftp_upload_task(
            panel_id,
            *transfer_id,
            *file_index,
            file,
            TransferStatus::Cancelled,
            *sent,
            *total,
            None,
        );
        Self::upsert_transfer(task, cx);
        Self::schedule_transfer_auto_dismiss(id, cx);
    }

    fn schedule_transfer_auto_dismiss(id: String, cx: &mut Context<TermuaWindow>) {
        cx.spawn(async move |this, cx| {
            Timer::after(AUTO_DISMISS_AFTER).await;
            let _ = this.update(cx, |_this, cx| {
                Self::remove_transfer(id.as_str(), cx);
                cx.notify();
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, atomic::AtomicBool};

    use gpui_transfer::TransferStatus;

    use super::TermuaWindow;

    #[test]
    fn sftp_transfer_keys_are_stable() {
        assert_eq!(TermuaWindow::sftp_upload_panel_prefix(7), "sftp-upload-7-");
        assert_eq!(TermuaWindow::sftp_upload_group_id(7, 9), "sftp-upload-7-9");
        assert_eq!(
            TermuaWindow::sftp_upload_task_id(7, 9, 2),
            "sftp-upload-7-9-2"
        );
    }

    #[test]
    fn sftp_upload_finished_message_uses_consistent_copy() {
        let label = gpui::SharedString::from("ssh 1");
        assert_eq!(
            TermuaWindow::sftp_upload_finished_message(&label, 1, 1024),
            "[ssh 1] Upload via SFTP complete: 1 file, 1.0 KiB"
        );
        assert_eq!(
            TermuaWindow::sftp_upload_finished_message(&label, 2, 2048),
            "[ssh 1] Upload via SFTP complete: 2 files, 2.0 KiB"
        );
    }

    #[test]
    fn sftp_upload_cancelled_message_uses_consistent_copy() {
        let label = gpui::SharedString::from("ssh 1");
        assert_eq!(
            TermuaWindow::sftp_upload_cancelled_message(&label),
            "[ssh 1] Upload via SFTP cancelled"
        );
    }

    #[test]
    fn sharing_started_message_uses_share_key_copy() {
        assert_eq!(
            TermuaWindow::sharing_started_message("AbC234xYz", "k3Y9a2"),
            "Sharing started\nShare Key: AbC234xYz-k3Y9a2"
        );
    }

    #[test]
    fn sftp_upload_task_builder_normalizes_progress_and_keys() {
        let cancel = Arc::new(AtomicBool::new(false));
        let (id, task) = TermuaWindow::build_sftp_upload_task(
            7,
            9,
            2,
            "foo.txt",
            TransferStatus::InProgress,
            12,
            0,
            Some(&cancel),
        );

        assert_eq!(id, "sftp-upload-7-9-2");
        assert_eq!(task.group_id.as_deref(), Some("sftp-upload-7-9"));
        assert_eq!(task.group_total, Some(3));
        assert_eq!(task.status, TransferStatus::InProgress);
        assert_eq!(task.bytes_done, Some(12));
        assert_eq!(task.bytes_total, None);
        assert_eq!(
            task.progress,
            gpui_transfer::TransferProgress::Determinate(1.0)
        );
        assert!(task.cancel.is_some());
    }
}

impl TermuaWindow {
    pub(super) fn unlock_from_overlay(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.lock_overlay.unlock_with_password(window, cx);
    }

    pub(super) fn process_pending_commands(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if cx.global::<TermuaAppState>().pending_commands.is_empty() {
            return;
        }

        let commands = {
            let state = cx.global_mut::<TermuaAppState>();
            std::mem::take(&mut state.pending_commands)
        };

        for cmd in commands {
            match cmd {
                PendingCommand::OpenLocalTerminal { backend_type, env } => {
                    self.add_local_terminal_with_params(backend_type, env, window, cx);
                    self.reload_sessions_sidebar(window, cx);
                }
                PendingCommand::OpenSshTerminal {
                    backend_type,
                    env,
                    opts,
                } => {
                    self.add_ssh_terminal_with_params(backend_type, env, opts, None, window, cx);
                    self.reload_sessions_sidebar(window, cx);
                }
                PendingCommand::OpenSerialTerminal {
                    backend_type,
                    name,
                    port,
                    baud,
                    data_bits,
                    parity,
                    stop_bits,
                    flow_control,
                    term,
                    charset,
                    session_id,
                } => {
                    self.add_serial_terminal_with_params(
                        backend_type,
                        name,
                        port,
                        baud,
                        data_bits,
                        parity,
                        stop_bits,
                        flow_control,
                        term,
                        charset,
                        session_id,
                        window,
                        cx,
                    );
                    self.reload_sessions_sidebar(window, cx);
                }
                PendingCommand::ReloadSessionsSidebar => {
                    self.reload_sessions_sidebar(window, cx);
                }
                PendingCommand::OpenCastPicker => {
                    self.open_cast_player_picker(window, cx);
                }
                PendingCommand::OpenJoinSharingDialog => {
                    self.open_join_sharing_dialog(window, cx);
                }
                PendingCommand::JoinRelaySharing {
                    relay_url,
                    room_id,
                    join_key,
                } => {
                    self.add_relay_viewer_terminal(relay_url, room_id, join_key, window, cx);
                }
            }
        }
    }

    pub(super) fn on_new_local_terminal(
        &mut self,
        _: &NewLocalTerminal,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if cx.global::<lock_screen::LockState>().locked() {
            return;
        }
        log::info!("NewLocalTerminal (window): adding new panel now");
        self.add_local_terminal(window, cx);
    }

    pub(super) fn on_play_cast(
        &mut self,
        _: &PlayCast,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if cx.global::<lock_screen::LockState>().locked() {
            return;
        }
        self.open_cast_player_picker(window, cx);
    }

    pub(super) fn on_open_sftp(
        &mut self,
        _: &OpenSftp,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if cx.global::<lock_screen::LockState>().locked() {
            return;
        }
        let Some(focused) = self
            .focused_terminal_view
            .as_ref()
            .and_then(|v| v.upgrade())
        else {
            notification::notify_deferred(
                notification::MessageKind::Error,
                "No active terminal to open SFTP for.",
                window,
                cx,
            );
            return;
        };

        if focused.read(cx).terminal.read(cx).sftp().is_none() {
            notification::notify_deferred(
                notification::MessageKind::Error,
                "SFTP is only available for SSH terminals.",
                window,
                cx,
            );
            return;
        };

        self.open_sftp_for_terminal_view(focused, window, cx);
    }

    pub(super) fn on_start_sharing(
        &mut self,
        _: &StartSharing,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if cx.global::<lock_screen::LockState>().locked() {
            return;
        }
        if !crate::sharing::sharing_feature_enabled(cx) {
            notification::notify_deferred(
                notification::MessageKind::Warning,
                "Sharing is disabled in Settings.",
                window,
                cx,
            );
            return;
        }
        let Some(focused) = self
            .focused_terminal_view
            .as_ref()
            .and_then(|v| v.upgrade())
        else {
            notification::notify_deferred(
                notification::MessageKind::Error,
                "No active terminal to share.",
                window,
                cx,
            );
            return;
        };

        let terminal_view_id = focused.entity_id();
        if cx
            .global::<RelaySharingState>()
            .hosts
            .contains_key(&terminal_view_id)
        {
            return;
        }

        let relay_url = crate::sharing::effective_relay_url(cx);
        let room_id = gen_room_id();
        let join_key = gen_join_key();
        cx.spawn_in(window, async move |this, window| {
            let conn = connect_relay(
                &relay_url,
                RelayClientToRelay::Register {
                    room_id: room_id.clone(),
                    join_key: join_key.clone(),
                    ttl_secs: Some(30 * 60),
                },
            )
            .await;

            let _ = this.update_in(window, move |this, window, cx| match conn {
                Ok(conn) => {
                    let dirty = Arc::new(AtomicBool::new(false));
                    let selection_dirty = Arc::new(AtomicBool::new(false));

                    cx.global_mut::<RelaySharingState>().hosts.insert(
                        terminal_view_id,
                        HostShare {
                            room_id: room_id.clone(),
                            controller_id: None,
                            pending_request: false,
                            conn: conn.clone(),
                            seq: 0,
                            dirty: dirty.clone(),
                            selection_dirty: selection_dirty.clone(),
                        },
                    );

                    let terminal = focused.read(cx).terminal.clone();
                    this.subscribe_host_terminal_for_sharing_frames(
                        terminal,
                        dirty.clone(),
                        selection_dirty.clone(),
                        window,
                        cx,
                    );

                    this.send_host_snapshot(&focused, cx);
                    this.spawn_relay_pump_for_host(terminal_view_id, focused.clone(), window, cx);
                    this.spawn_relay_publisher_for_host(
                        terminal_view_id,
                        focused.clone(),
                        conn,
                        dirty,
                        selection_dirty,
                        window,
                        cx,
                    );

                    notification::notify_deferred(
                        notification::MessageKind::Info,
                        Self::sharing_started_message(&room_id, &join_key),
                        window,
                        cx,
                    );
                }
                Err(err) => {
                    notification::notify_deferred(
                        notification::MessageKind::Error,
                        format!("Start sharing failed: {err:#}"),
                        window,
                        cx,
                    );
                }
            });
        })
        .detach();
    }

    pub(super) fn on_stop_sharing(
        &mut self,
        _: &StopSharing,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if cx.global::<lock_screen::LockState>().locked() {
            return;
        }
        let Some(focused) = self
            .focused_terminal_view
            .as_ref()
            .and_then(|v| v.upgrade())
        else {
            return;
        };
        let terminal_view_id = focused.entity_id();

        let host = cx
            .global_mut::<RelaySharingState>()
            .hosts
            .remove(&terminal_view_id);
        if let Some(host) = host {
            host.conn.send(RelayClientToRelay::Stop {
                room_id: host.room_id.clone(),
            });
            host.conn.close();
            notification::notify_deferred(
                notification::MessageKind::Info,
                "Stopped sharing.",
                window,
                cx,
            );
        }
    }

    pub(super) fn on_request_control(
        &mut self,
        _: &RequestControl,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if cx.global::<lock_screen::LockState>().locked() {
            return;
        }
        let Some(focused) = self
            .focused_terminal_view
            .as_ref()
            .and_then(|v| v.upgrade())
        else {
            return;
        };
        let terminal_view_id = focused.entity_id();
        let Some(viewer) = cx
            .global::<RelaySharingState>()
            .viewers
            .get(&terminal_view_id)
            .cloned()
        else {
            return;
        };
        let Some(viewer_id) = viewer.viewer_id.lock().ok().and_then(|v| v.clone()) else {
            notification::notify_deferred(
                notification::MessageKind::Warning,
                "Not joined yet.",
                window,
                cx,
            );
            return;
        };
        viewer.conn.send(RelayClientToRelay::Request {
            room_id: viewer.room_id.clone(),
            viewer_id,
            viewer_label: None,
        });
    }

    pub(super) fn on_release_control(
        &mut self,
        _: &ReleaseControl,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(focused) = self
            .focused_terminal_view
            .as_ref()
            .and_then(|v| v.upgrade())
        else {
            return;
        };
        let terminal_view_id = focused.entity_id();
        let Some(viewer) = cx
            .global::<RelaySharingState>()
            .viewers
            .get(&terminal_view_id)
            .cloned()
        else {
            return;
        };
        let Some(viewer_id) = viewer.viewer_id.lock().ok().and_then(|v| v.clone()) else {
            return;
        };
        viewer.conn.send(RelayClientToRelay::Release {
            room_id: viewer.room_id.clone(),
            viewer_id,
        });
    }

    pub(super) fn on_revoke_control(
        &mut self,
        _: &RevokeControl,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(focused) = self
            .focused_terminal_view
            .as_ref()
            .and_then(|v| v.upgrade())
        else {
            return;
        };
        let terminal_view_id = focused.entity_id();
        let Some(host) = cx
            .global_mut::<RelaySharingState>()
            .hosts
            .get_mut(&terminal_view_id)
        else {
            return;
        };
        // Ensure local host state does not keep denying new requests as "busy".
        crate::sharing::clear_host_control_state(host);
        host.conn.send(RelayClientToRelay::Revoked {
            room_id: host.room_id.clone(),
        });
    }

    fn send_host_snapshot(
        &mut self,
        terminal_view: &gpui::Entity<TerminalView>,
        cx: &mut Context<Self>,
    ) {
        let terminal_view_id = terminal_view.entity_id();
        let terminal = terminal_view.read(cx).terminal.clone();
        let term_read = terminal.read(cx);
        let viewport_line_numbers = Self::host_viewport_line_numbers(&term_read);
        let payload = gpui_term::remote::RemoteTerminalContent::from_local(
            term_read.last_content(),
            term_read.total_lines(),
            term_read.viewport_lines(),
            viewport_line_numbers,
        );

        let Ok(payload_json) = serde_json::to_value(payload) else {
            return;
        };

        let (room_id, seq, conn) = {
            let Some(host) = cx
                .global_mut::<RelaySharingState>()
                .hosts
                .get_mut(&terminal_view_id)
            else {
                return;
            };
            host.seq = host.seq.wrapping_add(1);
            (host.room_id.clone(), host.seq, host.conn.clone())
        };

        conn.send(RelayClientToRelay::Snapshot {
            room_id,
            seq,
            payload: payload_json,
        });
    }

    fn send_host_frame(
        &mut self,
        terminal_view: &gpui::Entity<TerminalView>,
        cx: &mut Context<Self>,
    ) {
        let terminal_view_id = terminal_view.entity_id();
        let terminal = terminal_view.read(cx).terminal.clone();
        let term_read = terminal.read(cx);
        let viewport_line_numbers = Self::host_viewport_line_numbers(&term_read);
        let payload = gpui_term::remote::RemoteTerminalContent::from_local(
            term_read.last_content(),
            term_read.total_lines(),
            term_read.viewport_lines(),
            viewport_line_numbers,
        );

        let Ok(payload_json) = serde_json::to_value(payload) else {
            return;
        };

        let (room_id, seq, conn) = {
            let Some(host) = cx
                .global_mut::<RelaySharingState>()
                .hosts
                .get_mut(&terminal_view_id)
            else {
                return;
            };
            host.seq = host.seq.wrapping_add(1);
            (host.room_id.clone(), host.seq, host.conn.clone())
        };

        conn.send(RelayClientToRelay::Frame {
            room_id,
            seq,
            payload: payload_json,
        });
    }

    fn host_viewport_line_numbers(terminal: &gpui_term::Terminal) -> Vec<Option<usize>> {
        let total_lines = terminal.total_lines();
        let viewport_lines = terminal.viewport_lines().max(1);
        let display_offset = terminal.last_content().display_offset;
        let viewport_top = total_lines
            .saturating_sub(viewport_lines)
            .saturating_sub(display_offset);
        let rows = terminal.last_content().terminal_bounds.num_lines().max(1);
        terminal.logical_line_numbers_from_top(viewport_top, rows)
    }

    fn send_host_selection_update(
        &mut self,
        terminal_view: &gpui::Entity<TerminalView>,
        cx: &mut Context<Self>,
    ) {
        let terminal_view_id = terminal_view.entity_id();
        let terminal = terminal_view.read(cx).terminal.clone();
        let term_read = terminal.read(cx);
        let payload =
            gpui_term::remote::RemoteSelectionUpdate::from_local(term_read.last_content());

        let Ok(payload_json) = serde_json::to_value(payload) else {
            return;
        };

        let (room_id, seq, conn) = {
            let Some(host) = cx
                .global_mut::<RelaySharingState>()
                .hosts
                .get_mut(&terminal_view_id)
            else {
                return;
            };
            host.seq = host.seq.wrapping_add(1);
            (host.room_id.clone(), host.seq, host.conn.clone())
        };

        conn.send(RelayClientToRelay::Selection {
            room_id,
            seq,
            payload: payload_json,
        });
    }

    fn spawn_relay_pump_for_host(
        &mut self,
        terminal_view_id: gpui::EntityId,
        terminal_view: gpui::Entity<TerminalView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(host) = cx
            .global::<RelaySharingState>()
            .hosts
            .get(&terminal_view_id)
            .cloned()
        else {
            return;
        };

        let host_conn = host.conn;
        cx.spawn_in(window, async move |this, window| {
            while let Some(msg) = host_conn.recv().await {
                let _ = this.update_in(window, |this, window, cx| {
                    this.handle_host_relay_message(
                        terminal_view_id,
                        &terminal_view,
                        msg,
                        window,
                        cx,
                    );
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn spawn_relay_pump_for_viewer(
        &mut self,
        terminal_view_id: gpui::EntityId,
        terminal: gpui::Entity<gpui_term::Terminal>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(viewer) = cx
            .global::<RelaySharingState>()
            .viewers
            .get(&terminal_view_id)
            .cloned()
        else {
            return;
        };

        let viewer_conn = viewer.conn;
        let controlled = viewer.controlled;
        let viewer_id = viewer.viewer_id;

        cx.spawn_in(window, async move |this, window| {
            while let Some(msg) = viewer_conn.recv().await {
                let should_close = matches!(msg, RelayRelayToClient::Error { .. });
                let _ = this.update_in(window, |this, window, cx| {
                    this.handle_viewer_relay_message(
                        terminal_view_id,
                        &terminal,
                        &viewer_id,
                        &controlled,
                        msg,
                        window,
                        cx,
                    );
                    cx.notify();
                });
                if should_close {
                    viewer_conn.close();
                    break;
                }
            }
        })
        .detach();
    }

    fn spawn_relay_publisher_for_host(
        &mut self,
        terminal_view_id: gpui::EntityId,
        terminal_view: gpui::Entity<TerminalView>,
        conn: crate::sharing::RelayConn,
        dirty: Arc<AtomicBool>,
        selection_dirty: Arc<AtomicBool>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.spawn_in(window, async move |this, window| {
            let mut tick: u32 = 0;
            let mut last_frame_at = Instant::now()
                .checked_sub(Duration::from_secs(3600))
                .unwrap_or_else(Instant::now);
            let mut last_selection_at = last_frame_at;
            let mut last_display_offset: Option<usize> = None;
            loop {
                Timer::after(Duration::from_millis(20)).await;
                tick = tick.wrapping_add(1);

                let keep_running = this
                    .update_in(window, |this, _window, cx| {
                        if !cx
                            .global::<RelaySharingState>()
                            .hosts
                            .contains_key(&terminal_view_id)
                        {
                            return false;
                        }

                        if tick.is_multiple_of(50) {
                            conn.send(RelayClientToRelay::Ping);
                        }

                        // Host-initiated scroll does not emit `TerminalEvent::Wakeup`, so detect it
                        // by observing `display_offset` and mark frames dirty.
                        let display_offset = terminal_view
                            .read(cx)
                            .terminal
                            .read(cx)
                            .last_content()
                            .display_offset;
                        match last_display_offset {
                            Some(prev) if prev != display_offset => {
                                last_display_offset = Some(display_offset);
                                dirty.store(true, Ordering::Relaxed);
                            }
                            None => {
                                last_display_offset = Some(display_offset);
                            }
                            _ => {}
                        }

                        // Only send frames after terminal changes, with a conservative rate cap.
                        if dirty.load(Ordering::Relaxed)
                            && last_frame_at.elapsed() >= Duration::from_millis(50)
                        {
                            dirty.store(false, Ordering::Relaxed);
                            last_frame_at = Instant::now();
                            this.send_host_frame(&terminal_view, cx);
                        }

                        // Selection updates can be frequent; send smaller messages with a separate
                        // cap.
                        if selection_dirty.load(Ordering::Relaxed)
                            && last_selection_at.elapsed() >= Duration::from_millis(33)
                        {
                            selection_dirty.store(false, Ordering::Relaxed);
                            last_selection_at = Instant::now();
                            this.send_host_selection_update(&terminal_view, cx);
                        }
                        true
                    })
                    .unwrap_or(false);

                if !keep_running {
                    break;
                }
            }
        })
        .detach();
    }

    fn subscribe_host_terminal_for_sharing_frames(
        &mut self,
        terminal: gpui::Entity<gpui_term::Terminal>,
        dirty: Arc<AtomicBool>,
        selection_dirty: Arc<AtomicBool>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let sub = cx.subscribe_in(
            &terminal,
            window,
            move |_this, _terminal, event, _window, _cx| match event {
                TerminalEvent::Wakeup => {
                    dirty.store(true, Ordering::Relaxed);
                }
                TerminalEvent::SelectionsChanged => {
                    selection_dirty.store(true, Ordering::Relaxed);
                }
                _ => {}
            },
        );
        self._subscriptions.push(sub);
    }

    fn handle_host_relay_message(
        &mut self,
        terminal_view_id: gpui::EntityId,
        terminal_view: &gpui::Entity<TerminalView>,
        msg: RelayRelayToClient,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(host) = cx
            .global_mut::<RelaySharingState>()
            .hosts
            .get_mut(&terminal_view_id)
        else {
            return;
        };
        match msg {
            RelayRelayToClient::CtrlRequest {
                room_id: _,
                viewer_id,
                viewer_label,
            } => {
                if host.controller_id.is_some() || host.pending_request {
                    host.conn.send(RelayClientToRelay::Denied {
                        room_id: host.room_id.clone(),
                        viewer_id,
                        reason: "busy".to_string(),
                    });
                    return;
                }
                host.pending_request = true;
                self.open_control_confirm_dialog(
                    terminal_view_id,
                    viewer_id,
                    viewer_label,
                    window,
                    cx,
                );
            }
            RelayRelayToClient::CtrlRelease {
                room_id: _,
                viewer_id,
            } => {
                if host.controller_id.as_deref() == Some(&viewer_id) {
                    host.conn.send(RelayClientToRelay::Released {
                        room_id: host.room_id.clone(),
                        viewer_id,
                    });
                    host.controller_id = None;
                }
            }
            RelayRelayToClient::CtrlReleased {
                room_id: _,
                viewer_id,
            } => {
                // The relay may clear control immediately on viewer release to avoid "busy" races.
                // Treat CtrlReleased as authoritative and idempotent.
                if host.controller_id.as_deref() == Some(&viewer_id) {
                    host.controller_id = None;
                }
                host.pending_request = false;
            }
            RelayRelayToClient::InputEvent {
                room_id: _,
                viewer_id,
                payload,
            } => {
                let is_controller = host.controller_id.as_deref() == Some(&viewer_id);
                let dirty = host.dirty.clone();
                let selection_dirty = host.selection_dirty.clone();
                if !is_controller {
                    return;
                }
                let Ok(ev) = serde_json::from_value::<RemoteInputEvent>(payload) else {
                    return;
                };
                let is_selection = matches!(ev, RemoteInputEvent::SetSelectionRange { .. });
                self.apply_remote_input_to_host_terminal(terminal_view_id, terminal_view, ev, cx);
                if is_selection {
                    selection_dirty.store(true, Ordering::Relaxed);
                } else {
                    dirty.store(true, Ordering::Relaxed);
                }
            }
            RelayRelayToClient::Error { code: _, message } => {
                notification::notify_deferred(
                    notification::MessageKind::Error,
                    message,
                    window,
                    cx,
                );
            }
            RelayRelayToClient::Ok
            | RelayRelayToClient::Pong
            | RelayRelayToClient::Joined { .. }
            | RelayRelayToClient::Snapshot { .. }
            | RelayRelayToClient::Frame { .. }
            | RelayRelayToClient::Selection { .. }
            | RelayRelayToClient::CtrlDenied { .. }
            | RelayRelayToClient::CtrlGranted { .. }
            | RelayRelayToClient::CtrlRevoked { .. } => {}
        }
    }

    fn handle_viewer_relay_message(
        &mut self,
        terminal_view_id: gpui::EntityId,
        terminal: &gpui::Entity<gpui_term::Terminal>,
        viewer_id: &Arc<Mutex<Option<String>>>,
        controlled: &Arc<AtomicBool>,
        msg: RelayRelayToClient,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match msg {
            RelayRelayToClient::Joined {
                room_id: _,
                viewer_id: id,
            } => {
                if let Ok(mut guard) = viewer_id.lock() {
                    *guard = Some(id);
                }
                notification::notify_deferred(
                    notification::MessageKind::Info,
                    "Joined sharing.",
                    window,
                    cx,
                );
            }
            RelayRelayToClient::Snapshot {
                room_id: _,
                seq,
                payload,
            } => {
                let Ok(content) = serde_json::from_value::<RemoteTerminalContent>(payload) else {
                    return;
                };
                crate::sharing::apply_remote_snapshot(
                    terminal,
                    RemoteSnapshot { seq, content },
                    cx,
                );
            }
            RelayRelayToClient::Frame {
                room_id: _,
                seq,
                payload,
            } => {
                let Ok(content) = serde_json::from_value::<RemoteTerminalContent>(payload) else {
                    return;
                };
                crate::sharing::apply_remote_frame(terminal, RemoteFrame { seq, content }, cx);
            }
            RelayRelayToClient::Selection {
                room_id: _,
                seq: _,
                payload,
            } => {
                let Ok(update) =
                    serde_json::from_value::<gpui_term::remote::RemoteSelectionUpdate>(payload)
                else {
                    return;
                };
                crate::sharing::apply_remote_selection_update(terminal, update, cx);
            }
            RelayRelayToClient::CtrlGranted {
                room_id: _,
                viewer_id: granted,
            } => {
                let mine = viewer_id.lock().ok().and_then(|v| v.clone());
                if mine.as_deref() != Some(&granted) {
                    return;
                }
                controlled.store(true, Ordering::Relaxed);
                terminal.update(cx, |term, cx| {
                    term.dispatch_backend_event(
                        Box::new(RemoteBackendEvent::SetControlled(true)),
                        cx,
                    );
                });
                notification::notify_deferred(
                    notification::MessageKind::Info,
                    "Control granted.",
                    window,
                    cx,
                );
            }
            RelayRelayToClient::CtrlDenied { room_id: _, reason } => {
                controlled.store(false, Ordering::Relaxed);
                terminal.update(cx, |term, cx| {
                    term.dispatch_backend_event(
                        Box::new(RemoteBackendEvent::SetControlled(false)),
                        cx,
                    );
                });
                notification::notify_deferred(
                    notification::MessageKind::Warning,
                    format!("Control denied: {reason}"),
                    window,
                    cx,
                );
            }
            RelayRelayToClient::CtrlReleased {
                room_id: _,
                viewer_id: released,
            } => {
                let mine = viewer_id.lock().ok().and_then(|v| v.clone());
                if mine.as_deref() != Some(&released) {
                    return;
                }
                controlled.store(false, Ordering::Relaxed);
                terminal.update(cx, |term, cx| {
                    term.dispatch_backend_event(
                        Box::new(RemoteBackendEvent::SetControlled(false)),
                        cx,
                    );
                });
                notification::notify_deferred(
                    notification::MessageKind::Info,
                    "Control released.",
                    window,
                    cx,
                );
            }
            RelayRelayToClient::CtrlRevoked { room_id: _ } => {
                controlled.store(false, Ordering::Relaxed);
                terminal.update(cx, |term, cx| {
                    term.dispatch_backend_event(
                        Box::new(RemoteBackendEvent::SetControlled(false)),
                        cx,
                    );
                });
                notification::notify_deferred(
                    notification::MessageKind::Warning,
                    "Control revoked.",
                    window,
                    cx,
                );
            }
            RelayRelayToClient::Error { code: _, message } => {
                cx.global_mut::<RelaySharingState>()
                    .viewers
                    .remove(&terminal_view_id);
                controlled.store(false, Ordering::Relaxed);
                terminal.update(cx, |term, cx| {
                    term.dispatch_backend_event(
                        Box::new(RemoteBackendEvent::SetControlled(false)),
                        cx,
                    );
                });
                notification::notify_deferred(
                    notification::MessageKind::Error,
                    message,
                    window,
                    cx,
                );
            }
            RelayRelayToClient::Ok
            | RelayRelayToClient::Pong
            | RelayRelayToClient::CtrlRequest { .. }
            | RelayRelayToClient::CtrlRelease { .. }
            | RelayRelayToClient::InputEvent { .. } => {}
        }
    }

    fn apply_remote_input_to_host_terminal(
        &mut self,
        terminal_view_id: gpui::EntityId,
        terminal_view: &gpui::Entity<TerminalView>,
        ev: gpui_term::remote::RemoteInputEvent,
        cx: &mut Context<Self>,
    ) {
        if terminal_view.entity_id() != terminal_view_id {
            return;
        }

        let terminal = terminal_view.read(cx).terminal.clone();
        match ev {
            gpui_term::remote::RemoteInputEvent::Keystroke { keystroke } => {
                if let Ok(k) = gpui::Keystroke::parse(&keystroke) {
                    let alt_is_meta = TerminalSettings::global(cx).option_as_meta;
                    terminal.update(cx, |t, _cx| {
                        t.try_keystroke(&k, alt_is_meta);
                    });
                }
            }
            gpui_term::remote::RemoteInputEvent::Paste { text } => {
                terminal.update(cx, |t, _| t.paste(&text));
            }
            gpui_term::remote::RemoteInputEvent::Text { text } => {
                terminal.update(cx, |t, _| t.input(text.into_bytes()));
            }
            gpui_term::remote::RemoteInputEvent::ScrollLines { delta } => {
                terminal.update(cx, |t, _| {
                    if delta > 0 {
                        t.scroll_up_by(delta as usize);
                    } else if delta < 0 {
                        t.scroll_down_by((-delta) as usize);
                    }
                });
            }
            gpui_term::remote::RemoteInputEvent::ScrollToTop => {
                terminal.update(cx, |t, _| t.scroll_to_top());
            }
            gpui_term::remote::RemoteInputEvent::ScrollToBottom => {
                terminal.update(cx, |t, _| t.scroll_to_bottom());
            }
            gpui_term::remote::RemoteInputEvent::SetSelectionRange { range } => {
                terminal.update(cx, |t, _| {
                    t.set_selection_range(range.map(gpui_term::SelectionRange::from));
                });
            }
        }
    }

    fn open_control_confirm_dialog(
        &mut self,
        terminal_view_id: gpui::EntityId,
        viewer_id: String,
        viewer_label: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(Some(root)) = window.root::<gpui_component::Root>() else {
            return;
        };

        let label = viewer_label.unwrap_or_else(|| viewer_id.clone());
        root.update(cx, |root, cx| {
            root.open_dialog(
                move |dialog, _window, _app| {
                    let detail = format!("Viewer requested control:\n{label}");
                    dialog
                        .title("Request Control".to_string())
                        .child(gpui_component::text::TextView::markdown(
                            "termua-sharing-ctrl-request",
                            detail,
                        ))
                        .button_props(
                            gpui_component::dialog::DialogButtonProps::default()
                                .ok_text("Grant".to_string())
                                .cancel_text("Deny".to_string()),
                        )
                        .on_ok({
                            let viewer_id = viewer_id.clone();
                            move |_, _window, app| {
                                if let Some(host) = app
                                    .global_mut::<RelaySharingState>()
                                    .hosts
                                    .get_mut(&terminal_view_id)
                                {
                                    host.pending_request = false;
                                    host.controller_id = Some(viewer_id.clone());
                                    host.conn.send(RelayClientToRelay::Granted {
                                        room_id: host.room_id.clone(),
                                        viewer_id: viewer_id.clone(),
                                    });
                                }
                                true
                            }
                        })
                        .on_cancel({
                            let viewer_id = viewer_id.clone();
                            move |_, _window, app| {
                                if let Some(host) = app
                                    .global_mut::<RelaySharingState>()
                                    .hosts
                                    .get_mut(&terminal_view_id)
                                {
                                    host.pending_request = false;
                                    host.conn.send(RelayClientToRelay::Denied {
                                        room_id: host.room_id.clone(),
                                        viewer_id: viewer_id.clone(),
                                        reason: "denied".to_string(),
                                    });
                                }
                                true
                            }
                        })
                        .confirm()
                },
                window,
                cx,
            );
        });
    }

    fn open_join_sharing_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use gpui_component::input::{Input, InputState};

        let Some(Some(root)) = window.root::<gpui_component::Root>() else {
            return;
        };

        if !crate::sharing::sharing_feature_enabled(cx) {
            notification::notify_deferred(
                notification::MessageKind::Warning,
                "Sharing is disabled in Settings.",
                window,
                cx,
            );
            return;
        }

        // Important: keep input state stable across renders. Creating `InputState` inside the
        // dialog builder closure can cause it to be re-created on each re-render, making typing
        // appear to "do nothing".
        let relay_input = cx.new(|cx| InputState::new(window, cx));
        let share_key_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("AbC123xYz-k3Y9a1"));
        let relay_url = crate::sharing::effective_relay_url(cx);
        relay_input.update(cx, |state, cx| state.set_value(&relay_url, window, cx));

        root.update(cx, |root, cx| {
            let relay_input = relay_input.clone();
            let share_key_input = share_key_input.clone();

            root.open_dialog(
                move |dialog, _window, _app| {
                    dialog
                        .title("Join Sharing".to_string())
                        .w(px(540.0))
                        .child(
                            v_flex()
                                .gap_2()
                                .child("Relay URL".to_string())
                                .child(Input::new(&relay_input))
                                .child("Share Key".to_string())
                                .child(Input::new(&share_key_input)),
                        )
                        .button_props(
                            gpui_component::dialog::DialogButtonProps::default()
                                .ok_text("Join".to_string())
                                .cancel_text("Cancel".to_string()),
                        )
                        .on_ok({
                            let relay_input = relay_input.clone();
                            let share_key_input = share_key_input.clone();

                            move |_, window, app| {
                                let relay_url = relay_input.read(app).value().trim().to_string();
                                let share_key =
                                    share_key_input.read(app).value().trim().to_string();
                                if relay_url.is_empty() || share_key.is_empty() {
                                    notification::notify_app(
                                        notification::MessageKind::Warning,
                                        "Relay URL / Share Key cannot be empty.",
                                        window,
                                        app,
                                    );
                                    return false;
                                }
                                if !relay_url.starts_with("ws://")
                                    && !relay_url.starts_with("wss://")
                                {
                                    notification::notify_app(
                                        notification::MessageKind::Warning,
                                        "Relay URL must start with ws:// or wss://",
                                        window,
                                        app,
                                    );
                                    return false;
                                }
                                let (room_id, join_key) = match parse_share_key(&share_key) {
                                    Ok(parsed) => parsed,
                                    Err(err) => {
                                        notification::notify_app(
                                            notification::MessageKind::Warning,
                                            format!("Invalid Share Key: {err}"),
                                            window,
                                            app,
                                        );
                                        return false;
                                    }
                                };
                                if room_id.is_empty() || join_key.is_empty() {
                                    notification::notify_app(
                                        notification::MessageKind::Warning,
                                        "Invalid Share Key.",
                                        window,
                                        app,
                                    );
                                    return false;
                                }
                                app.global_mut::<TermuaAppState>().pending_command(
                                    PendingCommand::JoinRelaySharing {
                                        relay_url,
                                        room_id,
                                        join_key,
                                    },
                                );
                                app.refresh_windows();
                                let _ = window;
                                true
                            }
                        })
                        .confirm()
                },
                window,
                cx,
            );
        });

        let focus = share_key_input.read(cx).focus_handle(cx);
        window.defer(cx, move |window, cx| window.focus(&focus, cx));
    }

    fn add_local_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.add_local_terminal_with_params(TerminalType::WezTerm, HashMap::new(), window, cx);
    }

    fn open_cast_player_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use gpui::PathPromptOptions;

        let picker = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Select cast file to play".into()),
        });

        cx.spawn_in(window, async move |view, window| {
            let Ok(Ok(Some(mut paths))) = picker.await else {
                return;
            };
            let Some(path) = paths.pop() else {
                return;
            };

            let _ = view.update_in(window, |this, window, cx| {
                this.open_cast_player_tab(path, window, cx);
            });
        })
        .detach();
    }

    fn open_cast_player_tab(
        &mut self,
        cast_path: std::path::PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let playback_speed = cx
            .try_global::<crate::settings::RecordingSettings>()
            .map(crate::settings::RecordingSettings::playback_speed_or_default)
            .unwrap_or(1.0);
        let env = cast_player_child_env(&cast_path, playback_speed);

        let panel =
            self.build_terminal_panel(PanelKind::Recorder, TerminalType::WezTerm, env, window, cx);

        self.dock_area.update(cx, |dock, cx| {
            dock.add_panel(
                Arc::new(panel.clone()) as Arc<dyn PanelView>,
                DockPlacement::Center,
                None,
                window,
                cx,
            );
        });
    }

    fn add_local_terminal_with_params(
        &mut self,
        backend_type: TerminalType,
        env: HashMap<String, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let panel = self.build_terminal_panel(PanelKind::Local, backend_type, env, window, cx);
        self.dock_area.update(cx, |dock, cx| {
            dock.add_panel(
                Arc::new(panel) as Arc<dyn PanelView>,
                DockPlacement::Center,
                None,
                window,
                cx,
            );
        });
        // `DockArea` will re-render itself, but we also mutate our own state (`next_terminal_id`),
        // so we must notify.
        cx.notify();
    }

    fn add_relay_viewer_terminal(
        &mut self,
        relay_url: String,
        room_id: String,
        join_key: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.spawn_in(window, async move |this, window| {
            let conn = connect_relay(
                &relay_url,
                RelayClientToRelay::Join {
                    room_id: room_id.clone(),
                    join_key: join_key.clone(),
                },
            )
            .await;

            let _ = this.update_in(window, move |this, window, cx| match conn {
                Ok(conn) => {
                    let id = this.next_terminal_id;
                    this.next_terminal_id += 1;

                    let tab_label: SharedString = format!("share {id}").into();
                    let tab_tooltip: SharedString =
                        format!("Share Key {}", compose_share_key(&room_id, &join_key)).into();

                    let viewer_id = Arc::new(Mutex::new(None::<String>));
                    let controlled = Arc::new(AtomicBool::new(false));

                    let room_id_for_input = room_id.clone();
                    let conn_for_input = conn.clone();
                    let viewer_id_for_input = Arc::clone(&viewer_id);
                    let send_input: Arc<dyn Send + Sync + Fn(RemoteInputEvent)> =
                        Arc::new(move |ev| {
                            let Some(viewer_id) =
                                viewer_id_for_input.lock().ok().and_then(|v| v.clone())
                            else {
                                return;
                            };
                            let Ok(payload) = serde_json::to_value(ev) else {
                                return;
                            };
                            conn_for_input.send(RelayClientToRelay::InputEvent {
                                room_id: room_id_for_input.clone(),
                                viewer_id,
                                payload,
                            });
                        });

                    let terminal = crate::sharing::make_remote_terminal(
                        send_input,
                        Arc::clone(&controlled),
                        cx,
                    );
                    let panel = this.build_wired_terminal_panel(
                        id,
                        PanelKind::Local,
                        tab_label,
                        Some(tab_tooltip),
                        terminal,
                        window,
                        cx,
                    );
                    let terminal_view = panel.read(cx).terminal_view();

                    let terminal_view_id = terminal_view.entity_id();
                    cx.global_mut::<RelaySharingState>().viewers.insert(
                        terminal_view_id,
                        ViewerShare {
                            room_id,
                            viewer_id: Arc::clone(&viewer_id),
                            controlled: Arc::clone(&controlled),
                            conn,
                        },
                    );

                    let relay_terminal = terminal_view.read(cx).terminal.clone();
                    this.spawn_relay_pump_for_viewer(terminal_view_id, relay_terminal, window, cx);
                    this.dock_area.update(cx, |dock, cx| {
                        dock.add_panel(
                            Arc::new(panel) as Arc<dyn PanelView>,
                            DockPlacement::Center,
                            None,
                            window,
                            cx,
                        );
                    });
                    cx.notify();
                }
                Err(err) => {
                    notification::notify_deferred(
                        notification::MessageKind::Error,
                        format!("Join sharing failed: {err:#}"),
                        window,
                        cx,
                    );
                }
            });
        })
        .detach();
    }

    #[allow(clippy::too_many_arguments)]
    fn add_serial_terminal_with_params(
        &mut self,
        backend_type: TerminalType,
        name: String,
        port: String,
        baud: u32,
        data_bits: u8,
        parity: crate::store::SerialParity,
        stop_bits: crate::store::SerialStopBits,
        flow_control: crate::store::SerialFlowControl,
        _term: String,
        _charset: String,
        session_id: Option<i64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let opts = SerialOptions {
            port: port.clone(),
            baud,
            data_bits,
            parity: match parity {
                crate::store::SerialParity::None => SerialParity::None,
                crate::store::SerialParity::Even => SerialParity::Even,
                crate::store::SerialParity::Odd => SerialParity::Odd,
            },
            stop_bits: match stop_bits {
                crate::store::SerialStopBits::One => SerialStopBits::One,
                crate::store::SerialStopBits::Two => SerialStopBits::Two,
            },
            flow_control: match flow_control {
                crate::store::SerialFlowControl::None => SerialFlowControl::None,
                crate::store::SerialFlowControl::Software => SerialFlowControl::Software,
                crate::store::SerialFlowControl::Hardware => SerialFlowControl::Hardware,
            },
        };

        log::debug!(
            "termua: opening serial session (backend={backend_type:?}) port={} baud={}",
            opts.port,
            opts.baud
        );

        let builder = TerminalBuilder::new_with_pty(
            backend_type,
            PtySource::Serial { opts: opts.clone() },
            CursorShape::default(),
            None,
            None,
        );

        let builder = match builder {
            Ok(builder) => builder,
            Err(err) => {
                if let Some(_session_id) = session_id {
                    let reason = err.root_cause().to_string();
                    let hint = crate::serial::open_failure_hint(&port, &err);
                    let message: SharedString = match hint {
                        Some(hint) => format!(
                            "Failed to open serial port `{port}`.\n\nError:\n{reason}\n\n{hint}"
                        )
                        .into(),
                        None => format!("Failed to open serial port `{port}`.\n\nError:\n{reason}")
                            .into(),
                    };

                    // Clicking a saved Serial session should only show a toast; editing the
                    // session (e.g. changing port) is done via right-click → Edit.
                    let message: SharedString = format!(
                        "{message}\n\nTip: Right-click the session and choose Edit to change the \
                         port."
                    )
                    .into();
                    window.defer(cx, move |window, app| {
                        crate::notification::notify_app(
                            crate::notification::MessageKind::Error,
                            message,
                            window,
                            app,
                        );
                    });
                    return;
                }

                let reason = err.root_cause().to_string();
                let hint = crate::serial::open_failure_hint(&port, &err);
                let message = match hint {
                    Some(hint) => format!(
                        "Failed to open serial port `{port}`.\n\nError:\n{reason}\n\n{hint}"
                    ),
                    None => format!("Failed to open serial port `{port}`.\n\nError:\n{reason}"),
                };

                notification::notify_deferred(
                    notification::MessageKind::Error,
                    message,
                    window,
                    cx,
                );
                return;
            }
        };

        let panel = self.build_serial_terminal_panel_from_builder(builder, name, opts, window, cx);
        self.dock_area.update(cx, |dock, cx| {
            dock.add_panel(
                Arc::new(panel) as Arc<dyn PanelView>,
                DockPlacement::Center,
                None,
                window,
                cx,
            );
        });
        cx.notify();
    }

    fn open_ssh_host_verification_dialog(
        &mut self,
        opts: SshOptions,
        message: String,
        decision_tx: smol::channel::Sender<bool>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(Some(root)) = window.root::<gpui_component::Root>() else {
            log::warn!("termua: dialog requested but window root is not gpui_component::Root");
            let _ = decision_tx.try_send(false);
            return;
        };

        let target = ssh_target_label(&opts);

        root.update(cx, |root, cx| {
            root.open_dialog(
                move |dialog, _window, app| {
                    let decision_tx_ok = decision_tx.clone();
                    let decision_tx_cancel = decision_tx.clone();

                    dialog
                        .title(Self::ssh_host_verification_dialog_title(app))
                        .w(px(720.))
                        .child(Self::ssh_host_verification_dialog_body(&target, &message))
                        .button_props(
                            gpui_component::dialog::DialogButtonProps::default()
                                .ok_text(t!("SshHostVerify.Button.TrustContinue").to_string())
                                .cancel_text(t!("SshHostVerify.Button.Reject").to_string()),
                        )
                        .on_ok(move |_, _window, _app| {
                            let _ = decision_tx_ok.try_send(true);
                            true
                        })
                        .on_cancel(move |_, _window, _app| {
                            let _ = decision_tx_cancel.try_send(false);
                            true
                        })
                        .confirm()
                },
                window,
                cx,
            );
        });
    }

    fn ssh_host_verification_dialog_title(app: &App) -> gpui::AnyElement {
        use gpui_component::ActiveTheme as _;

        h_flex()
            .gap_2()
            .items_center()
            .child(
                Icon::default()
                    .path(TermuaIcon::AlertCircle)
                    .text_color(app.theme().warning),
            )
            .child(t!("SshHostVerify.Title").to_string())
            .into_any_element()
    }

    fn ssh_host_verification_dialog_body(target: &str, message: &str) -> gpui::AnyElement {
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .items_start()
                    .child(div().child(t!("SshHostVerify.Label.Target").to_string()))
                    .child(
                        div().min_w_0().child(
                            gpui_component::text::TextView::markdown(
                                "termua-ssh-host-verify-text-target",
                                target.to_string(),
                            )
                            .selectable(true),
                        ),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_start()
                    .child(div().child(t!("SshHostVerify.Label.Message").to_string()))
                    .child(
                        div().min_w_0().child(
                            gpui_component::text::TextView::markdown(
                                "termua-ssh-host-verify-text-message",
                                message.to_string(),
                            )
                            .selectable(true),
                        ),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_start()
                    .child(div().child(t!("SshHostVerify.Label.Note").to_string()))
                    .child(
                        div().min_w_0().child(
                            gpui_component::text::TextView::markdown(
                                "termua-ssh-host-verify-text-note",
                                t!("SshHostVerify.NoteText").to_string(),
                            )
                            .selectable(true),
                        ),
                    ),
            )
            .into_any_element()
    }

    fn ssh_host_key_mismatch_dialog_title(app: &App) -> gpui::AnyElement {
        use gpui_component::ActiveTheme as _;

        h_flex()
            .gap_2()
            .items_center()
            .child(
                Icon::default()
                    .path(TermuaIcon::AlertCircle)
                    .text_color(app.theme().danger),
            )
            .child(t!("SshHostKeyMismatch.Title").to_string())
            .into_any_element()
    }

    fn ssh_host_key_mismatch_markdown_row(
        label_selector: &'static str,
        label: String,
        value_selector: &'static str,
        markdown_id: &'static str,
        markdown: String,
    ) -> gpui::AnyElement {
        h_flex()
            .gap_2()
            .items_start()
            .child(
                div()
                    .debug_selector(|| label_selector.to_string())
                    .child(label),
            )
            .child(
                div()
                    .min_w_0()
                    .debug_selector(|| value_selector.to_string())
                    .child(
                        gpui_component::text::TextView::markdown(markdown_id, markdown)
                            .selectable(true),
                    ),
            )
            .into_any_element()
    }

    fn ssh_host_key_mismatch_dialog_body(
        target: String,
        host: String,
        port: u16,
        reason: String,
        got_fingerprint: Option<String>,
        known_hosts_label: String,
        fix_cmd: Option<String>,
    ) -> gpui::AnyElement {
        let mut column = v_flex()
            .gap_2()
            .child(Self::ssh_host_key_mismatch_markdown_row(
                "termua-ssh-hostkey-mismatch-label-target",
                t!("SshHostKeyMismatch.Label.Target").to_string(),
                "termua-ssh-hostkey-mismatch-value-target",
                "termua-ssh-hostkey-mismatch-text-target",
                target,
            ))
            .child(Self::ssh_host_key_mismatch_markdown_row(
                "termua-ssh-hostkey-mismatch-label-server",
                t!("SshHostKeyMismatch.Label.Server").to_string(),
                "termua-ssh-hostkey-mismatch-value-server",
                "termua-ssh-hostkey-mismatch-text-server",
                format!("{host}:{port}"),
            ))
            .child(Self::ssh_host_key_mismatch_markdown_row(
                "termua-ssh-hostkey-mismatch-label-reason",
                t!("SshHostKeyMismatch.Label.Reason").to_string(),
                "termua-ssh-hostkey-mismatch-value-reason",
                "termua-ssh-hostkey-mismatch-text-reason",
                reason,
            ));

        if let Some(fp) = got_fingerprint {
            column = column.child(Self::ssh_host_key_mismatch_markdown_row(
                "termua-ssh-hostkey-mismatch-label-fingerprint",
                t!("SshHostKeyMismatch.Label.GotFingerprint").to_string(),
                "termua-ssh-hostkey-mismatch-value-fingerprint",
                "termua-ssh-hostkey-mismatch-text-fingerprint",
                fp,
            ));
        }

        column = column.child(Self::ssh_host_key_mismatch_markdown_row(
            "termua-ssh-hostkey-mismatch-label-known-hosts",
            t!("SshHostKeyMismatch.Label.KnownHosts").to_string(),
            "termua-ssh-hostkey-mismatch-value-known-hosts",
            "termua-ssh-hostkey-mismatch-text-known-hosts",
            known_hosts_label,
        ));

        if let Some(cmd) = fix_cmd {
            column = column.child(Self::ssh_host_key_mismatch_markdown_row(
                "termua-ssh-hostkey-mismatch-label-manual-fix",
                t!("SshHostKeyMismatch.Label.ManualFix").to_string(),
                "termua-ssh-hostkey-mismatch-value-manual-fix",
                "termua-ssh-hostkey-mismatch-text-manual-fix",
                cmd,
            ));
        }

        column
            .child(Self::ssh_host_key_mismatch_markdown_row(
                "termua-ssh-hostkey-mismatch-label-note",
                t!("SshHostKeyMismatch.Label.Note").to_string(),
                "termua-ssh-hostkey-mismatch-value-note",
                "termua-ssh-hostkey-mismatch-text-note",
                t!("SshHostKeyMismatch.NoteText").to_string(),
            ))
            .into_any_element()
    }

    fn close_dialog(window: &mut Window, app: &mut App) {
        gpui_component::Root::update(window, app, |root, window, cx| {
            root.close_dialog(window, cx);
        });
    }

    fn queue_open_ssh_terminal(
        backend_type: TerminalType,
        env: HashMap<String, String>,
        opts: SshOptions,
        app: &mut App,
    ) {
        app.global_mut::<TermuaAppState>()
            .pending_commands
            .push(PendingCommand::OpenSshTerminal {
                backend_type,
                env,
                opts,
            });
        app.refresh_windows();
    }

    fn ssh_host_key_mismatch_dialog_footer_elements<C>(
        backend_type: TerminalType,
        env: HashMap<String, String>,
        opts: SshOptions,
        known_hosts_path: Option<std::path::PathBuf>,
        host: String,
        port: u16,
        cancel: C,
        window: &mut Window,
        app: &mut App,
    ) -> Vec<gpui::AnyElement>
    where
        C: FnOnce(&mut Window, &mut App) -> gpui::AnyElement,
    {
        let retry_env = env.clone();
        let retry_opts = opts.clone();
        let retry_button = Button::new("termua-ssh-hostkey-mismatch-retry")
            .label(t!("SshHostKeyMismatch.Button.Retry").to_string())
            .on_click(move |_, window, app| {
                Self::close_dialog(window, app);
                Self::queue_open_ssh_terminal(
                    backend_type,
                    retry_env.clone(),
                    retry_opts.clone(),
                    app,
                );
            });

        let remove_env = env;
        let remove_opts = opts;
        let remove_and_retry_button = Button::new("termua-ssh-hostkey-mismatch-remove-retry")
            .label(t!("SshHostKeyMismatch.Button.RemoveRetry").to_string())
            .primary()
            .on_click(move |_, window, app| {
                let Some(path) = known_hosts_path.as_ref() else {
                    notification::notify_app(
                        notification::MessageKind::Error,
                        t!("SshHostKeyMismatch.Error.MissingKnownHostsPath").to_string(),
                        window,
                        app,
                    );
                    return;
                };

                let summary = match remove_known_host_entry(path, &host, port) {
                    Ok(summary) => summary,
                    Err(err) => {
                        notification::notify_app(
                            notification::MessageKind::Error,
                            t!(
                                "SshHostKeyMismatch.Error.FailedUpdateKnownHosts",
                                err = format!("{err:#}")
                            )
                            .to_string(),
                            window,
                            app,
                        );
                        return;
                    }
                };

                Self::close_dialog(window, app);

                notification::notify_app(
                    notification::MessageKind::Info,
                    if summary.is_empty() {
                        format!("Updated {}", path.display())
                    } else {
                        summary
                    },
                    window,
                    app,
                );

                Self::queue_open_ssh_terminal(
                    backend_type,
                    remove_env.clone(),
                    remove_opts.clone(),
                    app,
                );
            });

        vec![
            cancel(window, app),
            retry_button.into_any_element(),
            remove_and_retry_button.into_any_element(),
        ]
    }

    pub(crate) fn open_ssh_host_key_mismatch_dialog(
        &mut self,
        backend_type: TerminalType,
        env: HashMap<String, String>,
        opts: SshOptions,
        reason: String,
        details: SshHostKeyMismatchDetails,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(Some(root)) = window.root::<gpui_component::Root>() else {
            log::warn!("termua: dialog requested but window root is not gpui_component::Root");
            return;
        };

        let target = ssh_target_label(&opts);
        let default_host = opts.host.trim().to_string();
        let default_port = opts.port.unwrap_or(22);
        let host = details
            .server_host
            .clone()
            .unwrap_or_else(|| default_host.clone());
        let port = details.server_port.unwrap_or(default_port);

        let known_hosts_path = details
            .known_hosts_path
            .clone()
            .or_else(default_known_hosts_path);

        let known_hosts_label = known_hosts_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "~/.ssh/known_hosts".to_string());

        let fix_cmd = known_hosts_path.as_ref().map(|p| {
            if port == 22 {
                format!("ssh-keygen -R \"{host}\" -f \"{}\"", p.display())
            } else {
                format!("ssh-keygen -R \"[{host}]:{port}\" -f \"{}\"", p.display())
            }
        });

        root.update(cx, |root, cx| {
            root.open_dialog(
                move |dialog, _window, app| {
                    let known_hosts_path_for_footer = known_hosts_path.clone();
                    let host_for_footer = host.clone();
                    let port_for_footer = port;
                    let env_for_footer = env.clone();
                    let opts_for_footer = opts.clone();

                    dialog
                        .title(Self::ssh_host_key_mismatch_dialog_title(app))
                        .w(px(720.))
                        .child(Self::ssh_host_key_mismatch_dialog_body(
                            target.clone(),
                            host.clone(),
                            port,
                            reason.clone(),
                            details.got_fingerprint.clone(),
                            known_hosts_label.clone(),
                            fix_cmd.clone(),
                        ))
                        .footer(move |_ok, cancel, window, app| {
                            Self::ssh_host_key_mismatch_dialog_footer_elements(
                                backend_type,
                                env_for_footer.clone(),
                                opts_for_footer.clone(),
                                known_hosts_path_for_footer.clone(),
                                host_for_footer.clone(),
                                port_for_footer,
                                cancel,
                                window,
                                app,
                            )
                        })
                },
                window,
                cx,
            );
        });
    }

    pub(crate) fn add_ssh_terminal_with_params(
        &mut self,
        backend_type: TerminalType,
        env: HashMap<String, String>,
        opts: SshOptions,
        session_id: Option<i64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Building the SSH PTY involves a blocking login handshake. Run that work in a background
        // thread and only attach the terminal panel on success.
        let builder_fn = self.ssh_terminal_builder.clone();
        let env_for_thread = env.clone();
        let opts_for_error = opts.clone();
        let opts_for_panel = opts.clone();
        let opts_for_prompt = opts.clone();
        let background = cx.background_executor().clone();

        let (verify_tx, verify_rx) =
            smol::channel::unbounded::<gpui_term::SshHostVerificationPrompt>();
        // Keep a sender alive on the UI thread so closing the prompt channel can't happen on the
        // background thread.
        let verify_tx_keepalive = verify_tx.clone();

        // Handle host verification prompts while the background handshake is running.
        cx.spawn_in(window, async move |view, window| {
            while let Ok(req) = verify_rx.recv().await {
                let (decision_tx, decision_rx) = smol::channel::bounded::<bool>(1);
                let message = req.message;
                let _ = view.update_in(window, |this, window, cx| {
                    this.open_ssh_host_verification_dialog(
                        opts_for_prompt.clone(),
                        message,
                        decision_tx.clone(),
                        window,
                        cx,
                    );
                });

                let decision = decision_rx.recv().await.unwrap_or(false);
                let _ = req.reply.send(decision).await;
            }
        })
        .detach();

        cx.spawn_in(window, async move |view, window| {
            let session_id = session_id;
            let verify_tx_for_task = verify_tx.clone();
            let task = background.spawn(async move {
                // Route SSH host verification prompts (unknown host keys) back to the UI thread.
                // If no UI consumes them, gpui_term will time out and treat the host as untrusted.
                let _guard = gpui_term::set_thread_ssh_host_verification_prompt_sender(Some(
                    verify_tx_for_task,
                ));
                (builder_fn)(backend_type, env_for_thread, opts)
            });

            let result = task.await;

            let _ = view.update_in(window, |this, window, cx| {
                this.finish_add_ssh_terminal_task(
                    result,
                    backend_type,
                    env,
                    opts_for_error,
                    opts_for_panel,
                    session_id,
                    window,
                    cx,
                );
            });

            // Keep the sender alive on the UI thread until the handshake completes.
            drop(verify_tx_keepalive);
        })
        .detach();
    }

    fn finish_add_ssh_terminal_task(
        &mut self,
        result: anyhow::Result<TerminalBuilder>,
        backend_type: TerminalType,
        env: HashMap<String, String>,
        opts_for_error: SshOptions,
        opts_for_panel: SshOptions,
        session_id: Option<i64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(builder) => {
                let panel =
                    self.build_ssh_terminal_panel_from_builder(builder, opts_for_panel, window, cx);
                self.dock_area.update(cx, |dock, cx| {
                    dock.add_panel(
                        Arc::new(panel) as Arc<dyn PanelView>,
                        DockPlacement::Center,
                        None,
                        window,
                        cx,
                    );
                });
                self.clear_connecting_session(session_id, cx);
                cx.notify();
            }
            Err(err) => {
                let root_reason = err.root_cause().to_string();
                if let Some(details) = parse_ssh_host_key_mismatch(&root_reason) {
                    self.open_ssh_host_key_mismatch_dialog(
                        backend_type,
                        env,
                        opts_for_error,
                        root_reason,
                        details,
                        window,
                        cx,
                    );
                    self.clear_connecting_session(session_id, cx);
                    return;
                }
                let _env = env;

                let id = self.next_terminal_id;
                self.next_terminal_id += 1;

                let tab_label =
                    dedupe_tab_label(&mut self.ssh_tab_label_counts, opts_for_error.name.as_str());
                let tab_tooltip = ssh_tab_tooltip(&opts_for_error);
                let message = ssh_connect_failure_message(&opts_for_error, &err);

                let panel = cx.new(|cx| {
                    SshErrorPanel::new(id, tab_label, Some(tab_tooltip), message.into(), cx)
                });

                self.dock_area.update(cx, |dock, cx| {
                    dock.add_panel(
                        Arc::new(panel) as Arc<dyn PanelView>,
                        DockPlacement::Center,
                        None,
                        window,
                        cx,
                    );
                });
                self.clear_connecting_session(session_id, cx);
                cx.notify();
            }
        }
    }

    fn clear_connecting_session(&mut self, session_id: Option<i64>, cx: &mut Context<Self>) {
        let Some(session_id) = session_id else {
            return;
        };
        self.sessions_sidebar.update(cx, |sidebar, cx| {
            sidebar.set_connecting(session_id, false, cx);
        });
    }

    pub(super) fn open_session_by_id(
        &mut self,
        id: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Ok(Some(session)) = crate::store::load_session(id) else {
            return;
        };

        let backend_type = match session.backend {
            crate::settings::TerminalBackend::Alacritty => TerminalType::Alacritty,
            crate::settings::TerminalBackend::Wezterm => TerminalType::WezTerm,
        };

        let protocol = session.protocol.clone();
        match protocol {
            crate::store::SessionType::Local => {
                self.open_saved_local_session(backend_type, session, window, cx);
            }
            crate::store::SessionType::Ssh => {
                self.open_saved_ssh_session(backend_type, session, id, window, cx);
            }
            crate::store::SessionType::Serial => {
                self.open_saved_serial_session(backend_type, session, id, window, cx);
            }
        }
    }

    fn open_saved_local_session(
        &mut self,
        backend_type: TerminalType,
        session: crate::store::Session,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let shell_program = session.shell_program.unwrap_or_default();
        let env = build_local_terminal_env(
            shell_program.as_str(),
            session.term.as_str(),
            session.charset.as_str(),
        );
        self.add_local_terminal_with_params(backend_type, env, window, cx);
    }

    fn open_saved_ssh_session(
        &mut self,
        backend_type: TerminalType,
        session: crate::store::Session,
        session_id: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(host) = session.ssh_host.as_deref() else {
            return;
        };
        let port = session.ssh_port.unwrap_or(22);

        let env = build_local_terminal_env("", session.term.as_str(), session.charset.as_str());
        let proxy = ssh_proxy_from_session(&session);
        let name = session.label;
        let group = session.group_path;

        let auth = match session.ssh_auth_type {
            Some(crate::store::SshAuthType::Config) => Authentication::Config,
            Some(crate::store::SshAuthType::Password) => {
                let user = session.ssh_user.unwrap_or_else(|| "root".to_string());
                let password = session.ssh_password.unwrap_or_default();
                if password.trim().is_empty() {
                    notification::notify_deferred(
                        notification::MessageKind::Error,
                        "Missing saved SSH password for this session.",
                        window,
                        cx,
                    );
                    return;
                }
                Authentication::Password(user, password)
            }
            None => {
                // Back-compat: default to config auth if not recorded.
                Authentication::Config
            }
        };

        let opts = SshOptions {
            group,
            name,
            host: host.to_string(),
            port: Some(port),
            auth,
            proxy,
            backend: cx
                .try_global::<crate::settings::SshBackendPreference>()
                .map(|pref| pref.backend)
                .unwrap_or_default(),
            tcp_nodelay: session.ssh_tcp_nodelay,
            tcp_keepalive: session.ssh_tcp_keepalive,
        };
        self.add_ssh_terminal_with_params(backend_type, env, opts, Some(session_id), window, cx);
    }

    fn open_saved_serial_session(
        &mut self,
        backend_type: TerminalType,
        session: crate::store::Session,
        session_id: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(port) = session.serial_port.clone() else {
            notification::notify_deferred(
                notification::MessageKind::Error,
                "Missing saved serial port for this session.",
                window,
                cx,
            );
            return;
        };

        let baud = session.serial_baud.unwrap_or(9600);
        let data_bits = session.serial_data_bits.unwrap_or(8);
        let parity = session
            .serial_parity
            .unwrap_or(crate::store::SerialParity::None);
        let stop_bits = session
            .serial_stop_bits
            .unwrap_or(crate::store::SerialStopBits::One);
        let flow_control = session
            .serial_flow_control
            .unwrap_or(crate::store::SerialFlowControl::None);

        self.add_serial_terminal_with_params(
            backend_type,
            session.label,
            port,
            baud,
            data_bits,
            parity,
            stop_bits,
            flow_control,
            session.term,
            session.charset,
            Some(session_id),
            window,
            cx,
        );
    }

    fn reload_sessions_sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let sidebar = self.sessions_sidebar.clone();
        sidebar.update(cx, |sidebar, cx| {
            sidebar.reload(window, cx);
        });
    }

    fn register_terminal_target_and_focus(
        &mut self,
        id: usize,
        tab_label: SharedString,
        terminal_view: &gpui::Entity<TerminalView>,
        terminal_weak: gpui::WeakEntity<gpui_term::Terminal>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::assistant::register_terminal_target(cx, id, tab_label, terminal_weak.clone());

        let focused_terminal_view = terminal_view.downgrade();
        let focused_terminal = terminal_weak;
        let focus_handle = terminal_view.read(cx).focus_handle.clone();
        let sub = cx.on_focus_in(&focus_handle, window, move |this, _window, cx| {
            this.focused_terminal_view = Some(focused_terminal_view.clone());
            crate::assistant::set_focused_terminal(cx, Some(id), Some(focused_terminal.clone()));
        });
        self._subscriptions.push(sub);
    }

    fn subscribe_terminal_view_events(
        &mut self,
        terminal_view: &gpui::Entity<TerminalView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let source_terminal_view = terminal_view.clone();
        let source_terminal_view_for_cb = source_terminal_view.clone();
        let subscription = cx.subscribe_in(
            &source_terminal_view,
            window,
            move |this, _, event, _window, cx| match event {
                TerminalEvent::UserInput(input) => this.on_terminal_user_input(
                    source_terminal_view_for_cb.clone(),
                    input.clone(),
                    cx,
                ),
                TerminalEvent::Toast {
                    level,
                    title,
                    detail,
                } => {
                    let kind = match level {
                        gpui::PromptLevel::Info => crate::notification::MessageKind::Info,
                        gpui::PromptLevel::Warning => crate::notification::MessageKind::Warning,
                        gpui::PromptLevel::Critical => crate::notification::MessageKind::Error,
                    };
                    let message = match detail.as_deref() {
                        Some(detail) if !detail.trim().is_empty() => format!("{title}\n{detail}"),
                        _ => title.clone(),
                    };
                    crate::notification::record(kind, message, cx);
                }
                _ => {}
            },
        );
        self._subscriptions.push(subscription);
    }

    fn create_terminal_view(
        &self,
        kind: PanelKind,
        terminal: gpui::Entity<gpui_term::Terminal>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Entity<TerminalView> {
        if kind == PanelKind::Recorder {
            return cx.new(|cx| TerminalView::new_with_context_menu(terminal, window, cx, false));
        }

        let provider = self.terminal_context_menu_provider.clone();
        cx.new(|cx| {
            TerminalView::new_with_context_menu_provider(terminal, window, cx, true, Some(provider))
        })
    }

    fn build_wired_terminal_panel(
        &mut self,
        id: usize,
        kind: PanelKind,
        tab_label: SharedString,
        tab_tooltip: Option<SharedString>,
        terminal: gpui::Entity<gpui_term::Terminal>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Entity<TerminalPanel> {
        self.subscribe_terminal_events_for_messages(
            terminal.clone(),
            id,
            tab_label.clone(),
            window,
            cx,
        );

        let terminal_weak = terminal.downgrade();
        let terminal_view = self.create_terminal_view(kind, terminal, window, cx);

        self.register_terminal_target_and_focus(
            id,
            tab_label.clone(),
            &terminal_view,
            terminal_weak,
            window,
            cx,
        );
        self.subscribe_terminal_view_events(&terminal_view, window, cx);

        let focus: FocusHandle = terminal_view.read(cx).focus_handle.clone();
        window.focus(&focus, cx);

        cx.new(|_| TerminalPanel::new(id, kind, tab_label, tab_tooltip, terminal_view))
    }

    fn build_ssh_terminal_panel_from_builder(
        &mut self,
        builder: TerminalBuilder,
        opts: SshOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Entity<TerminalPanel> {
        let id = self.next_terminal_id;
        self.next_terminal_id += 1;

        let tab_label = dedupe_tab_label(&mut self.ssh_tab_label_counts, opts.name.as_str());
        let tab_tooltip = ssh_tab_tooltip(&opts);

        let terminal = cx.new(move |cx| builder.subscribe(cx));
        self.build_wired_terminal_panel(
            id,
            PanelKind::Ssh,
            tab_label,
            Some(tab_tooltip),
            terminal,
            window,
            cx,
        )
    }

    fn build_serial_terminal_panel_from_builder(
        &mut self,
        builder: TerminalBuilder,
        name: String,
        opts: SerialOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Entity<TerminalPanel> {
        let id = self.next_terminal_id;
        self.next_terminal_id += 1;

        let tab_label = terminal_panel_tab_name(PanelKind::Serial, id);
        let tab_tooltip: SharedString = format!("{name}\n{} @ {}", opts.port, opts.baud).into();

        let terminal = cx.new(move |cx| builder.subscribe(cx));
        self.build_wired_terminal_panel(
            id,
            PanelKind::Serial,
            tab_label,
            Some(tab_tooltip),
            terminal,
            window,
            cx,
        )
    }

    fn build_terminal_panel(
        &mut self,
        kind: PanelKind,
        backend_type: TerminalType,
        env: HashMap<String, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Entity<TerminalPanel> {
        let id = self.next_terminal_id;
        self.next_terminal_id += 1;

        let env = match kind {
            PanelKind::Local => crate::shell_integration::maybe_inject_local_shell_osc133(env, id),
            PanelKind::Ssh | PanelKind::Serial | PanelKind::Recorder => env,
        };

        let tab_label = match kind {
            PanelKind::Local => crate::panel::local_terminal_panel_tab_name(
                &env,
                id,
                &mut self.local_tab_label_counts,
            ),
            PanelKind::Ssh | PanelKind::Serial | PanelKind::Recorder => {
                terminal_panel_tab_name(kind, id)
            }
        };

        let terminal = cx.new(|cx| {
            TerminalBuilder::new(
                backend_type,
                env,
                CursorShape::default(),
                None,
                id as u64,
                None,
            )
            .expect("local terminal builder should succeed")
            .subscribe(cx)
        });
        self.build_wired_terminal_panel(id, kind, tab_label, None, terminal, window, cx)
    }

    fn open_sftp_for_terminal_view(
        &mut self,
        terminal_view: gpui::Entity<TerminalView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut tab_label: gpui::SharedString = "SFTP".into();
        let tab_panels = self.dock_area.read(cx).visible_tab_panels(cx);
        for tab_panel in tab_panels {
            let Some(active_panel) = tab_panel.read(cx).active_panel(cx) else {
                continue;
            };

            let Ok(terminal_panel) = active_panel.view().downcast::<TerminalPanel>() else {
                continue;
            };

            let terminal_panel = terminal_panel.read(cx);
            if terminal_panel.terminal_view().entity_id() == terminal_view.entity_id() {
                tab_label = terminal_panel.tab_label();
                break;
            }
        }

        let panel = match crate::panel::sftp_panel::SftpDockPanel::open_for_terminal_view(
            terminal_view,
            tab_label,
            window,
            cx,
        ) {
            Ok(panel) => panel,
            Err(err) => {
                notification::notify_deferred(
                    notification::MessageKind::Error,
                    err.to_string(),
                    window,
                    cx,
                );
                return;
            }
        };

        self.dock_area.update(cx, |dock, cx| {
            dock.add_panel(panel, DockPlacement::Bottom, None, window, cx);
            if !dock.is_dock_open(DockPlacement::Bottom, cx) {
                dock.toggle_dock(DockPlacement::Bottom, window, cx);
            }
        });
        cx.notify();
    }

    fn on_terminal_user_input(
        &mut self,
        source: gpui::Entity<TerminalView>,
        input: TerminalUserInput,
        cx: &mut Context<Self>,
    ) {
        cx.global::<lock_screen::LockState>().report_activity();

        if cx.global::<lock_screen::LockState>().locked() {
            return;
        }

        if !cx.global::<TermuaAppState>().multi_exec_enabled {
            return;
        }

        // Only broadcast to panes that are currently visible: the active tab in each visible
        // TabPanel (splits). This intentionally skips background tabs.
        let tab_panels = self.dock_area.read(cx).visible_tab_panels(cx);
        for tab_panel in tab_panels {
            let Some(active_panel) = tab_panel.read(cx).active_panel(cx) else {
                continue;
            };

            let Ok(terminal_panel) = active_panel.view().downcast::<TerminalPanel>() else {
                continue;
            };

            let target_terminal_view = terminal_panel.read(cx).terminal_view();
            if target_terminal_view.entity_id() == source.entity_id() {
                continue;
            }

            match &input {
                TerminalUserInput::Keystroke(keystroke) => {
                    let keystroke = keystroke.clone();
                    target_terminal_view.update(cx, |view, cx| {
                        view.terminal.update(cx, |term, cx| {
                            term.try_keystroke(
                                &keystroke,
                                TerminalSettings::global(cx).option_as_meta,
                            );
                        });
                    });
                }
                TerminalUserInput::Text(text) => {
                    let bytes = text.clone().into_bytes();
                    target_terminal_view.update(cx, |view, cx| {
                        view.terminal.update(cx, |term, _| {
                            term.input(bytes.clone());
                        });
                    });
                }
                TerminalUserInput::Paste(text) => {
                    let text = text.clone();
                    target_terminal_view.update(cx, |view, cx| {
                        view.terminal.update(cx, |term, _| {
                            term.paste(&text);
                        });
                    });
                }
            }
        }
    }
}
