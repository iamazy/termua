use std::time::Duration;

use gpui::{AppContext, ParentElement, Styled, div};
use gpui_component::{ActiveTheme, WindowExt, input::InputState, select::SearchableVec};

use super::{
    state::{build_nav_tree_items, sidebar_nav_specs},
    *,
};

#[test]
fn search_matches_case_insensitively_on_title_and_keywords() {
    let entries = SettingMeta::all();
    let results = search_settings(entries, "FoNt");
    assert!(!results.is_empty());
    assert!(results.iter().any(|r| r.id == "terminal.font_family"));
    assert!(results.iter().any(|r| r.id == "terminal.font_size"));
}

#[test]
fn setting_meta_falls_back_to_embedded_copy_when_translation_missing() {
    let _guard = crate::locale::lock();
    crate::locale::set_locale("en");

    let meta = SettingMeta {
        id: "terminal.fake_missing_translation",
        title: "Fallback Title",
        description: "Fallback Description",
        keywords: &[],
        section: SettingsNavSection::Terminal,
        page: SettingsPage::TerminalFont,
    };

    assert_eq!(meta.localized_title(), "Fallback Title");
    assert_eq!(meta.localized_description(), "Fallback Description");
}

#[test]
fn assistant_settings_are_present_in_settings_meta_and_sidebar() {
    let entries = SettingMeta::all();
    assert!(
        entries.iter().any(|m| m.id == "assistant.provider"),
        "expected assistant.provider to exist in SettingMeta::all"
    );
    assert!(
        entries.iter().any(|m| m.id == "assistant.model"),
        "expected assistant.model to exist in SettingMeta::all"
    );

    let groups = sidebar_nav_specs();
    assert!(
        groups
            .iter()
            .any(|g| g.section == SettingsNavSection::Assistant),
        "expected Assistant group in settings sidebar"
    );

    assert!(
        SettingsWindow::supports_setting_id("assistant.provider"),
        "expected SettingsWindow to support rendering assistant.provider"
    );
    assert!(
        SettingsWindow::supports_setting_id("assistant.api_key"),
        "expected SettingsWindow to support rendering assistant.api_key"
    );
}

#[gpui::test]
fn assistant_model_dropdown_does_not_render_inline_fetch_error_below_control(
    cx: &mut gpui::TestAppContext,
) {
    use std::{cell::RefCell, rc::Rc};

    cx.update(|app| {
        gpui_component::init(app);
        menubar::init(app);
        gpui_term::init(app);
    });

    let settings_entity_slot: Rc<RefCell<Option<gpui::Entity<SettingsWindow>>>> =
        Rc::new(RefCell::new(None));
    let slot_for_window = settings_entity_slot.clone();

    let (root, window_cx) = cx.add_window_view(move |window, cx| {
        let settings = cx.new(|cx| SettingsWindow::new(window, cx));
        *slot_for_window.borrow_mut() = Some(settings.clone());
        gpui_component::Root::new(settings, window, cx)
    });

    let settings = settings_entity_slot
        .borrow()
        .clone()
        .expect("settings view should be created");

    window_cx.update(|window, cx| {
        settings.update(cx, |this, cx| {
            this.selected_page = SettingsPage::Assistant;
            this.assistant_model_fetch_error = Some("boom".into());
            cx.notify();
            window.refresh();
        });
    });

    window_cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(900.)),
            gpui::AvailableSpace::Definite(gpui::px(700.)),
        ),
        move |_, _| div().size_full().child(root),
    );
    window_cx.run_until_parked();

    window_cx
        .debug_bounds("termua-settings-assistant-model-dropdown-button")
        .expect("expected assistant model dropdown to be rendered");

    assert!(
        window_cx
            .debug_bounds("termua-settings-assistant-model-inline-error")
            .is_none(),
        "expected assistant model dropdown to not render fetch errors inline beneath the control"
    );
}

#[gpui::test]
fn assistant_model_dropdown_filters_options_from_typed_query(cx: &mut gpui::TestAppContext) {
    use std::{cell::RefCell, rc::Rc};

    cx.update(|app| {
        gpui_component::init(app);
        menubar::init(app);
        gpui_term::init(app);
    });

    let settings_entity_slot: Rc<RefCell<Option<gpui::Entity<SettingsWindow>>>> =
        Rc::new(RefCell::new(None));
    let slot_for_window = settings_entity_slot.clone();

    let (root, window_cx) = cx.add_window_view(move |window, cx| {
        let settings = cx.new(|cx| SettingsWindow::new(window, cx));
        *slot_for_window.borrow_mut() = Some(settings.clone());
        gpui_component::Root::new(settings, window, cx)
    });

    let settings = settings_entity_slot
        .borrow()
        .clone()
        .expect("settings view should be created");

    window_cx.update(|window, cx| {
        settings.update(cx, |this, cx| {
            this.selected_page = SettingsPage::Assistant;
            this.settings.assistant.provider = Some("openai".into());
            this.settings.assistant.model = Some("gpt-5.4".into());
            this.assistant_model_candidates =
                vec!["gpt-5.4".into(), "gpt-4.1-mini".into(), "o4-mini".into()];
            this.assistant_model_select.update(cx, |select, cx| {
                select.set_items(
                    SearchableVec::new(vec![
                        super::state::AssistantModelSelectItem::default_item(),
                        super::state::AssistantModelSelectItem::for_model("gpt-5.4".into()),
                        super::state::AssistantModelSelectItem::for_model("gpt-4.1-mini".into()),
                        super::state::AssistantModelSelectItem::for_model("o4-mini".into()),
                    ]),
                    window,
                    cx,
                );
                select.set_selected_value(&gpui::SharedString::from("gpt-5.4"), window, cx);
            });
            cx.notify();
            window.refresh();
        });
    });

    window_cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(900.)),
            gpui::AvailableSpace::Definite(gpui::px(700.)),
        ),
        move |_, _| div().size_full().child(root),
    );
    window_cx.run_until_parked();

    let bounds = window_cx
        .debug_bounds("termua-settings-assistant-model-dropdown-button")
        .expect("expected assistant model dropdown to be rendered");
    window_cx.simulate_click(bounds.center(), gpui::Modifiers::none());
    window_cx.run_until_parked();

    window_cx
        .debug_bounds("termua-settings-assistant-model-option-gpt_5_4")
        .expect("expected assistant model option to render before filtering");

    window_cx.simulate_keystrokes("enter");
    window_cx.run_until_parked();
    window_cx.simulate_input("__definitely_no_matching_model__");
    window_cx.run_until_parked();

    assert!(
        window_cx
            .debug_bounds("termua-settings-assistant-model-option-gpt_5_4")
            .is_none(),
        "expected assistant model options to be filtered by the typed query"
    );
}

#[test]
fn lock_screen_settings_are_present_in_settings_meta_and_sidebar() {
    let entries = SettingMeta::all();
    assert!(
        entries.iter().any(|m| m.id == "lock_screen.enabled"),
        "expected lock_screen.enabled to exist in SettingMeta::all"
    );
    assert!(
        entries.iter().any(|m| m.id == "lock_screen.timeout_secs"),
        "expected lock_screen.timeout_secs to exist in SettingMeta::all"
    );

    let groups = sidebar_nav_specs();
    assert!(
        groups
            .iter()
            .any(|g| g.section == SettingsNavSection::Security),
        "expected Security group in settings sidebar"
    );

    assert!(
        SettingsWindow::supports_setting_id("lock_screen.enabled"),
        "expected SettingsWindow to support rendering lock_screen.enabled"
    );
    assert!(
        SettingsWindow::supports_setting_id("lock_screen.timeout_secs"),
        "expected SettingsWindow to support rendering lock_screen.timeout_secs"
    );
}

#[test]
fn terminal_suggestions_settings_are_present_in_settings_meta_and_supported() {
    let entries = SettingMeta::all();
    assert!(
        entries
            .iter()
            .any(|m| m.id == "terminal.suggestions_enabled"),
        "expected terminal.suggestions_enabled to exist in SettingMeta::all"
    );
    assert!(
        entries
            .iter()
            .any(|m| m.id == "terminal.suggestions_max_items"),
        "expected terminal.suggestions_max_items to exist in SettingMeta::all"
    );
    assert!(
        entries
            .iter()
            .any(|m| m.id == "terminal.suggestions_json_dir"),
        "expected terminal.suggestions_json_dir to exist in SettingMeta::all"
    );

    assert!(
        SettingsWindow::supports_setting_id("terminal.suggestions_enabled"),
        "expected SettingsWindow to support rendering terminal.suggestions_enabled"
    );
    assert!(
        SettingsWindow::supports_setting_id("terminal.suggestions_max_items"),
        "expected SettingsWindow to support rendering terminal.suggestions_max_items"
    );
    assert!(
        SettingsWindow::supports_setting_id("terminal.suggestions_json_dir"),
        "expected SettingsWindow to support rendering terminal.suggestions_json_dir"
    );
}

