use std::sync::{Arc, Mutex};

use gpui::{
    AppContext, Context, Entity, IntoElement, ParentElement, Render, Styled, Subscription, Window,
    div,
};

use super::*;
use crate::store::{Session, SessionEnvVar, SessionType};

fn test_session_env(
    term: &str,
    charset: &str,
    colorterm: Option<&str>,
) -> Option<Vec<SessionEnvVar>> {
    let mut env = vec![
        SessionEnvVar {
            name: "TERM".to_string(),
            value: term.to_string(),
        },
        SessionEnvVar {
            name: "CHARSET".to_string(),
            value: charset.to_string(),
        },
    ];
    if let Some(colorterm) = colorterm {
        env.push(SessionEnvVar {
            name: "COLORTERM".to_string(),
            value: colorterm.to_string(),
        });
    }
    Some(env)
}

#[gpui::test]
fn folder_icons_toggle_with_expansion(cx: &mut gpui::TestAppContext) {
    cx.update(|app| {
        gpui_component::init(app);
    });

    let db_path = crate::store::tests::unique_test_db_path("sessions-sidebar-folder-icons");
    let _guard = crate::store::tests::override_termua_db_path(db_path);

    crate::store::save_local_session(
        "Group",
        "bash",
        crate::settings::TerminalBackend::Wezterm,
        "xterm-256color",
        "UTF-8",
    )
    .unwrap();

    struct Harness {
        sidebar: Entity<SessionsSidebarView>,
    }

    impl Harness {
        fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
            let sidebar = cx.new(|cx| SessionsSidebarView::new(window, cx));
            Self { sidebar }
        }
    }

    impl Render for Harness {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(self.sidebar.clone())
        }
    }

    let (harness, cx) = cx.add_window_view(Harness::new);

    cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(600.)),
            gpui::AvailableSpace::Definite(gpui::px(400.)),
        ),
        move |_, _| div().size_full().child(harness),
    );
    cx.run_until_parked();

    cx.debug_bounds("termua-sessions-folder-icon-open-Group")
        .expect("expected expanded folder rows to show the open folder icon");

    let row_bounds = cx
        .debug_bounds("termua-sessions-folder-row-Group")
        .expect("expected folder rows to be debuggable");
    cx.simulate_click(row_bounds.center(), gpui::Modifiers::none());
    cx.run_until_parked();

    cx.debug_bounds("termua-sessions-folder-icon-closed-Group")
        .expect("expected collapsed folder rows to show the closed folder icon");
}

#[gpui::test]
fn local_session_icon_is_debuggable(cx: &mut gpui::TestAppContext) {
    cx.update(|app| {
        gpui_component::init(app);
    });

    let db_path = crate::store::tests::unique_test_db_path("sessions-sidebar-local-icon");
    let _guard = crate::store::tests::override_termua_db_path(db_path);

    let session_id = crate::store::save_local_session(
        "local",
        "bash",
        crate::settings::TerminalBackend::Wezterm,
        "xterm-256color",
        "UTF-8",
    )
    .unwrap();

    struct Harness {
        sidebar: Entity<SessionsSidebarView>,
    }

    impl Harness {
        fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
            let sidebar = cx.new(|cx| SessionsSidebarView::new(window, cx));
            Self { sidebar }
        }
    }

    impl Render for Harness {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(self.sidebar.clone())
        }
    }

    let (harness, cx) = cx.add_window_view(Harness::new);
    cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(600.)),
            gpui::AvailableSpace::Definite(gpui::px(400.)),
        ),
        move |_, _| div().size_full().child(harness),
    );
    cx.run_until_parked();

    let selector: &'static str =
        Box::leak(format!("termua-sessions-session-icon-local-{session_id}").into_boxed_str());
    cx.debug_bounds(selector)
        .expect("expected local session icon to be debuggable");
}

