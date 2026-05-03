use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    ops::RangeInclusive,
    panic::Location,
    rc::Rc,
    time::{Duration, Instant},
};

use gpui::{
    AbsoluteLength, AnyElement, App, AvailableSpace, BorderStyle, Bounds, ContentMask, Context,
    DispatchPhase, Element, ElementId, Entity, FocusHandle, FontStyle, GlobalElementId,
    HighlightStyle, Hitbox, Hsla, InputHandler, InteractiveElement, Interactivity, IntoElement,
    LayoutId, ModifiersChangedEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ParentElement, Pixels, Point, ReadGlobal, ShapedLine, SharedString, StatefulInteractiveElement,
    StrikethroughStyle, Style, TextAlign, TextRun, TextStyle, UTF16Selection, UnderlineStyle,
    WhiteSpace, Window, div, fill, outline, point, px, relative, size,
};
use gpui_component::{ActiveTheme, Theme};

use crate::{
    CellFlags, CursorRenderShape, GridPoint, HoveredWord, IndexedCell, NamedColor, TermColor,
    TerminalMode, convert_color,
    settings::{CursorShape, TerminalSettings},
    snippet::parse_snippet_suffix,
    terminal::{Terminal, TerminalBounds},
    util::ensure_minimum_contrast,
    view::{
        BlockContext, BlockProperties, TerminalView,
        line_number::{
            LineNumberPaintData, LineNumberState, compute_line_number_layout,
            compute_line_number_paint_data, paint_line_numbers,
            reserve_left_padding_without_line_numbers, should_relayout_for_mode_change,
            should_show_line_numbers,
        },
        scrolling::{
            SCROLLBAR_WIDTH, ScrollbarLayoutState, overlay_scrollbar_layout_state,
            paint_overlay_scrollbar, scroll_offset_for_drag_delta,
            scroll_offset_for_line_coord_centered, scroll_offset_for_thumb_center_y,
            scrollbar_bounds_for_terminal, scrollbar_marker_y_for_line_coord,
            scrollbar_track_bounds, search_match_index_for_scrollbar_click,
            search_match_index_for_scrollbar_hover, thumb_bounds_for_track,
        },
    },
};

fn dominant_effective_background_color(cells: &[IndexedCell]) -> TermColor {
    // The terminal grid is discrete (rows/cols). When the window width is not an exact multiple of
    // `cell_width`, there will be leftover pixels on the right that Vim (and other TUIs) cannot
    // draw into. To avoid a visible "stripe" there, fill the full bounds with the dominant cell
    // background color for the current frame.
    //
    // We sample (bounded) to keep this cheap for large grids.
    if cells.is_empty() {
        return TermColor::Named(NamedColor::Background);
    }

    let sample_limit = 512usize;
    let step = (cells.len() / sample_limit).max(1);

    let mut counts: HashMap<TermColor, usize> = HashMap::new();
    for cell in cells.iter().step_by(step).take(sample_limit) {
        let mut fg = cell.fg;
        let mut bg = cell.bg;
        if cell.flags.contains(CellFlags::INVERSE) {
            std::mem::swap(&mut fg, &mut bg);
        }
        *counts.entry(bg).or_insert(0) += 1;
    }

    counts
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .map(|(bg, _)| bg)
        .unwrap_or(TermColor::Named(NamedColor::Background))
}

fn compute_terminal_layout_metrics(
    bounds: Bounds<Pixels>,
    cell_width: Pixels,
    line_height: Pixels,
    show_scrollbar: bool,
    show_line_numbers: bool,
    reserve_left_padding_without_line_numbers: bool,
    total_lines_for_digits: usize,
) -> (TerminalBounds, Pixels, Pixels, usize, Pixels) {
    let line_numbers = compute_line_number_layout(
        cell_width,
        show_line_numbers,
        reserve_left_padding_without_line_numbers,
        total_lines_for_digits,
    );
    let gutter = line_numbers.gutter;
    let line_number_width = line_numbers.line_number_width;
    let line_number_digits = line_numbers.line_number_digits;

    let scrollbar_width = if show_scrollbar {
        SCROLLBAR_WIDTH
    } else {
        Pixels::ZERO
    };

    let mut size = bounds.size;
    // The scrollbar is an overlay; do not reserve horizontal space for it.
    size.width -= gutter;

    // Workaround: if the terminal is effectively one column wide, some wide
    // characters can trigger incorrect wrap/damage behavior in the backend.
    if size.width < cell_width * 2.0 {
        size.width = cell_width * 2.0;
    }

    let mut origin = bounds.origin;
    origin.x += gutter;

    (
        TerminalBounds::new(line_height, cell_width, Bounds { origin, size }),
        gutter,
        line_number_width,
        line_number_digits,
        scrollbar_width,
    )
}

/// The information generated during layout that is necessary for painting.
pub struct LayoutState {
    hitbox: Hitbox,
    bg_quads: Vec<BgQuad>,
    text_spans: Vec<TextSpan>,
    relative_highlighted_ranges: Vec<(RangeInclusive<GridPoint>, Hsla)>,
    cursor: Option<CursorLayout>,
    background_color: Hsla,
    dimensions: TerminalBounds,
    mode: TerminalMode,
    display_offset: usize,
    line_number_state: LineNumberState,
    line_number_paint_data: Option<LineNumberPaintData>,
    block_below_cursor_element: Option<AnyElement>,
    base_text_style: TextStyle,
    scrollbar: ScrollbarLayoutState,
    scrollbar_visible: bool,
    scrollbar_markers: Vec<Pixels>,
    scrollbar_active_marker: Option<Pixels>,
}

/// Helper struct for converting backend cursor points to displayed cursor points.
struct DisplayCursor {
    line: i32,
    col: usize,
}

impl DisplayCursor {
    fn from(cursor_point: GridPoint, display_offset: usize) -> Self {
        Self {
            line: cursor_point.line + display_offset as i32,
            col: cursor_point.column,
        }
    }

    pub fn line(&self) -> i32 {
        self.line
    }

    pub fn col(&self) -> usize {
        self.col
    }
}

pub struct CursorLayout {
    origin: Point<Pixels>,
    block_width: Pixels,
    line_height: Pixels,
    color: Hsla,
    shape: CursorShape,
    block_text: Option<ShapedLine>,
    cursor_name: Option<AnyElement>,
}

impl CursorLayout {
    pub fn new(
        origin: Point<Pixels>,
        block_width: Pixels,
        line_height: Pixels,
        color: Hsla,
        shape: CursorShape,
        block_text: Option<ShapedLine>,
    ) -> CursorLayout {
        CursorLayout {
            origin,
            block_width,
            line_height,
            color,
            shape,
            block_text,
            cursor_name: None,
        }
    }

    pub fn bounding_rect(&self, origin: Point<Pixels>) -> Bounds<Pixels> {
        Bounds {
            origin: self.origin + origin,
            size: size(self.block_width, self.line_height),
        }
    }

    fn bounds(&self, origin: Point<Pixels>) -> Bounds<Pixels> {
        match self.shape {
            CursorShape::Bar => Bounds {
                origin: self.origin + origin,
                size: size(px(2.0), self.line_height),
            },
            CursorShape::Block | CursorShape::Hollow => Bounds {
                origin: self.origin + origin,
                size: size(self.block_width, self.line_height),
            },
            CursorShape::Underline => Bounds {
                origin: self.origin + origin + Point::new(Pixels::ZERO, self.line_height - px(2.0)),
                size: size(self.block_width, px(2.0)),
            },
        }
    }

    pub fn paint(&mut self, origin: Point<Pixels>, window: &mut Window, cx: &mut App) {
        let bounds = self.bounds(origin);

        // Draw background or border quad
        let cursor = if matches!(self.shape, CursorShape::Hollow) {
            outline(bounds, self.color, BorderStyle::Solid)
        } else {
            fill(bounds, self.color)
        };

        if let Some(name) = &mut self.cursor_name {
            name.paint(window, cx);
        }

        window.paint_quad(cursor);

        if let Some(block_text) = &self.block_text {
            let _ = block_text.paint(
                self.origin + origin,
                self.line_height,
                TextAlign::Left,
                None,
                window,
                cx,
            );
        }
    }
}

/// The GPUI element that paints the terminal.
/// We need to keep a reference to the model for mouse events, do we need it for any other terminal
/// stuff, or can we move that to connection?
pub struct TerminalElement {
    terminal: Entity<Terminal>,
    terminal_view: Entity<TerminalView>,
    focus: FocusHandle,
    focused: bool,
    cursor_visible: bool,
    interactivity: Interactivity,
    block_below_cursor: Option<Rc<BlockProperties>>,
}

/// Paints a small slice of terminal cells using the same renderer as the main terminal element.
///
/// This is used by the search scrollbar/minimap hover preview, so the preview text can preserve
/// per-cell colors/styles instead of falling back to a single foreground color.
pub(crate) struct ScrollbarPreviewTextElement {
    cells: Vec<IndexedCell>,
    cell_width: Pixels,
    line_height: Pixels,
    cols: usize,
    highlight_range: Option<RangeInclusive<GridPoint>>,
}

impl ScrollbarPreviewTextElement {
    pub(crate) fn new(
        cells: Vec<IndexedCell>,
        cell_width: Pixels,
        line_height: Pixels,
        cols: usize,
        highlight_range: Option<RangeInclusive<GridPoint>>,
    ) -> Self {
        Self {
            cells,
            cell_width,
            line_height,
            cols,
            highlight_range,
        }
    }
}

pub(crate) struct ScrollbarPreviewTextLayoutState {
    bg_quads: Vec<BgQuad>,
    highlight_quads: Vec<BgQuad>,
    spans: Vec<TextSpan>,
}

fn highlight_quads_for_range(
    range: &RangeInclusive<GridPoint>,
    cols: usize,
    color: Hsla,
) -> Vec<BgQuad> {
    if cols == 0 {
        return Vec::new();
    }

    let start = range.start();
    let end = range.end();
    if end.line < start.line {
        return Vec::new();
    }

    let start_line = start.line.max(0) as usize;
    let end_line = end.line.max(0) as usize;
    let max_col = cols.saturating_sub(1);

    let mut out = Vec::new();
    for line in start_line..=end_line {
        let start_col = if line == start_line { start.column } else { 0 };
        let mut end_col = if line == end_line {
            end.column
        } else {
            max_col
        };

        if start_col > max_col {
            continue;
        }
        if end_col > max_col {
            end_col = max_col;
        }
        if end_col < start_col {
            continue;
        }

        out.push(BgQuad {
            point: GridPoint::new(line as i32, start_col),
            cells: end_col - start_col + 1,
            color,
        });
    }

    out
}

fn placeholder_highlight_bgs(theme: &Theme) -> (Hsla, Hsla) {
    // Deeper, theme-matching placeholder backgrounds (active, inactive).
    // Used both for suggestions dropdown placeholder segments and in-terminal snippet placeholders.
    (theme.selection, theme.selection.opacity(0.5))
}

fn snippet_placeholder_bg_quads(
    session: &crate::snippet::SnippetSession,
    cols: usize,
    active_bg: Hsla,
    inactive_bg: Hsla,
) -> Vec<BgQuad> {
    use unicode_width::UnicodeWidthChar as _;

    if cols == 0 {
        return Vec::new();
    }

    let mut cell_offsets = Vec::<usize>::with_capacity(session.rendered.chars().count() + 1);
    cell_offsets.push(0);
    let mut off = 0usize;
    for ch in session.rendered.chars() {
        let w = ch.width().unwrap_or(0);
        off = off.saturating_add(w);
        cell_offsets.push(off);
    }

    let active_is_placeholder = session
        .tabstops
        .get(session.active)
        .is_some_and(|t| t.index != 0);

    let mut out = Vec::<BgQuad>::new();
    for (idx, stop) in session.tabstops.iter().enumerate() {
        if stop.index == 0 {
            continue;
        }

        let bg = if active_is_placeholder && idx == session.active {
            active_bg
        } else {
            inactive_bg
        };

        let start_chars = stop
            .range_chars
            .start
            .min(cell_offsets.len().saturating_sub(1));
        let end_chars = stop
            .range_chars
            .end
            .min(cell_offsets.len().saturating_sub(1));

        let start_cell = cell_offsets[start_chars];
        let mut end_cell = cell_offsets[end_chars];
        if end_cell <= start_cell {
            end_cell = start_cell.saturating_add(1);
        }

        let start_abs = session.start_point.column.saturating_add(start_cell);
        let end_abs_incl = session
            .start_point
            .column
            .saturating_add(end_cell.saturating_sub(1));

        let start = GridPoint::new(
            session
                .start_point
                .line
                .saturating_add((start_abs / cols) as i32),
            start_abs % cols,
        );
        let end = GridPoint::new(
            session
                .start_point
                .line
                .saturating_add((end_abs_incl / cols) as i32),
            end_abs_incl % cols,
        );

        out.extend(highlight_quads_for_range(&(start..=end), cols, bg));
    }

    out
}

impl Element for ScrollbarPreviewTextElement {
    type RequestLayoutState = ();
    type PrepaintState = ScrollbarPreviewTextLayoutState;

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
        let layout_id = window.request_layout(style, None, cx);
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        if self.cells.is_empty() {
            return ScrollbarPreviewTextLayoutState {
                bg_quads: Vec::new(),
                highlight_quads: Vec::new(),
                spans: Vec::new(),
            };
        }

        let terminal_settings = TerminalSettings::global(cx);
        let minimum_contrast = terminal_settings.minimum_contrast;
        let line_height = terminal_settings.line_height.value();
        let font_weight = terminal_settings.font_weight;

        let text_style = TextStyle {
            font_family: terminal_settings.font_family.clone(),
            font_features: terminal_settings.font_features.clone(),
            font_weight,
            font_fallbacks: terminal_settings.font_fallbacks.clone(),
            font_size: terminal_settings.font_size.into(),
            font_style: FontStyle::Normal,
            line_height: line_height.into(),
            background_color: Some(cx.theme().background),
            white_space: WhiteSpace::Normal,
            // Will be overridden per-cell.
            color: cx.theme().foreground,
            ..Default::default()
        };

        // Cells in `ScrollbarPreviewTextElement` are re-based such that the first row starts at
        // line 0, so we can use `start_line_offset = 0`.
        let (bg_quads, spans) = build_plan(
            &self.cells,
            0,
            cx.theme(),
            &text_style,
            None,
            minimum_contrast,
        );

        let highlight_quads = self
            .highlight_range
            .as_ref()
            .map(|range| {
                let color = cx.theme().selection.opacity(0.45);
                highlight_quads_for_range(range, self.cols, color)
            })
            .unwrap_or_default();

        // Ensure the text system warms the font cache before paint to minimize jitter on hover.
        // (The main terminal element already does this work for its own glyphs.)
        let _ = window.text_system().resolve_font(&text_style.font());

