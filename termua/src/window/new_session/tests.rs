use gpui::{ParentElement, Render, Styled, div};
use gpui_common::TermuaIcon;

use super::*;
use crate::env::build_local_terminal_env;

#[test]
fn new_session_connect_enabled() {
    assert!(!connect_enabled(Protocol::Ssh, "", "22"));
    assert!(!connect_enabled(Protocol::Ssh, "example.com", ""));
    assert!(!connect_enabled(Protocol::Ssh, "example.com", "0"));
    assert!(!connect_enabled(Protocol::Ssh, "example.com", "65536"));
    assert!(connect_enabled(Protocol::Ssh, "example.com", "1"));
    assert!(connect_enabled(Protocol::Ssh, "example.com", "22"));
    assert!(connect_enabled(Protocol::Ssh, "example.com", "65535"));

    assert!(connect_enabled(Protocol::Shell, "", ""));
}

#[gpui::test]
fn new_session_renders_lock_overlay_when_locked(cx: &mut gpui::TestAppContext) {
    use std::time::Duration;

    cx.update(|app| {
        gpui_component::init(app);
        menubar::init(app);
        gpui_term::init(app);
        app.set_global(crate::lock_screen::LockState::new_for_test(
            Duration::from_secs(60),
        ));
        app.set_global(crate::notification::NotifyState::default());
    });

    let window = cx.add_empty_window();
    window.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(900.)),
            gpui::AvailableSpace::Definite(gpui::px(600.)),
        ),
        |window, app| {
            let view = app.new(|cx| NewSessionWindow::new(window, cx));
            app.global_mut::<crate::lock_screen::LockState>()
                .force_lock_for_test();
            div().size_full().child(view)
        },
    );
    window.run_until_parked();

    assert!(
        window.debug_bounds("termua-lock-overlay").is_some(),
        "expected New Session to render the lock overlay while locked"
    );
    assert!(
        window.debug_bounds("termua-lock-drag-overlay").is_some(),
        "expected a drag overlay so the window remains movable while locked"
    );
    assert!(window.debug_bounds("termua-lock-password-input").is_some());
}

#[gpui::test]
fn new_session_ssh_password_mode_renders_split_user_and_host_inputs(cx: &mut gpui::TestAppContext) {
    cx.update(|app| {
        menubar::init(app);
        gpui_term::init(app);
    });

    let win = cx.add_empty_window();
    win.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(800.)),
            gpui::AvailableSpace::Definite(gpui::px(600.)),
        ),
        |window, app| {
            let view = app.new(|cx| NewSessionWindow::new(window, cx));
            view.update(app, |this, cx| {
                this.set_protocol(Protocol::Ssh, cx);
                this.ssh
                    .set_auth_type_for_test_only(SshAuthType::Password, window, cx);
            });
            div().size_full().child(view)
        },
    );
    win.run_until_parked();

    assert!(
        win.debug_bounds("termua-new-session-ssh-user-input")
            .is_some(),
        "expected user input in password mode"
    );
    assert!(
        win.debug_bounds("termua-new-session-ssh-at-label")
            .is_some(),
        "expected @ label in password mode"
    );
    assert!(
        win.debug_bounds("termua-new-session-ssh-host-input")
            .is_some(),
        "expected host input in password mode"
    );

    let host = win
        .debug_bounds("termua-new-session-ssh-host-input")
        .expect("host input bounds should exist");
    assert!(
        host.size.width >= gpui::px(120.0),
        "expected host input to be usable, got {host:?}"
    );
}

#[gpui::test]
fn new_session_ssh_user_input_has_min_width_120px_and_grows_until_200px(
    cx: &mut gpui::TestAppContext,
) {
    cx.update(|app| {
        menubar::init(app);
        gpui_term::init(app);
    });

    let mut measure = |user_value: &str| -> gpui::Pixels {
        let user_value = user_value.to_string();
        let win = cx.add_empty_window();
        win.draw(
            gpui::point(gpui::px(0.), gpui::px(0.)),
            gpui::size(
                gpui::AvailableSpace::Definite(gpui::px(800.)),
                gpui::AvailableSpace::Definite(gpui::px(600.)),
            ),
            |window, app| {
                let view = app.new(|cx| NewSessionWindow::new(window, cx));
                view.update(app, |this, cx| {
                    this.set_protocol(Protocol::Ssh, cx);
                    this.ssh
                        .set_auth_type_for_test_only(SshAuthType::Password, window, cx);
                });

                let user_input = view.read(app).ssh.user_input.clone();
                user_input.update(app, |input, cx| {
                    input.set_value(&user_value, window, cx);
                });

                div().size_full().child(view)
            },
        );
        win.run_until_parked();

        win.debug_bounds("termua-new-session-ssh-user-input")
            .expect("user input bounds should exist")
            .size
            .width
    };

    let w1 = measure("a");
    let w10 = measure("abcdefghij");
    let w40 = measure("0123456789012345678901234567890123456789");
    let w200 = measure(
        "0123456789012345678901234567890123456789012345678901234567890123456789\
         0123456789012345678901234567890123456789012345678901234567890123456789\
         0123456789012345678901234567890123456789012345678901234567890123456789",
    );

    let dw = (w10 - w1).abs();
    assert!(
        (w1 - gpui::px(120.0)).abs() <= gpui::px(1.0),
        "expected min width 120px; got {w1:?}"
    );
    assert!(
        dw <= gpui::px(1.0),
        "expected short text to keep fixed width; got {w1:?} vs {w10:?} (dw={dw:?})"
    );
    assert!(
        w40 > w10,
        "expected width to grow when text is long; got {w10:?} vs {w40:?}"
    );
    assert!(
        w200 <= gpui::px(200.0) + gpui::px(1.0),
        "expected width to cap at 200px; got {w200:?}"
    );
}

