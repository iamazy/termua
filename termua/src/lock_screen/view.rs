use gpui::{
    AnyElement, Context, CursorStyle, InteractiveElement as _, IntoElement as _, MouseButton,
    ParentElement as _, SharedString, Styled, Window, div, prelude::FluentBuilder as _,
};
use gpui_component::{
    ActiveTheme as _,
    input::{Input, InputState},
    v_flex,
};
use rust_i18n::t;

pub fn render_lock_overlay<T: 'static>(
    lock_error: Option<SharedString>,
    lock_password_input: gpui::Entity<InputState>,
    unlock: fn(&mut T, &mut Window, &mut Context<T>),
    cx: &mut Context<T>,
) -> AnyElement {
    let left_inset = if cfg!(target_os = "macos") {
        // Leave room for macOS traffic-light window controls.
        gpui::px(84.)
    } else {
        gpui::px(0.)
    };
    let right_inset = if cfg!(target_os = "macos") {
        gpui::px(0.)
    } else {
        // Leave room for right-side window controls.
        gpui::px(140.)
    };

    div()
        .id("termua-lock-overlay")
        .debug_selector(|| "termua-lock-overlay".to_string())
        .absolute()
        .top_0()
        .right_0()
        .bottom_0()
        .left_0()
        .bg(cx.theme().background.opacity(0.92))
        .on_mouse_down(MouseButton::Left, |_ev, _window, cx| {
            cx.stop_propagation();
        })
        .on_mouse_down(MouseButton::Right, |_ev, _window, cx| {
            cx.stop_propagation();
        })
        .on_mouse_move(|_ev, _window, cx| {
            cx.stop_propagation();
        })
        .on_key_down(
            cx.listener(move |this, ev: &gpui::KeyDownEvent, window, cx| {
                if ev.is_held {
                    return;
                }
                if ev.keystroke.key.as_str() == "enter" {
                    unlock(this, window, cx);
                    cx.stop_propagation();
                }
            }),
        )
        .child(
            v_flex().size_full().items_center().justify_center().child(
                v_flex()
                    .w(gpui::px(360.))
                    .gap_2()
                    .child(
                        div()
                            .w_full()
                            .text_center()
                            .text_lg()
                            .text_color(cx.theme().foreground)
                            .child(t!("LockScreen.Title").to_string()),
                    )
                    .when_some(lock_error, |this, err| {
                        this.child(div().text_sm().text_color(cx.theme().danger).child(err))
                    })
                    .child(
                        div()
                            .debug_selector(|| "termua-lock-password-input".to_string())
                            .child(Input::new(&lock_password_input).mask_toggle()),
                    ),
            ),
        )
        .child(
            div()
                .absolute()
                .top_0()
                .left(left_inset)
                .right(right_inset)
                .h(gpui_component::TITLE_BAR_HEIGHT)
                .debug_selector(|| "termua-lock-drag-overlay".to_string())
                .cursor(CursorStyle::OpenHand)
                .on_mouse_down(MouseButton::Left, |_ev, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                    window.start_window_move();
                }),
        )
        .into_any_element()
}