        ScrollbarPreviewTextLayoutState {
            bg_quads,
            highlight_quads,
            spans,
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        state: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let origin = bounds.origin;
        for quad in &state.bg_quads {
            quad.paint(origin, self.cell_width, self.line_height, window);
        }
        for quad in &state.highlight_quads {
            quad.paint(origin, self.cell_width, self.line_height, window);
        }
        for span in &state.spans {
            span.paint(origin, self.cell_width, self.line_height, window, cx);
        }
    }
}

impl IntoElement for ScrollbarPreviewTextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl InteractiveElement for TerminalElement {
    fn interactivity(&mut self) -> &mut Interactivity {
        &mut self.interactivity
    }
}

impl StatefulInteractiveElement for TerminalElement {}

impl TerminalElement {
    pub fn new(
        terminal: Entity<Terminal>,
        terminal_view: Entity<TerminalView>,
        focus: FocusHandle,
        focused: bool,
        cursor_visible: bool,
        block_below_cursor: Option<Rc<BlockProperties>>,
    ) -> TerminalElement {
        TerminalElement {
            terminal,
            terminal_view,
            focused,
            focus: focus.clone(),
            cursor_visible,
            block_below_cursor,
            interactivity: Default::default(),
        }
        .track_focus(&focus)
    }

    /// Computes the cursor position and expected block width, may return a zero width if
    /// x_for_index returns the same position for sequential indexes. Use em_width instead
    fn shape_cursor(
        cursor_point: DisplayCursor,
        size: TerminalBounds,
        text_fragment: &ShapedLine,
    ) -> Option<(Point<Pixels>, Pixels)> {
        if cursor_point.line() < size.num_lines() as i32 {
            let cursor_width = if text_fragment.width == Pixels::ZERO {
                size.cell_width()
            } else {
                text_fragment.width
            };

            // Cursor should always surround as much of the text as possible,
            // hence when on pixel boundaries round the origin down and the width up
            Some((
                point(
                    (cursor_point.col() as f32 * size.cell_width()).floor(),
                    (cursor_point.line() as f32 * size.line_height()).floor(),
                ),
                cursor_width.ceil(),
            ))
        } else {
            None
        }
    }

    fn generic_button_handler<E>(
        connection: Entity<Terminal>,
        focus_handle: FocusHandle,
        steal_focus: bool,
        f: impl Fn(&mut Terminal, &E, &mut Context<Terminal>),
    ) -> impl Fn(&E, &mut Window, &mut App) {
        move |event, window, cx| {
            if steal_focus {
                window.focus(&focus_handle, cx);
            } else if !focus_handle.is_focused(window) {
                return;
            }
            connection.update(cx, |terminal, cx| {
                f(terminal, event, cx);

                cx.notify();
            })
        }
    }

    fn register_mouse_listeners(
        &mut self,
        mode: TerminalMode,
        hitbox: &Hitbox,
        window: &mut Window,
    ) {
        self.register_mouse_left_down_listener();
        self.register_window_mouse_move_listener(hitbox.clone(), window);
        self.register_mouse_left_up_listener();
        self.register_window_mouse_left_up_listener(window);
        self.register_mouse_middle_down_listener();
        self.register_scroll_wheel_listener();
        self.register_mouse_mode_listeners(mode);
    }

    fn handle_suggestions_overlay_left_down(
        terminal: &Entity<Terminal>,
        terminal_view: &Entity<TerminalView>,
        e: &MouseDownEvent,
        cx: &mut App,
    ) -> bool {
        if let Some(hit) = suggestions_overlay_layout(terminal, terminal_view, cx)
            .map(|layout| layout.hit_test(e.position))
        {
            match hit {
                SuggestionsOverlayHitTest::Row(row) => {
                    terminal_view.update(cx, |view: &mut TerminalView, view_cx| {
                        let (content, cursor_line_id) = {
                            let terminal = view.terminal.read(view_cx);
                            (terminal.last_content().clone(), terminal.cursor_line_id())
                        };
                        let _ =
                            view.accept_suggestion_at_index(row, &content, cursor_line_id, view_cx);
                        view_cx.notify();
                    });
                    cx.stop_propagation();
                    return true;
                }
                SuggestionsOverlayHitTest::Panel => {
                    // Overlay UI should behave like UI chrome; don't let clicks start a terminal
                    // selection behind it.
                    cx.stop_propagation();
                    return true;
                }
                SuggestionsOverlayHitTest::Outside => {
                    terminal_view.update(cx, |view: &mut TerminalView, view_cx| {
                        view.close_suggestions(view_cx);
                    });
                }
            }
        }

        false
    }

    fn handle_scrollbar_left_down(
        terminal: &Entity<Terminal>,
        terminal_view: &Entity<TerminalView>,
        e: &MouseDownEvent,
        cx: &mut App,
    ) -> bool {
        // Scrollbar interaction: clicking/dragging on the scrollbar should not start a terminal
        // selection.
        let term_bounds = terminal.read(cx).last_content().terminal_bounds.bounds;
        let sb_width = if TerminalSettings::global(cx).show_scrollbar {
            SCROLLBAR_WIDTH
        } else {
            Pixels::ZERO
        };
        let sb_bounds = scrollbar_bounds_for_terminal(term_bounds, sb_width);
        if !sb_bounds.contains(&e.position) {
            return false;
        }

        terminal_view.update(cx, |view: &mut TerminalView, view_cx| {
            view.set_scrollbar_hovered(true, view_cx);
            view.begin_scrollbar_drag(e.position.y, view_cx);
            view.set_mouse_left_down_in_terminal(false);

            let track = scrollbar_track_bounds(sb_bounds);
            let total_lines = view.terminal.read(view_cx).total_lines();
            let viewport_lines = view.terminal.read(view_cx).viewport_lines();
            let current_offset = view.terminal.read(view_cx).last_content().display_offset;

            // Determine whether the press is on the thumb; only track presses jump.
            let thumb_bounds =
                thumb_bounds_for_track(track, total_lines, viewport_lines, current_offset);

            if thumb_bounds.contains(&e.position) {
                return;
            }

            let marker_hit_radius = px(7.0);
            let matches = view.terminal.read(view_cx).matches();
            let best = search_match_index_for_scrollbar_click(
                track,
                total_lines,
                viewport_lines,
                matches,
                e.position.y,
                marker_hit_radius,
            );

            let target_offset = if let Some(match_idx) = best {
                let line = matches[match_idx].start().line;
                let target_offset =
                    scroll_offset_for_line_coord_centered(total_lines, viewport_lines, line);
                view.terminal.update(view_cx, |term, _| {
                    term.activate_match(match_idx);
                });
                target_offset
            } else {
                scroll_offset_for_thumb_center_y(track, e.position.y, total_lines, viewport_lines)
            };

            view.apply_scrollbar_target_offset(target_offset, view_cx);
            view.set_scrollbar_drag_origin(e.position.y, target_offset);
        });

        true
    }

    fn begin_terminal_left_drag(
        terminal: &Entity<Terminal>,
        terminal_view: &Entity<TerminalView>,
        e: &MouseDownEvent,
        cx: &mut App,
    ) {
        // Keep selection dragging alive even when the cursor leaves the terminal hitbox. The
        // corresponding reset is handled both by the terminal hitbox mouse-up handler and a
        // window-level mouse-up handler (for releases outside the terminal).
        terminal_view.update(cx, |view: &mut TerminalView, _| {
            view.end_scrollbar_drag();
            view.set_mouse_left_down_in_terminal(true);
        });

        let scroll_top = terminal_view.read(cx).scroll_top();
        terminal.update(cx, |terminal, cx| {
            let mut adjusted_event = e.clone();
            if scroll_top > Pixels::ZERO && terminal.last_content().display_offset == 0 {
                adjusted_event.position.y += scroll_top;
            }
            terminal.mouse_down(&adjusted_event, cx);
            cx.notify();
        });
    }

    fn update_scrollbar_hover_state(
        terminal_view: &Entity<TerminalView>,
        suggestions_hovered_row: Option<usize>,
        sb_bounds: Bounds<Pixels>,
        track: Bounds<Pixels>,
        e: &MouseMoveEvent,
        cx: &mut App,
    ) {
        // Auto-hide overlay scrollbar: update hover state as the mouse enters/leaves the scrollbar
        // lane.
        let sb_hovered = sb_bounds.contains(&e.position);
        terminal_view.update(cx, |view: &mut TerminalView, view_cx| {
            view.set_suggestions_hovered(suggestions_hovered_row, view_cx);
            view.set_scrollbar_hovered(sb_hovered, view_cx);

            // Scrollbar preview tooltip for search markers.
            if !sb_hovered || view.scrollbar_dragging() {
                view.clear_scrollbar_preview(view_cx);
                return;
            }

            let total_lines = view.terminal.read(view_cx).total_lines();
            let viewport_lines = view.terminal.read(view_cx).viewport_lines();
            let matches = view.terminal.read(view_cx).matches();
            let marker_hit_radius = px(7.0);

            if let Some(match_idx) = search_match_index_for_scrollbar_hover(
                track,
                total_lines,
                viewport_lines,
                matches,
                e.position.y,
                marker_hit_radius,
            ) {
                view.set_scrollbar_preview_for_match(match_idx, e.position, view_cx);
            } else {
                view.clear_scrollbar_preview(view_cx);
            }
        });
    }

    fn handle_scrollbar_drag_mouse_move(
        terminal_view: &Entity<TerminalView>,
        track: Bounds<Pixels>,
        e: &MouseMoveEvent,
        cx: &mut App,
    ) -> bool {
        if !terminal_view.read(cx).scrollbar_dragging() {
            return false;
        }

        // If we lost the actual MouseUp, clear drag state as soon as possible.
        if !e.dragging() {
            terminal_view.update(cx, |view: &mut TerminalView, _| {
                view.end_scrollbar_drag();
            });
            return true;
        }

        terminal_view.update(cx, |view: &mut TerminalView, view_cx| {
            let total_lines = view.terminal.read(view_cx).total_lines();
            let viewport_lines = view.terminal.read(view_cx).viewport_lines();
            if let Some((drag_start_y, drag_start_offset)) = view.scrollbar_drag_origin() {
                let target_offset = scroll_offset_for_drag_delta(
                    track,
                    drag_start_y,
                    e.position.y,
                    drag_start_offset,
                    total_lines,
                    viewport_lines,
                );
                view.apply_scrollbar_target_offset(target_offset, view_cx);
            }
        });

        true
    }

    fn handle_missing_left_up_for_terminal_drag(
        terminal: &Entity<Terminal>,
        terminal_view: &Entity<TerminalView>,
        dragging_from_terminal: bool,
        e: &MouseMoveEvent,
        cx: &mut App,
    ) -> bool {
        // If we somehow missed the real MouseUp (e.g. released while the window was not
        // receiving pointer events), clear the drag state on the next move so we don't get stuck
        // in a "selecting" state.
        if !dragging_from_terminal || e.dragging() {
            return false;
        }

        terminal_view.update(cx, |view: &mut TerminalView, _| {
            view.set_mouse_left_down_in_terminal(false);
        });

        terminal.update(cx, |terminal, cx| {
            terminal.mouse_up(
                &MouseUpEvent {
                    button: MouseButton::Left,
                    position: e.position,
                    modifiers: e.modifiers,
                    click_count: 0,
                },
                cx,
            );
            cx.notify();
        });

        true
    }

    #[allow(clippy::too_many_arguments)]
    fn handle_selection_drag_mouse_move(
        terminal: &Entity<Terminal>,
        terminal_view: &Entity<TerminalView>,
        hitbox: &Hitbox,
        focus: &FocusHandle,
        dragging_from_terminal: bool,
        e: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut App,
    ) {
        // If the drag started in this terminal view, keep updating the selection even if the
        // cursor leaves the terminal hitbox (or focus changes).
        if !e.dragging()
            || cx.has_active_drag()
            || (!dragging_from_terminal && !focus.is_focused(window))
        {
            return;
        }

        let hovered = hitbox.is_hovered(window);
        let scroll_top = terminal_view.read(cx).scroll_top();
        terminal.update(cx, |terminal, cx| {
            if dragging_from_terminal || terminal.selection_started() || hovered {
                let mut adjusted_event = e.clone();
                if scroll_top > Pixels::ZERO && terminal.last_content().display_offset == 0 {
                    adjusted_event.position.y += scroll_top;
                }
                // Use the terminal content region for drag-scroll detection so the scrollbar area
                // doesn't affect selection math.
                let region = terminal.last_content().terminal_bounds.bounds;
                terminal.mouse_drag(&adjusted_event, region, cx);
                cx.notify();
            }
        });
    }

    fn handle_scrollbar_hover_notify(
        terminal_view: &Entity<TerminalView>,
        sb_bounds: Bounds<Pixels>,
        position: Point<Pixels>,
        cx: &mut App,
    ) -> bool {
        if !sb_bounds.contains(&position) {
            return false;
        }

        // Keep hover tooltip responsive.
        terminal_view.update(cx, |_, view_cx| view_cx.notify());
        true
    }

    fn handle_terminal_mouse_move_if_hovered(
        terminal: &Entity<Terminal>,
        hitbox: &Hitbox,
        sb_bounds: Bounds<Pixels>,
        e: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut App,
    ) {
        if !hitbox.is_hovered(window) {
            return;
        }

        terminal.update(cx, |terminal, cx| {
            // Ignore hover updates over the scrollbar region.
            if terminal
                .last_content()
                .terminal_bounds
                .bounds
                .contains(&e.position)
                && !sb_bounds.contains(&e.position)
            {
                terminal.mouse_move(e, cx);
            }
        });
    }

    fn register_mouse_left_down_listener(&mut self) {
        let focus = self.focus.clone();
        let terminal = self.terminal.clone();
        let terminal_view = self.terminal_view.clone();

        self.interactivity.on_mouse_down(MouseButton::Left, {
            let terminal = terminal.clone();
            let focus = focus.clone();
            let terminal_view = terminal_view.clone();

            move |e, window, cx| {
                window.focus(&focus, cx);

                if Self::handle_suggestions_overlay_left_down(&terminal, &terminal_view, e, cx) {
                    return;
                }

                if Self::handle_scrollbar_left_down(&terminal, &terminal_view, e, cx) {
                    return;
                }

                Self::begin_terminal_left_drag(&terminal, &terminal_view, e, cx);
            }
        });
    }