#[gpui::test]
fn sessions_open_only_on_double_click(cx: &mut gpui::TestAppContext) {
    cx.update(|app| {
        gpui_component::init(app);
    });

    let db_path = crate::store::tests::unique_test_db_path("sessions-sidebar-double-click");
    let _guard = crate::store::tests::override_termua_db_path(db_path);

    let session_id = crate::store::save_local_session(
        "local",
        "bash",
        crate::settings::TerminalBackend::Wezterm,
        "xterm-256color",
        "UTF-8",
    )
    .unwrap();

    struct Harness {
        sidebar: Entity<SessionsSidebarView>,
        _sub: Subscription,
    }

    impl Harness {
        fn new(opened: Arc<Mutex<Vec<i64>>>, window: &mut Window, cx: &mut Context<Self>) -> Self {
            let sidebar = cx.new(|cx| SessionsSidebarView::new(window, cx));
            let sub = cx.subscribe_in(&sidebar, window, move |_, _, ev, _, _| {
                let SessionsSidebarEvent::OpenSession(id) = ev;
                opened.lock().unwrap().push(*id);
            });
            Self { sidebar, _sub: sub }
        }
    }

    impl Render for Harness {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(self.sidebar.clone())
        }
    }

    let opened = Arc::new(Mutex::new(Vec::new()));
    let opened_for_view = opened.clone();
    let (harness, cx) =
        cx.add_window_view(move |window, cx| Harness::new(opened_for_view, window, cx));
    let sidebar = cx.update(|_window, app| harness.read(app).sidebar.clone());

    cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(600.)),
            gpui::AvailableSpace::Definite(gpui::px(400.)),
        ),
        move |_, _| div().size_full().child(harness),
    );
    cx.run_until_parked();

    // Single click selects but does not open.
    cx.update(|_window, app| {
        sidebar.update(app, |this, cx| {
            this.handle_session_click(
                format!("session:local:{session_id}").into(),
                session_id,
                false,
                cx,
            );
        });
    });
    assert!(
        opened.lock().unwrap().is_empty(),
        "single click should not open a session"
    );

    // Double click opens.
    cx.update(|_window, app| {
        sidebar.update(app, |this, cx| {
            this.handle_session_click(
                format!("session:local:{session_id}").into(),
                session_id,
                true,
                cx,
            );
        });
    });
    cx.run_until_parked();

    assert_eq!(
        opened.lock().unwrap().as_slice(),
        &[session_id],
        "double click should open the selected session"
    );
}

