use std::panic::Location;

use gpui::{
    AnyElement, App, Bounds, Context, Element, ElementId, GlobalElementId, Hsla,
    InteractiveElement, IntoElement, LayoutId, MouseButton, MouseDownEvent, ParentElement, Pixels,
    ReadGlobal, Style, Styled, Window, div, fill, point, px, relative, size,
};
use gpui_component::{
    ActiveTheme,
    scroll::{Scrollbar, ScrollbarShow},
};

use super::TerminalView;
use crate::{
    element::ScrollbarPreviewTextElement,
    settings::TerminalSettings,
    view::scrolling::{
        SCROLLBAR_ACTIVE_MARKER_SIZE, SCROLLBAR_MARKER_LIMIT, SCROLLBAR_MARKER_SIZE,
        SCROLLBAR_WIDTH, ScrollbarMarkerSpec, ScrollbarPreview,
        scroll_offset_for_line_coord_centered, scrollbar_marker_specs,
    },
};

#[derive(Clone, Copy)]
struct ScrollbarPreviewLayoutState {
    view_bounds: Bounds<Pixels>,
    line_height: Pixels,
    cell_width: Pixels,
    total_lines: usize,
}

fn format_scrollbar_preview_line_number(one_based: usize, digits: usize) -> String {
    let digits = digits.max(1);
    format!("{:>width$}\u{00A0}", one_based, width = digits)
}

#[derive(Clone)]
struct ScrollbarMarkersElement {
    marker_specs: Vec<ScrollbarMarkerSpec>,
    lane_bounds: Bounds<Pixels>,
    marker_color: Hsla,
    active_marker_color: Hsla,
}

impl ScrollbarMarkersElement {
    fn new(
        marker_specs: Vec<ScrollbarMarkerSpec>,
        lane_bounds: Bounds<Pixels>,
        marker_color: Hsla,
        active_marker_color: Hsla,
    ) -> Self {
        Self {
            marker_specs,
            lane_bounds,
            marker_color,
            active_marker_color,
        }
    }
}

impl Element for ScrollbarMarkersElement {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, None, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        _: &mut Window,
        _: &mut App,
    ) -> Self::PrepaintState {
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        _: &mut Self::PrepaintState,
        window: &mut Window,
        _: &mut App,
    ) {
        for spec in &self.marker_specs {
            let local_bounds = scrollbar_marker_local_bounds(*spec, self.lane_bounds);
            let marker_bounds = Bounds {
                origin: bounds.origin + local_bounds.origin,
                size: local_bounds.size,
            };
            let color = if spec.active {
                self.active_marker_color
            } else {
                self.marker_color
            };
            window.paint_quad(fill(marker_bounds, color));
        }
    }
}

impl IntoElement for ScrollbarMarkersElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

fn scrollbar_marker_size(spec: ScrollbarMarkerSpec) -> Pixels {
    if spec.active {
        SCROLLBAR_ACTIVE_MARKER_SIZE
    } else {
        SCROLLBAR_MARKER_SIZE
    }
}

fn scrollbar_marker_local_bounds(
    spec: ScrollbarMarkerSpec,
    lane_bounds: Bounds<Pixels>,
) -> Bounds<Pixels> {
    let marker_size = scrollbar_marker_size(spec);
    let top = (spec.y - lane_bounds.origin.y - marker_size / 2.0).clamp(
        Pixels::ZERO,
        (lane_bounds.size.height - marker_size).max(Pixels::ZERO),
    );
    let left = (lane_bounds.size.width - marker_size) / 2.0;
    Bounds {
        origin: point(left, top),
        size: size(marker_size, marker_size),
    }
}

fn scrollbar_marker_window_bounds(
    spec: ScrollbarMarkerSpec,
    lane_bounds: Bounds<Pixels>,
) -> Bounds<Pixels> {
    let local_bounds = scrollbar_marker_local_bounds(spec, lane_bounds);
    Bounds {
        origin: lane_bounds.origin + local_bounds.origin,
        size: local_bounds.size,
    }
}