    fn register_window_mouse_move_listener(&mut self, hitbox: Hitbox, window: &mut Window) {
        let focus = self.focus.clone();
        let terminal = self.terminal.clone();
        let terminal_view = self.terminal_view.clone();

        window.on_mouse_event({
            let terminal = terminal.clone();
            let focus = focus.clone();
            let terminal_view = terminal_view.clone();
            move |e: &MouseMoveEvent, phase, window, cx| {
                if phase != DispatchPhase::Bubble {
                    return;
                }

                let suggestions_hovered_row =
                    suggestions_overlay_row_at_position(&terminal, &terminal_view, e.position, cx);

                let term_bounds = terminal.read(cx).last_content().terminal_bounds.bounds;
                let sb_width = if TerminalSettings::global(cx).show_scrollbar {
                    SCROLLBAR_WIDTH
                } else {
                    Pixels::ZERO
                };
                let sb_bounds = scrollbar_bounds_for_terminal(term_bounds, sb_width);
                let track = scrollbar_track_bounds(sb_bounds);

                Self::update_scrollbar_hover_state(
                    &terminal_view,
                    suggestions_hovered_row,
                    sb_bounds,
                    track,
                    e,
                    cx,
                );

                if Self::handle_scrollbar_drag_mouse_move(&terminal_view, track, e, cx) {
                    return;
                }

                let dragging_from_terminal = terminal_view.read(cx).mouse_left_down_in_terminal();
                if Self::handle_missing_left_up_for_terminal_drag(
                    &terminal,
                    &terminal_view,
                    dragging_from_terminal,
                    e,
                    cx,
                ) {
                    return;
                }

                Self::handle_selection_drag_mouse_move(
                    &terminal,
                    &terminal_view,
                    &hitbox,
                    &focus,
                    dragging_from_terminal,
                    e,
                    window,
                    cx,
                );

                if Self::handle_scrollbar_hover_notify(&terminal_view, sb_bounds, e.position, cx) {
                    return;
                }

                Self::handle_terminal_mouse_move_if_hovered(
                    &terminal, &hitbox, sb_bounds, e, window, cx,
                );
            }
        });
    }

    fn register_mouse_left_up_listener(&mut self) {
        let focus = self.focus.clone();
        let terminal = self.terminal.clone();
        let terminal_view = self.terminal_view.clone();

        self.interactivity.on_mouse_up(MouseButton::Left, {
            let terminal = terminal.clone();
            let focus = focus.clone();
            let terminal_view = terminal_view.clone();
            move |e, window, cx| {
                if !focus.is_focused(window) {
                    return;
                }

                let was_scrollbar_dragging = terminal_view.read(cx).scrollbar_dragging();
                terminal_view.update(cx, |view: &mut TerminalView, _| {
                    view.set_mouse_left_down_in_terminal(false);
                    view.end_scrollbar_drag();
                });

                if was_scrollbar_dragging {
                    terminal_view.update(cx, |_, view_cx| view_cx.notify());
                    return;
                }

                terminal.update(cx, |terminal, cx| {
                    terminal.mouse_up(e, cx);
                    cx.notify();
                });
            }
        });
    }

    fn register_window_mouse_left_up_listener(&mut self, window: &mut Window) {
        let terminal = self.terminal.clone();
        let terminal_view = self.terminal_view.clone();

        // Ensure we end selection even when the mouse button is released outside the terminal
        // hitbox (e.g. while dragging selection across other UI).
        window.on_mouse_event({
            let terminal = terminal.clone();
            let terminal_view = terminal_view.clone();
            move |e: &MouseUpEvent, phase, _window, cx| {
                if phase != DispatchPhase::Bubble {
                    return;
                }
                if e.button != MouseButton::Left {
                    return;
                }
                if !terminal_view.read(cx).mouse_left_down_in_terminal()
                    && !terminal_view.read(cx).scrollbar_dragging()
                {
                    return;
                }

                let was_scrollbar_dragging = terminal_view.read(cx).scrollbar_dragging();
                terminal_view.update(cx, |view: &mut TerminalView, _| {
                    view.set_mouse_left_down_in_terminal(false);
                    view.end_scrollbar_drag();
                });

                if was_scrollbar_dragging {
                    terminal_view.update(cx, |_, view_cx| view_cx.notify());
                    return;
                }

                terminal.update(cx, |terminal, cx| {
                    terminal.mouse_up(e, cx);
                    cx.notify();
                });
            }
        });
    }

    fn register_mouse_middle_down_listener(&mut self) {
        let terminal = self.terminal.clone();
        let focus = self.focus.clone();
        self.interactivity.on_mouse_down(
            MouseButton::Middle,
            TerminalElement::generic_button_handler(
                terminal.clone(),
                focus.clone(),
                true,
                move |terminal, e, cx| {
                    terminal.mouse_down(e, cx);
                },
            ),
        );
    }

    fn register_scroll_wheel_listener(&mut self) {
        self.interactivity.on_scroll_wheel({
            let terminal_view = self.terminal_view.downgrade();
            move |e, window, cx| {
                terminal_view
                    .update(cx, |terminal_view, cx| {
                        if terminal_view.focus_handle.is_focused(window) {
                            terminal_view.scroll_wheel(e, window, cx);
                            cx.notify();
                        }
                    })
                    .ok();
            }
        });
    }

    fn register_mouse_mode_listeners(&mut self, mode: TerminalMode) {
        if !mode.intersects(TerminalMode::MOUSE_MODE) {
            return;
        }

        let terminal = self.terminal.clone();
        let focus = self.focus.clone();

        // Mouse mode handlers:
        // All mouse modes need the extra click handlers
        self.interactivity.on_mouse_down(
            MouseButton::Right,
            TerminalElement::generic_button_handler(
                terminal.clone(),
                focus.clone(),
                true,
                move |terminal, e, cx| {
                    terminal.mouse_down(e, cx);
                },
            ),
        );
        self.interactivity.on_mouse_up(
            MouseButton::Right,
            TerminalElement::generic_button_handler(
                terminal.clone(),
                focus.clone(),
                false,
                move |terminal, e, cx| {
                    terminal.mouse_up(e, cx);
                },
            ),
        );
        self.interactivity.on_mouse_up(
            MouseButton::Middle,
            TerminalElement::generic_button_handler(
                terminal,
                focus,
                false,
                move |terminal, e, cx| {
                    terminal.mouse_up(e, cx);
                },
            ),
        );
    }
}

struct PrepaintTypography {
    minimum_contrast: f32,
    show_scrollbar: bool,
    show_line_numbers_setting: bool,
    text_style: TextStyle,
    link_style: HighlightStyle,
    rem_size: Pixels,
    line_height_px: Pixels,
    cell_width: Pixels,
}

struct SyncedLayout {
    dimensions: TerminalBounds,
    gutter: Pixels,
    line_number_width: Pixels,
    line_number_digits: usize,
    scrollbar_width: Pixels,
    scrollbar_visible: bool,
    last_hovered_word: Option<HoveredWord>,
}

struct PrepaintArtifacts {
    mode: TerminalMode,
    display_offset: usize,
    line_number_paint_data: Option<LineNumberPaintData>,
    cursor: crate::Cursor,
    cursor_char: char,
    scroll_top: Pixels,
    background_color: Hsla,
    relative_highlighted_ranges: Vec<(RangeInclusive<GridPoint>, Hsla)>,
    bg_quads: Vec<BgQuad>,
    text_spans: Vec<TextSpan>,
    scrollbar: ScrollbarLayoutState,
    scrollbar_markers: Vec<Pixels>,
    scrollbar_active_marker: Option<Pixels>,
}

impl TerminalElement {
    #[allow(clippy::too_many_arguments)]
    fn prepaint_layout_state_for(
        terminal: &Entity<Terminal>,
        terminal_view: &Entity<TerminalView>,
        focused: bool,
        block_below_cursor: Option<&Rc<BlockProperties>>,
        bounds: Bounds<Pixels>,
        hitbox: Hitbox,
        window: &mut Window,
        cx: &mut App,
    ) -> LayoutState {
        let typography = Self::collect_prepaint_typography(window, cx);
        let synced_layout = Self::sync_and_compute_prepaint_layout(
            terminal,
            terminal_view,
            bounds,
            &typography,
            window,
            cx,
        );
        Self::build_layout_state_for_prepaint(
            terminal,
            terminal_view,
            focused,
            block_below_cursor,
            bounds,
            hitbox,
            typography,
            synced_layout,
            window,
            cx,
        )
    }

    fn collect_prepaint_typography(window: &mut Window, cx: &mut App) -> PrepaintTypography {
        let terminal_settings = TerminalSettings::global(cx);
        let minimum_contrast = terminal_settings.minimum_contrast;
        let show_scrollbar = terminal_settings.show_scrollbar;
        let show_line_numbers_setting = terminal_settings.show_line_numbers;
        let font_weight = terminal_settings.font_weight;
        let line_height = terminal_settings.line_height.value();
        let text_style =
            Self::build_terminal_text_style(terminal_settings, font_weight, line_height, cx);
        let link_style = Self::build_terminal_link_style(font_weight, cx);

        let text_system = cx.text_system();
        let rem_size = window.rem_size();
        let font_pixels = text_style.font_size.to_pixels(rem_size);
        let line_height_px = f32::from(font_pixels) * line_height.to_pixels(rem_size);
        let font_id = text_system.resolve_font(&text_style.font());
        let cell_width = text_system
            .advance(font_id, font_pixels, 'm')
            .unwrap()
            .width;

        PrepaintTypography {
            minimum_contrast,
            show_scrollbar,
            show_line_numbers_setting,
            text_style,
            link_style,
            rem_size,
            line_height_px,
            cell_width,
        }
    }

