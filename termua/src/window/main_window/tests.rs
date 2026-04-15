use std::{
    borrow::Cow,
    collections::HashMap,
    ops::RangeInclusive,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use gpui::{
    AppContext, Bounds, Context, InteractiveElement, IntoElement, Keystroke, Modifiers,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, ScrollWheelEvent,
    SharedString, Styled, Window, div,
};
use gpui_component::input::InputState;
use gpui_term::{
    Authentication, CursorShape, Event as TerminalEvent, SshOptions, Terminal, TerminalBackend,
    TerminalBounds, TerminalType, TerminalView,
};

use super::*;
use crate::{
    TermuaAppState, ToggleSessionsSidebar, lock_screen,
    menu::Quit,
    notification,
    ssh::{SshHostKeyMismatchDetails, SshTerminalBuilderFn},
};

#[gpui::test]
fn ssh_host_key_mismatch_dialog_renders_label_prefixes(cx: &mut gpui::TestAppContext) {
    use std::{cell::RefCell, rc::Rc};

    cx.update(|app| {
        gpui_component::init(app);
        menubar::init(app);
        gpui_term::init(app);
        gpui_dock::init(app);
        app.set_global(TermuaAppState::default());
    });

    let termua_slot: Rc<RefCell<Option<gpui::Entity<TermuaWindow>>>> = Rc::new(RefCell::new(None));
    let termua_slot_for_view = Rc::clone(&termua_slot);

    let (root, cx) = cx.add_window_view(|window, cx| {
        let view = cx.new(|cx| TermuaWindow::new(window, cx));
        *termua_slot_for_view.borrow_mut() = Some(view.clone());
        gpui_component::Root::new(view, window, cx)
    });

    cx.update(|window, app| {
        let termua = termua_slot
            .borrow()
            .as_ref()
            .expect("expected TermuaWindow view to be captured")
            .clone();

        termua.update(app, |this, cx| {
            this.open_ssh_host_key_mismatch_dialog(
                TerminalType::WezTerm,
                HashMap::new(),
                SshOptions {
                    group: "ssh".to_string(),
                    name: "prod".to_string(),
                    host: "127.0.0.1".to_string(),
                    port: Some(22),
                    auth: Authentication::Config,
                    proxy: gpui_term::SshProxyMode::Inherit,
                    backend: gpui_term::SshBackend::default(),
                    tcp_nodelay: false,
                    tcp_keepalive: false,
                },
                "host key mismatch".to_string(),
                SshHostKeyMismatchDetails {
                    got_fingerprint: Some("SHA256:demo".to_string()),
                    known_hosts_path: Some(std::path::PathBuf::from("/tmp/known_hosts")),
                    server_host: Some("127.0.0.1".to_string()),
                    server_port: Some(22),
                },
                window,
                cx,
            );
        });
    });

    cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(900.)),
            gpui::AvailableSpace::Definite(gpui::px(600.)),
        ),
        move |_, _| div().size_full().child(root),
    );
    cx.run_until_parked();

    for selector in [
        "termua-ssh-hostkey-mismatch-label-target",
        "termua-ssh-hostkey-mismatch-label-server",
        "termua-ssh-hostkey-mismatch-label-reason",
        "termua-ssh-hostkey-mismatch-label-fingerprint",
        "termua-ssh-hostkey-mismatch-label-known-hosts",
        "termua-ssh-hostkey-mismatch-label-manual-fix",
        "termua-ssh-hostkey-mismatch-label-note",
        "termua-ssh-hostkey-mismatch-value-target",
        "termua-ssh-hostkey-mismatch-value-server",
        "termua-ssh-hostkey-mismatch-value-reason",
        "termua-ssh-hostkey-mismatch-value-fingerprint",
        "termua-ssh-hostkey-mismatch-value-known-hosts",
        "termua-ssh-hostkey-mismatch-value-manual-fix",
        "termua-ssh-hostkey-mismatch-value-note",
    ] {
        assert!(
            cx.debug_bounds(selector).is_some(),
            "expected {selector} to be debuggable"
        );
    }
}

#[gpui::test]
fn request_quit_without_open_tabs_does_not_open_confirmation_dialog(cx: &mut gpui::TestAppContext) {
    use std::{cell::RefCell, rc::Rc};

    let termua_slot: Rc<RefCell<Option<gpui::Entity<TermuaWindow>>>> = Rc::new(RefCell::new(None));
    let termua_slot_for_view = termua_slot.clone();

    cx.update(|app| {
        gpui_component::init(app);
        menubar::init(app);
        gpui_term::init(app);
        gpui_dock::init(app);
        app.set_global(TermuaAppState::default());
    });

    let (root, window_cx) = cx.add_window_view(|window, cx| {
        let view = cx.new(|cx| TermuaWindow::new(window, cx));
        *termua_slot_for_view.borrow_mut() = Some(view.clone());
        gpui_component::Root::new(view, window, cx)
    });

    window_cx.update(|window, cx| {
        let termua = termua_slot
            .borrow()
            .clone()
            .expect("expected TermuaWindow view");
        termua.update(cx, |this, cx| {
            this.request_quit(window, cx);
        });
    });

    window_cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(900.)),
            gpui::AvailableSpace::Definite(gpui::px(600.)),
        ),
        move |_, _| div().size_full().child(root),
    );
    window_cx.run_until_parked();

    assert!(
        window_cx.debug_bounds("termua-quit-confirm-body").is_none(),
        "did not expect quit confirmation dialog without open tabs"
    );
}

#[gpui::test]
fn request_quit_with_open_tabs_requires_confirmation(cx: &mut gpui::TestAppContext) {
    use std::{cell::RefCell, rc::Rc};

    use gpui::{App, Context, EventEmitter, FocusHandle, Focusable, Render, Window, div};
    use gpui_dock::{DockPlacement, Panel, PanelEvent, PanelView};

    struct DummyPanel {
        focus: FocusHandle,
    }

    impl DummyPanel {
        fn new(cx: &mut Context<Self>) -> Self {
            Self {
                focus: cx.focus_handle(),
            }
        }
    }

    impl EventEmitter<PanelEvent> for DummyPanel {}

    impl Focusable for DummyPanel {
        fn focus_handle(&self, _: &App) -> FocusHandle {
            self.focus.clone()
        }
    }

    impl Panel for DummyPanel {
        fn panel_name(&self) -> &'static str {
            "termua.test.quit_confirm_dummy_panel"
        }

        fn tab_name(&self, _: &App) -> Option<SharedString> {
            Some("Terminal".into())
        }

        fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().child("Terminal")
        }
    }

    impl Render for DummyPanel {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full()
        }
    }

    let termua_slot: Rc<RefCell<Option<gpui::Entity<TermuaWindow>>>> = Rc::new(RefCell::new(None));
    let termua_slot_for_view = termua_slot.clone();

    cx.update(|app| {
        gpui_component::init(app);
        menubar::init(app);
        gpui_term::init(app);
        gpui_dock::init(app);
        app.set_global(TermuaAppState::default());
    });

    let (root, window_cx) = cx.add_window_view(|window, cx| {
        let view = cx.new(|cx| TermuaWindow::new(window, cx));
        *termua_slot_for_view.borrow_mut() = Some(view.clone());
        gpui_component::Root::new(view, window, cx)
    });

    window_cx.update(|window, cx| {
        let panel: Arc<dyn PanelView> = Arc::new(cx.new(DummyPanel::new));
        let termua = termua_slot
            .borrow()
            .clone()
            .expect("expected TermuaWindow view");

        termua.update(cx, |this, cx| {
            this.dock_area.update(cx, |dock, cx| {
                dock.add_panel(panel, DockPlacement::Center, None, window, cx);
            });
            this.request_quit(window, cx);
        });
    });

    window_cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(900.)),
            gpui::AvailableSpace::Definite(gpui::px(600.)),
        ),
        move |_, _| div().size_full().child(root),
    );
    window_cx.run_until_parked();

    assert!(
        window_cx.debug_bounds("termua-quit-confirm-body").is_some(),
        "expected quit confirmation dialog when tabs are open"
    );
}

