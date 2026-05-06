use gpui::{
    App, AppContext, Bounds, Context, FocusHandle, Focusable, FontWeight, InteractiveElement,
    IntoElement, KeyDownEvent, ObjectFit, ParentElement, Render, Styled, StyledImage, Window,
    WindowBounds, WindowDecorations, WindowHandle, WindowOptions, div, img, px, size,
};
use gpui_common::TermuaIcon;
use gpui_component::{ActiveTheme, Icon, Root, Sizable, TitleBar, h_flex, text::TextView};
use rust_i18n::t;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const GIT_COMMIT: &str = include_str!(concat!(env!("OUT_DIR"), "/git-commit.txt"));

pub struct AboutWindow {
    focus_handle: FocusHandle,
    version: String,
    commit: String,
}

impl AboutWindow {
    pub fn open(app: &mut App) -> anyhow::Result<WindowHandle<Root>> {
        let initial_size = size(px(400.), px(320.));
        let initial_bounds = Bounds::centered(None, initial_size, app);

        let handle = app.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(initial_bounds)),
                titlebar: Some(TitleBar::title_bar_options()),
                window_decorations: cfg!(target_os = "linux").then_some(WindowDecorations::Client),
                window_min_size: Some(initial_size),
                ..Default::default()
            },
            |window, cx| {
                window.set_window_title(t!("About.Title").as_ref());
                let view = cx.new(|cx| Self::new(window, cx));
                cx.new(|cx| Root::new(view, window, cx))
            },
        )?;
        Ok(handle)
    }

    fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            version: VERSION.to_string(),
            commit: GIT_COMMIT.trim().to_string(),
        }
    }
}

impl Focusable for AboutWindow {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for AboutWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                TitleBar::new().child(
                    h_flex()
                        .id("termua-about-titlebar-left")
                        .items_center()
                        .gap_x_1()
                        .child(
                            div()
                                .debug_selector(|| "termua-about-titlebar-icon".to_string())
                                .child(Icon::default().path(TermuaIcon::Info).small()),
                        )
                        .child(div().text_sm().child(t!("About.Title").to_string())),
                ),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_4()
                    .child(
                        img(TermuaIcon::Termua)
                            .w(px(64.))
                            .h(px(64.))
                            .object_fit(ObjectFit::Contain),
                    )
                    .child(
                        div()
                            .text_xl()
                            .font_weight(FontWeight::BOLD)
                            .child(format!("Termua {}", self.version)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap_1()
                            .text_sm()
                            .child("Commit")
                            .child(
                                div().min_w_0().child(
                                    TextView::markdown("termua-about-commit", self.commit.as_str())
                                        .selectable(true)
                                        .text_color(cx.theme().muted_foreground),
                                ),
                            ),
                    ),
            )
            .on_key_down(cx.listener(|_this, ev: &KeyDownEvent, window, _cx| {
                if ev.keystroke.key.as_str() == "escape" {
                    window.remove_window();
                }
            }))
            .children(gpui_component::Root::render_dialog_layer(window, cx))
    }
}
