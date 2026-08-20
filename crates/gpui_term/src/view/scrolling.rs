use std::{cell::Cell, cmp, ops::RangeInclusive, rc::Rc, time::Duration};

use gpui::{
    App, Bounds, Context, Pixels, Point, ReadGlobal, ScrollWheelEvent, Window, point, px, size,
};
use gpui_component::scroll::ScrollbarHandle;
use smol::Timer;

use super::{BlockProperties, TerminalScrollState, TerminalView};
use crate::{
    GridPoint, point_to_viewport,
    settings::TerminalSettings,
    terminal::{
        ScrollLineDown, ScrollLineUp, ScrollPageDown, ScrollPageUp, ScrollToBottom, ScrollToTop,
        ToggleViMode,
    },
};

pub(crate) const SCROLLBAR_WIDTH: Pixels = px(14.0);
pub(crate) const SCROLLBAR_MARKER_HIT_RADIUS: Pixels = px(7.0);
pub(crate) const SCROLLBAR_MARKER_LIMIT: usize = 4096;
pub(crate) const SCROLLBAR_MARKER_SIZE: Pixels = px(4.0);
pub(crate) const SCROLLBAR_ACTIVE_MARKER_SIZE: Pixels = px(6.0);

#[derive(Debug, Clone, Copy)]
struct ScrollbarHandleState {
    line_height: Pixels,
    total_lines: usize,
    viewport_lines: usize,
    display_offset: usize,
}

