use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, MouseButton, ParentElement, Pixels,
    ReadGlobal, Styled, div, px,
};
use gpui_component::{
    ActiveTheme,
    scroll::{Scrollbar, ScrollbarShow},
};

use super::{ScrollbarPreviewLayoutState, TerminalView, format_scrollbar_preview_line_number};
use crate::{
    element::ScrollbarPreviewTextElement,
    settings::TerminalSettings,
    view::scrolling::{
        SCROLLBAR_ACTIVE_MARKER_SIZE, SCROLLBAR_MARKER_LIMIT, SCROLLBAR_MARKER_SIZE,
        SCROLLBAR_WIDTH, ScrollbarPreview, scroll_offset_for_line_coord_centered,
        scrollbar_marker_specs,
    },
};

impl TerminalView {
    pub(super) fn render_terminal_scrollbar_overlay(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !TerminalSettings::global(cx).show_scrollbar {
            return None;
        }

        Some(
            div()
                .id("terminal-scrollbar-overlay")
                .debug_selector(|| "terminal-scrollbar-overlay".to_string())
                .absolute()
                .top_0()
                .right_0()
                .bottom_0()
                .w(SCROLLBAR_WIDTH)
                .child(
                    Scrollbar::vertical(&self.terminal_scrollbar_handle)
                        .id("terminal-scrollbar")
                        .scrollbar_show(ScrollbarShow::Hover),
                )
                .into_any_element(),
        )
    }