#[gpui::test]
fn menu_quit_with_open_tabs_opens_confirmation_dialog_without_panicking(
    cx: &mut gpui::TestAppContext,
) {
    use std::{cell::RefCell, rc::Rc};

    use gpui::{App, Context, EventEmitter, FocusHandle, Focusable, Render, Window, div};
    use gpui_dock::{DockPlacement, Panel, PanelEvent, PanelView};

    struct DummyPanel {
        focus: FocusHandle,
    }

    impl DummyPanel {
        fn new(cx: &mut Context<Self>) -> Self {
            Self {
                focus: cx.focus_handle(),
            }
        }
    }

    impl EventEmitter<PanelEvent> for DummyPanel {}

    impl Focusable for DummyPanel {
        fn focus_handle(&self, _: &App) -> FocusHandle {
            self.focus.clone()
        }
    }

    impl Panel for DummyPanel {
        fn panel_name(&self) -> &'static str {
            "termua.test.menu_quit_confirm_dummy_panel"
        }

        fn tab_name(&self, _: &App) -> Option<SharedString> {
            Some("Terminal".into())
        }

        fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().child("Terminal")
        }
    }

    impl Render for DummyPanel {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full()
        }
    }

    let termua_slot: Rc<RefCell<Option<gpui::Entity<TermuaWindow>>>> = Rc::new(RefCell::new(None));
    let termua_slot_for_view = termua_slot.clone();

    cx.update(|app| {
        gpui_component::init(app);
        menubar::init(app);
        gpui_term::init(app);
        gpui_dock::init(app);
        app.set_global(TermuaAppState::default());
        crate::menu::register(app);
    });

    let (root, window_cx) = cx.add_window_view(|window, cx| {
        let view = cx.new(|cx| TermuaWindow::new(window, cx));
        *termua_slot_for_view.borrow_mut() = Some(view.clone());
        gpui_component::Root::new(view, window, cx)
    });

    window_cx.update(|window, cx| {
        let root_handle = window
            .window_handle()
            .downcast::<gpui_component::Root>()
            .expect("expected Root window handle");
        cx.global_mut::<TermuaAppState>().main_window = Some(root_handle);

        let panel: Arc<dyn PanelView> = Arc::new(cx.new(DummyPanel::new));
        let termua = termua_slot
            .borrow()
            .clone()
            .expect("expected TermuaWindow view");

        termua.update(cx, |this, cx| {
            this.dock_area.update(cx, |dock, cx| {
                dock.add_panel(panel, DockPlacement::Center, None, window, cx);
            });
        });
    });

    window_cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(900.)),
            gpui::AvailableSpace::Definite(gpui::px(600.)),
        ),
        move |_, _| div().size_full().child(root),
    );
    window_cx.run_until_parked();

    window_cx.update(|_, app| {
        app.dispatch_action(&Quit);
    });
    window_cx.run_until_parked();

    assert!(
        window_cx.debug_bounds("termua-quit-confirm-body").is_some(),
        "expected quit confirmation dialog when menu quit is triggered with open tabs"
    );
}

#[gpui::test]
fn ssh_connect_does_not_block_main_thread(cx: &mut gpui::TestAppContext) {
    cx.update(|app| {
        gpui_component::init(app);
        menubar::init(app);
        gpui_term::init(app);
        gpui_dock::init(app);
        app.set_global(TermuaAppState::default());
    });

    let slow_builder: SshTerminalBuilderFn = Arc::new(|_backend, _env, _opts| {
        std::thread::sleep(Duration::from_millis(400));
        Err(anyhow::anyhow!("simulated slow ssh connect"))
    });

    let (view, window_cx) = cx.add_window_view(|window, cx| {
        TermuaWindow::new_with_ssh_terminal_builder(window, slow_builder, cx)
    });

    let start = Instant::now();
    window_cx.update(|window, cx| {
        view.update(cx, |this, cx| {
            this.add_ssh_terminal_with_params(
                TerminalType::WezTerm,
                HashMap::new(),
                SshOptions {
                    group: "ssh".to_string(),
                    name: "prod".to_string(),
                    host: "example.com".to_string(),
                    port: Some(22),
                    auth: Authentication::Password("alice".to_string(), "pw".to_string()),
                    proxy: gpui_term::SshProxyMode::Inherit,
                    backend: gpui_term::SshBackend::default(),
                    tcp_nodelay: false,
                    tcp_keepalive: false,
                },
                None,
                window,
                cx,
            );
        });
    });

    assert!(
        start.elapsed() < Duration::from_millis(150),
        "ssh connect should be validated in background without blocking the UI thread"
    );
}

#[gpui::test]
fn ssh_connect_failure_opens_an_error_tab(cx: &mut gpui::TestAppContext) {
    cx.update(|app| {
        gpui_component::init(app);
        menubar::init(app);
        gpui_term::init(app);
        gpui_dock::init(app);
        app.set_global(TermuaAppState::default());
    });

    let failing_builder: SshTerminalBuilderFn = Arc::new(|_backend, _env, _opts| {
        Err(anyhow::anyhow!(
            "ssh login error: password auth status: Denied"
        ))
    });

    let (view, window_cx) = cx.add_window_view(|window, cx| {
        TermuaWindow::new_with_ssh_terminal_builder(window, failing_builder, cx)
    });

    window_cx.update(|window, cx| {
        view.update(cx, |this, cx| {
            this.add_ssh_terminal_with_params(
                TerminalType::WezTerm,
                HashMap::new(),
                SshOptions {
                    group: "ssh".to_string(),
                    name: "prod".to_string(),
                    host: "example.com".to_string(),
                    port: Some(22),
                    auth: Authentication::Password("alice".to_string(), "pw".to_string()),
                    proxy: gpui_term::SshProxyMode::Inherit,
                    backend: gpui_term::SshBackend::default(),
                    tcp_nodelay: false,
                    tcp_keepalive: false,
                },
                None,
                window,
                cx,
            );
        });
    });

    for _ in 0..10 {
        let view_for_draw = view.clone();
        window_cx.draw(
            gpui::point(gpui::px(0.), gpui::px(0.)),
            gpui::size(
                gpui::AvailableSpace::Definite(gpui::px(900.)),
                gpui::AvailableSpace::Definite(gpui::px(600.)),
            ),
            move |_, _| div().size_full().child(view_for_draw),
        );
        window_cx.run_until_parked();

        if window_cx.debug_bounds("termua-ssh-error-panel").is_some() {
            break;
        }
    }

    window_cx
        .debug_bounds("termua-ssh-error-panel")
        .expect("expected an ssh error panel tab to render");
}