#[gpui::test]
fn ssh_sessions_show_connecting_and_block_repeat_double_click(cx: &mut gpui::TestAppContext) {
    cx.update(|app| {
        gpui_component::init(app);
    });

    let db_path = crate::store::tests::unique_test_db_path("sessions-sidebar-connecting");
    let _guard = crate::store::tests::override_termua_db_path(db_path);

    let session_id = crate::store::save_ssh_session_password(
        "ssh",
        "prod",
        crate::settings::TerminalBackend::Wezterm,
        "example.com",
        22,
        "alice",
        "pw",
        "xterm-256color",
        "UTF-8",
    )
    .unwrap();

    struct Harness {
        sidebar: Entity<SessionsSidebarView>,
        _sub: Subscription,
    }

    impl Harness {
        fn new(
            opened: Arc<Mutex<Vec<i64>>>,
            sidebar_out: Arc<Mutex<Option<Entity<SessionsSidebarView>>>>,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) -> Self {
            let sidebar = cx.new(|cx| SessionsSidebarView::new(window, cx));
            *sidebar_out.lock().unwrap() = Some(sidebar.clone());

            let sub = cx.subscribe_in(&sidebar, window, move |_, _, ev, _, _| {
                let SessionsSidebarEvent::OpenSession(id) = ev;
                opened.lock().unwrap().push(*id);
            });
            Self { sidebar, _sub: sub }
        }
    }

    impl Render for Harness {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(self.sidebar.clone())
        }
    }

    let opened = Arc::new(Mutex::new(Vec::new()));
    let sidebar_out: Arc<Mutex<Option<Entity<SessionsSidebarView>>>> = Arc::new(Mutex::new(None));
    let opened_for_view = opened.clone();
    let sidebar_out_for_view = sidebar_out.clone();
    let (harness, window_cx) = cx.add_window_view(move |window, cx| {
        Harness::new(opened_for_view, sidebar_out_for_view, window, cx)
    });

    window_cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(600.)),
            gpui::AvailableSpace::Definite(gpui::px(400.)),
        ),
        move |_, _| div().size_full().child(harness),
    );
    window_cx.run_until_parked();

    let _sidebar = sidebar_out
        .lock()
        .unwrap()
        .clone()
        .expect("expected sidebar to be constructed");

    // First double click opens and marks as connecting.
    window_cx.update(|_window, app| {
        let sidebar = sidebar_out
            .lock()
            .unwrap()
            .clone()
            .expect("expected sidebar to be constructed");
        sidebar.update(app, |this, cx| {
            this.handle_session_click(
                format!("session:ssh:{session_id}").into(),
                session_id,
                true,
                cx,
            );
        });
    });
    window_cx.run_until_parked();

    let connecting_selector: &'static str =
        Box::leak(format!("termua-sessions-ssh-connecting-{session_id}").into_boxed_str());
    window_cx
        .debug_bounds(connecting_selector)
        .expect("expected connecting indicator to be visible for ssh sessions");

    assert_eq!(
        opened.lock().unwrap().as_slice(),
        &[session_id],
        "expected first double click to emit an open session event"
    );

    // Second double click should be blocked while connecting.
    window_cx.update(|_window, app| {
        let sidebar = sidebar_out
            .lock()
            .unwrap()
            .clone()
            .expect("expected sidebar to be constructed");
        sidebar.update(app, |this, cx| {
            this.handle_session_click(
                format!("session:ssh:{session_id}").into(),
                session_id,
                true,
                cx,
            );
        });
    });
    window_cx.run_until_parked();

    assert_eq!(
        opened.lock().unwrap().as_slice(),
        &[session_id],
        "expected connecting ssh sessions to not re-open on repeat double click"
    );
}

#[gpui::test]
fn sessions_can_be_deleted_via_right_click_menu(cx: &mut gpui::TestAppContext) {
    cx.update(|app| {
        gpui_component::init(app);
    });

    let db_path = crate::store::tests::unique_test_db_path("sessions-sidebar-delete");
    let _guard = crate::store::tests::override_termua_db_path(db_path);

    let session_id = crate::store::save_local_session(
        "local",
        "bash",
        crate::settings::TerminalBackend::Wezterm,
        "xterm-256color",
        "UTF-8",
    )
    .unwrap();
    let row_selector: &'static str =
        Box::leak(format!("termua-sessions-session-row-{session_id}").into_boxed_str());

    let (root, cx) = cx.add_window_view(|window, cx| {
        let sidebar = cx.new(|cx| SessionsSidebarView::new(window, cx));
        gpui_component::Root::new(sidebar, window, cx)
    });

    let sidebar = cx.update(|window, app| {
        root.read(app)
            .view()
            .clone()
            .downcast::<SessionsSidebarView>()
            .unwrap_or_else(|_| {
                panic!(
                    "expected sessions sidebar root view to downcast to SessionsSidebarView: {:?}",
                    window.window_handle()
                )
            })
    });

    let root_for_draw = root.clone();
    cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(600.)),
            gpui::AvailableSpace::Definite(gpui::px(400.)),
        ),
        move |_, _| div().size_full().child(root_for_draw),
    );
    cx.run_until_parked();

    cx.update(|_window, app| {
        sidebar.update(app, |this, cx| {
            this.handle_session_context_click(
                format!("session:local:{session_id}").into(),
                session_id,
                cx,
            );
            assert_eq!(this.hovered_session_id_for_test(), Some(session_id));
            assert_eq!(
                SessionsSidebarView::context_menu_items_for_hovered_session(
                    this.hovered_session_id_for_test()
                ),
                &[
                    super::render::SessionsSidebarContextMenuItem::Edit,
                    super::render::SessionsSidebarContextMenuItem::Delete,
                ]
            );
        });
    });

    assert!(
        crate::store::load_session(session_id).unwrap().is_some(),
        "expected preparing the context menu to not delete immediately"
    );

    cx.update(|window, app| {
        sidebar.update(app, |this, cx| {
            this.delete_session_by_id(session_id, window, cx);
        });
    });
    cx.run_until_parked();

    assert!(
        crate::store::load_session(session_id).unwrap().is_none(),
        "expected the session to be deleted from sqlite"
    );
    assert!(
        crate::store::load_all_sessions().unwrap().is_empty(),
        "expected sessions sqlite db to be empty after deletion"
    );

    // Render a fresh window to assert the row is truly gone (debug bounds map retains keys).
    let (root, cx) = cx.add_window_view(|window, cx| {
        let sidebar = cx.new(|cx| SessionsSidebarView::new(window, cx));
        gpui_component::Root::new(sidebar, window, cx)
    });

    cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(600.)),
            gpui::AvailableSpace::Definite(gpui::px(400.)),
        ),
        move |_, _| div().size_full().child(root),
    );
    cx.run_until_parked();

    assert!(
        cx.debug_bounds(row_selector).is_none(),
        "expected the deleted session row to be removed from the tree"
    );
}