#[gpui::test]
fn new_session_left_pane_does_not_shrink_when_user_is_long(cx: &mut gpui::TestAppContext) {
    cx.update(|app| {
        menubar::init(app);
        gpui_term::init(app);
    });

    let win = cx.add_empty_window();
    win.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(720.)),
            gpui::AvailableSpace::Definite(gpui::px(480.)),
        ),
        |window, app| {
            let view = app.new(|cx| NewSessionWindow::new(window, cx));
            view.update(app, |this, cx| {
                this.set_protocol(Protocol::Ssh, cx);
                this.ssh
                    .set_auth_type_for_test_only(SshAuthType::Password, window, cx);
            });

            let user_input = view.read(app).ssh.user_input.clone();
            user_input.update(app, |input, cx| {
                input.set_value(
                    "this-is-a-very-very-very-very-very-very-long-username",
                    window,
                    cx,
                );
            });

            div().size_full().child(view)
        },
    );
    win.run_until_parked();

    let left = win
        .debug_bounds("termua-new-session-left-pane")
        .expect("left pane bounds should exist");
    let dw = (left.size.width - gpui::px(260.0)).abs();
    assert!(
        dw <= gpui::px(1.0),
        "expected left pane width ~260px, got {left:?}"
    );
}

#[gpui::test]
fn new_session_ssh_hides_password_input_in_config_mode(cx: &mut gpui::TestAppContext) {
    cx.update(|app| {
        menubar::init(app);
        gpui_term::init(app);
    });

    let win = cx.add_empty_window();
    win.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(800.)),
            gpui::AvailableSpace::Definite(gpui::px(600.)),
        ),
        |window, app| {
            let view = app.new(|cx| NewSessionWindow::new(window, cx));
            view.update(app, |this, cx| {
                this.set_protocol(Protocol::Ssh, cx);
                this.ssh
                    .set_auth_type_for_test_only(SshAuthType::Config, window, cx);
            });
            div().size_full().child(view)
        },
    );
    win.run_until_parked();
    assert!(
        win.debug_bounds("termua-new-session-ssh-password-input")
            .is_none(),
        "password input should be hidden in SSH Config mode"
    );
}

#[gpui::test]
fn new_session_ssh_port_invalid_shows_error(cx: &mut gpui::TestAppContext) {
    cx.update(|app| {
        menubar::init(app);
        gpui_term::init(app);
    });

    let win = cx.add_empty_window();
    win.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(800.)),
            gpui::AvailableSpace::Definite(gpui::px(600.)),
        ),
        |window, app| {
            let view = app.new(|cx| NewSessionWindow::new(window, cx));
            view.update(app, |this, cx| this.set_protocol(Protocol::Ssh, cx));

            let (host_input, port_input) = {
                let this = view.read(app);
                (this.ssh.host_input.clone(), this.ssh.port_input.clone())
            };
            host_input.update(app, |input, cx| {
                input.set_value("example.com", window, cx);
            });
            port_input.update(app, |input, cx| {
                input.set_value("0", window, cx);
            });

            div().size_full().child(view)
        },
    );
    win.run_until_parked();

    assert!(
        win.debug_bounds("termua-new-session-ssh-port-error")
            .is_some(),
        "expected port validation error to be rendered"
    );
}

#[gpui::test]
fn new_session_ssh_renders_password_input(cx: &mut gpui::TestAppContext) {
    cx.update(|app| {
        menubar::init(app);
        gpui_term::init(app);
    });

    let win = cx.add_empty_window();
    win.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(800.)),
            gpui::AvailableSpace::Definite(gpui::px(600.)),
        ),
        |window, app| {
            let view = app.new(|cx| NewSessionWindow::new(window, cx));
            view.update(app, |this, cx| this.set_protocol(Protocol::Ssh, cx));
            div().size_full().child(view)
        },
    );
    win.run_until_parked();

    assert!(
        win.debug_bounds("termua-new-session-ssh-password-input")
            .is_some(),
        "expected password input to be rendered"
    );
}

#[gpui::test]
fn new_session_window_is_wrapped_in_gpui_component_root(cx: &mut gpui::TestAppContext) {
    let handle = {
        let mut app = cx.app.borrow_mut();
        menubar::init(&mut app);
        gpui_term::init(&mut app);
        NewSessionWindow::open(&mut app).unwrap()
    };

    handle
        .update(cx, |_, window, _cx| {
            assert!(window.root::<gpui_component::Root>().flatten().is_some());
        })
        .unwrap();
}

#[gpui::test]
fn new_session_protocol_tabs_fill_available_width(cx: &mut gpui::TestAppContext) {
    cx.update(|app| {
        menubar::init(app);
        gpui_term::init(app);
    });

    let cx = cx.add_empty_window();
    cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(800.)),
            gpui::AvailableSpace::Definite(gpui::px(600.)),
        ),
        |window, app| {
            let view = app.new(|cx| NewSessionWindow::new(window, cx));
            div().size_full().child(view)
        },
    );
    cx.run_until_parked();

    let shell = cx
        .debug_bounds("termua-new-session-tab-shell")
        .expect("shell tab should exist");
    let ssh = cx
        .debug_bounds("termua-new-session-tab-ssh")
        .expect("ssh tab should exist");
    let serial = cx
        .debug_bounds("termua-new-session-tab-serial")
        .expect("serial tab should exist");
    let bar = cx
        .debug_bounds("termua-new-session-protocol-tabbar")
        .expect("tab bar should exist");

    // No gaps between adjacent tabs (allow 1px due to borders/rounding).
    let gap1 = (ssh.left() - shell.right()).abs();
    assert!(gap1 <= gpui::px(1.0), "expected no gap, got {gap1:?}");
    let gap2 = (serial.left() - ssh.right()).abs();
    assert!(gap2 <= gpui::px(1.0), "expected no gap, got {gap2:?}");

    // Tabs should fill the bar edge-to-edge (no outer padding).
    let left_pad = (shell.left() - bar.left()).abs();
    let right_pad = (bar.right() - serial.right()).abs();
    assert!(
        left_pad <= gpui::px(1.0),
        "expected no left padding, got {left_pad:?}"
    );
    assert!(
        right_pad <= gpui::px(1.0),
        "expected no right padding, got {right_pad:?}"
    );

    // They should be equal width (allow 1px).
    let dw1 = (shell.size.width - ssh.size.width).abs();
    assert!(dw1 <= gpui::px(1.0), "expected equal widths, got {dw1:?}");
    let dw2 = (ssh.size.width - serial.size.width).abs();
    assert!(dw2 <= gpui::px(1.0), "expected equal widths, got {dw2:?}");
}