#[gpui::test]
fn ssh_connect_clears_sessions_sidebar_connecting_state(cx: &mut gpui::TestAppContext) {
    cx.update(|app| {
        gpui_component::init(app);
        menubar::init(app);
        gpui_term::init(app);
        gpui_dock::init(app);
        app.set_global(TermuaAppState::default());
    });

    let tmp_dir = std::env::temp_dir().join(format!(
        "termua-ssh-sidebar-connecting-cleared-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&tmp_dir).unwrap();
    let db_path = tmp_dir.join("termua").join("termua.db");
    let _guard = crate::store::tests::override_termua_db_path(db_path);

    let session_id = crate::store::save_ssh_session_config(
        "ssh",
        "prod",
        crate::settings::TerminalBackend::Wezterm,
        "example.com",
        22,
        "xterm-256color",
        "UTF-8",
    )
    .unwrap();

    let failing_builder: SshTerminalBuilderFn = Arc::new(|_backend, _env, _opts| {
        std::thread::sleep(Duration::from_millis(80));
        Err(anyhow::anyhow!("simulated ssh connect failure"))
    });

    let (view, window_cx) = cx.add_window_view(|window, cx| {
        TermuaWindow::new_with_ssh_terminal_builder(window, failing_builder, cx)
    });

    let sidebar = window_cx.update(|_window, cx| view.read(cx).sessions_sidebar.clone());

    window_cx.update(|window, cx| {
        view.update(cx, |this, cx| {
            this.sessions_sidebar.update(cx, |sidebar, cx| {
                sidebar.set_connecting(session_id, true, cx);
            });
            this.open_session_by_id(session_id, window, cx);
        });
    });

    assert!(
        window_cx.update(|_window, cx| sidebar.read(cx).is_connecting(session_id)),
        "expected connecting state to be set while ssh handshake is in-flight"
    );

    // Allow the background handshake to complete and for the UI thread to process the result.
    for _ in 0..30 {
        let view_for_draw = view.clone();
        window_cx.draw(
            gpui::point(gpui::px(0.), gpui::px(0.)),
            gpui::size(
                gpui::AvailableSpace::Definite(gpui::px(900.)),
                gpui::AvailableSpace::Definite(gpui::px(600.)),
            ),
            move |_, _| div().size_full().child(view_for_draw),
        );
        window_cx.run_until_parked();

        if window_cx.update(|_window, cx| !sidebar.read(cx).is_connecting(session_id)) {
            break;
        }
    }

    assert!(
        window_cx.update(|_window, cx| !sidebar.read(cx).is_connecting(session_id)),
        "expected connecting state to be cleared once the ssh handshake completes"
    );
}

#[gpui::test]
fn recorder_terminal_view_disables_context_menu(cx: &mut gpui::TestAppContext) {
    cx.update(|app| {
        gpui_component::init(app);
        gpui_term::init(app);
    });

    let cx = cx.add_empty_window();
    cx.update(|window, cx| {
        let active = Arc::new(AtomicBool::new(false));
        let term = cx.new(|_| {
            Terminal::new(
                TerminalType::WezTerm,
                Box::new(FakeBackend::new(Arc::clone(&active))),
            )
        });
        let terminal_view =
            cx.new(|cx| TerminalView::new_with_context_menu(term, window, cx, false));
        assert!(!terminal_view.read(cx).context_menu_enabled());
    });
}

#[gpui::test]
fn main_window_renders_sessions_sidebar_by_default(cx: &mut gpui::TestAppContext) {
    cx.update(|app| {
        gpui_component::init(app);
        menubar::init(app);
        gpui_term::init(app);
        gpui_dock::init(app);
        app.set_global(TermuaAppState::default());
        app.set_global(lock_screen::LockState::new_for_test(Duration::from_secs(
            60,
        )));
        app.set_global(notification::NotifyState::default());
    });

    let window = cx.add_empty_window();
    window.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(900.)),
            gpui::AvailableSpace::Definite(gpui::px(600.)),
        ),
        |window, app| {
            let view = app.new(|cx| TermuaWindow::new(window, cx));
            div().size_full().child(view)
        },
    );
    window.run_until_parked();

    assert!(
        window.debug_bounds("termua-sessions-sidebar").is_some(),
        "sessions sidebar should render in the main window"
    );
}

#[gpui::test]
fn main_window_renders_right_sidebar_when_enabled(cx: &mut gpui::TestAppContext) {
    cx.update(|app| {
        gpui_component::init(app);
        menubar::init(app);
        gpui_term::init(app);
        gpui_dock::init(app);
        app.set_global(TermuaAppState::default());
        app.set_global(lock_screen::LockState::new_for_test(Duration::from_secs(
            60,
        )));
        app.set_global(notification::NotifyState::default());
        let mut right = crate::right_sidebar::RightSidebarState::default();
        right.visible = true;
        app.set_global(right);
    });

    let window = cx.add_empty_window();
    window.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(900.)),
            gpui::AvailableSpace::Definite(gpui::px(600.)),
        ),
        |window, app| {
            let view = app.new(|cx| TermuaWindow::new(window, cx));
            div().size_full().child(view)
        },
    );
    window.run_until_parked();

    assert!(
        window.debug_bounds("termua-right-sidebar").is_some(),
        "right sidebar should render when enabled"
    );
}

#[gpui::test]
fn main_window_renders_lock_overlay_when_locked(cx: &mut gpui::TestAppContext) {
    cx.update(|app| {
        gpui_component::init(app);
        menubar::init(app);
        gpui_term::init(app);
        gpui_dock::init(app);
        app.set_global(TermuaAppState::default());
        app.set_global(lock_screen::LockState::new_for_test(Duration::from_secs(
            60,
        )));
        app.set_global(notification::NotifyState::default());
    });

    let window = cx.add_empty_window();
    window.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(900.)),
            gpui::AvailableSpace::Definite(gpui::px(600.)),
        ),
        |window, app| {
            let view = app.new(|cx| TermuaWindow::new(window, cx));
            app.global_mut::<lock_screen::LockState>()
                .force_lock_for_test();
            div().size_full().child(view)
        },
    );
    window.run_until_parked();

    assert!(window.debug_bounds("termua-lock-overlay").is_some());
    assert!(
        window.debug_bounds("termua-lock-drag-overlay").is_some(),
        "expected a drag overlay so the window remains movable while locked"
    );
    assert!(window.debug_bounds("termua-lock-password-input").is_some());
}

#[gpui::test]
fn sftp_events_are_recorded_in_message_center(cx: &mut gpui::TestAppContext) {
    use gpui_term::Terminal;

    cx.update(|app| {
        gpui_component::init(app);
        menubar::init(app);
        gpui_term::init(app);
        gpui_dock::init(app);
        app.set_global(TermuaAppState::default());
        app.set_global(lock_screen::LockState::new_for_test(Duration::from_secs(
            60,
        )));
        app.set_global(notification::NotifyState::default());
    });

    let (view, window_cx) = cx.add_window_view(|window, cx| TermuaWindow::new(window, cx));
    let view_for_draw = view.clone();

    // Create a terminal + terminal view that doesn't require a real PTY, and mount it so
    // `subscribe_in` delivers events through the window's event loop.
    let (terminal, terminal_view_for_draw) = window_cx.update(|window, app| {
        let recording_active = Arc::new(AtomicBool::new(false));
        let terminal = app.new(|_cx| {
            Terminal::new(
                TerminalType::WezTerm,
                Box::new(FakeBackend::new(recording_active.clone())),
            )
        });
        let terminal_view = app.new(|cx| TerminalView::new(terminal.clone(), window, cx));
        (terminal, terminal_view)
    });

    window_cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(900.)),
            gpui::AvailableSpace::Definite(gpui::px(600.)),
        ),
        move |_, _| {
            div()
                .size_full()
                .child(view_for_draw)
                // Render alongside the main window so the terminal is associated with this
                // window.
                .child(terminal_view_for_draw)
        },
    );
    window_cx.run_until_parked();

    window_cx.update(|window, app| {
        view.update(app, |this, cx| {
            this.subscribe_terminal_events_for_messages(
                terminal.clone(),
                0,
                "test".into(),
                window,
                cx,
            );
        });

        terminal.update(app, |_term, cx| {
            cx.emit(TerminalEvent::SftpUploadFinished {
                files: vec![("a.txt".to_string(), 1)],
                total_bytes: 1,
            });
        });
    });
    window_cx.run_until_parked();

    let recorded = window_cx.update(|_window, app| {
        app.global::<notification::NotifyState>()
            .messages
            .iter()
            .any(|m| m.message.as_ref().contains("Upload via SFTP complete"))
    });
    assert!(recorded, "expected SFTP message to be recorded");
}

