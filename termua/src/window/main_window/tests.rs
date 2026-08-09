use std::{
    borrow::Cow,
    collections::HashMap,
    ops::RangeInclusive,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use gpui::{
    AppContext, Bounds, Context, InteractiveElement, IntoElement, Keystroke, Modifiers,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, ScrollWheelEvent,
    SharedString, Styled, Window, div,
};
use gpui_component::input::InputState;
use gpui_dock::{DockPlacement, PanelView};
use gpui_term::{
    Authentication, CursorShape, Event as TerminalEvent, SshOptions, Terminal, TerminalBackend,
    TerminalBounds, TerminalType, TerminalView, UserInput as TerminalUserInput,
};
use rust_i18n::t;

use super::*;
use crate::{
    SshParams, TermuaAppState, ToggleSessionsSidebar, lock_screen,
    menu::Quit,
    notification,
    ssh::{SshHostKeyMismatchDetails, SshTerminalBuilderFn, SshTerminalFactory},
};

fn unique_workspace_settings_path(label: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock before unix epoch")
        .as_nanos();
    std::env::temp_dir()
        .join(format!("termua-main-window-{label}-{nanos}"))
        .join("settings.json")
}

fn add_fake_local_terminal(
    window_view: &mut TermuaWindow,
    backend: TerminalType,
    window: &mut Window,
    cx: &mut Context<TermuaWindow>,
) {
    add_fake_local_terminal_with_launch(window_view, backend, None, window, cx);
}

fn add_fake_local_terminal_with_launch(
    window_view: &mut TermuaWindow,
    backend: TerminalType,
    launch_state: Option<crate::panel::TerminalLaunchState>,
    window: &mut Window,
    cx: &mut Context<TermuaWindow>,
) {
    let id = window_view.next_terminal_id;
    window_view.next_terminal_id += 1;
    let terminal = cx.new(|_| {
        Terminal::new(
            backend,
            Box::new(FakeBackend::new(Arc::new(AtomicBool::new(false)))),
        )
    });
    let panel = window_view.build_wired_terminal_panel(
        id,
        crate::panel::PanelKind::Local,
        format!("terminal {id}").into(),
        None,
        launch_state,
        terminal,
        window,
        cx,
    );
    window_view.dock_area.update(cx, |dock, cx| {
        dock.add_panel(
            Arc::new(panel) as Arc<dyn PanelView>,
            DockPlacement::Center,
            None,
            window,
            cx,
        );
    });
}

#[gpui::test]
fn sftp_panel_opens_in_center_workspace_instead_of_bottom_dock(cx: &mut gpui::TestAppContext) {
    cx.update(|app| {
        gpui_component::init(app);
        menubar::init(app);
        gpui_term::init(app);
        gpui_dock::init(app);
        app.set_global(TermuaAppState::default());
    });

    let (view, window_cx) = cx.add_window_view(|window, cx| TermuaWindow::new(window, cx));
    window_cx.update(|window, cx| {
        view.update(cx, |this, cx| {
            add_fake_local_terminal(this, TerminalType::Alacritty, window, cx);
        });

        let sftp = cx.new(|cx| {
            crate::panel::sftp_panel::SftpDockPanel::restoring(
                crate::panel::sftp_panel::SftpPanelState {
                    version: 1,
                    tab_label: "SFTP test".to_string(),
                    terminal_id: 1,
                    current_dir: None,
                },
                cx,
            )
        });

        let panel = Arc::new(sftp) as Arc<dyn PanelView>;
        view.update(cx, |this, cx| {
            this.add_sftp_panel(panel.clone(), window, cx);
        });

        let dock = view.read(cx).dock_area.read(cx);
        assert!(dock.bottom_dock().is_none());
        assert!(dock.center().find_panel(panel.clone(), cx).is_some());

        let shared_tabs = dock
            .visible_tab_panels(cx)
            .into_iter()
            .find(|tabs| tabs.read(cx).panels().iter().any(|item| item == &panel))
            .expect("SFTP panel should be in a visible center tab group");
        assert!(shared_tabs.read(cx).panels().iter().any(|item| {
            item.view()
                .downcast::<crate::panel::TerminalPanel>()
                .is_ok()
        }));
    });
}

fn update_first_panel_state(
    value: &mut serde_json::Value,
    panel_name: &str,
    update: &mut impl FnMut(&mut serde_json::Value),
) -> bool {
    if value["panel_name"] == panel_name {
        update(value);
        return true;
    }
    match value {
        serde_json::Value::Array(values) => values
            .iter_mut()
            .any(|value| update_first_panel_state(value, panel_name, update)),
        serde_json::Value::Object(values) => values
            .values_mut()
            .any(|value| update_first_panel_state(value, panel_name, update)),
        _ => false,
    }
}

fn save_workspace_with_modified_fake_terminal(
    view: &gpui::Entity<TermuaWindow>,
    cx: &mut gpui::App,
    mut update: impl FnMut(&mut serde_json::Value),
) {
    let state = view.read(cx).dock_area.read(cx).dump(cx);
    let mut value = serde_json::to_value(state).expect("serialize workspace state");
    assert!(update_first_panel_state(
        &mut value,
        crate::panel::TERMINAL_PANEL_NAME,
        &mut update,
    ));
    crate::workspace::save_to_path(
        &crate::workspace::state_path(),
        serde_json::from_value(value).expect("deserialize modified workspace state"),
    )
    .expect("save modified workspace state");
}

#[gpui::test]
fn main_window_restores_saved_dock_layout(cx: &mut gpui::TestAppContext) {
    let settings_path = unique_workspace_settings_path("restore-layout");
    let _guard = crate::settings::override_settings_json_path(settings_path);

    cx.update(|app| {
        gpui_component::init(app);
        menubar::init(app);
        gpui_term::init(app);
        gpui_dock::init(app);
        app.set_global(TermuaAppState::default());
    });

    let (first, first_cx) = cx.add_window_view(|window, cx| TermuaWindow::new(window, cx));
    first_cx.update(|window, cx| {
        first.update(cx, |this, cx| {
            let dock_area = this.dock_area.clone();
            dock_area.update(cx, |dock, cx| {
                dock.set_version(crate::workspace::STATE_VERSION, window, cx);
                if dock.is_dock_open(gpui_dock::DockPlacement::Left, cx) {
                    dock.toggle_dock(gpui_dock::DockPlacement::Left, window, cx);
                }
                crate::workspace::save_to_path(&crate::workspace::state_path(), dock.dump(cx))
                    .expect("save workspace layout");
            });
        });
    });

    let (restored, restored_cx) = cx.add_window_view(|window, cx| TermuaWindow::new(window, cx));
    restored_cx.update(|_window, cx| {
        let is_left_open = restored
            .read(cx)
            .dock_area
            .read(cx)
            .is_dock_open(gpui_dock::DockPlacement::Left, cx);
        assert!(!is_left_open, "saved left dock should remain closed");
    });

    std::fs::remove_dir_all(crate::settings::settings_dir_path()).ok();
}

#[gpui::test]
fn main_window_unwraps_fixed_sidebars_from_saved_tab_panels(cx: &mut gpui::TestAppContext) {
    let settings_path = unique_workspace_settings_path("unwrap-fixed-sidebars");
    let _guard = crate::settings::override_settings_json_path(settings_path);

    cx.update(|app| {
        gpui_component::init(app);
        menubar::init(app);
        gpui_term::init(app);
        gpui_dock::init(app);
        app.set_global(TermuaAppState::default());
    });

    let (first, first_cx) = cx.add_window_view(|window, cx| TermuaWindow::new(window, cx));
    first_cx.update(|_window, cx| {
        let state = first.read(cx).dock_area.read(cx).dump(cx);
        let mut value = serde_json::to_value(state).expect("serialize workspace state");
        for dock_name in ["left_dock", "right_dock"] {
            let panel = value[dock_name]["panel"].take();
            value[dock_name]["panel"] = serde_json::json!({
                "panel_name": "TabPanel",
                "children": [panel],
                "info": { "tabs": { "active_index": 0 } }
            });
        }
        let state = serde_json::from_value(value).expect("deserialize wrapped workspace state");
        crate::workspace::save_to_path(&crate::workspace::state_path(), state)
            .expect("save wrapped workspace state");
    });

    let (restored, restored_cx) = cx.add_window_view(|window, cx| TermuaWindow::new(window, cx));
    restored_cx.update(|_window, cx| {
        let dock_area = restored.read(cx).dock_area.read(cx);
        assert!(matches!(
            dock_area.left_dock().expect("left dock").read(cx).panel(),
            gpui_dock::DockItem::Panel { .. }
        ));
        assert!(matches!(
            dock_area.right_dock().expect("right dock").read(cx).panel(),
            gpui_dock::DockItem::Panel { .. }
        ));
    });

    std::fs::remove_dir_all(crate::settings::settings_dir_path()).ok();
}

#[gpui::test]
fn main_window_saves_dock_layout_after_change(cx: &mut gpui::TestAppContext) {
    let settings_path = unique_workspace_settings_path("save-layout");
    let _guard = crate::settings::override_settings_json_path(settings_path);

    cx.update(|app| {
        gpui_component::init(app);
        menubar::init(app);
        gpui_term::init(app);
        gpui_dock::init(app);
        app.set_global(TermuaAppState::default());
    });

    let (view, window_cx) = cx.add_window_view(|window, cx| TermuaWindow::new(window, cx));
    window_cx.update(|window, cx| {
        view.update(cx, |this, cx| {
            this.dock_area.update(cx, |dock, cx| {
                dock.toggle_dock(gpui_dock::DockPlacement::Left, window, cx);
            });
        });
    });

    window_cx.run_until_parked();
    window_cx
        .executor()
        .advance_clock(Duration::from_millis(20));
    window_cx.run_until_parked();

    let saved = crate::workspace::load_from_path(&crate::workspace::state_path())
        .expect("layout change should save workspace state");
    assert_eq!(saved.version, Some(crate::workspace::STATE_VERSION));
    std::fs::remove_dir_all(crate::settings::settings_dir_path()).ok();
}

#[gpui::test]
fn main_window_restores_local_terminal_panel(cx: &mut gpui::TestAppContext) {
    let settings_path = unique_workspace_settings_path("restore-local-terminal");
    let _guard = crate::settings::override_settings_json_path(settings_path);

    cx.update(|app| {
        gpui_component::init(app);
        menubar::init(app);
        gpui_term::init(app);
        gpui_dock::init(app);
        app.set_global(TermuaAppState::default());
    });

    let restore_attempts = Arc::new(AtomicUsize::new(0));
    let restored_terminal_builder = {
        let restore_attempts = restore_attempts.clone();
        Arc::new(
            move |launch: &crate::panel::TerminalLaunchState, _id: usize| {
                restore_attempts.fetch_add(1, Ordering::SeqCst);
                let crate::panel::TerminalLaunchState::Local { backend_type, .. } = launch else {
                    anyhow::bail!("expected local terminal launch state");
                };
                Ok((
                    crate::panel::PanelKind::Local,
                    Box::new(FakeSshTerminalFactory {
                        backend: *backend_type,
                        recording_active: Arc::new(AtomicBool::new(false)),
                    }) as Box<dyn SshTerminalFactory>,
                ))
            },
        )
    };
    let ssh_terminal_builder: SshTerminalBuilderFn = Arc::new(|backend, _env, _opts| {
        Ok(Box::new(FakeSshTerminalFactory {
            backend,
            recording_active: Arc::new(AtomicBool::new(false)),
        }) as Box<dyn SshTerminalFactory>)
    });

    let first_restore_builder = restored_terminal_builder.clone();
    let first_ssh_builder = ssh_terminal_builder.clone();
    let (first, first_cx) = cx.add_window_view(move |window, cx| {
        TermuaWindow::new_with_terminal_builders(
            window,
            first_ssh_builder,
            first_restore_builder,
            cx,
        )
    });
    first_cx.update(|window, cx| {
        first.update(cx, |this, cx| {
            // Simulate a long-running app where many terminal IDs were previously consumed.
            this.next_terminal_id = 42;
            let env = HashMap::from([("TERMUA_SHELL".to_string(), "sh".to_string())]);
            add_fake_local_terminal_with_launch(
                this,
                TerminalType::WezTerm,
                Some(crate::panel::TerminalLaunchState::Local {
                    backend_type: TerminalType::WezTerm,
                    env,
                }),
                window,
                cx,
            );
            crate::workspace::save_to_path(
                &crate::workspace::state_path(),
                this.dock_area.read(cx).dump(cx),
            )
            .expect("save local terminal layout");
        });
    });

    let (restored, restored_cx) = cx.add_window_view(move |window, cx| {
        TermuaWindow::new_with_terminal_builders(
            window,
            ssh_terminal_builder,
            restored_terminal_builder,
            cx,
        )
    });
    restored_cx.update(|_window, cx| {
        let panel = restored
            .read(cx)
            .dock_area
            .read(cx)
            .visible_tab_panels(cx)
            .into_iter()
            .find_map(|tabs| tabs.read(cx).active_panel(cx))
            .expect("restored terminal tab");
        assert!(
            panel
                .view()
                .downcast::<crate::panel::TerminalPanel>()
                .is_ok(),
            "saved terminal panel should be rebuilt by its registered factory"
        );
        assert_eq!(
            restored.read(cx).next_terminal_id,
            1,
            "new tabs after restart should reuse the smallest available terminal ID"
        );
        let next_label = restored.update(cx, |this, _cx| {
            crate::panel::local_terminal_panel_tab_name(
                &HashMap::from([("TERMUA_SHELL".to_string(), "sh".to_string())]),
                this.next_terminal_id,
                &mut this.local_tab_label_counts,
            )
        });
        assert_eq!(
            next_label.as_ref(),
            "sh 2",
            "restored local tabs should contribute to per-shell label numbering"
        );
    });
    assert_eq!(restore_attempts.load(Ordering::SeqCst), 1);

    std::fs::remove_dir_all(crate::settings::settings_dir_path()).ok();
}

#[gpui::test]
fn main_window_reports_unsupported_saved_terminal_state(cx: &mut gpui::TestAppContext) {
    let settings_path = unique_workspace_settings_path("unsupported-terminal-state");
    let _guard = crate::settings::override_settings_json_path(settings_path);
    cx.update(|app| {
        gpui_component::init(app);
        menubar::init(app);
        gpui_term::init(app);
        gpui_dock::init(app);
        app.set_global(TermuaAppState::default());
    });

    let (first, first_cx) = cx.add_window_view(|window, cx| TermuaWindow::new(window, cx));
    first_cx.update(|window, cx| {
        first.update(cx, |this, cx| {
            add_fake_local_terminal(this, TerminalType::Alacritty, window, cx)
        });
        save_workspace_with_modified_fake_terminal(&first, cx, |panel| {
            panel["info"]["panel"]["version"] = serde_json::json!(usize::MAX);
        });
    });

    let (restored, restored_cx) = cx.add_window_view(|window, cx| TermuaWindow::new(window, cx));
    restored_cx.update(|_, cx| {
        let panel = restored.read(cx).dock_area.read(cx).visible_tab_panels(cx)[0]
            .read(cx)
            .active_panel(cx)
            .expect("restored error panel");
        assert!(
            panel
                .view()
                .downcast::<crate::panel::SshErrorPanel>()
                .is_ok()
        );
    });
    std::fs::remove_dir_all(crate::settings::settings_dir_path()).ok();
}

#[gpui::test]
fn main_window_reports_malformed_saved_terminal_state(cx: &mut gpui::TestAppContext) {
    let settings_path = unique_workspace_settings_path("malformed-terminal-state");
    let _guard = crate::settings::override_settings_json_path(settings_path);
    cx.update(|app| {
        gpui_component::init(app);
        menubar::init(app);
        gpui_term::init(app);
        gpui_dock::init(app);
        app.set_global(TermuaAppState::default());
    });

    let (first, first_cx) = cx.add_window_view(|window, cx| TermuaWindow::new(window, cx));
    first_cx.update(|window, cx| {
        first.update(cx, |this, cx| {
            add_fake_local_terminal(this, TerminalType::WezTerm, window, cx)
        });
        save_workspace_with_modified_fake_terminal(&first, cx, |panel| {
            panel["info"]["panel"] = serde_json::json!({ "invalid": true });
        });
    });

    let (restored, restored_cx) = cx.add_window_view(|window, cx| TermuaWindow::new(window, cx));
    restored_cx.update(|_, cx| {
        let panel = restored.read(cx).dock_area.read(cx).visible_tab_panels(cx)[0]
            .read(cx)
            .active_panel(cx)
            .expect("restored error panel");
        assert!(
            panel
                .view()
                .downcast::<crate::panel::SshErrorPanel>()
                .is_ok()
        );
    });
    std::fs::remove_dir_all(crate::settings::settings_dir_path()).ok();
}

#[gpui::test]
fn main_window_reports_malformed_saved_sftp_state(cx: &mut gpui::TestAppContext) {
    let settings_path = unique_workspace_settings_path("malformed-sftp-state");
    let _guard = crate::settings::override_settings_json_path(settings_path);
    cx.update(|app| {
        gpui_component::init(app);
        menubar::init(app);
        gpui_term::init(app);
        gpui_dock::init(app);
        app.set_global(TermuaAppState::default());
    });

    let (first, first_cx) = cx.add_window_view(|window, cx| TermuaWindow::new(window, cx));
    first_cx.update(|window, cx| {
        first.update(cx, |this, cx| {
            add_fake_local_terminal(this, TerminalType::Alacritty, window, cx)
        });
        save_workspace_with_modified_fake_terminal(&first, cx, |panel| {
            panel["panel_name"] = serde_json::json!(crate::panel::SFTP_PANEL_NAME);
            panel["info"]["panel"] = serde_json::json!({ "invalid": true });
        });
    });

    let (restored, restored_cx) = cx.add_window_view(|window, cx| TermuaWindow::new(window, cx));
    restored_cx.update(|_, cx| {
        let panel = restored.read(cx).dock_area.read(cx).visible_tab_panels(cx)[0]
            .read(cx)
            .active_panel(cx)
            .expect("restored error panel");
        assert!(
            panel
                .view()
                .downcast::<crate::panel::SshErrorPanel>()
                .is_ok()
        );
    });
    std::fs::remove_dir_all(crate::settings::settings_dir_path()).ok();
}

#[gpui::test]
fn footbar_backend_tracks_opened_terminal(cx: &mut gpui::TestAppContext) {
    cx.update(|app| {
        gpui_component::init(app);
        menubar::init(app);
        gpui_term::init(app);
        gpui_dock::init(app);
        app.set_global(TermuaAppState::default());
        app.activate(true);
    });

    let (view, window_cx) = cx.add_window_view(|window, cx| TermuaWindow::new(window, cx));
    let root = view.clone();
    window_cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(900.)),
            gpui::AvailableSpace::Definite(gpui::px(600.)),
        ),
        move |_, _| div().size_full().child(root),
    );
    window_cx.run_until_parked();
    window_cx.update(|window, cx| {
        view.update(cx, |this, cx| {
            add_fake_local_terminal(this, TerminalType::Alacritty, window, cx);
        });
    });
    window_cx.run_until_parked();
    window_cx.update(|_, app| {
        assert_eq!(
            app.global::<crate::footbar::FocusedTerminalBackendState>()
                .backend(),
            Some(TerminalType::Alacritty)
        );
    });
    assert!(window_cx.debug_bounds("termua-footbar-backend").is_some());
    assert!(
        window_cx
            .debug_bounds("termua-footbar-backend-image")
            .is_some()
    );
    let backend = window_cx
        .debug_bounds("termua-footbar-backend")
        .expect("backend icon");
    let issues = window_cx
        .debug_bounds("termua-footbar-issues")
        .expect("issues icon");
    assert!(
        backend.origin.x < issues.origin.x,
        "backend icon should be before the right-side buttons"
    );

    window_cx.deactivate_window();
    window_cx.run_until_parked();
    window_cx.update(|_, app| {
        assert_eq!(
            app.global::<crate::footbar::FocusedTerminalBackendState>()
                .backend(),
            Some(TerminalType::Alacritty),
            "app deactivation must not clear the active terminal tab backend"
        );
    });
    assert!(window_cx.debug_bounds("termua-footbar-backend").is_some());

    window_cx.update(|window, cx| {
        view.update(cx, |this, cx| {
            add_fake_local_terminal(this, TerminalType::WezTerm, window, cx);
        });
    });
    window_cx.run_until_parked();
    window_cx.update(|_, app| {
        assert_eq!(
            app.global::<crate::footbar::FocusedTerminalBackendState>()
                .backend(),
            Some(TerminalType::WezTerm)
        );
    });
    assert!(window_cx.debug_bounds("termua-footbar-backend").is_some());
    assert!(
        window_cx
            .debug_bounds("termua-footbar-backend-image")
            .is_some()
    );
}