#[gpui::test]
fn new_session_tabs_include_serial(cx: &mut gpui::TestAppContext) {
    cx.update(|app| {
        menubar::init(app);
        gpui_term::init(app);
    });

    let cx = cx.add_empty_window();
    cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(800.)),
            gpui::AvailableSpace::Definite(gpui::px(600.)),
        ),
        |window, app| {
            let view = app.new(|cx| NewSessionWindow::new(window, cx));
            div().size_full().child(view)
        },
    );
    cx.run_until_parked();

    assert!(
        cx.debug_bounds("termua-new-session-tab-serial").is_some(),
        "serial tab should exist"
    );
}

#[test]
fn new_session_ssh_main_page_is_under_session_node_and_connection_page_is_addressable() {
    assert_eq!(
        page_for_tree_item_id(Protocol::Ssh, "ssh.session"),
        Page::SshSession
    );
    assert_eq!(
        page_for_tree_item_id(Protocol::Ssh, "ssh.connection"),
        Page::SshConnection
    );

    let default = default_selected_item_id(Protocol::Ssh);
    assert_eq!(default.as_ref(), "ssh.session");
}

#[gpui::test]
fn new_session_ssh_proxy_page_renders_proxy_controls(cx: &mut gpui::TestAppContext) {
    cx.update(|app| {
        menubar::init(app);
        gpui_term::init(app);
    });

    let win = cx.add_empty_window();
    win.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(860.)),
            gpui::AvailableSpace::Definite(gpui::px(640.)),
        ),
        |window, app| {
            let view = app.new(|cx| NewSessionWindow::new(window, cx));
            view.update(app, |this, cx| {
                this.set_protocol(Protocol::Ssh, cx);
                this.selected_item_id = "ssh.proxy".into();
                this.sync_nav_tree_selection(cx);
                // Force a deterministic selection so command/workdir inputs render.
                this.ssh.set_proxy_mode(SshProxyMode::Command, window, cx);
                cx.notify();
            });
            div().size_full().child(view)
        },
    );
    win.run_until_parked();

    assert!(
        win.debug_bounds("termua-new-session-ssh-proxy-type")
            .is_some()
    );
    assert!(
        win.debug_bounds("termua-new-session-ssh-proxy-command")
            .is_some()
    );
    assert!(
        win.debug_bounds("termua-new-session-ssh-proxy-working-dir")
            .is_some()
    );
}

#[gpui::test]
fn new_session_ssh_proxy_page_renders_jumpserver_controls(cx: &mut gpui::TestAppContext) {
    cx.update(|app| {
        menubar::init(app);
        gpui_term::init(app);
    });

    let win = cx.add_empty_window();
    win.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(860.)),
            gpui::AvailableSpace::Definite(gpui::px(640.)),
        ),
        |window, app| {
            let view = app.new(|cx| NewSessionWindow::new(window, cx));
            view.update(app, |this, cx| {
                this.set_protocol(Protocol::Ssh, cx);
                this.selected_item_id = "ssh.proxy".into();
                this.sync_nav_tree_selection(cx);
                this.ssh
                    .set_proxy_mode(SshProxyMode::JumpServer, window, cx);
                cx.notify();
            });
            div().size_full().child(view)
        },
    );
    win.run_until_parked();

    assert!(
        win.debug_bounds("termua-new-session-ssh-proxy-type")
            .is_some()
    );
    assert!(
        win.debug_bounds("termua-new-session-ssh-proxy-jump-chain")
            .is_some()
    );
    assert!(
        win.debug_bounds("termua-new-session-ssh-proxy-jump-add")
            .is_some()
    );
}

#[gpui::test]
fn new_session_shell_and_ssh_session_pages_render_type_dropdowns(cx: &mut gpui::TestAppContext) {
    cx.update(|app| {
        menubar::init(app);
        gpui_term::init(app);
    });

    // Shell session page is default.
    let shell = cx.add_empty_window();
    shell.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(800.)),
            gpui::AvailableSpace::Definite(gpui::px(600.)),
        ),
        |window, app| {
            let view = app.new(|cx| NewSessionWindow::new(window, cx));
            div().size_full().child(view)
        },
    );
    shell.run_until_parked();
    assert!(
        shell
            .debug_bounds("termua-new-session-shell-type")
            .is_some()
    );

    // Switch to SSH protocol and ensure the SSH session page has its type dropdown.
    let ssh = cx.add_empty_window();
    ssh.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(800.)),
            gpui::AvailableSpace::Definite(gpui::px(600.)),
        ),
        |window, app| {
            let view = app.new(|cx| NewSessionWindow::new(window, cx));
            view.update(app, |this, cx| this.set_protocol(Protocol::Ssh, cx));
            div().size_full().child(view)
        },
    );
    ssh.run_until_parked();
    assert!(ssh.debug_bounds("termua-new-session-ssh-type").is_some());
}

#[gpui::test]
fn new_session_ssh_connection_page_renders_tcp_socket_switches(cx: &mut gpui::TestAppContext) {
    cx.update(|app| {
        menubar::init(app);
        gpui_term::init(app);
    });

    let win = cx.add_empty_window();
    win.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(800.)),
            gpui::AvailableSpace::Definite(gpui::px(600.)),
        ),
        |window, app| {
            let view = app.new(|cx| NewSessionWindow::new(window, cx));
            view.update(app, |this, cx| {
                this.set_protocol(Protocol::Ssh, cx);
                this.selected_item_id = "ssh.connection".into();
                this.sync_nav_tree_selection(cx);
                cx.notify();
            });
            div().size_full().child(view)
        },
    );
    win.run_until_parked();

    assert!(
        win.debug_bounds("termua-new-session-ssh-tcp-nodelay")
            .is_some()
    );
    assert!(
        win.debug_bounds("termua-new-session-ssh-tcp-keepalive")
            .is_some()
    );
}

