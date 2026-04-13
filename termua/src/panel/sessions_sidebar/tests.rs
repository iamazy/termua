use std::sync::{Arc, Mutex};

use gpui::{
    AppContext, Context, Entity, IntoElement, ParentElement, Render, Styled, Subscription, Window,
    div,
};

use super::*;
use crate::store::{Session, SessionType};

#[gpui::test]
fn folder_icons_toggle_with_expansion(cx: &mut gpui::TestAppContext) {
    cx.update(|app| {
        gpui_component::init(app);
    });

    let tmp_dir = std::env::temp_dir().join(format!(
        "termua-sessions-sidebar-folder-icons-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&tmp_dir).unwrap();
    let db_path = tmp_dir.join("termua").join("termua.db");
    let _guard = crate::store::tests::override_termua_db_path(db_path);

    crate::store::save_local_session(
        "Group",
        "bash",
        crate::settings::TerminalBackend::Wezterm,
        "bash",
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

    let tmp_dir = std::env::temp_dir().join(format!(
        "termua-sessions-sidebar-local-icon-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&tmp_dir).unwrap();
    let db_path = tmp_dir.join("termua").join("termua.db");
    let _guard = crate::store::tests::override_termua_db_path(db_path);

    let session_id = crate::store::save_local_session(
        "local",
        "bash",
        crate::settings::TerminalBackend::Wezterm,
        "bash",
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

    let tmp_dir = std::env::temp_dir().join(format!(
        "termua-sessions-sidebar-double-click-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&tmp_dir).unwrap();
    let db_path = tmp_dir.join("termua").join("termua.db");
    let _guard = crate::store::tests::override_termua_db_path(db_path);

    let session_id = crate::store::save_local_session(
        "local",
        "bash",
        crate::settings::TerminalBackend::Wezterm,
        "bash",
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
        Box::leak(format!("termua-sessions-session-item-{session_id}").into_boxed_str());
    let bounds = cx
        .debug_bounds(selector)
        .expect("expected the local session tree row to be debuggable");

    // Single click selects but does not open.
    cx.simulate_click(bounds.center(), gpui::Modifiers::none());
    assert!(
        opened.lock().unwrap().is_empty(),
        "single click should not open a session"
    );

    // Double click opens.
    cx.simulate_event(gpui::MouseDownEvent {
        position: bounds.center(),
        modifiers: gpui::Modifiers::none(),
        button: gpui::MouseButton::Left,
        click_count: 2,
        first_mouse: false,
    });
    cx.simulate_event(gpui::MouseUpEvent {
        position: bounds.center(),
        modifiers: gpui::Modifiers::none(),
        button: gpui::MouseButton::Left,
        click_count: 2,
    });

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

    let tmp_dir = std::env::temp_dir().join(format!(
        "termua-sessions-sidebar-connecting-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&tmp_dir).unwrap();
    let db_path = tmp_dir.join("termua").join("termua.db");
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

    let selector: &'static str =
        Box::leak(format!("termua-sessions-session-item-{session_id}").into_boxed_str());
    let bounds = window_cx
        .debug_bounds(selector)
        .expect("expected the ssh session tree row to be debuggable");

    // First double click opens and marks as connecting.
    window_cx.simulate_event(gpui::MouseDownEvent {
        position: bounds.center(),
        modifiers: gpui::Modifiers::none(),
        button: gpui::MouseButton::Left,
        click_count: 2,
        first_mouse: false,
    });
    window_cx.simulate_event(gpui::MouseUpEvent {
        position: bounds.center(),
        modifiers: gpui::Modifiers::none(),
        button: gpui::MouseButton::Left,
        click_count: 2,
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
    window_cx.simulate_event(gpui::MouseDownEvent {
        position: bounds.center(),
        modifiers: gpui::Modifiers::none(),
        button: gpui::MouseButton::Left,
        click_count: 2,
        first_mouse: false,
    });
    window_cx.simulate_event(gpui::MouseUpEvent {
        position: bounds.center(),
        modifiers: gpui::Modifiers::none(),
        button: gpui::MouseButton::Left,
        click_count: 2,
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

    let tmp_dir = std::env::temp_dir().join(format!(
        "termua-sessions-sidebar-delete-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&tmp_dir).unwrap();
    let db_path = tmp_dir.join("termua").join("termua.db");
    let _guard = crate::store::tests::override_termua_db_path(db_path);

    let session_id = crate::store::save_local_session(
        "local",
        "bash",
        crate::settings::TerminalBackend::Wezterm,
        "bash",
        "xterm-256color",
        "UTF-8",
    )
    .unwrap();

    let (root, cx) = cx.add_window_view(|window, cx| {
        let sidebar = cx.new(|cx| SessionsSidebarView::new(window, cx));
        gpui_component::Root::new(sidebar, window, cx)
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

    let row_selector: &'static str =
        Box::leak(format!("termua-sessions-session-item-{session_id}").into_boxed_str());
    let row_bounds = cx
        .debug_bounds(row_selector)
        .expect("expected the session tree row to be debuggable");

    // Open context menu via right click.
    cx.simulate_mouse_move(
        row_bounds.center(),
        None::<gpui::MouseButton>,
        gpui::Modifiers::none(),
    );
    cx.simulate_mouse_down(
        row_bounds.center(),
        gpui::MouseButton::Right,
        gpui::Modifiers::none(),
    );
    cx.simulate_mouse_up(
        row_bounds.center(),
        gpui::MouseButton::Right,
        gpui::Modifiers::none(),
    );
    cx.run_until_parked();

    // Force a re-draw so deferred menu UI is visible in debug bounds.
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

    assert!(
        crate::store::load_session(session_id).unwrap().is_some(),
        "expected right click to not delete immediately"
    );

    let delete_bounds = cx
        .debug_bounds("termua-sessions-context-delete")
        .expect("expected a Delete item in the context menu");
    cx.debug_bounds("termua-sessions-context-delete-icon")
        .expect("expected a Delete icon in the context menu");

    // Escape should dismiss the menu without deleting.
    cx.simulate_keystrokes("escape");
    cx.run_until_parked();

    // Click where Delete used to be. If the menu is truly dismissed, this should be a no-op.
    cx.simulate_click(delete_bounds.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    assert!(
        crate::store::load_session(session_id).unwrap().is_some(),
        "expected escape to not delete the session"
    );

    // Open menu again and delete via keyboard (PopupMenu behavior).
    cx.simulate_mouse_down(
        row_bounds.center(),
        gpui::MouseButton::Right,
        gpui::Modifiers::none(),
    );
    cx.simulate_mouse_up(
        row_bounds.center(),
        gpui::MouseButton::Right,
        gpui::Modifiers::none(),
    );
    cx.run_until_parked();

    cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(600.)),
            gpui::AvailableSpace::Definite(gpui::px(400.)),
        ),
        move |_, _| div().size_full().child(root),
    );
    cx.run_until_parked();

    // With multiple items, keyboard selection behavior can vary by platform/theme.
    // Click the Delete item directly to keep this test deterministic.
    let delete_bounds = cx
        .debug_bounds("termua-sessions-context-delete")
        .expect("expected a Delete item in the context menu");
    cx.debug_bounds("termua-sessions-context-delete-icon")
        .expect("expected a Delete icon in the context menu");
    cx.simulate_click(delete_bounds.center(), gpui::Modifiers::none());
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
            term: "xterm".to_string(),
            charset: "UTF-8".to_string(),
            shell_program: None,
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
            term: "xterm".to_string(),
            charset: "UTF-8".to_string(),
            shell_program: None,
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

    let tmp_dir = std::env::temp_dir().join(format!(
        "termua-sessions-sidebar-edit-menu-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&tmp_dir).unwrap();
    let db_path = tmp_dir.join("termua").join("termua.db");
    let _guard = crate::store::tests::override_termua_db_path(db_path);

    let session_id = crate::store::save_local_session(
        "local",
        "bash",
        crate::settings::TerminalBackend::Wezterm,
        "bash",
        "xterm-256color",
        "UTF-8",
    )
    .unwrap();

    let (root, cx) = cx.add_window_view(|window, cx| {
        let sidebar = cx.new(|cx| SessionsSidebarView::new(window, cx));
        gpui_component::Root::new(sidebar, window, cx)
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

    let row_selector: &'static str =
        Box::leak(format!("termua-sessions-session-item-{session_id}").into_boxed_str());
    let row_bounds = cx
        .debug_bounds(row_selector)
        .expect("expected the session tree row to be debuggable");

    cx.simulate_mouse_move(
        row_bounds.center(),
        None::<gpui::MouseButton>,
        gpui::Modifiers::none(),
    );
    cx.simulate_mouse_down(
        row_bounds.center(),
        gpui::MouseButton::Right,
        gpui::Modifiers::none(),
    );
    cx.simulate_mouse_up(
        row_bounds.center(),
        gpui::MouseButton::Right,
        gpui::Modifiers::none(),
    );
    cx.run_until_parked();

    // Force a re-draw so deferred menu UI is visible in debug bounds.
    cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(600.)),
            gpui::AvailableSpace::Definite(gpui::px(400.)),
        ),
        move |_, _| div().size_full().child(root),
    );
    cx.run_until_parked();

    cx.debug_bounds("termua-sessions-context-edit")
        .expect("expected context menu to include an Edit item");
    cx.debug_bounds("termua-sessions-context-edit-icon")
        .expect("expected context menu to include an Edit icon");
}

#[gpui::test]
fn powershell_sessions_show_pwsh_icon(cx: &mut gpui::TestAppContext) {
    cx.update(|app| {
        gpui_component::init(app);
    });

    let tmp_dir = std::env::temp_dir().join(format!(
        "termua-sessions-sidebar-pwsh-icon-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&tmp_dir).unwrap();
    let db_path = tmp_dir.join("termua").join("termua.db");
    let _guard = crate::store::tests::override_termua_db_path(db_path);

    let session_id = crate::store::save_local_session(
        "local",
        "powershell",
        crate::settings::TerminalBackend::Wezterm,
        "pwsh",
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
        Box::leak(format!("termua-sessions-session-icon-pwsh-{session_id}").into_boxed_str());
    cx.debug_bounds(icon_selector)
        .expect("expected PowerShell sessions to render pwsh.svg as their icon");
}

#[gpui::test]
fn nushell_sessions_show_nushell_icon(cx: &mut gpui::TestAppContext) {
    cx.update(|app| {
        gpui_component::init(app);
    });

    let tmp_dir = std::env::temp_dir().join(format!(
        "termua-sessions-sidebar-nushell-icon-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&tmp_dir).unwrap();
    let db_path = tmp_dir.join("termua").join("termua.db");
    let _guard = crate::store::tests::override_termua_db_path(db_path);

    let session_id = crate::store::save_local_session(
        "local",
        "nushell",
        crate::settings::TerminalBackend::Wezterm,
        "nu",
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
        Box::leak(format!("termua-sessions-session-icon-nushell-{session_id}").into_boxed_str());
    cx.debug_bounds(icon_selector)
        .expect("expected Nushell sessions to render nushell.png as their icon");
}

#[gpui::test]
fn blank_area_right_click_shows_new_session_menu_item(cx: &mut gpui::TestAppContext) {
    cx.update(|app| {
        gpui_component::init(app);
    });

    let tmp_dir = std::env::temp_dir().join(format!(
        "termua-sessions-sidebar-blank-new-session-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&tmp_dir).unwrap();
    let db_path = tmp_dir.join("termua").join("termua.db");
    let _guard = crate::store::tests::override_termua_db_path(db_path);

    let (root, cx) = cx.add_window_view(|window, cx| {
        let sidebar = cx.new(|cx| SessionsSidebarView::new(window, cx));
        gpui_component::Root::new(sidebar, window, cx)
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

    let sidebar_bounds = cx
        .debug_bounds("termua-sessions-sidebar")
        .expect("expected sidebar bounds");

    let click = gpui::point(
        sidebar_bounds.left() + gpui::px(20.0),
        sidebar_bounds.bottom() - gpui::px(20.0),
    );
    cx.simulate_mouse_move(click, None::<gpui::MouseButton>, gpui::Modifiers::none());
    cx.simulate_mouse_down(click, gpui::MouseButton::Right, gpui::Modifiers::none());
    cx.simulate_mouse_up(click, gpui::MouseButton::Right, gpui::Modifiers::none());
    cx.run_until_parked();

    // Force a re-draw so deferred menu UI is visible in debug bounds.
    cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(600.)),
            gpui::AvailableSpace::Definite(gpui::px(400.)),
        ),
        move |_, _| div().size_full().child(root),
    );
    cx.run_until_parked();

    cx.debug_bounds("termua-sessions-context-new-session")
        .expect("expected New Session item in blank-area context menu");
}

#[gpui::test]
fn folder_right_click_shows_new_session_menu_item(cx: &mut gpui::TestAppContext) {
    cx.update(|app| {
        gpui_component::init(app);
    });

    let tmp_dir = std::env::temp_dir().join(format!(
        "termua-sessions-sidebar-folder-new-session-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&tmp_dir).unwrap();
    let db_path = tmp_dir.join("termua").join("termua.db");
    let _guard = crate::store::tests::override_termua_db_path(db_path);

    crate::store::save_local_session(
        "Group",
        "bash",
        crate::settings::TerminalBackend::Wezterm,
        "bash",
        "xterm-256color",
        "UTF-8",
    )
    .unwrap();

    let (root, cx) = cx.add_window_view(|window, cx| {
        let sidebar = cx.new(|cx| SessionsSidebarView::new(window, cx));
        gpui_component::Root::new(sidebar, window, cx)
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

    let folder_bounds = cx
        .debug_bounds("termua-sessions-folder-row-Group")
        .expect("expected folder row bounds");

    cx.simulate_mouse_move(
        folder_bounds.center(),
        None::<gpui::MouseButton>,
        gpui::Modifiers::none(),
    );
    cx.simulate_mouse_down(
        folder_bounds.center(),
        gpui::MouseButton::Right,
        gpui::Modifiers::none(),
    );
    cx.simulate_mouse_up(
        folder_bounds.center(),
        gpui::MouseButton::Right,
        gpui::Modifiers::none(),
    );
    cx.run_until_parked();

    // Force a re-draw so deferred menu UI is visible in debug bounds.
    cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(600.)),
            gpui::AvailableSpace::Definite(gpui::px(400.)),
        ),
        move |_, _| div().size_full().child(root),
    );
    cx.run_until_parked();

    cx.debug_bounds("termua-sessions-context-new-session")
        .expect("expected New Session item in folder context menu");
}

#[gpui::test]
fn session_labels_do_not_wrap_when_sidebar_is_narrow(cx: &mut gpui::TestAppContext) {
    cx.update(|app| {
        gpui_component::init(app);
    });

    let tmp_dir = std::env::temp_dir().join(format!(
        "termua-sessions-sidebar-nowrap-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&tmp_dir).unwrap();
    let db_path = tmp_dir.join("termua").join("termua.db");
    let _guard = crate::store::tests::override_termua_db_path(db_path);

    let session_id = crate::store::save_local_session(
        "Group",
        "This is a very long session name that should not wrap",
        crate::settings::TerminalBackend::Wezterm,
        "bash",
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

    let tmp_dir = std::env::temp_dir().join(format!(
        "termua-sessions-sidebar-reload-coalesce-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&tmp_dir).unwrap();
    let db_path = tmp_dir.join("termua").join("termua.db");
    let _guard = crate::store::tests::override_termua_db_path(db_path);

    crate::store::save_local_session(
        "local",
        "bash",
        crate::settings::TerminalBackend::Wezterm,
        "bash",
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
fn repeated_delete_requests_for_same_session_are_ignored(cx: &mut gpui::TestAppContext) {
    cx.update(|app| {
        gpui_component::init(app);
    });

    let tmp_dir = std::env::temp_dir().join(format!(
        "termua-sessions-sidebar-delete-dedupe-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&tmp_dir).unwrap();
    let db_path = tmp_dir.join("termua").join("termua.db");
    let _guard = crate::store::tests::override_termua_db_path(db_path);

    let session_id = crate::store::save_local_session(
        "local",
        "bash",
        crate::settings::TerminalBackend::Wezterm,
        "bash",
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
