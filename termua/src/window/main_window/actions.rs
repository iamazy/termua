//! TermuaWindow behavior and event handling.

mod sftp;
mod ssh;
mod terminal;

use std::sync::Arc;

use gpui::{
    App, ClipboardItem, Context, InteractiveElement, IntoElement, ParentElement, ReadGlobal,
    Styled, Window, div, px,
};
use gpui_common::TermuaIcon;
use gpui_component::{
    ActiveTheme as _, Icon,
    button::{Button, ButtonVariants},
    dialog::{DialogAction, DialogClose, DialogFooter},
    h_flex, v_flex,
};
use rust_i18n::t;

use super::{TermuaWindow, state::WebShareEntry};
use crate::{
    NewLocalTerminal, OpenSftp, PendingCommand, PlayCast, RevokeWebControl, ShareTerminalWeb,
    TermuaAppState, lock_screen, notification, panel::TerminalPanel,
};

pub(super) fn web_share_started_message(tab_label: &str, url: &str) -> String {
    format!(
        "Web terminal sharing started for tab \"{tab_label}\" on the trusted LAN. URL \
         copied:\n{url}"
    )
}

pub(super) fn web_share_stopped_message(tab_label: &str) -> String {
    format!("Web terminal sharing stopped for tab \"{tab_label}\".")
}

pub(super) fn web_share_start_failed_message(
    tab_label: &str,
    port: u16,
    error: &std::io::Error,
) -> String {
    if error.kind() == std::io::ErrorKind::AddrInUse {
        format!(
            "Failed to share terminal \"{tab_label}\": port {port} is already in use. Change it \
             in Terminal / Sharing."
        )
    } else {
        format!("Failed to share terminal \"{tab_label}\": {error}")
    }
}