#[test]
fn build_tree_items_filters_by_query_and_keeps_ancestors() {
    let sessions = vec![
        Session {
            id: 1,
            protocol: SessionType::Ssh,
            group_path: "ssh>prod".to_string(),
            label: "db".to_string(),
            backend: crate::settings::TerminalBackend::Wezterm,
            env: test_session_env("xterm", "UTF-8", None),
            ssh_host: Some("db.example.com".to_string()),
            ssh_port: Some(22),
            ssh_auth_type: None,
            ssh_user: None,
            ssh_credential_username: None,
            ssh_password: None,
            ssh_tcp_nodelay: false,
            ssh_tcp_keepalive: false,
            ssh_proxy_mode: None,
            ssh_proxy_command: None,
            ssh_proxy_workdir: None,
            ssh_proxy_env: None,
            ssh_proxy_jump: None,
            serial_port: None,
            serial_baud: None,
            serial_data_bits: None,
            serial_parity: None,
            serial_stop_bits: None,
            serial_flow_control: None,
        },
        Session {
            id: 2,
            protocol: SessionType::Ssh,
            group_path: "ssh>staging".to_string(),
            label: "api".to_string(),
            backend: crate::settings::TerminalBackend::Wezterm,
            env: test_session_env("xterm", "UTF-8", None),
            ssh_host: Some("api.example.com".to_string()),
            ssh_port: Some(22),
            ssh_auth_type: None,
            ssh_user: None,
            ssh_credential_username: None,
            ssh_password: None,
            ssh_tcp_nodelay: false,
            ssh_tcp_keepalive: false,
            ssh_proxy_mode: None,
            ssh_proxy_command: None,
            ssh_proxy_workdir: None,
            ssh_proxy_env: None,
            ssh_proxy_jump: None,
            serial_port: None,
            serial_baud: None,
            serial_data_bits: None,
            serial_parity: None,
            serial_stop_bits: None,
            serial_flow_control: None,
        },
    ];

    let items = tree::build_tree_items(&sessions, "db");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].label.as_ref(), "ssh");
    assert_eq!(items[0].children.len(), 1);
    assert_eq!(items[0].children[0].label.as_ref(), "prod");
    assert_eq!(items[0].children[0].children.len(), 1);
    assert_eq!(items[0].children[0].children[0].label.as_ref(), "db");

    let items = tree::build_tree_items(&sessions, "staging");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].label.as_ref(), "ssh");
    assert_eq!(items[0].children.len(), 1);
    assert_eq!(items[0].children[0].label.as_ref(), "staging");
    assert_eq!(items[0].children[0].children.len(), 1);
    assert_eq!(items[0].children[0].children[0].label.as_ref(), "api");

    let items = tree::build_tree_items(&sessions, "api.example.com");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].label.as_ref(), "ssh");
    assert_eq!(items[0].children.len(), 1);
    assert_eq!(items[0].children[0].label.as_ref(), "staging");
    assert_eq!(items[0].children[0].children.len(), 1);
    assert_eq!(items[0].children[0].children[0].label.as_ref(), "api");
}

