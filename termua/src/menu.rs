//! Application menus and top-level actions.

use std::{collections::HashMap, env, process::Command};

use gpui::{
    App, AppContext, InteractiveElement, KeyBinding, Menu, MenuItem, ParentElement, Styled,
    actions, px, relative,
};
use gpui_common::TermuaIcon;
use gpui_component::{
    ActiveTheme as _, Icon, IconName, Sizable,
    button::{Button, ButtonVariants},
    checkbox::Checkbox,
    h_flex, orange_500, v_flex,
};
use gpui_term::TerminalType;
use rust_i18n::t;

use crate::{
    PendingCommand, TermuaAppState,
    config::SettingsWindow,
    new_session::NewSessionWindow,
    right_sidebar::{RightSidebarState, RightSidebarTab},
    window::about::AboutWindow,
};

actions!(
    termua,
    [
        Quit,
        OpenAbout,
        OpenNewSession,
        NewLocalTerminal,
        NewWindow,
        OpenSettings,
        OpenSftp,
        ShareTerminalWeb,
        RevokeWebControl,
        ToggleSessionsSidebar,
        ToggleMessagesSidebar,
        ToggleAssistantSidebar,
        ToggleMultiExec,
        PlayCast,
        CheckForUpdates
    ]
);

pub(crate) fn register(cx: &mut App) {
    cx.on_action(quit);
    cx.on_action(open_about);
    cx.on_action(open_new_session);
    cx.on_action(new_local_terminal);
    cx.on_action(new_window);
    cx.on_action(open_settings);
    cx.on_action(toggle_sessions_sidebar);
    cx.on_action(toggle_messages_sidebar);
    cx.on_action(toggle_assistant_sidebar);
    cx.on_action(toggle_multi_exec);
    cx.on_action(play_cast);
    cx.on_action(check_for_updates);
}

fn update_window(cx: &mut App) -> Option<gpui::WindowHandle<gpui_component::Root>> {
    cx.active_window()
        .and_then(|window| window.downcast::<gpui_component::Root>())
        .or_else(|| {
            cx.try_global::<TermuaAppState>()
                .and_then(|state| state.main_window)
        })
}

fn check_for_updates(_: &CheckForUpdates, cx: &mut App) {
    let window = update_window(cx);
    let Some(window) = window else {
        log::warn!("CheckForUpdates: no application window is available");
        return;
    };
    let background = cx.background_executor().clone();
    cx.spawn(async move |cx| {
        let result = background
            .spawn(async { crate::update::check_latest() })
            .await;
        let _ = cx.update(|app| {
            let (kind, message) = match result {
                Ok(crate::update::CheckResult::UpdateAvailable { tag, url }) => {
                    show_update_dialog(window, &tag, &url, app);
                    return;
                }
                Ok(crate::update::CheckResult::UpToDate) => (
                    crate::notification::MessageKind::Success,
                    t!("Update.UpToDate").to_string(),
                ),
                Err(error) => (
                    crate::notification::MessageKind::Error,
                    t!("Update.CheckFailed", error = error).to_string(),
                ),
            };
            window
                .update(app, |_, window, app| {
                    crate::notification::notify_app(kind, message, window, app);
                })
                .ok();
        });
    })
    .detach();
}

pub(crate) fn check_for_updates_startup(
    cx: &mut App,
    window: gpui::WindowHandle<gpui_component::Root>,
) {
    if let Some(crate::update::CheckResult::UpdateAvailable { tag, url }) =
        crate::update::load_startup_update()
    {
        show_update_dialog(window, &tag, &url, cx);
    }

    let background = cx.background_executor().clone();
    background
        .spawn(async {
            match crate::update::check_latest() {
                Ok(result) => {
                    if let Err(error) = crate::update::persist_startup_result(&result) {
                        log::warn!("failed to save startup update check result: {error:#}");
                    }
                }
                Err(error) => {
                    log::warn!("startup update check failed: {error:#}");
                }
            }
        })
        .detach();
}