#[test]
fn terminal_ligatures_setting_is_present_in_settings_meta_and_supported() {
    let entries = SettingMeta::all();
    assert!(
        entries.iter().any(|m| m.id == "terminal.ligatures"),
        "expected terminal.ligatures to exist in SettingMeta::all"
    );
    assert!(
        SettingsWindow::supports_setting_id("terminal.ligatures"),
        "expected SettingsWindow to support rendering terminal.ligatures"
    );
}

#[test]
fn terminal_font_fallback_setting_is_not_present_in_settings_meta_or_supported() {
    let entries = SettingMeta::all();
    assert!(
        !entries.iter().any(|m| m.id == "terminal.font_fallbacks"),
        "expected terminal.font_fallbacks to be absent from SettingMeta::all"
    );
    assert!(
        !SettingsWindow::supports_setting_id("terminal.font_fallbacks"),
        "expected SettingsWindow to not support rendering terminal.font_fallbacks"
    );
}

#[gpui::test]
fn settings_window_is_wrapped_in_gpui_component_root(cx: &mut gpui::TestAppContext) {
    let handle = {
        let mut app = cx.app.borrow_mut();
        menubar::init(&mut app);
        gpui_term::init(&mut app);
        SettingsWindow::open(&mut app).unwrap()
    };

    handle
        .update(cx, |_, window, _cx| {
            assert!(window.root::<gpui_component::Root>().flatten().is_some());
            assert!(window.root::<SettingsWindow>().flatten().is_none());
        })
        .unwrap();
}

#[gpui::test]
fn settings_window_lock_password_input_accepts_text(cx: &mut gpui::TestAppContext) {
    use gpui_component::WindowExt;

    // Point settings.json to a temp directory so this test is hermetic.
    let tmp_dir = std::env::temp_dir().join(format!(
        "termua-settings-test-lock-screen-input-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&tmp_dir).unwrap();

    let path = tmp_dir.join("termua").join("settings.json");
    let _guard = crate::settings::override_settings_json_path(path.clone());
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }

    cx.update(|app| {
        gpui_component::init(app);
        menubar::init(app);
        gpui_term::init(app);
        app.set_global(crate::lock_screen::LockState::new_for_test(
            Duration::from_secs(60),
        ));
    });

    let (root, cx) = cx.add_window_view(|window, cx| {
        let settings = cx.new(|cx| SettingsWindow::new(window, cx));
        gpui_component::Root::new(settings, window, cx)
    });

    cx.update(|_window, app| {
        app.global_mut::<crate::lock_screen::LockState>()
            .force_lock_for_test();
    });

    cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(900.)),
            gpui::AvailableSpace::Definite(gpui::px(700.)),
        ),
        move |_, _| div().size_full().child(root),
    );
    cx.run_until_parked();

    let input_bounds = cx
        .debug_bounds("termua-lock-password-input")
        .expect("lock password input should exist");
    cx.simulate_click(input_bounds.center(), gpui::Modifiers::none());
    cx.run_until_parked();

    cx.simulate_input("pw");

    let value = cx.update(|window, app| {
        let Some(input) = window.focused_input(app) else {
            panic!("expected lock password input to be focused");
        };
        let input: gpui::Entity<InputState> = input;
        input.read(app).value().to_string()
    });

    assert_eq!(value, "pw");
}

#[gpui::test]
fn settings_window_incorrect_password_clears_lock_input(cx: &mut gpui::TestAppContext) {
    use std::sync::Arc;

    use gpui_component::WindowExt;

    struct FakeAuthenticator;

    impl crate::lock_screen::Authenticator for FakeAuthenticator {
        fn verify_password(&self, password: &str) -> anyhow::Result<bool> {
            Ok(password == "pw")
        }
    }

    // Point settings.json to a temp directory so this test is hermetic.
    let tmp_dir = std::env::temp_dir().join(format!(
        "termua-settings-test-lock-screen-wrong-password-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&tmp_dir).unwrap();

    let path = tmp_dir.join("termua").join("settings.json");
    let _guard = crate::settings::override_settings_json_path(path.clone());
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }

    cx.update(|app| {
        gpui_component::init(app);
        menubar::init(app);
        gpui_term::init(app);
        app.set_global(crate::lock_screen::LockState::new_for_test_with_auth(
            Duration::from_secs(60),
            Arc::new(FakeAuthenticator),
        ));
    });

    let (root, cx) = cx.add_window_view(|window, cx| {
        let settings = cx.new(|cx| SettingsWindow::new(window, cx));
        gpui_component::Root::new(settings, window, cx)
    });

    cx.update(|_window, app| {
        app.global_mut::<crate::lock_screen::LockState>()
            .force_lock_for_test();
    });

    cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(900.)),
            gpui::AvailableSpace::Definite(gpui::px(700.)),
        ),
        move |_, _| div().size_full().child(root),
    );
    cx.run_until_parked();

    let input_bounds = cx
        .debug_bounds("termua-lock-password-input")
        .expect("lock password input should exist");
    cx.simulate_click(input_bounds.center(), gpui::Modifiers::none());
    cx.run_until_parked();

    cx.update(|window, app| {
        let Some(input) = window.focused_input(app) else {
            panic!("expected lock password input to be focused");
        };
        let input: gpui::Entity<InputState> = input;
        input.update(app, |state, cx| state.set_value("bad", window, cx));
    });
    cx.run_until_parked();
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();

    let value = cx.update(|window, app| {
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
fn settings_window_titlebar_shows_settings_icon(cx: &mut gpui::TestAppContext) {
    cx.update(|app| {
        gpui_component::init(app);
        menubar::init(app);
        gpui_term::init(app);
        app.set_global(crate::notification::NotifyState::default());
    });

    let cx = cx.add_empty_window();
    cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(800.)),
            gpui::AvailableSpace::Definite(gpui::px(600.)),
        ),
        |window, app| {
            let view = app.new(|cx| SettingsWindow::new(window, cx));
            div().size_full().child(view)
        },
    );

    cx.run_until_parked();
    assert!(cx.debug_bounds("termua-settings-titlebar-icon").is_some());
}

#[gpui::test]
fn appearance_theme_page_renders_palette_icon_in_heading(cx: &mut gpui::TestAppContext) {
    // Point settings.json to a temp directory so this test is hermetic.
    let tmp_dir = std::env::temp_dir().join(format!(
        "termua-settings-test-appearance-theme-page-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&tmp_dir).unwrap();

    // Write a settings.json that selects "Appearance / Theme" as the last settings page.
    let path = tmp_dir.join("termua").join("settings.json");
    let _guard = crate::settings::override_settings_json_path(path.clone());
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(
        &path,
        r#"{
              "ui": { "last_settings_page": "nav.page.appearance.theme" }
            }"#,
    )
    .unwrap();

    cx.update(|app| {
        gpui_component::init(app);
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
            let view = app.new(|cx| SettingsWindow::new(window, cx));
            assert_eq!(view.read(app).selected_page, SettingsPage::AppearanceTheme);
            div().size_full().child(view)
        },
    );

    cx.run_until_parked();
    assert!(
        cx.debug_bounds("termua-settings-appearance-theme-heading-icon")
            .is_some()
    );
}

#[gpui::test]
fn appearance_theme_page_renders_new_theme_button(cx: &mut gpui::TestAppContext) {
    // Point settings.json to a temp directory so this test is hermetic.
    let tmp_dir = std::env::temp_dir().join(format!(
        "termua-settings-test-appearance-theme-new-theme-button-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&tmp_dir).unwrap();

    // Write a settings.json that selects "Appearance / Theme" as the last settings page.
    let path = tmp_dir.join("termua").join("settings.json");
    let _guard = crate::settings::override_settings_json_path(path.clone());
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(
        &path,
        r#"{
              "ui": { "last_settings_page": "nav.page.appearance.theme" }
            }"#,
    )
    .unwrap();

    cx.update(|app| {
        gpui_component::init(app);
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
            let view = app.new(|cx| SettingsWindow::new(window, cx));
            assert_eq!(view.read(app).selected_page, SettingsPage::AppearanceTheme);
            div().size_full().child(view)
        },
    );

    cx.run_until_parked();
    assert!(
        cx.debug_bounds("termua-settings-appearance-theme-new-theme-button")
            .is_some()
    );
}

#[gpui::test]
fn assistant_page_renders_zeroclaw_controls(cx: &mut gpui::TestAppContext) {
    // Point settings.json to a temp directory so this test is hermetic.
    let tmp_dir = std::env::temp_dir().join(format!(
        "termua-settings-test-assistant-page-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&tmp_dir).unwrap();

    // Write a settings.json that selects "Assistant / ZeroClaw" as the last settings page.
    let path = tmp_dir.join("termua").join("settings.json");
    let _guard = crate::settings::override_settings_json_path(path.clone());
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(
        &path,
        r#"{
              "ui": { "last_settings_page": "nav.page.assistant.zeroclaw" }
            }"#,
    )
    .unwrap();

    cx.update(|app| {
        gpui_component::init(app);
        menubar::init(app);
        gpui_term::init(app);
    });

    let cx = cx.add_empty_window();
    cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(900.)),
            gpui::AvailableSpace::Definite(gpui::px(700.)),
        ),
        |window, app| {
            let view = app.new(|cx| SettingsWindow::new(window, cx));
            assert_eq!(view.read(app).selected_page, SettingsPage::Assistant);
            div().size_full().child(view)
        },
    );

    cx.run_until_parked();
    assert!(
        cx.debug_bounds("termua-settings-assistant-enabled-switch")
            .is_some(),
        "expected assistant enabled switch to exist"
    );
    assert!(
        cx.debug_bounds("termua-settings-assistant-status-indicator")
            .is_some(),
        "expected assistant status indicator to exist"
    );
    assert!(
        cx.debug_bounds("termua-settings-assistant-provider-select")
            .is_some(),
        "expected assistant provider select to exist"
    );
    assert!(
        cx.debug_bounds("termua-settings-assistant-model-dropdown-button")
            .is_some(),
        "expected assistant model dropdown button to exist"
    );
}

