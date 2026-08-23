use gpui::App;

#[derive(Clone, Copy, Debug)]
pub struct MenuBarSettings {
    pub auto_collapse: bool,
}

impl Default for MenuBarSettings {
    fn default() -> Self {
        Self {
            auto_collapse: true,
        }
    }
}

impl gpui::Global for MenuBarSettings {}

pub fn set_auto_collapse(auto_collapse: bool, cx: &mut App) {
    if cx.has_global::<MenuBarSettings>() {
        cx.global_mut::<MenuBarSettings>().auto_collapse = auto_collapse;
    } else {
        cx.set_global(MenuBarSettings { auto_collapse });
    }
    cx.refresh_windows();
}

pub fn auto_collapse(cx: &App) -> bool {
    cx.try_global::<MenuBarSettings>()
        .map_or(true, |settings| settings.auto_collapse)
}

rust_i18n::i18n!("../../locales");

mod menu_bar;
mod state;
mod titlebar;

pub use menu_bar::FoldableAppMenuBar;
pub use titlebar::MenubarTitleBar;

/// Initialize gpui_menubar (keybindings for menubar context).
///
/// Note: this does not call `gpui_component::init(cx)`. The application should do that once.
pub fn init(cx: &mut App) {
    menu_bar::init(cx);
}