fn show_update_dialog(
    window: gpui::WindowHandle<gpui_component::Root>,
    tag: &str,
    url: &str,
    app: &mut App,
) {
    let tag = tag.to_string();
    let url = url.to_string();
    let title = t!("Update.AvailableTitle").to_string();
    let body = t!("Update.AvailableBody").to_string();
    let latest_version = t!("Update.LatestVersion").to_string();
    let do_not_remind = t!("Update.DoNotRemind").to_string();
    let _ = window.update(app, move |root, window, cx| {
        // The update dialog can open while the main window is not the active
        // window (for example at startup, or when the background update check
        // finishes after the user switched away). Without this, the first click
        // on the dialog only activates the window and is not delivered to the
        // close button.
        window.activate_window();

        let preference = cx.new(|_| UpdateReminderPreference {
            tag: tag.clone(),
            url: url.clone(),
            label: do_not_remind.clone(),
            suppressed: false,
        });
        root.open_dialog(
            move |dialog, _window, cx| {
                let link_url = url.clone();
                dialog
                    .close_button(false)
                    .title(
                        h_flex()
                            .w_full()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_2()
                                    .text_sm()
                                    .child(
                                        Icon::default()
                                            .path(TermuaIcon::CircleQuestion)
                                            .with_size(px(20.))
                                            .text_color(orange_500()),
                                    )
                                    .child(title.clone()),
                            )
                            .child(
                                Button::new("termua-update-dialog-close")
                                    .small()
                                    .ghost()
                                    .icon(IconName::Close)
                                    .debug_selector(|| "termua-update-dialog-close".to_string())
                                    .on_click(|_, window, cx| {
                                        gpui_component::WindowExt::close_dialog(window, cx);
                                    }),
                            ),
                    )
                    .child(
                        v_flex()
                            .pt_2()
                            .min_w(px(420.))
                            .gap_4()
                            .text_sm()
                            .child(gpui::div().child(body.clone()))
                            .child(
                                h_flex()
                                    .w_full()
                                    .items_center()
                                    .justify_between()
                                    .rounded_lg()
                                    .border_1()
                                    .border_color(cx.theme().border)
                                    .p_3()
                                    .child(
                                        gpui::div().text_xs().child(format!("{latest_version}:")),
                                    )
                                    .child(
                                        Button::new("termua-update-version-link")
                                            .small()
                                            .compact()
                                            .link()
                                            .label(tag.clone())
                                            .debug_selector(|| {
                                                "termua-update-version-link".to_string()
                                            })
                                            .on_click(move |_, _, app| {
                                                app.open_url(&link_url);
                                            }),
                                    ),
                            )
                            .child(gpui::div().pt_2().child(preference.clone())),
                    )
            },
            window,
            cx,
        );
    });
}

struct UpdateReminderPreference {
    tag: String,
    url: String,
    label: String,
    suppressed: bool,
}

impl gpui::Render for UpdateReminderPreference {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        Checkbox::new("termua-update-do-not-remind")
            .xsmall()
            .checked(self.suppressed)
            .child(
                gpui::div()
                    .line_height(relative(1.2))
                    .child(self.label.clone()),
            )
            .on_click(cx.listener(|this, checked, _window, cx| {
                this.suppressed = *checked;
                if let Err(error) = crate::update::set_startup_update_suppressed(
                    &this.tag,
                    &this.url,
                    this.suppressed,
                ) {
                    log::warn!("failed to save update reminder preference: {error:#}");
                }
                cx.notify();
            }))
    }
}