    pub(super) fn render_scrollbar_marker_overlay(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let terminal_settings = TerminalSettings::global(cx);
        if !terminal_settings.show_scrollbar {
            return None;
        }
        if !(self.scrollbar_dragging()
            || self.scrollbar_hovered()
            || self.scrollbar_revealed()
            || self.is_search_open())
        {
            return None;
        }

        let terminal = self.terminal.read(cx);
        let matches = terminal.matches();
        let total_lines = terminal.total_lines();
        let viewport_lines = terminal.viewport_lines();
        if matches.is_empty() || total_lines == 0 || viewport_lines == 0 {
            return None;
        }

        let geometry = self.scrollbar_geometry(cx);
        let active_match_index = terminal.active_match_index();
        let marker_specs = scrollbar_marker_specs(
            geometry.track,
            total_lines,
            viewport_lines,
            matches,
            active_match_index,
            SCROLLBAR_MARKER_LIMIT,
        );
        if marker_specs.is_empty() {
            return None;
        }

        let marker_color = cx.theme().foreground.opacity(0.30);
        let active_marker_color = cx.theme().foreground.opacity(0.70);

        let mut overlay = div()
            .id("terminal-scrollbar-markers")
            .debug_selector(|| "terminal-scrollbar-markers".to_string())
            .absolute()
            .top_0()
            .right_0()
            .bottom_0()
            .w(SCROLLBAR_WIDTH);

        for spec in marker_specs {
            let marker_size = if spec.active {
                SCROLLBAR_ACTIVE_MARKER_SIZE
            } else {
                SCROLLBAR_MARKER_SIZE
            };
            let marker_top = (spec.y - geometry.bounds.origin.y - marker_size / 2.0).clamp(
                Pixels::ZERO,
                (geometry.bounds.size.height - marker_size).max(Pixels::ZERO),
            );
            let marker_right = (SCROLLBAR_WIDTH - marker_size) / 2.0;
            let marker_bg = if spec.active {
                active_marker_color
            } else {
                marker_color
            };
            let match_idx = spec.match_index;

            overlay = overlay.child(
                div()
                    .absolute()
                    .top(marker_top)
                    .right(marker_right)
                    .w(marker_size)
                    .h(marker_size)
                    .bg(marker_bg)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            let (total_lines, viewport_lines, line) = {
                                let terminal = this.terminal.read(cx);
                                let Some(search_match) = terminal.matches().get(match_idx) else {
                                    cx.stop_propagation();
                                    return;
                                };
                                (
                                    terminal.total_lines(),
                                    terminal.viewport_lines(),
                                    search_match.start().line,
                                )
                            };
                            let target_offset = scroll_offset_for_line_coord_centered(
                                total_lines,
                                viewport_lines,
                                line,
                            );
                            this.terminal.update(cx, |term, _| {
                                term.activate_match(match_idx);
                            });
                            this.apply_scrollbar_target_offset(target_offset, cx);
                            cx.stop_propagation();
                        }),
                    ),
            );
        }

        Some(overlay.into_any_element())
    }

    pub(super) fn render_scrollbar_preview_overlay(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let preview = self.scrollbar_preview().cloned()?;
        let terminal_settings = TerminalSettings::global(cx);
        if !terminal_settings.show_scrollbar {
            return None;
        }

        let ScrollbarPreview {
            anchor,
            start_line_from_top,
            cols,
            rows,
            cells,
            match_range,
            ..
        } = preview;

        let theme = cx.theme();
        let ScrollbarPreviewLayoutState {
            view_bounds,
            line_height,
            cell_width,
            total_lines,
        } = self.scrollbar_preview_layout_state(cx);

        let content_h = line_height * (rows.max(1) as f32) + px(16.0);
        let anchor_y = anchor.y - view_bounds.origin.y;
        let y = scrollbar_preview_overlay_top(anchor_y, view_bounds.size.height, content_h);

        let panel_bg = theme.popover;
        let panel_border = theme.border.opacity(0.9);
        let line_no_fg = theme.foreground.opacity(0.40);
        let line_no_digits = total_lines.max(1).to_string().len();

        let mut body = div()
            .id("terminal-scrollbar-preview")
            .debug_selector(|| "terminal-scrollbar-preview".to_string())
            .absolute()
            .left_0()
            .top(y)
            .right(SCROLLBAR_WIDTH)
            .bg(panel_bg)
            .border_1()
            .border_color(panel_border)
            .rounded_md()
            .shadow_lg()
            .py(px(8.0))
            .px(px(10.0))
            .font_family(terminal_settings.font_family.clone())
            .text_size(terminal_settings.font_size)
            .font_weight(terminal_settings.font_weight)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| {
                    cx.stop_propagation();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|_, _, _, cx| {
                    cx.stop_propagation();
                }),
            )
            .flex_col();

        let line_numbers = render_scrollbar_preview_line_numbers(
            rows,
            start_line_from_top,
            line_height,
            line_no_fg,
            line_no_digits,
        );

        let text = div()
            .h(line_height * (rows.max(1) as f32))
            .flex_1()
            .overflow_hidden()
            .child(ScrollbarPreviewTextElement::new(
                cells,
                cell_width,
                line_height,
                cols,
                Some(match_range),
            ));

        body = body.child(
            div()
                .flex()
                .items_start()
                .overflow_hidden()
                .child(line_numbers)
                .child(text),
        );

        Some(body.into_any_element())
    }
}

fn scrollbar_preview_overlay_top(
    anchor_y: Pixels,
    view_height: Pixels,
    content_h: Pixels,
) -> Pixels {
    let mut y = anchor_y - content_h / 2.0;
    let top_pad = px(12.0);
    let bottom_pad = px(56.0);
    y = y.clamp(top_pad, (view_height - content_h - bottom_pad).max(top_pad));
    y
}

fn render_scrollbar_preview_line_numbers(
    rows: usize,
    start_line_from_top: usize,
    line_height: Pixels,
    line_no_fg: gpui::Hsla,
    line_no_digits: usize,
) -> gpui::Div {
    let mut line_numbers = div().flex_col().flex_shrink_0();
    for i in 0..rows {
        let line_no = start_line_from_top.saturating_add(i).saturating_add(1);
        line_numbers = line_numbers.child(
            div()
                .h(line_height)
                .whitespace_nowrap()
                .overflow_hidden()
                .text_color(line_no_fg)
                .child(format_scrollbar_preview_line_number(
                    line_no,
                    line_no_digits,
                )),
        );
    }
    line_numbers
}
