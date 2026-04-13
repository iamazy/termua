use gpui::App;

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