#[gpui::test]
fn terminal_toast_events_are_recorded_in_message_center(cx: &mut gpui::TestAppContext) {
    use std::{cell::RefCell, rc::Rc};

    use gpui_term::Terminal;

    cx.update(|app| {
        gpui_component::init(app);
        menubar::init(app);
        gpui_term::init(app);
        gpui_dock::init(app);
        app.set_global(TermuaAppState::default());
        app.set_global(lock_screen::LockState::new_for_test(Duration::from_secs(
            60,
        )));
        app.set_global(notification::NotifyState::default());
    });

    let termua_slot: Rc<RefCell<Option<gpui::Entity<TermuaWindow>>>> = Rc::new(RefCell::new(None));
    let slot_for_root = termua_slot.clone();

    let (root, window_cx) = cx.add_window_view(|window, cx| {
        let view = cx.new(|cx| TermuaWindow::new(window, cx));
        *slot_for_root.borrow_mut() = Some(view.clone());
        gpui_component::Root::new(view, window, cx)
    });
    let termua = termua_slot
        .borrow()
        .as_ref()
        .expect("expected TermuaWindow view to be captured")
        .clone();

    let (terminal, terminal_view_for_draw) = window_cx.update(|window, app| {
        let recording_active = Arc::new(AtomicBool::new(false));
        let terminal = app.new(|_cx| {
            Terminal::new(
                TerminalType::WezTerm,
                Box::new(FakeBackend::new(recording_active.clone())),
            )
        });
        let terminal_view = app.new(|cx| TerminalView::new(terminal.clone(), window, cx));
        (terminal, terminal_view)
    });

    window_cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(900.)),
            gpui::AvailableSpace::Definite(gpui::px(600.)),
        ),
        move |_, _| div().size_full().child(root).child(terminal_view_for_draw),
    );
    window_cx.run_until_parked();

    window_cx.update(|window, app| {
        termua.update(app, |this, cx| {
            this.subscribe_terminal_events_for_messages(
                terminal.clone(),
                0,
                "test".into(),
                window,
                cx,
            );
        });

        terminal.update(app, |_term, cx| {
            cx.emit(TerminalEvent::Toast {
                level: gpui::PromptLevel::Warning,
                title: "Upload failed".to_string(),
                detail: Some("demo.txt: permission denied".to_string()),
            });
        });
    });
    window_cx.run_until_parked();

    let recorded = window_cx.update(|_window, app| {
        app.global::<notification::NotifyState>()
            .messages
            .iter()
            .any(|m| {
                m.message
                    .as_ref()
                    .contains("Upload failed\ndemo.txt: permission denied")
            })
    });
    assert!(recorded, "expected terminal toast to be recorded");

    window_cx.update(|window, app| {
        let root = gpui_component::Root::read(window, app);
        let notifications = root.notification.read(app).notifications();
        assert!(
            !notifications.is_empty(),
            "expected terminal toast to produce a popup notification"
        );
    });
}

#[gpui::test]
fn sftp_upload_per_file_progress_creates_multiple_transfer_tasks(cx: &mut gpui::TestAppContext) {
    use gpui_term::Terminal;
    use gpui_transfer::TransferCenterState;

    cx.update(|app| {
        gpui_component::init(app);
        menubar::init(app);
        gpui_term::init(app);
        gpui_dock::init(app);
        app.set_global(TermuaAppState::default());
        app.set_global(lock_screen::LockState::new_for_test(Duration::from_secs(
            60,
        )));
        app.set_global(notification::NotifyState::default());
    });

    let (view, window_cx) = cx.add_window_view(|window, cx| TermuaWindow::new(window, cx));
    let view_for_draw = view.clone();

    let (terminal, terminal_view_for_draw) = window_cx.update(|window, app| {
        let recording_active = Arc::new(AtomicBool::new(false));
        let terminal = app.new(|_cx| {
            Terminal::new(
                TerminalType::WezTerm,
                Box::new(FakeBackend::new(recording_active.clone())),
            )
        });
        let terminal_view = app.new(|cx| TerminalView::new(terminal.clone(), window, cx));
        (terminal, terminal_view)
    });

    window_cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(900.)),
            gpui::AvailableSpace::Definite(gpui::px(600.)),
        ),
        move |_, _| {
            div()
                .size_full()
                .child(view_for_draw)
                .child(terminal_view_for_draw)
        },
    );
    window_cx.run_until_parked();

    let cancel_a = Arc::new(AtomicBool::new(false));
    let cancel_b = Arc::new(AtomicBool::new(false));

    window_cx.update(|window, app| {
        view.update(app, |this, cx| {
            this.subscribe_terminal_events_for_messages(
                terminal.clone(),
                0,
                "test".into(),
                window,
                cx,
            );
        });

        terminal.update(app, |_term, cx| {
            cx.emit(TerminalEvent::SftpUploadFileProgress {
                transfer_id: 1,
                file_index: 0,
                file: "a.txt".to_string(),
                sent: 1,
                total: 10,
                cancel: Arc::clone(&cancel_a),
            });
            cx.emit(TerminalEvent::SftpUploadFileProgress {
                transfer_id: 1,
                file_index: 1,
                file: "b.bin".to_string(),
                sent: 2,
                total: 20,
                cancel: Arc::clone(&cancel_b),
            });
        });
    });
    window_cx.run_until_parked();

    let tasks = window_cx.update(|_window, app| app.global::<TransferCenterState>().tasks_sorted());
    assert_eq!(
        tasks.len(),
        2,
        "expected one TransferTask per uploaded file"
    );

    let mut by_id = std::collections::HashMap::new();
    for task in tasks {
        by_id.insert(task.id.clone(), task);
    }

    let a = by_id.get("sftp-upload-0-1-0").expect("expected a.txt task");
    assert!(
        a.cancel.as_ref().is_some_and(|t| Arc::ptr_eq(t, &cancel_a)),
        "expected a.txt task to carry the cancel token"
    );

    let b = by_id.get("sftp-upload-0-1-1").expect("expected b.bin task");
    assert!(
        b.cancel.as_ref().is_some_and(|t| Arc::ptr_eq(t, &cancel_b)),
        "expected b.bin task to carry the cancel token"
    );
}

#[gpui::test]
fn main_window_pressing_enter_unlocks(cx: &mut gpui::TestAppContext) {
    use std::sync::Arc;

    use gpui_component::WindowExt;

    struct FakeAuthenticator;

    impl lock_screen::Authenticator for FakeAuthenticator {
        fn verify_password(&self, password: &str) -> anyhow::Result<bool> {
            Ok(password == "pw")
        }
    }

    cx.update(|app| {
        gpui_component::init(app);
        menubar::init(app);
        gpui_term::init(app);
        gpui_dock::init(app);
        app.set_global(TermuaAppState::default());
        app.set_global(lock_screen::LockState::new_for_test_with_auth(
            Duration::from_secs(60),
            Arc::new(FakeAuthenticator),
        ));
    });

    let (root, window_cx) = cx.add_window_view(|window, cx| {
        let view = cx.new(|cx| TermuaWindow::new(window, cx));
        gpui_component::Root::new(view, window, cx)
    });

    window_cx.update(|_window, app| {
        app.global_mut::<lock_screen::LockState>()
            .force_lock_for_test();
    });
    window_cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(900.)),
            gpui::AvailableSpace::Definite(gpui::px(600.)),
        ),
        move |_, _| div().size_full().child(root),
    );
    window_cx.run_until_parked();

    let input_bounds = window_cx
        .debug_bounds("termua-lock-password-input")
        .expect("lock password input should exist");
    window_cx.simulate_click(input_bounds.center(), gpui::Modifiers::none());
    window_cx.run_until_parked();

    // Type password and hit Enter.
    window_cx.update(|window, app| {
        let Some(input) = window.focused_input(app) else {
            panic!("expected lock password input to be focused");
        };
        let input: gpui::Entity<InputState> = input;
        input.update(app, |state, cx| state.set_value("pw", window, cx));
    });
    window_cx.run_until_parked();
    window_cx.simulate_keystrokes("enter");
    window_cx.run_until_parked();

    assert!(
        window_cx.update(|_window, app| !app.global::<lock_screen::LockState>().locked()),
        "expected Enter to attempt unlock and clear lock state"
    );
}