fn quit(_: &Quit, cx: &mut App) {
    let active_root = cx
        .active_window()
        .and_then(|window| window.downcast::<gpui_component::Root>());
    let main_window = cx
        .try_global::<TermuaAppState>()
        .and_then(|state| state.main_window);

    cx.defer(move |cx| {
        let dispatch_to_root =
            |root_handle: gpui::WindowHandle<gpui_component::Root>, cx: &mut App| -> bool {
                root_handle
                    .update(cx, |root, window, cx| {
                        let Ok(termua) = root
                            .view()
                            .clone()
                            .downcast::<crate::window::main_window::TermuaWindow>()
                        else {
                            return false;
                        };

                        termua.update(cx, |this, cx| this.request_quit(window, cx));
                        true
                    })
                    .unwrap_or(false)
            };

        if let Some(root) = active_root
            && dispatch_to_root(root, cx)
        {
            return;
        }

        if let Some(root) = main_window
            && dispatch_to_root(root, cx)
        {
            return;
        }

        cx.quit();
    });
}

fn open_about(_: &OpenAbout, cx: &mut App) {
    // Reuse existing About window if it's still open.
    let existing = cx.global::<TermuaAppState>().about_window;
    if let Some(handle) = existing {
        if handle
            .update(cx, |_, window, _cx| {
                window.activate_window();
            })
            .is_ok()
        {
            return;
        }
    }

    match AboutWindow::open(cx) {
        Ok(handle) => {
            cx.global_mut::<TermuaAppState>().about_window = Some(handle);
        }
        Err(err) => log::error!("OpenAbout: failed to open about window: {err:#}"),
    }
}

fn open_new_session(_: &OpenNewSession, cx: &mut App) {
    if let Err(err) = NewSessionWindow::open(cx) {
        log::error!("OpenNewSession: failed to open window: {err:#}");
    }
}

fn new_local_terminal(_: &NewLocalTerminal, cx: &mut App) {
    if cx.global::<TermuaAppState>().main_window.is_none() {
        log::warn!("NewLocalTerminal: main window not ready yet");
        return;
    }

    cx.global_mut::<TermuaAppState>()
        .pending_command(PendingCommand::OpenLocalTerminal {
            backend_type: TerminalType::WezTerm,
            env: HashMap::new(),
        });
    cx.refresh_windows();
}

fn new_window(_: &NewWindow, cx: &mut App) {
    if cx.global::<TermuaAppState>().main_window.is_none() {
        log::warn!("NewWindow: main window not ready yet");
        return;
    };

    match env::current_exe() {
        Ok(path) => {
            let mut child = Command::new(path);
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;

                use windows::Win32::System::Threading::CREATE_NEW_PROCESS_GROUP;
                child.creation_flags(CREATE_NEW_PROCESS_GROUP.0);
            }

            #[cfg(unix)]
            {
                use std::os::unix::prelude::CommandExt;
                unsafe {
                    child.pre_exec(|| {
                        let _ = rustix::process::setsid();
                        Ok(())
                    });
                }
            }

            if let Err(err) = child.spawn() {
                log::error!("failed to launch new window: {err}");
            }
        }
        Err(err) => log::error!("failed to get current exe path: {err}"),
    }
}

fn open_settings(_: &OpenSettings, cx: &mut App) {
    let existing = cx.global::<TermuaAppState>().settings_window;
    if let Some(handle) = existing {
        if handle
            .update(cx, |_, window, _cx| {
                window.activate_window();
            })
            .is_ok()
        {
            return;
        }
    }

    match SettingsWindow::open(cx) {
        Ok(handle) => {
            cx.global_mut::<TermuaAppState>().settings_window = Some(handle);
        }
        Err(err) => log::error!("OpenSettings: failed to open settings window: {err:#}"),
    }
}

pub(crate) fn toggle_multi_exec(_: &ToggleMultiExec, cx: &mut App) {
    let enabled = {
        let state = cx.global_mut::<TermuaAppState>();
        state.multi_exec_enabled = !state.multi_exec_enabled;
        state.multi_exec_enabled
    };

    // Keep the menu item's checkmark state in sync across all platforms.
    set_app_menus(cx, build_menus(enabled));
}

pub(crate) fn toggle_sessions_sidebar(_: &ToggleSessionsSidebar, cx: &mut App) {
    {
        let state = cx.global_mut::<TermuaAppState>();
        state.sessions_sidebar_visible = !state.sessions_sidebar_visible;
    }
    cx.refresh_windows();
}

