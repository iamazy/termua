use std::collections::VecDeque;

use gpui::{
    AnyElement, App, Context, IntoElement, ParentElement, SharedString, Styled, Window, div,
};
use gpui_component::{ActiveTheme, Icon, IconName, Sizable, h_flex, notification::Notification};

use crate::globals::{ensure_app_global, ensure_ctx_global};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MessageKind {
    Info,
    Success,
    Warning,
    Error,
}

impl MessageKind {
    fn as_notification(self, message: SharedString) -> Notification {
        let message = message.as_ref().trim().to_string();
        if message.is_empty() {
            return Notification::new();
        }

        let message: SharedString = message.into();
        let icon_name = match self {
            Self::Info => IconName::Info,
            Self::Success => IconName::CircleCheck,
            Self::Warning => IconName::TriangleAlert,
            Self::Error => IconName::CircleX,
        };

        Notification::new().autohide(true).content({
            let message_for_content = message;
            let icon_name_for_content = icon_name;
            move |_, _window: &mut Window, cx: &mut Context<Notification>| -> AnyElement {
                // Keep the icon inline with the first line of text.
                let sev_color = match self {
                    Self::Info => cx.theme().info,
                    Self::Success => cx.theme().success,
                    Self::Warning => cx.theme().warning,
                    Self::Error => cx.theme().danger,
                };

                let icon = Icon::new(icon_name_for_content.clone())
                    .text_color(sev_color)
                    .small();

                h_flex()
                    .items_start()
                    .gap_2()
                    .child(div().mt_0p5().child(icon))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .whitespace_normal()
                            .child(message_for_content.clone()),
                    )
                    .into_any_element()
            }
        })
    }
}

#[derive(Clone, Debug)]
pub struct MessageEntry {
    pub id: u64,
    pub kind: MessageKind,
    pub message: SharedString,
}

pub struct NotifyState {
    pub messages: VecDeque<MessageEntry>,
    next_id: u64,
    max_messages: usize,
}

impl Default for NotifyState {
    fn default() -> Self {
        Self {
            messages: VecDeque::new(),
            next_id: 1,
            max_messages: 200,
        }
    }
}

impl gpui::Global for NotifyState {}

impl NotifyState {
    pub fn clear(&mut self) {
        self.messages.clear();
    }

    pub fn push(&mut self, kind: MessageKind, message: SharedString) -> u64 {
        let message = message.as_ref().trim().to_string();
        if message.is_empty() {
            return 0;
        }

        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);

        self.messages.push_back(MessageEntry {
            id,
            kind,
            message: message.into(),
        });

        while self.messages.len() > self.max_messages {
            self.messages.pop_front();
        }

        id
    }
}

fn push_toast_in_window_with_app(
    notification: Notification,
    window: &mut gpui::Window,
    app: &mut App,
) {
    gpui_component::Root::update(window, app, |root, window, cx| {
        root.notification.update(cx, |list, cx| {
            list.push(notification, window, cx);
        });
    });
}

fn push_toast_in_window_with_context<T>(
    notification: Notification,
    window: &mut gpui::Window,
    cx: &mut Context<T>,
) {
    let Some(Some(root)) = window.root::<gpui_component::Root>() else {
        log::warn!("termua: notification requested but window root is not gpui_component::Root");
        return;
    };

    root.update(cx, |root, cx| {
        root.notification.update(cx, |list, cx| {
            list.push(notification, window, cx);
        });
    });
}

pub fn notify_app(
    kind: MessageKind,
    message: impl Into<SharedString>,
    window: &mut gpui::Window,
    app: &mut App,
) {
    ensure_app_global::<NotifyState>(app);

    let message: SharedString = message.into();
    app.global_mut::<NotifyState>().push(kind, message.clone());
    push_toast_in_window_with_app(kind.as_notification(message), window, app);
    app.refresh_windows();
}

pub fn notify<T>(
    kind: MessageKind,
    message: impl Into<SharedString>,
    window: &mut gpui::Window,
    cx: &mut Context<T>,
) {
    ensure_ctx_global::<NotifyState, _>(cx);

    let message: SharedString = message.into();
    cx.global_mut::<NotifyState>().push(kind, message.clone());
    push_toast_in_window_with_context(kind.as_notification(message), window, cx);
    cx.refresh_windows();
}

pub fn notify_deferred<T>(
    kind: MessageKind,
    message: impl Into<SharedString>,
    window: &mut gpui::Window,
    cx: &mut Context<T>,
) {
    let message: SharedString = message.into();
    window.defer(cx, move |window, app| {
        notify_app(kind, message, window, app);
    });
}

pub fn record<T>(kind: MessageKind, message: impl Into<SharedString>, cx: &mut Context<T>) {
    ensure_ctx_global::<NotifyState, _>(cx);

    let message: SharedString = message.into();
    cx.global_mut::<NotifyState>().push(kind, message);
    cx.refresh_windows();
}
