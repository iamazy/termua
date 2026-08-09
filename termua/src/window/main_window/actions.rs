//! TermuaWindow behavior and event handling.

mod sftp;
mod ssh;
mod terminal;

use gpui::{
    App, ClipboardItem, Context, InteractiveElement, IntoElement, ParentElement, Styled, Window,
    div, px,
};
use gpui_common::TermuaIcon;
use gpui_component::{
    ActiveTheme as _, Icon,
    button::{Button, ButtonVariants},
    dialog::{DialogAction, DialogClose, DialogFooter},
    h_flex, v_flex,
};
use rust_i18n::t;

use super::TermuaWindow;
use crate::{
    NewLocalTerminal, OpenSftp, PendingCommand, PlayCast, ShareTerminalWeb, TermuaAppState,
    lock_screen, notification,
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
    pub(crate) fn open_web_control_request_dialog(
        &mut self,
        peer: std::net::SocketAddr,
        decision_tx: smol::channel::Sender<bool>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(Some(root)) = window.root::<gpui_component::Root>() else {
            log::warn!("termua: web control dialog requested without a component root");
            let _ = decision_tx.try_send(false);
            return;
        };

        root.update(cx, |root, cx| {
            root.open_dialog(
                move |dialog, _window, app| {
                    let allow_tx = decision_tx.clone();
                    let deny_tx = decision_tx.clone();
                    let title = h_flex()
                        .gap_2()
                        .items_center()
                        .debug_selector(|| "termua-web-control-dialog-title".to_string())
                        .child(
                            Icon::default()
                                .path(TermuaIcon::LockOpen)
                                .text_color(app.theme().warning),
                        )
                        .child("Browser control request")
                        .into_any_element();
                    let source = v_flex()
                        .gap_1()
                        .text_sm()
                        .p_3()
                        .rounded_md()
                        .border_1()
                        .border_color(app.theme().border.opacity(0.8))
                        .bg(app.theme().border.opacity(0.12))
                        .debug_selector(|| "termua-web-control-dialog-source".to_string())
                        .child(div().text_sm().text_color(app.theme().muted_foreground).child(
                            "Request source",
                        ))
                        .child(div().child(peer.to_string()));
                    let notice = h_flex()
                        .gap_2()
                        .items_start()
                        .text_sm()
                        .p_3()
                        .rounded_md()
                        .border_1()
                        .border_color(app.theme().warning.opacity(0.3))
                        .bg(app.theme().warning.opacity(0.08))
                        .text_color(app.theme().warning)
                        .debug_selector(|| "termua-web-control-dialog-notice".to_string())
                        .child(Icon::default().path(TermuaIcon::AlertCircle))
                        .child(div().min_w_0().child(
                            "If allowed, this browser can send keyboard input to the current terminal until sharing ends.",
                        ));

                    dialog
                        .title(title)
                        .w(px(560.))
                        .child(
                            v_flex()
                                .gap_3()
                                .child("A browser on your local network is requesting control of this terminal.")
                                .child(source)
                                .child(notice),
                        )
                        .footer(
                            DialogFooter::new()
                                .child(DialogClose::new().child(
                                    Button::new("termua-web-control-dialog-deny")
                                        .label("Deny")
                                        .debug_selector(|| {
                                            "termua-web-control-dialog-deny".to_string()
                                        }),
                                ))
                                .child(DialogAction::new().child(
                                    Button::new("termua-web-control-dialog-allow")
                                        .primary()
                                        .label("Allow control")
                                        .debug_selector(|| {
                                            "termua-web-control-dialog-allow".to_string()
                                        }),
                                )),
                        )
                        .on_ok(move |_, _window, _app| {
                            let _ = allow_tx.try_send(true);
                            true
                        })
                        .on_cancel(move |_, _window, _app| {
                            let _ = deny_tx.try_send(false);
                            true
                        })
                },
                window,
                cx,
            );
        });
    }

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
                    self.sessions_sidebar.update(cx, |sidebar, cx| {
                        sidebar.clear_operation_error(window, cx);
                    });
                    self.reload_sessions_sidebar(window, cx);
                }
                PendingCommand::ShowSessionsSidebarError(message) => {
                    self.sessions_sidebar.update(cx, |sidebar, cx| {
                        sidebar.show_error(message, window, cx);
                    });
                }
                PendingCommand::OpenCastPicker => {
                    self.open_cast_player_picker(window, cx);
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

    pub(super) fn on_share_terminal_web(
        &mut self,
        _: &ShareTerminalWeb,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if cx.global::<lock_screen::LockState>().locked() {
            return;
        }
        if let Some(server) = self.web_share.take() {
            server.shutdown();
            self.web_share_active
                .store(false, std::sync::atomic::Ordering::Relaxed);
            self.web_share_subscription = None;
            notification::notify_deferred(
                notification::MessageKind::Info,
                "Web terminal sharing stopped.",
                window,
                cx,
            );
            return;
        }
        if self.web_share_starting {
            return;
        }
        let Some(terminal_view) = self
            .focused_terminal_view
            .as_ref()
            .and_then(|view| view.upgrade())
        else {
            notification::notify_deferred(
                notification::MessageKind::Error,
                "No active terminal to share.",
                window,
                cx,
            );
            return;
        };
        let terminal = terminal_view.read(cx).terminal.clone();
        let snapshot = gpui_term::capture_terminal_screen(terminal.read(cx).last_content());
        let token = crate::web::generate_token();
        let xterm_theme = crate::web::XtermTheme::from_app_theme(cx.theme());
        self.web_share_starting = true;

        cx.spawn_in(window, async move |this, window| {
            let result =
                crate::web::WebShareServer::bind_with_theme(token.clone(), snapshot, xterm_theme)
                    .await;
            let _ = this.update_in(window, move |this, window, cx| {
                this.web_share_starting = false;
                let server = match result {
                    Ok(server) => std::sync::Arc::new(server),
                    Err(error) => {
                        notification::notify_deferred(
                            notification::MessageKind::Error,
                            format!("Failed to start web terminal: {error}"),
                            window,
                            cx,
                        );
                        return;
                    }
                };
                if cx.global::<lock_screen::LockState>().locked() {
                    server.shutdown();
                    return;
                }

                let url = format!(
                    "http://{}:{}/#token={token}",
                    crate::web::local_network_ip(),
                    server.local_addr().port()
                );
                cx.write_to_clipboard(ClipboardItem::new_string(url.clone()));
                notification::notify_deferred(
                    notification::MessageKind::Warning,
                    format!("Web terminal is available on the trusted LAN. URL copied:\n{url}"),
                    window,
                    cx,
                );

                server.set_history_before(
                    terminal
                        .read(cx)
                        .total_lines()
                        .saturating_sub(terminal.read(cx).viewport_lines()),
                );

                let snapshot_server = std::sync::Arc::clone(&server);
                this.web_share_subscription =
                    Some(cx.subscribe(&terminal, move |_this, terminal, event, cx| {
                        if !matches!(event, gpui_term::Event::ContentUpdated) {
                            return;
                        }
                        let snapshot =
                            gpui_term::capture_terminal_screen(terminal.read(cx).last_content());
                        snapshot_server.update_snapshot(snapshot);
                        snapshot_server.set_history_before(
                            terminal
                                .read(cx)
                                .total_lines()
                                .saturating_sub(terminal.read(cx).viewport_lines()),
                        );
                    }));

                let input_rx = server.inputs();
                let input_terminal = terminal.downgrade();
                cx.spawn(async move |_this, cx| {
                    while let Ok(input) = input_rx.recv().await {
                        let Some(terminal) = input_terminal.upgrade() else {
                            break;
                        };
                        let _ = terminal.update(cx, |terminal, _cx| terminal.input(input.data));
                    }
                })
                .detach();

                let history_rx = server.history_requests();
                let history_terminal = terminal.downgrade();
                let history_server = std::sync::Arc::clone(&server);
                cx.spawn(async move |_this, cx| {
                    while let Ok(request) = history_rx.recv().await {
                        let Some(terminal) = history_terminal.upgrade() else {
                            break;
                        };
                        let (start, columns, rows, cells) = terminal.update(cx, |terminal, _cx| {
                            let available = terminal
                                .total_lines()
                                .saturating_sub(terminal.viewport_lines());
                            let before = request.before.min(available);
                            let start = before.saturating_sub(200);
                            let count = before.saturating_sub(start);
                            let (columns, rows, cells) =
                                terminal.preview_cells_from_top(start, count);
                            (start, columns, rows, cells)
                        });
                        let ansi = gpui_term::serialize_terminal_rows_ansi(columns, rows, &cells);
                        history_server.send_history(request.client_id, start, ansi);
                    }
                })
                .detach();

                let control_rx = server.control_requests();
                let approval_server = std::sync::Arc::clone(&server);
                cx.spawn_in(window, async move |this, window| {
                    while let Ok(request) = control_rx.recv().await {
                        let (decision_tx, decision_rx) = smol::channel::bounded(1);
                        let opened = this.update_in(window, move |this, window, cx| {
                            this.open_web_control_request_dialog(
                                request.peer,
                                decision_tx,
                                window,
                                cx,
                            );
                        });
                        if opened.is_err() {
                            break;
                        }
                        if decision_rx.recv().await == Ok(true) {
                            let _ = approval_server.approve_control(request.request_id);
                        } else {
                            approval_server.deny_control(request.request_id);
                        }
                    }
                })
                .detach();

                this.web_share = Some(server);
                this.web_share_active
                    .store(true, std::sync::atomic::Ordering::Relaxed);

                let expiring_server = this.web_share.as_ref().cloned().unwrap();
                let expiring_terminal = terminal.downgrade();
                cx.spawn(async move |this, cx| {
                    for _ in 0..30 * 60 {
                        smol::Timer::after(std::time::Duration::from_secs(1)).await;
                        if expiring_terminal.upgrade().is_none() {
                            break;
                        }
                    }
                    expiring_server.shutdown();
                    let _ =
                        this.update(cx, |this, cx| {
                            if this.web_share.as_ref().is_some_and(|active| {
                                std::sync::Arc::ptr_eq(active, &expiring_server)
                            }) {
                                this.web_share = None;
                                this.web_share_active
                                    .store(false, std::sync::atomic::Ordering::Relaxed);
                                this.web_share_subscription = None;
                                cx.notify();
                            }
                        });
                })
                .detach();
            });
        })
        .detach();
    }
}