#[gpui::test]
fn lock_screen_page_renders_controls(cx: &mut gpui::TestAppContext) {
    // Point settings.json to a temp directory so this test is hermetic.
    let tmp_dir = std::env::temp_dir().join(format!(
        "termua-settings-test-lock-screen-page-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&tmp_dir).unwrap();

    // Write a settings.json that selects "Security / Lock screen" as the last settings page.
    let path = tmp_dir.join("termua").join("settings.json");
    let _guard = crate::settings::override_settings_json_path(path.clone());
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(
        &path,
        r#"{
              "ui": { "last_settings_page": "nav.page.security.lock_screen" }
            }"#,
    )
    .unwrap();

    cx.update(|app| {
        gpui_component::init(app);
        menubar::init(app);
        gpui_term::init(app);
        app.set_global(crate::lock_screen::LockState::new_for_test(
            Duration::from_secs(60),
        ));
    });

    let cx = cx.add_empty_window();
    cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(900.)),
            gpui::AvailableSpace::Definite(gpui::px(700.)),
        ),
        |window, app| {
            let view = app.new(|cx| SettingsWindow::new(window, cx));
            assert_eq!(
                view.read(app).selected_page,
                SettingsPage::SecurityLockScreen
            );
            div().size_full().child(view)
        },
    );

    cx.run_until_parked();
    assert!(
        cx.debug_bounds("termua-settings-lock-screen-enabled-switch")
            .is_some(),
        "expected lock screen enabled switch to exist"
    );
    assert!(
        cx.debug_bounds("termua-settings-lock-screen-timeout-dropdown")
            .is_some(),
        "expected lock screen timeout dropdown to exist"
    );
}

#[gpui::test]
fn recording_page_renders_playback_speed_as_select(cx: &mut gpui::TestAppContext) {
    let tmp_dir = std::env::temp_dir().join(format!(
        "termua-settings-test-recording-page-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&tmp_dir).unwrap();

    let path = tmp_dir.join("termua").join("settings.json");
    let _guard = crate::settings::override_settings_json_path(path.clone());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        r#"{
  "ui": {
    "last_settings_page": "nav.page.recording.cast"
  }
}"#,
    )
    .unwrap();

    cx.update(|app| {
        gpui_component::init(app);
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
            let view = app.new(|cx| SettingsWindow::new(window, cx));
            assert_eq!(view.read(app).selected_page, SettingsPage::RecordingCast);
            div().size_full().child(view)
        },
    );

    cx.run_until_parked();
    assert!(
        cx.debug_bounds("termua-settings-recording-playback-speed-select")
            .is_some(),
        "expected playback speed select to exist"
    );
    let speed_bounds = cx
        .debug_bounds("termua-settings-recording-playback-speed-select")
        .expect("expected playback speed select bounds");
    assert!(
        speed_bounds.size.width <= gpui::px(140.),
        "expected playback speed select to be narrower, got {:?}",
        speed_bounds.size.width
    );
    assert!(
        cx.debug_bounds("termua-settings-recording-playback-speed-input")
            .is_none(),
        "did not expect playback speed text input to exist"
    );
}

#[gpui::test]
fn clicking_new_theme_button_opens_theme_editor_sheet(cx: &mut gpui::TestAppContext) {
    // Point settings.json to a temp directory so this test is hermetic.
    let tmp_dir = std::env::temp_dir().join(format!(
        "termua-settings-test-theme-editor-sheet-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&tmp_dir).unwrap();

    // Write a settings.json that selects "Appearance / Theme" as the last settings page.
    let path = tmp_dir.join("termua").join("settings.json");
    let _guard = crate::settings::override_settings_json_path(path.clone());
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(
        &path,
        r#"{
              "ui": { "last_settings_page": "nav.page.appearance.theme" }
            }"#,
    )
    .unwrap();

    cx.update(|app| {
        gpui_component::init(app);
        menubar::init(app);
        gpui_term::init(app);
    });

    let (view, cx) = cx.add_window_view(|window, cx| {
        let settings = cx.new(|cx| SettingsWindow::new(window, cx));
        gpui_component::Root::new(settings, window, cx)
    });
    cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(900.)),
            gpui::AvailableSpace::Definite(gpui::px(700.)),
        ),
        move |_, _| div().size_full().child(view),
    );

    cx.run_until_parked();

    let bounds = cx
        .debug_bounds("termua-settings-appearance-theme-new-theme-button")
        .expect("new theme button should exist");
    cx.simulate_click(bounds.center(), gpui::Modifiers::none());
    cx.run_until_parked();

    assert!(
        cx.debug_bounds("termua-theme-editor-sheet").is_some(),
        "expected theme editor sheet to open"
    );
}

#[gpui::test]
fn theme_editor_sheet_lists_background_color(cx: &mut gpui::TestAppContext) {
    // Point settings.json to a temp directory so this test is hermetic.
    let tmp_dir = std::env::temp_dir().join(format!(
        "termua-settings-test-theme-editor-sheet-background-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&tmp_dir).unwrap();

    // Write a settings.json that selects "Appearance / Theme" as the last settings page.
    let path = tmp_dir.join("termua").join("settings.json");
    let _guard = crate::settings::override_settings_json_path(path.clone());
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(
        &path,
        r#"{
              "ui": { "last_settings_page": "nav.page.appearance.theme" }
            }"#,
    )
    .unwrap();

    cx.update(|app| {
        gpui_component::init(app);
        menubar::init(app);
        gpui_term::init(app);
    });

    let (root, cx) = cx.add_window_view(|window, cx| {
        let settings = cx.new(|cx| SettingsWindow::new(window, cx));
        gpui_component::Root::new(settings, window, cx)
    });
    cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(900.)),
            gpui::AvailableSpace::Definite(gpui::px(700.)),
        ),
        move |_, _| div().size_full().child(root),
    );

    cx.run_until_parked();

    let bounds = cx
        .debug_bounds("termua-settings-appearance-theme-new-theme-button")
        .expect("new theme button should exist");
    cx.simulate_click(bounds.center(), gpui::Modifiers::none());
    cx.run_until_parked();

    assert!(
        cx.debug_bounds("termua-theme-editor-color-background")
            .is_some(),
        "expected theme editor to list background color"
    );
}