fn scrollbar_marker_match_index_at_position(
    marker_specs: &[ScrollbarMarkerSpec],
    lane_bounds: Bounds<Pixels>,
    position: gpui::Point<Pixels>,
) -> Option<usize> {
    marker_specs.iter().find_map(|spec| {
        scrollbar_marker_window_bounds(*spec, lane_bounds)
            .contains(&position)
            .then_some(spec.match_index)
    })
}

impl TerminalView {
    fn scrollbar_preview_layout_state(&self, cx: &App) -> ScrollbarPreviewLayoutState {
        let terminal = self.terminal.read(cx);
        let terminal_bounds = &terminal.last_content().terminal_bounds;
        ScrollbarPreviewLayoutState {
            view_bounds: terminal_bounds.bounds,
            line_height: terminal_bounds.line_height,
            cell_width: terminal_bounds.cell_width,
            total_lines: terminal.total_lines(),
        }
    }

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
        let marker_specs_for_click = marker_specs.clone();
        let lane_bounds = geometry.bounds;

        let overlay = div()
            .id("terminal-scrollbar-markers")
            .debug_selector(|| "terminal-scrollbar-markers".to_string())
            .absolute()
            .top_0()
            .right_0()
            .bottom_0()
            .w(SCROLLBAR_WIDTH)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                    let Some(match_idx) = scrollbar_marker_match_index_at_position(
                        &marker_specs_for_click,
                        lane_bounds,
                        e.position,
                    ) else {
                        return;
                    };

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
                    let target_offset =
                        scroll_offset_for_line_coord_centered(total_lines, viewport_lines, line);
                    this.terminal.update(cx, |term, _| {
                        term.activate_match(match_idx);
                    });
                    this.apply_scrollbar_target_offset(target_offset, cx);
                    cx.stop_propagation();
                }),
            )
            .child(ScrollbarMarkersElement::new(
                marker_specs,
                lane_bounds,
                marker_color,
                active_marker_color,
            ));

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

#[cfg(test)]
pub(super) mod scrollbar_preview_tests {
    use std::{borrow::Cow, ops::RangeInclusive, rc::Rc};

    use gpui::{
        AppContext, Bounds, Context as GpuiContext, Entity, InteractiveElement, Keystroke,
        Modifiers, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels,
        ScrollWheelEvent, Styled, Window, div, point, px, size,
    };
    use gpui_component::Root;

    use super::{format_scrollbar_preview_line_number, scrollbar_marker_match_index_at_position};
    use crate::{
        Cell, GridPoint, IndexedCell, TerminalBackend, TerminalContent, TerminalShutdownPolicy,
        TerminalType, settings::CursorShape, terminal::TerminalBounds, view::TerminalView,
    };

    #[test]
    fn format_scrollbar_preview_line_number_right_aligns() {
        assert_eq!(format_scrollbar_preview_line_number(3, 1), "3\u{00A0}");
        assert_eq!(format_scrollbar_preview_line_number(3, 4), "   3\u{00A0}");
    }

    #[test]
    fn format_scrollbar_preview_line_number_uses_trailing_space() {
        // The preview renderer now uses real terminal cells (with fixed-width positioning), so
        // we no longer need to preserve spaces via NBSP substitution.
        assert_eq!(format_scrollbar_preview_line_number(1, 1), "1\u{00A0}");
    }

    pub(crate) struct PreviewBackend {
        content: TerminalContent,
        matches: Vec<RangeInclusive<GridPoint>>,
        total_lines: usize,
        viewport_lines: usize,
        preview_cols: usize,
        preview_rows: usize,
        preview_cells: Vec<IndexedCell>,
    }

