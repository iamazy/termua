use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use gpui::{AppContext, Context, Focusable, ParentElement, ReadGlobal, Styled, Window, px};
use gpui_component::v_flex;
use gpui_term::{
    Event as TerminalEvent, RemoteBackendEvent, TerminalSettings, TerminalView,
    remote::{RemoteFrame, RemoteInputEvent, RemoteSnapshot, RemoteTerminalContent},
};
use smol::Timer;

use super::TermuaWindow;
use crate::{
    PendingCommand, TermuaAppState, lock_screen, notification,
    sharing::{
        ClientToRelay as RelayClientToRelay, HostShare, RelaySharingState,
        RelayToClient as RelayRelayToClient, ReleaseControl, RequestControl, RevokeControl,
        StartSharing, StopSharing, connect_relay, gen_join_key, gen_room_id, parse_share_key,
    },
};

#[derive(thiserror::Error, Debug, Eq, PartialEq)]
enum JoinSharingInputError {
    #[error("Relay URL / Share Key cannot be empty.")]
    EmptyFields,
    #[error("Relay URL must start with ws:// or wss://")]
    InvalidRelayUrl,
    #[error("Invalid Share Key: {0}")]
    InvalidShareKey(String),
}

fn build_join_sharing_pending_command(
    relay_url: &str,
    share_key: &str,
) -> Result<PendingCommand, JoinSharingInputError> {
    let relay_url = relay_url.trim().to_string();
    let share_key = share_key.trim();

    if relay_url.is_empty() || share_key.is_empty() {
        return Err(JoinSharingInputError::EmptyFields);
    }
    if !relay_url.starts_with("ws://") && !relay_url.starts_with("wss://") {
        return Err(JoinSharingInputError::InvalidRelayUrl);
    }

    let (room_id, join_key) = parse_share_key(share_key)
        .map_err(|err| JoinSharingInputError::InvalidShareKey(err.to_string()))?;
    if room_id.is_empty() || join_key.is_empty() {
        return Err(JoinSharingInputError::InvalidShareKey(
            share_key.to_string(),
        ));
    }

    Ok(PendingCommand::JoinRelaySharing {
        relay_url,
        room_id,
        join_key,
    })
}

impl TermuaWindow {
    pub(in crate::window::main_window) fn on_start_sharing(
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

    pub(in crate::window::main_window) fn on_stop_sharing(
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

    pub(in crate::window::main_window) fn on_request_control(
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

    pub(in crate::window::main_window) fn on_release_control(
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

    pub(in crate::window::main_window) fn on_revoke_control(
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

    pub(super) fn spawn_relay_pump_for_viewer(
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

    pub(super) fn open_join_sharing_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
                                let command = match build_join_sharing_pending_command(
                                    &relay_url, &share_key,
                                ) {
                                    Ok(command) => command,
                                    Err(err) => {
                                        notification::notify_app(
                                            notification::MessageKind::Warning,
                                            err.to_string(),
                                            window,
                                            app,
                                        );
                                        return false;
                                    }
                                };
                                app.global_mut::<TermuaAppState>().pending_command(command);
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
}

#[cfg(test)]
mod tests {
    use super::{JoinSharingInputError, build_join_sharing_pending_command};
    use crate::PendingCommand;

    #[test]
    fn build_join_sharing_pending_command_accepts_valid_share_key() {
        let command =
            build_join_sharing_pending_command(" wss://relay.example/ws ", " AbC234xYz-k3Y9a2 ")
                .expect("valid join sharing input");

        match command {
            PendingCommand::JoinRelaySharing {
                relay_url,
                room_id,
                join_key,
            } => {
                assert_eq!(relay_url, "wss://relay.example/ws");
                assert_eq!(room_id, "AbC234xYz");
                assert_eq!(join_key, "k3Y9a2");
            }
            other => panic!("expected JoinRelaySharing, got {other:?}"),
        }
    }

    #[test]
    fn build_join_sharing_pending_command_rejects_empty_fields() {
        assert_eq!(
            build_join_sharing_pending_command("", "AbC234xYz-k3Y9a2")
                .expect_err("empty relay URL"),
            JoinSharingInputError::EmptyFields
        );
        assert_eq!(
            build_join_sharing_pending_command("ws://relay.example/ws", " ")
                .expect_err("empty share key"),
            JoinSharingInputError::EmptyFields
        );
    }

    #[test]
    fn build_join_sharing_pending_command_rejects_non_websocket_relay_url() {
        assert_eq!(
            build_join_sharing_pending_command("https://relay.example/ws", "AbC234xYz-k3Y9a2")
                .expect_err("non-websocket relay URL"),
            JoinSharingInputError::InvalidRelayUrl
        );
    }

    #[test]
    fn build_join_sharing_pending_command_rejects_invalid_share_key() {
        let err = build_join_sharing_pending_command("ws://relay.example/ws", "bad-key")
            .expect_err("invalid share key");

        assert!(matches!(err, JoinSharingInputError::InvalidShareKey(_)));
        assert!(err.to_string().starts_with("Invalid Share Key: "));
    }
}
