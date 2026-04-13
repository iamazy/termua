use gpui::{Entity, IntoElement, ParentElement, Render, Styled as _};

/// Convenience wrapper to build a titlebar that includes the in-window menubar (Linux/Windows).
pub struct MenubarTitleBar;

#[cfg(test)]
const FORCE_MACOS_ENV: &str = "MENUBAR_FORCE_MACOS";

fn is_macos() -> bool {
    #[cfg(test)]
    if std::env::var_os(FORCE_MACOS_ENV).is_some() {
        return true;
    }

    cfg!(target_os = "macos")
}

// Persist the menubar entity across frames. `MenubarTitleBar::build` is typically called from a
// view's `render`, so creating the menubar inside `build` would otherwise recreate it every frame
// and reset all interaction state.
struct MenubarTitleBarState {
    menubar: Entity<crate::FoldableAppMenuBar>,
}

// TODO: Remove this when GPUI has released v0.2.3 (mirrors gpui-component's TitleBarState).
impl Render for MenubarTitleBarState {
    fn render(&mut self, _: &mut gpui::Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        gpui::div()
    }
}

impl MenubarTitleBar {
    pub fn build(window: &mut gpui::Window, cx: &mut gpui::App) -> gpui_component::TitleBar {
        let is_macos = is_macos();
        let mut tb = gpui_component::TitleBar::new();

        if !is_macos {
            let state = window.use_state(cx, |window, cx| MenubarTitleBarState {
                menubar: crate::FoldableAppMenuBar::new(window, cx),
            });
            tb = tb.child(state.read(cx).menubar.clone());
        }

        // On macOS we use the native OS menubar, so the in-window titlebar is usually redundant.
        // When the window enters fullscreen, hide the titlebar to avoid leaving an empty top strip.
        if is_macos && window.is_fullscreen() {
            tb = tb
                .h(gpui::px(0.))
                .border_0()
                .p(gpui::px(0.))
                .overflow_hidden();
        }

        tb
    }
}

#[cfg(test)]
mod tests {
    use gpui::{
        AvailableSpace, Context, InteractiveElement as _, IntoElement, ParentElement as _, Render,
        Styled as _, Window, point, px, size,
    };

    use super::{FORCE_MACOS_ENV, MenubarTitleBar};

    struct TitlebarTestView;

    impl Render for TitlebarTestView {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            gpui::div().size_full().child(
                gpui::div()
                    .debug_selector(|| "menubar-titlebar".to_string())
                    .child(MenubarTitleBar::build(window, cx)),
            )
        }
    }

    #[gpui::test]
    fn macos_fullscreen_hides_titlebar(cx: &mut gpui::TestAppContext) {
        cx.update(|app| {
            gpui_component::init(app);
            crate::init(app);
            app.activate(true);
        });

        let (root, cx) = cx.add_window_view(|_, _| TitlebarTestView);

        // This is a macOS-only behavior, but we force-enable it in tests so we can validate the
        // layout change on any host OS.
        unsafe {
            std::env::set_var(FORCE_MACOS_ENV, "1");
        }

        cx.update(|window, _| {
            if !window.is_fullscreen() {
                window.toggle_fullscreen();
            }
        });

        cx.draw(
            point(px(0.), px(0.)),
            size(
                AvailableSpace::Definite(px(900.)),
                AvailableSpace::Definite(px(600.)),
            ),
            move |_, _| gpui::div().size_full().child(root),
        );
        cx.run_until_parked();

        let bounds = cx
            .debug_bounds("menubar-titlebar")
            .expect("expected debug selector to be present");
        assert_eq!(bounds.size.height, px(0.));

        unsafe {
            std::env::remove_var(FORCE_MACOS_ENV);
        }
    }
}
