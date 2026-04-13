use gpui::{AnyElement, AppContext, Context, Entity, SharedString, Window};
use gpui_component::input::InputState;
use rust_i18n::t;

use super::LockState;

pub struct LockOverlayState {
    pub password_input: Entity<InputState>,
    pub error: Option<SharedString>,
}

fn set_input_placeholder<T>(
    input: &Entity<InputState>,
    placeholder: String,
    window: &mut Window,
    cx: &mut Context<T>,
) {
    input.update(cx, |state, cx| {
        state.set_placeholder(placeholder, window, cx);
    });
}

impl LockOverlayState {
    pub fn new<T: 'static>(window: &mut Window, cx: &mut Context<T>) -> Self {
        if cx.try_global::<LockState>().is_none() {
            cx.set_global(LockState::new_default());
        }

        let password_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("LockScreen.Placeholder.Password").to_string())
                .masked(true)
        });

        Self {
            password_input,
            error: None,
        }
    }

    pub fn render_overlay_if_locked<T: 'static>(
        &self,
        unlock: fn(&mut T, &mut Window, &mut Context<T>),
        cx: &mut Context<T>,
    ) -> Option<AnyElement> {
        if !cx.global::<LockState>().locked() {
            return None;
        }

        Some(crate::lock_screen::view::render_lock_overlay(
            self.error.clone(),
            self.password_input.clone(),
            unlock,
            cx,
        ))
    }

    fn clear_password_input<T>(&mut self, window: &mut Window, cx: &mut Context<T>) {
        self.password_input.update(cx, |state, cx| {
            state.set_value("", window, cx);
            state.set_masked(true, window, cx);
        });
    }

    pub fn unlock_with_password<T: 'static>(&mut self, window: &mut Window, cx: &mut Context<T>) {
        let password = self.password_input.read(cx).value().to_string();
        match cx.global_mut::<LockState>().try_unlock(&password) {
            Ok(true) => {
                self.clear_password_input(window, cx);
                self.error = None;
            }
            Ok(false) => {
                self.clear_password_input(window, cx);
                self.error = Some(t!("LockScreen.Error.IncorrectPassword").to_string().into());
            }
            Err(err) => {
                self.clear_password_input(window, cx);
                self.error = Some(err.to_string().into());
            }
        }
        cx.refresh_windows();
        cx.notify();
        window.refresh();
    }

    pub fn sync_localized_placeholders<T>(&self, window: &mut Window, cx: &mut Context<T>) {
        set_input_placeholder(
            &self.password_input,
            t!("LockScreen.Placeholder.Password").to_string(),
            window,
            cx,
        );
    }
}