#[gpui::test]
fn main_window_reconnects_saved_ssh_terminal_panel(cx: &mut gpui::TestAppContext) {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let settings_path = unique_workspace_settings_path("restore-ssh-terminal");
    let _settings_guard = crate::settings::override_settings_json_path(settings_path);
    let db_path = crate::store::tests::unique_test_db_path("restore-ssh-terminal");
    let _db_guard = crate::store::tests::override_termua_db_path(db_path);
    let session_id = crate::store::save_ssh_session_config(
        "ssh",
        "prod",
        crate::settings::TerminalBackend::Wezterm,
        "example.com",
        22,
        "xterm-256color",
        "UTF-8",
    )
    .expect("save ssh session");

    cx.update(|app| {
        gpui_component::init(app);
        menubar::init(app);
        gpui_term::init(app);
        gpui_dock::init(app);
        app.set_global(TermuaAppState::default());
    });

    let attempts = Arc::new(AtomicUsize::new(0));
    let builder: SshTerminalBuilderFn = {
        let attempts = attempts.clone();
        Arc::new(move |backend, _env, _opts| {
            attempts.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(FakeSshTerminalFactory {
                backend,
                recording_active: Arc::new(AtomicBool::new(false)),
            }) as Box<dyn SshTerminalFactory>)
        })
    };

    let (first, first_cx) = cx.add_window_view(|window, cx| {
        TermuaWindow::new_with_ssh_terminal_builder(window, builder.clone(), cx)
    });
    first_cx.update(|window, cx| {
        first.update(cx, |this, cx| {
            this.open_session_by_id(session_id, window, cx);
        });
    });
    for _ in 0..20 {
        first_cx.run_until_parked();
        if attempts.load(Ordering::SeqCst) >= 1 {
            break;
        }
    }
    first_cx.run_until_parked();
    first_cx.update(|_window, cx| {
        crate::workspace::save_to_path(
            &crate::workspace::state_path(),
            first.read(cx).dock_area.read(cx).dump(cx),
        )
        .expect("save ssh terminal layout");
    });

    let (restored, restored_cx) = cx.add_window_view(|window, cx| {
        TermuaWindow::new_with_ssh_terminal_builder(window, builder, cx)
    });
    for _ in 0..30 {
        restored_cx.run_until_parked();
        let restored_ssh = restored_cx.update(|_window, cx| {
            restored
                .read(cx)
                .dock_area
                .read(cx)
                .visible_tab_panels(cx)
                .into_iter()
                .flat_map(|tabs| tabs.read(cx).panels().to_vec())
                .filter_map(|panel| panel.view().downcast::<crate::panel::TerminalPanel>().ok())
                .any(|panel| panel.read(cx).kind() == crate::panel::PanelKind::Ssh)
        });
        if restored_ssh {
            break;
        }
    }

    assert!(attempts.load(Ordering::SeqCst) >= 2);
    let restored_ssh = restored_cx.update(|_window, cx| {
        restored
            .read(cx)
            .dock_area
            .read(cx)
            .visible_tab_panels(cx)
            .into_iter()
            .flat_map(|tabs| tabs.read(cx).panels().to_vec())
            .filter_map(|panel| panel.view().downcast::<crate::panel::TerminalPanel>().ok())
            .any(|panel| panel.read(cx).kind() == crate::panel::PanelKind::Ssh)
    });
    assert!(
        restored_ssh,
        "saved SSH tab should reconnect as a terminal panel"
    );

    std::fs::remove_dir_all(crate::settings::settings_dir_path()).ok();
}