#[gpui::test]
fn main_window_incorrect_password_clears_lock_input(cx: &mut gpui::TestAppContext) {
    use std::sync::Arc;

    use gpui_component::WindowExt;

    struct FakeAuthenticator;

    impl lock_screen::Authenticator for FakeAuthenticator {
        fn verify_password(&self, password: &str) -> anyhow::Result<bool> {
            Ok(password == "pw")
        }
    }

    cx.update(|app| {
        gpui_component::init(app);
        menubar::init(app);
        gpui_term::init(app);
        gpui_dock::init(app);
        app.set_global(TermuaAppState::default());
        app.set_global(lock_screen::LockState::new_for_test_with_auth(
            Duration::from_secs(60),
            Arc::new(FakeAuthenticator),
        ));
    });

    let (root, window_cx) = cx.add_window_view(|window, cx| {
        let view = cx.new(|cx| TermuaWindow::new(window, cx));
        gpui_component::Root::new(view, window, cx)
    });

    window_cx.update(|_window, app| {
        app.global_mut::<lock_screen::LockState>()
            .force_lock_for_test();
    });
    window_cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(900.)),
            gpui::AvailableSpace::Definite(gpui::px(600.)),
        ),
        move |_, _| div().size_full().child(root),
    );
    window_cx.run_until_parked();

    let input_bounds = window_cx
        .debug_bounds("termua-lock-password-input")
        .expect("lock password input should exist");
    window_cx.simulate_click(input_bounds.center(), gpui::Modifiers::none());
    window_cx.run_until_parked();

    window_cx.update(|window, app| {
        let Some(input) = window.focused_input(app) else {
            panic!("expected lock password input to be focused");
        };
        let input: gpui::Entity<InputState> = input;
        input.update(app, |state, cx| state.set_value("bad", window, cx));
    });
    window_cx.run_until_parked();
    window_cx.simulate_keystrokes("enter");
    window_cx.run_until_parked();

    assert!(
        window_cx.update(|_window, app| app.global::<lock_screen::LockState>().locked()),
        "sanity: incorrect password should keep the app locked"
    );

    let value = window_cx.update(|window, app| {
        let Some(input) = window.focused_input(app) else {
            panic!("expected lock password input to still be focused");
        };
        let input: gpui::Entity<InputState> = input;
        input.read(app).value().to_string()
    });
    assert_eq!(
        value, "",
        "expected input to be cleared on incorrect password"
    );
}

#[gpui::test]
fn main_window_focuses_lock_input_on_lock(cx: &mut gpui::TestAppContext) {
    use gpui_component::WindowExt;

    cx.update(|app| {
        gpui_component::init(app);
        menubar::init(app);
        gpui_term::init(app);
        gpui_dock::init(app);
        app.set_global(TermuaAppState::default());
        app.set_global(lock_screen::LockState::new_for_test(Duration::from_secs(
            60,
        )));
    });

    let (root, window_cx) = cx.add_window_view(|window, cx| {
        let view = cx.new(|cx| TermuaWindow::new(window, cx));
        gpui_component::Root::new(view, window, cx)
    });

    window_cx.update(|_window, app| {
        app.global_mut::<lock_screen::LockState>()
            .force_lock_for_test();
    });

    window_cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(900.)),
            gpui::AvailableSpace::Definite(gpui::px(600.)),
        ),
        move |_, _| div().size_full().child(root),
    );
    window_cx.run_until_parked();

    assert!(
        window_cx.update(|window, app| window.focused_input(app).is_some()),
        "expected lock password input to be focused"
    );
}

#[gpui::test]
fn main_window_lock_password_input_accepts_text(cx: &mut gpui::TestAppContext) {
    use gpui_component::WindowExt;

    cx.update(|app| {
        gpui_component::init(app);
        menubar::init(app);
        gpui_term::init(app);
        gpui_dock::init(app);
        app.set_global(TermuaAppState::default());
        app.set_global(lock_screen::LockState::new_for_test(Duration::from_secs(
            60,
        )));
    });

    let (root, window_cx) = cx.add_window_view(|window, cx| {
        let view = cx.new(|cx| TermuaWindow::new(window, cx));
        gpui_component::Root::new(view, window, cx)
    });

    window_cx.update(|_window, app| {
        app.global_mut::<lock_screen::LockState>()
            .force_lock_for_test();
    });

    window_cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(900.)),
            gpui::AvailableSpace::Definite(gpui::px(600.)),
        ),
        move |_, _| div().size_full().child(root),
    );
    window_cx.run_until_parked();

    let input_bounds = window_cx
        .debug_bounds("termua-lock-password-input")
        .expect("lock password input should exist");
    window_cx.simulate_click(input_bounds.center(), gpui::Modifiers::none());
    window_cx.run_until_parked();

    window_cx.simulate_input("pw");

    let value = window_cx.update(|window, app| {
        let Some(input) = window.focused_input(app) else {
            panic!("expected lock password input to be focused");
        };
        let input: gpui::Entity<InputState> = input;
        input.read(app).value().to_string()
    });

    assert_eq!(value, "pw");
}

#[gpui::test]
fn sessions_sidebar_visibility_can_be_toggled(cx: &mut gpui::TestAppContext) {
    cx.update(|app| {
        gpui_component::init(app);
        menubar::init(app);
        gpui_term::init(app);
        gpui_dock::init(app);
        app.set_global(TermuaAppState::default());
        app.set_global(lock_screen::LockState::new_for_test(Duration::from_secs(
            60,
        )));
    });

    {
        let window = cx.add_empty_window();
        window.draw(
            gpui::point(gpui::px(0.), gpui::px(0.)),
            gpui::size(
                gpui::AvailableSpace::Definite(gpui::px(900.)),
                gpui::AvailableSpace::Definite(gpui::px(600.)),
            ),
            |window, app| {
                let view = app.new(|cx| TermuaWindow::new(window, cx));
                div().size_full().child(view)
            },
        );
        window.run_until_parked();
        assert!(window.debug_bounds("termua-sessions-sidebar").is_some());
    }

    cx.update(|app| crate::menu::toggle_sessions_sidebar(&ToggleSessionsSidebar, app));

    let window = cx.add_empty_window();
    window.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(900.)),
            gpui::AvailableSpace::Definite(gpui::px(600.)),
        ),
        |window, app| {
            let view = app.new(|cx| TermuaWindow::new(window, cx));
            div().size_full().child(view)
        },
    );
    window.run_until_parked();
    assert!(window.debug_bounds("termua-sessions-sidebar").is_none());
}