impl Default for ScrollbarHandleState {
    fn default() -> Self {
        Self {
            line_height: px(1.0),
            total_lines: 0,
            viewport_lines: 0,
            display_offset: 0,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct TerminalScrollbarHandle {
    state: Rc<Cell<ScrollbarHandleState>>,
    target_display_offset: Rc<Cell<Option<usize>>>,
    dragging: Rc<Cell<bool>>,
}

impl TerminalScrollbarHandle {
    pub(crate) fn update(
        &self,
        line_height: Pixels,
        total_lines: usize,
        viewport_lines: usize,
        display_offset: usize,
    ) {
        self.state.set(ScrollbarHandleState {
            line_height: line_height.max(px(1.0)),
            total_lines,
            viewport_lines,
            display_offset,
        });
    }

    pub(crate) fn take_target_display_offset(&self) -> Option<usize> {
        self.target_display_offset.take()
    }

    pub(crate) fn is_dragging(&self) -> bool {
        self.dragging.get()
    }
}

impl ScrollbarHandle for TerminalScrollbarHandle {
    fn viewport_bounds(&self) -> Bounds<Pixels> {
        let state = self.state.get();
        Bounds {
            origin: Point::default(),
            size: size(
                Pixels::ZERO,
                state.viewport_lines as f32 * state.line_height,
            ),
        }
    }

    fn offset(&self) -> Point<Pixels> {
        let state = self.state.get();
        let max_offset = state.total_lines.saturating_sub(state.viewport_lines);
        let display_offset = state.display_offset.min(max_offset);
        let scroll_offset_from_top = max_offset.saturating_sub(display_offset);

        point(
            Pixels::ZERO,
            -(scroll_offset_from_top as f32 * state.line_height),
        )
    }

    fn set_offset(&self, offset: Point<Pixels>) {
        let mut state = self.state.get();
        let max_offset = state.total_lines.saturating_sub(state.viewport_lines);
        if max_offset == 0 {
            state.display_offset = 0;
            self.state.set(state);
            self.target_display_offset.set(Some(0));
            return;
        }

        let offset_delta = (offset.y / state.line_height).round() as i32;
        let display_offset = (max_offset as i32 + offset_delta).clamp(0, max_offset as i32);
        state.display_offset = display_offset as usize;
        self.state.set(state);
        self.target_display_offset
            .set(Some(display_offset as usize));
    }

    fn content_size(&self) -> gpui::Size<Pixels> {
        let state = self.state.get();
        size(
            Pixels::ZERO,
            state.total_lines.max(state.viewport_lines) as f32 * state.line_height,
        )
    }

    fn start_drag(&self) {
        self.dragging.set(true);
    }

    fn end_drag(&self) {
        self.dragging.set(false);
    }
}

pub(crate) struct ScrollState {
    pub(crate) block_below_cursor: Option<Rc<BlockProperties>>,
    pub(crate) scroll_top: Pixels,
    /// True while the primary mouse button is held down after a press inside this terminal view.
    /// Used to keep selection dragging alive even when the cursor leaves the terminal hitbox.
    pub(crate) mouse_left_down_in_terminal: bool,
    /// Whether the pointer is currently within the scrollbar lane.
    ///
    /// We keep this in the view so the element can render an overlay scrollbar that
    /// auto-hides when the mouse leaves the scrollbar region.
    pub(crate) scrollbar_hovered: bool,
    /// Whether the scrollbar is temporarily revealed due to scroll-wheel/scroll actions.
    pub(crate) scrollbar_revealed: bool,
    pub(crate) scrollbar_reveal_epoch: usize,
    /// Whether to auto-follow the live view as output arrives.
    /// This tracks the "block below cursor" extra scroll space; terminal scrollback is handled
    /// separately via `TerminalContent.display_offset`.
    pub(crate) stick_to_bottom: bool,

    /// Hover preview for search markers in the overlay scrollbar.
    pub(crate) scrollbar_preview: Option<ScrollbarPreview>,
}

impl Default for ScrollState {
    fn default() -> Self {
        Self {
            block_below_cursor: None,
            scroll_top: Pixels::ZERO,
            mouse_left_down_in_terminal: false,
            scrollbar_hovered: false,
            scrollbar_revealed: false,
            scrollbar_reveal_epoch: 0,
            stick_to_bottom: true,
            scrollbar_preview: None,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ScrollbarPreview {
    pub(crate) match_index: usize,
    pub(crate) anchor: Point<Pixels>,
    pub(crate) start_line_from_top: usize,
    pub(crate) cols: usize,
    pub(crate) rows: usize,
    pub(crate) cells: Vec<crate::IndexedCell>,
    /// The matched range in preview-local coordinates (re-based so the preview's first row is line
    /// 0).
    pub(crate) match_range: RangeInclusive<GridPoint>,
}

pub(crate) fn scrollbar_bounds_for_terminal(
    terminal_bounds: Bounds<Pixels>,
    scrollbar_width: Pixels,
) -> Bounds<Pixels> {
    // The terminal bounds describe the character grid. The scrollbar and marker lane starts
    // immediately after it so terminal characters never render underneath the lane.
    let w = scrollbar_width.max(Pixels::ZERO);
    Bounds {
        origin: point(terminal_bounds.right(), terminal_bounds.origin.y),
        size: size(w, terminal_bounds.size.height),
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ScrollbarMarkerSpec {
    pub(crate) match_index: usize,
    pub(crate) y: Pixels,
    pub(crate) active: bool,
}

#[derive(Clone, Copy)]
pub(crate) struct ScrollbarMarkerViewport {
    lane_bounds: Bounds<Pixels>,
    line_height: Pixels,
    content_y_offset: Pixels,
    display_offset: usize,
    viewport_lines: usize,
}

impl ScrollbarMarkerViewport {
    pub(crate) fn new(
        lane_bounds: Bounds<Pixels>,
        line_height: Pixels,
        content_y_offset: Pixels,
        display_offset: usize,
        viewport_lines: usize,
    ) -> Self {
        Self {
            lane_bounds,
            line_height,
            content_y_offset,
            display_offset,
            viewport_lines,
        }
    }
}

fn visible_match_index_range(
    matches: &[RangeInclusive<GridPoint>],
    display_offset: usize,
    viewport_lines: usize,
) -> std::ops::Range<usize> {
    let top_line = -(display_offset as i128);
    let bottom_line = top_line + viewport_lines as i128;
    let start =
        matches.partition_point(|search_match| i128::from(search_match.start().line) < top_line);
    let end =
        matches.partition_point(|search_match| i128::from(search_match.start().line) < bottom_line);
    start..end
}

fn match_viewport_row(search_match: &RangeInclusive<GridPoint>, display_offset: usize) -> usize {
    (i128::from(search_match.start().line) + display_offset as i128) as usize
}

fn marker_y_for_viewport_row(
    lane_bounds: Bounds<Pixels>,
    line_height: Pixels,
    content_y_offset: Pixels,
    viewport_row: usize,
) -> Option<Pixels> {
    let y = lane_bounds.origin.y + (viewport_row as f32 + 0.5) * line_height - content_y_offset;
    (y >= lane_bounds.top() && y < lane_bounds.bottom()).then_some(y)
}

pub(crate) fn scrollbar_marker_specs(
    viewport: ScrollbarMarkerViewport,
    matches: &[RangeInclusive<GridPoint>],
    active_match_index: Option<usize>,
    limit: usize,
) -> Vec<ScrollbarMarkerSpec> {
    if matches.is_empty()
        || viewport.line_height <= Pixels::ZERO
        || viewport.viewport_lines == 0
        || limit == 0
    {
        return Vec::new();
    }

    let mut specs: Vec<ScrollbarMarkerSpec> = Vec::new();
    let mut last_row = None;
    for match_index in
        visible_match_index_range(matches, viewport.display_offset, viewport.viewport_lines)
    {
        let search_match = &matches[match_index];
        let viewport_row = match_viewport_row(search_match, viewport.display_offset);
        let Some(y) = marker_y_for_viewport_row(
            viewport.lane_bounds,
            viewport.line_height,
            viewport.content_y_offset,
            viewport_row,
        ) else {
            continue;
        };
        let active = active_match_index == Some(match_index);

        if last_row == Some(viewport_row) {
            let spec = specs.last_mut().expect("a grouped marker must exist");
            if active {
                spec.match_index = match_index;
                spec.active = true;
            }
            continue;
        }

        if specs.len() >= limit {
            break;
        }

        last_row = Some(viewport_row);
        specs.push(ScrollbarMarkerSpec {
            match_index,
            y,
            active,
        });
    }

    specs
}

pub(crate) fn buffer_index_for_line_coord(
    total_lines: usize,
    viewport_lines: usize,
    line_coord: i32,
) -> usize {
    if total_lines == 0 || viewport_lines == 0 {
        return 0;
    }

    let top_line: i64 = viewport_lines as i64 - total_lines as i64;
    let idx_i64 = i64::from(line_coord) - top_line;
    idx_i64
        .clamp(0, total_lines.saturating_sub(1) as i64)
        .max(0) as usize
}

pub(crate) fn scroll_offset_for_line_coord_centered(
    total_lines: usize,
    viewport_lines: usize,
    line_coord: i32,
) -> usize {
    if total_lines == 0 || viewport_lines == 0 {
        return 0;
    }

    let max_offset = total_lines.saturating_sub(viewport_lines);
    if max_offset == 0 {
        return 0;
    }

    // Convert the backend-relative `line_coord` to a stable buffer index from the top of
    // scrollback (`0..total_lines-1`).
    let idx = buffer_index_for_line_coord(total_lines, viewport_lines, line_coord);

    // Choose a viewport start such that the target line is roughly centered (clamped to range).
    let half = viewport_lines / 2;
    let start_from_top = idx.saturating_sub(half).min(max_offset);

    // `display_offset` is measured from bottom: 0 = bottom, max_offset = top.
    max_offset.saturating_sub(start_from_top)
}

pub(crate) fn search_match_index_for_scrollbar_hover(
    viewport: ScrollbarMarkerViewport,
    matches: &[RangeInclusive<GridPoint>],
    hover_y: Pixels,
    hit_radius: Pixels,
) -> Option<usize> {
    if matches.is_empty() || hit_radius <= Pixels::ZERO || viewport.viewport_lines == 0 {
        return None;
    }

    let mut last_row = None;
    let mut best = None;
    for match_index in
        visible_match_index_range(matches, viewport.display_offset, viewport.viewport_lines)
    {
        let viewport_row = match_viewport_row(&matches[match_index], viewport.display_offset);
        if last_row == Some(viewport_row) {
            continue;
        }
        last_row = Some(viewport_row);

        let Some(y) = marker_y_for_viewport_row(
            viewport.lane_bounds,
            viewport.line_height,
            viewport.content_y_offset,
            viewport_row,
        ) else {
            continue;
        };
        let distance = (y - hover_y).abs();
        if distance <= hit_radius
            && best
                .map(|(_, best_distance)| distance < best_distance)
                .unwrap_or(true)
        {
            best = Some((match_index, distance));
        }
    }
    best.map(|(match_index, _)| match_index)
}

impl TerminalView {
    pub(super) fn max_scroll_top(&self, cx: &App) -> Pixels {
        let terminal = self.terminal.read(cx);

        let Some(block) = self.scroll.block_below_cursor.as_ref() else {
            return Pixels::ZERO;
        };

        let content = terminal.last_content();
        let line_height = content.terminal_bounds.line_height;
        let viewport_lines = terminal.viewport_lines();
        let cursor =
            point_to_viewport(content.display_offset, content.cursor.point).unwrap_or_default();
        let max_scroll_top_in_lines =
            (block.height as usize).saturating_sub(viewport_lines.saturating_sub(cursor.line + 1));

        max_scroll_top_in_lines as f32 * line_height
    }

    /// Zed-like behavior: if the user is looking at history (terminal scrollback) or has scrolled
    /// away from the live block content, any input that is forwarded to the PTY should snap the
    /// view back to the bottom.
    pub(super) fn snap_to_bottom_on_input(&mut self, cx: &mut Context<Self>) {
        let TerminalScrollState {
            display_offset,
            line_height,
            ..
        } = self.terminal_scroll_state(cx);

        // History scrolling: ensure we immediately return to the live viewport on any PTY input.
        // (Some input paths, like `try_keystroke`, don't go through backend `input()`.)
        if display_offset != 0 {
            self.terminal.update(cx, |term, _| term.scroll_to_bottom());
            self.scroll.scroll_top = Pixels::ZERO;
            self.scroll.stick_to_bottom = true;
            cx.notify();
            return;
        }

        // Block-below-cursor extra scroll space (e.g. prompt block). Only applies in live view.
        if self.scroll.block_below_cursor.is_some() {
            let max = self.max_scroll_top(cx);
            let at_bottom = self.scroll.scroll_top + line_height / 2.0 >= max;

            if !at_bottom {
                self.scroll.scroll_top = max;
            }
        }

        // Typing implies "follow output" unless the user scrolls away again.
        self.scroll.stick_to_bottom = true;
        cx.notify();
    }

    pub(crate) fn set_mouse_left_down_in_terminal(&mut self, down: bool) {
        self.scroll.mouse_left_down_in_terminal = down;
    }

    pub(crate) fn mouse_left_down_in_terminal(&self) -> bool {
        self.scroll.mouse_left_down_in_terminal
    }

    pub(crate) fn scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Scroll-wheel usage should reveal the overlay scrollbar briefly, even if the pointer
        // isn't within the scrollbar lane.
        //
        // Note: This is kept view-local so we can avoid allocating dedicated layout space.
        // The element decides whether to paint the scrollbar based on this flag.
        // (Timer-based auto-hide happens in `reveal_scrollbar_for_scroll`.)
        //
        // We don't require focus checks here; callers already gate on focus.
        self.reveal_scrollbar_for_scroll(window, cx);

        let TerminalScrollState {
            is_remote_mirror,
            display_offset,
            line_height,
        } = self.terminal_scroll_state(cx);

        if is_remote_mirror {
            self.terminal.update(cx, |term, _| term.scroll_wheel(event));
            return;
        }

        if self.scroll.block_below_cursor.is_some() && display_offset == 0 {
            let y_delta = event.delta.pixel_delta(line_height).y;
            if y_delta < Pixels::ZERO || self.scroll.scroll_top > Pixels::ZERO {
                let max = self.max_scroll_top(cx);
                self.scroll.scroll_top = cmp::max(
                    Pixels::ZERO,
                    cmp::min(self.scroll.scroll_top - y_delta, max),
                );
                self.scroll.stick_to_bottom = self.scroll.scroll_top + line_height / 2.0 >= max;
                cx.notify();
                return;
            }
        }
        // Scrolling the terminal history should never keep a block-scroll offset.
        self.scroll.scroll_top = Pixels::ZERO;
        self.scroll.stick_to_bottom = false;
        self.terminal.update(cx, |term, _| term.scroll_wheel(event));
    }

    pub(crate) fn scrollbar_dragging(&self) -> bool {
        self.terminal_scrollbar_handle.is_dragging()
    }

    pub(crate) fn scrollbar_hovered(&self) -> bool {
        self.scroll.scrollbar_hovered
    }

    pub(crate) fn scrollbar_revealed(&self) -> bool {
        self.scroll.scrollbar_revealed
    }

    pub(crate) fn scrollbar_bounds(&self, cx: &App) -> Bounds<Pixels> {
        let terminal = self.terminal.read(cx);
        let width = if TerminalSettings::global(cx).show_scrollbar {
            SCROLLBAR_WIDTH
        } else {
            Pixels::ZERO
        };
        scrollbar_bounds_for_terminal(terminal.last_content().terminal_bounds.bounds, width)
    }

    pub(crate) fn sync_terminal_scrollbar_handle(&self, cx: &App) {
        let terminal = self.terminal.read(cx);
        let content = terminal.last_content();
        self.terminal_scrollbar_handle.update(
            content.terminal_bounds.line_height,
            terminal.total_lines(),
            terminal.viewport_lines(),
            content.display_offset,
        );
    }

    pub(crate) fn apply_terminal_scrollbar_target(&mut self, cx: &mut Context<Self>) {
        let Some(target_offset) = self.terminal_scrollbar_handle.take_target_display_offset()
        else {
            return;
        };

        self.apply_scrollbar_target_offset(target_offset, cx);
    }

    pub(crate) fn set_scrollbar_hovered(&mut self, hovered: bool, cx: &mut Context<Self>) {
        if self.scroll.scrollbar_hovered != hovered {
            self.scroll.scrollbar_hovered = hovered;
            cx.notify();
        }
    }

    fn reveal_scrollbar_for_scroll(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if !TerminalSettings::global(cx).show_scrollbar {
            return;
        }
        self.scroll.scrollbar_revealed = true;
        self.scroll.scrollbar_reveal_epoch = self.scroll.scrollbar_reveal_epoch.wrapping_add(1);
        let epoch = self.scroll.scrollbar_reveal_epoch;
        cx.notify();

        // Auto-hide after a short delay. If the user is hovering/dragging at that time, the
        // scrollbar stays visible due to those signals (not this temporary reveal flag).
        cx.spawn(async move |this, cx| {
            Timer::after(Duration::from_millis(900)).await;
            let _ = this.update(cx, |this, cx| {
                if this.scroll.scrollbar_reveal_epoch != epoch {
                    return;
                }
                if this.scroll.scrollbar_revealed {
                    this.scroll.scrollbar_revealed = false;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(crate) fn scroll_top(&self) -> Pixels {
        self.scroll.scroll_top
    }

    pub(crate) fn scrollbar_preview(&self) -> Option<&ScrollbarPreview> {
        self.scroll.scrollbar_preview.as_ref()
    }

    pub(crate) fn clear_scrollbar_preview(&mut self, cx: &mut Context<Self>) {
        if self.scroll.scrollbar_preview.take().is_some() {
            cx.notify();
        }
    }

    pub(crate) fn set_scrollbar_preview_for_match(
        &mut self,
        match_index: usize,
        anchor: gpui::Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let Some((start, cols, rows, cells, match_range)) = (|| {
            let terminal = self.terminal.read(cx);
            let matches = terminal.matches();
            if match_index >= matches.len() {
                return None;
            }

            let total_lines = terminal.total_lines();
            let viewport_lines = terminal.viewport_lines();
            let match_range = matches[match_index].clone();

            let start_line_coord = match_range.start().line;
            let end_line_coord = match_range.end().line;
            let start_line_from_top =
                buffer_index_for_line_coord(total_lines, viewport_lines, start_line_coord);
            let end_line_from_top =
                buffer_index_for_line_coord(total_lines, viewport_lines, end_line_coord);

            let context_above = 3usize;
            let total = 7usize;
            let start = start_line_from_top.saturating_sub(context_above);

            let (cols, rows, cells) = terminal.preview_cells_from_top(start, total);
            if rows == 0 || cells.is_empty() {
                return None;
            }

            // Convert the match range into preview-local coordinates (preview starts at line 0).
            let local_start_line = start_line_from_top.saturating_sub(start);
            let local_end_line = end_line_from_top.saturating_sub(start);
            let local_range = RangeInclusive::new(
                GridPoint::new(local_start_line as i32, match_range.start().column),
                GridPoint::new(local_end_line as i32, match_range.end().column),
            );

            Some((start, cols, rows, cells, local_range))
        })() else {
            self.clear_scrollbar_preview(cx);
            return;
        };

        // Avoid re-fetching preview text while the pointer moves within the same marker.
        if let Some(prev) = self.scroll.scrollbar_preview.as_mut()
            && prev.match_index == match_index
            && prev.start_line_from_top == start
        {
            prev.anchor = anchor;
            cx.notify();
            return;
        }

        self.scroll.scrollbar_preview = Some(ScrollbarPreview {
            match_index,
            anchor,
            start_line_from_top: start,
            cols,
            rows,
            cells,
            match_range,
        });
        cx.notify();
    }

    pub(crate) fn update_scrollbar_preview_at(
        &mut self,
        lane: Bounds<Pixels>,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let match_idx = {
            let terminal = self.terminal.read(cx);
            let content = terminal.last_content();
            let content_y_offset = if content.display_offset == 0 {
                self.scroll_top()
            } else {
                Pixels::ZERO
            };
            search_match_index_for_scrollbar_hover(
                ScrollbarMarkerViewport::new(
                    lane,
                    content.terminal_bounds.line_height,
                    content_y_offset,
                    content.display_offset,
                    terminal.viewport_lines(),
                ),
                terminal.matches(),
                position.y,
                SCROLLBAR_MARKER_HIT_RADIUS,
            )
        };

        if let Some(match_idx) = match_idx {
            self.set_scrollbar_preview_for_match(match_idx, position, cx);
        } else {
            self.clear_scrollbar_preview(cx);
        }
    }

    pub(crate) fn apply_scrollbar_target_offset(
        &mut self,
        target_offset: usize,
        cx: &mut Context<Self>,
    ) {
        let current = {
            let terminal = self.terminal.read(cx);
            terminal.last_content().display_offset
        };
        self.scroll_to_display_offset_from_current(current, target_offset, cx);
    }

    fn scroll_to_display_offset_from_current(
        &mut self,
        current_offset: usize,
        target_offset: usize,
        cx: &mut Context<Self>,
    ) {
        let (is_remote_mirror, max_offset) = {
            let terminal = self.terminal.read(cx);
            let total_lines = terminal.total_lines();
            let viewport_lines = terminal.viewport_lines();
            (
                terminal.is_remote_mirror(),
                total_lines.saturating_sub(viewport_lines),
            )
        };

        let current_offset = current_offset.min(max_offset);
        let target_offset = target_offset.min(max_offset);
        if target_offset == current_offset {
            if !is_remote_mirror {
                self.scroll.scroll_top = Pixels::ZERO;
                self.scroll.stick_to_bottom = target_offset == 0;
            }
            return;
        }

        if !is_remote_mirror {
            // Scrolling the terminal history should never keep the extra "block below cursor"
            // scroll.
            self.scroll.scroll_top = Pixels::ZERO;
        }

        self.terminal.update(cx, |term, _| {
            if target_offset == 0 {
                term.scroll_to_bottom();
            } else if target_offset == max_offset {
                term.scroll_to_top();
            } else if target_offset > current_offset {
                term.scroll_up_by(target_offset - current_offset);
            } else {
                term.scroll_down_by(current_offset - target_offset);
            }
        });

        if !is_remote_mirror {
            self.scroll.stick_to_bottom = target_offset == 0;
            cx.notify();
        }
    }

    // `scroll_to_display_offset_from_current` is the only implementation we need right now.

    pub(super) fn scroll_line_up(
        &mut self,
        _: &ScrollLineUp,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reveal_scrollbar_for_scroll(window, cx);
        let TerminalScrollState {
            is_remote_mirror,
            display_offset,
            line_height,
        } = self.terminal_scroll_state(cx);
        if is_remote_mirror {
            self.terminal.update(cx, |term, _| term.scroll_line_up());
            return;
        }
        if self.scroll.block_below_cursor.is_some()
            && display_offset == 0
            && self.scroll.scroll_top > Pixels::ZERO
        {
            self.scroll.scroll_top = cmp::max(self.scroll.scroll_top - line_height, Pixels::ZERO);
            let max = self.max_scroll_top(cx);
            self.scroll.stick_to_bottom = self.scroll.scroll_top + line_height / 2.0 >= max;
            return;
        }

        self.terminal.update(cx, |term, _| term.scroll_line_up());
        // Terminal scrollback and block scrolling are mutually exclusive.
        self.scroll.scroll_top = Pixels::ZERO;
        self.scroll.stick_to_bottom = false;
        cx.notify();
    }

    pub(super) fn scroll_line_down(
        &mut self,
        _: &ScrollLineDown,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reveal_scrollbar_for_scroll(window, cx);
        let TerminalScrollState {
            is_remote_mirror,
            display_offset,
            line_height,
        } = self.terminal_scroll_state(cx);
        if is_remote_mirror {
            self.terminal.update(cx, |term, _| term.scroll_line_down());
            return;
        }
        if self.scroll.block_below_cursor.is_some() && display_offset == 0 {
            let max_scroll_top = self.max_scroll_top(cx);
            if self.scroll.scroll_top < max_scroll_top {
                self.scroll.scroll_top =
                    cmp::min(self.scroll.scroll_top + line_height, max_scroll_top);
            }
            self.scroll.stick_to_bottom =
                self.scroll.scroll_top + line_height / 2.0 >= max_scroll_top;
            return;
        }

        self.terminal.update(cx, |term, _| term.scroll_line_down());
        // Terminal scrollback and block scrolling are mutually exclusive.
        self.scroll.scroll_top = Pixels::ZERO;
        self.scroll.stick_to_bottom = false;
        cx.notify();
    }

    pub(super) fn scroll_page_up(
        &mut self,
        _: &ScrollPageUp,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reveal_scrollbar_for_scroll(window, cx);
        let (is_remote_mirror, line_height, viewport_lines) = {
            let terminal = self.terminal.read(cx);
            (
                terminal.is_remote_mirror(),
                terminal.last_content().terminal_bounds.line_height(),
                terminal.viewport_lines(),
            )
        };
        if is_remote_mirror {
            self.terminal.update(cx, |term, _| term.scroll_page_up());
            return;
        }
        if self.scroll.scroll_top == Pixels::ZERO {
            self.terminal.update(cx, |term, _| term.scroll_page_up());
            self.scroll.scroll_top = Pixels::ZERO;
            self.scroll.stick_to_bottom = false;
        } else {
            let visible_block_lines = (self.scroll.scroll_top / line_height) as usize;
            let visible_content_lines = viewport_lines - visible_block_lines;

            if visible_block_lines >= viewport_lines {
                self.scroll.scroll_top =
                    ((visible_block_lines - viewport_lines) as f32) * line_height;
            } else {
                self.scroll.scroll_top = px(0.);
                self.terminal
                    .update(cx, |term, _| term.scroll_up_by(visible_content_lines));
            }
            self.scroll.stick_to_bottom =
                self.scroll.scroll_top + line_height / 2.0 >= self.max_scroll_top(cx);
        }
        cx.notify();
    }

    pub(super) fn scroll_page_down(
        &mut self,
        _: &ScrollPageDown,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reveal_scrollbar_for_scroll(window, cx);
        let TerminalScrollState {
            is_remote_mirror,
            display_offset,
            ..
        } = self.terminal_scroll_state(cx);
        if is_remote_mirror {
            self.terminal.update(cx, |term, _| term.scroll_page_down());
            return;
        }
        self.terminal.update(cx, |term, _| term.scroll_page_down());
        // Scrolling the terminal history should not apply block scrolling offsets.
        // `scroll_top` is only meaningful while we're at the live view.
        if self.scroll.block_below_cursor.is_some() && display_offset == 0 {
            self.scroll.scroll_top = self.max_scroll_top(cx);
            self.scroll.stick_to_bottom = true;
        } else {
            self.scroll.scroll_top = Pixels::ZERO;
            self.scroll.stick_to_bottom = false;
        }
        cx.notify();
    }

    pub(super) fn scroll_to_top(
        &mut self,
        _: &ScrollToTop,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reveal_scrollbar_for_scroll(window, cx);
        let is_remote_mirror = self.terminal_scroll_state(cx).is_remote_mirror;
        if is_remote_mirror {
            self.terminal.update(cx, |term, _| term.scroll_to_top());
            return;
        }
        self.terminal.update(cx, |term, _| term.scroll_to_top());
        self.scroll.scroll_top = Pixels::ZERO;
        self.scroll.stick_to_bottom = false;
        cx.notify();
    }

    pub(super) fn scroll_to_bottom(
        &mut self,
        _: &ScrollToBottom,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reveal_scrollbar_for_scroll(window, cx);
        let is_remote_mirror = self.terminal_scroll_state(cx).is_remote_mirror;
        if is_remote_mirror {
            self.terminal.update(cx, |term, _| term.scroll_to_bottom());
            return;
        }
        self.terminal.update(cx, |term, _| term.scroll_to_bottom());
        if self.scroll.block_below_cursor.is_some() {
            self.scroll.scroll_top = self.max_scroll_top(cx);
        } else {
            self.scroll.scroll_top = Pixels::ZERO;
        }
        self.scroll.stick_to_bottom = true;
        cx.notify();
    }

    pub(super) fn toggle_vi_mode(
        &mut self,
        _: &ToggleViMode,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.terminal.update(cx, |term, _| term.toggle_vi_mode());
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Bounds, Pixels, point, px, size};
    use gpui_component::scroll::ScrollbarHandle as _;

    use super::{
        SCROLLBAR_MARKER_LIMIT, ScrollbarMarkerViewport, TerminalScrollbarHandle,
        buffer_index_for_line_coord, scroll_offset_for_line_coord_centered,
        scrollbar_bounds_for_terminal, scrollbar_marker_specs,
        search_match_index_for_scrollbar_hover,
    };
    use crate::GridPoint;

    fn marker_viewport(
        lane_bounds: Bounds<Pixels>,
        line_height: Pixels,
        display_offset: usize,
        viewport_lines: usize,
    ) -> ScrollbarMarkerViewport {
        ScrollbarMarkerViewport::new(
            lane_bounds,
            line_height,
            Pixels::ZERO,
            display_offset,
            viewport_lines,
        )
    }

    #[test]
    fn terminal_scrollbar_handle_maps_display_offset_to_component_offset() {
        let handle = TerminalScrollbarHandle::default();
        handle.update(px(10.0), 100, 10, 0);

        assert_eq!(handle.content_size().height, px(1000.0));
        assert_eq!(handle.offset().y, px(-900.0));

        handle.update(px(10.0), 100, 10, 90);
        assert_eq!(handle.offset().y, px(0.0));
    }

    #[test]
    fn terminal_scrollbar_handle_maps_component_offset_to_display_offset() {
        let handle = TerminalScrollbarHandle::default();
        handle.update(px(10.0), 100, 10, 0);

        handle.set_offset(point(Pixels::ZERO, px(-450.0)));
        assert_eq!(handle.take_target_display_offset(), Some(45));
        assert_eq!(handle.offset().y, px(-450.0));

        handle.set_offset(point(Pixels::ZERO, px(100.0)));
        assert_eq!(handle.take_target_display_offset(), Some(90));
        assert_eq!(handle.offset().y, Pixels::ZERO);

        handle.set_offset(point(Pixels::ZERO, px(-10_000.0)));
        assert_eq!(handle.take_target_display_offset(), Some(0));
        assert_eq!(handle.offset().y, px(-900.0));
    }

    #[test]
    fn terminal_scrollbar_handle_handles_unscrollable_content_and_zero_line_height() {
        let handle = TerminalScrollbarHandle::default();
        handle.update(Pixels::ZERO, 5, 10, 99);

        assert_eq!(handle.content_size().height, px(10.0));
        assert_eq!(handle.offset().y, Pixels::ZERO);

        handle.set_offset(point(Pixels::ZERO, px(-100.0)));
        assert_eq!(handle.take_target_display_offset(), Some(0));
    }

    #[test]
    fn scrollbar_bounds_for_terminal_returns_dedicated_lane_bounds() {
        let terminal_bounds = Bounds {
            origin: point(px(10.0), px(20.0)),
            size: size(px(80.0), px(100.0)),
        };

        let bounds = scrollbar_bounds_for_terminal(terminal_bounds, px(14.0));

        assert_eq!(bounds.origin, point(px(90.0), px(20.0)));
        assert_eq!(bounds.size, size(px(14.0), px(100.0)));
    }

    #[test]
    fn scrollbar_markers_align_with_visible_terminal_rows() {
        let lane = Bounds {
            origin: point(px(0.0), px(20.0)),
            size: size(px(14.0), px(40.0)),
        };
        let matches = vec![
            GridPoint::new(-2, 0)..=GridPoint::new(-2, 1),
            GridPoint::new(-1, 0)..=GridPoint::new(-1, 1),
            GridPoint::new(1, 0)..=GridPoint::new(1, 1),
            GridPoint::new(2, 0)..=GridPoint::new(2, 1),
        ];

        let specs = scrollbar_marker_specs(
            marker_viewport(lane, px(10.0), 2, 4),
            &matches,
            None,
            SCROLLBAR_MARKER_LIMIT,
        );

        assert_eq!(specs.len(), 3);
        assert_eq!(
            specs.iter().map(|spec| spec.y).collect::<Vec<_>>(),
            vec![px(25.0), px(35.0), px(55.0)]
        );
    }

    #[test]
    fn scrollbar_markers_follow_terminal_content_pixel_offset() {
        let lane = Bounds {
            origin: point(px(0.0), px(20.0)),
            size: size(px(14.0), px(40.0)),
        };
        let matches = vec![GridPoint::new(0, 0)..=GridPoint::new(0, 1)];

        let specs = scrollbar_marker_specs(
            ScrollbarMarkerViewport::new(lane, px(10.0), px(3.0), 0, 4),
            &matches,
            None,
            SCROLLBAR_MARKER_LIMIT,
        );

        assert_eq!(specs[0].y, px(22.0));
    }

    #[test]
    fn scrollbar_markers_exclude_rows_shifted_out_of_view() {
        let lane = Bounds {
            origin: point(px(0.0), px(20.0)),
            size: size(px(14.0), px(40.0)),
        };
        let matches = vec![
            GridPoint::new(0, 0)..=GridPoint::new(0, 1),
            GridPoint::new(1, 0)..=GridPoint::new(1, 1),
        ];

        let specs = scrollbar_marker_specs(
            ScrollbarMarkerViewport::new(lane, px(10.0), px(11.0), 0, 4),
            &matches,
            None,
            SCROLLBAR_MARKER_LIMIT,
        );

        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].match_index, 1);
        assert_eq!(specs[0].y, px(24.0));
    }

    #[test]
    fn scroll_offset_for_line_coord_centered_scrolls_to_make_line_visible() {
        // total=100, viewport=10 => max_offset=90, top_line=-90, bottom_line=9.
        let total_lines = 100;
        let viewport_lines = 10;

        // Topmost line should scroll to top.
        assert_eq!(
            scroll_offset_for_line_coord_centered(total_lines, viewport_lines, -90),
            90
        );

        // Bottommost line should scroll to bottom.
        assert_eq!(
            scroll_offset_for_line_coord_centered(total_lines, viewport_lines, 9),
            0
        );

        // A line at the top of the live viewport should keep us close to bottom (line_coord=0).
        // With centering, this should land at offset=5 (puts idx=90 near middle).
        assert_eq!(
            scroll_offset_for_line_coord_centered(total_lines, viewport_lines, 0),
            5
        );
    }

    #[test]
    fn search_match_index_for_scrollbar_hover_picks_nearest_marker() {
        let lane = Bounds {
            origin: point(px(0.0), px(0.0)),
            size: size(px(10.0), px(100.0)),
        };

        let matches = vec![
            GridPoint::new(-2, 0)..=GridPoint::new(-2, 1),
            GridPoint::new(-1, 0)..=GridPoint::new(-1, 1),
            GridPoint::new(0, 0)..=GridPoint::new(0, 1),
        ];

        let hit = search_match_index_for_scrollbar_hover(
            marker_viewport(lane, px(10.0), 2, 3),
            &matches,
            px(16.0),
            px(6.0),
        );
        assert_eq!(hit, Some(1));

        let miss = search_match_index_for_scrollbar_hover(
            marker_viewport(lane, px(10.0), 2, 3),
            &matches,
            px(50.0),
            px(0.5),
        );
        assert_eq!(miss, None);
    }

    #[test]
    fn scrollbar_marker_specs_marks_group_active_when_duplicate_y_contains_active_match() {
        let track = Bounds {
            origin: point(px(0.0), px(0.0)),
            size: size(px(10.0), px(100.0)),
        };
        let matches = vec![
            GridPoint::new(-1, 0)..=GridPoint::new(-1, 1),
            GridPoint::new(-1, 4)..=GridPoint::new(-1, 5),
        ];

        let specs = scrollbar_marker_specs(
            marker_viewport(track, px(10.0), 1, 3),
            &matches,
            Some(1),
            SCROLLBAR_MARKER_LIMIT,
        );

        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].match_index, 1);
        assert!(specs[0].active);
    }

    #[test]
    fn scrollbar_marker_limit_keeps_active_duplicate_on_last_included_row() {
        let lane = Bounds {
            origin: point(px(0.0), px(0.0)),
            size: size(px(10.0), px(100.0)),
        };
        let matches = vec![
            GridPoint::new(0, 0)..=GridPoint::new(0, 1),
            GridPoint::new(0, 4)..=GridPoint::new(0, 5),
            GridPoint::new(1, 0)..=GridPoint::new(1, 1),
        ];

        let specs =
            scrollbar_marker_specs(marker_viewport(lane, px(10.0), 0, 3), &matches, Some(1), 1);

        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].match_index, 1);
        assert!(specs[0].active);
    }

    #[test]
    fn scrollbar_marker_specs_preserves_first_seen_order_while_grouping_duplicates() {
        let track = Bounds {
            origin: point(px(0.0), px(0.0)),
            size: size(px(10.0), px(100.0)),
        };
        let matches = vec![
            GridPoint::new(-2, 0)..=GridPoint::new(-2, 1),
            GridPoint::new(-1, 0)..=GridPoint::new(-1, 1),
            GridPoint::new(-1, 4)..=GridPoint::new(-1, 5),
            GridPoint::new(0, 0)..=GridPoint::new(0, 1),
        ];

        let specs = scrollbar_marker_specs(
            marker_viewport(track, px(10.0), 2, 3),
            &matches,
            Some(2),
            SCROLLBAR_MARKER_LIMIT,
        );

        assert_eq!(specs.len(), 3);
        assert_eq!(specs[0].match_index, 0);
        assert_eq!(specs[1].match_index, 2);
        assert_eq!(specs[2].match_index, 3);
        assert!(specs[1].active);
    }

    #[test]
    fn scrollbar_marker_specs_respects_limit() {
        let track = Bounds {
            origin: point(px(0.0), px(0.0)),
            size: size(px(10.0), px(100.0)),
        };
        let matches = vec![
            GridPoint::new(-4, 0)..=GridPoint::new(-4, 1),
            GridPoint::new(-3, 0)..=GridPoint::new(-3, 1),
            GridPoint::new(-2, 0)..=GridPoint::new(-2, 1),
        ];

        let specs =
            scrollbar_marker_specs(marker_viewport(track, px(10.0), 4, 5), &matches, Some(2), 2);

        assert_eq!(specs.len(), 2);
        assert_eq!(specs[0].match_index, 0);
        assert_eq!(specs[1].match_index, 1);
        assert!(!specs.iter().any(|spec| spec.active));
    }

    #[test]
    fn buffer_index_for_line_coord_maps_scrollback_and_viewport() {
        // total=100, viewport=10 => top_line = -90. So line_coord=-90 => idx=0, line_coord=9 =>
        // idx=99.
        assert_eq!(buffer_index_for_line_coord(100, 10, -90), 0);
        assert_eq!(buffer_index_for_line_coord(100, 10, 9), 99);
        assert_eq!(buffer_index_for_line_coord(100, 10, 0), 90);
    }
}