#[gpui::test]
fn main_window_restores_right_sidebar_stable_state(cx: &mut gpui::TestAppContext) {
    let settings_path = unique_workspace_settings_path("restore-right-sidebar");
    let _guard = crate::settings::override_settings_json_path(settings_path);

    cx.update(|app| {
        gpui_component::init(app);
        menubar::init(app);
        gpui_term::init(app);
        gpui_dock::init(app);
        app.set_global(TermuaAppState::default());
    });

    let (first, first_cx) = cx.add_window_view(|window, cx| TermuaWindow::new(window, cx));
    first_cx.update(|_window, cx| {
        cx.global_mut::<crate::right_sidebar::RightSidebarState>()
            .active_tab = crate::right_sidebar::RightSidebarTab::Assistant;
        cx.global_mut::<crate::assistant::AssistantState>()
            .push(crate::assistant::AssistantRole::User, "remember this");
        crate::workspace::save_to_path(
            &crate::workspace::state_path(),
            first.read(cx).dock_area.read(cx).dump(cx),
        )
        .expect("save right sidebar state");

        cx.global_mut::<crate::right_sidebar::RightSidebarState>()
            .active_tab = crate::right_sidebar::RightSidebarTab::Notifications;
        cx.global_mut::<crate::assistant::AssistantState>().clear();
    });

    let (_restored, restored_cx) = cx.add_window_view(|window, cx| TermuaWindow::new(window, cx));
    restored_cx.update(|_window, cx| {
        assert_eq!(
            cx.global::<crate::right_sidebar::RightSidebarState>()
                .active_tab,
            crate::right_sidebar::RightSidebarTab::Assistant
        );
        let messages = &cx.global::<crate::assistant::AssistantState>().messages;
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content.as_ref(), "remember this");
    });

    std::fs::remove_dir_all(crate::settings::settings_dir_path()).ok();
}