#[gpui::test]
fn theme_editor_preview_restores_theme_on_close(cx: &mut gpui::TestAppContext) {
    use gpui_component::Colorize as _;

    // Point settings.json to a temp directory so this test is hermetic.
    let tmp_dir = std::env::temp_dir().join(format!(
        "termua-settings-test-theme-editor-preview-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&tmp_dir).unwrap();

    // Write a settings.json that selects "Appearance / Theme" as the last settings page.
    let path = tmp_dir.join("termua").join("settings.json");
    let _guard = crate::settings::override_settings_json_path(path.clone());
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(
        &path,
        r#"{
              "ui": { "last_settings_page": "nav.page.appearance.theme" }
            }"#,
    )
    .unwrap();

    cx.update(|app| {
        gpui_component::init(app);
        menubar::init(app);
        gpui_term::init(app);
    });

    let (root, cx) = cx.add_window_view(|window, cx| {
        let settings = cx.new(|cx| SettingsWindow::new(window, cx));
        gpui_component::Root::new(settings, window, cx)
    });
    cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(900.)),
            gpui::AvailableSpace::Definite(gpui::px(700.)),
        ),
        move |_, _| div().size_full().child(root),
    );

    cx.run_until_parked();

    let original_background = cx.update(|_window, app| app.theme().background.to_hex());

    // Open theme editor.
    let bounds = cx
        .debug_bounds("termua-settings-appearance-theme-new-theme-button")
        .expect("new theme button should exist");
    cx.simulate_click(bounds.center(), gpui::Modifiers::none());
    cx.run_until_parked();

    // Update background color (preview).
    let input_bounds = cx
        .debug_bounds("termua-theme-editor-color-background-input")
        .expect("background input should exist");
    cx.simulate_click(input_bounds.center(), gpui::Modifiers::none());
    cx.update(|window, app| {
        let Some(input) = window.focused_input(app) else {
            panic!("expected background input to be focused");
        };
        input.update(app, |state, cx| state.set_value("#000000ff", window, cx));
    });
    cx.run_until_parked();

    let preview_background = cx.update(|_window, app| app.theme().background.to_hex());
    assert_ne!(
        preview_background, original_background,
        "expected preview to change background"
    );

    // Close sheet and ensure theme is restored.
    let cancel_bounds = cx
        .debug_bounds("termua-theme-editor-cancel-button")
        .expect("cancel button should exist");
    cx.simulate_click(cancel_bounds.center(), gpui::Modifiers::none());
    cx.run_until_parked();

    let restored_background = cx.update(|_window, app| app.theme().background.to_hex());
    assert_eq!(
        restored_background, original_background,
        "expected theme to be restored after closing theme editor"
    );
}

#[gpui::test]
fn theme_editor_sheet_lists_muted_color(cx: &mut gpui::TestAppContext) {
    // Point settings.json to a temp directory so this test is hermetic.
    let tmp_dir = std::env::temp_dir().join(format!(
        "termua-settings-test-theme-editor-sheet-muted-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&tmp_dir).unwrap();

    // Write a settings.json that selects "Appearance / Theme" as the last settings page.
    let path = tmp_dir.join("termua").join("settings.json");
    let _guard = crate::settings::override_settings_json_path(path.clone());
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(
        &path,
        r#"{
              "ui": { "last_settings_page": "nav.page.appearance.theme" }
            }"#,
    )
    .unwrap();

    cx.update(|app| {
        gpui_component::init(app);
        menubar::init(app);
        gpui_term::init(app);
    });

    let (root, cx) = cx.add_window_view(|window, cx| {
        let settings = cx.new(|cx| SettingsWindow::new(window, cx));
        gpui_component::Root::new(settings, window, cx)
    });
    cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(900.)),
            gpui::AvailableSpace::Definite(gpui::px(700.)),
        ),
        move |_, _| div().size_full().child(root),
    );

    cx.run_until_parked();

    let bounds = cx
        .debug_bounds("termua-settings-appearance-theme-new-theme-button")
        .expect("new theme button should exist");
    cx.simulate_click(bounds.center(), gpui::Modifiers::none());
    cx.run_until_parked();

    assert!(
        cx.debug_bounds("termua-theme-editor-color-muted").is_some(),
        "expected theme editor to list muted color"
    );
}

#[gpui::test]
fn theme_editor_preserves_previous_changes_across_fields(cx: &mut gpui::TestAppContext) {
    use gpui_component::Colorize as _;

    // Point settings.json to a temp directory so this test is hermetic.
    let tmp_dir = std::env::temp_dir().join(format!(
        "termua-settings-test-theme-editor-multi-field-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&tmp_dir).unwrap();

    // Write a settings.json that selects "Appearance / Theme" as the last settings page.
    let path = tmp_dir.join("termua").join("settings.json");
    let _guard = crate::settings::override_settings_json_path(path.clone());
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(
        &path,
        r#"{
              "ui": { "last_settings_page": "nav.page.appearance.theme" }
            }"#,
    )
    .unwrap();

    cx.update(|app| {
        gpui_component::init(app);
        menubar::init(app);
        gpui_term::init(app);
    });

    let (root, cx) = cx.add_window_view(|window, cx| {
        let settings = cx.new(|cx| SettingsWindow::new(window, cx));
        gpui_component::Root::new(settings, window, cx)
    });
    cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(900.)),
            gpui::AvailableSpace::Definite(gpui::px(700.)),
        ),
        move |_, _| div().size_full().child(root),
    );

    cx.run_until_parked();

    // Open theme editor.
    let bounds = cx
        .debug_bounds("termua-settings-appearance-theme-new-theme-button")
        .expect("new theme button should exist");
    cx.simulate_click(bounds.center(), gpui::Modifiers::none());
    cx.run_until_parked();

    // Update background.
    let background_input_bounds = cx
        .debug_bounds("termua-theme-editor-color-background-input")
        .expect("background input should exist");
    cx.simulate_click(background_input_bounds.center(), gpui::Modifiers::none());
    cx.update(|window, app| {
        let Some(input) = window.focused_input(app) else {
            panic!("expected background input to be focused");
        };
        input.update(app, |state, cx| state.set_value("#000000ff", window, cx));
    });
    cx.run_until_parked();

    let background_after_first_change = cx.update(|_window, app| app.theme().background.to_hex());

    // Update muted and ensure background stays the same.
    let muted_input_bounds = cx
        .debug_bounds("termua-theme-editor-color-muted-input")
        .expect("muted input should exist");
    cx.simulate_click(muted_input_bounds.center(), gpui::Modifiers::none());
    cx.update(|window, app| {
        let Some(input) = window.focused_input(app) else {
            panic!("expected muted input to be focused");
        };
        input.update(app, |state, cx| state.set_value("#ffffff", window, cx));
    });
    cx.run_until_parked();

    let background_after_second_change = cx.update(|_window, app| app.theme().background.to_hex());

    assert_eq!(
        background_after_second_change, background_after_first_change,
        "expected background preview to be preserved when changing other fields"
    );
}

#[gpui::test]
fn theme_editor_renders_color_swatch_for_background(cx: &mut gpui::TestAppContext) {
    // Point settings.json to a temp directory so this test is hermetic.
    let tmp_dir = std::env::temp_dir().join(format!(
        "termua-settings-test-theme-editor-swatch-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&tmp_dir).unwrap();

    // Write a settings.json that selects "Appearance / Theme" as the last settings page.
    let path = tmp_dir.join("termua").join("settings.json");
    let _guard = crate::settings::override_settings_json_path(path.clone());
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(
        &path,
        r#"{
              "ui": { "last_settings_page": "nav.page.appearance.theme" }
            }"#,
    )
    .unwrap();

    cx.update(|app| {
        gpui_component::init(app);
        menubar::init(app);
        gpui_term::init(app);
    });

    let (root, cx) = cx.add_window_view(|window, cx| {
        let settings = cx.new(|cx| SettingsWindow::new(window, cx));
        gpui_component::Root::new(settings, window, cx)
    });
    cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(900.)),
            gpui::AvailableSpace::Definite(gpui::px(700.)),
        ),
        move |_, _| div().size_full().child(root),
    );

    cx.run_until_parked();

    // Open theme editor.
    let bounds = cx
        .debug_bounds("termua-settings-appearance-theme-new-theme-button")
        .expect("new theme button should exist");
    cx.simulate_click(bounds.center(), gpui::Modifiers::none());
    cx.run_until_parked();

    assert!(
        cx.debug_bounds("termua-theme-editor-color-background-swatch")
            .is_some(),
        "expected a color swatch for background"
    );
}