#[gpui::test]
fn new_session_ssh_tcp_nodelay_defaults_to_true(cx: &mut gpui::TestAppContext) {
    use std::sync::{Arc, Mutex};

    cx.update(|app| {
        menubar::init(app);
        gpui_term::init(app);
    });

    let win = cx.add_empty_window();
    let view_slot: Arc<Mutex<Option<Entity<NewSessionWindow>>>> = Arc::new(Mutex::new(None));
    let view_slot_for_draw = Arc::clone(&view_slot);

    win.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(800.)),
            gpui::AvailableSpace::Definite(gpui::px(600.)),
        ),
        move |window, app| {
            let view = app.new(|cx| NewSessionWindow::new(window, cx));
            view.update(app, |this, cx| {
                this.set_protocol(Protocol::Ssh, cx);
            });
            *view_slot_for_draw.lock().unwrap() = Some(view.clone());
            div().size_full().child(view)
        },
    );
    win.run_until_parked();

    let view = view_slot
        .lock()
        .unwrap()
        .clone()
        .expect("expected view to be captured");

    win.update(|_window, app| {
        assert!(
            view.read(app).ssh.tcp_nodelay,
            "expected TCP_NODELAY to default to enabled for new SSH sessions"
        );
    });
}

#[gpui::test]
fn new_session_type_dropdown_buttons_render_icons_for_alacritty_and_wezterm(
    cx: &mut gpui::TestAppContext,
) {
    // Point settings.json to a temp directory so this test is hermetic.
    let tmp_dir = std::env::temp_dir().join(format!(
        "termua-new-session-test-type-icons-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&tmp_dir).unwrap();

    // Set the default backend to Wezterm so the initial icon is predictable.
    let path = tmp_dir.join("termua").join("settings.json");
    let _guard = crate::settings::override_settings_json_path(path.clone());
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(
        &path,
        r#"{
          "terminal": { "default_backend": "wezterm" }
        }"#,
    )
    .unwrap();

    cx.update(|app| {
        menubar::init(app);
        gpui_term::init(app);
    });

    // Shell type icon: wezterm (default) and alacritty (after update).
    let shell = cx.add_empty_window();
    shell.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(800.)),
            gpui::AvailableSpace::Definite(gpui::px(600.)),
        ),
        |window, app| {
            let view = app.new(|cx| NewSessionWindow::new(window, cx));
            div().size_full().child(view)
        },
    );
    shell.run_until_parked();
    assert!(
        shell
            .debug_bounds("termua-new-session-shell-type-icon-wezterm")
            .is_some()
    );

    let shell_alacritty = cx.add_empty_window();
    shell_alacritty.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(800.)),
            gpui::AvailableSpace::Definite(gpui::px(600.)),
        ),
        |window, app| {
            let view = app.new(|cx| NewSessionWindow::new(window, cx));
            view.update(app, |this, cx| {
                this.shell
                    .common
                    .set_type(TermBackend::Alacritty, window, cx);
                cx.notify();
            });
            div().size_full().child(view)
        },
    );
    shell_alacritty.run_until_parked();
    assert!(
        shell_alacritty
            .debug_bounds("termua-new-session-shell-type-icon-alacritty")
            .is_some()
    );

    // SSH type icon: wezterm (default) and alacritty (after update).
    let ssh = cx.add_empty_window();
    ssh.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(800.)),
            gpui::AvailableSpace::Definite(gpui::px(600.)),
        ),
        |window, app| {
            let view = app.new(|cx| NewSessionWindow::new(window, cx));
            view.update(app, |this, cx| this.set_protocol(Protocol::Ssh, cx));
            div().size_full().child(view)
        },
    );
    ssh.run_until_parked();
    assert!(
        ssh.debug_bounds("termua-new-session-ssh-type-icon-wezterm")
            .is_some()
    );

    let ssh_alacritty = cx.add_empty_window();
    ssh_alacritty.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(800.)),
            gpui::AvailableSpace::Definite(gpui::px(600.)),
        ),
        |window, app| {
            let view = app.new(|cx| NewSessionWindow::new(window, cx));
            view.update(app, |this, cx| {
                this.set_protocol(Protocol::Ssh, cx);
                this.ssh.common.set_type(TermBackend::Alacritty, window, cx);
                cx.notify();
            });
            div().size_full().child(view)
        },
    );
    ssh_alacritty.run_until_parked();
    assert!(
        ssh_alacritty
            .debug_bounds("termua-new-session-ssh-type-icon-alacritty")
            .is_some()
    );
}

#[gpui::test]
fn new_session_default_type_matches_terminal_default_backend_setting(
    cx: &mut gpui::TestAppContext,
) {
    // Point settings.json to a temp directory so this test is hermetic.
    let tmp_dir = std::env::temp_dir().join(format!(
        "termua-new-session-test-default-backend-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&tmp_dir).unwrap();

    // Write a settings.json that selects Alacritty as the default backend.
    let path = tmp_dir.join("termua").join("settings.json");
    let _guard = crate::settings::override_settings_json_path(path.clone());
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(
        &path,
        r#"{
          "terminal": { "default_backend": "alacritty" }
        }"#,
    )
    .unwrap();

    cx.update(|app| {
        menubar::init(app);
        gpui_term::init(app);
    });

    let shell = cx.add_empty_window();
    shell.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(800.)),
            gpui::AvailableSpace::Definite(gpui::px(600.)),
        ),
        |window, app| {
            let view = app.new(|cx| NewSessionWindow::new(window, cx));
            div().size_full().child(view)
        },
    );
    shell.run_until_parked();
    assert!(
        shell
            .debug_bounds("termua-new-session-shell-type-icon-alacritty")
            .is_some()
    );
}

#[gpui::test]
fn new_session_type_controls_render_select_component(cx: &mut gpui::TestAppContext) {
    cx.update(|app| {
        menubar::init(app);
        gpui_term::init(app);
    });

    let shell = cx.add_empty_window();
    shell.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(800.)),
            gpui::AvailableSpace::Definite(gpui::px(600.)),
        ),
        |window, app| {
            let view = app.new(|cx| NewSessionWindow::new(window, cx));
            div().size_full().child(view)
        },
    );
    shell.run_until_parked();
    assert!(
        shell
            .debug_bounds("termua-new-session-shell-type-select")
            .is_some()
    );

    let ssh = cx.add_empty_window();
    ssh.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(800.)),
            gpui::AvailableSpace::Definite(gpui::px(600.)),
        ),
        |window, app| {
            let view = app.new(|cx| NewSessionWindow::new(window, cx));
            view.update(app, |this, cx| this.set_protocol(Protocol::Ssh, cx));
            div().size_full().child(view)
        },
    );
    ssh.run_until_parked();
    assert!(
        ssh.debug_bounds("termua-new-session-ssh-type-select")
            .is_some()
    );
}