pub(crate) fn toggle_messages_sidebar(_: &ToggleMessagesSidebar, cx: &mut App) {
    if cx.try_global::<RightSidebarState>().is_none() {
        cx.set_global(RightSidebarState::default());
    }
    cx.global_mut::<RightSidebarState>()
        .toggle_tab(RightSidebarTab::Notifications);
    cx.refresh_windows();
}

pub(crate) fn toggle_assistant_sidebar(_: &ToggleAssistantSidebar, cx: &mut App) {
    let assistant_enabled = cx
        .try_global::<crate::settings::AssistantSettings>()
        .map(|s| s.enabled)
        .unwrap_or(true);

    if cx.try_global::<RightSidebarState>().is_none() {
        cx.set_global(RightSidebarState::default());
    }

    // Block opening the assistant panel when the feature is disabled,
    // but allow closing it if it was already open (e.g. after a settings change).
    let state = cx.global::<RightSidebarState>();
    if !assistant_enabled && !(state.visible && state.active_tab == RightSidebarTab::Assistant) {
        return;
    }

    cx.global_mut::<RightSidebarState>()
        .toggle_tab(RightSidebarTab::Assistant);
    cx.refresh_windows();
}

fn play_cast(_: &PlayCast, cx: &mut App) {
    if cx.global::<TermuaAppState>().main_window.is_none() {
        log::warn!("PlayCast: main window not ready yet");
        return;
    }

    cx.global_mut::<TermuaAppState>()
        .pending_command(PendingCommand::OpenCastPicker);
    cx.refresh_windows();
}

pub(crate) fn bind_menu_shortcuts(cx: &mut App) {
    #[cfg(not(target_os = "macos"))]
    cx.bind_keys([
        KeyBinding::new("ctrl-shift-n", OpenNewSession, None),
        KeyBinding::new("ctrl-n", NewLocalTerminal, None),
        KeyBinding::new("ctrl-q", Quit, None),
        KeyBinding::new("ctrl-,", OpenSettings, None),
        KeyBinding::new("ctrl-shift-a", ToggleAssistantSidebar, Some("!Terminal")),
        KeyBinding::new("ctrl-shift-m", ToggleMessagesSidebar, None),
    ]);

    #[cfg(target_os = "macos")]
    cx.bind_keys([
        KeyBinding::new("cmd-shift-n", OpenNewSession, None),
        KeyBinding::new("cmd-n", NewLocalTerminal, None),
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-,", OpenSettings, None),
        KeyBinding::new("cmd-shift-a", ToggleAssistantSidebar, Some("!Terminal")),
        KeyBinding::new("cmd-shift-m", ToggleMessagesSidebar, None),
    ]);
}

pub(crate) fn build_menus(multi_exec_enabled: bool) -> Vec<Menu> {
    // menus[0] is the fold/app menu (menubar crate expects this).
    vec![
        Menu::new(t!("Menu.App.Name").to_string()).items(vec![
            MenuItem::action(t!("Menu.App.AboutTermua").to_string(), OpenAbout),
            MenuItem::action(t!("Menu.App.OpenSettings").to_string(), OpenSettings),
            MenuItem::separator(),
            MenuItem::action(t!("Menu.App.CheckForUpdates").to_string(), CheckForUpdates),
            MenuItem::separator(),
            MenuItem::action(t!("Menu.App.Quit").to_string(), Quit),
        ]),
        Menu::new(t!("Menu.Session.Name").to_string()).items(vec![
            MenuItem::action(t!("Menu.Session.NewSession").to_string(), OpenNewSession),
            MenuItem::action(t!("Menu.Session.NewWindow").to_string(), NewWindow),
        ]),
        Menu::new(t!("Menu.Recorder.Name").to_string()).items(vec![MenuItem::action(
            t!("Menu.Recorder.Play").to_string(),
            PlayCast,
        )]),
        Menu::new(t!("Menu.Run.Name").to_string()).items(vec![
            MenuItem::action(t!("Menu.Run.MultiExecute").to_string(), ToggleMultiExec)
                .checked(multi_exec_enabled),
        ]),
    ]
}

