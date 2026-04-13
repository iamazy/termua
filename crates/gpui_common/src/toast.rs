use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, MouseButton, ParentElement, Pixels,
    PromptLevel, Styled, Window, div, px,
};
use gpui_component::{ActiveTheme, Icon, IconName, Sizable};

use crate::CloseFn;

#[derive(Clone, Debug)]
pub struct Toast {
    pub level: PromptLevel,
    pub title: String,
    pub detail: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastVariant {
    /// Popover-colored panel with a left severity stripe, intended for "global" overlays.
    StripedPopover,
    /// Severity-colored solid panel (info/warn/danger), intended for local views.
    SolidSeverity,
}

#[derive(Clone, Copy, Debug)]
pub struct ToastRenderOptions {
    pub id: &'static str,
    pub variant: ToastVariant,
    pub right: Pixels,
    pub bottom: Pixels,
    pub ideal_width: Pixels,
    pub min_width: Pixels,
    pub max_width: Pixels,
    /// Horizontal padding to subtract from the viewport when clamping toast width.
    pub viewport_width_padding: Pixels,
    pub panel_opacity: f32,
    pub show_close_button: bool,
}

impl ToastRenderOptions {
    pub fn striped_popover(id: &'static str) -> Self {
        Self {
            id,
            variant: ToastVariant::StripedPopover,
            right: px(12.0),
            bottom: px(12.0),
            ideal_width: px(420.0),
            min_width: px(260.0),
            max_width: px(520.0),
            viewport_width_padding: px(24.0),
            panel_opacity: 0.98,
            show_close_button: true,
        }
    }

    pub fn solid_severity(id: &'static str) -> Self {
        Self {
            id,
            variant: ToastVariant::SolidSeverity,
            right: px(12.0),
            bottom: px(12.0),
            ideal_width: px(420.0),
            min_width: px(260.0),
            max_width: px(520.0),
            viewport_width_padding: px(24.0),
            panel_opacity: 0.95,
            show_close_button: false,
        }
    }
}

pub fn render_toast<T: 'static>(
    toast: &Toast,
    window: &Window,
    cx: &mut Context<T>,
    opts: ToastRenderOptions,
    on_close: Option<CloseFn<T>>,
) -> AnyElement {
    // Avoid holding an immutable borrow of `cx` across `cx.listener` calls.
    let (
        popover,
        popover_foreground,
        border,
        muted,
        muted_foreground,
        info,
        info_foreground,
        warning,
        warning_foreground,
        danger,
        danger_foreground,
    ) = {
        let t = cx.theme();
        (
            t.popover,
            t.popover_foreground,
            t.border,
            t.muted,
            t.muted_foreground,
            t.info,
            t.info_foreground,
            t.warning,
            t.warning_foreground,
            t.danger,
            t.danger_foreground,
        )
    };

    let (sev_bg, sev_fg, icon) = match toast.level {
        PromptLevel::Info => (info, info_foreground, IconName::Info),
        PromptLevel::Warning => (warning, warning_foreground, IconName::TriangleAlert),
        PromptLevel::Critical => (danger, danger_foreground, IconName::CircleX),
    };

    let viewport = window.viewport_size();
    let toast_w = opts
        .ideal_width
        .min((viewport.width - opts.viewport_width_padding).max(Pixels::ZERO))
        .max(opts.min_width.min(viewport.width.max(Pixels::ZERO)));

    let close_button = if opts.show_close_button {
        on_close.clone().map(|on_close| {
            div()
                .absolute()
                .top(px(8.0))
                .right(px(8.0))
                .cursor_pointer()
                .rounded_md()
                .w(px(28.0))
                .h(px(28.0))
                .flex()
                .items_center()
                .justify_center()
                .bg(muted.opacity(0.25))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        (on_close)(this, cx);
                        cx.stop_propagation();
                    }),
                )
                .child(Icon::new(IconName::Close).small())
                .into_any_element()
        })
    } else {
        None
    };

    match opts.variant {
        ToastVariant::StripedPopover => {
            let panel_bg = popover.opacity(opts.panel_opacity);
            let panel_border = border.opacity(0.9);

            // Layout strategy:
            // - Use an absolute left stripe and absolute close button so text can wrap naturally.
            let mut content = div()
                .relative()
                .p(px(10.0))
                .pl(px(14.0)) // leave room for the stripe
                .pr(if close_button.is_some() {
                    px(42.0) // leave room for the close button
                } else {
                    px(10.0)
                })
                .flex_col()
                .child(
                    div()
                        .flex()
                        .items_start()
                        .child(
                            div()
                                .mr(px(8.0))
                                .text_color(sev_fg)
                                .child(Icon::new(icon).small()),
                        )
                        .child(
                            div()
                                .text_sm()
                                .whitespace_normal()
                                .child(toast.title.clone()),
                        ),
                );

            if let Some(detail) = toast.detail.as_ref() {
                content = content.child(
                    div()
                        .mt(px(6.0))
                        .text_xs()
                        .text_color(muted_foreground)
                        .whitespace_normal()
                        .child(detail.clone()),
                );
            }

            let mut body = div()
                .relative()
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .bottom_0()
                        .w(px(4.0))
                        .bg(sev_bg),
                )
                .child(content);

            if let Some(close_button) = close_button {
                body = body.child(close_button);
            }

            div()
                .id(opts.id)
                .absolute()
                .right(opts.right)
                .bottom(opts.bottom)
                // Fixed/limited width, but let height grow with content.
                .w(toast_w)
                .max_w(opts.max_width)
                .bg(panel_bg)
                .border_1()
                .border_color(panel_border)
                .rounded_md()
                .shadow_lg()
                .text_color(popover_foreground)
                .child(body)
                .into_any_element()
        }
        ToastVariant::SolidSeverity => {
            let panel_bg = popover.opacity(opts.panel_opacity);
            let panel_border = border.opacity(0.9);

            let mut content = div().relative().p(px(10.0)).pl(px(14.0)).flex_col().child(
                div()
                    .flex()
                    .items_start()
                    .child(
                        div()
                            .mr(px(8.0))
                            .text_color(sev_fg)
                            .child(Icon::new(icon).small()),
                    )
                    .child(
                        div()
                            .text_sm()
                            .whitespace_normal()
                            .child(toast.title.clone()),
                    ),
            );

            if let Some(detail) = toast.detail.as_ref() {
                content = content.child(
                    div()
                        .mt(px(6.0))
                        .text_xs()
                        .text_color(muted_foreground)
                        .whitespace_normal()
                        .child(detail.clone()),
                );
            }

            div()
                .id(opts.id)
                .absolute()
                .right(opts.right)
                .bottom(opts.bottom)
                .w(toast_w)
                .max_w(opts.max_width)
                .bg(panel_bg)
                .border_1()
                .border_color(panel_border)
                .rounded_md()
                .shadow_lg()
                .text_color(popover_foreground)
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .bottom_0()
                        .w(px(4.0))
                        .bg(sev_bg),
                )
                .child(content)
                .into_any_element()
        }
    }
}