#[gpui::test]
fn theme_editor_long_color_labels_do_not_wrap(cx: &mut gpui::TestAppContext) {
    use std::rc::Rc;

    // Point settings.json to a temp directory so this test is hermetic.
    let tmp_dir = std::env::temp_dir().join(format!(
        "termua-settings-test-theme-editor-long-label-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&tmp_dir).unwrap();

    // Write a settings.json that selects "Appearance / Theme" as the last settings page.
    let path = tmp_dir.join("termua").join("settings.json");
    let _guard = crate::settings::override_settings_json_path(path.clone());
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(
        &path,
        r#"{
              "ui": { "last_settings_page": "nav.page.appearance.theme" }
            }"#,
    )
    .unwrap();

    cx.update(|app| {
        gpui_component::init(app);
        menubar::init(app);
        gpui_term::init(app);

        // Ensure there's at least one long key so we can verify layout behavior.
        let mut light = (*gpui_component::Theme::global(app).light_theme).clone();
        light.colors.description_list_label_foreground = Some("#123456FF".into());
        gpui_component::Theme::global_mut(app).apply_config(&Rc::new(light));
        gpui_component::Theme::change(gpui_component::ThemeMode::Light, None, app);
    });

    let (root, cx) = cx.add_window_view(|window, cx| {
        let settings = cx.new(|cx| SettingsWindow::new(window, cx));
        gpui_component::Root::new(settings, window, cx)
    });
    cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(900.)),
            gpui::AvailableSpace::Definite(gpui::px(700.)),
        ),
        move |_, _| div().size_full().child(root),
    );

    cx.run_until_parked();

    // Open theme editor.
    let bounds = cx
        .debug_bounds("termua-settings-appearance-theme-new-theme-button")
        .expect("new theme button should exist");
    cx.simulate_click(bounds.center(), gpui::Modifiers::none());
    cx.run_until_parked();

    let long_row_bounds = cx
        .debug_bounds("termua-theme-editor-color-description_list_label_foreground")
        .expect("expected long color key row to be present");
    assert!(
        long_row_bounds.size.height <= gpui::px(33.),
        "expected long color labels to stay on a single line"
    );
}

#[gpui::test]
fn theme_editor_footer_buttons_are_right_aligned(cx: &mut gpui::TestAppContext) {
    // Point settings.json to a temp directory so this test is hermetic.
    let tmp_dir = std::env::temp_dir().join(format!(
        "termua-settings-test-theme-editor-footer-align-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&tmp_dir).unwrap();

    // Write a settings.json that selects "Appearance / Theme" as the last settings page.
    let path = tmp_dir.join("termua").join("settings.json");
    let _guard = crate::settings::override_settings_json_path(path.clone());
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(
        &path,
        r#"{
              "ui": { "last_settings_page": "nav.page.appearance.theme" }
            }"#,
    )
    .unwrap();

    cx.update(|app| {
        gpui_component::init(app);
        menubar::init(app);
        gpui_term::init(app);
    });

    let (root, cx) = cx.add_window_view(|window, cx| {
        let settings = cx.new(|cx| SettingsWindow::new(window, cx));
        gpui_component::Root::new(settings, window, cx)
    });
    cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(900.)),
            gpui::AvailableSpace::Definite(gpui::px(700.)),
        ),
        move |_, _| div().size_full().child(root),
    );

    cx.run_until_parked();

    // Open theme editor.
    let bounds = cx
        .debug_bounds("termua-settings-appearance-theme-new-theme-button")
        .expect("new theme button should exist");
    cx.simulate_click(bounds.center(), gpui::Modifiers::none());
    cx.run_until_parked();

    let footer_bounds = cx
        .debug_bounds("termua-theme-editor-footer")
        .expect("footer should exist");
    let save_bounds = cx
        .debug_bounds("termua-theme-editor-save-button")
        .expect("save button should exist");
    let cancel_bounds = cx
        .debug_bounds("termua-theme-editor-cancel-button")
        .expect("cancel button should exist");

    let save_right = save_bounds.origin.x + save_bounds.size.width;
    let footer_right = footer_bounds.origin.x + footer_bounds.size.width;
    assert!(
        footer_right - save_right <= gpui::px(40.),
        "expected save button to be near the right edge of the footer"
    );
    assert!(
        cancel_bounds.origin.x < save_bounds.origin.x,
        "expected cancel button to be left of save button"
    );
}

#[gpui::test]
fn theme_editor_sheet_keeps_settings_window_draggable(cx: &mut gpui::TestAppContext) {
    // Point settings.json to a temp directory so this test is hermetic.
    let tmp_dir = std::env::temp_dir().join(format!(
        "termua-settings-test-theme-editor-drag-overlay-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&tmp_dir).unwrap();

    // Write a settings.json that selects "Appearance / Theme" as the last settings page.
    let path = tmp_dir.join("termua").join("settings.json");
    let _guard = crate::settings::override_settings_json_path(path.clone());
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(
        &path,
        r#"{
              "ui": { "last_settings_page": "nav.page.appearance.theme" }
            }"#,
    )
    .unwrap();

    cx.update(|app| {
        gpui_component::init(app);
        menubar::init(app);
        gpui_term::init(app);
    });

    let (root, cx) = cx.add_window_view(|window, cx| {
        let settings = cx.new(|cx| SettingsWindow::new(window, cx));
        gpui_component::Root::new(settings, window, cx)
    });
    cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(900.)),
            gpui::AvailableSpace::Definite(gpui::px(700.)),
        ),
        move |_, _| div().size_full().child(root),
    );

    cx.run_until_parked();

    // Open theme editor.
    let bounds = cx
        .debug_bounds("termua-settings-appearance-theme-new-theme-button")
        .expect("new theme button should exist");
    cx.simulate_click(bounds.center(), gpui::Modifiers::none());
    cx.run_until_parked();

    assert!(
        cx.debug_bounds("termua-settings-sheet-drag-overlay")
            .is_some(),
        "expected a drag overlay so the window remains movable while the sheet is active"
    );
}

#[gpui::test]
fn theme_editor_mode_switch_toggles_preview_mode(cx: &mut gpui::TestAppContext) {
    // Point settings.json to a temp directory so this test is hermetic.
    let tmp_dir = std::env::temp_dir().join(format!(
        "termua-settings-test-theme-editor-mode-switch-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&tmp_dir).unwrap();

    // Write a settings.json that selects "Appearance / Theme" as the last settings page.
    let path = tmp_dir.join("termua").join("settings.json");
    let _guard = crate::settings::override_settings_json_path(path.clone());
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(
        &path,
        r#"{
              "ui": { "last_settings_page": "nav.page.appearance.theme" }
            }"#,
    )
    .unwrap();

    cx.update(|app| {
        gpui_component::init(app);
        menubar::init(app);
        gpui_term::init(app);
    });

    let (root, cx) = cx.add_window_view(|window, cx| {
        let settings = cx.new(|cx| SettingsWindow::new(window, cx));
        gpui_component::Root::new(settings, window, cx)
    });
    cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(900.)),
            gpui::AvailableSpace::Definite(gpui::px(700.)),
        ),
        move |_, _| div().size_full().child(root),
    );

    cx.run_until_parked();

    // Open theme editor.
    let bounds = cx
        .debug_bounds("termua-settings-appearance-theme-new-theme-button")
        .expect("new theme button should exist");
    cx.simulate_click(bounds.center(), gpui::Modifiers::none());
    cx.run_until_parked();

    let original_is_dark = cx.update(|_window, app| app.theme().mode.is_dark());

    let switch_bounds = cx
        .debug_bounds("termua-theme-editor-mode-switch")
        .expect("mode switch should exist");
    cx.simulate_click(switch_bounds.center(), gpui::Modifiers::none());
    cx.run_until_parked();

    let toggled_is_dark = cx.update(|_window, app| app.theme().mode.is_dark());
    assert_ne!(
        toggled_is_dark, original_is_dark,
        "expected mode switch to toggle preview mode"
    );

    // Close.
    let cancel_bounds = cx
        .debug_bounds("termua-theme-editor-cancel-button")
        .expect("cancel button should exist");
    cx.simulate_click(cancel_bounds.center(), gpui::Modifiers::none());
    cx.run_until_parked();
}