#[gpui::test]
fn new_session_group_controls_render_inputs(cx: &mut gpui::TestAppContext) {
    cx.update(|app| {
        menubar::init(app);
        gpui_term::init(app);
    });

    // Shell session page is default.
    let shell = cx.add_empty_window();
    shell.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(800.)),
            gpui::AvailableSpace::Definite(gpui::px(600.)),
        ),
        |window, app| {
            let view = app.new(|cx| NewSessionWindow::new(window, cx));
            div().size_full().child(view)
        },
    );
    shell.run_until_parked();
    assert!(
        shell
            .debug_bounds("termua-new-session-shell-group-input")
            .is_some()
    );

    // SSH session page (under Session).
    let ssh = cx.add_empty_window();
    ssh.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(800.)),
            gpui::AvailableSpace::Definite(gpui::px(600.)),
        ),
        |window, app| {
            let view = app.new(|cx| NewSessionWindow::new(window, cx));
            view.update(app, |this, cx| this.set_protocol(Protocol::Ssh, cx));
            div().size_full().child(view)
        },
    );
    ssh.run_until_parked();
    assert!(
        ssh.debug_bounds("termua-new-session-ssh-group-input")
            .is_some()
    );
}

#[gpui::test]
fn new_session_term_and_charset_controls_render_selects(cx: &mut gpui::TestAppContext) {
    cx.update(|app| {
        menubar::init(app);
        gpui_term::init(app);
    });

    // Shell session page is default.
    let shell = cx.add_empty_window();
    shell.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(800.)),
            gpui::AvailableSpace::Definite(gpui::px(600.)),
        ),
        |window, app| {
            let view = app.new(|cx| NewSessionWindow::new(window, cx));
            div().size_full().child(view)
        },
    );
    shell.run_until_parked();
    assert!(
        shell
            .debug_bounds("termua-new-session-shell-term-select")
            .is_some()
    );
    assert!(
        shell
            .debug_bounds("termua-new-session-shell-charset-select")
            .is_some()
    );

    // SSH session page.
    let ssh = cx.add_empty_window();
    ssh.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(800.)),
            gpui::AvailableSpace::Definite(gpui::px(600.)),
        ),
        |window, app| {
            let view = app.new(|cx| NewSessionWindow::new(window, cx));
            view.update(app, |this, cx| this.set_protocol(Protocol::Ssh, cx));
            div().size_full().child(view)
        },
    );
    ssh.run_until_parked();
    assert!(
        ssh.debug_bounds("termua-new-session-ssh-term-select")
            .is_some()
    );
    assert!(
        ssh.debug_bounds("termua-new-session-ssh-charset-select")
            .is_some()
    );
}

#[gpui::test]
fn new_session_type_select_is_left_aligned(cx: &mut gpui::TestAppContext) {
    // Point settings.json to a temp directory so this test is hermetic.
    let tmp_dir = std::env::temp_dir().join(format!(
        "termua-new-session-test-type-left-aligned-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&tmp_dir).unwrap();

    // Set the default backend to Wezterm so the type content selector id is predictable.
    let path = tmp_dir.join("termua").join("settings.json");
    let _guard = crate::settings::override_settings_json_path(path.clone());
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(
        &path,
        r#"{
          "terminal": { "default_backend": "wezterm" }
        }"#,
    )
    .unwrap();

    cx.update(|app| {
        menubar::init(app);
        gpui_term::init(app);
    });

    let shell = cx.add_empty_window();
    shell.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(800.)),
            gpui::AvailableSpace::Definite(gpui::px(600.)),
        ),
        |window, app| {
            let view = app.new(|cx| NewSessionWindow::new(window, cx));
            div().size_full().child(view)
        },
    );
    shell.run_until_parked();

    let shell_select = shell
        .debug_bounds("termua-new-session-shell-type-select")
        .expect("shell type select should exist");
    let shell_content = shell
        .debug_bounds("termua-new-session-shell-type-icon-content-wezterm")
        .expect("shell type content should exist");
    let shell_left_gap = shell_content.left() - shell_select.left();
    assert!(
        shell_left_gap <= gpui::px(60.0),
        "expected shell type to be left-aligned"
    );

    let ssh = cx.add_empty_window();
    ssh.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(800.)),
            gpui::AvailableSpace::Definite(gpui::px(600.)),
        ),
        |window, app| {
            let view = app.new(|cx| NewSessionWindow::new(window, cx));
            view.update(app, |this, cx| this.set_protocol(Protocol::Ssh, cx));
            div().size_full().child(view)
        },
    );
    ssh.run_until_parked();

    let ssh_select = ssh
        .debug_bounds("termua-new-session-ssh-type-select")
        .expect("ssh type select should exist");
    let ssh_content = ssh
        .debug_bounds("termua-new-session-ssh-type-icon-content-wezterm")
        .expect("ssh type content should exist");
    let ssh_left_gap = ssh_content.left() - ssh_select.left();
    assert!(
        ssh_left_gap <= gpui::px(60.0),
        "expected ssh type to be left-aligned"
    );
}

#[gpui::test]
fn new_session_shell_program_control_uses_select_component(cx: &mut gpui::TestAppContext) {
    cx.update(|app| {
        menubar::init(app);
        gpui_term::init(app);
    });

    let shell = cx.add_empty_window();
    shell.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(800.)),
            gpui::AvailableSpace::Definite(gpui::px(600.)),
        ),
        |window, app| {
            let view = app.new(|cx| NewSessionWindow::new(window, cx));
            div().size_full().child(view)
        },
    );
    shell.run_until_parked();
    assert!(
        shell
            .debug_bounds("termua-new-session-shell-program-select")
            .is_some()
    );
    assert!(
        shell
            .debug_bounds("termua-new-session-shell-program")
            .is_none()
    );
}