pub(super) fn web_share_idle_timeout(minutes: u16) -> std::time::Duration {
    std::time::Duration::from_secs(u64::from(minutes) * 60)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WebShareToggleAction {
    Start,
    Stop,
    Wait,
}

pub(super) fn web_share_toggle_action(active: bool, starting: bool) -> WebShareToggleAction {
    if active {
        WebShareToggleAction::Stop
    } else if starting {
        WebShareToggleAction::Wait
    } else {
        WebShareToggleAction::Start
    }
}

pub(super) fn show_web_line_numbers(cx: &App) -> bool {
    gpui_term::TerminalSettings::global(cx).show_line_numbers
}

impl TermuaWindow {
    pub(super) fn stop_web_shares_without_tabs(&mut self, cx: &mut Context<Self>) {
        let open_terminals: std::collections::HashSet<_> = self
            .dock_area
            .read(cx)
            .all_tab_panels(cx)
            .into_iter()
            .flat_map(|tabs| tabs.read(cx).panels().to_vec())
            .filter_map(|panel| panel.view().downcast::<TerminalPanel>().ok())
            .map(|panel| panel.read(cx).terminal_view().read(cx).terminal.entity_id())
            .collect();
        let stale_shares: Vec<_> = self
            .web_shares
            .keys()
            .filter(|terminal_id| !open_terminals.contains(terminal_id))
            .copied()
            .collect();
        for terminal_id in stale_shares {
            self.stop_web_share_for_terminal(terminal_id, cx);
        }
        self.web_shares_starting
            .retain(|terminal_id| open_terminals.contains(terminal_id));
    }

    pub(super) fn stop_web_share_for_terminal(
        &mut self,
        terminal_id: gpui::EntityId,
        cx: &mut Context<Self>,
    ) -> Option<WebShareEntry> {
        self.web_shares_starting.remove(&terminal_id);
        let share = self.web_shares.remove(&terminal_id);
        if let Some(share) = &share {
            share.server.shutdown();
        }
        self.web_share_indicator.deactivate(terminal_id);
        cx.set_global(self.web_share_indicator.clone());
        share
    }

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
                        let cancel_button = Button::new("termua-quit-confirm-cancel")
                            .label(t!("MainWindow.QuitConfirm.Button.Cancel").to_string())
                            .debug_selector(|| "termua-quit-confirm-cancel".to_string());
                        let quit_button = Button::new("termua-quit-confirm-quit")
                            .label(t!("MainWindow.QuitConfirm.Button.Quit").to_string())
                            .primary()
                            .debug_selector(|| "termua-quit-confirm-quit".to_string());

                        dialog
                            .title(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(
                                        Icon::default()
                                            .path(TermuaIcon::AlertCircle)
                                            .text_color(_app.theme().warning),
                                    )
                                    .child(t!("MainWindow.QuitConfirm.Title").to_string()),
                            )
                            .child(
                                h_flex()
                                    .gap_3()
                                    .items_start()
                                    .debug_selector(|| "termua-quit-confirm-body".to_string())
                                    .child(
                                        div()
                                            .flex_1()
                                            .child(t!("MainWindow.QuitConfirm.Body").to_string()),
                                    ),
                            )
                            .footer(
                                DialogFooter::new()
                                    .child(DialogClose::new().child(cancel_button))
                                    .child(DialogAction::new().child(quit_button)),
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
    fn terminal_tab_label(
        &self,
        terminal_view: &gpui::Entity<gpui_term::TerminalView>,
        cx: &App,
    ) -> gpui::SharedString {
        self.dock_area
            .read(cx)
            .visible_tab_panels(cx)
            .into_iter()
            .filter_map(|tab_panel| tab_panel.read(cx).active_panel(cx))
            .filter_map(|panel| panel.view().downcast::<TerminalPanel>().ok())
            .find_map(|panel| {
                let panel = panel.read(cx);
                (panel.terminal_view().entity_id() == terminal_view.entity_id())
                    .then(|| panel.tab_label())
            })
            .unwrap_or_else(|| "terminal".into())
    }

    pub(crate) fn open_web_control_request_dialog(
        &mut self,
        peer: std::net::SocketAddr,
        tab_label: String,
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
                        .child("Browser control request — ")
                        .child(
                            div()
                                .debug_selector(|| "termua-web-control-dialog-tab-name".to_string())
                                .child(tab_label.clone()),
                        )
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
                        .child(
                            div()
                                .text_sm()
                                .text_color(app.theme().muted_foreground)
                                .child("Request source"),
                        )
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
                            "If allowed, this browser can send keyboard input to the current \
                             terminal until sharing ends.",
                        ));

                    dialog
                        .title(title)
                        .w(px(560.))
                        .child(
                            v_flex()
                                .gap_3()
                                .child(
                                    "A browser on your local network is requesting control of \
                                     this terminal.",
                                )
                                .child(source)
                                .child(notice),
                        )
                        .footer(
                            DialogFooter::new()
                                .child(
                                    DialogClose::new().child(
                                        Button::new("termua-web-control-dialog-deny")
                                            .label("Deny")
                                            .debug_selector(|| {
                                                "termua-web-control-dialog-deny".to_string()
                                            }),
                                    ),
                                )
                                .child(
                                    DialogAction::new().child(
                                        Button::new("termua-web-control-dialog-allow")
                                            .primary()
                                            .label("Allow control")
                                            .debug_selector(|| {
                                                "termua-web-control-dialog-allow".to_string()
                                            }),
                                    ),
                                ),
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

    pub(super) fn on_revoke_web_control(
        &mut self,
        _: &RevokeWebControl,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(terminal_id) = self
            .focused_terminal_view
            .as_ref()
            .and_then(|view| view.upgrade())
            .map(|view| view.read(cx).terminal.entity_id())
        else {
            return;
        };
        if let Some(share) = self.web_shares.get(&terminal_id) {
            share.server.revoke_control();
        }
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
        let tab_label = self.terminal_tab_label(&terminal_view, cx);
        let terminal = terminal_view.read(cx).terminal.clone();
        let terminal_id = terminal.entity_id();
        match web_share_toggle_action(
            self.web_shares.contains_key(&terminal_id),
            self.web_shares_starting.contains(&terminal_id),
        ) {
            WebShareToggleAction::Stop => {
                let share = self
                    .stop_web_share_for_terminal(terminal_id, cx)
                    .expect("active web share must exist");
                terminal_view.update(cx, |_, cx| cx.notify());
                notification::notify_deferred(
                    notification::MessageKind::Info,
                    web_share_stopped_message(share.tab_label.as_ref()),
                    window,
                    cx,
                );
                return;
            }
            WebShareToggleAction::Wait => return,
            WebShareToggleAction::Start => {
                self.web_shares_starting.insert(terminal_id);
            }
        }
        let snapshot = gpui_term::capture_terminal_screen_with_line_numbers(
            terminal.read(cx),
            show_web_line_numbers(cx),
        );
        let token = crate::web::generate_token();
        let xterm_theme = crate::web::XtermTheme::from_app_theme(cx.theme());
        let xterm_font_family =
            crate::web::xterm_font_family(gpui_term::TerminalSettings::global(cx));
        let web_share_manager = Arc::clone(&self.web_share_manager);
        let (web_share_port, web_share_timeout_minutes) =
            if cx.has_global::<crate::settings::WebSharingSettings>() {
                let settings = cx.global::<crate::settings::WebSharingSettings>();
                (settings.port, settings.timeout_minutes)
            } else {
                (
                    crate::settings::DEFAULT_WEB_SHARING_PORT,
                    crate::settings::DEFAULT_WEB_SHARING_TIMEOUT_MINUTES,
                )
            };
        let web_share_timeout = web_share_idle_timeout(web_share_timeout_minutes);

        cx.spawn_in(window, async move |this, window| {
            let result = crate::web::WebShareServer::bind_with_manager(
                &web_share_manager,
                web_share_port,
                token.clone(),
                snapshot,
                xterm_theme,
                xterm_font_family,
                tab_label.to_string(),
            )
            .await;
            let _ = this.update_in(window, move |this, window, cx| {
                if !this.web_shares_starting.remove(&terminal_id) {
                    if let Ok(server) = result {
                        server.shutdown();
                    }
                    return;
                }
                let server = match result {
                    Ok(server) => std::sync::Arc::new(server),
                    Err(error) => {
                        notification::notify_deferred(
                            notification::MessageKind::Error,
                            web_share_start_failed_message(
                                tab_label.as_ref(),
                                web_share_port,
                                &error,
                            ),
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
                    "http://{}:{}{}#token={token}",
                    crate::web::local_network_ip(),
                    server.local_addr().port(),
                    server.session_path()
                );
                cx.write_to_clipboard(ClipboardItem::new_string(url.clone()));
                notification::notify_deferred(
                    notification::MessageKind::Warning,
                    web_share_started_message(tab_label.as_ref(), &url),
                    window,
                    cx,
                );

                let snapshot_server = std::sync::Arc::clone(&server);
                let content_subscription =
                    cx.subscribe(&terminal, move |_this, terminal, event, cx| {
                        if !matches!(event, gpui_term::Event::ContentUpdated) {
                            return;
                        }
                        let snapshot = gpui_term::capture_terminal_screen_with_line_numbers(
                            terminal.read(cx),
                            show_web_line_numbers(cx),
                        );
                        snapshot_server.update_snapshot(snapshot);
                    });
                let settings_server = std::sync::Arc::clone(&server);
                let settings_terminal = terminal.clone();
                let settings_subscription =
                    cx.observe_global::<gpui_term::TerminalSettings>(move |_this, cx| {
                        let snapshot = gpui_term::capture_terminal_screen_with_line_numbers(
                            settings_terminal.read(cx),
                            show_web_line_numbers(cx),
                        );
                        settings_server.update_snapshot(snapshot);
                    });

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

                let control_rx = server.control_requests();
                let approval_server = std::sync::Arc::clone(&server);
                let approval_tab_label = tab_label.to_string();
                cx.spawn_in(window, async move |this, window| {
                    while let Ok(request) = control_rx.recv().await {
                        let (decision_tx, decision_rx) = smol::channel::bounded(1);
                        let request_tab_label = approval_tab_label.clone();
                        let opened = this.update_in(window, move |this, window, cx| {
                            this.open_web_control_request_dialog(
                                request.peer,
                                request_tab_label,
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

                this.web_shares.insert(
                    terminal_id,
                    WebShareEntry {
                        server,
                        tab_label,
                        _subscriptions: vec![content_subscription, settings_subscription],
                    },
                );
                this.web_share_indicator.activate(terminal_id, url);
                cx.set_global(this.web_share_indicator.clone());
                terminal_view.update(cx, |_, cx| cx.notify());

                let expiring_server = this
                    .web_shares
                    .get(&terminal_id)
                    .map(|share| std::sync::Arc::clone(&share.server))
                    .unwrap();
                let expiring_terminal = terminal.downgrade();
                let expiring_terminal_view = terminal_view.downgrade();
                cx.spawn(async move |this, cx| {
                    loop {
                        smol::Timer::after(std::time::Duration::from_secs(1)).await;
                        let client_count = expiring_server.client_count();
                        let _ = this.update(cx, |this, cx| {
                            if this
                                .web_share_indicator
                                .set_client_count(terminal_id, client_count)
                                && let Some(terminal_view) = expiring_terminal_view.upgrade()
                            {
                                terminal_view.update(cx, |_, cx| cx.notify());
                            }
                        });
                        if expiring_server.is_closed()
                            || expiring_terminal.upgrade().is_none()
                            || expiring_server.is_inactive_for(web_share_timeout)
                        {
                            break;
                        }
                    }
                    expiring_server.shutdown();
                    let _ = this.update(cx, |this, cx| {
                        if this.web_shares.get(&terminal_id).is_some_and(|active| {
                            std::sync::Arc::ptr_eq(&active.server, &expiring_server)
                        }) {
                            this.web_shares.remove(&terminal_id);
                            this.web_share_indicator.deactivate(terminal_id);
                            cx.set_global(this.web_share_indicator.clone());
                            if let Some(terminal_view) = expiring_terminal_view.upgrade() {
                                terminal_view.update(cx, |_, cx| cx.notify());
                            }
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