#[gpui::test]
fn theme_editor_save_writes_a_theme_file(cx: &mut gpui::TestAppContext) {
    // Point settings.json to a temp directory so this test is hermetic.
    let tmp_dir = std::env::temp_dir().join(format!(
        "termua-settings-test-theme-editor-save-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&tmp_dir).unwrap();

    // Write a settings.json that selects "Appearance / Theme" as the last settings page.
    let path = tmp_dir.join("termua").join("settings.json");
    let _guard = crate::settings::override_settings_json_path(path.clone());
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(
        &path,
        r#"{
              "ui": { "last_settings_page": "nav.page.appearance.theme" }
            }"#,
    )
    .unwrap();

    cx.update(|app| {
        gpui_component::init(app);
        menubar::init(app);
        gpui_term::init(app);
    });

    let (root, cx) = cx.add_window_view(|window, cx| {
        let settings = cx.new(|cx| SettingsWindow::new(window, cx));
        gpui_component::Root::new(settings, window, cx)
    });
    cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(900.)),
            gpui::AvailableSpace::Definite(gpui::px(700.)),
        ),
        move |_, _| div().size_full().child(root),
    );

    cx.run_until_parked();

    // Open theme editor.
    let bounds = cx
        .debug_bounds("termua-settings-appearance-theme-new-theme-button")
        .expect("new theme button should exist");
    cx.simulate_click(bounds.center(), gpui::Modifiers::none());
    cx.run_until_parked();

    // Set theme name.
    let name_bounds = cx
        .debug_bounds("termua-theme-editor-name-input")
        .expect("theme name input should exist");
    cx.simulate_click(name_bounds.center(), gpui::Modifiers::none());
    cx.update(|window, app| {
        let Some(input) = window.focused_input(app) else {
            panic!("expected name input to be focused");
        };
        input.update(app, |state, cx| state.set_value("My Theme", window, cx));
    });
    cx.run_until_parked();

    // Save.
    let save_bounds = cx
        .debug_bounds("termua-theme-editor-save-button")
        .expect("save button should exist");
    cx.simulate_click(save_bounds.center(), gpui::Modifiers::none());
    cx.run_until_parked();

    let sheet_closed = cx.update(|window, app| !window.has_active_sheet(app));
    assert!(
        sheet_closed,
        "expected theme editor sheet to close after save"
    );

    // Verify file written.
    let themes_dir = crate::theme_manager::themes_dir_path();
    let entries = std::fs::read_dir(&themes_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|e| e.to_str()) == Some("json"))
        .collect::<Vec<_>>();
    assert!(
        !entries.is_empty(),
        "expected at least one theme json file to be written"
    );

    let contents = std::fs::read_to_string(entries[0].path()).unwrap();
    assert!(
        contents.contains("\"My Theme\""),
        "expected theme file to contain theme name"
    );
}

#[gpui::test]
fn non_theme_pages_do_not_reserve_icon_space_in_heading(cx: &mut gpui::TestAppContext) {
    // Point settings.json to a temp directory so this test is hermetic.
    let tmp_dir = std::env::temp_dir().join(format!(
        "termua-settings-test-non-theme-heading-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&tmp_dir).unwrap();

    // Use a non-theme page, e.g. Terminal / Font.
    let path = tmp_dir.join("termua").join("settings.json");
    let _guard = crate::settings::override_settings_json_path(path.clone());
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(
        &path,
        r#"{
              "ui": { "last_settings_page": "nav.page.terminal.font" }
            }"#,
    )
    .unwrap();

    cx.update(|app| {
        gpui_component::init(app);
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
            let view = app.new(|cx| SettingsWindow::new(window, cx));
            assert_eq!(view.read(app).selected_page, SettingsPage::TerminalFont);
            div().size_full().child(view)
        },
    );

    cx.run_until_parked();

    let heading = cx
        .debug_bounds("termua-settings-page-heading")
        .expect("heading should exist");
    let text = cx
        .debug_bounds("termua-settings-page-heading-text")
        .expect("heading text should exist");

    let left_gap = (text.left() - heading.left()).abs();
    assert!(
        left_gap <= gpui::px(1.0),
        "expected heading text to be left-aligned; got gap {left_gap:?}"
    );
}

#[gpui::test]
fn settings_window_uses_last_selected_page_from_settings_json(cx: &mut gpui::TestAppContext) {
    // Point settings.json to a temp directory so this test is hermetic.
    let tmp_dir = std::env::temp_dir().join(format!(
        "termua-settings-test-last-page-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&tmp_dir).unwrap();

    // Write a settings.json that selects "Terminal / Behavior" as the last settings page.
    let path = tmp_dir.join("termua").join("settings.json");
    let _guard = crate::settings::override_settings_json_path(path.clone());
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(
        &path,
        r#"{
              "ui": { "last_settings_page": "nav.page.terminal.behavior" }
            }"#,
    )
    .unwrap();

    cx.update(|app| {
        gpui_component::init(app);
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
            let view = app.new(|cx| SettingsWindow::new(window, cx));
            assert_eq!(view.read(app).selected_page, SettingsPage::TerminalBehavior);
            div().size_full().child(view)
        },
    );

    cx.run_until_parked();
}

#[gpui::test]
fn terminal_suggestions_page_renders_static_suggestions_controls(cx: &mut gpui::TestAppContext) {
    // Point settings.json to a temp directory so this test is hermetic.
    let tmp_dir = std::env::temp_dir().join(format!(
        "termua-settings-test-terminal-suggestions-page-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&tmp_dir).unwrap();

    // Select the Terminal / Suggestions page.
    let path = tmp_dir.join("termua").join("settings.json");
    let _guard = crate::settings::override_settings_json_path(path.clone());
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(
        &path,
        r#"{
              "ui": { "last_settings_page": "nav.page.terminal.suggestions" }
            }"#,
    )
    .unwrap();

    cx.update(|app| {
        gpui_component::init(app);
        menubar::init(app);
        gpui_term::init(app);
    });

    let (view, cx) = cx.add_window_view(|window, cx| SettingsWindow::new(window, cx));
    let view_for_draw = view.clone();
    cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(900.)),
            gpui::AvailableSpace::Definite(gpui::px(700.)),
        ),
        move |_, _| div().size_full().child(view_for_draw),
    );

    cx.run_until_parked();

    cx.update(|_window, app| {
        assert_eq!(
            view.read(app).selected_page,
            SettingsPage::TerminalSuggestions
        );
    });

    let reload_bounds = cx
        .debug_bounds("termua-settings-static-suggestions-reload")
        .expect("expected Reload button to be present");
    assert!(
        reload_bounds.origin.x > gpui::px(500.),
        "expected Reload button to be aligned near the right edge"
    );

    let hint_bounds = cx
        .debug_bounds("termua-settings-suggestions-json-dir-hint")
        .expect("expected Suggestions JSON dir hint to be present");
    assert!(
        hint_bounds.origin.x < reload_bounds.origin.x,
        "expected Suggestions JSON dir hint to be on the left side"
    );
}

#[gpui::test]
fn terminal_page_renders_default_backend_select(cx: &mut gpui::TestAppContext) {
    // Point settings.json to a temp directory so this test is hermetic.
    let tmp_dir = std::env::temp_dir().join(format!(
        "termua-settings-test-terminal-page-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&tmp_dir).unwrap();

    // Select the Terminal root page.
    let path = tmp_dir.join("termua").join("settings.json");
    let _guard = crate::settings::override_settings_json_path(path.clone());
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(
        &path,
        r#"{
              "ui": { "last_settings_page": "nav.group.terminal" }
            }"#,
    )
    .unwrap();

    cx.update(|app| {
        gpui_component::init(app);
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
            let view = app.new(|cx| SettingsWindow::new(window, cx));
            assert_eq!(view.read(app).selected_page, SettingsPage::Terminal);
            div().size_full().child(view)
        },
    );

    cx.run_until_parked();
    assert!(
        cx.debug_bounds("termua-settings-terminal-default-backend-select")
            .is_some()
    );
    assert!(
        cx.debug_bounds("termua-settings-terminal-ssh-backend-select")
            .is_some()
    );
    assert!(
        cx.debug_bounds("termua-settings-terminal-default-backend-icon-alacritty")
            .is_some()
    );
}

#[gpui::test]
fn terminal_cursor_page_renders_cursor_shape_options_with_glyphs(cx: &mut gpui::TestAppContext) {
    // Point settings.json to a temp directory so this test is hermetic.
    let tmp_dir = std::env::temp_dir().join(format!(
        "termua-settings-test-terminal-cursor-page-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&tmp_dir).unwrap();

    // Select the Terminal / Cursor page.
    let path = tmp_dir.join("termua").join("settings.json");
    let _guard = crate::settings::override_settings_json_path(path.clone());
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(
        &path,
        r#"{
                  "ui": { "last_settings_page": "nav.page.terminal.cursor" }
                }"#,
    )
    .unwrap();

    cx.update(|app| {
        gpui_component::init(app);
        menubar::init(app);
        gpui_term::init(app);
    });

    let (view, cx) = cx.add_window_view(|window, cx| SettingsWindow::new(window, cx));
    let view_for_draw = view.clone();
    cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(900.)),
            gpui::AvailableSpace::Definite(gpui::px(700.)),
        ),
        move |_, _| div().size_full().child(view_for_draw),
    );

    cx.run_until_parked();

    cx.update(|_window, app| {
        assert_eq!(view.read(app).selected_page, SettingsPage::TerminalCursor);
    });

    let button_bounds = cx
        .debug_bounds("termua-settings-terminal-cursor-shape-button-Block-█")
        .expect("expected cursor shape button to render glyph preview");
    cx.simulate_click(button_bounds.center(), gpui::Modifiers::none());
    cx.run_until_parked();

    assert!(
        cx.debug_bounds("termua-settings-terminal-cursor-shape-option-Block-█")
            .is_some(),
        "expected Block cursor option to render glyph preview"
    );
    assert!(
        cx.debug_bounds("termua-settings-terminal-cursor-shape-option-Underline-_")
            .is_some(),
        "expected Underline cursor option to render glyph preview"
    );
    assert!(
        cx.debug_bounds("termua-settings-terminal-cursor-shape-option-Bar-⎸")
            .is_some(),
        "expected Bar cursor option to render glyph preview"
    );
    assert!(
        cx.debug_bounds("termua-settings-terminal-cursor-shape-option-Hollow-▯")
            .is_some(),
        "expected Hollow cursor option to render glyph preview"
    );
}