#[gpui::test]
fn sessions_context_menu_includes_edit_item(cx: &mut gpui::TestAppContext) {
    cx.update(|app| {
        gpui_component::init(app);
    });

    let db_path = crate::store::tests::unique_test_db_path("sessions-sidebar-edit-menu");
    let _guard = crate::store::tests::override_termua_db_path(db_path);

    let session_id = crate::store::save_local_session(
        "local",
        "bash",
        crate::settings::TerminalBackend::Wezterm,
        "xterm-256color",
        "UTF-8",
    )
    .unwrap();

    assert_eq!(
        SessionsSidebarView::context_menu_items_for_hovered_session(Some(session_id)),
        &[
            super::render::SessionsSidebarContextMenuItem::Edit,
            super::render::SessionsSidebarContextMenuItem::Delete,
        ]
    );
}

#[gpui::test]
fn local_sessions_always_show_terminal_icon(cx: &mut gpui::TestAppContext) {
    cx.update(|app| {
        gpui_component::init(app);
    });

    let db_path = crate::store::tests::unique_test_db_path("sessions-sidebar-pwsh-icon");
    let _guard = crate::store::tests::override_termua_db_path(db_path);

    let session_id = crate::store::save_local_session(
        "local",
        "powershell",
        crate::settings::TerminalBackend::Wezterm,
        "xterm-256color",
        "UTF-8",
    )
    .unwrap();

    let (root, cx) = cx.add_window_view(|window, cx| {
        let sidebar = cx.new(|cx| SessionsSidebarView::new(window, cx));
        gpui_component::Root::new(sidebar, window, cx)
    });

    cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(600.)),
            gpui::AvailableSpace::Definite(gpui::px(400.)),
        ),
        move |_, _| div().size_full().child(root),
    );
    cx.run_until_parked();

    let icon_selector: &'static str =
        Box::leak(format!("termua-sessions-session-icon-local-{session_id}").into_boxed_str());
    cx.debug_bounds(icon_selector)
        .expect("expected local sessions to render the generic terminal icon");
}

#[gpui::test]
fn blank_area_right_click_shows_new_session_menu_item(cx: &mut gpui::TestAppContext) {
    cx.update(|app| {
        gpui_component::init(app);
    });

    let db_path = crate::store::tests::unique_test_db_path("sessions-sidebar-blank-new-session");
    let _guard = crate::store::tests::override_termua_db_path(db_path);

    let (root, cx) = cx.add_window_view(|window, cx| {
        let sidebar = cx.new(|cx| SessionsSidebarView::new(window, cx));
        gpui_component::Root::new(sidebar, window, cx)
    });

    let sidebar = cx.update(|_window, app| {
        root.read(app)
            .view()
            .clone()
            .downcast::<SessionsSidebarView>()
            .expect("expected sidebar root view")
    });

    let root_for_draw = root.clone();
    cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(600.)),
            gpui::AvailableSpace::Definite(gpui::px(400.)),
        ),
        move |_, _| div().size_full().child(root_for_draw),
    );
    cx.run_until_parked();

    cx.update(|_window, app| {
        sidebar.update(app, |this, cx| {
            this.handle_background_context_click(cx);
            assert_eq!(this.hovered_session_id_for_test(), None);
        });
    });

    assert_eq!(
        SessionsSidebarView::context_menu_items_for_hovered_session(None),
        &[super::render::SessionsSidebarContextMenuItem::NewSession]
    );
}