#[test]
fn terminal_context_menu_labels_follow_the_active_locale() {
    let _guard = crate::locale::lock();
    crate::locale::set_locale("zh-CN");

    assert_eq!(t!("Terminal.ContextMenu.Recording"), "录制");
    assert_eq!(t!("Terminal.ContextMenu.RecordingActive"), "录制中");
    assert_eq!(t!("Terminal.ContextMenu.Copy"), "复制");
    assert_eq!(t!("Terminal.ContextMenu.Paste"), "粘贴");
    assert_eq!(t!("Terminal.ContextMenu.SelectAll"), "全选");
    assert_eq!(t!("Terminal.ContextMenu.Clear"), "清空");
}

#[cfg_attr(target_os = "macos", ignore)]
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
                SshParams {
                    env: HashMap::new(),
                    name: "prod".to_string(),
                    opts: SshOptions {
                        host: "127.0.0.1".to_string(),
                        port: Some(22),
                        auth: Authentication::Config,
                        proxy: gpui_term::SshProxyMode::Inherit,
                        backend: gpui_term::SshBackend::default(),
                        tcp_nodelay: false,
                        tcp_keepalive: false,
                    },
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

#[cfg_attr(target_os = "macos", ignore)]
#[gpui::test]
fn web_control_request_dialog_renders_source_and_security_notice(cx: &mut gpui::TestAppContext) {
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
        let (decision_tx, _decision_rx) = smol::channel::bounded(1);
        termua.update(app, |this, cx| {
            this.open_web_control_request_dialog(
                "192.168.1.20:54321".parse().unwrap(),
                decision_tx,
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
        "termua-web-control-dialog-title",
        "termua-web-control-dialog-source",
        "termua-web-control-dialog-notice",
        "termua-web-control-dialog-deny",
        "termua-web-control-dialog-allow",
    ] {
        assert!(
            cx.debug_bounds(selector).is_some(),
            "expected {selector} to be rendered"
        );
    }

    let source_height = cx
        .debug_bounds("termua-web-control-dialog-source")
        .unwrap()
        .size
        .height;
    let notice_height = cx
        .debug_bounds("termua-web-control-dialog-notice")
        .unwrap()
        .size
        .height;
    assert!(
        source_height <= gpui::px(76.) && notice_height <= gpui::px(76.),
        "expected compact detail text: source={source_height:?}, notice={notice_height:?}"
    );
}

#[cfg_attr(target_os = "macos", ignore)]
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

#[cfg_attr(target_os = "macos", ignore)]
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

#[cfg_attr(target_os = "macos", ignore)]
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
                SshParams {
                    env: HashMap::new(),
                    name: "prod".to_string(),
                    opts: SshOptions {
                        host: "example.com".to_string(),
                        port: Some(22),
                        auth: Authentication::Password("alice".to_string(), "pw".to_string()),
                        proxy: gpui_term::SshProxyMode::Inherit,
                        backend: gpui_term::SshBackend::default(),
                        tcp_nodelay: false,
                        tcp_keepalive: false,
                    },
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
                SshParams {
                    env: HashMap::new(),
                    name: "prod".to_string(),
                    opts: SshOptions {
                        host: "example.com".to_string(),
                        port: Some(22),
                        auth: Authentication::Password("alice".to_string(), "pw".to_string()),
                        proxy: gpui_term::SshProxyMode::Inherit,
                        backend: gpui_term::SshBackend::default(),
                        tcp_nodelay: false,
                        tcp_keepalive: false,
                    },
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

    let db_path = crate::store::tests::unique_test_db_path("ssh-sidebar-connecting-cleared");
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
fn recorder_terminal_view_enables_context_menu(cx: &mut gpui::TestAppContext) {
    cx.update(|app| {
        gpui_component::init(app);
        gpui_dock::init(app);
        gpui_term::init(app);
        app.set_global(TermuaAppState::default());
    });

    let cx = cx.add_empty_window();
    cx.update(|window, cx| {
        let termua = cx.new(|cx| TermuaWindow::new(window, cx));
        let active = Arc::new(AtomicBool::new(false));
        let term = cx.new(|_| {
            Terminal::new(
                TerminalType::WezTerm,
                Box::new(FakeBackend::new(Arc::clone(&active))),
            )
        });
        let terminal_view = termua.update(cx, |this, cx| {
            this.create_terminal_view(crate::panel::PanelKind::Recorder, term, window, cx)
        });
        assert!(terminal_view.read(cx).context_menu_enabled());
    });
}

#[gpui::test]
#[cfg_attr(target_os = "macos", ignore)]
fn recorder_terminal_view_supports_copy_and_select_all_shortcuts(cx: &mut gpui::TestAppContext) {
    use std::{cell::RefCell, rc::Rc};

    cx.update(|app| {
        gpui_component::init(app);
        gpui_dock::init(app);
        gpui_term::init(app);
        crate::menu::bind_menu_shortcuts(app);
        app.set_global(TermuaAppState::default());
    });

    let copy_count = Arc::new(AtomicUsize::new(0));
    let select_all_count = Arc::new(AtomicUsize::new(0));
    let copy_count_for_window = Arc::clone(&copy_count);
    let select_all_count_for_window = Arc::clone(&select_all_count);
    let terminal_view_slot = Rc::new(RefCell::new(None));
    let terminal_view_slot_for_window = Rc::clone(&terminal_view_slot);
    let (_root, window_cx) = cx.add_window_view(move |window, cx| {
        let termua = cx.new(|cx| TermuaWindow::new(window, cx));
        let term = cx.new(|_| {
            Terminal::new(
                TerminalType::WezTerm,
                Box::new(FakeBackend::with_action_counts(
                    Arc::new(AtomicBool::new(false)),
                    copy_count_for_window,
                    select_all_count_for_window,
                )),
            )
        });
        let terminal_view = termua.update(cx, |this, cx| {
            this.create_terminal_view(crate::panel::PanelKind::Recorder, term, window, cx)
        });
        *terminal_view_slot_for_window.borrow_mut() = Some(terminal_view.clone());
        gpui_component::Root::new(terminal_view, window, cx)
    });
    let terminal_view = terminal_view_slot
        .borrow()
        .clone()
        .expect("expected recorder terminal view");

    window_cx.update(|window, cx| {
        let focus_handle = terminal_view.read(cx).focus_handle.clone();
        window.focus(&focus_handle, cx);
    });
    window_cx.run_until_parked();
    window_cx.update(|window, cx| {
        let focus_handle = terminal_view.read(cx).focus_handle.clone();
        assert!(
            window
                .highest_precedence_binding_for_action_in(
                    &crate::ToggleAssistantSidebar,
                    &focus_handle,
                )
                .is_none(),
            "assistant shortcut must not compete with terminal shortcuts"
        );
    });
    #[cfg(target_os = "macos")]
    {
        window_cx.simulate_keystrokes("cmd-c");
        window_cx.simulate_keystrokes("cmd-a");
    }
    #[cfg(not(target_os = "macos"))]
    {
        window_cx.simulate_keystrokes("ctrl-shift-c");
        window_cx.simulate_keystrokes("ctrl-shift-a");
    }

    assert_eq!(copy_count.load(Ordering::SeqCst), 1);
    assert_eq!(select_all_count.load(Ordering::SeqCst), 1);
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
    let overlay_bounds = window
        .debug_bounds("termua-lock-overlay")
        .expect("expected lock overlay bounds");
    assert_eq!(overlay_bounds.origin.y, gpui_component::TITLE_BAR_HEIGHT);
    assert!(
        window.debug_bounds("termua-window-titlebar").is_some(),
        "titlebar should remain visible while locked so window controls remain available"
    );
    assert!(
        window.debug_bounds("foldable-app-menu-bar").is_none(),
        "in-window menu should be hidden while locked"
    );
    assert!(window.debug_bounds("termua-lock-password-input").is_some());
}

#[cfg_attr(target_os = "macos", ignore)]
#[gpui::test]
fn close_terminal_event_closes_local_terminal_tab(cx: &mut gpui::TestAppContext) {
    use std::{cell::RefCell, rc::Rc};

    use gpui_dock::{DockPlacement, PanelView};

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

    let terminal = window_cx.update(|window, app| {
        let recording_active = Arc::new(AtomicBool::new(false));
        let terminal = app.new(|_cx| {
            Terminal::new(
                TerminalType::WezTerm,
                Box::new(FakeBackend::new(recording_active.clone())),
            )
        });
        let terminal_view = app.new(|cx| TerminalView::new(terminal.clone(), window, cx));
        let panel = app.new(|_| {
            crate::panel::TerminalPanel::new(
                42,
                crate::panel::PanelKind::Local,
                "bash".into(),
                None,
                terminal_view,
            )
        });

        termua.update(app, |this, cx| {
            this.subscribe_terminal_events_for_messages(
                terminal.clone(),
                42,
                "bash".into(),
                window,
                cx,
            );
            this.dock_area.update(cx, |dock, cx| {
                dock.add_panel(
                    Arc::new(panel) as Arc<dyn PanelView>,
                    DockPlacement::Center,
                    None,
                    window,
                    cx,
                );
            });
        });

        terminal
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

    let terminal_tabs_before = window_cx.update(|_window, app| {
        termua
            .read(app)
            .dock_area
            .read(app)
            .visible_tab_panels(app)
            .into_iter()
            .filter_map(|tab_panel| tab_panel.read(app).active_panel(app))
            .filter(|panel| {
                panel
                    .view()
                    .downcast::<crate::panel::TerminalPanel>()
                    .is_ok()
            })
            .count()
    });
    assert_eq!(
        terminal_tabs_before, 1,
        "expected one terminal tab before close event"
    );

    window_cx.update(|_window, app| {
        app.set_global(crate::footbar::FocusedTerminalBackendState::focused(
            42,
            TerminalType::WezTerm,
        ));
        terminal.update(app, |_terminal, cx| {
            cx.emit(TerminalEvent::CloseTerminal);
        });
    });
    window_cx.run_until_parked();

    let terminal_tabs_after = window_cx.update(|_window, app| {
        termua
            .read(app)
            .dock_area
            .read(app)
            .visible_tab_panels(app)
            .into_iter()
            .filter_map(|tab_panel| tab_panel.read(app).active_panel(app))
            .filter(|panel| {
                panel
                    .view()
                    .downcast::<crate::panel::TerminalPanel>()
                    .is_ok()
            })
            .count()
    });
    assert_eq!(
        terminal_tabs_after, 0,
        "expected close event on local terminal to remove the terminal tab"
    );
    window_cx.update(|_, app| {
        assert_eq!(
            app.global::<crate::footbar::FocusedTerminalBackendState>()
                .backend(),
            None,
            "closing the focused terminal should hide its backend icon"
        );
    });
}

#[cfg_attr(target_os = "macos", ignore)]
#[gpui::test]
fn exited_ssh_terminal_closes_on_second_ctrl_d(cx: &mut gpui::TestAppContext) {
    use std::{cell::RefCell, rc::Rc};

    use gpui::Keystroke;
    use gpui_dock::{DockPlacement, PanelView};

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

    let terminal_view = window_cx.update(|window, app| {
        let recording_active = Arc::new(AtomicBool::new(false));
        let terminal = app.new(|_cx| {
            Terminal::new(
                TerminalType::WezTerm,
                Box::new(FakeBackend::with_exited(recording_active.clone(), true)),
            )
        });
        let terminal_view = app.new(|cx| TerminalView::new(terminal.clone(), window, cx));
        let panel = app.new(|_| {
            crate::panel::TerminalPanel::new(
                77,
                crate::panel::PanelKind::Ssh,
                "ssh demo".into(),
                None,
                terminal_view.clone(),
            )
        });

        termua.update(app, |this, cx| {
            this.subscribe_terminal_events_for_messages(
                terminal.clone(),
                77,
                "ssh demo".into(),
                window,
                cx,
            );
            this.subscribe_terminal_view_events(&terminal_view, window, cx);
            this.dock_area.update(cx, |dock, cx| {
                dock.add_panel(
                    Arc::new(panel) as Arc<dyn PanelView>,
                    DockPlacement::Center,
                    None,
                    window,
                    cx,
                );
            });
        });

        terminal_view
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

    window_cx.update(|_window, app| {
        let terminal = terminal_view.read(app).terminal.clone();
        terminal.update(app, |_terminal, cx| {
            cx.emit(TerminalEvent::CloseTerminal);
        });
    });
    window_cx.run_until_parked();

    let terminal_tabs_after_exit = window_cx.update(|_window, app| {
        termua
            .read(app)
            .dock_area
            .read(app)
            .visible_tab_panels(app)
            .into_iter()
            .filter_map(|tab_panel| tab_panel.read(app).active_panel(app))
            .filter(|panel| {
                panel
                    .view()
                    .downcast::<crate::panel::TerminalPanel>()
                    .is_ok()
            })
            .count()
    });
    assert_eq!(
        terminal_tabs_after_exit, 1,
        "expected close event on exited ssh terminal to stay open"
    );

    window_cx.update(|_window, app| {
        terminal_view.update(app, |_view, cx| {
            cx.emit(TerminalEvent::UserInput(TerminalUserInput::Keystroke(
                Keystroke::parse("ctrl-d").unwrap(),
            )));
        });
    });
    window_cx.run_until_parked();

    let terminal_tabs_after = window_cx.update(|_window, app| {
        termua
            .read(app)
            .dock_area
            .read(app)
            .visible_tab_panels(app)
            .into_iter()
            .filter_map(|tab_panel| tab_panel.read(app).active_panel(app))
            .filter(|panel| {
                panel
                    .view()
                    .downcast::<crate::panel::TerminalPanel>()
                    .is_ok()
            })
            .count()
    });
    assert_eq!(
        terminal_tabs_after, 0,
        "expected exited ssh terminal to close on Ctrl-D"
    );
}

#[cfg_attr(target_os = "macos", ignore)]
#[gpui::test]
fn close_terminal_event_keeps_recorder_tab_open(cx: &mut gpui::TestAppContext) {
    use std::{cell::RefCell, rc::Rc};

    use gpui_dock::{DockPlacement, PanelView};

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

    let terminal = window_cx.update(|window, app| {
        let recording_active = Arc::new(AtomicBool::new(false));
        let terminal = app.new(|_cx| {
            Terminal::new(
                TerminalType::WezTerm,
                Box::new(FakeBackend::with_exited(recording_active.clone(), true)),
            )
        });
        let terminal_view = app.new(|cx| TerminalView::new(terminal.clone(), window, cx));
        let panel = app.new(|_| {
            crate::panel::TerminalPanel::new(
                79,
                crate::panel::PanelKind::Recorder,
                "recorder 79".into(),
                None,
                terminal_view,
            )
        });

        termua.update(app, |this, cx| {
            this.subscribe_terminal_events_for_messages(
                terminal.clone(),
                79,
                "recorder 79".into(),
                window,
                cx,
            );
            this.dock_area.update(cx, |dock, cx| {
                dock.add_panel(
                    Arc::new(panel) as Arc<dyn PanelView>,
                    DockPlacement::Center,
                    None,
                    window,
                    cx,
                );
            });
        });

        terminal
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

    window_cx.update(|_window, app| {
        terminal.update(app, |_terminal, cx| {
            cx.emit(TerminalEvent::CloseTerminal);
        });
    });
    window_cx.run_until_parked();

    let terminal_tabs_after = window_cx.update(|_window, app| {
        termua
            .read(app)
            .dock_area
            .read(app)
            .visible_tab_panels(app)
            .into_iter()
            .filter_map(|tab_panel| tab_panel.read(app).active_panel(app))
            .filter(|panel| {
                panel
                    .view()
                    .downcast::<crate::panel::TerminalPanel>()
                    .is_ok()
            })
            .count()
    });
    assert_eq!(
        terminal_tabs_after, 1,
        "expected recorder tab to stay open after playback exit"
    );
}

#[cfg_attr(target_os = "macos", ignore)]
#[gpui::test]
fn exited_recorder_tab_closes_on_ctrl_d(cx: &mut gpui::TestAppContext) {
    use std::{cell::RefCell, rc::Rc};

    use gpui::Keystroke;
    use gpui_dock::{DockPlacement, PanelView};

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

    let terminal_view = window_cx.update(|window, app| {
        let recording_active = Arc::new(AtomicBool::new(false));
        let terminal = app.new(|_cx| {
            Terminal::new(
                TerminalType::WezTerm,
                Box::new(FakeBackend::with_exited(recording_active.clone(), true)),
            )
        });
        let terminal_view = app.new(|cx| TerminalView::new(terminal.clone(), window, cx));
        let panel = app.new(|_| {
            crate::panel::TerminalPanel::new(
                79,
                crate::panel::PanelKind::Recorder,
                "recorder 79".into(),
                None,
                terminal_view.clone(),
            )
        });

        termua.update(app, |this, cx| {
            this.subscribe_terminal_events_for_messages(
                terminal,
                79,
                "recorder 79".into(),
                window,
                cx,
            );
            this.subscribe_terminal_view_events(&terminal_view, window, cx);
            this.dock_area.update(cx, |dock, cx| {
                dock.add_panel(
                    Arc::new(panel) as Arc<dyn PanelView>,
                    DockPlacement::Center,
                    None,
                    window,
                    cx,
                );
            });
        });

        terminal_view
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

    window_cx.update(|_window, app| {
        terminal_view.update(app, |_view, cx| {
            cx.emit(TerminalEvent::UserInput(TerminalUserInput::Keystroke(
                Keystroke::parse("ctrl-d").unwrap(),
            )));
        });
    });
    window_cx.run_until_parked();

    let terminal_tabs_after = window_cx.update(|_window, app| {
        termua
            .read(app)
            .dock_area
            .read(app)
            .visible_tab_panels(app)
            .into_iter()
            .filter_map(|tab_panel| tab_panel.read(app).active_panel(app))
            .filter(|panel| {
                panel
                    .view()
                    .downcast::<crate::panel::TerminalPanel>()
                    .is_ok()
            })
            .count()
    });
    assert_eq!(
        terminal_tabs_after, 0,
        "expected exited recorder tab to close on Ctrl-D"
    );
}

#[cfg_attr(target_os = "macos", ignore)]
#[gpui::test]
fn active_ssh_terminal_does_not_close_on_first_ctrl_d(cx: &mut gpui::TestAppContext) {
    use std::{cell::RefCell, rc::Rc};

    use gpui::Keystroke;
    use gpui_dock::{DockPlacement, PanelView};

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

    let terminal_view = window_cx.update(|window, app| {
        let recording_active = Arc::new(AtomicBool::new(false));
        let terminal = app.new(|_cx| {
            Terminal::new(
                TerminalType::WezTerm,
                Box::new(FakeBackend::with_exited(recording_active.clone(), false)),
            )
        });
        let terminal_view = app.new(|cx| TerminalView::new(terminal.clone(), window, cx));
        let panel = app.new(|_| {
            crate::panel::TerminalPanel::new(
                78,
                crate::panel::PanelKind::Ssh,
                "ssh demo".into(),
                None,
                terminal_view.clone(),
            )
        });

        termua.update(app, |this, cx| {
            this.subscribe_terminal_view_events(&terminal_view, window, cx);
            this.dock_area.update(cx, |dock, cx| {
                dock.add_panel(
                    Arc::new(panel) as Arc<dyn PanelView>,
                    DockPlacement::Center,
                    None,
                    window,
                    cx,
                );
            });
        });

        terminal_view
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

    window_cx.update(|_window, app| {
        terminal_view.update(app, |_view, cx| {
            cx.emit(TerminalEvent::UserInput(TerminalUserInput::Keystroke(
                Keystroke::parse("ctrl-d").unwrap(),
            )));
        });
    });
    window_cx.run_until_parked();

    let terminal_tabs_after = window_cx.update(|_window, app| {
        termua
            .read(app)
            .dock_area
            .read(app)
            .visible_tab_panels(app)
            .into_iter()
            .filter_map(|tab_panel| tab_panel.read(app).active_panel(app))
            .filter(|panel| {
                panel
                    .view()
                    .downcast::<crate::panel::TerminalPanel>()
                    .is_ok()
            })
            .count()
    });
    assert_eq!(
        terminal_tabs_after, 1,
        "expected active ssh terminal to stay open on first Ctrl-D"
    );
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

#[cfg_attr(target_os = "macos", ignore)]
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

#[cfg_attr(target_os = "macos", ignore)]
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

#[cfg_attr(target_os = "macos", ignore)]
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

#[cfg_attr(target_os = "macos", ignore)]
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

#[cfg_attr(target_os = "macos", ignore)]
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
        after.size.width >= gpui::px(220.0),
        "expected right sidebar width to be clamped to >= 220px, got {:?}",
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

    let db_path = crate::store::tests::unique_test_db_path("sessions-click-through-fullscreen");
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
        Box::leak(format!("termua-sessions-session-row-{session_id_1}").into_boxed_str());
    let row_selector_2: &'static str =
        Box::leak(format!("termua-sessions-session-row-{session_id_2}").into_boxed_str());

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

#[cfg_attr(target_os = "macos", ignore)]
#[gpui::test]
fn ssh_sessions_with_missing_password_show_a_notification(cx: &mut gpui::TestAppContext) {
    use std::{cell::RefCell, rc::Rc};

    cx.update(|app| {
        gpui_component::init(app);
        menubar::init(app);
        gpui_term::init(app);
        gpui_dock::init(app);
        app.set_global(TermuaAppState::default());
    });

    let db_path = crate::store::tests::unique_test_db_path("missing-password");
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

    let termua_slot: Rc<RefCell<Option<gpui::Entity<TermuaWindow>>>> = Rc::new(RefCell::new(None));
    let slot_for_root = termua_slot.clone();
    let (root, cx) = cx.add_window_view(|window, cx| {
        let view = cx.new(|cx| TermuaWindow::new(window, cx));
        *slot_for_root.borrow_mut() = Some(view.clone());
        gpui_component::Root::new(view, window, cx)
    });
    let termua = termua_slot
        .borrow()
        .as_ref()
        .expect("expected TermuaWindow view to be captured")
        .clone();

    cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(900.)),
            gpui::AvailableSpace::Definite(gpui::px(600.)),
        ),
        move |_, _| div().size_full().child(root),
    );
    cx.run_until_parked();

    cx.update(|window, app| {
        termua.update(app, |this, cx| {
            this.open_session_by_id(id, window, cx);
        });
    });
    cx.run_until_parked();

    cx.update(|_window, app| {
        let notifications = app.global::<notification::NotifyState>().messages.clone();
        assert!(
            !notifications.is_empty(),
            "expected a notification when an ssh password session is missing its password"
        );
    });
}

struct FakeBackend {
    content: gpui_term::TerminalContent,
    recording_active: Arc<AtomicBool>,
    copy_count: Arc<AtomicUsize>,
    select_all_count: Arc<AtomicUsize>,
    exited: bool,
}

struct FakeSshTerminalFactory {
    backend: TerminalType,
    recording_active: Arc<AtomicBool>,
}

impl SshTerminalFactory for FakeSshTerminalFactory {
    fn build(self: Box<Self>, _cx: &mut Context<Terminal>) -> Terminal {
        Terminal::new(
            self.backend,
            Box::new(FakeBackend::new(self.recording_active)),
        )
    }
}

impl FakeBackend {
    fn new(recording_active: Arc<AtomicBool>) -> Self {
        Self::with_exited(recording_active, false)
    }

    fn with_exited(recording_active: Arc<AtomicBool>, exited: bool) -> Self {
        Self {
            content: gpui_term::TerminalContent::default(),
            recording_active,
            copy_count: Arc::new(AtomicUsize::new(0)),
            select_all_count: Arc::new(AtomicUsize::new(0)),
            exited,
        }
    }

    fn with_action_counts(
        recording_active: Arc<AtomicBool>,
        copy_count: Arc<AtomicUsize>,
        select_all_count: Arc<AtomicUsize>,
    ) -> Self {
        Self {
            content: gpui_term::TerminalContent::default(),
            recording_active,
            copy_count,
            select_all_count,
            exited: false,
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

    fn has_exited(&self) -> bool {
        self.exited
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

    fn select_all(&mut self) {
        self.select_all_count.fetch_add(1, Ordering::SeqCst);
    }

    fn copy(&mut self, _keep_selection: Option<bool>, _cx: &mut Context<Terminal>) {
        self.copy_count.fetch_add(1, Ordering::SeqCst);
    }

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