#[gpui::test]
fn new_session_shell_program_select_value_is_left_aligned(cx: &mut gpui::TestAppContext) {
    cx.update(|app| {
        menubar::init(app);
        gpui_term::init(app);
    });

    let shell = cx.add_empty_window();
    shell.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(800.)),
            gpui::AvailableSpace::Definite(gpui::px(600.)),
        ),
        |window, app| {
            let view = app.new(|cx| NewSessionWindow::new(window, cx));
            div().size_full().child(view)
        },
    );
    shell.run_until_parked();

    let select_bounds = shell
        .debug_bounds("termua-new-session-shell-program-select")
        .expect("shell program select should exist");
    let content_bounds = shell
        .debug_bounds("termua-new-session-shell-program-display-title")
        .expect("shell program display title should exist");
    let left_gap = content_bounds.left() - select_bounds.left();
    assert!(
        left_gap <= gpui::px(60.0),
        "expected shell program select value to be left-aligned"
    );
}

#[gpui::test]
fn new_session_shell_label_follows_shell_program(cx: &mut gpui::TestAppContext) {
    use std::sync::{Arc, Mutex};

    cx.update(|app| {
        menubar::init(app);
        gpui_term::init(app);
    });

    let win = cx.add_empty_window();
    let view_slot: Arc<Mutex<Option<Entity<NewSessionWindow>>>> = Arc::new(Mutex::new(None));
    let view_slot_for_draw = Arc::clone(&view_slot);

    win.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(800.)),
            gpui::AvailableSpace::Definite(gpui::px(600.)),
        ),
        move |window, app| {
            let view = app.new(|cx| NewSessionWindow::new(window, cx));
            *view_slot_for_draw.lock().unwrap() = Some(view.clone());
            div().size_full().child(view)
        },
    );
    win.run_until_parked();

    let view = view_slot
        .lock()
        .unwrap()
        .clone()
        .expect("expected view to be captured");

    // Changing the shell program should update the Label input to match.
    win.update(|window, app| {
        view.update(app, |this, cx| {
            this.shell.set_program("pwsh", window, cx);
            cx.notify();
            window.refresh();
        });
    });
    win.run_until_parked();

    win.update(|_window, app| {
        assert_eq!(
            view.read(app)
                .shell
                .common
                .label_input
                .read(app)
                .value()
                .as_ref(),
            "powershell"
        );
    });
}

#[gpui::test]
fn edit_session_does_not_render_connect_button(cx: &mut gpui::TestAppContext) {
    cx.update(|app| {
        menubar::init(app);
        gpui_term::init(app);
    });

    let win = cx.add_empty_window();
    win.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(860.)),
            gpui::AvailableSpace::Definite(gpui::px(640.)),
        ),
        |window, app| {
            let session = crate::store::Session {
                id: 1,
                protocol: crate::store::SessionType::Ssh,
                group_path: "ssh".to_string(),
                label: "prod".to_string(),
                backend: crate::settings::TerminalBackend::Wezterm,
                term: "xterm-256color".to_string(),
                charset: "UTF-8".to_string(),
                shell_program: None,
                ssh_host: Some("example.com".to_string()),
                ssh_port: Some(22),
                ssh_auth_type: Some(crate::store::SshAuthType::Password),
                ssh_user: Some("root".to_string()),
                ssh_credential_username: None,
                ssh_password: Some("pw".to_string()),
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
            };
            let view = app.new(|cx| NewSessionWindow::new_for_edit(session, window, cx));
            div().size_full().child(view)
        },
    );
    win.run_until_parked();

    assert!(
        win.debug_bounds("termua-edit-session-connect").is_none(),
        "edit session should not render a Connect button"
    );
}

#[gpui::test]
fn edit_session_disables_protocol_switching(cx: &mut gpui::TestAppContext) {
    use std::sync::{Arc, Mutex};

    cx.update(|app| {
        menubar::init(app);
        gpui_term::init(app);
    });

    let win = cx.add_empty_window();
    let view_slot: Arc<Mutex<Option<Entity<NewSessionWindow>>>> = Arc::new(Mutex::new(None));
    let view_slot_for_draw = Arc::clone(&view_slot);

    win.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(860.)),
            gpui::AvailableSpace::Definite(gpui::px(640.)),
        ),
        move |window, app| {
            let session = crate::store::Session {
                id: 1,
                protocol: crate::store::SessionType::Ssh,
                group_path: "ssh".to_string(),
                label: "prod".to_string(),
                backend: crate::settings::TerminalBackend::Wezterm,
                term: "xterm-256color".to_string(),
                charset: "UTF-8".to_string(),
                shell_program: None,
                ssh_host: Some("example.com".to_string()),
                ssh_port: Some(22),
                ssh_auth_type: Some(crate::store::SshAuthType::Password),
                ssh_user: Some("root".to_string()),
                ssh_credential_username: None,
                ssh_password: Some("pw".to_string()),
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
            };

            let view = app.new(|cx| NewSessionWindow::new_for_edit(session, window, cx));
            *view_slot_for_draw.lock().unwrap() = Some(view.clone());
            div().size_full().child(view)
        },
    );
    win.run_until_parked();

    let view = view_slot
        .lock()
        .unwrap()
        .clone()
        .expect("expected view to be captured");

    win.update(|window, app| {
        view.update(app, |this, cx| {
            this.set_protocol(Protocol::Shell, cx);
            window.refresh();
        });
    });
    win.run_until_parked();

    win.update(|_window, app| {
        assert_eq!(
            view.read(app).protocol,
            Protocol::Ssh,
            "editing an SSH session should not allow switching to Shell protocol"
        );
    });
}

#[gpui::test]
fn edit_session_password_input_is_locked_until_explicitly_edited(cx: &mut gpui::TestAppContext) {
    cx.update(|app| {
        menubar::init(app);
        gpui_term::init(app);
    });

    let win = cx.add_empty_window();

    win.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(860.)),
            gpui::AvailableSpace::Definite(gpui::px(640.)),
        ),
        move |window, app| {
            let session = crate::store::Session {
                id: 1,
                protocol: crate::store::SessionType::Ssh,
                group_path: "ssh".to_string(),
                label: "prod".to_string(),
                backend: crate::settings::TerminalBackend::Wezterm,
                term: "xterm-256color".to_string(),
                charset: "UTF-8".to_string(),
                shell_program: None,
                ssh_host: Some("example.com".to_string()),
                ssh_port: Some(22),
                ssh_auth_type: Some(crate::store::SshAuthType::Password),
                ssh_user: Some("root".to_string()),
                ssh_credential_username: None,
                ssh_password: Some("pw".to_string()),
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
            };

            let view = app.new(|cx| NewSessionWindow::new_for_edit(session, window, cx));
            div().size_full().child(view)
        },
    );
    win.run_until_parked();

    assert!(
        win.debug_bounds("termua-edit-session-password-edit")
            .is_some(),
        "expected Edit Password button to be rendered for edit sessions"
    );
}