    impl PreviewBackend {
        pub(crate) fn new() -> Self {
            // Give the renderer something to work with; actual bounds will be set via `set_size`.
            let content = TerminalContent::default();

            // Make a single match close to the bottom of the buffer.
            // With total_lines=100 and viewport_lines=20, line_coord=19 maps to buffer index 99.
            let matches = vec![RangeInclusive::new(
                GridPoint::new(19, 0),
                GridPoint::new(19, 1),
            )];

            let preview_cols = 24;
            let preview_rows = 7;
            let mut preview_cells = Vec::new();
            for row in 0..preview_rows {
                for col in 0..preview_cols {
                    preview_cells.push(IndexedCell {
                        point: GridPoint::new(row as i32, col),
                        cell: Cell {
                            c: if col == 0 { '>' } else { 'x' },
                            ..Default::default()
                        },
                    });
                }
            }

            Self {
                content,
                matches,
                total_lines: 100,
                viewport_lines: 20,
                preview_cols,
                preview_rows,
                preview_cells,
            }
        }
    }

    impl TerminalBackend for PreviewBackend {
        fn backend_name(&self) -> &'static str {
            "preview-test"
        }

        fn sync(&mut self, _window: &mut Window, _cx: &mut GpuiContext<crate::Terminal>) {}

        fn shutdown(
            &mut self,
            _policy: TerminalShutdownPolicy,
            _cx: &mut GpuiContext<crate::Terminal>,
        ) {
        }

        fn last_content(&self) -> &TerminalContent {
            &self.content
        }

        fn matches(&self) -> &[RangeInclusive<GridPoint>] {
            &self.matches
        }

        fn last_clicked_line(&self) -> Option<i32> {
            None
        }

        fn vi_mode_enabled(&self) -> bool {
            false
        }

        fn mouse_mode(&self, _shift: bool) -> bool {
            false
        }

        fn selection_started(&self) -> bool {
            false
        }

        fn set_cursor_shape(&mut self, _cursor_shape: CursorShape) {}

        fn total_lines(&self) -> usize {
            self.total_lines
        }

        fn viewport_lines(&self) -> usize {
            self.viewport_lines
        }

        fn activate_match(&mut self, _index: usize) {}

        fn select_matches(&mut self, _matches: &[RangeInclusive<GridPoint>]) {}

        fn select_all(&mut self) {}

        fn copy(&mut self, _keep_selection: Option<bool>, _cx: &mut GpuiContext<crate::Terminal>) {}

        fn clear(&mut self) {}

        fn scroll_line_up(&mut self) {}
        fn scroll_up_by(&mut self, _lines: usize) {}
        fn scroll_line_down(&mut self) {}
        fn scroll_down_by(&mut self, _lines: usize) {}
        fn scroll_page_up(&mut self) {}
        fn scroll_page_down(&mut self) {}
        fn scroll_to_top(&mut self) {}
        fn scroll_to_bottom(&mut self) {}

        fn scrolled_to_top(&self) -> bool {
            true
        }

        fn scrolled_to_bottom(&self) -> bool {
            true
        }

        fn set_size(&mut self, new_bounds: TerminalBounds) {
            self.content.terminal_bounds = new_bounds;
        }

        fn input(&mut self, _input: Cow<'static, [u8]>) {}

        fn paste(&mut self, _text: &str) {}

        fn focus_in(&self) {}

        fn focus_out(&mut self) {}

        fn toggle_vi_mode(&mut self) {}

        fn try_keystroke(&mut self, _keystroke: &Keystroke, _alt_is_meta: bool) -> bool {
            false
        }

        fn try_modifiers_change(
            &mut self,
            _modifiers: &Modifiers,
            _window: &Window,
            _cx: &mut GpuiContext<crate::Terminal>,
        ) {
        }

        fn mouse_move(&mut self, _e: &MouseMoveEvent, _cx: &mut GpuiContext<crate::Terminal>) {}

        fn select_word_at_event_position(&mut self, _e: &MouseDownEvent) {}

        fn mouse_drag(
            &mut self,
            _e: &MouseMoveEvent,
            _region: Bounds<Pixels>,
            _cx: &mut GpuiContext<crate::Terminal>,
        ) {
        }

        fn mouse_down(&mut self, _e: &MouseDownEvent, _cx: &mut GpuiContext<crate::Terminal>) {}

        fn mouse_up(&mut self, _e: &MouseUpEvent, _cx: &GpuiContext<crate::Terminal>) {}

        fn scroll_wheel(&mut self, _e: &ScrollWheelEvent) {}

        fn get_content(&self) -> String {
            String::new()
        }

        fn last_n_non_empty_lines(&self, _n: usize) -> Vec<String> {
            Vec::new()
        }

        fn preview_cells_from_top(
            &self,
            _start_line: usize,
            _count: usize,
        ) -> (usize, usize, Vec<IndexedCell>) {
            (
                self.preview_cols,
                self.preview_rows,
                self.preview_cells.clone(),
            )
        }
    }

