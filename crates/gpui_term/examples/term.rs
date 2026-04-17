use std::collections::HashMap;

use gpui::{
    App, AppContext, Application, Bounds, Focusable, WindowBackgroundAppearance, WindowBounds,
    WindowOptions, px, size,
};
use gpui_component::Root;
use gpui_component_assets::Assets;
use gpui_term::{CursorShape, TerminalBuilder, TerminalType, TerminalView};

fn main() {
    env_logger::init();
    Application::new().with_assets(Assets).run(|cx: &mut App| {
        gpui_term::init(cx);

        let bounds = Bounds::centered(None, size(px(1000.0), px(800.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |window, cx| {
                if std::env::var_os("WAYLAND_DISPLAY").is_some() {
                    window.set_background_appearance(WindowBackgroundAppearance::Transparent);
                }
                let terminal = cx.new(|cx| {
                    TerminalBuilder::new(
                        TerminalType::Alacritty,
                        HashMap::default(),
                        CursorShape::default(),
                        None,
                        0,
                    )
                    .unwrap()
                    .subscribe(cx)
                });
                let terminal_view = cx.new(|cx| TerminalView::new(terminal, window, cx));
                let root = cx.new(|cx| Root::new(terminal_view.clone(), window, cx));

                let focus_handle = terminal_view.read(cx).focus_handle(cx);
                window.focus(&focus_handle, cx);
                cx.activate(true);
                root
            },
        )
        .unwrap();
    });
}
