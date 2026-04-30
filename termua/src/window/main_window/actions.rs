//! TermuaWindow behavior and event handling.

mod sftp;
mod sharing;
mod ssh;
mod terminal;

use gpui::{App, Context, InteractiveElement, ParentElement, Window, div};
use rust_i18n::t;

use super::TermuaWindow;
use crate::{
    NewLocalTerminal, OpenSftp, PendingCommand, PlayCast, TermuaAppState, lock_screen, notification,
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
                                    )
                                    .show_cancel(true),
                            )
                            .on_ok(|_, _window, app| {
                                app.quit();
                                true
                            })
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
                    params,
                } => {
                    self.add_ssh_terminal_with_params(backend_type, params, None, window, cx);
                    self.reload_sessions_sidebar(window, cx);
                }
                PendingCommand::OpenSerialTerminal {
                    backend_type,
                    params,
                    session_id,
                } => {
                    self.add_serial_terminal_with_params(
                        backend_type,
                        params,
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
}