#[gpui::test]
fn sessions_sidebar_width_can_be_resized_by_dragging_splitter(cx: &mut gpui::TestAppContext) {
    cx.update(|app| {
        gpui_component::init(app);
        menubar::init(app);
        gpui_term::init(app);
        gpui_dock::init(app);
        let mut state = TermuaAppState::default();
        state.sessions_sidebar_width = gpui::px(360.0);
        app.set_global(state);
    });

    let (view, cx) = cx.add_window_view(|window, cx| TermuaWindow::new(window, cx));

    cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(900.)),
            gpui::AvailableSpace::Definite(gpui::px(600.)),
        ),
        move |_, _| div().size_full().child(view),
    );
    cx.run_until_parked();

    let before = cx
        .debug_bounds("termua-sessions-sidebar")
        .expect("expected sessions sidebar to render");

    let handle = cx
        .debug_bounds("gpui-dock-resize-handle-left")
        .expect("expected a dock-style resize handle for the sessions sidebar");
    let start = gpui::point(handle.center().x, before.center().y);
    let end = gpui::point(start.x + gpui::px(80.), start.y);

    cx.simulate_mouse_down(start, gpui::MouseButton::Left, gpui::Modifiers::none());
    // gpui-component's resize handle uses the drag system; a tiny initial move helps ensure the
    // drag session starts before we issue the "real" move.
    let mid = gpui::point(start.x + gpui::px(1.), start.y);
    cx.simulate_event(gpui::MouseMoveEvent {
        position: mid,
        pressed_button: Some(gpui::MouseButton::Left),
        modifiers: gpui::Modifiers::none(),
    });
    cx.run_until_parked();
    cx.simulate_event(gpui::MouseMoveEvent {
        position: end,
        pressed_button: Some(gpui::MouseButton::Left),
        modifiers: gpui::Modifiers::none(),
    });
    cx.run_until_parked();
    cx.simulate_mouse_up(end, gpui::MouseButton::Left, gpui::Modifiers::none());
    cx.run_until_parked();

    let after = cx
        .debug_bounds("termua-sessions-sidebar")
        .expect("expected sessions sidebar to still render");

    assert!(
        after.size.width > before.size.width,
        "dragging the splitter should increase the sidebar width"
    );
}

#[gpui::test]
fn sessions_sidebar_width_is_clamped_to_min_width(cx: &mut gpui::TestAppContext) {
    cx.update(|app| {
        gpui_component::init(app);
        menubar::init(app);
        gpui_term::init(app);
        gpui_dock::init(app);
        let mut state = TermuaAppState::default();
        state.sessions_sidebar_width = gpui::px(360.0);
        app.set_global(state);
    });

    let (view, cx) = cx.add_window_view(|window, cx| TermuaWindow::new(window, cx));

    cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(900.)),
            gpui::AvailableSpace::Definite(gpui::px(600.)),
        ),
        move |_, _| div().size_full().child(view),
    );
    cx.run_until_parked();

    let before = cx
        .debug_bounds("termua-sessions-sidebar")
        .expect("expected sessions sidebar to render");
    let handle = cx
        .debug_bounds("gpui-dock-resize-handle-left")
        .expect("expected a dock-style resize handle for the sessions sidebar");

    // Drag left far enough that we'd go below the desired minimum width if unclamped.
    let start = gpui::point(handle.center().x, before.center().y);
    let end = gpui::point(start.x - gpui::px(500.), start.y);

    cx.simulate_mouse_down(start, gpui::MouseButton::Left, gpui::Modifiers::none());
    let mid = gpui::point(start.x - gpui::px(1.), start.y);
    cx.simulate_event(gpui::MouseMoveEvent {
        position: mid,
        pressed_button: Some(gpui::MouseButton::Left),
        modifiers: gpui::Modifiers::none(),
    });
    cx.run_until_parked();
    cx.simulate_event(gpui::MouseMoveEvent {
        position: end,
        pressed_button: Some(gpui::MouseButton::Left),
        modifiers: gpui::Modifiers::none(),
    });
    cx.run_until_parked();
    cx.simulate_mouse_up(end, gpui::MouseButton::Left, gpui::Modifiers::none());
    cx.run_until_parked();

    let after = cx
        .debug_bounds("termua-sessions-sidebar")
        .expect("expected sessions sidebar to still render");

    assert!(
        after.size.width >= gpui::px(220.0),
        "expected sessions sidebar width to be clamped to >= 220px, got {:?}",
        after.size.width
    );
    assert!(
        after.size.width <= before.size.width,
        "expected dragging left to not increase width"
    );
}

#[gpui::test]
fn right_sidebar_width_is_clamped_to_min_width(cx: &mut gpui::TestAppContext) {
    cx.update(|app| {
        gpui_component::init(app);
        menubar::init(app);
        gpui_term::init(app);
        gpui_dock::init(app);
        app.set_global(TermuaAppState::default());
        let mut right = crate::right_sidebar::RightSidebarState::default();
        right.visible = true;
        right.width = gpui::px(360.0);
        app.set_global(right);
    });

    let (view, cx) = cx.add_window_view(|window, cx| TermuaWindow::new(window, cx));

    cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(900.)),
            gpui::AvailableSpace::Definite(gpui::px(600.)),
        ),
        move |_, _| div().size_full().child(view),
    );
    cx.run_until_parked();

    let before = cx
        .debug_bounds("termua-right-sidebar")
        .expect("expected right sidebar to render");
    let handle = cx
        .debug_bounds("gpui-dock-resize-handle-right")
        .expect("expected a dock-style resize handle for the right sidebar");

    // Drag right far enough that we'd go below the desired minimum width if unclamped.
    let start = gpui::point(handle.center().x, before.center().y);
    let end = gpui::point(start.x + gpui::px(500.), start.y);

    cx.simulate_mouse_down(start, gpui::MouseButton::Left, gpui::Modifiers::none());
    let mid = gpui::point(start.x + gpui::px(1.), start.y);
    cx.simulate_event(gpui::MouseMoveEvent {
        position: mid,
        pressed_button: Some(gpui::MouseButton::Left),
        modifiers: gpui::Modifiers::none(),
    });
    cx.run_until_parked();
    cx.simulate_event(gpui::MouseMoveEvent {
        position: end,
        pressed_button: Some(gpui::MouseButton::Left),
        modifiers: gpui::Modifiers::none(),
    });
    cx.run_until_parked();
    cx.simulate_mouse_up(end, gpui::MouseButton::Left, gpui::Modifiers::none());
    cx.run_until_parked();

    let after = cx
        .debug_bounds("termua-right-sidebar")
        .expect("expected right sidebar to still render");

    assert!(
        after.size.width >= gpui::px(320.0),
        "expected right sidebar width to be clamped to >= 320px, got {:?}",
        after.size.width
    );
}

#[gpui::test]
fn dock_toggle_buttons_are_hidden_in_termua(cx: &mut gpui::TestAppContext) {
    use std::sync::Arc;

    use gpui::{App, Context, EventEmitter, FocusHandle, Focusable, Render, Window, div};
    use gpui_dock::{DockPlacement, Panel, PanelEvent, PanelView};

    struct DummyPanel {
        focus: FocusHandle,
    }

    impl DummyPanel {
        fn new(cx: &mut Context<Self>) -> Self {
            Self {
                focus: cx.focus_handle(),
            }
        }
    }

    impl EventEmitter<PanelEvent> for DummyPanel {}

    impl Focusable for DummyPanel {
        fn focus_handle(&self, _: &App) -> FocusHandle {
            self.focus.clone()
        }
    }

    impl Panel for DummyPanel {
        fn panel_name(&self) -> &'static str {
            "termua.test.dock_toggle_dummy_panel"
        }

        fn tab_name(&self, _: &App) -> Option<SharedString> {
            Some("Terminal".into())
        }

        fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().child("Terminal")
        }
    }

    impl Render for DummyPanel {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .debug_selector(|| "termua-test-terminal-tab".to_string())
        }
    }

    cx.update(|app| {
        gpui_component::init(app);
        menubar::init(app);
        gpui_term::init(app);
        gpui_dock::init(app);
        app.set_global(TermuaAppState::default());
        let mut right = crate::right_sidebar::RightSidebarState::default();
        right.visible = true;
        app.set_global(right);
    });

    let (view, window_cx) = cx.add_window_view(|window, cx| TermuaWindow::new(window, cx));

    window_cx.update(|window, cx| {
        let panel: Arc<dyn PanelView> = Arc::new(cx.new(DummyPanel::new));
        view.update(cx, |this, cx| {
            this.dock_area.update(cx, |dock, cx| {
                dock.add_panel(panel, DockPlacement::Center, None, window, cx);
            });
        });
    });

    window_cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(900.)),
            gpui::AvailableSpace::Definite(gpui::px(600.)),
        ),
        move |_, _| div().size_full().child(view),
    );
    window_cx.run_until_parked();

    assert!(
        window_cx.debug_bounds("gpui-dock-toggle-left").is_none(),
        "expected Termua to hide the left dock toggle button"
    );
    assert!(
        window_cx.debug_bounds("gpui-dock-toggle-right").is_none(),
        "expected Termua to hide the right dock toggle button"
    );
}

