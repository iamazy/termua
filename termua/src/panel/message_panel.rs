use gpui::{
    App, ClipboardItem, Context, ElementId, FocusHandle, Focusable, InteractiveElement as _,
    IntoElement as _, ParentElement as _, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, px,
};
use gpui_common::TermuaIcon;
use gpui_component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    list::ListItem,
    scroll::{Scrollbar, ScrollbarShow},
    text::TextView,
    v_flex,
};
use rust_i18n::t;

use crate::notification::{MessageEntry, MessageKind, NotifyState};

pub struct MessageCenterView {
    focus_handle: FocusHandle,
    scroll_handle: ScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl Focusable for MessageCenterView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl MessageCenterView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        if cx.try_global::<NotifyState>().is_none() {
            cx.set_global(NotifyState::default());
        }

        let subs = vec![
            cx.observe_global::<NotifyState>(|_, cx| cx.notify()),
            cx.observe_window_activation(window, |_, _, cx| cx.notify()),
        ];

        Self {
            focus_handle: cx.focus_handle(),
            scroll_handle: ScrollHandle::default(),
            _subscriptions: subs,
        }
    }

    fn icon_for_kind(kind: MessageKind, cx: &App) -> gpui_component::Icon {
        match kind {
            MessageKind::Info => {
                gpui_component::Icon::new(IconName::Info).text_color(cx.theme().info)
            }
            MessageKind::Success => {
                gpui_component::Icon::new(IconName::CircleCheck).text_color(cx.theme().success)
            }
            MessageKind::Warning => {
                gpui_component::Icon::new(IconName::TriangleAlert).text_color(cx.theme().warning)
            }
            MessageKind::Error => {
                gpui_component::Icon::new(IconName::CircleX).text_color(cx.theme().danger)
            }
        }
    }

    fn render_row(entry: &MessageEntry, ix: usize, cx: &App) -> gpui::AnyElement {
        let id = entry.id;
        let message: SharedString = entry.message.clone();
        let icon = Self::icon_for_kind(entry.kind, cx).size_4();

        ListItem::new(ix)
            .text_sm()
            .py_0p5()
            .px_2()
            .child(
                h_flex()
                    .items_start()
                    .gap_1()
                    .child(div().mt_0p5().flex_shrink_0().child(icon))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .debug_selector(move || format!("termua-messages-item-{id}"))
                            .child(
                                TextView::markdown(
                                    ElementId::NamedInteger("termua-messages-text".into(), id),
                                    message,
                                )
                                .selectable(true),
                            ),
                    )
                    .child(
                        Button::new(ElementId::NamedInteger("termua-messages-copy".into(), id))
                            .xsmall()
                            .ghost()
                            .icon(Icon::new(IconName::Copy))
                            .tooltip(t!("Notifications.Tooltip.Copy").to_string())
                            .on_click({
                                let message = entry.message.clone();
                                move |_, _window, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        message.to_string(),
                                    ));
                                }
                            }),
                    ),
            )
            .into_any_element()
    }
}

impl Render for MessageCenterView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        let state = cx.global::<NotifyState>();
        let entries: Vec<MessageEntry> = state.messages.iter().cloned().collect();
        let can_clear = !entries.is_empty();
        let tab_header_height = px(32.);

        v_flex()
            .id("termua-messages-view")
            .size_full()
            .min_h_0()
            .items_stretch()
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .h(tab_header_height)
                    .text_sm()
                    .px_2()
                    .border_b_1()
                    .border_color(cx.theme().border.opacity(0.8))
                    .child(
                        h_flex()
                            .items_center()
                            .gap_1()
                            .child(
                                div()
                                    .debug_selector(|| "termua-messages-header-icon".to_string())
                                    .child(Icon::default().path(TermuaIcon::Message).size_4()),
                            )
                            .child(div().child(t!("Notifications.Title").to_string())),
                    )
                    .child(
                        Button::new("termua-messages-clear")
                            .xsmall()
                            .ghost()
                            .icon(Icon::default().path(TermuaIcon::Brush))
                            .disabled(!can_clear)
                            .on_click(|_, window: &mut Window, cx: &mut App| {
                                cx.global_mut::<NotifyState>().clear();
                                cx.refresh_windows();
                                window.refresh();
                            }),
                    ),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_stretch()
                    .child(
                        div()
                            .id("termua-messages-scroll-area")
                            .flex_1()
                            .min_h_0()
                            .flex_col()
                            .track_scroll(&self.scroll_handle)
                            .overflow_y_scroll()
                            .overflow_x_hidden()
                            .child(
                                v_flex().w_full().p_2().gap_1().children(
                                    entries
                                        .iter()
                                        .enumerate()
                                        .map(|(ix, entry)| Self::render_row(entry, ix, cx)),
                                ),
                            ),
                    )
                    .child(
                        div()
                            .w(px(16.0))
                            .flex_shrink_0()
                            .relative()
                            .h_full()
                            .min_h_0()
                            .child(
                                Scrollbar::vertical(&self.scroll_handle)
                                    .id("termua-messages-scrollbar")
                                    .scrollbar_show(ScrollbarShow::Always),
                            ),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use gpui::{AppContext, AvailableSpace, point, px, size};

    use super::*;

    #[gpui::test]
    fn notifications_panel_renders_header_icon(cx: &mut gpui::TestAppContext) {
        cx.update(|app| {
            gpui_component::init(app);
            gpui_term::init(app);
        });

        let (root, window_cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| MessageCenterView::new(window, cx));
            gpui_component::Root::new(view, window, cx)
        });

        window_cx.draw(
            point(px(0.), px(0.)),
            size(
                AvailableSpace::Definite(px(520.)),
                AvailableSpace::Definite(px(240.)),
            ),
            move |_, _| div().size_full().child(root),
        );
        window_cx.run_until_parked();

        window_cx
            .debug_bounds("termua-messages-header-icon")
            .expect("expected notifications header icon to render");
    }
}