#[gpui::test]
fn folder_right_click_shows_new_session_menu_item(cx: &mut gpui::TestAppContext) {
    cx.update(|app| {
        gpui_component::init(app);
    });

    let db_path = crate::store::tests::unique_test_db_path("sessions-sidebar-folder-new-session");
    let _guard = crate::store::tests::override_termua_db_path(db_path);

    crate::store::save_local_session(
        "Group",
        "bash",
        crate::settings::TerminalBackend::Wezterm,
        "xterm-256color",
        "UTF-8",
    )
    .unwrap();

    let (root, cx) = cx.add_window_view(|window, cx| {
        let sidebar = cx.new(|cx| SessionsSidebarView::new(window, cx));
        gpui_component::Root::new(sidebar, window, cx)
    });

    let sidebar = cx.update(|_window, app| {
        root.read(app)
            .view()
            .clone()
            .downcast::<SessionsSidebarView>()
            .expect("expected sidebar root view")
    });

    let root_for_draw = root.clone();
    cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(600.)),
            gpui::AvailableSpace::Definite(gpui::px(400.)),
        ),
        move |_, _| div().size_full().child(root_for_draw),
    );
    cx.run_until_parked();

    cx.update(|_window, app| {
        sidebar.update(app, |this, cx| {
            this.handle_session_context_click("session:local:999".into(), 999, cx);
            assert_eq!(this.hovered_session_id_for_test(), Some(999));
            this.handle_background_context_click(cx);
            assert_eq!(this.hovered_session_id_for_test(), None);
        });
    });

    assert_eq!(
        SessionsSidebarView::context_menu_items_for_hovered_session(None),
        &[super::render::SessionsSidebarContextMenuItem::NewSession]
    );
}

#[gpui::test]
fn sidebar_shows_load_error_when_disk_sessions_cannot_be_parsed(cx: &mut gpui::TestAppContext) {
    cx.update(|app| {
        gpui_component::init(app);
    });

    let db_path = crate::store::tests::unique_test_db_path("sessions-sidebar-load-error");
    let _guard = crate::store::tests::override_termua_db_path(db_path.clone());

    let session_id = crate::store::save_local_session(
        "local",
        "bash",
        crate::settings::TerminalBackend::Wezterm,
        "xterm-256color",
        "UTF-8",
    )
    .unwrap();

    let conn = rusqlite::Connection::open(db_path).unwrap();
    conn.execute(
        "UPDATE sessions SET backend = 'alacritty2' WHERE id = ?1",
        rusqlite::params![session_id],
    )
    .unwrap();

    let (root, cx) = cx.add_window_view(|window, cx| {
        let sidebar = cx.new(|cx| SessionsSidebarView::new(window, cx));
        gpui_component::Root::new(sidebar, window, cx)
    });

    cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(600.)),
            gpui::AvailableSpace::Definite(gpui::px(240.)),
        ),
        move |_, _| div().size_full().child(root),
    );
    cx.run_until_parked();

    cx.debug_bounds("termua-sessions-sidebar-load-error")
        .expect("expected a visible load error when disk sessions cannot be parsed");
}

