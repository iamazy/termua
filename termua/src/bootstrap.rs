use gpui::{App, AppContext, Application, WindowDecorations, WindowOptions};
use gpui_common::TermuaAssets;
use gpui_component::TitleBar;
use gpui_transfer::TransferCenterState;

use crate::TermuaAppState;

pub(crate) fn run(settings: crate::settings::SettingsFile) {
    Application::new()
        .with_assets(TermuaAssets)
        .run(move |cx: &mut App| {
            init_app(cx, &settings);
        });
}

fn init_app(cx: &mut App, settings: &crate::settings::SettingsFile) {
    menubar::init(cx);
    gpui_term::init(cx);
    gpui_dock::init(cx);
    crate::panel::assistant_panel::bind_keybindings(cx);

    match crate::command_history::CommandHistory::load_default() {
        Ok(history) => {
            gpui_term::set_suggestion_history_provider(cx, Some(std::sync::Arc::new(history)));
        }
        Err(err) => {
            log::warn!("termua: failed to load command history: {err:#}");
        }
    }

    match crate::static_suggestions::StaticSuggestionsDb::load_default() {
        Ok(db) => {
            gpui_term::set_suggestion_static_provider(cx, Some(std::sync::Arc::new(db)));
        }
        Err(err) => {
            log::warn!("termua: failed to load static suggestions: {err:#}");
        }
    }

    let themes_dir = crate::theme_manager::themes_dir_path();
    if let Err(err) = crate::theme_manager::ensure_builtin_themes_installed(&themes_dir) {
        log::warn!("failed to install built-in themes: {err:#}");
    }
    if let Err(err) = crate::theme_manager::watch_user_themes_dir(themes_dir, cx) {
        log::warn!("failed to watch themes directory: {err:#}");
    }

    // Create runtime globals before applying settings so `apply_to_app` can update them.
    cx.set_global(crate::lock_screen::LockState::new_default());

    // Apply settings (including theme mode) even if no terminal is opened on startup.
    // This keeps initial appearance consistent after `gpui_term::init` initializes
    // gpui-component defaults.
    settings.apply_to_app(None, cx);
    settings.apply_terminal_keybindings(cx);

    cx.set_global(TermuaAppState::default());
    cx.set_global(crate::notification::NotifyState::default());
    cx.set_global(crate::right_sidebar::RightSidebarState::default());
    crate::assistant::ensure_app_globals(cx);
    crate::sharing::init_globals(cx);
    cx.set_global(TransferCenterState::default());

    cx.activate(true);
    // Keep app-level handlers for OS menu validation and as a fallback when the focused
    // element tree doesn't contain our window-level action handler (e.g. when a popup menu
    // owns focus). Avoid re-entrant window updates by deferring the mutation.
    crate::menu::register(cx);

    crate::menu::sync_app_menus(cx);
    crate::menu::bind_menu_shortcuts(cx);

    let settings_for_window = settings.clone();
    let main_window = cx
        .open_window(
            WindowOptions {
                titlebar: Some(TitleBar::title_bar_options()),
                window_decorations: cfg!(target_os = "linux").then_some(WindowDecorations::Client),
                ..Default::default()
            },
            move |window, cx| {
                // If theme mode is `System`, we want to sync this window’s appearance
                // immediately on creation (not only on future OS appearance changes).
                settings_for_window.apply_to_app(Some(window), cx);
                let view = cx.new(|cx| crate::window::main_window::TermuaWindow::new(window, cx));
                cx.new(|cx| gpui_component::Root::new(view, window, cx))
            },
        )
        .unwrap();

    cx.global_mut::<TermuaAppState>().main_window = Some(main_window);
}
