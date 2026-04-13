use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement, Pixels, Styled, div, px,
};
use gpui_component::ActiveTheme;

#[derive(Clone, Debug)]
pub struct TransferFooterBar {
    pub id: &'static str,
    pub title: String,
    pub bar_width: Pixels,
    pub fill_width: Pixels,
}

pub fn render_transfer_footer_bar<T: 'static>(
    bar: TransferFooterBar,
    cx: &mut Context<T>,
) -> AnyElement {
    render_transfer_footer_bar_with_action(bar, None, cx)
}

pub fn render_transfer_footer_bar_with_action<T: 'static>(
    bar: TransferFooterBar,
    action: Option<AnyElement>,
    cx: &mut Context<T>,
) -> AnyElement {
    let (border, popover, muted_foreground, muted, selection) = {
        let t = cx.theme();
        (
            t.border,
            t.popover,
            t.muted_foreground,
            t.muted,
            t.selection,
        )
    };

    let mut root = div()
        .id(bar.id)
        .relative()
        .px(px(10.0))
        .py(px(8.0))
        .border_t_1()
        .border_color(border.opacity(0.8))
        .bg(popover.opacity(0.65))
        .child(
            div()
                .text_xs()
                .text_color(muted_foreground)
                .child(bar.title),
        )
        .child(
            div()
                .mt(px(6.0))
                .w(bar.bar_width)
                .rounded_sm()
                .bg(muted.opacity(0.25))
                .py(px(3.0))
                .child(
                    div()
                        .w(bar.fill_width)
                        .rounded_sm()
                        .bg(selection)
                        .py(px(3.0)),
                ),
        );

    if let Some(action) = action {
        root = root.child(div().absolute().top(px(6.0)).right(px(10.0)).child(action));
    }

    root.into_any_element()
}
