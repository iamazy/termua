use std::sync::Arc;

use gpui::{App, Context, SharedString, Window};
use gpui_common::format_bytes;
use gpui_dock::PanelView;
use gpui_term::Event as TerminalEvent;
use gpui_transfer::{
    AUTO_DISMISS_AFTER, TransferCenterState, TransferKind, TransferProgress, TransferStatus,
    TransferTask,
};
use smol::Timer;

use super::TermuaWindow;
use crate::{
    notification,
    panel::{PanelKind, TerminalPanel},
    sharing::compose_share_key,
};

impl TermuaWindow {
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
            move |this, _terminal, event, window, cx| {
                this.handle_terminal_event_for_messages(panel_id, &tab_label, event, window, cx);
            },
        );
        self._subscriptions.push(sub);
    }

    fn handle_terminal_event_for_messages(
        &mut self,
        panel_id: usize,
        tab_label: &SharedString,
        event: &TerminalEvent,
        window: &mut Window,
        cx: &mut Context<TermuaWindow>,
    ) {
        if Self::handle_terminal_event_toast(event, window, cx) {
            return;
        }
        if Self::handle_terminal_event_sftp_upload(panel_id, tab_label, event, window, cx) {
            return;
        }
        if matches!(event, TerminalEvent::CloseTerminal) {
            self.close_terminal_panel_on_event(panel_id, window, cx);
        }
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

    pub(super) fn find_visible_terminal_panel(
        &self,
        cx: &App,
        mut predicate: impl FnMut(&TerminalPanel, &App) -> bool,
    ) -> Option<Arc<dyn PanelView>> {
        self.dock_area
            .read(cx)
            .visible_tab_panels(cx)
            .into_iter()
            .filter_map(|tab_panel| tab_panel.read(cx).active_panel(cx))
            .find(|panel| {
                panel
                    .view()
                    .downcast::<TerminalPanel>()
                    .ok()
                    .is_some_and(|terminal_panel| predicate(&terminal_panel.read(cx), cx))
            })
    }

    pub(super) fn close_terminal_panel(
        &mut self,
        panel: Arc<dyn PanelView>,
        window: &mut Window,
        cx: &mut Context<TermuaWindow>,
    ) {
        self.dock_area.update(cx, |dock, cx| {
            dock.remove_panel_from_all_docks(panel, window, cx);
        });
        cx.notify();
    }

    fn close_terminal_panel_on_event(
        &mut self,
        panel_id: usize,
        window: &mut Window,
        cx: &mut Context<TermuaWindow>,
    ) {
        let Some(panel) = self.find_visible_terminal_panel(cx, |terminal_panel, cx| {
            if terminal_panel.id() != panel_id {
                return false;
            }

            match terminal_panel.kind() {
                PanelKind::Recorder => false,
                PanelKind::Ssh => !terminal_panel
                    .terminal_view()
                    .read(cx)
                    .terminal
                    .read(cx)
                    .has_exited(),
                PanelKind::Local | PanelKind::Serial => true,
            }
        }) else {
            return;
        };

        self.close_terminal_panel(panel, window, cx);
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

    pub(in crate::window::main_window::actions) fn sharing_started_message(
        room_id: &str,
        join_key: &str,
    ) -> String {
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