#[test]
fn local_terminal_env_includes_shell_term_and_locale() {
    let env = build_local_terminal_env("/bin/zsh", "xterm-256color", "UTF-8");
    assert_eq!(env.get("SHELL"), Some(&"/bin/zsh".to_string()));
    assert_eq!(env.get("TERMUA_SHELL"), Some(&"/bin/zsh".to_string()));
    assert_eq!(env.get("TERM"), Some(&"xterm-256color".to_string()));
    assert_eq!(env.get("LANG"), Some(&"en_US.UTF-8".to_string()));

    let env = build_local_terminal_env("/bin/bash", "screen-256color", "ASCII");
    assert_eq!(env.get("SHELL"), Some(&"/bin/bash".to_string()));
    assert_eq!(env.get("TERMUA_SHELL"), Some(&"/bin/bash".to_string()));
    assert_eq!(env.get("TERM"), Some(&"screen-256color".to_string()));
    assert_eq!(env.get("LANG"), Some(&"C".to_string()));
}

#[test]
fn shell_program_select_item_shows_full_name_and_shell_icons() {
    let nu = ShellProgramSelectItem::new("nu".into());
    assert_eq!(nu.title().as_ref(), "nushell");
    assert_eq!(nu.icon_path(), Some(TermuaIcon::Nushell));

    let pwsh = ShellProgramSelectItem::new("pwsh".into());
    assert_eq!(pwsh.title().as_ref(), "powershell");
    assert_eq!(pwsh.icon_path(), Some(TermuaIcon::Pwsh));

    let powershell = ShellProgramSelectItem::new("powershell".into());
    assert_eq!(powershell.title().as_ref(), "powershell");
    assert_eq!(powershell.icon_path(), Some(TermuaIcon::Pwsh));

    let bash = ShellProgramSelectItem::new("bash".into());
    assert_eq!(bash.title().as_ref(), "bash");
    assert_eq!(bash.icon_path(), Some(TermuaIcon::Terminal));

    let sh = ShellProgramSelectItem::new("sh".into());
    assert_eq!(sh.title().as_ref(), "sh");
    assert_eq!(sh.icon_path(), Some(TermuaIcon::Sh));

    let zsh = ShellProgramSelectItem::new("zsh".into());
    assert_eq!(zsh.title().as_ref(), "zsh");
    assert_eq!(zsh.icon_path(), Some(TermuaIcon::Terminal));
}

#[test]
fn shell_program_select_item_uses_themed_icon_renderer() {
    let bash = ShellProgramSelectItem::new("bash".into());
    assert!(bash.uses_themed_icon());

    let nu = ShellProgramSelectItem::new("nu".into());
    assert!(nu.uses_themed_icon());

    let pwsh = ShellProgramSelectItem::new("pwsh".into());
    assert!(!pwsh.uses_themed_icon());
}

#[test]
fn edit_mode_disabled_protocol_tab_uses_not_allowed_cursor() {
    let selected_ix = Protocol::Ssh.tab_index();
    assert_eq!(
        NewSessionWindow::disabled_protocol_tab_cursor_style(
            true,
            selected_ix,
            Protocol::Shell.tab_index()
        ),
        Some(gpui::CursorStyle::OperationNotAllowed)
    );
    assert_eq!(
        NewSessionWindow::disabled_protocol_tab_cursor_style(true, selected_ix, selected_ix),
        None
    );
    assert_eq!(
        NewSessionWindow::disabled_protocol_tab_cursor_style(
            false,
            selected_ix,
            Protocol::Shell.tab_index()
        ),
        None
    );
}

#[gpui::test]
fn new_local_connect_persists_session_in_store(cx: &mut gpui::TestAppContext) {
    use std::sync::{Arc, Mutex};

    cx.update(|app| {
        menubar::init(app);
        gpui_term::init(app);
        gpui_component::init(app);
    });

    let tmp_dir = std::env::temp_dir().join(format!(
        "termua-new-session-local-persist-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&tmp_dir).unwrap();
    let db_path = tmp_dir.join("termua").join("termua.db");
    let _guard = crate::store::tests::override_termua_db_path(db_path);

    let win = cx.add_empty_window();
    let view_slot: Arc<Mutex<Option<Entity<NewSessionWindow>>>> = Arc::new(Mutex::new(None));
    let view_slot_for_draw = Arc::clone(&view_slot);

    win.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(860.)),
            gpui::AvailableSpace::Definite(gpui::px(640.)),
        ),
        move |window, app| {
            let view = app.new(|cx| NewSessionWindow::new(window, cx));
            *view_slot_for_draw.lock().unwrap() = Some(view.clone());
            div().size_full().child(view)
        },
    );
    win.run_until_parked();

    let view = view_slot
        .lock()
        .unwrap()
        .clone()
        .expect("expected view to be captured");

    win.update(|_window, app| {
        view.read(app)
            .persist_new_local_session_for_connect(app)
            .expect("expected local session persistence to succeed");
    });
    win.run_until_parked();

    let sessions = crate::store::load_all_sessions()
        .unwrap()
        .into_iter()
        .filter(|s| s.protocol == crate::store::SessionType::Local)
        .collect::<Vec<_>>();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].group_path, "local");
    assert_eq!(sessions[0].label, "bash");
}

