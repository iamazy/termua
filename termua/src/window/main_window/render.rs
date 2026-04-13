//! TermuaWindow rendering.

use gpui::{
    Context, InteractiveElement, IntoElement, ParentElement, Render, Styled, Window, div,
    prelude::FluentBuilder,
};
use gpui_component::v_flex;
use menubar::MenubarTitleBar;

use super::TermuaWindow;
use crate::{TermuaAppState, lock_screen, right_sidebar};

impl TermuaWindow {
    fn sync_sidebar_docks(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let sessions_visible = cx.global::<TermuaAppState>().sessions_sidebar_visible;
        let sessions_width = cx.global::<TermuaAppState>().sessions_sidebar_width;
        let right_visible = cx.global::<right_sidebar::RightSidebarState>().visible;
        let right_width = cx.global::<right_sidebar::RightSidebarState>().width;

        let (left_dock, right_dock) = {
            let dock_area = self.dock_area.read(cx);
            (
                dock_area.left_dock().cloned(),
                dock_area.right_dock().cloned(),
            )
        };

        if let Some(left_dock) = left_dock {
            if left_dock.read(cx).is_open() != sessions_visible {
                left_dock.update(cx, |dock, cx| dock.set_open(sessions_visible, window, cx));
            }
            let live = left_dock.read(cx).size();
            if live != sessions_width {
                cx.global_mut::<TermuaAppState>().sessions_sidebar_width = live;
            }
        }

        if let Some(right_dock) = right_dock {
            if right_dock.read(cx).is_open() != right_visible {
                right_dock.update(cx, |dock, cx| dock.set_open(right_visible, window, cx));
            }
            let live = right_dock.read(cx).size();
            if live != right_width {
                cx.global_mut::<right_sidebar::RightSidebarState>().width = live;
            }
        }
    }

    fn render_lock_overlay(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        self.lock_overlay
            .render_overlay_if_locked(Self::unlock_from_overlay, cx)
    }

    fn render_center_area(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        self.sync_sidebar_docks(window, cx);

        div()
            .flex_1()
            .min_h_0()
            .w_full()
            .h_full()
            .relative()
            .child(self.dock_area.clone())
            .into_any_element()
    }
}

impl Render for TermuaWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let lock_overlay = self.render_lock_overlay(cx);
        let center = self.render_center_area(window, cx);

        v_flex()
            .size_full()
            .items_stretch()
            // Consider any interaction within the window as "activity" for the lock timer.
            // This prevents the app from locking while the user is interacting with non-terminal
            // components (e.g. SFTP, settings panels, etc.).
            .on_any_mouse_down(|_ev, _window, cx| {
                cx.global::<lock_screen::LockState>().report_activity();
            })
            .on_mouse_move(|_ev, _window, cx| {
                cx.global::<lock_screen::LockState>().report_activity();
            })
            .on_key_down(cx.listener(|_this, ev: &gpui::KeyDownEvent, _window, cx| {
                if ev.is_held {
                    return;
                }
                cx.global::<lock_screen::LockState>().report_activity();
            }))
            // Handle app-level menu actions inside the window update cycle (menu dispatch happens
            // via `window.dispatch_action`, so calling `WindowHandle::update` from an app-level
            // `cx.on_action` handler can fail due to re-entrancy).
            .on_action(cx.listener(Self::on_new_local_terminal))
            .on_action(cx.listener(Self::on_play_cast))
            .on_action(cx.listener(Self::on_open_sftp))
            .on_action(cx.listener(Self::on_start_sharing))
            .on_action(cx.listener(Self::on_stop_sharing))
            .on_action(cx.listener(Self::on_request_control))
            .on_action(cx.listener(Self::on_release_control))
            .on_action(cx.listener(Self::on_revoke_control))
            .child(MenubarTitleBar::build(window, cx))
            .child(center)
            .child(self.footbar.clone())
            .children(gpui_component::Root::render_sheet_layer(window, cx))
            .children(gpui_component::Root::render_dialog_layer(window, cx))
            .children(gpui_component::Root::render_notification_layer(window, cx))
            .when_some(lock_overlay, |this, overlay| this.child(overlay))
    }
}
