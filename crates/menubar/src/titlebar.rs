use gpui::{
    Entity, InteractiveElement as _, IntoElement, MouseButton, ParentElement, Render, Styled as _,
};

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
            tb = tb.child(state.read(cx).menubar.clone()).child(
                gpui::div()
                    .h_full()
                    .flex_1()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        gpui_component::GlobalState::suppress_text_selection(cx);
                    }),
            );
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
    use std::sync::Mutex;

    use gpui::{
        AppContext as _, AvailableSpace, Context, InteractiveElement as _, IntoElement, Modifiers,
        MouseButton, ParentElement as _, Render, Styled as _, Window, point, px, size,
    };
    use gpui_base::TextSelection;
    use gpui_component::{
        Root,
        text::{TextView, TextViewState},
    };

    use super::{FORCE_MACOS_ENV, MenubarTitleBar};

    static MACOS_ENV_LOCK: Mutex<()> = Mutex::new(());

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

    struct SelectableContentTestView {
        text: gpui::Entity<TextViewState>,
    }

    impl SelectableContentTestView {
        fn new(cx: &mut Context<Self>) -> Self {
            Self {
                text: cx.new(|cx| TextViewState::markdown("notification message", cx)),
            }
        }
    }

    impl Render for SelectableContentTestView {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            gpui::div()
                .size_full()
                .child(MenubarTitleBar::build(window, cx))
                .child(
                    gpui::div()
                        .h(px(40.))
                        .child(TextView::new(&self.text).selectable(true)),
                )
        }
    }

    #[gpui::test]
    fn macos_fullscreen_hides_titlebar(cx: &mut gpui::TestAppContext) {
        let _env_guard = MACOS_ENV_LOCK.lock().unwrap();

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

    #[cfg_attr(target_os = "macos", ignore)]
    #[gpui::test]
    fn dragging_titlebar_does_not_start_text_selection(cx: &mut gpui::TestAppContext) {
        let _env_guard = MACOS_ENV_LOCK.lock().unwrap();

        cx.update(|app| {
            gpui_component::init(app);
            crate::init(app);
            app.activate(true);
        });

        let (_, cx) = cx.add_window_view(|window, cx| {
            let content = cx.new(SelectableContentTestView::new);
            Root::new(content, window, cx)
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });

        // Native window movement can consume the matching mouse-up. Moving back over selectable
        // content must not extend a selection that accidentally began in the titlebar.
        cx.simulate_mouse_down(
            point(px(400.), px(17.)),
            MouseButton::Left,
            Modifiers::default(),
        );
        cx.simulate_mouse_move(
            point(px(140.), px(50.)),
            Some(MouseButton::Left),
            Modifiers::default(),
        );
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });

        let selected = cx.update(|window, cx| TextSelection::selected_text(window, cx));
        assert_eq!(selected, "");
    }
}