#[gpui::test]
fn fullscreen_with_terminal_tab_does_not_block_sessions_tree_clicks(cx: &mut gpui::TestAppContext) {
    use gpui::{
        App, Context, EventEmitter, FocusHandle, Focusable, IntoElement, Render, Window, div,
    };
    use gpui_dock::{DockPlacement, Panel, PanelEvent, PanelView};

    cx.update(|app| {
        gpui_component::init(app);
        menubar::init(app);
        gpui_term::init(app);
        gpui_dock::init(app);
        app.set_global(TermuaAppState::default());
    });

    let tmp_dir = std::env::temp_dir().join(format!(
        "termua-sessions-click-through-fullscreen-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&tmp_dir).unwrap();
    let db_path = tmp_dir.join("termua").join("termua.db");
    let _guard = crate::store::tests::override_termua_db_path(db_path);

    let session_id_1 = crate::store::save_ssh_session_password(
        "ssh",
        "prod-1",
        crate::settings::TerminalBackend::Wezterm,
        "example.com",
        22,
        "root",
        "pw123",
        "xterm-256color",
        "UTF-8",
    )
    .unwrap();
    let session_id_2 = crate::store::save_ssh_session_password(
        "ssh",
        "prod-2",
        crate::settings::TerminalBackend::Wezterm,
        "example.com",
        22,
        "root",
        "pw123",
        "xterm-256color",
        "UTF-8",
    )
    .unwrap();

    // Remove the stored password, so opening this session is a no-pty, in-app flow
    // (notification) instead of actually creating a terminal/pty.
    let _ = crate::keychain::delete_ssh_password(session_id_1);
    let _ = crate::keychain::delete_ssh_password(session_id_2);

    struct TerminalTabHarness {
        focus: FocusHandle,
        terminal_view: gpui::Entity<TerminalView>,
    }

    impl TerminalTabHarness {
        fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
            let active = Arc::new(AtomicBool::new(false));
            let term = cx.new(|_| {
                Terminal::new(
                    TerminalType::WezTerm,
                    Box::new(FakeBackend::new(Arc::clone(&active))),
                )
            });
            let terminal_view = cx.new(|cx| TerminalView::new(term, window, cx));
            Self {
                focus: terminal_view.read(cx).focus_handle.clone(),
                terminal_view,
            }
        }
    }

    impl EventEmitter<PanelEvent> for TerminalTabHarness {}

    impl Focusable for TerminalTabHarness {
        fn focus_handle(&self, _cx: &App) -> FocusHandle {
            self.focus.clone()
        }
    }

    impl Panel for TerminalTabHarness {
        fn panel_name(&self) -> &'static str {
            "termua.test.terminal_tab_harness"
        }

        fn tab_name(&self, _cx: &App) -> Option<SharedString> {
            Some("Terminal".into())
        }

        fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            "Terminal"
        }
    }

    impl Render for TerminalTabHarness {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .debug_selector(|| "termua-test-terminal-tab".to_string())
                .child(self.terminal_view.clone())
        }
    }

    struct RootHarness {
        termua: gpui::Entity<TermuaWindow>,
    }

    impl RootHarness {
        fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
            let termua = cx.new(|cx| TermuaWindow::new(window, cx));
            Self { termua }
        }
    }

    impl Render for RootHarness {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(self.termua.clone())
        }
    }

    let (root, window_cx) = cx.add_window_view(|window, cx| RootHarness::new(window, cx));

    // Render a few frames at "windowed" size.
    for _ in 0..2 {
        let root_for_draw = root.clone();
        window_cx.draw(
            gpui::point(gpui::px(0.), gpui::px(0.)),
            gpui::size(
                gpui::AvailableSpace::Definite(gpui::px(900.)),
                gpui::AvailableSpace::Definite(gpui::px(600.)),
            ),
            move |_, _| div().size_full().child(root_for_draw),
        );
        window_cx.run_until_parked();
    }

    let row_selector_1: &'static str =
        Box::leak(format!("termua-sessions-session-item-{session_id_1}").into_boxed_str());
    let row_selector_2: &'static str =
        Box::leak(format!("termua-sessions-session-item-{session_id_2}").into_boxed_str());

    // Baseline: clicking the sessions tree selects a session.
    let row_1_bounds = window_cx
        .debug_bounds(row_selector_1)
        .expect("expected session row 1 to be debuggable");
    window_cx.simulate_event(gpui::MouseDownEvent {
        position: row_1_bounds.center(),
        modifiers: gpui::Modifiers::none(),
        button: gpui::MouseButton::Left,
        click_count: 1,
        first_mouse: false,
    });
    window_cx.simulate_event(gpui::MouseUpEvent {
        position: row_1_bounds.center(),
        modifiers: gpui::Modifiers::none(),
        button: gpui::MouseButton::Left,
        click_count: 1,
    });
    window_cx.run_until_parked();

    let selected_after_first_click = window_cx.update(|_, cx| {
        root.read(cx)
            .termua
            .read(cx)
            .sessions_sidebar
            .read(cx)
            .selected_item_id_for_test()
            .to_string()
    });
    assert_eq!(
        selected_after_first_click.as_str(),
        format!("session:ssh:{session_id_1}"),
        "expected clicking session row 1 to select it"
    );

    // Add a terminal tab (the bug report says the sessions tree becomes unclickable once a tab
    // exists, especially after fullscreen).
    window_cx.update(|window, cx| {
        let panel: Arc<dyn PanelView> = Arc::new(cx.new(|cx| TerminalTabHarness::new(window, cx)));
        root.update(cx, |this, cx| {
            this.termua.update(cx, |termua, cx| {
                termua.dock_area.update(cx, |dock, cx| {
                    dock.add_panel(panel, DockPlacement::Center, None, window, cx);
                });
            });
        });
    });

    for _ in 0..2 {
        let root_for_draw = root.clone();
        window_cx.draw(
            gpui::point(gpui::px(0.), gpui::px(0.)),
            gpui::size(
                gpui::AvailableSpace::Definite(gpui::px(900.)),
                gpui::AvailableSpace::Definite(gpui::px(600.)),
            ),
            move |_, _| div().size_full().child(root_for_draw),
        );
        window_cx.run_until_parked();
    }

    // Simulate a fullscreen transition after creating a tab.
    for _ in 0..2 {
        let root_for_draw = root.clone();
        window_cx.draw(
            gpui::point(gpui::px(0.), gpui::px(0.)),
            gpui::size(
                gpui::AvailableSpace::Definite(gpui::px(2560.)),
                gpui::AvailableSpace::Definite(gpui::px(1600.)),
            ),
            move |_, _| div().size_full().child(root_for_draw),
        );
        window_cx.run_until_parked();
    }

    let row_2_bounds = window_cx
        .debug_bounds(row_selector_2)
        .expect("expected session row 2 to be debuggable after adding a terminal tab");

    let sessions_sidebar = window_cx
        .debug_bounds("termua-sessions-sidebar")
        .expect("expected sessions sidebar to render");
    let terminal_tab = window_cx
        .debug_bounds("termua-test-terminal-tab")
        .expect("expected terminal tab panel to render");
    let sidebar_right = sessions_sidebar.origin.x + sessions_sidebar.size.width;
    assert!(
        terminal_tab.origin.x >= sidebar_right,
        "expected terminal tab bounds to start to the right of the sessions sidebar; \
         sidebar_right={sidebar_right:?}, terminal_tab_origin_x={:?}",
        terminal_tab.origin.x
    );

    window_cx.simulate_event(gpui::MouseDownEvent {
        position: row_2_bounds.center(),
        modifiers: gpui::Modifiers::none(),
        button: gpui::MouseButton::Left,
        click_count: 1,
        first_mouse: false,
    });
    window_cx.simulate_event(gpui::MouseUpEvent {
        position: row_2_bounds.center(),
        modifiers: gpui::Modifiers::none(),
        button: gpui::MouseButton::Left,
        click_count: 1,
    });
    window_cx.run_until_parked();

    let selected = window_cx.update(|_, cx| {
        root.read(cx)
            .termua
            .read(cx)
            .sessions_sidebar
            .read(cx)
            .selected_item_id_for_test()
            .to_string()
    });
    assert_eq!(
        selected.as_str(),
        format!("session:ssh:{session_id_2}"),
        "expected clicking the sessions tree row to update selection even after adding a terminal \
         tab"
    );
}