#[gpui::test]
fn terminal_font_page_renders_font_family_select(cx: &mut gpui::TestAppContext) {
    // Point settings.json to a temp directory so this test is hermetic.
    let tmp_dir = std::env::temp_dir().join(format!(
        "termua-settings-test-terminal-font-page-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&tmp_dir).unwrap();

    // Write a settings.json that selects "Terminal / Font" as the last settings page.
    let path = tmp_dir.join("termua").join("settings.json");
    let _guard = crate::settings::override_settings_json_path(path.clone());
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(
        &path,
        r#"{
              "ui": { "last_settings_page": "nav.page.terminal.font" }
            }"#,
    )
    .unwrap();

    cx.update(|app| {
        gpui_component::init(app);
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
            let view = app.new(|cx| SettingsWindow::new(window, cx));
            assert_eq!(view.read(app).selected_page, SettingsPage::TerminalFont);
            div().size_full().child(view)
        },
    );

    cx.run_until_parked();
    assert!(
        cx.debug_bounds("termua-settings-terminal-font-family-select")
            .is_some()
    );
    assert!(
        cx.debug_bounds("termua-settings-terminal-font-fallbacks-select")
            .is_none()
    );
}

#[gpui::test]
fn terminal_font_page_renders_ligatures_switch(cx: &mut gpui::TestAppContext) {
    let tmp_dir = std::env::temp_dir().join(format!(
        "termua-settings-test-terminal-font-ligatures-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&tmp_dir).unwrap();

    let path = tmp_dir.join("termua").join("settings.json");
    let _guard = crate::settings::override_settings_json_path(path.clone());
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(
        &path,
        r#"{
              "ui": { "last_settings_page": "nav.page.terminal.font" }
            }"#,
    )
    .unwrap();

    cx.update(|app| {
        gpui_component::init(app);
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
            let view = app.new(|cx| SettingsWindow::new(window, cx));
            assert_eq!(view.read(app).selected_page, SettingsPage::TerminalFont);
            div().size_full().child(view)
        },
    );

    cx.run_until_parked();
    assert!(
        cx.debug_bounds("termua-settings-terminal-ligatures-switch")
            .is_some(),
        "expected ligatures switch to render on Terminal / Font page"
    );
}

#[gpui::test]
fn terminal_font_family_dropdown_renders_font_preview_options(cx: &mut gpui::TestAppContext) {
    // Point settings.json to a temp directory so this test is hermetic.
    let tmp_dir = std::env::temp_dir().join(format!(
        "termua-settings-test-terminal-font-family-dropdown-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&tmp_dir).unwrap();

    // Write a settings.json that selects "Terminal / Font" as the last settings page.
    let path = tmp_dir.join("termua").join("settings.json");
    let _guard = crate::settings::override_settings_json_path(path.clone());
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(
        &path,
        r#"{
              "ui": { "last_settings_page": "nav.page.terminal.font" }
            }"#,
    )
    .unwrap();

    cx.update(|app| {
        gpui_component::init(app);
        menubar::init(app);
        gpui_term::init(app);
    });

    let (view, cx) = cx.add_window_view(|window, cx| {
        let settings = cx.new(|cx| SettingsWindow::new(window, cx));
        gpui_component::Root::new(settings, window, cx)
    });
    cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(900.)),
            gpui::AvailableSpace::Definite(gpui::px(700.)),
        ),
        move |_, _| div().size_full().child(view),
    );

    cx.run_until_parked();

    let bounds = cx
        .debug_bounds("termua-settings-terminal-font-family-select")
        .expect("font family select should exist");
    cx.simulate_click(bounds.center(), gpui::Modifiers::none());
    cx.simulate_keystrokes("down");
    cx.run_until_parked();

    assert!(
        cx.debug_bounds("termua-settings-terminal-font-family-option-_ZedMono")
            .is_some(),
        "expected at least one font preview option to be rendered"
    );
}

#[gpui::test]
fn terminal_font_family_dropdown_filters_options_from_typed_query(cx: &mut gpui::TestAppContext) {
    let tmp_dir = std::env::temp_dir().join(format!(
        "termua-settings-test-terminal-font-family-filter-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&tmp_dir).unwrap();

    let path = tmp_dir.join("termua").join("settings.json");
    let _guard = crate::settings::override_settings_json_path(path.clone());
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(
        &path,
        r#"{
              "ui": { "last_settings_page": "nav.page.terminal.font" }
            }"#,
    )
    .unwrap();

    cx.update(|app| {
        gpui_component::init(app);
        menubar::init(app);
        gpui_term::init(app);
    });

    let (view, cx) = cx.add_window_view(|window, cx| {
        let settings = cx.new(|cx| SettingsWindow::new(window, cx));
        gpui_component::Root::new(settings, window, cx)
    });
    cx.draw(
        gpui::point(gpui::px(0.), gpui::px(0.)),
        gpui::size(
            gpui::AvailableSpace::Definite(gpui::px(900.)),
            gpui::AvailableSpace::Definite(gpui::px(700.)),
        ),
        move |_, _| div().size_full().child(view),
    );

    cx.run_until_parked();

    let bounds = cx
        .debug_bounds("termua-settings-terminal-font-family-select")
        .expect("font family select should exist");
    cx.simulate_click(bounds.center(), gpui::Modifiers::none());
    cx.run_until_parked();

    cx.debug_bounds("termua-settings-terminal-font-family-option-_ZedMono")
        .expect("expected selected font option to be visible before filtering");
    cx.simulate_keystrokes("enter");
    cx.run_until_parked();
    cx.simulate_input("__definitely_no_matching_font__");
    cx.run_until_parked();

    assert!(
        cx.debug_bounds("termua-settings-terminal-font-family-option-_ZedMono")
            .is_none(),
        "expected font options to be filtered by the typed query"
    );
}

#[test]
fn setting_metadata_covers_all_controls() {
    let ids: std::collections::HashSet<_> = SettingMeta::all().iter().map(|m| m.id).collect();

    // Security / Lock screen
    assert!(ids.contains("lock_screen.enabled"));
    assert!(ids.contains("lock_screen.timeout_secs"));

    // Appearance
    assert!(ids.contains("appearance.theme"));
    assert!(ids.contains("appearance.light_theme"));
    assert!(ids.contains("appearance.dark_theme"));
    assert!(ids.contains("appearance.language"));

    // Terminal / Font
    assert!(ids.contains("terminal.font_family"));
    assert!(!ids.contains("terminal.font_fallbacks"));
    assert!(ids.contains("terminal.font_size"));
    assert!(ids.contains("terminal.ligatures"));

    // Terminal / Terminal
    assert!(ids.contains("terminal.default_backend"));
    assert!(ids.contains("terminal.ssh_backend"));

    // Terminal / Cursor
    assert!(ids.contains("terminal.cursor_shape"));
    assert!(ids.contains("terminal.blinking"));

    // Terminal / Rendering
    assert!(ids.contains("terminal.show_scrollbar"));
    assert!(ids.contains("terminal.show_line_numbers"));

    // Terminal / Behavior
    assert!(ids.contains("terminal.option_as_meta"));
    assert!(ids.contains("terminal.copy_on_select"));
    assert!(ids.contains("terminal.suggestions_enabled"));
    assert!(ids.contains("terminal.suggestions_max_items"));
    assert!(ids.contains("terminal.sftp_upload_max_concurrency"));

    // Terminal / Suggestions
    assert!(ids.contains("terminal.suggestions_json_dir"));

    // Terminal / Key Bindings
    assert!(ids.contains("terminal.keybindings.copy"));
    assert!(ids.contains("terminal.keybindings.paste"));
    assert!(ids.contains("terminal.keybindings.select_all"));
    assert!(ids.contains("terminal.keybindings.clear"));
    assert!(ids.contains("terminal.keybindings.search"));
    assert!(ids.contains("terminal.keybindings.search_next"));
    assert!(ids.contains("terminal.keybindings.search_previous"));
    assert!(ids.contains("terminal.keybindings.increase_font_size"));
    assert!(ids.contains("terminal.keybindings.decrease_font_size"));
    assert!(ids.contains("terminal.keybindings.reset_font_size"));

    // Recording
    assert!(ids.contains("recording.include_input_by_default"));
    assert!(ids.contains("recording.playback_speed"));

    // Logging
    assert!(ids.contains("logging.level"));
    assert!(ids.contains("logging.path"));
}

#[test]
fn search_results_are_sorted_by_title() {
    let entries = vec![
        SettingMeta {
            id: "test.z",
            title: "Zebra",
            description: "z",
            keywords: &["animals"],
            section: SettingsNavSection::Terminal,
            page: SettingsPage::TerminalFont,
        },
        SettingMeta {
            id: "test.a",
            title: "Apple",
            description: "a",
            keywords: &["animals"],
            section: SettingsNavSection::Terminal,
            page: SettingsPage::TerminalFont,
        },
    ];

    let results = search_settings(&entries, "animals");
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].title, "Apple");
    assert_eq!(results[1].title, "Zebra");
}

