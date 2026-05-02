use std::{
    collections::HashMap,
    sync::{Arc, Mutex, atomic::AtomicBool},
};

use gpui::{AppContext, Context, FocusHandle, ReadGlobal, SharedString, Window};
use gpui_dock::{DockPlacement, PanelView};
use gpui_term::{
    CursorShape, Event as TerminalEvent, PtySource, SerialOptions, SshOptions, TerminalBuilder,
    TerminalSettings, TerminalType, TerminalView, UserInput as TerminalUserInput,
    remote::RemoteInputEvent,
};

use super::TermuaWindow;
use crate::{
    SerialParams, TermuaAppState,
    env::{build_terminal_env, cast_player_child_env},
    lock_screen, notification,
    panel::{PanelKind, TerminalPanel, terminal_panel_tab_name},
    sharing::{
        ClientToRelay as RelayClientToRelay, RelaySharingState, ViewerShare, compose_share_key,
        connect_relay,
    },
    ssh::{dedupe_tab_label, ssh_tab_tooltip},
};

impl TermuaWindow {
    pub(super) fn add_local_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.add_local_terminal_with_params(TerminalType::WezTerm, HashMap::new(), window, cx);
    }

    pub(super) fn open_cast_player_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    pub(super) fn add_local_terminal_with_params(
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

    pub(super) fn add_relay_viewer_terminal(
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

    pub(super) fn add_serial_terminal_with_params(
        &mut self,
        backend_type: TerminalType,
        params: SerialParams,
        session_id: Option<i64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let opts = params.to_options();

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
        );

        let builder = match builder {
            Ok(builder) => builder,
            Err(err) => {
                if let Some(_session_id) = session_id {
                    let reason = err.root_cause().to_string();
                    let hint = crate::serial::open_failure_hint(&params.port, &err);
                    let message: SharedString = match hint {
                        Some(hint) => format!(
                            "Failed to open serial port `{}`.\n\nError:\n{reason}\n\n{hint}",
                            params.port
                        )
                        .into(),
                        None => format!(
                            "Failed to open serial port `{}`.\n\nError:\n{reason}",
                            params.port
                        )
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
                let hint = crate::serial::open_failure_hint(&params.port, &err);
                let message = match hint {
                    Some(hint) => format!(
                        "Failed to open serial port `{}`.\n\nError:\n{reason}\n\n{hint}",
                        params.port
                    ),
                    None => format!(
                        "Failed to open serial port `{}`.\n\nError:\n{reason}",
                        params.port
                    ),
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

        let panel = self.build_serial_panel_from_builder(builder, params.name, opts, window, cx);
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

    pub(in crate::window::main_window) fn open_session_by_id(
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
        let session_env = session.env.clone().unwrap_or_default();
        let env = build_terminal_env(
            gpui_term::shell::default_shell_program(),
            session.term(),
            session.colorterm(),
            session.charset(),
            &session_env,
        );
        self.add_local_terminal_with_params(backend_type, env, window, cx);
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
            SerialParams {
                name: session.label,
                port,
                baud,
                data_bits,
                parity,
                stop_bits,
                flow_control,
            },
            Some(session_id),
            window,
            cx,
        );
    }

    pub(super) fn reload_sessions_sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    pub(crate) fn subscribe_terminal_view_events(
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
            move |this, _, event, window, cx| match event {
                TerminalEvent::UserInput(input) => {
                    if this.close_exited_terminal_panel(
                        &source_terminal_view_for_cb,
                        input,
                        window,
                        cx,
                    ) {
                        return;
                    }
                    this.on_terminal_user_input(
                        source_terminal_view_for_cb.clone(),
                        input.clone(),
                        cx,
                    )
                }
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

    pub(super) fn build_ssh_panel_from_builder(
        &mut self,
        builder: TerminalBuilder,
        name: String,
        opts: SshOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Entity<TerminalPanel> {
        let id = self.next_terminal_id;
        self.next_terminal_id += 1;

        let tab_label = dedupe_tab_label(&mut self.ssh_tab_label_counts, name.as_str());
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

    fn build_serial_panel_from_builder(
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
            TerminalBuilder::new(backend_type, env, CursorShape::default(), None, id as u64)
                .expect("local terminal builder should succeed")
                .subscribe(cx)
        });
        self.build_wired_terminal_panel(id, kind, tab_label, None, terminal, window, cx)
    }

    pub(super) fn open_sftp_for_terminal_view(
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

    fn close_exited_terminal_panel(
        &mut self,
        source: &gpui::Entity<TerminalView>,
        input: &TerminalUserInput,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let TerminalUserInput::Keystroke(keystroke) = input else {
            return false;
        };
        if keystroke.key.as_str() != "d"
            || !keystroke.modifiers.control
            || keystroke.modifiers.alt
            || keystroke.modifiers.platform
            || keystroke.modifiers.function
            || keystroke.modifiers.shift
        {
            return false;
        }

        let Some(panel) = self.find_visible_terminal_panel(cx, |terminal_panel, cx| {
            matches!(terminal_panel.kind(), PanelKind::Ssh | PanelKind::Recorder)
                && terminal_panel.terminal_view().entity_id() == source.entity_id()
                && terminal_panel
                    .terminal_view()
                    .read(cx)
                    .terminal
                    .read(cx)
                    .has_exited()
        }) else {
            return false;
        };

        self.close_terminal_panel(panel, window, cx);
        true
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

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use gpui::{AppContext, Entity};
    use gpui_term::TerminalType;

    use super::TermuaWindow;
    use crate::{
        SerialParams, TermuaAppState, notification,
        store::{SerialFlowControl, SerialParity, SerialStopBits},
    };

    fn init_test_app(cx: &mut gpui::TestAppContext) {
        cx.update(|app| {
            gpui_component::init(app);
            menubar::init(app);
            gpui_term::init(app);
            gpui_dock::init(app);
            app.set_global(TermuaAppState::default());
            crate::sharing::init_globals(app);
        });
    }

    fn add_termua_window(
        cx: &mut gpui::TestAppContext,
    ) -> (Entity<TermuaWindow>, &mut gpui::VisualTestContext) {
        let slot: Rc<RefCell<Option<Entity<TermuaWindow>>>> = Rc::new(RefCell::new(None));
        let slot_for_root = Rc::clone(&slot);
        let (_root, window_cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| TermuaWindow::new(window, cx));
            *slot_for_root.borrow_mut() = Some(view.clone());
            gpui_component::Root::new(view, window, cx)
        });

        let termua = slot
            .borrow()
            .as_ref()
            .expect("expected TermuaWindow view to be captured")
            .clone();
        (termua, window_cx)
    }

    #[gpui::test]
    fn add_relay_viewer_terminal_invalid_url_reports_notification(cx: &mut gpui::TestAppContext) {
        init_test_app(cx);
        let (termua, window_cx) = add_termua_window(cx);

        window_cx.update(|window, app| {
            termua.update(app, |this, cx| {
                this.add_relay_viewer_terminal(
                    "http://relay.example/ws".to_string(),
                    "AbC234xYz".to_string(),
                    "k3Y9a2".to_string(),
                    window,
                    cx,
                );
            });
        });
        window_cx.run_until_parked();

        window_cx.update(|_window, app| {
            let notifications = &app.global::<notification::NotifyState>().messages;
            assert!(
                notifications
                    .iter()
                    .any(|msg| msg.message.contains("Join sharing failed:")),
                "expected a join sharing error notification for an invalid relay URL"
            );
        });
    }

    #[gpui::test]
    fn saved_serial_session_open_failure_includes_edit_hint(cx: &mut gpui::TestAppContext) {
        init_test_app(cx);
        let (termua, window_cx) = add_termua_window(cx);

        window_cx.update(|window, app| {
            termua.update(app, |this, cx| {
                this.add_serial_terminal_with_params(
                    TerminalType::WezTerm,
                    SerialParams {
                        name: "broken".to_string(),
                        port: "/tmp/termua-no-such-serial-port".to_string(),
                        baud: 115200,
                        data_bits: 8,
                        parity: SerialParity::None,
                        stop_bits: SerialStopBits::One,
                        flow_control: SerialFlowControl::None,
                    },
                    Some(42),
                    window,
                    cx,
                );
            });
        });
        window_cx.run_until_parked();

        window_cx.update(|_window, app| {
            let notifications = &app.global::<notification::NotifyState>().messages;
            let message = notifications
                .iter()
                .find(|msg| msg.message.contains("Failed to open serial port"))
                .map(|msg| msg.message.as_ref())
                .expect("expected a serial open failure notification");
            assert!(
                message
                    .contains("Tip: Right-click the session and choose Edit to change the port."),
                "expected saved-session serial failures to include the edit hint"
            );
        });
    }

    #[gpui::test]
    fn ad_hoc_serial_open_failure_omits_saved_session_edit_hint(cx: &mut gpui::TestAppContext) {
        init_test_app(cx);
        let (termua, window_cx) = add_termua_window(cx);

        window_cx.update(|window, app| {
            termua.update(app, |this, cx| {
                this.add_serial_terminal_with_params(
                    TerminalType::WezTerm,
                    SerialParams {
                        name: "broken".to_string(),
                        port: "/tmp/termua-no-such-serial-port".to_string(),
                        baud: 115200,
                        data_bits: 8,
                        parity: SerialParity::None,
                        stop_bits: SerialStopBits::One,
                        flow_control: SerialFlowControl::None,
                    },
                    None,
                    window,
                    cx,
                );
            });
        });
        window_cx.run_until_parked();

        window_cx.update(|_window, app| {
            let notifications = &app.global::<notification::NotifyState>().messages;
            let message = notifications
                .iter()
                .find(|msg| msg.message.contains("Failed to open serial port"))
                .map(|msg| msg.message.as_ref())
                .expect("expected a serial open failure notification");
            assert!(
                !message
                    .contains("Tip: Right-click the session and choose Edit to change the port."),
                "expected ad-hoc serial failures to omit the saved-session edit hint"
            );
        });
    }
}
