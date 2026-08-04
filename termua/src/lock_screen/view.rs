use gpui::{
    AnyElement, App, Context, Entity, InteractiveElement as _, IntoElement, MouseButton,
    ParentElement as _, SharedString, Styled, Window, div, prelude::FluentBuilder as _,
};
use gpui_component::{
    ActiveTheme as _, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    input::{Input, InputState},
    v_flex,
};
use rust_i18n::t;

pub(super) fn lock_password_masked_for_reveal(reveal: bool) -> bool {
    !reveal
}

pub(super) fn lock_password_reveal_icon(reveal: bool) -> IconName {
    if reveal {
        IconName::EyeOff
    } else {
        IconName::Eye
    }
}

fn render_lock_password_reveal_button<T: 'static>(
    password_input: Entity<InputState>,
    reveal_pressed: Entity<bool>,
    cx: &mut Context<T>,
) -> impl IntoElement {
    let reveal = *reveal_pressed.read(cx);
    div()
        .debug_selector(|| "termua-lock-password-reveal".to_string())
        .on_mouse_down(MouseButton::Left, {
            let password_input = password_input.clone();
            let reveal_pressed = reveal_pressed.clone();
            move |_ev, window, cx| {
                set_lock_password_reveal(&password_input, &reveal_pressed, true, window, cx);
            }
        })
        .on_mouse_up(MouseButton::Left, move |_ev, window, cx| {
            set_lock_password_reveal(&password_input, &reveal_pressed, false, window, cx);
        })
        .child(
            Button::new("termua-lock-password-reveal-button")
                .icon(lock_password_reveal_icon(reveal))
                .xsmall()
                .ghost()
                .tab_stop(false),
        )
}

fn set_lock_password_reveal(
    password_input: &Entity<InputState>,
    reveal_pressed: &Entity<bool>,
    reveal: bool,
    window: &mut Window,
    cx: &mut App,
) {
    reveal_pressed.update(cx, |pressed, cx| {
        *pressed = reveal;
        cx.notify();
    });
    password_input.update(cx, |state, cx| {
        state.set_masked(lock_password_masked_for_reveal(reveal), window, cx);
    });
    cx.refresh_windows();
    window.prevent_default();
    cx.stop_propagation();
}

pub fn render_lock_overlay<T: 'static>(
    lock_error: Option<SharedString>,
    lock_password_input: Entity<InputState>,
    lock_password_reveal_pressed: Entity<bool>,
    unlock: fn(&mut T, &mut Window, &mut Context<T>),
    cx: &mut Context<T>,
) -> AnyElement {
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
                            .child(Input::new(&lock_password_input).suffix(
                                render_lock_password_reveal_button(
                                    lock_password_input.clone(),
                                    lock_password_reveal_pressed,
                                    cx,
                                ),
                            )),
                    ),
            ),
        )
        .into_any_element()
}