#[gpui::test]
fn dock_tab_move_buttons_render_when_tabs_overflow(cx: &mut gpui::TestAppContext) {
    use std::sync::Arc;

    use gpui::{App, Context, EventEmitter, FocusHandle, Focusable, Render, Window, div};
    use gpui_dock::{DockPlacement, Panel, PanelEvent, PanelView};

    struct DummyPanel {
        focus: FocusHandle,
        label: SharedString,
    }

    impl DummyPanel {
        fn new(label: impl Into<SharedString>, cx: &mut Context<Self>) -> Self {
            Self {
                focus: cx.focus_handle(),
                label: label.into(),
            }
        }
    }

    impl EventEmitter<PanelEvent> for DummyPanel {}

    impl Focusable for DummyPanel {
        fn focus_handle(&self, _: &App) -> FocusHandle {
            self.focus.clone()
        }
    }

    impl Panel for DummyPanel {
        fn panel_name(&self) -> &'static str {
            "termua.test.dummy_panel"
        }

        fn tab_name(&self, _: &App) -> Option<SharedString> {
            Some(self.label.clone())
        }

        fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().child(self.label.clone())
        }
    }

    impl Render for DummyPanel {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full()
        }
    }

    cx.update(|app| {
        gpui_component::init(app);
        menubar::init(app);
        gpui_term::init(app);
        gpui_dock::init(app);
        app.set_global(TermuaAppState::default());
    });

    let (view, window_cx) = cx.add_window_view(|window, cx| TermuaWindow::new(window, cx));

    // With the sessions sidebar visible, the dock area gets very little horizontal space.
    // Add enough tabs to guarantee overflow so the dock shows left/right navigation buttons.
    window_cx.update(|window, cx| {
        for ix in 0..24usize {
            let panel: Arc<dyn PanelView> =
                Arc::new(cx.new(|cx| {
                    DummyPanel::new(format!("Tab {ix} - This is a very long tab name"), cx)
                }));
            view.update(cx, |this, cx| {
                this.dock_area.update(cx, |dock, cx| {
                    dock.add_panel(panel.clone(), DockPlacement::Center, None, window, cx);
                });
            });
        }
    });

    // Tab overflow detection is updated asynchronously (via `window.defer` in TabPanel), so
    // render a few frames to let the scroll handle settle and for overflow controls to appear.
    for _ in 0..3 {
        let view_for_draw = view.clone();
        window_cx.draw(
            gpui::point(gpui::px(0.), gpui::px(0.)),
            gpui::size(
                gpui::AvailableSpace::Definite(gpui::px(520.)),
                gpui::AvailableSpace::Definite(gpui::px(360.)),
            ),
            move |_, _| div().size_full().child(view_for_draw),
        );
        window_cx.run_until_parked();
    }

    assert!(
        window_cx.debug_bounds("gpui-dock-tab-move-left").is_some(),
        "expected dock tab move-left button to render when tabs overflow"
    );
    assert!(
        window_cx.debug_bounds("gpui-dock-tab-move-right").is_some(),
        "expected dock tab move-right button to render when tabs overflow"
    );
}

#[gpui::test]
fn ssh_sessions_with_missing_password_show_a_notification(cx: &mut gpui::TestAppContext) {
    cx.update(|app| {
        gpui_component::init(app);
        menubar::init(app);
        gpui_term::init(app);
        gpui_dock::init(app);
        app.set_global(TermuaAppState::default());
    });

    let tmp_dir = std::env::temp_dir().join(format!(
        "termua-sessions-test-missing-password-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&tmp_dir).unwrap();
    let db_path = tmp_dir.join("termua").join("termua.db");
    let _guard = crate::store::tests::override_termua_db_path(db_path);

    let id = crate::store::save_ssh_session_password(
        "ssh",
        "prod",
        crate::settings::TerminalBackend::Wezterm,
        "example.com",
        22,
        "root",
        "pw123",
        "xterm-256color",
        "UTF-8",
    )
    .unwrap();

    // Remove the stored password, so opening this session should report a missing password
    // instead of silently doing nothing.
    let _ = crate::keychain::delete_ssh_password(id);

    let (root, cx) = cx.add_window_view(|window, cx| {
        let view = cx.new(|cx| TermuaWindow::new(window, cx));
        gpui_component::Root::new(view, window, cx)
    });

    cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(900.)),
            gpui::AvailableSpace::Definite(gpui::px(600.)),
        ),
        move |_, _| div().size_full().child(root),
    );
    cx.run_until_parked();

    let selector: &'static str =
        Box::leak(format!("termua-sessions-session-item-{id}").into_boxed_str());
    let row_bounds = cx
        .debug_bounds(selector)
        .expect("expected the ssh session row to be debuggable");

    cx.simulate_event(gpui::MouseDownEvent {
        position: row_bounds.center(),
        modifiers: gpui::Modifiers::none(),
        button: gpui::MouseButton::Left,
        click_count: 2,
        first_mouse: false,
    });
    cx.simulate_event(gpui::MouseUpEvent {
        position: row_bounds.center(),
        modifiers: gpui::Modifiers::none(),
        button: gpui::MouseButton::Left,
        click_count: 2,
    });

    cx.update(|window, app| {
        let root = gpui_component::Root::read(window, app);
        let notifications = root.notification.read(app).notifications();
        assert!(
            !notifications.is_empty(),
            "expected a notification when an ssh password session is missing its password"
        );
    });
}

struct FakeBackend {
    content: gpui_term::TerminalContent,
    recording_active: Arc<AtomicBool>,
}

impl FakeBackend {
    fn new(recording_active: Arc<AtomicBool>) -> Self {
        Self {
            content: gpui_term::TerminalContent::default(),
            recording_active,
        }
    }
}

impl TerminalBackend for FakeBackend {
    fn backend_name(&self) -> &'static str {
        "fake"
    }

    fn sync(&mut self, _window: &mut Window, _cx: &mut Context<Terminal>) {}

    fn last_content(&self) -> &gpui_term::TerminalContent {
        &self.content
    }

    fn matches(&self) -> &[RangeInclusive<gpui_term::GridPoint>] {
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

    fn select_matches(&mut self, _matches: &[RangeInclusive<gpui_term::GridPoint>]) {}

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

    fn cast_recording_active(&self) -> bool {
        self.recording_active.load(Ordering::SeqCst)
    }

    fn start_cast_recording(&mut self, _opts: gpui_term::CastRecordingOptions) -> gpui::Result<()> {
        self.recording_active.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn stop_cast_recording(&mut self) {
        self.recording_active.store(false, Ordering::SeqCst);
    }

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

    fn set_env(&mut self, _env: std::collections::HashMap<String, String>) {}

    fn sftp(&self) -> Option<wezterm_ssh::Sftp> {
        None
    }
}