#[gpui::test]
fn new_local_connect_with_empty_label_and_group_enqueues_sidebar_reload_after_persist(
    cx: &mut gpui::TestAppContext,
) {
    use std::sync::{Arc, Mutex};

    struct DummyRootView;

    impl Render for DummyRootView {
        fn render(
            &mut self,
            _window: &mut gpui::Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            div()
        }
    }

    cx.update(|app| {
        menubar::init(app);
        gpui_term::init(app);
        gpui_component::init(app);
        app.set_global(crate::TermuaAppState::default());
    });

    let tmp_dir = std::env::temp_dir().join(format!(
        "termua-new-session-local-reload-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&tmp_dir).unwrap();
    let db_path = tmp_dir.join("termua").join("termua.db");
    let _guard = crate::store::tests::override_termua_db_path(db_path);

    let (_root, main_window_cx) = cx.add_window_view(|window, cx| {
        let view = cx.new(|_| DummyRootView);
        gpui_component::Root::new(view, window, cx)
    });
    main_window_cx.update(|window, app| {
        let root_handle = window
            .window_handle()
            .downcast::<gpui_component::Root>()
            .expect("expected Root window handle");
        app.global_mut::<crate::TermuaAppState>().main_window = Some(root_handle);
    });

    let win = cx.add_empty_window();
    let view_slot: Arc<Mutex<Option<Entity<NewSessionWindow>>>> = Arc::new(Mutex::new(None));
    let view_slot_for_draw = Arc::clone(&view_slot);

    win.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(860.)),
            gpui::AvailableSpace::Definite(gpui::px(640.)),
        ),
        move |window, app| {
            let view = app.new(|cx| NewSessionWindow::new(window, cx));
            *view_slot_for_draw.lock().unwrap() = Some(view.clone());
            div().size_full().child(view)
        },
    );
    win.run_until_parked();

    let view = view_slot
        .lock()
        .unwrap()
        .clone()
        .expect("expected view to be captured");

    win.update(|window, app| {
        view.update(app, |this, cx| {
            this.shell.common.label_input.update(cx, |input, cx| {
                input.set_value("", window, cx);
            });
            this.shell.common.group_input.update(cx, |input, cx| {
                input.set_value("", window, cx);
            });
        });

        let result = view.update(app, |this, cx| this.connect_new_session(cx));
        assert!(result.is_ok(), "expected local connect to succeed");
    });
    win.run_until_parked();

    let pending = win.update(|_window, app| {
        app.global::<crate::TermuaAppState>()
            .pending_commands
            .clone()
    });
    assert!(
        pending
            .iter()
            .any(|cmd| matches!(cmd, crate::PendingCommand::ReloadSessionsSidebar)),
        "expected persistence completion to enqueue a sidebar reload, got {pending:?}"
    );

    let sessions = crate::store::load_all_sessions()
        .unwrap()
        .into_iter()
        .filter(|s| s.protocol == crate::store::SessionType::Local)
        .collect::<Vec<_>>();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].group_path, "local");
    assert_eq!(sessions[0].label, "bash");
}

#[gpui::test]
fn new_session_connect_error_does_not_lock_submit_state(cx: &mut gpui::TestAppContext) {
    use std::sync::{Arc, Mutex};

    cx.update(|app| {
        menubar::init(app);
        gpui_term::init(app);
        gpui_component::init(app);
    });

    let win = cx.add_empty_window();
    let view_slot: Arc<Mutex<Option<Entity<NewSessionWindow>>>> = Arc::new(Mutex::new(None));
    let view_slot_for_draw = Arc::clone(&view_slot);

    win.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(860.)),
            gpui::AvailableSpace::Definite(gpui::px(640.)),
        ),
        move |window, app| {
            let view = app.new(|cx| NewSessionWindow::new(window, cx));
            *view_slot_for_draw.lock().unwrap() = Some(view.clone());
            div().size_full().child(view)
        },
    );
    win.run_until_parked();

    let view = view_slot
        .lock()
        .unwrap()
        .clone()
        .expect("expected view to be captured");

    win.update(|_window, app| {
        let result = view.update(app, |this, cx| this.connect_new_session(cx));
        assert!(
            result.is_err(),
            "expected connect to fail without main window"
        );
        assert!(
            app.global::<crate::TermuaAppState>()
                .pending_commands
                .is_empty(),
            "failed connect should not enqueue pending commands"
        );
        assert!(
            !view.read(app).submit_in_flight,
            "failed connect should not leave submit state locked"
        );
    });
}

#[gpui::test]
fn edit_session_repeat_save_is_ignored_while_submit_is_in_flight(cx: &mut gpui::TestAppContext) {
    use std::sync::{Arc, Mutex};

    cx.update(|app| {
        menubar::init(app);
        gpui_term::init(app);
        gpui_component::init(app);
    });

    let win = cx.add_empty_window();
    let view_slot: Arc<Mutex<Option<Entity<NewSessionWindow>>>> = Arc::new(Mutex::new(None));
    let view_slot_for_draw = Arc::clone(&view_slot);

    win.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(860.)),
            gpui::AvailableSpace::Definite(gpui::px(640.)),
        ),
        move |window, app| {
            let session = crate::store::Session {
                id: 1,
                protocol: crate::store::SessionType::Ssh,
                group_path: "ssh".to_string(),
                label: "prod".to_string(),
                backend: crate::settings::TerminalBackend::Wezterm,
                term: "xterm-256color".to_string(),
                charset: "UTF-8".to_string(),
                shell_program: None,
                ssh_host: Some("example.com".to_string()),
                ssh_port: Some(22),
                ssh_auth_type: Some(crate::store::SshAuthType::Password),
                ssh_user: Some("root".to_string()),
                ssh_credential_username: None,
                ssh_password: Some("pw".to_string()),
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
            };

            let view = app.new(|cx| NewSessionWindow::new_for_edit(session, window, cx));
            *view_slot_for_draw.lock().unwrap() = Some(view.clone());
            div().size_full().child(view)
        },
    );
    win.run_until_parked();

    let view = view_slot
        .lock()
        .unwrap()
        .clone()
        .expect("expected view to be captured");

    win.update(|window, app| {
        view.update(app, |this, _cx| {
            this.submit_in_flight = true;
        });

        let before = app.global::<crate::TermuaAppState>().pending_commands.len();
        let result = view.update(app, |this, cx| this.save_edit_session(window, cx));
        assert!(result.is_ok(), "repeat save should be ignored cleanly");
        assert_eq!(
            app.global::<crate::TermuaAppState>().pending_commands.len(),
            before,
            "ignored repeat save should not enqueue extra work"
        );
        assert!(
            view.read(app).submit_in_flight,
            "ignored repeat save should keep the current in-flight state"
        );
    });
}