pub(crate) fn set_app_menus(cx: &mut App, menus: Vec<Menu>) {
    #[cfg(test)]
    let snapshot = snapshot_menus(&menus);

    cx.set_menus(menus);

    #[cfg(test)]
    {
        if cx.has_global::<MenuSnapshot>() {
            *cx.global_mut::<MenuSnapshot>() = snapshot;
        } else {
            cx.set_global(snapshot);
        }
    }
}

pub(crate) fn sync_app_menus(cx: &mut App) {
    let multi_exec_enabled = cx
        .try_global::<TermuaAppState>()
        .map(|state| state.multi_exec_enabled)
        .unwrap_or(false);
    set_app_menus(cx, build_menus(multi_exec_enabled));
}

#[cfg(test)]
#[derive(Clone, Debug, Default)]
pub(crate) struct MenuSnapshot {
    pub menus: Vec<MenuSnapshotMenu>,
}

#[cfg(test)]
impl gpui::Global for MenuSnapshot {}

#[cfg(test)]
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub(crate) struct MenuSnapshotMenu {
    pub name: String,
    pub items: Vec<MenuSnapshotItem>,
}

#[cfg(test)]
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub(crate) enum MenuSnapshotItem {
    Separator,
    Submenu(MenuSnapshotMenu),
    SystemMenu { name: String },
    Action { name: String, checked: bool },
}

#[cfg(test)]
fn snapshot_menus(menus: &[Menu]) -> MenuSnapshot {
    MenuSnapshot {
        menus: menus.iter().map(snapshot_menu).collect(),
    }
}

#[cfg(test)]
fn snapshot_menu(menu: &Menu) -> MenuSnapshotMenu {
    MenuSnapshotMenu {
        name: menu.name.to_string(),
        items: menu.items.iter().map(snapshot_item).collect(),
    }
}

#[cfg(test)]
fn snapshot_item(item: &MenuItem) -> MenuSnapshotItem {
    match item {
        MenuItem::Separator => MenuSnapshotItem::Separator,
        MenuItem::Submenu(menu) => MenuSnapshotItem::Submenu(snapshot_menu(menu)),
        MenuItem::SystemMenu(menu) => MenuSnapshotItem::SystemMenu {
            name: menu.name.to_string(),
        },
        MenuItem::Action { name, checked, .. } => MenuSnapshotItem::Action {
            name: name.to_string(),
            checked: *checked,
        },
    }
}

#[cfg(test)]
mod tests {
    use gpui::{AppContext, AsKeystroke, MenuItem, Render, div};
    use gpui_component::WindowExt;

    use super::*;

    struct UpdateDialogTestView;

    impl Render for UpdateDialogTestView {
        fn render(
            &mut self,
            _window: &mut gpui::Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            div()
        }
    }