#[gpui::test]
fn session_labels_do_not_wrap_when_sidebar_is_narrow(cx: &mut gpui::TestAppContext) {
    cx.update(|app| {
        gpui_component::init(app);
    });

    let db_path = crate::store::tests::unique_test_db_path("sessions-sidebar-nowrap");
    let _guard = crate::store::tests::override_termua_db_path(db_path);

    let session_id = crate::store::save_local_session(
        "Group",
        "This is a very long session name that should not wrap",
        crate::settings::TerminalBackend::Wezterm,
        "xterm-256color",
        "UTF-8",
    )
    .unwrap();

    let row_selector: &'static str =
        Box::leak(format!("termua-sessions-session-item-{session_id}").into_boxed_str());

    let (root, cx) = cx.add_window_view(|window, cx| {
        let sidebar = cx.new(|cx| SessionsSidebarView::new(window, cx));
        gpui_component::Root::new(sidebar, window, cx)
    });

    let root_for_draw = root.clone();
    cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(600.)),
            gpui::AvailableSpace::Definite(gpui::px(200.)),
        ),
        move |_, _| div().size_full().child(root_for_draw),
    );
    cx.run_until_parked();

    let wide = cx
        .debug_bounds(row_selector)
        .expect("expected session label bounds at wide width");

    cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(160.)),
            gpui::AvailableSpace::Definite(gpui::px(200.)),
        ),
        move |_, _| div().size_full().child(root),
    );
    cx.run_until_parked();

    let narrow = cx
        .debug_bounds(row_selector)
        .expect("expected session label bounds at narrow width");

    assert!(
        narrow.size.height <= wide.size.height + gpui::px(0.5),
        "expected label height to stay single-line when sidebar is narrow; wide={:?}, narrow={:?}",
        wide.size.height,
        narrow.size.height
    );
}

#[gpui::test]
fn reload_coalesces_while_previous_reload_is_in_flight(cx: &mut gpui::TestAppContext) {
    cx.update(|app| {
        gpui_component::init(app);
    });

    let db_path = crate::store::tests::unique_test_db_path("sessions-sidebar-reload-coalesce");
    let _guard = crate::store::tests::override_termua_db_path(db_path);

    crate::store::save_local_session(
        "local",
        "bash",
        crate::settings::TerminalBackend::Wezterm,
        "xterm-256color",
        "UTF-8",
    )
    .unwrap();

    struct Harness {
        sidebar: Entity<SessionsSidebarView>,
    }

    impl Harness {
        fn new(
            sidebar_out: Arc<Mutex<Option<Entity<SessionsSidebarView>>>>,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) -> Self {
            let sidebar = cx.new(|cx| SessionsSidebarView::new(window, cx));
            *sidebar_out.lock().unwrap() = Some(sidebar.clone());
            Self { sidebar }
        }
    }

    impl Render for Harness {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(self.sidebar.clone())
        }
    }

    let sidebar_out: Arc<Mutex<Option<Entity<SessionsSidebarView>>>> = Arc::new(Mutex::new(None));
    let sidebar_out_for_view = sidebar_out.clone();
    let (harness, window_cx) =
        cx.add_window_view(move |window, cx| Harness::new(sidebar_out_for_view, window, cx));

    window_cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(600.)),
            gpui::AvailableSpace::Definite(gpui::px(400.)),
        ),
        move |_, _| div().size_full().child(harness),
    );
    window_cx.run_until_parked();

    let sidebar = sidebar_out
        .lock()
        .unwrap()
        .clone()
        .expect("expected sidebar to be captured");

    window_cx.update(|window, app| {
        sidebar.update(app, |this, cx| {
            this.reload(window, cx);
            this.reload(window, cx);
            assert!(this.reload_in_flight, "reload should be marked in-flight");
            assert!(
                this.reload_pending,
                "second reload request should be coalesced as pending"
            );
        });
    });
    window_cx.run_until_parked();

    window_cx.update(|_window, app| {
        let this = sidebar.read(app);
        assert!(
            !this.reload_in_flight,
            "reload state should clear after async work completes"
        );
        assert!(
            !this.reload_pending,
            "pending reload flag should clear after queued reload runs"
        );
    });
}

