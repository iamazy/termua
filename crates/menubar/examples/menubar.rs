use gpui::{
    App, Context, Menu, MenuItem, Window, WindowDecorations, WindowOptions, actions, div,
    prelude::*,
};
use gpui_component::{Root, TitleBar, v_flex};
use gpui_component_assets::Assets;
use menubar::MenubarTitleBar;

struct ExampleView;

impl Render for ExampleView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .child(MenubarTitleBar::build(window, cx))
            .child(
                div()
                    .flex_1()
                    .justify_center()
                    .items_center()
                    .child("gpui_menubar example"),
            )
    }
}

fn main() {
    env_logger::init();
    gpui_platform::application()
        .with_assets(Assets)
        .run(|cx: &mut App| {
            gpui_component::init(cx);
            menubar::init(cx);

            cx.activate(true);
            cx.on_action(quit);
            cx.on_action(about);

            // menus[0] is the fold/app menu.
            cx.set_menus(vec![
                Menu::new("Menu").items(vec![
                    MenuItem::action("About", About),
                    MenuItem::separator(),
                    MenuItem::action("Quit", Quit),
                ]),
                Menu::new("File").items(vec![
                    MenuItem::action("New", NewFile),
                    MenuItem::separator(),
                    MenuItem::action("Close", CloseFile),
                ]),
                Menu::new("Edit").items(vec![
                    MenuItem::action("Copy", Copy),
                    MenuItem::action("Paste", Paste),
                    MenuItem::action("Select All", SelectAll),
                ]),
            ]);

            cx.open_window(
                WindowOptions {
                    titlebar: Some(TitleBar::title_bar_options()),
                    window_decorations: cfg!(target_os = "linux")
                        .then_some(WindowDecorations::Client),
                    ..Default::default()
                },
                |window, cx| {
                    let view = cx.new(|_| ExampleView);
                    let root = cx.new(|cx| Root::new(view, window, cx));
                    cx.activate(true);
                    root
                },
            )
            .unwrap();
        });
}

actions!(
    menubar_example,
    [Quit, About, NewFile, CloseFile, Copy, Paste, SelectAll]
);

fn quit(_: &Quit, cx: &mut App) {
    cx.quit();
}

fn about(_: &About, _: &mut App) {
    // Keep it simple for a smoke example.
    eprintln!("gpui_menubar example");
}