    #[gpui::test]
    fn update_dialog_opens_without_reentering_root_update(cx: &mut gpui::TestAppContext) {
        let _guard = crate::locale::lock();
        crate::locale::set_locale("en");

        cx.update(gpui_component::init);
        let (root, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|_| UpdateDialogTestView);
            gpui_component::Root::new(view, window, cx)
        });
        let window = cx.update(|window, _app| {
            window
                .window_handle()
                .downcast::<gpui_component::Root>()
                .expect("expected Root window handle")
        });
        cx.draw(
            gpui::point(gpui::px(0.), gpui::px(0.)),
            gpui::size(
                gpui::AvailableSpace::Definite(gpui::px(800.)),
                gpui::AvailableSpace::Definite(gpui::px(600.)),
            ),
            move |_, _| div().size_full().child(root),
        );
        cx.run_until_parked();

        cx.cx
            .update(|app| show_update_dialog(window, "v0.1.5", "https://example.com/release", app));
        cx.run_until_parked();

        cx.update(|window, app| {
            assert!(
                window.has_active_dialog(app),
                "expected update dialog to be active"
            );
        });
    }

    #[test]
    fn menu_labels_follow_the_active_locale() {
        let _guard = crate::locale::lock();
        crate::locale::set_locale("zh-CN");

        let menus = build_menus(false);
        assert!(!menus.is_empty());

        // We intentionally assert a non-English label to ensure the menu is localized.
        assert_eq!(menus[0].name.as_ref(), "Termua");
        match &menus[0].items[0] {
            MenuItem::Action { name, .. } => assert_eq!(name.as_ref(), "关于 Termua"),
            _ => panic!("expected first Termua menu item to be an Action"),
        }
    }

    #[test]
    fn termua_menu_contains_settings_item() {
        let _guard = crate::locale::lock();
        crate::locale::set_locale("en");

        let menus = build_menus(false);
        assert!(!menus.is_empty());
        assert_eq!(menus[0].name.as_ref(), "Termua");

        // Termua menu: About, Settings, <separator>, Check for updates, <separator>, Quit
        assert_eq!(menus[0].items.len(), 6);
        match &menus[0].items[0] {
            MenuItem::Action { name, .. } => assert_eq!(name.as_ref(), "About Termua"),
            _ => panic!("expected first Termua menu item to be an Action"),
        }
        match &menus[0].items[1] {
            MenuItem::Action { name, .. } => assert_eq!(name.as_ref(), "Open Settings"),
            _ => panic!("expected second Termua menu item to be an Action"),
        }
        assert!(matches!(menus[0].items[2], MenuItem::Separator));
        match &menus[0].items[3] {
            MenuItem::Action { name, .. } => assert_eq!(name.as_ref(), "Check for updates"),
            _ => panic!("expected fourth Termua menu item to be an Action"),
        }
        assert!(matches!(menus[0].items[4], MenuItem::Separator));
        match &menus[0].items[5] {
            MenuItem::Action { name, .. } => assert_eq!(name.as_ref(), "Quit"),
            _ => panic!("expected Quit to be an Action"),
        }

        // Run menu: Multi Execute (unchecked by default)
        assert!(menus.iter().any(|m| m.name.as_ref() == "Run"));
        let run_menu = menus.iter().find(|m| m.name.as_ref() == "Run").unwrap();
        assert_eq!(run_menu.items.len(), 1);
        match &run_menu.items[0] {
            MenuItem::Action { name, checked, .. } => {
                assert_eq!(name.as_ref(), "Multi Execute");
                assert_eq!(*checked, false);
            }
            _ => panic!("expected Multi Execute to be an Action"),
        }
    }

    #[test]
    fn new_session_menu_item() {
        let _guard = crate::locale::lock();
        crate::locale::set_locale("en");

        let menus = build_menus(false);
        let session_menu = menus
            .iter()
            .find(|m| m.name.as_ref() == "Session")
            .expect("Session menu should exist");

        assert!(
            session_menu.items.iter().any(|item| matches!(
                item,
                MenuItem::Action { name, .. } if name.as_ref() == "New Session"
            )),
            "Session menu should contain 'New Session'"
        );
        assert!(
            session_menu.items.iter().all(|item| !matches!(
                item,
                MenuItem::Action { name, .. } if name.as_ref() == "New Terminal"
            )),
            "Session menu should not contain 'New Terminal'"
        );
    }

    #[test]
    fn recorder_menu_contains_play_item_only() {
        let _guard = crate::locale::lock();
        crate::locale::set_locale("en");

        let menus = build_menus(false);
        let recorder_menu = menus
            .iter()
            .find(|m| m.name.as_ref() == "Recorder")
            .expect("Recorder menu should exist");

        assert!(
            recorder_menu.items.iter().any(|item| matches!(
                item,
                MenuItem::Action { name, .. } if name.as_ref() == "Play"
            )),
            "Recorder menu should contain 'Play'"
        );
        assert!(
            recorder_menu.items.iter().all(|item| !matches!(
                item,
                MenuItem::Action { name, .. } if name.as_ref() == "Recording"
            )),
            "Recorder menu should not contain 'Recording'"
        );
    }

    #[gpui::test]
    fn menu_shortcuts_are_bound(cx: &mut gpui::TestAppContext) {
        let _guard = crate::locale::lock();
        crate::locale::set_locale("en");

        cx.update(|app| {
            menubar::init(app);
            gpui_term::init(app);

            app.set_global(TermuaAppState::default());
            register(app);
            bind_menu_shortcuts(app);
        });

        let cx = cx.add_empty_window();
        cx.draw(
            gpui::point(gpui::px(0.), gpui::px(0.)),
            gpui::size(
                gpui::AvailableSpace::Definite(gpui::px(800.)),
                gpui::AvailableSpace::Definite(gpui::px(600.)),
            ),
            |_, _| div(),
        );
        cx.run_until_parked();

        cx.update(|window, _| {
            let quit = window
                .highest_precedence_binding_for_action(&Quit)
                .expect("Quit should have a binding");
            let expected_quit = if cfg!(target_os = "macos") {
                "cmd-q"
            } else {
                "ctrl-q"
            };
            assert_eq!(quit.keystrokes()[0].as_keystroke().unparse(), expected_quit);

            let new_term = window
                .highest_precedence_binding_for_action(&crate::NewLocalTerminal)
                .expect("NewLocalTerminal should have a binding");
            let expected_new_term = if cfg!(target_os = "macos") {
                "cmd-n"
            } else {
                "ctrl-n"
            };
            assert_eq!(
                new_term.keystrokes()[0].as_keystroke().unparse(),
                expected_new_term
            );
        });
    }

    #[cfg_attr(target_os = "macos", ignore)]
    #[gpui::test]
    fn open_settings_opens_a_single_settings_window(cx: &mut gpui::TestAppContext) {
        let _guard = crate::locale::lock();
        crate::locale::set_locale("en");

        let mut app = cx.app.borrow_mut();
        menubar::init(&mut app);
        gpui_term::init(&mut app);

        app.set_global(TermuaAppState::default());
        register(&mut app);

        assert_eq!(app.windows().len(), 0);
        app.dispatch_action(&OpenSettings);
        assert_eq!(app.windows().len(), 1);

        // Dispatching again should reuse the existing Settings window.
        app.dispatch_action(&OpenSettings);
        assert_eq!(app.windows().len(), 1);
    }

    #[gpui::test]
    fn set_language_rebuilds_app_menus(cx: &mut gpui::TestAppContext) {
        let _guard = crate::locale::lock();
        crate::locale::set_locale("en");

        cx.update(|app| {
            menubar::init(app);
            gpui_term::init(app);

            app.set_global(TermuaAppState::default());
            set_app_menus(app, build_menus(false));
        });

        let snapshot_en = cx.update(|app| app.global::<MenuSnapshot>().clone());
        let Some(termua_menu_en) = snapshot_en.menus.first() else {
            panic!("expected menus to exist");
        };
        let Some(MenuSnapshotItem::Action { name, .. }) = termua_menu_en.items.first() else {
            panic!("expected first Termua menu item to be an action");
        };
        assert_eq!(name, "About Termua");

        cx.update(|app| crate::settings::set_language(crate::settings::Language::ZhCn, app));

        let snapshot_zh = cx.update(|app| app.global::<MenuSnapshot>().clone());
        let Some(termua_menu_zh) = snapshot_zh.menus.first() else {
            panic!("expected menus to exist");
        };
        let Some(MenuSnapshotItem::Action { name, .. }) = termua_menu_zh.items.first() else {
            panic!("expected first Termua menu item to be an action");
        };
        assert_eq!(name, "关于 Termua");
    }
}
