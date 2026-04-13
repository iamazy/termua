use gpui::{
    AnyElement, IntoElement, ParentElement, Styled, div, percentage, prelude::FluentBuilder, px,
};
use gpui_component::{IconName, h_flex, list::ListItem, tree::TreeEntry};

pub(super) fn nav_tree_row(ix: usize, entry: &TreeEntry, selected: bool) -> ListItem {
    let item = entry.item();
    let label = item.label.clone();
    let depth = entry.depth();
    let is_folder = entry.is_folder();
    let is_expanded = entry.is_expanded();

    let chevron_or_spacer: AnyElement = if is_folder {
        gpui_component::Icon::new(IconName::ChevronRight)
            .size_4()
            .when(is_expanded, |this| this.rotate(percentage(90. / 360.)))
            .into_any_element()
    } else {
        div().w(px(16.)).into_any_element()
    };

    ListItem::new(ix)
        .selected(selected)
        .text_sm()
        .py_0p5()
        .px_2()
        .pl(px(10.) + px(14.) * depth)
        .child(
            h_flex()
                .items_center()
                .gap_1()
                .child(chevron_or_spacer)
                .child(div().min_w_0().child(label)),
        )
}