    #[gpui::test]
    fn scrollbar_preview_is_not_obscured_by_footer_bar(cx: &mut gpui::TestAppContext) {
        cx.update(|app| {
            crate::init(app);
        });

        let view_slot: Rc<std::cell::RefCell<Option<Entity<TerminalView>>>> =
            Rc::new(std::cell::RefCell::new(None));
        let view_slot_for_window = view_slot.clone();

        let (root, v) = cx.add_window_view(|window, cx| {
            let terminal = cx.new(|_| {
                crate::Terminal::new(TerminalType::WezTerm, Box::new(PreviewBackend::new()))
            });
            let terminal_view = cx.new(|cx| TerminalView::new(terminal, window, cx));
            *view_slot_for_window.borrow_mut() = Some(terminal_view.clone());

            terminal_view.update(cx, |this, cx| {
                // Anchor the preview near the bottom of the window so the default clamp behavior
                // would overlap a bottom footer bar overlay.
                this.set_scrollbar_preview_for_match(0, point(px(0.0), px(690.0)), cx);
            });

            Root::new(terminal_view, window, cx)
        });

        v.draw(
            point(px(0.0), px(0.0)),
            size(
                gpui::AvailableSpace::Definite(px(900.0)),
                gpui::AvailableSpace::Definite(px(700.0)),
            ),
            move |_, _| {
                div().size_full().relative().child(root).child(
                    // Simulate a bottom "footer bar" overlay that can obscure the preview
                    // tooltip when a search marker is near the bottom of the scrollbar.
                    div()
                        .debug_selector(|| "test-footerbar".to_string())
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .h(px(48.0)),
                )
            },
        );

        v.run_until_parked();

        let view = view_slot
            .borrow()
            .clone()
            .expect("expected terminal view to be captured");
        let preview_set = v.read_entity(&view, |this, _app| this.scrollbar_preview().is_some());
        assert!(preview_set, "expected scrollbar preview state to be set");

        let preview_bounds = v
            .debug_bounds("terminal-scrollbar-preview")
            .expect("scrollbar preview should exist");
        let footer_bounds = v
            .debug_bounds("test-footerbar")
            .expect("test footer bar should exist");

        let preview_bottom = preview_bounds.origin.y + preview_bounds.size.height;
        assert!(
            preview_bottom <= footer_bounds.origin.y,
            "expected preview bottom ({preview_bottom:?}) to be above footer bar top ({:?})",
            footer_bounds.origin.y
        );
    }

    #[test]
    fn scrollbar_marker_match_index_at_position_hits_marker_rect_only() {
        use crate::view::scrolling::ScrollbarMarkerSpec;

        let lane = Bounds {
            origin: point(px(100.0), px(20.0)),
            size: size(px(14.0), px(100.0)),
        };
        let specs = vec![
            ScrollbarMarkerSpec {
                match_index: 3,
                y: px(40.0),
                active: false,
            },
            ScrollbarMarkerSpec {
                match_index: 7,
                y: px(80.0),
                active: true,
            },
        ];

        assert_eq!(
            scrollbar_marker_match_index_at_position(&specs, lane, point(px(107.0), px(80.0))),
            Some(7)
        );
        assert_eq!(
            scrollbar_marker_match_index_at_position(&specs, lane, point(px(107.0), px(60.0))),
            None
        );
    }
}