#[test]
fn sidebar_nav_specs_match_settings_nav_requirements() {
    let specs = sidebar_nav_specs();

    assert_eq!(specs.len(), 6);
    let appearance = specs
        .iter()
        .find(|group| group.section == SettingsNavSection::Appearance)
        .expect("expected Appearance group");
    assert_eq!(appearance.items.len(), 2);
    assert!(
        appearance
            .items
            .iter()
            .any(|item| item.page == SettingsPage::AppearanceTheme)
    );
    assert!(
        appearance
            .items
            .iter()
            .any(|item| item.page == SettingsPage::AppearanceLanguage)
    );

    assert!(
        specs
            .iter()
            .any(|group| group.section == SettingsNavSection::Terminal)
    );
    assert!(
        specs
            .iter()
            .any(|group| group.section == SettingsNavSection::Recording)
    );

    let logging = specs
        .iter()
        .find(|group| group.section == SettingsNavSection::Logging)
        .expect("expected Logging group");
    assert_eq!(logging.items.len(), 1);
    assert_eq!(logging.items[0].page, SettingsPage::Logging);

    let assistant = specs
        .iter()
        .find(|group| group.section == SettingsNavSection::Assistant)
        .expect("expected Assistant group");
    assert_eq!(assistant.items.len(), 1);
    assert_eq!(assistant.items[0].page, SettingsPage::Assistant);

    let security = specs
        .iter()
        .find(|group| group.section == SettingsNavSection::Security)
        .expect("expected Security group");
    assert_eq!(security.items.len(), 1);
    assert_eq!(security.items[0].page, SettingsPage::SecurityLockScreen);
}

#[test]
fn sidebar_nav_tree_items_are_sorted_alphabetically() {
    let _guard = crate::locale::lock();
    crate::locale::set_locale("en");

    let specs = sidebar_nav_specs();
    let group_labels: Vec<&str> = specs.iter().map(|group| group.label.as_str()).collect();
    let mut sorted_group_labels = group_labels.clone();
    sorted_group_labels.sort_unstable();
    assert_eq!(group_labels, sorted_group_labels);

    for group in &specs {
        let item_labels: Vec<&str> = group.items.iter().map(|item| item.label.as_str()).collect();
        let mut sorted_item_labels = item_labels.clone();
        sorted_item_labels.sort_unstable();
        assert_eq!(item_labels, sorted_item_labels, "group: {}", group.label);
    }

    let tree_items = build_nav_tree_items();
    let tree_group_labels: Vec<&str> = tree_items.iter().map(|item| item.label.as_ref()).collect();
    assert_eq!(tree_group_labels, sorted_group_labels);

    for group in &tree_items {
        let child_labels: Vec<&str> = group
            .children
            .iter()
            .map(|item| item.label.as_ref())
            .collect();
        let mut sorted_child_labels = child_labels.clone();
        sorted_child_labels.sort_unstable();
        assert_eq!(
            child_labels, sorted_child_labels,
            "tree group: {}",
            group.label
        );
    }
}

#[test]
fn sidebar_nav_order_matches_english_in_chinese_locale() {
    let _guard = crate::locale::lock();

    crate::locale::set_locale("en");
    let english_specs = sidebar_nav_specs();
    let english_group_order: Vec<SettingsNavSection> =
        english_specs.iter().map(|group| group.section).collect();
    let english_item_order: Vec<Vec<SettingsPage>> = english_specs
        .iter()
        .map(|group| group.items.iter().map(|item| item.page).collect())
        .collect();

    crate::locale::set_locale("zh-CN");
    let chinese_specs = sidebar_nav_specs();
    let chinese_group_order: Vec<SettingsNavSection> =
        chinese_specs.iter().map(|group| group.section).collect();
    let chinese_item_order: Vec<Vec<SettingsPage>> = chinese_specs
        .iter()
        .map(|group| group.items.iter().map(|item| item.page).collect())
        .collect();

    assert_eq!(chinese_group_order, english_group_order);
    assert_eq!(chinese_item_order, english_item_order);
}

#[test]
fn sidebar_nav_specs_do_not_include_terminal_root_item() {
    let specs = sidebar_nav_specs();
    let terminal = specs
        .iter()
        .find(|g| g.section == SettingsNavSection::Terminal)
        .unwrap();
    assert!(
        terminal
            .items
            .iter()
            .all(|i| i.page != SettingsPage::Terminal),
        "did not expect SettingsPage::Terminal under the Terminal section"
    );
}

#[test]
fn every_setting_meta_id_has_a_supported_control() {
    for meta in SettingMeta::all() {
        assert!(
            SettingsWindow::supports_setting_id(meta.id),
            "unsupported setting id: {}",
            meta.id
        );
    }
}

#[test]
fn recording_playback_speed_setting_has_a_supported_control() {
    assert!(SettingsWindow::supports_setting_id(
        "recording.playback_speed"
    ));
}

#[test]
fn next_stepped_value_clamps_to_min_max() {
    assert_eq!(
        super::view::next_stepped_value(10.0, -1.0, 1.0, 6.0, 64.0),
        9.0
    );
    assert_eq!(
        super::view::next_stepped_value(6.0, -1.0, 1.0, 6.0, 64.0),
        6.0
    );
    assert_eq!(
        super::view::next_stepped_value(64.0, 1.0, 1.0, 6.0, 64.0),
        64.0
    );
    assert_eq!(
        super::view::next_stepped_value(10.0, 2.0, 0.5, 6.0, 64.0),
        11.0
    );
}

#[gpui::test]
fn select_page_by_nav_id_updates_selected_page_and_persists_nav_id(cx: &mut gpui::TestAppContext) {
    // Point settings.json to a temp directory so this test is hermetic.
    let tmp_dir = std::env::temp_dir().join(format!(
        "termua-settings-test-select-page-by-nav-id-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&tmp_dir).unwrap();

    let path = tmp_dir.join("termua").join("settings.json");
    let _guard = crate::settings::override_settings_json_path(path.clone());
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }

    cx.update(|app| {
        gpui_component::init(app);
        menubar::init(app);
        gpui_term::init(app);
    });

    let (view, cx) = cx.add_window_view(|window, cx| SettingsWindow::new(window, cx));

    let initial_page = cx.update(|_window, app| view.read(app).selected_page);

    let selected_unknown = cx.update(|_window, app| {
        view.update(app, |this, cx| {
            this.select_page_by_nav_id("nav.unknown.id", cx)
        })
    });
    assert!(!selected_unknown);
    assert_eq!(
        cx.update(|_window, app| view.read(app).selected_page),
        initial_page
    );

    let selected_theme = cx.update(|_window, app| {
        view.update(app, |this, cx| {
            this.select_page_by_nav_id("nav.page.appearance.theme", cx)
        })
    });
    assert!(selected_theme);

    let (page, last) = cx.update(|_window, app| {
        let this = view.read(app);
        (
            this.selected_page,
            this.settings.ui.last_settings_page.clone(),
        )
    });
    assert_eq!(page, SettingsPage::AppearanceTheme);
    assert_eq!(
        last.as_deref(),
        Some("nav.page.appearance.theme"),
        "expected selected nav id to be persisted"
    );
}