    fn sync_and_compute_prepaint_layout(
        terminal: &Entity<Terminal>,
        terminal_view: &Entity<TerminalView>,
        bounds: Bounds<Pixels>,
        typography: &PrepaintTypography,
        window: &mut Window,
        cx: &mut App,
    ) -> SyncedLayout {
        let scrollbar_visible =
            Self::should_show_scrollbar_overlay(typography.show_scrollbar, terminal_view, cx);

        // Use the previous snapshot mode as an early hint; we'll reconcile after sync.
        let (initial_mode, total_lines_for_digits) =
            Self::initial_terminal_mode_and_total_lines(terminal, cx);
        let mut show_line_numbers_for_layout =
            should_show_line_numbers(typography.show_line_numbers_setting, initial_mode);
        let mut reserve_left_padding_without_line_numbers_for_layout =
            reserve_left_padding_without_line_numbers(
                typography.show_line_numbers_setting,
                initial_mode,
            );

        let (
            mut dimensions,
            mut gutter,
            mut line_number_width,
            mut line_number_digits,
            mut scrollbar_width,
        ) = compute_terminal_layout_metrics(
            bounds,
            typography.cell_width,
            typography.line_height_px,
            typography.show_scrollbar,
            show_line_numbers_for_layout,
            reserve_left_padding_without_line_numbers_for_layout,
            total_lines_for_digits,
        );

        // First sync using the early mode hint.
        let hover_word = terminal_view.read(cx).hover_word.clone();
        let mut last_hovered_word =
            Self::sync_terminal_for_prepaint(terminal, dimensions, bounds, &hover_word, window, cx);

        // After syncing, reconcile line number visibility with the updated mode.
        let mode_after_sync = terminal.read(cx).last_content().mode;
        if should_relayout_for_mode_change(
            typography.show_line_numbers_setting,
            initial_mode,
            mode_after_sync,
        ) {
            show_line_numbers_for_layout =
                should_show_line_numbers(typography.show_line_numbers_setting, mode_after_sync);
            reserve_left_padding_without_line_numbers_for_layout =
                reserve_left_padding_without_line_numbers(
                    typography.show_line_numbers_setting,
                    mode_after_sync,
                );

            let total_lines_for_digits = terminal.read(cx).total_lines();
            (
                dimensions,
                gutter,
                line_number_width,
                line_number_digits,
                scrollbar_width,
            ) = compute_terminal_layout_metrics(
                bounds,
                typography.cell_width,
                typography.line_height_px,
                typography.show_scrollbar,
                show_line_numbers_for_layout,
                reserve_left_padding_without_line_numbers_for_layout,
                total_lines_for_digits,
            );

            last_hovered_word = Self::sync_terminal_for_prepaint(
                terminal,
                dimensions,
                bounds,
                &hover_word,
                window,
                cx,
            );
        }

        SyncedLayout {
            dimensions,
            gutter,
            line_number_width,
            line_number_digits,
            scrollbar_width,
            scrollbar_visible,
            last_hovered_word,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn build_layout_state_for_prepaint(
        terminal: &Entity<Terminal>,
        terminal_view: &Entity<TerminalView>,
        focused: bool,
        block_below_cursor: Option<&Rc<BlockProperties>>,
        bounds: Bounds<Pixels>,
        hitbox: Hitbox,
        typography: PrepaintTypography,
        synced_layout: SyncedLayout,
        window: &mut Window,
        cx: &mut App,
    ) -> LayoutState {
        let SyncedLayout {
            dimensions,
            gutter,
            line_number_width,
            line_number_digits,
            scrollbar_width,
            scrollbar_visible,
            last_hovered_word,
        } = synced_layout;

        let artifacts = Self::compute_prepaint_artifacts(
            terminal,
            terminal_view,
            dimensions,
            line_number_digits,
            scrollbar_width,
            scrollbar_visible,
            &typography,
            last_hovered_word.as_ref(),
            cx,
        );

        // Layout cursor. Rectangle is used for IME, so we should lay it out even if we don't end
        // up showing it.
        let cursor = Self::layout_cursor(
            focused,
            artifacts.cursor,
            artifacts.cursor_char,
            artifacts.display_offset,
            dimensions,
            &typography.text_style,
            window,
            cx,
        );

        let block_below_cursor_element = Self::prepaint_block_below_cursor_element(
            terminal,
            block_below_cursor,
            bounds,
            artifacts.scroll_top,
            typography.rem_size,
            dimensions,
            gutter,
            window,
            cx,
        );

        LayoutState {
            hitbox,
            bg_quads: artifacts.bg_quads,
            text_spans: artifacts.text_spans,
            cursor,
            background_color: artifacts.background_color,
            dimensions,
            display_offset: artifacts.display_offset,
            relative_highlighted_ranges: artifacts.relative_highlighted_ranges,
            mode: artifacts.mode,
            line_number_state: LineNumberState {
                gutter,
                line_number_width,
                line_number_digits,
            },
            line_number_paint_data: artifacts.line_number_paint_data,
            block_below_cursor_element,
            base_text_style: typography.text_style,
            scrollbar: artifacts.scrollbar,
            scrollbar_visible,
            scrollbar_markers: artifacts.scrollbar_markers,
            scrollbar_active_marker: artifacts.scrollbar_active_marker,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn compute_prepaint_artifacts(
        terminal: &Entity<Terminal>,
        terminal_view: &Entity<TerminalView>,
        dimensions: TerminalBounds,
        line_number_digits: usize,
        scrollbar_width: Pixels,
        scrollbar_visible: bool,
        typography: &PrepaintTypography,
        last_hovered_word: Option<&HoveredWord>,
        cx: &App,
    ) -> PrepaintArtifacts {
        let terminal_read = terminal.read(cx);
        let search_matches = terminal_read.matches().to_vec();
        let total_lines = terminal_read.total_lines();
        let viewport_lines = terminal_read.viewport_lines();
        let line_number_paint_data = (line_number_digits != 0)
            .then(|| {
                compute_line_number_paint_data(
                    terminal_read,
                    terminal_read.last_content().display_offset,
                    dimensions.num_lines(),
                )
            })
            .flatten();
        let active_match_index = terminal_read.active_match_index();
        let clicked_line = terminal_read.last_line();
        let cursor_line_id = terminal_read.cursor_line_id();

        let content = terminal_read.last_content();
        let cells = content.cells.as_slice();
        let mode = content.mode;
        let display_offset = content.display_offset;
        let cursor_char = content.cursor_char;
        let selection = content.selection.as_ref();
        let cursor = content.cursor;
        let terminal_bounds = content.terminal_bounds;

        let terminal_view_read = terminal_view.read(cx);
        let scroll_top = terminal_view_read.scroll_top();

        let (scrollbar, scrollbar_markers, scrollbar_active_marker) =
            Self::compute_scrollbar_layout_and_markers(
                scrollbar_visible,
                dimensions,
                scrollbar_width,
                total_lines,
                viewport_lines,
                display_offset,
                terminal_view_read,
                &search_matches,
                active_match_index,
            );

        let relative_highlighted_ranges = Self::build_relative_highlighted_ranges(
            &search_matches,
            active_match_index,
            selection,
            clicked_line,
            terminal_bounds,
            cx,
        );

        let (bg_quads, text_spans) = Self::build_bg_quads_and_text_spans(
            cells,
            display_offset,
            &typography.text_style,
            typography.link_style,
            last_hovered_word,
            typography.minimum_contrast,
            terminal_view_read,
            content,
            cursor_line_id,
            terminal_bounds,
            cx,
        );

        let background_color = Self::background_color_for_cells(cells, cx);

        PrepaintArtifacts {
            mode,
            display_offset,
            line_number_paint_data,
            cursor,
            cursor_char,
            scroll_top,
            background_color,
            relative_highlighted_ranges,
            bg_quads,
            text_spans,
            scrollbar,
            scrollbar_markers,
            scrollbar_active_marker,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn compute_scrollbar_layout_and_markers(
        scrollbar_visible: bool,
        dimensions: TerminalBounds,
        scrollbar_width: Pixels,
        total_lines: usize,
        viewport_lines: usize,
        display_offset: usize,
        terminal_view: &TerminalView,
        search_matches: &[RangeInclusive<GridPoint>],
        active_match_index: Option<usize>,
    ) -> (ScrollbarLayoutState, Vec<Pixels>, Option<Pixels>) {
        // While the user is dragging the scrollbar, use the view-local "virtual" offset so the
        // thumb updates immediately, without waiting for the backend sync.
        let display_offset_for_thumb = terminal_view
            .scrollbar_virtual_offset()
            .unwrap_or(display_offset);
        let scrollbar = overlay_scrollbar_layout_state(
            dimensions.bounds,
            scrollbar_width,
            total_lines,
            viewport_lines,
            display_offset_for_thumb,
        );

        let (scrollbar_markers, scrollbar_active_marker) = Self::compute_scrollbar_markers(
            scrollbar_visible,
            &scrollbar,
            search_matches,
            total_lines,
            viewport_lines,
            active_match_index,
        );

        (scrollbar, scrollbar_markers, scrollbar_active_marker)
    }

    #[allow(clippy::too_many_arguments)]
    fn build_bg_quads_and_text_spans(
        cells: &[crate::IndexedCell],
        display_offset: usize,
        text_style: &TextStyle,
        link_style: HighlightStyle,
        last_hovered_word: Option<&HoveredWord>,
        minimum_contrast: f32,
        terminal_view: &TerminalView,
        content: &crate::TerminalContent,
        cursor_line_id: Option<i64>,
        terminal_bounds: TerminalBounds,
        cx: &App,
    ) -> (Vec<BgQuad>, Vec<TextSpan>) {
        // `cells[i].point.line` is backend-relative (may be negative when scrolled back).
        // `build_plan` expects viewport-relative line numbers, so offset by `display_offset` to
        // keep the rendered text anchored in the viewport when scrolling.
        let start_line_offset = (display_offset.min(i32::MAX as usize)) as i32;
        let link = last_hovered_word.map(|w| (link_style, &w.word_match));
        let (mut bg_quads, text_spans) = build_plan(
            cells,
            start_line_offset,
            cx.theme(),
            text_style,
            link,
            minimum_contrast,
        );

        if let Some(snippet) =
            terminal_view.snippet_snapshot_for_content(content, cursor_line_id, cx)
        {
            let cols = terminal_bounds.num_columns().max(1);
            let (active_bg, inactive_bg) = placeholder_highlight_bgs(cx.theme());
            bg_quads.extend(snippet_placeholder_bg_quads(
                &snippet,
                cols,
                active_bg,
                inactive_bg,
            ));
        }

        (bg_quads, text_spans)
    }

    fn background_color_for_cells(cells: &[crate::IndexedCell], cx: &App) -> Hsla {
        let bg_color = dominant_effective_background_color(cells);
        convert_color(&bg_color, cx.theme())
    }

    fn initial_terminal_mode_and_total_lines(
        terminal: &Entity<Terminal>,
        cx: &App,
    ) -> (TerminalMode, usize) {
        let terminal = terminal.read(cx);
        (terminal.last_content().mode, terminal.total_lines())
    }

    fn should_show_scrollbar_overlay(
        show_scrollbar: bool,
        terminal_view: &Entity<TerminalView>,
        cx: &App,
    ) -> bool {
        show_scrollbar && {
            let view = terminal_view.read(cx);
            view.scrollbar_dragging()
                || view.scrollbar_hovered()
                || view.scrollbar_revealed()
                || view.is_search_open()
        }
    }

    fn build_terminal_link_style(font_weight: gpui::FontWeight, cx: &App) -> HighlightStyle {
        HighlightStyle {
            color: Some(cx.theme().info_hover),
            font_weight: Some(font_weight),
            font_style: None,
            background_color: None,
            underline: Some(UnderlineStyle {
                thickness: px(1.0),
                color: Some(cx.theme().info_hover),
                wavy: false,
            }),
            strikethrough: None,
            fade_out: None,
        }
    }

    fn build_terminal_text_style(
        terminal_settings: &TerminalSettings,
        font_weight: gpui::FontWeight,
        line_height: AbsoluteLength,
        cx: &App,
    ) -> TextStyle {
        TextStyle {
            font_family: terminal_settings.font_family.clone(),
            font_features: terminal_settings.font_features.clone(),
            font_weight,
            font_fallbacks: terminal_settings.font_fallbacks.clone(),
            font_size: terminal_settings.font_size.into(),
            font_style: FontStyle::Normal,
            line_height: line_height.into(),
            background_color: Some(cx.theme().background),
            white_space: WhiteSpace::Normal,
            // These are going to be overridden per-cell
            color: cx.theme().foreground,
            ..Default::default()
        }
    }

    fn sync_terminal_for_prepaint(
        terminal: &Entity<Terminal>,
        dimensions: TerminalBounds,
        bounds: Bounds<Pixels>,
        hover_word: &Option<HoveredWord>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<HoveredWord> {
        terminal.update(cx, |terminal, cx| {
            terminal.set_size(dimensions);
            terminal.sync(window, cx);

            if window.modifiers().secondary()
                && bounds.contains(&window.mouse_position())
                && hover_word.is_some()
            {
                if terminal.last_content().last_hovered_word.as_ref() == hover_word.as_ref() {
                    terminal.last_content().last_hovered_word.clone()
                } else {
                    None
                }
            } else {
                None
            }
        })
    }

    fn compute_scrollbar_markers(
        scrollbar_visible: bool,
        scrollbar: &ScrollbarLayoutState,
        search_matches: &[RangeInclusive<GridPoint>],
        total_lines: usize,
        viewport_lines: usize,
        active_match_index: Option<usize>,
    ) -> (Vec<Pixels>, Option<Pixels>) {
        if scrollbar_visible && !search_matches.is_empty() && total_lines > 0 && viewport_lines > 0
        {
            let track = scrollbar.track_bounds;
            let mut seen_y: HashSet<i32> = HashSet::new();
            let mut ys: Vec<Pixels> = Vec::new();

            for m in search_matches {
                if let Some(y) = scrollbar_marker_y_for_line_coord(
                    track,
                    total_lines,
                    viewport_lines,
                    m.start().line,
                ) {
                    let key = ((y - track.origin.y) / px(1.0)).round() as i32;
                    if seen_y.insert(key) {
                        ys.push(y);
                        if ys.len() >= 4096 {
                            break;
                        }
                    }
                }
            }

            let active_y = active_match_index
                .and_then(|idx| search_matches.get(idx))
                .and_then(|range| {
                    scrollbar_marker_y_for_line_coord(
                        track,
                        total_lines,
                        viewport_lines,
                        range.start().line,
                    )
                });

            (ys, active_y)
        } else {
            (Vec::new(), None)
        }
    }

    fn build_relative_highlighted_ranges(
        search_matches: &[RangeInclusive<GridPoint>],
        active_match_index: Option<usize>,
        selection: Option<&crate::SelectionRange>,
        clicked_line: Option<i32>,
        terminal_bounds: TerminalBounds,
        cx: &App,
    ) -> Vec<(RangeInclusive<GridPoint>, Hsla)> {
        let mut relative_highlighted_ranges = Vec::new();

        let search_color = cx.theme().selection.opacity(0.25);
        for search_match in search_matches {
            relative_highlighted_ranges.push((search_match.clone(), search_color));
        }
        if let Some(active_idx) = active_match_index
            && let Some(active_range) = search_matches.get(active_idx)
        {
            relative_highlighted_ranges
                .push((active_range.clone(), cx.theme().selection.opacity(0.55)));
        }
        if let Some(selection) = selection {
            relative_highlighted_ranges
                .push((selection.start..=selection.end, cx.theme().selection));
        }

        if let Some(line) = clicked_line {
            relative_highlighted_ranges.push((
                GridPoint::new(line, 0)..=GridPoint::new(line, terminal_bounds.last_column()),
                cx.theme().selection.opacity(0.5),
            ));
        }

        relative_highlighted_ranges
    }

    #[allow(clippy::too_many_arguments)]
    fn layout_cursor(
        focused: bool,
        cursor: crate::Cursor,
        cursor_char: char,
        display_offset: usize,
        dimensions: TerminalBounds,
        text_style: &TextStyle,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<CursorLayout> {
        if cursor.shape == CursorRenderShape::Hidden {
            return None;
        }

        let cursor_point = DisplayCursor::from(cursor.point, display_offset);
        let cursor_text = {
            let text = cursor_char.to_string();
            let len = text.len();
            window.text_system().shape_line(
                text.into(),
                text_style.font_size.to_pixels(window.rem_size()),
                &[TextRun {
                    len,
                    font: text_style.font(),
                    color: cx.theme().background,
                    background_color: None,
                    underline: Default::default(),
                    strikethrough: None,
                }],
                None,
            )
        };

        let cursor_color = cx.theme().caret;
        let (cursor_position, block_width) =
            TerminalElement::shape_cursor(cursor_point, dimensions, &cursor_text)?;

        let (shape, text) = match cursor.shape {
            CursorRenderShape::Block if !focused => (CursorShape::Hollow, None),
            CursorRenderShape::Block => (CursorShape::Block, Some(cursor_text)),
            CursorRenderShape::Underline => (CursorShape::Underline, None),
            CursorRenderShape::Bar => (CursorShape::Bar, None),
            CursorRenderShape::Hollow => (CursorShape::Hollow, None),
            CursorRenderShape::Hidden => unreachable!(),
        };

        Some(CursorLayout::new(
            cursor_position,
            block_width,
            dimensions.line_height,
            cursor_color,
            shape,
            text,
        ))
    }

    #[allow(clippy::too_many_arguments)]
    fn prepaint_block_below_cursor_element(
        terminal: &Entity<Terminal>,
        block_below_cursor: Option<&Rc<BlockProperties>>,
        bounds: Bounds<Pixels>,
        scroll_top: Pixels,
        rem_size: Pixels,
        dimensions: TerminalBounds,
        gutter: Pixels,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        let block = block_below_cursor?;

        let terminal_read = terminal.read(cx);
        if terminal_read.last_content().display_offset != 0 {
            return None;
        }

        let target_line = terminal_read.last_content().cursor.point.line + 1;
        let render = &block.render;
        let mut block_cx = BlockContext {
            window,
            context: cx,
            dimensions,
        };
        let element = render(&mut block_cx);
        let mut element = div().occlude().child(element).into_any_element();
        let available_space = size(
            AvailableSpace::Definite(dimensions.width() + gutter),
            AvailableSpace::Definite(block.height as f32 * dimensions.line_height()),
        );
        let origin = bounds.origin + point(px(0.), target_line as f32 * dimensions.line_height())
            - point(px(0.), scroll_top);
        window.with_rem_size(Some(rem_size), |window| {
            element.prepaint_as_root(origin, available_space, window, cx);
        });
        Some(element)
    }
}

impl Element for TerminalElement {
    type RequestLayoutState = ();
    type PrepaintState = LayoutState;

    fn id(&self) -> Option<ElementId> {
        self.interactivity.element_id.clone()
    }

    fn source_location(&self) -> Option<&'static Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let layout_id = self.interactivity.request_layout(
            global_id,
            inspector_id,
            window,
            cx,
            |mut style, window, cx| {
                style.size.width = relative(1.).into();
                style.size.height = relative(1.).into();

                window.request_layout(style, None, cx)
            },
        );
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let terminal = self.terminal.clone();
        let terminal_view = self.terminal_view.clone();
        let focused = self.focused;
        let block_below_cursor = self.block_below_cursor.clone();

        self.interactivity.prepaint(
            global_id,
            inspector_id,
            bounds,
            bounds.size,
            window,
            cx,
            move |_, _, hitbox, window, cx| {
                TerminalElement::prepaint_layout_state_for(
                    &terminal,
                    &terminal_view,
                    focused,
                    block_below_cursor.as_ref(),
                    bounds,
                    hitbox.unwrap(),
                    window,
                    cx,
                )
            },
        )
    }

    fn paint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        layout: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let paint_start = Instant::now();
        let focus = self.focus.clone();
        let terminal = self.terminal.clone();
        let terminal_view = self.terminal_view.clone();
        let cursor_visible = self.cursor_visible;
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            let scroll_top = if self.block_below_cursor.is_some() && layout.display_offset == 0 {
                terminal_view.read(cx).scroll_top()
            } else {
                Pixels::ZERO
            };

            window.paint_quad(fill(bounds, layout.background_color));

            let origin = bounds.origin + Point::new(layout.line_number_state.gutter, px(0.))
                - Point::new(px(0.), scroll_top);

            let marked_text_cloned = terminal_element_marked_text(&terminal_view, cx);

            let terminal_input_handler = TerminalInputHandler {
                terminal: terminal.clone(),
                terminal_view: terminal_view.clone(),
                cursor_bounds: layout
                    .cursor
                    .as_ref()
                    .map(|cursor| cursor.bounding_rect(origin)),
            };

            self.register_mouse_listeners(layout.mode, &layout.hitbox, window);
            let mouse_pos = window.mouse_position();
            let should_point = layout.scrollbar.bounds.contains(&mouse_pos)
                || (window.modifiers().secondary()
                    && layout.dimensions.bounds.contains(&mouse_pos)
                    && terminal_view.read(cx).hover_word.is_some());
            window.set_cursor_style(
                if should_point {
                    gpui::CursorStyle::PointingHand
                } else {
                    gpui::CursorStyle::IBeam
                },
                &layout.hitbox,
            );

            let original_cursor = layout.cursor.take();
            let block_below_cursor_element = layout.block_below_cursor_element.take();
            self.interactivity.paint(
                global_id,
                inspector_id,
                bounds,
                Some(&layout.hitbox),
                window,
                cx,
                |_, window, cx| {
                    terminal_element_install_input_handlers(
                        &focus,
                        &terminal_view,
                        terminal_input_handler,
                        window,
                        cx,
                    );
                    terminal_element_install_modifier_listener(&terminal, window);

                    terminal_element_paint_static_layers(
                        bounds, scroll_top, origin, layout, window, cx,
                    );
                    let text_paint_time =
                        terminal_element_paint_text_spans(origin, layout, window, cx);

                    let cursor_bounds = original_cursor
                        .as_ref()
                        .map(|cursor| cursor.bounding_rect(origin));

                    let mut original_cursor = original_cursor;
                    let mut block_below_cursor_element = block_below_cursor_element;
                    terminal_element_paint_overlays(
                        cursor_visible,
                        marked_text_cloned,
                        &mut original_cursor,
                        cursor_bounds,
                        &terminal_view,
                        bounds,
                        origin,
                        layout,
                        &mut block_below_cursor_element,
                        window,
                        cx,
                    );

                    terminal_element_log_paint_stats(layout, paint_start, text_paint_time);
                },
            );
        });
    }
}

fn terminal_element_marked_text(terminal_view: &Entity<TerminalView>, cx: &App) -> Option<String> {
    let ime_state = &terminal_view.read(cx).ime_state;
    ime_state.as_ref().map(|state| state.marked_text.clone())
}

fn terminal_element_install_input_handlers(
    focus: &FocusHandle,
    terminal_view: &Entity<TerminalView>,
    terminal_input_handler: TerminalInputHandler,
    window: &mut Window,
    cx: &mut App,
) {
    if terminal_view.read(cx).is_search_open() {
        window.handle_input(
            focus,
            SearchInputHandler {
                terminal_view: terminal_view.clone(),
            },
            cx,
        );
    } else {
        window.handle_input(focus, terminal_input_handler, cx);
    }
}

fn terminal_element_install_modifier_listener(terminal: &Entity<Terminal>, window: &mut Window) {
    let terminal = terminal.clone();
    window.on_key_event(move |event: &ModifiersChangedEvent, phase, window, cx| {
        if phase != DispatchPhase::Bubble {
            return;
        }

        terminal.update(cx, |term, cx| {
            term.try_modifiers_change(&event.modifiers, window, cx)
        });
    });
}

fn terminal_element_paint_static_layers(
    bounds: Bounds<Pixels>,
    scroll_top: Pixels,
    origin: Point<Pixels>,
    layout: &LayoutState,
    window: &mut Window,
    cx: &mut App,
) {
    paint_line_numbers(
        bounds,
        scroll_top,
        layout.line_number_state,
        layout.mode,
        &layout.dimensions,
        layout.line_number_paint_data.as_ref(),
        &layout.base_text_style,
        window,
        cx,
    );

    for rect in &layout.bg_quads {
        rect.paint(
            origin,
            layout.dimensions.cell_width,
            layout.dimensions.line_height,
            window,
        );
    }

    for (relative_highlighted_range, color) in layout.relative_highlighted_ranges.iter() {
        if let Some((start_y, highlighted_range_lines)) =
            to_highlighted_range_lines(relative_highlighted_range, layout, origin)
        {
            let hr = HighlightedRange {
                start_y,
                line_height: layout.dimensions.line_height,
                lines: highlighted_range_lines,
                color: *color,
                corner_radius: 0.15 * layout.dimensions.line_height,
            };
            hr.paint(true, bounds, window);
        }
    }
}

fn terminal_element_paint_text_spans(
    origin: Point<Pixels>,
    layout: &LayoutState,
    window: &mut Window,
    cx: &mut App,
) -> Duration {
    // Paint batched text runs instead of individual cells.
    let text_paint_start = Instant::now();
    for span in &layout.text_spans {
        span.paint(
            origin,
            layout.dimensions.cell_width,
            layout.dimensions.line_height,
            window,
            cx,
        );
    }
    text_paint_start.elapsed()
}

#[allow(clippy::too_many_arguments)]
fn terminal_element_paint_overlays(
    cursor_visible: bool,
    marked_text: Option<String>,
    original_cursor: &mut Option<CursorLayout>,
    cursor_bounds: Option<Bounds<Pixels>>,
    terminal_view: &Entity<TerminalView>,
    bounds: Bounds<Pixels>,
    origin: Point<Pixels>,
    layout: &LayoutState,
    block_below_cursor_element: &mut Option<AnyElement>,
    window: &mut Window,
    cx: &mut App,
) {
    if let Some(text_to_mark) = marked_text.as_ref()
        && !text_to_mark.is_empty()
        && let Some(cursor_layout) = original_cursor.as_ref()
    {
        let ime_position = cursor_layout.bounding_rect(origin).origin;
        let mut ime_style = layout.base_text_style.clone();
        ime_style.underline = Some(UnderlineStyle {
            color: Some(ime_style.color),
            thickness: px(1.0),
            wavy: false,
        });

        let shaped_line = window.text_system().shape_line(
            text_to_mark.clone().into(),
            ime_style.font_size.to_pixels(window.rem_size()),
            &[TextRun {
                len: text_to_mark.len(),
                font: ime_style.font(),
                color: ime_style.color,
                background_color: None,
                underline: ime_style.underline,
                strikethrough: None,
            }],
            None,
        );
        // TODO: log err
        let _ = shaped_line.paint(
            ime_position,
            layout.dimensions.line_height,
            TextAlign::Left,
            None,
            window,
            cx,
        );
    }

    if cursor_visible
        && marked_text.is_none()
        && let Some(mut cursor) = original_cursor.take()
    {
        cursor.paint(origin, window, cx);
    }

    let suggestions_snapshot = terminal_view.read(cx).suggestions_snapshot();
    if let Some(cursor_bounds) = cursor_bounds
        && let Some((items, selected)) = suggestions_snapshot.as_ref()
        && !items.is_empty()
    {
        paint_suggestions_overlay(
            cursor_bounds,
            bounds,
            layout.dimensions,
            items,
            *selected,
            &layout.base_text_style,
            window,
            cx,
        );
    }

    if let Some(mut element) = block_below_cursor_element.take() {
        element.paint(window, cx);
    }

    // Scrollbar.
    if layout.scrollbar_visible && layout.scrollbar.bounds.size.width > Pixels::ZERO {
        paint_overlay_scrollbar(
            &layout.scrollbar,
            &layout.scrollbar_markers,
            layout.scrollbar_active_marker,
            window,
            cx,
        );
    }
}

fn terminal_element_log_paint_stats(
    layout: &LayoutState,
    paint_start: Instant,
    text_paint_time: Duration,
) {
    let total_paint_time = paint_start.elapsed();
    log::debug!(
        "Terminal paint: {} text runs, {} rects, text paint took {:?}, total paint took {:?}",
        layout.text_spans.len(),
        layout.bg_quads.len(),
        text_paint_time,
        total_paint_time
    );
}

impl IntoElement for TerminalElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

struct TerminalInputHandler {
    terminal: Entity<Terminal>,
    terminal_view: Entity<TerminalView>,
    cursor_bounds: Option<Bounds<Pixels>>,
}

struct SearchInputHandler {
    terminal_view: Entity<TerminalView>,
}

impl InputHandler for SearchInputHandler {
    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _: &mut Window,
        cx: &mut App,
    ) -> Option<UTF16Selection> {
        let caret = self.terminal_view.read(cx).search_cursor_utf16();
        Some(UTF16Selection {
            range: caret..caret,
            reversed: false,
        })
    }

    fn marked_text_range(
        &mut self,
        _window: &mut Window,
        cx: &mut App,
    ) -> Option<std::ops::Range<usize>> {
        self.terminal_view.read(cx).search_marked_text_range()
    }

    fn text_for_range(
        &mut self,
        _: std::ops::Range<usize>,
        _: &mut Option<std::ops::Range<usize>>,
        _: &mut Window,
        _: &mut App,
    ) -> Option<String> {
        None
    }

    fn replace_text_in_range(
        &mut self,
        _replacement_range: Option<std::ops::Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut App,
    ) {
        let terminal_view = self.terminal_view.clone();
        let text = text.to_string();
        cx.defer(move |cx| {
            terminal_view.update(cx, |view, view_cx| {
                view.clear_search_marked_text(view_cx);
                view.commit_search_text(&text, view_cx);
            });
        });
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range_utf16: Option<std::ops::Range<usize>>,
        new_text: &str,
        new_marked_range: Option<std::ops::Range<usize>>,
        _window: &mut Window,
        cx: &mut App,
    ) {
        let terminal_view = self.terminal_view.clone();
        let new_text = new_text.to_string();
        cx.defer(move |cx| {
            terminal_view.update(cx, |view, view_cx| {
                view.set_search_marked_text(new_text.clone(), new_marked_range, view_cx);
            });
        });
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut App) {
        let terminal_view = self.terminal_view.clone();
        cx.defer(move |cx| {
            terminal_view.update(cx, |view, view_cx| {
                view.clear_search_marked_text(view_cx);
            });
        });
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: std::ops::Range<usize>,
        _window: &mut Window,
        cx: &mut App,
    ) -> Option<Bounds<Pixels>> {
        // Approximate a caret bounds inside the search panel, so IME candidate windows appear
        // near the search input.
        let view = self.terminal_view.read(cx);
        let origin = view.search_panel_pos()
            + point(px(22.0), px(64.0))
            + point(px(7.5) * range_utf16.start as f32, px(0.0));
        Some(Bounds {
            origin,
            size: size(px(1.0), px(18.0)),
        })
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<usize> {
        None
    }

    fn apple_press_and_hold_enabled(&mut self) -> bool {
        false
    }
}

impl InputHandler for TerminalInputHandler {
    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _: &mut Window,
        cx: &mut App,
    ) -> Option<UTF16Selection> {
        if self
            .terminal
            .read(cx)
            .last_content()
            .mode
            .contains(TerminalMode::ALT_SCREEN)
        {
            None
        } else {
            Some(UTF16Selection {
                range: 0..0,
                reversed: false,
            })
        }
    }

    fn marked_text_range(
        &mut self,
        _window: &mut Window,
        cx: &mut App,
    ) -> Option<std::ops::Range<usize>> {
        self.terminal_view.read(cx).marked_text_range()
    }

    fn text_for_range(
        &mut self,
        _: std::ops::Range<usize>,
        _: &mut Option<std::ops::Range<usize>>,
        _: &mut Window,
        _: &mut App,
    ) -> Option<String> {
        None
    }

    fn replace_text_in_range(
        &mut self,
        _replacement_range: Option<std::ops::Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut App,
    ) {
        let terminal_view = self.terminal_view.clone();
        let text = text.to_string();
        cx.defer(move |cx| {
            terminal_view.update(cx, |view, view_cx| {
                view.clear_marked_text(view_cx);
                view.commit_text(&text, view_cx);
            });
        });
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range_utf16: Option<std::ops::Range<usize>>,
        new_text: &str,
        new_marked_range: Option<std::ops::Range<usize>>,
        _window: &mut Window,
        cx: &mut App,
    ) {
        let terminal_view = self.terminal_view.clone();
        let new_text = new_text.to_string();
        cx.defer(move |cx| {
            terminal_view.update(cx, |view, view_cx| {
                view.set_marked_text(new_text.clone(), new_marked_range, view_cx);
            });
        });
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut App) {
        let terminal_view = self.terminal_view.clone();
        cx.defer(move |cx| {
            terminal_view.update(cx, |view, view_cx| {
                view.clear_marked_text(view_cx);
            });
        });
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: std::ops::Range<usize>,
        _window: &mut Window,
        cx: &mut App,
    ) -> Option<Bounds<Pixels>> {
        let term_bounds = self.terminal_view.read(cx).terminal_bounds(cx);

        let mut bounds = self.cursor_bounds?;
        let offset_x = term_bounds.cell_width * range_utf16.start as f32;
        bounds.origin.x += offset_x;

        Some(bounds)
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<usize> {
        None
    }

    fn apple_press_and_hold_enabled(&mut self) -> bool {
        false
    }
}

#[derive(Debug)]
pub struct HighlightedRange {
    pub start_y: Pixels,
    pub line_height: Pixels,
    pub lines: Vec<HighlightedRangeLine>,
    pub color: Hsla,
    pub corner_radius: Pixels,
}

#[derive(Debug)]
pub struct HighlightedRangeLine {
    pub start_x: Pixels,
    pub end_x: Pixels,
}

impl HighlightedRange {
    pub fn paint(&self, fill: bool, bounds: Bounds<Pixels>, window: &mut Window) {
        if self.lines.len() >= 2 && self.lines[0].start_x > self.lines[1].end_x {
            self.paint_lines(self.start_y, &self.lines[0..1], fill, bounds, window);
            self.paint_lines(
                self.start_y + self.line_height,
                &self.lines[1..],
                fill,
                bounds,
                window,
            );
        } else {
            self.paint_lines(self.start_y, &self.lines, fill, bounds, window);
        }
    }

    fn paint_lines(
        &self,
        start_y: Pixels,
        lines: &[HighlightedRangeLine],
        fill: bool,
        _bounds: Bounds<Pixels>,
        window: &mut Window,
    ) {
        if lines.is_empty() {
            return;
        }

        let first_line = lines.first().unwrap();
        let last_line = lines.last().unwrap();

        let first_top_left = point(first_line.start_x, start_y);
        let first_top_right = point(first_line.end_x, start_y);

        let curve_height = point(Pixels::ZERO, self.corner_radius);
        let curve_width = |start_x: Pixels, end_x: Pixels| {
            let max = (end_x - start_x) / 2.;
            let width = if max < self.corner_radius {
                max
            } else {
                self.corner_radius
            };

            point(width, Pixels::ZERO)
        };

        let top_curve_width = curve_width(first_line.start_x, first_line.end_x);
        let mut builder = if fill {
            gpui::PathBuilder::fill()
        } else {
            gpui::PathBuilder::stroke(px(1.))
        };
        builder.move_to(first_top_right - top_curve_width);
        builder.curve_to(first_top_right + curve_height, first_top_right);

        let mut iter = lines.iter().enumerate().peekable();
        while let Some((ix, line)) = iter.next() {
            let bottom_right = point(line.end_x, start_y + (ix + 1) as f32 * self.line_height);

            if let Some((_, next_line)) = iter.peek() {
                let next_top_right = point(next_line.end_x, bottom_right.y);

                match next_top_right.x.partial_cmp(&bottom_right.x).unwrap() {
                    Ordering::Equal => {
                        builder.line_to(bottom_right);
                    }
                    Ordering::Less => {
                        let curve_width = curve_width(next_top_right.x, bottom_right.x);
                        builder.line_to(bottom_right - curve_height);
                        if self.corner_radius > Pixels::ZERO {
                            builder.curve_to(bottom_right - curve_width, bottom_right);
                        }
                        builder.line_to(next_top_right + curve_width);
                        if self.corner_radius > Pixels::ZERO {
                            builder.curve_to(next_top_right + curve_height, next_top_right);
                        }
                    }
                    Ordering::Greater => {
                        let curve_width = curve_width(bottom_right.x, next_top_right.x);
                        builder.line_to(bottom_right - curve_height);
                        if self.corner_radius > Pixels::ZERO {
                            builder.curve_to(bottom_right + curve_width, bottom_right);
                        }
                        builder.line_to(next_top_right - curve_width);
                        if self.corner_radius > Pixels::ZERO {
                            builder.curve_to(next_top_right + curve_height, next_top_right);
                        }
                    }
                }
            } else {
                let curve_width = curve_width(line.start_x, line.end_x);
                builder.line_to(bottom_right - curve_height);
                if self.corner_radius > Pixels::ZERO {
                    builder.curve_to(bottom_right - curve_width, bottom_right);
                }

                let bottom_left = point(line.start_x, bottom_right.y);
                builder.line_to(bottom_left + curve_width);
                if self.corner_radius > Pixels::ZERO {
                    builder.curve_to(bottom_left - curve_height, bottom_left);
                }
            }
        }

        if first_line.start_x > last_line.start_x {
            let curve_width = curve_width(last_line.start_x, first_line.start_x);
            let second_top_left = point(last_line.start_x, start_y + self.line_height);
            builder.line_to(second_top_left + curve_height);
            if self.corner_radius > Pixels::ZERO {
                builder.curve_to(second_top_left + curve_width, second_top_left);
            }
            let first_bottom_left = point(first_line.start_x, second_top_left.y);
            builder.line_to(first_bottom_left - curve_width);
            if self.corner_radius > Pixels::ZERO {
                builder.curve_to(first_bottom_left - curve_height, first_bottom_left);
            }
        }

        builder.line_to(first_top_left + curve_height);
        if self.corner_radius > Pixels::ZERO {
            builder.curve_to(first_top_left + top_curve_width, first_top_left);
        }
        builder.line_to(first_top_right - top_curve_width);

        if let Ok(path) = builder.build() {
            window.paint_path(path, self.color);
        }
    }
}

fn to_highlighted_range_lines(
    range: &RangeInclusive<GridPoint>,
    layout: &LayoutState,
    origin: Point<Pixels>,
) -> Option<(Pixels, Vec<HighlightedRangeLine>)> {
    // Step 1. Normalize the points to be viewport relative.
    // When display_offset = 1, here's how the grid is arranged:
    //-2,0 -2,1...
    //--- Viewport top
    //-1,0 -1,1...
    //--------- Terminal Top
    // 0,0  0,1...
    // 1,0  1,1...
    //--- Viewport Bottom
    // 2,0  2,1...
    //--------- Terminal Bottom

    // Normalize to viewport relative, from terminal relative.
    // lines are i32s, which are negative above the top left corner of the terminal
    // If the user has scrolled, we use the display_offset to tell us which offset
    // of the grid data we should be looking at. But for the rendering step, we don't
    // want negatives. We want things relative to the 'viewport' (the area of the grid
    // which is currently shown according to the display offset)
    let unclamped_start = GridPoint::new(
        range.start().line + layout.display_offset as i32,
        range.start().column,
    );
    let unclamped_end = GridPoint::new(
        range.end().line + layout.display_offset as i32,
        range.end().column,
    );

    // Step 2. Clamp range to viewport, and return None if it doesn't overlap
    if unclamped_end.line < 0 || unclamped_start.line > layout.dimensions.num_lines() as i32 {
        return None;
    }

    let clamped_start_line = unclamped_start.line.max(0) as usize;
    let clamped_end_line = unclamped_end.line.min(layout.dimensions.num_lines() as i32) as usize;
    // Convert the start of the range to pixels
    let start_y = origin.y + clamped_start_line as f32 * layout.dimensions.line_height;

    // Step 3. Expand ranges that cross lines into a collection of single-line ranges.
    //  (also convert to pixels)
    let mut highlighted_range_lines = Vec::new();
    for line in clamped_start_line..=clamped_end_line {
        let mut line_start = 0usize;
        let mut line_end = layout.dimensions.num_columns();

        if line == clamped_start_line {
            line_start = unclamped_start.column;
        }
        if line == clamped_end_line {
            line_end = unclamped_end.column + 1; // +1 for inclusive
        }

        highlighted_range_lines.push(HighlightedRangeLine {
            start_x: origin.x + line_start as f32 * layout.dimensions.cell_width,
            end_x: origin.x + line_end as f32 * layout.dimensions.cell_width,
        });
    }

    Some((start_y, highlighted_range_lines))
}

#[derive(Clone, Debug)]
pub(crate) struct BgQuad {
    pub point: GridPoint,
    pub cells: usize,
    pub color: Hsla,
}

impl BgQuad {
    pub(crate) fn paint(
        &self,
        origin: Point<Pixels>,
        cell_width: Pixels,
        line_height: Pixels,
        window: &mut Window,
    ) {
        let pos = point(
            (origin.x + self.point.column as f32 * cell_width).floor(),
            origin.y + self.point.line as f32 * line_height,
        );
        let size = point((cell_width * self.cells as f32).ceil(), line_height).into();
        window.paint_quad(fill(gpui::Bounds::new(pos, size), self.color));
    }
}

#[derive(Debug)]
pub(crate) struct TextSpan {
    pub start: GridPoint,
    pub text: SharedString,
    pub style: TextRun,
    pub font_size: AbsoluteLength,
}

impl TextSpan {
    pub(crate) fn paint(
        &self,
        origin: Point<Pixels>,
        cell_width: Pixels,
        line_height: Pixels,
        window: &mut Window,
        cx: &mut App,
    ) {
        let pos = Point::new(
            origin.x + self.start.column as f32 * cell_width,
            origin.y + self.start.line as f32 * line_height,
        );

        let _ = window
            .text_system()
            .shape_line(
                self.text.clone(),
                self.font_size.to_pixels(window.rem_size()),
                std::slice::from_ref(&self.style),
                Some(cell_width),
            )
            .paint(pos, line_height, TextAlign::Left, None, window, cx);
    }
}

/// Mutable builder used only during `build_plan`.
///
/// We keep this separate from `TextSpan` so the final paint plan can store a `SharedString` (cheap
/// clone on paint) while still allowing incremental construction without reallocating shared
/// buffers.
#[derive(Debug)]
struct TextSpanBuilder {
    start: GridPoint,
    text: String,
    cells: usize,
    style: TextRun,
    font_size: AbsoluteLength,
}

impl TextSpanBuilder {
    fn new(start: GridPoint, ch: char, style: TextRun, font_size: AbsoluteLength) -> Self {
        let mut text = String::with_capacity(64);
        text.push(ch);
        Self {
            start,
            text,
            cells: 1,
            style,
            font_size,
        }
    }

    fn compatible_with(&self, style: &TextRun, at: GridPoint) -> bool {
        self.start.line == at.line
            && self.start.column + self.cells == at.column
            && self.style.font == style.font
            && self.style.color == style.color
            && self.style.background_color == style.background_color
            && self.style.underline == style.underline
            && self.style.strikethrough == style.strikethrough
    }

    fn push_char(&mut self, ch: char) {
        self.text.push(ch);
        self.style.len += ch.len_utf8();
        self.cells += 1;
    }

    fn push_zerowidth(&mut self, chars: &[char]) {
        for &ch in chars {
            self.text.push(ch);
            self.style.len += ch.len_utf8();
        }
    }

    fn finish(self) -> TextSpan {
        TextSpan {
            start: self.start,
            text: self.text.into(),
            style: self.style,
            font_size: self.font_size,
        }
    }
}

pub(crate) fn build_plan(
    grid: &[IndexedCell],
    start_line_offset: i32,
    theme: &Theme,
    base: &TextStyle,
    hyperlink: Option<(gpui::HighlightStyle, &RangeInclusive<GridPoint>)>,
    minimum_contrast: f32,
) -> (Vec<BgQuad>, Vec<TextSpan>) {
    // Heuristic capacities to reduce allocations in the hot paint path.
    let mut bg_quads: Vec<BgQuad> = Vec::with_capacity(grid.len() / 4);
    let mut spans: Vec<TextSpan> = Vec::with_capacity(grid.len() / 8);

    let mut active_bg: Option<(i32, usize, Hsla, usize)> = None; // (line, start_col, color, cells)
    let mut active_span: Option<TextSpanBuilder> = None;

    let mut last_line: Option<i32> = None;
    let mut prev_cell_had_zw = false;

    for cell in grid {
        let line = start_line_offset + cell.point.line;
        let col = cell.point.column;

        if last_line != Some(line) {
            debug_assert!(
                last_line.is_none() || last_line <= Some(line),
                "expected grid cells to be in display order (non-decreasing line numbers)"
            );
            flush_bg(&mut active_bg, &mut bg_quads);
            flush_span(&mut active_span, &mut spans);
            prev_cell_had_zw = false;
            last_line = Some(line);
        }

        let mut fg = cell.fg;
        let mut bg = cell.bg;
        if cell.flags.contains(CellFlags::INVERSE) {
            std::mem::swap(&mut fg, &mut bg);
        }

        // Background quads: single-line runs only (cheap and predictable).
        if bg != TermColor::Named(NamedColor::Background) {
            let color = convert_color(&bg, theme);
            match active_bg.as_mut() {
                Some((bg_line, _, bg_color, bg_cells))
                    if *bg_line == line && *bg_color == color =>
                {
                    *bg_cells += 1;
                }
                Some(_) => {
                    flush_bg(&mut active_bg, &mut bg_quads);
                    active_bg = Some((line, col, color, 1));
                }
                None => active_bg = Some((line, col, color, 1)),
            }
        } else {
            flush_bg(&mut active_bg, &mut bg_quads);
        }

        // Skip spacer cells for wide characters.
        if cell.flags.contains(CellFlags::WIDE_CHAR_SPACER) {
            continue;
        }

        // Skip the "extra trailing space" pattern after emoji sequences.
        if cell.c == ' ' && prev_cell_had_zw {
            prev_cell_had_zw = false;
            continue;
        }
        prev_cell_had_zw = matches!(cell.zerowidth(), Some(chars) if !chars.is_empty());

        if cell_is_trivial(cell) {
            continue;
        }

        let style = cell_style(cell, fg, bg, theme, base, hyperlink, minimum_contrast);
        let at = GridPoint::new(line, col);
        let zw = cell.zerowidth();

        match active_span.as_mut() {
            Some(span) if span.compatible_with(&style, at) => {
                span.push_char(cell.c);
                if let Some(chars) = zw {
                    span.push_zerowidth(chars);
                }
            }
            Some(_) => {
                flush_span(&mut active_span, &mut spans);
                let mut span = TextSpanBuilder::new(at, cell.c, style, base.font_size);
                if let Some(chars) = zw {
                    span.push_zerowidth(chars);
                }
                active_span = Some(span);
            }
            None => {
                let mut span = TextSpanBuilder::new(at, cell.c, style, base.font_size);
                if let Some(chars) = zw {
                    span.push_zerowidth(chars);
                }
                active_span = Some(span);
            }
        }
    }

    flush_bg(&mut active_bg, &mut bg_quads);
    flush_span(&mut active_span, &mut spans);

    (bg_quads, spans)
}

fn flush_bg(active: &mut Option<(i32, usize, Hsla, usize)>, out: &mut Vec<BgQuad>) {
    let Some((line, start_col, color, cells)) = active.take() else {
        return;
    };
    out.push(BgQuad {
        point: GridPoint::new(line, start_col),
        cells,
        color,
    });
}

fn flush_span(active: &mut Option<TextSpanBuilder>, out: &mut Vec<TextSpan>) {
    if let Some(span) = active.take() {
        out.push(span.finish());
    }
}

fn cell_is_trivial(cell: &IndexedCell) -> bool {
    if cell.c != ' ' {
        return false;
    }
    if cell.bg != TermColor::Named(NamedColor::Background) {
        return false;
    }
    if cell.hyperlink().is_some() {
        return false;
    }
    !cell.flags.intersects(
        CellFlags::INVERSE
            | CellFlags::UNDERLINE
            | CellFlags::DOUBLE_UNDERLINE
            | CellFlags::CURLY_UNDERLINE
            | CellFlags::DOTTED_UNDERLINE
            | CellFlags::DASHED_UNDERLINE
            | CellFlags::STRIKEOUT,
    )
}

fn preserve_exact_colors(ch: char) -> bool {
    let c = ch as u32;
    matches!(c, 0x2500..=0x259F | 0x25A0..=0x25FF | 0xE0B0..=0xE0D4)
}

fn cell_style(
    cell: &IndexedCell,
    fg: TermColor,
    bg: TermColor,
    theme: &Theme,
    base: &TextStyle,
    hyperlink: Option<(gpui::HighlightStyle, &RangeInclusive<GridPoint>)>,
    minimum_contrast: f32,
) -> TextRun {
    let flags = cell.cell.flags;

    let mut fg = convert_color(&fg, theme);
    let bg = convert_color(&bg, theme);
    if !preserve_exact_colors(cell.c) {
        fg = ensure_minimum_contrast(fg, bg, minimum_contrast);
    }
    if flags.intersects(CellFlags::DIM) {
        fg.a *= 0.7;
    }

    let underline = (flags.intersects(
        CellFlags::UNDERLINE
            | CellFlags::DOUBLE_UNDERLINE
            | CellFlags::CURLY_UNDERLINE
            | CellFlags::DOTTED_UNDERLINE
            | CellFlags::DASHED_UNDERLINE,
    ) || cell.cell.hyperlink().is_some())
    .then(|| UnderlineStyle {
        color: Some(fg),
        thickness: px(1.0),
        wavy: flags.contains(CellFlags::CURLY_UNDERLINE),
    });
    let strikethrough = flags
        .intersects(CellFlags::STRIKEOUT)
        .then(|| StrikethroughStyle {
            color: Some(fg),
            thickness: px(1.0),
        });

    let weight = if flags.intersects(CellFlags::BOLD) {
        gpui::FontWeight::BOLD
    } else {
        base.font_weight
    };
    let font_style = if flags.intersects(CellFlags::ITALIC) {
        gpui::FontStyle::Italic
    } else {
        gpui::FontStyle::Normal
    };

    let mut run = TextRun {
        len: cell.c.len_utf8(),
        color: fg,
        background_color: None,
        font: gpui::Font {
            weight,
            style: font_style,
            ..base.font()
        },
        underline,
        strikethrough,
    };

    if let Some((style, range)) = hyperlink
        && range.contains(&cell.point)
    {
        if let Some(underline) = style.underline {
            run.underline = Some(underline);
        }
        if let Some(color) = style.color {
            run.color = color;
        }
    }

    run
}

#[allow(clippy::too_many_arguments)]
fn paint_suggestions_overlay(
    cursor_bounds: Bounds<Pixels>,
    terminal_view_bounds: Bounds<Pixels>,
    dimensions: TerminalBounds,
    items: &[crate::suggestions::SuggestionItem],
    selected: Option<usize>,
    base_text_style: &TextStyle,
    window: &mut Window,
    cx: &mut App,
) {
    const DESC_LINE_HEIGHT_FACTOR: f32 = 0.50;
    const DESC_FONT_SIZE_FACTOR: f32 = 0.70;

    let settings = TerminalSettings::global(cx);
    let max_items = settings.suggestions_max_items.max(1);
    let Some(layout) = compute_suggestions_overlay_layout(
        cursor_bounds,
        terminal_view_bounds,
        dimensions,
        items,
        max_items,
    ) else {
        return;
    };
    let items = &items[..layout.items_len];

    let (panel_bg, panel_border, panel_fg, selected_bg) = {
        let theme = cx.theme();
        (
            theme.popover,
            theme.border.opacity(0.9),
            theme.popover_foreground,
            theme.selection.opacity(0.22),
        )
    };
    let cell_width = dimensions.cell_width;
    let pad_x = layout.pad_x;
    let pad_y = layout.pad_y;
    let panel_x = layout.panel_bounds.origin.x;
    let panel_y = layout.panel_bounds.origin.y;
    let panel_w = layout.panel_bounds.size.width;

    paint_suggestions_overlay_panel(&layout, panel_bg, panel_border, window);

    let text_style = {
        let mut s = base_text_style.clone();
        s.color = panel_fg;
        s
    };
    let font_size = text_style.font_size.to_pixels(window.rem_size());
    let desc_font_size = font_size * DESC_FONT_SIZE_FACTOR;
    let desc_fg = panel_fg.opacity(0.68);
    let label_line_height = layout.label_line_height;
    let desc_line_height = label_line_height * DESC_LINE_HEIGHT_FACTOR;

    // Selection highlight.
    paint_suggestions_overlay_selection(
        &layout,
        panel_x,
        panel_y,
        panel_w,
        pad_y,
        selected,
        items.len(),
        selected_bg,
        window,
    );

    // Text rows.
    let max_text_w = (panel_w - pad_x * 2.0).max(Pixels::ZERO);
    let max_chars = ((max_text_w / cell_width).floor() as usize).saturating_sub(1);

    let placeholder_bg = placeholder_highlight_bgs(cx.theme()).1;
    let ctx = SuggestionsOverlayRowPaintCtx {
        layout: &layout,
        panel_x,
        panel_y,
        pad_x,
        pad_y,
        cell_width,
        label_line_height,
        desc_line_height,
        max_text_w,
        max_chars,
        placeholder_bg,
        text_style: &text_style,
        font_size,
        desc_font_size,
        desc_fg,
    };
    for (idx, item) in items.iter().enumerate() {
        paint_suggestions_overlay_row(idx, item, &ctx, window, cx);
    }
}

fn paint_suggestions_overlay_panel(
    layout: &SuggestionsOverlayLayout,
    panel_bg: Hsla,
    panel_border: Hsla,
    window: &mut Window,
) {
    window.paint_quad(fill(layout.panel_bounds, panel_bg));
    window.paint_quad(outline(
        layout.panel_bounds,
        panel_border,
        BorderStyle::Solid,
    ));
}

#[allow(clippy::too_many_arguments)]
fn paint_suggestions_overlay_selection(
    layout: &SuggestionsOverlayLayout,
    panel_x: Pixels,
    panel_y: Pixels,
    panel_w: Pixels,
    pad_y: Pixels,
    selected: Option<usize>,
    items_len: usize,
    selected_bg: Hsla,
    window: &mut Window,
) {
    let Some(selected) = selected else {
        return;
    };
    if items_len == 0 {
        return;
    }

    let selected = selected.min(items_len.saturating_sub(1));
    let row_y = panel_y + pad_y + layout.row_offsets[selected];
    let row_h = layout.row_heights[selected];
    let selected_row_bounds = Bounds {
        origin: point(panel_x, row_y),
        size: size(panel_w, row_h),
    };
    window.paint_quad(fill(selected_row_bounds, selected_bg));
}

struct SuggestionsOverlayRowPaintCtx<'a> {
    layout: &'a SuggestionsOverlayLayout,
    panel_x: Pixels,
    panel_y: Pixels,
    pad_x: Pixels,
    pad_y: Pixels,
    cell_width: Pixels,
    label_line_height: Pixels,
    desc_line_height: Pixels,
    max_text_w: Pixels,
    max_chars: usize,
    placeholder_bg: Hsla,
    text_style: &'a TextStyle,
    font_size: Pixels,
    desc_font_size: Pixels,
    desc_fg: Hsla,
}

fn suggestion_label_and_placeholders(
    item: &crate::suggestions::SuggestionItem,
    max_chars: usize,
) -> (String, Vec<std::ops::Range<usize>>) {
    let (mut label, mut placeholder_ranges_chars) = parse_snippet_suffix(&item.full_text)
        .map(|snippet| {
            let ranges = snippet
                .tabstops
                .iter()
                .filter(|t| t.index != 0 && t.range_chars.start != t.range_chars.end)
                .map(|t| t.range_chars.clone())
                .collect::<Vec<_>>();
            (snippet.rendered, ranges)
        })
        .unwrap_or_else(|| (item.full_text.clone(), Vec::new()));

    if max_chars > 0 && label.chars().count() > max_chars {
        label = label.chars().take(max_chars).collect::<String>();
        let max_chars = label.chars().count();
        for r in &mut placeholder_ranges_chars {
            r.start = r.start.min(max_chars);
            r.end = r.end.min(max_chars);
        }
        placeholder_ranges_chars.retain(|r| r.start < r.end);
    }

    placeholder_ranges_chars.sort_by(|a, b| a.start.cmp(&b.start).then_with(|| a.end.cmp(&b.end)));
    placeholder_ranges_chars.dedup();
    (label, placeholder_ranges_chars)
}

fn paint_suggestions_overlay_row(
    idx: usize,
    item: &crate::suggestions::SuggestionItem,
    ctx: &SuggestionsOverlayRowPaintCtx,
    window: &mut Window,
    cx: &mut App,
) {
    let (label, placeholder_ranges_chars) = suggestion_label_and_placeholders(item, ctx.max_chars);

    let runs = vec![TextRun {
        len: label.len(),
        font: ctx.text_style.font(),
        color: ctx.text_style.color,
        background_color: None,
        underline: Default::default(),
        strikethrough: None,
    }];

    let shaped = window
        .text_system()
        .shape_line(label.clone().into(), ctx.font_size, &runs, None);

    let row_y = ctx.panel_y + ctx.pad_y + ctx.layout.row_offsets[idx];
    let pos = point(ctx.panel_x + ctx.pad_x, row_y);

    if !placeholder_ranges_chars.is_empty() {
        // Dropdown-only: placeholder background highlight. We paint explicit quads instead of
        // relying on text run background rendering.
        for range in placeholder_ranges_chars {
            let start_col = range.start;
            let end_col = range.end;
            if end_col <= start_col {
                continue;
            }

            let origin = point(pos.x + ctx.cell_width * (start_col as f32), pos.y);
            let size = size(
                ctx.cell_width * ((end_col - start_col) as f32),
                ctx.label_line_height,
            );
            window.paint_quad(fill(Bounds { origin, size }, ctx.placeholder_bg));
        }
    }

    let _ = shaped.paint(
        pos,
        ctx.label_line_height,
        TextAlign::Left,
        None,
        window,
        cx,
    );

    let Some(desc) = item.description.as_deref() else {
        return;
    };
    let desc = desc.trim();
    if desc.is_empty() {
        return;
    }

    let mut desc_label = desc.to_string();
    if ctx.max_chars > 0 && desc_label.chars().count() > ctx.max_chars {
        desc_label = desc_label.chars().take(ctx.max_chars).collect::<String>();
    }

    let desc_runs = vec![TextRun {
        len: desc_label.len(),
        font: ctx.text_style.font(),
        color: ctx.desc_fg,
        background_color: None,
        underline: Default::default(),
        strikethrough: None,
    }];

    let shaped_desc =
        window
            .text_system()
            .shape_line(desc_label.into(), ctx.desc_font_size, &desc_runs, None);

    let desc_pos = point(pos.x, row_y + ctx.label_line_height);
    let _ = shaped_desc.paint(
        desc_pos,
        ctx.desc_line_height,
        TextAlign::Right,
        Some(ctx.max_text_w),
        window,
        cx,
    );
}

#[derive(Clone, Debug)]
struct SuggestionsOverlayLayout {
    panel_bounds: Bounds<Pixels>,
    pad_x: Pixels,
    pad_y: Pixels,
    label_line_height: Pixels,
    row_offsets: Vec<Pixels>,
    row_heights: Vec<Pixels>,
    items_len: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SuggestionsOverlayHitTest {
    Outside,
    Panel,
    Row(usize),
}

impl SuggestionsOverlayLayout {
    fn row_at(&self, position: Point<Pixels>) -> Option<usize> {
        if !self.panel_bounds.contains(&position) {
            return None;
        }

        let inner_y = position.y - (self.panel_bounds.origin.y + self.pad_y);
        if inner_y < Pixels::ZERO {
            return None;
        }

        for (idx, (&row_y, &row_h)) in self
            .row_offsets
            .iter()
            .zip(self.row_heights.iter())
            .enumerate()
        {
            if inner_y >= row_y && inner_y < row_y + row_h {
                return Some(idx);
            }
        }

        None
    }

    fn hit_test(&self, position: Point<Pixels>) -> SuggestionsOverlayHitTest {
        if !self.panel_bounds.contains(&position) {
            return SuggestionsOverlayHitTest::Outside;
        }

        if let Some(row) = self.row_at(position) {
            return SuggestionsOverlayHitTest::Row(row);
        }

        SuggestionsOverlayHitTest::Panel
    }
}

fn compute_suggestions_overlay_layout(
    cursor_bounds: Bounds<Pixels>,
    terminal_view_bounds: Bounds<Pixels>,
    dimensions: TerminalBounds,
    items: &[crate::suggestions::SuggestionItem],
    max_items: usize,
) -> Option<SuggestionsOverlayLayout> {
    const DESC_LINE_HEIGHT_FACTOR: f32 = 0.78;

    let items_len = items.len().min(max_items);
    if items_len == 0 {
        return None;
    }

    let items = &items[..items_len];

    let label_line_height = dimensions.line_height;
    let desc_line_height = label_line_height * DESC_LINE_HEIGHT_FACTOR;
    let cell_width = dimensions.cell_width;

    let pad_x = px(10.0);
    let pad_y = px(8.0);

    let mut row_offsets = Vec::<Pixels>::with_capacity(items.len());
    let mut row_heights = Vec::<Pixels>::with_capacity(items.len());
    let mut list_h = Pixels::ZERO;

    for item in items.iter() {
        row_offsets.push(list_h);
        let has_desc = item
            .description
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty());
        let row_h = if has_desc {
            label_line_height + desc_line_height
        } else {
            label_line_height
        };
        row_heights.push(row_h);
        list_h += row_h;
    }

    let panel_h = pad_y * 2.0 + list_h;

    let mut panel_x = cursor_bounds.origin.x;
    let mut panel_y = cursor_bounds.origin.y + label_line_height;

    let bounds_right = terminal_view_bounds.origin.x + terminal_view_bounds.size.width;
    let bounds_bottom = terminal_view_bounds.origin.y + terminal_view_bounds.size.height;

    // Compute a reasonable width based on visible content, clamped into a sane range.
    let max_label_chars = items
        .iter()
        .map(|item| item.full_text.chars().count())
        .max()
        .unwrap_or(1);
    let min_cols = 24usize.min(dimensions.num_columns().max(1)).max(10);
    let max_cols = 60usize.min(dimensions.num_columns().max(1)).max(min_cols);
    let desired_cols = (max_label_chars + 2).clamp(min_cols, max_cols);
    let mut panel_w = cell_width * (desired_cols as f32) + pad_x * 2.0;

    // Flip above the cursor if we'd go past the bottom.
    if panel_y + panel_h > bounds_bottom {
        panel_y = cursor_bounds.origin.y - panel_h;
    }

    // Clamp vertically into the terminal view bounds.
    let min_y = terminal_view_bounds.origin.y;
    let max_y = bounds_bottom - panel_h;
    panel_y = if max_y < min_y {
        min_y
    } else {
        panel_y.clamp(min_y, max_y)
    };

    // Clamp horizontally into the terminal view bounds.
    if panel_x + panel_w > bounds_right {
        panel_x = (bounds_right - panel_w).max(terminal_view_bounds.origin.x);
    }
    if panel_x < terminal_view_bounds.origin.x {
        panel_x = terminal_view_bounds.origin.x;
    }
    if panel_x + panel_w > bounds_right {
        panel_w = (bounds_right - panel_x).max(cell_width * 10.0);
    }

    let panel_bounds = Bounds {
        origin: point(panel_x, panel_y),
        size: size(panel_w, panel_h),
    };

    Some(SuggestionsOverlayLayout {
        panel_bounds,
        pad_x,
        pad_y,
        label_line_height,
        row_offsets,
        row_heights,
        items_len,
    })
}

fn suggestions_overlay_layout(
    terminal: &Entity<Terminal>,
    terminal_view: &Entity<TerminalView>,
    cx: &mut App,
) -> Option<SuggestionsOverlayLayout> {
    let (items, _highlighted) = terminal_view.read(cx).suggestions_snapshot()?;
    if items.is_empty() {
        return None;
    }

    let settings = TerminalSettings::global(cx);
    let max_items = settings.suggestions_max_items.max(1);

    let (content, total_lines_for_digits) = {
        let terminal = terminal.read(cx);
        (terminal.last_content().clone(), terminal.total_lines())
    };

    let cursor_bounds = cursor_bounds_for_suggestions_overlay(&content)?;
    let terminal_view_bounds =
        terminal_view_bounds_for_suggestions_overlay(&content, total_lines_for_digits, cx);

    compute_suggestions_overlay_layout(
        cursor_bounds,
        terminal_view_bounds,
        content.terminal_bounds,
        &items,
        max_items,
    )
}

fn suggestions_overlay_row_at_position(
    terminal: &Entity<Terminal>,
    terminal_view: &Entity<TerminalView>,
    position: Point<Pixels>,
    cx: &mut App,
) -> Option<usize> {
    suggestions_overlay_layout(terminal, terminal_view, cx)?.row_at(position)
}

fn cursor_bounds_for_suggestions_overlay(
    content: &crate::TerminalContent,
) -> Option<Bounds<Pixels>> {
    let dimensions = content.terminal_bounds;
    if dimensions.num_lines() == 0 || dimensions.num_columns() == 0 {
        return None;
    }

    let viewport = crate::point_to_viewport(content.display_offset, content.cursor.point)?;
    let line = viewport.line.min(dimensions.num_lines().saturating_sub(1));
    let column = viewport.column.min(dimensions.last_column());

    Some(Bounds {
        origin: point(
            dimensions.bounds.origin.x + dimensions.cell_width * (column as f32),
            dimensions.bounds.origin.y + dimensions.line_height * (line as f32),
        ),
        size: size(dimensions.cell_width, dimensions.line_height),
    })
}

fn terminal_view_bounds_for_suggestions_overlay(
    content: &crate::TerminalContent,
    total_lines_for_digits: usize,
    cx: &App,
) -> Bounds<Pixels> {
    let terminal_settings = TerminalSettings::global(cx);
    let show_line_numbers_setting = terminal_settings.show_line_numbers;
    let show_line_numbers = should_show_line_numbers(show_line_numbers_setting, content.mode);
    let reserve_left_padding_without_line_numbers_for_layout =
        reserve_left_padding_without_line_numbers(show_line_numbers_setting, content.mode);
    let line_number_state = compute_line_number_layout(
        content.terminal_bounds.cell_width,
        show_line_numbers,
        reserve_left_padding_without_line_numbers_for_layout,
        total_lines_for_digits,
    );

    let term_bounds = content.terminal_bounds.bounds;
    Bounds {
        origin: point(
            term_bounds.origin.x - line_number_state.gutter,
            term_bounds.origin.y,
        ),
        size: size(
            term_bounds.size.width + line_number_state.gutter,
            term_bounds.size.height,
        ),
    }
}

#[cfg(test)]
mod suggestions_overlay_desc_tests {
    use gpui::{Bounds, point, px, size};

    use super::{SuggestionsOverlayHitTest, compute_suggestions_overlay_layout};
    use crate::terminal::TerminalBounds;

    #[test]
    fn layout_does_not_reserve_desc_row_when_empty() {
        let dimensions = TerminalBounds::new(
            px(10.0),
            px(5.0),
            Bounds {
                origin: point(px(0.0), px(0.0)),
                size: size(px(800.0), px(600.0)),
            },
        );

        let cursor_bounds = Bounds {
            origin: point(px(10.0), px(10.0)),
            size: size(px(5.0), px(10.0)),
        };
        let terminal_view_bounds = Bounds {
            origin: point(px(0.0), px(0.0)),
            size: size(px(800.0), px(600.0)),
        };

        let items = vec![
            crate::suggestions::SuggestionItem {
                full_text: "ls -al".to_string(),
                score: 0,
                description: None,
            },
            crate::suggestions::SuggestionItem {
                full_text: "ls -lah".to_string(),
                score: 0,
                description: None,
            },
        ];

        let layout = compute_suggestions_overlay_layout(
            cursor_bounds,
            terminal_view_bounds,
            dimensions,
            &items,
            10,
        )
        .expect("layout");

        let pad_y = px(8.0);
        let expected = pad_y * 2.0 + dimensions.line_height * (items.len() as f32);
        assert_eq!(layout.panel_bounds.size.height, expected);
    }

    #[test]
    fn layout_does_not_panic_when_panel_taller_than_view() {
        let dimensions = TerminalBounds::new(
            px(10.0),
            px(12.0),
            Bounds {
                origin: point(px(0.0), px(0.0)),
                size: size(px(800.0), px(600.0)),
            },
        );

        let cursor_bounds = Bounds {
            origin: point(px(110.0), px(105.0)),
            size: size(px(10.0), px(12.0)),
        };
        let terminal_view_bounds = Bounds {
            origin: point(px(100.0), px(100.0)),
            // Too short to fit the full panel; previously this panicked in `clamp`.
            size: size(px(200.0), px(10.0)),
        };

        let items = vec![
            crate::suggestions::SuggestionItem {
                full_text: "ls -al".to_string(),
                score: 0,
                description: None,
            },
            crate::suggestions::SuggestionItem {
                full_text: "ls -lah".to_string(),
                score: 0,
                description: None,
            },
        ];

        let layout = compute_suggestions_overlay_layout(
            cursor_bounds,
            terminal_view_bounds,
            dimensions,
            &items,
            10,
        )
        .expect("layout");

        assert_eq!(layout.panel_bounds.origin.y, terminal_view_bounds.origin.y);
    }

    #[test]
    fn hit_test_distinguishes_row_panel_and_outside() {
        let dimensions = TerminalBounds::new(
            px(10.0),
            px(5.0),
            Bounds {
                origin: point(px(0.0), px(0.0)),
                size: size(px(800.0), px(600.0)),
            },
        );

        let cursor_bounds = Bounds {
            origin: point(px(10.0), px(10.0)),
            size: size(px(5.0), px(10.0)),
        };
        let terminal_view_bounds = Bounds {
            origin: point(px(0.0), px(0.0)),
            size: size(px(800.0), px(600.0)),
        };

        let items = vec![
            crate::suggestions::SuggestionItem {
                full_text: "ls -al".to_string(),
                score: 0,
                description: None,
            },
            crate::suggestions::SuggestionItem {
                full_text: "ls -lah".to_string(),
                score: 0,
                description: None,
            },
        ];

        let layout = compute_suggestions_overlay_layout(
            cursor_bounds,
            terminal_view_bounds,
            dimensions,
            &items,
            10,
        )
        .expect("layout");

        let row_point = point(
            layout.panel_bounds.origin.x + layout.pad_x + px(1.0),
            layout.panel_bounds.origin.y + layout.pad_y + layout.row_offsets[0] + px(1.0),
        );
        assert_eq!(
            layout.hit_test(row_point),
            SuggestionsOverlayHitTest::Row(0)
        );

        let panel_padding_point = point(
            layout.panel_bounds.origin.x + px(1.0),
            layout.panel_bounds.origin.y + px(1.0),
        );
        assert_eq!(
            layout.hit_test(panel_padding_point),
            SuggestionsOverlayHitTest::Panel
        );

        let outside_point = point(
            layout.panel_bounds.origin.x - px(1.0),
            layout.panel_bounds.origin.y,
        );
        assert_eq!(
            layout.hit_test(outside_point),
            SuggestionsOverlayHitTest::Outside
        );
    }

    #[test]
    fn layout_only_adds_desc_row_when_present() {
        let dimensions = TerminalBounds::new(
            px(10.0),
            px(5.0),
            Bounds {
                origin: point(px(0.0), px(0.0)),
                size: size(px(800.0), px(600.0)),
            },
        );

        let cursor_bounds = Bounds {
            origin: point(px(10.0), px(10.0)),
            size: size(px(5.0), px(10.0)),
        };
        let terminal_view_bounds = Bounds {
            origin: point(px(0.0), px(0.0)),
            size: size(px(800.0), px(600.0)),
        };

        let items = vec![
            crate::suggestions::SuggestionItem {
                full_text: "ls --all".to_string(),
                score: 0,
                description: Some("list all".to_string()),
            },
            crate::suggestions::SuggestionItem {
                full_text: "ls -al".to_string(),
                score: 0,
                description: None,
            },
        ];

        let layout = compute_suggestions_overlay_layout(
            cursor_bounds,
            terminal_view_bounds,
            dimensions,
            &items,
            10,
        )
        .expect("layout");

        let pad_y = px(8.0);
        let desc_line_height = dimensions.line_height * 0.78;
        let expected =
            pad_y * 2.0 + (dimensions.line_height + desc_line_height) + dimensions.line_height;
        assert_eq!(layout.panel_bounds.size.height, expected);
    }
}

#[cfg(test)]
mod tests {
    use std::ops::RangeInclusive;

    use gpui::{Bounds, point, px, size};
    use gpui_component::Theme;

    use super::{
        compute_terminal_layout_metrics, highlight_quads_for_range, placeholder_highlight_bgs,
        snippet_placeholder_bg_quads,
    };
    use crate::{GridPoint, TerminalMode, view::line_number::should_relayout_for_mode_change};

    #[test]
    fn placeholder_highlight_colors_are_theme_derived() {
        let theme = Theme::default();
        let (active, inactive) = placeholder_highlight_bgs(&theme);
        assert_eq!(active, theme.selection.opacity(0.48));
        assert_eq!(inactive, theme.selection.opacity(0.33));
    }

    #[test]
    fn hides_line_numbers_adds_minimum_left_padding() {
        let bounds = Bounds {
            origin: point(px(0.0), px(0.0)),
            size: size(px(300.0), px(200.0)),
        };

        // If line numbers are hidden, we still want some breathing room from the left edge.
        // (Minimum of 14px, otherwise one-third of a cell width.)
        let (_dimensions, gutter, _line_number_width, _line_number_digits, _scrollbar_width) =
            compute_terminal_layout_metrics(bounds, px(6.0), px(12.0), false, false, true, 100);

        assert_eq!(gutter, px(14.0));
    }

    #[test]
    fn relayouts_when_exiting_alt_screen_with_hidden_line_numbers() {
        assert!(should_relayout_for_mode_change(
            false,
            TerminalMode::ALT_SCREEN,
            TerminalMode::empty()
        ));
    }

    #[test]
    fn highlight_quads_for_range_single_line() {
        let range = RangeInclusive::new(GridPoint::new(2, 4), GridPoint::new(2, 7));
        let quads = highlight_quads_for_range(&range, 10, gpui::opaque_grey(0.5, 1.0));
        assert_eq!(quads.len(), 1);
        assert_eq!(quads[0].point, GridPoint::new(2, 4));
        assert_eq!(quads[0].cells, 4);
    }

    #[test]
    fn highlight_quads_for_range_multi_line_spans_full_middle_lines() {
        let range = RangeInclusive::new(GridPoint::new(1, 3), GridPoint::new(3, 2));
        let quads = highlight_quads_for_range(&range, 8, gpui::opaque_grey(0.5, 1.0));
        assert_eq!(quads.len(), 3);
        // line 1: cols 3..=7
        assert_eq!(quads[0].point, GridPoint::new(1, 3));
        assert_eq!(quads[0].cells, 5);
        // line 2: full line 0..=7
        assert_eq!(quads[1].point, GridPoint::new(2, 0));
        assert_eq!(quads[1].cells, 8);
        // line 3: cols 0..=2
        assert_eq!(quads[2].point, GridPoint::new(3, 0));
        assert_eq!(quads[2].cells, 3);
    }

    #[test]
    fn snippet_highlights_empty_placeholder_as_single_cell() {
        let mut session = crate::snippet::SnippetSession::new(
            "".to_string(),
            vec![crate::snippet::TabStop {
                index: 1,
                range_chars: 0..0,
            }],
        );
        session.start_point = GridPoint::new(0, 0);
        session.active = 0;

        let quads = snippet_placeholder_bg_quads(
            &session,
            80,
            gpui::black().opacity(0.9),
            gpui::black().opacity(0.4),
        );
        assert_eq!(quads.len(), 1);
        assert_eq!(quads[0].point, GridPoint::new(0, 0));
        assert_eq!(quads[0].cells, 1);
    }

    #[test]
    fn snippet_highlights_ascii_char_as_one_cell() {
        let mut session = crate::snippet::SnippetSession::new(
            "s".to_string(),
            vec![crate::snippet::TabStop {
                index: 1,
                range_chars: 0..1,
            }],
        );
        session.start_point = GridPoint::new(0, 0);
        session.active = 0;

        let quads = snippet_placeholder_bg_quads(
            &session,
            80,
            gpui::black().opacity(0.9),
            gpui::black().opacity(0.4),
        );
        assert_eq!(quads.len(), 1);
        assert_eq!(quads[0].cells, 1);
    }

    #[test]
    fn snippet_highlights_wide_char_as_two_cells() {
        let mut session = crate::snippet::SnippetSession::new(
            "你".to_string(),
            vec![crate::snippet::TabStop {
                index: 1,
                range_chars: 0..1,
            }],
        );
        session.start_point = GridPoint::new(0, 0);
        session.active = 0;

        let quads = snippet_placeholder_bg_quads(
            &session,
            80,
            gpui::black().opacity(0.9),
            gpui::black().opacity(0.4),
        );
        assert_eq!(quads.len(), 1);
        assert_eq!(quads[0].cells, 2);
    }

    #[test]
    fn snippet_wrapping_placeholder_splits_into_multiple_quads() {
        let mut session = crate::snippet::SnippetSession::new(
            "abcd".to_string(),
            vec![crate::snippet::TabStop {
                index: 1,
                range_chars: 0..4,
            }],
        );
        session.start_point = GridPoint::new(0, 3);
        session.active = 0;

        let quads = snippet_placeholder_bg_quads(
            &session,
            4,
            gpui::black().opacity(0.9),
            gpui::black().opacity(0.4),
        );
        assert_eq!(quads.len(), 2);
        assert_eq!(quads[0].point, GridPoint::new(0, 3));
        assert_eq!(quads[0].cells, 1);
        assert_eq!(quads[1].point, GridPoint::new(1, 0));
        assert_eq!(quads[1].cells, 3);
    }
}