#[gpui::test]
fn sidebar_shows_session_persistence_errors(cx: &mut gpui::TestAppContext) {
    cx.update(|app| {
        gpui_component::init(app);
    });

    let db_path = crate::store::tests::unique_test_db_path("sessions-sidebar-persist-error");
    let _guard = crate::store::tests::override_termua_db_path(db_path);

    struct Harness {
        sidebar: Entity<SessionsSidebarView>,
    }

    impl Harness {
        fn new(
            sidebar_out: Arc<Mutex<Option<Entity<SessionsSidebarView>>>>,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) -> Self {
            let sidebar = cx.new(|cx| SessionsSidebarView::new(window, cx));
            *sidebar_out.lock().unwrap() = Some(sidebar.clone());
            Self { sidebar }
        }
    }

    impl Render for Harness {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(self.sidebar.clone())
        }
    }

    let sidebar_out: Arc<Mutex<Option<Entity<SessionsSidebarView>>>> = Arc::new(Mutex::new(None));
    let sidebar_out_for_view = sidebar_out.clone();
    let (harness, window_cx) =
        cx.add_window_view(move |window, cx| Harness::new(sidebar_out_for_view, window, cx));

    window_cx.update(|window, app| {
        let sidebar = sidebar_out
            .lock()
            .unwrap()
            .clone()
            .expect("expected sidebar to be captured");
        sidebar.update(app, |this, cx| {
            this.show_error(
                "Failed to persist local session: term is required",
                window,
                cx,
            );
        });
    });

    window_cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(600.)),
            gpui::AvailableSpace::Definite(gpui::px(400.)),
        ),
        move |_, _| div().size_full().child(harness),
    );
    window_cx.run_until_parked();

    window_cx
        .debug_bounds("termua-sessions-sidebar-operation-error")
        .expect("expected persistence error to be visible in the sessions sidebar");
}

#[gpui::test]
fn repeated_delete_requests_for_same_session_are_ignored(cx: &mut gpui::TestAppContext) {
    cx.update(|app| {
        gpui_component::init(app);
    });

    let db_path = crate::store::tests::unique_test_db_path("sessions-sidebar-delete-dedupe");
    let _guard = crate::store::tests::override_termua_db_path(db_path);

    let session_id = crate::store::save_local_session(
        "local",
        "bash",
        crate::settings::TerminalBackend::Wezterm,
        "xterm-256color",
        "UTF-8",
    )
    .unwrap();

    struct Harness {
        sidebar: Entity<SessionsSidebarView>,
    }

    impl Harness {
        fn new(
            sidebar_out: Arc<Mutex<Option<Entity<SessionsSidebarView>>>>,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) -> Self {
            let sidebar = cx.new(|cx| SessionsSidebarView::new(window, cx));
            *sidebar_out.lock().unwrap() = Some(sidebar.clone());
            Self { sidebar }
        }
    }

    impl Render for Harness {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(self.sidebar.clone())
        }
    }

    let sidebar_out: Arc<Mutex<Option<Entity<SessionsSidebarView>>>> = Arc::new(Mutex::new(None));
    let sidebar_out_for_view = sidebar_out.clone();
    let (harness, window_cx) =
        cx.add_window_view(move |window, cx| Harness::new(sidebar_out_for_view, window, cx));

    window_cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(600.)),
            gpui::AvailableSpace::Definite(gpui::px(400.)),
        ),
        move |_, _| div().size_full().child(harness),
    );
    window_cx.run_until_parked();

    let sidebar = sidebar_out
        .lock()
        .unwrap()
        .clone()
        .expect("expected sidebar to be captured");

    window_cx.update(|window, app| {
        sidebar.update(app, |this, cx| {
            this.delete_session_by_id(session_id, window, cx);
            this.delete_session_by_id(session_id, window, cx);
            assert!(
                this.deleting_session_ids.contains(&session_id),
                "session should be marked deleting immediately"
            );
            assert_eq!(
                this.deleting_session_ids.len(),
                1,
                "repeat delete should not create duplicate in-flight entries"
            );
        });
    });
    window_cx.run_until_parked();

    assert!(
        crate::store::load_session(session_id).unwrap().is_none(),
        "session should be deleted from sqlite"
    );

    window_cx.update(|_window, app| {
        let this = sidebar.read(app);
        assert!(
            this.deleting_session_ids.is_empty(),
            "deleting state should clear after async delete completes"
        );
    });
}
