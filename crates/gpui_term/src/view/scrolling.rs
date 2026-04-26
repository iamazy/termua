use std::{cmp, ops::RangeInclusive, rc::Rc, time::Duration};

use gpui::{
    App, BorderStyle, Bounds, Context, Pixels, Point, ReadGlobal, ScrollWheelEvent, Window, fill,
    outline, point, px, size,
};
use gpui_component::ActiveTheme;
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
pub(crate) const SCROLLBAR_PAD: Pixels = px(2.0);
const MIN_THUMB_HEIGHT: Pixels = px(18.0);
const MARKER_SIZE: Pixels = px(4.0);
const ACTIVE_MARKER_SIZE: Pixels = px(6.0);

pub(crate) struct ScrollState {
    pub(crate) block_below_cursor: Option<Rc<BlockProperties>>,
    pub(crate) scroll_top: Pixels,
    /// True while the primary mouse button is held down after a press inside this terminal view.
    /// Used to keep selection dragging alive even when the cursor leaves the terminal hitbox.
    pub(crate) mouse_left_down_in_terminal: bool,
    /// True while the primary mouse button is dragging the scrollbar/minimap.
    pub(crate) scrollbar_dragging: bool,
    /// Whether the pointer is currently within the scrollbar lane.
    ///
    /// We keep this in the view so the element can render an overlay scrollbar that
    /// auto-hides when the mouse leaves the scrollbar region.
    pub(crate) scrollbar_hovered: bool,
    /// Whether the scrollbar is temporarily revealed due to scroll-wheel/scroll actions.
    pub(crate) scrollbar_revealed: bool,
    pub(crate) scrollbar_reveal_epoch: usize,
    /// Last scroll target requested by the scrollbar during an active drag/press.
    /// Prevents repeated scroll ops when the platform emits multiple move events at the same
    /// position (common cause of thumb flicker/jitter).
    pub(crate) scrollbar_last_target_offset: Option<usize>,
    /// View-local scroll position while dragging the scrollbar.
    ///
    /// `TerminalContent.display_offset` updates only after the backend processes queued scroll
    /// ops; keeping a virtual offset makes the scrollbar thumb feel responsive during drags.
    pub(crate) scrollbar_virtual_offset: Option<usize>,
    /// Initial pointer position and scroll offset for a scrollbar drag.
    ///
    /// We keep the drag mapping delta-based so a press doesn't "jump" the thumb to the pointer.
    pub(crate) scrollbar_drag_start_y: Option<Pixels>,
    pub(crate) scrollbar_drag_start_offset: Option<usize>,
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
            scrollbar_dragging: false,
            scrollbar_hovered: false,
            scrollbar_revealed: false,
            scrollbar_reveal_epoch: 0,
            scrollbar_last_target_offset: None,
            scrollbar_virtual_offset: None,
            scrollbar_drag_start_y: None,
            scrollbar_drag_start_offset: None,
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

#[derive(Clone)]
pub(crate) struct ScrollbarLayoutState {
    pub(crate) bounds: Bounds<Pixels>,
    pub(crate) track_bounds: Bounds<Pixels>,
    pub(crate) thumb_bounds: Bounds<Pixels>,
}

pub(crate) fn scrollbar_track_bounds(sb: Bounds<Pixels>) -> Bounds<Pixels> {
    let pad = SCROLLBAR_PAD
        .min(sb.size.width / 2.0)
        .min(sb.size.height / 2.0);
    Bounds {
        origin: sb.origin + point(pad, pad),
        size: size(
            (sb.size.width - pad * 2.0).max(Pixels::ZERO),
            (sb.size.height - pad * 2.0).max(Pixels::ZERO),
        ),
    }
}

pub(crate) fn scrollbar_bounds_for_terminal(
    terminal_bounds: Bounds<Pixels>,
    scrollbar_width: Pixels,
) -> Bounds<Pixels> {
    // Overlay scrollbar: place it inside the terminal bounds so the terminal content can
    // render underneath it (no dedicated layout gutter).
    let w = scrollbar_width.min(terminal_bounds.size.width.max(Pixels::ZERO));
    Bounds {
        origin: point(
            terminal_bounds.origin.x + (terminal_bounds.size.width - w).max(Pixels::ZERO),
            terminal_bounds.origin.y,
        ),
        size: size(w, terminal_bounds.size.height),
    }
}

pub(crate) fn overlay_scrollbar_layout_state(
    terminal_bounds: Bounds<Pixels>,
    scrollbar_width: Pixels,
    total_lines: usize,
    viewport_lines: usize,
    display_offset_for_thumb: usize,
) -> ScrollbarLayoutState {
    let bounds = scrollbar_bounds_for_terminal(terminal_bounds, scrollbar_width);
    let track_bounds = scrollbar_track_bounds(bounds);
    let thumb_bounds = thumb_bounds_for_track(
        track_bounds,
        total_lines,
        viewport_lines,
        display_offset_for_thumb,
    );
    ScrollbarLayoutState {
        bounds,
        track_bounds,
        thumb_bounds,
    }
}

pub(crate) fn thumb_bounds_for_track(
    track_bounds: Bounds<Pixels>,
    total_lines: usize,
    viewport_lines: usize,
    display_offset_for_thumb: usize,
) -> Bounds<Pixels> {
    let max_offset = total_lines.saturating_sub(viewport_lines);

    let track_h = track_bounds.size.height.max(Pixels::ZERO);
    if total_lines == 0 || max_offset == 0 || track_h <= Pixels::ZERO {
        return Bounds {
            origin: track_bounds.origin,
            size: size(track_bounds.size.width, track_h),
        };
    }

    let ratio = (viewport_lines as f32 / total_lines.max(1) as f32).min(1.0);
    let thumb_h = (track_h * ratio).max(MIN_THUMB_HEIGHT).min(track_h);
    let y_range = (track_h - thumb_h).max(Pixels::ZERO);

    let t = if max_offset == 0 {
        0.0
    } else {
        (display_offset_for_thumb as f32 / max_offset as f32).clamp(0.0, 1.0)
    };
    let thumb_y = track_bounds.origin.y + (1.0 - t) * y_range;
    Bounds {
        origin: point(track_bounds.origin.x, thumb_y),
        size: size(track_bounds.size.width, thumb_h),
    }
}

pub(crate) fn scrollbar_marker_y_for_line_coord(
    track_bounds: Bounds<Pixels>,
    total_lines: usize,
    viewport_lines: usize,
    line_coord: i32,
) -> Option<Pixels> {
    if total_lines == 0 || track_bounds.size.height <= Pixels::ZERO {
        return None;
    }

    let top_line = viewport_lines as i32 - total_lines as i32;
    let mut idx = line_coord.saturating_sub(top_line);
    let max_idx = total_lines.saturating_sub(1) as i32;
    if idx < 0 {
        idx = 0;
    } else if idx > max_idx {
        idx = max_idx;
    }

    // Map to the center of the corresponding "line band" in the track, similar to a minimap.
    // This avoids piling markers right on the top/bottom edges where they get clamped.
    let denom = total_lines.max(1) as f32;
    let t = (idx as f32 + 0.5) / denom;
    Some(track_bounds.origin.y + track_bounds.size.height * t.clamp(0.0, 1.0))
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
    track_bounds: Bounds<Pixels>,
    total_lines: usize,
    viewport_lines: usize,
    matches: &[RangeInclusive<GridPoint>],
    hover_y: Pixels,
    hit_radius: Pixels,
) -> Option<usize> {
    if matches.is_empty() || hit_radius <= Pixels::ZERO || total_lines == 0 || viewport_lines == 0 {
        return None;
    }

    // Approximate the hovered line by inverting the minimap mapping.
    let h = track_bounds.size.height;
    if h <= Pixels::ZERO {
        return None;
    }
    let mut t = (hover_y - track_bounds.origin.y) / h;
    t = t.clamp(0.0, 1.0);
    let idx = ((t * total_lines.max(1) as f32).floor() as i64)
        .clamp(0, total_lines.saturating_sub(1) as i64) as usize;
    let top_line = viewport_lines as i32 - total_lines as i32;
    let approx_line = top_line.saturating_add(idx as i32);

    let i = matches.partition_point(|m| m.start().line < approx_line);

    let min_y = hover_y - hit_radius;
    let max_y = hover_y + hit_radius;

    let mut best: Option<(usize, Pixels)> = None;

    // Scan down (increasing line -> increasing marker y) until we pass the hit window.
    let mut j = i;
    while j < matches.len() {
        let line = matches[j].start().line;
        let Some(y) =
            scrollbar_marker_y_for_line_coord(track_bounds, total_lines, viewport_lines, line)
        else {
            break;
        };
        if y > max_y {
            break;
        }
        let dy = (y - hover_y).abs();
        if dy <= hit_radius && best.map(|(_, best_dy)| dy < best_dy).unwrap_or(true) {
            best = Some((j, dy));
            if dy <= px(0.5) {
                break;
            }
        }
        j += 1;
    }

    // Scan up.
    let mut j = i;
    while j > 0 {
        j -= 1;
        let line = matches[j].start().line;
        let Some(y) =
            scrollbar_marker_y_for_line_coord(track_bounds, total_lines, viewport_lines, line)
        else {
            break;
        };
        if y < min_y {
            break;
        }
        let dy = (y - hover_y).abs();
        if dy <= hit_radius && best.map(|(_, best_dy)| dy < best_dy).unwrap_or(true) {
            best = Some((j, dy));
            if dy <= px(0.5) {
                break;
            }
        }
    }

    best.map(|(idx, _)| idx)
}

pub(crate) fn search_match_index_for_scrollbar_click(
    track_bounds: Bounds<Pixels>,
    total_lines: usize,
    viewport_lines: usize,
    matches: &[RangeInclusive<GridPoint>],
    click_y: Pixels,
    hit_radius: Pixels,
) -> Option<usize> {
    if matches.is_empty() || hit_radius <= Pixels::ZERO {
        return None;
    }

    let mut best: Option<(usize, Pixels)> = None;
    for (idx, m) in matches.iter().enumerate() {
        let line = m.start().line;
        let Some(y) =
            scrollbar_marker_y_for_line_coord(track_bounds, total_lines, viewport_lines, line)
        else {
            continue;
        };
        let dy = (y - click_y).abs();
        if dy <= hit_radius && best.map(|(_, best_dy)| dy < best_dy).unwrap_or(true) {
            best = Some((idx, dy));
            if dy <= px(0.5) {
                break;
            }
        }
    }

    best.map(|(idx, _)| idx)
}

pub(crate) fn scroll_offset_for_drag_delta(
    track: Bounds<Pixels>,
    drag_start_y: Pixels,
    current_y: Pixels,
    drag_start_offset: usize,
    total_lines: usize,
    viewport_lines: usize,
) -> usize {
    let max_offset = total_lines.saturating_sub(viewport_lines);
    if max_offset == 0 || track.size.height <= Pixels::ZERO {
        return 0;
    }

    // Match the thumb sizing math used during rendering so dragging feels 1:1 with the thumb.
    let track_h = track.size.height.max(Pixels::ZERO);
    let thumb_h = if total_lines == 0 || track_h <= Pixels::ZERO {
        track_h
    } else {
        let ratio = (viewport_lines as f32 / total_lines.max(1) as f32).min(1.0);
        (track_h * ratio).max(MIN_THUMB_HEIGHT).min(track_h)
    };
    let y_range = (track_h - thumb_h).max(Pixels::ZERO);
    if y_range <= Pixels::ZERO {
        return drag_start_offset.min(max_offset);
    }

    // Delta-based mapping prevents "jumping" the thumb to the pointer on press.
    let dy = current_y - drag_start_y;
    let delta_offset = (-(dy / y_range) * max_offset as f32).round();
    let target = (drag_start_offset as f32 + delta_offset).clamp(0.0, max_offset as f32);
    (target.round() as usize).min(max_offset)
}

pub(crate) fn scroll_offset_for_thumb_center_y(
    track: Bounds<Pixels>,
    y: Pixels,
    total_lines: usize,
    viewport_lines: usize,
) -> usize {
    let max_offset = total_lines.saturating_sub(viewport_lines);
    if max_offset == 0 || track.size.height <= Pixels::ZERO {
        return 0;
    }

    let track_h = track.size.height.max(Pixels::ZERO);
    let thumb_h = if total_lines == 0 || track_h <= Pixels::ZERO {
        track_h
    } else {
        let ratio = (viewport_lines as f32 / total_lines.max(1) as f32).min(1.0);
        (track_h * ratio).max(MIN_THUMB_HEIGHT).min(track_h)
    };
    let y_range = (track_h - thumb_h).max(Pixels::ZERO);
    if y_range <= Pixels::ZERO {
        return 0;
    }

    // Center the thumb at the click point (clamped to track).
    let thumb_y = (y - track.origin.y - thumb_h / 2.0).clamp(Pixels::ZERO, y_range);
    let t_pos = (thumb_y / y_range).clamp(0.0, 1.0);
    (((1.0 - t_pos) * max_offset as f32).round() as usize).min(max_offset)
}

pub(crate) fn paint_overlay_scrollbar(
    sb: &ScrollbarLayoutState,
    markers: &[Pixels],
    active_marker: Option<Pixels>,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    // Keep the scrollbar lane visually crisp: a subtle track + a clearer thumb.
    window.paint_quad(outline(
        sb.bounds,
        cx.theme().foreground.opacity(0.10),
        BorderStyle::Solid,
    ));
    window.paint_quad(fill(sb.track_bounds, cx.theme().foreground.opacity(0.03)));

    // Thumb: indicates the current viewport.
    window.paint_quad(fill(sb.thumb_bounds, cx.theme().selection.opacity(0.8)));
    window.paint_quad(outline(
        sb.thumb_bounds,
        cx.theme().selection.opacity(0.60),
        BorderStyle::Solid,
    ));

    if markers.is_empty() && active_marker.is_none() {
        return;
    }

    let track = sb.track_bounds;
    let min_y = track.origin.y;
    let marker_color = cx.theme().foreground.opacity(0.30);

    for &y in markers {
        let w = MARKER_SIZE.min(track.size.width);
        let h = MARKER_SIZE.min(track.size.height);
        if w <= Pixels::ZERO || h <= Pixels::ZERO {
            break;
        }

        let x = track.origin.x + (track.size.width - w) / 2.0;
        let mut y0 = y - h / 2.0;
        let max_y = track.origin.y + (track.size.height - h).max(Pixels::ZERO);
        if y0 < min_y {
            y0 = min_y;
        } else if y0 > max_y {
            y0 = max_y;
        }

        window.paint_quad(fill(
            Bounds {
                origin: point(x, y0),
                size: size(w, h),
            },
            marker_color,
        ));
    }

    if let Some(y) = active_marker {
        let w = ACTIVE_MARKER_SIZE.min(track.size.width);
        let h = ACTIVE_MARKER_SIZE.min(track.size.height);
        if w > Pixels::ZERO && h > Pixels::ZERO {
            let x = track.origin.x + (track.size.width - w) / 2.0;
            let min_y = track.origin.y;
            let max_y = track.origin.y + (track.size.height - h).max(Pixels::ZERO);
            let mut y0 = y - h / 2.0;
            if y0 < min_y {
                y0 = min_y;
            } else if y0 > max_y {
                y0 = max_y;
            }

            window.paint_quad(fill(
                Bounds {
                    origin: point(x, y0),
                    size: size(w, h),
                },
                cx.theme().foreground.opacity(0.70),
            ));
        }
    }
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
        self.scroll.scrollbar_dragging
    }

    pub(crate) fn scrollbar_hovered(&self) -> bool {
        self.scroll.scrollbar_hovered
    }

    pub(crate) fn scrollbar_revealed(&self) -> bool {
        self.scroll.scrollbar_revealed
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

    pub(crate) fn scrollbar_virtual_offset(&self) -> Option<usize> {
        self.scroll.scrollbar_virtual_offset
    }

    pub(crate) fn scrollbar_drag_origin(&self) -> Option<(Pixels, usize)> {
        Some((
            self.scroll.scrollbar_drag_start_y?,
            self.scroll.scrollbar_drag_start_offset?,
        ))
    }

    pub(crate) fn set_scrollbar_drag_origin(&mut self, mouse_y: Pixels, offset: usize) {
        self.scroll.scrollbar_drag_start_y = Some(mouse_y);
        self.scroll.scrollbar_drag_start_offset = Some(offset);
    }

    pub(crate) fn begin_scrollbar_drag(&mut self, mouse_y: Pixels, cx: &mut Context<Self>) {
        self.scroll.scrollbar_dragging = true;
        let current = {
            let terminal = self.terminal.read(cx);
            terminal.last_content().display_offset
        };
        self.scroll.scrollbar_virtual_offset = Some(current);
        self.scroll.scrollbar_last_target_offset = Some(current);
        self.set_scrollbar_drag_origin(mouse_y, current);
    }

    pub(crate) fn end_scrollbar_drag(&mut self) {
        self.scroll.scrollbar_dragging = false;
        self.scroll.scrollbar_last_target_offset = None;
        self.scroll.scrollbar_virtual_offset = None;
        self.scroll.scrollbar_drag_start_y = None;
        self.scroll.scrollbar_drag_start_offset = None;
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

    pub(crate) fn apply_scrollbar_target_offset(
        &mut self,
        target_offset: usize,
        cx: &mut Context<Self>,
    ) {
        if self.scroll.scrollbar_last_target_offset == Some(target_offset) {
            return;
        }
        self.scroll.scrollbar_last_target_offset = Some(target_offset);
        let current = self.scroll.scrollbar_virtual_offset.unwrap_or_else(|| {
            let terminal = self.terminal.read(cx);
            terminal.last_content().display_offset
        });
        self.scroll_to_display_offset_from_current(current, target_offset, cx);
        self.scroll.scrollbar_virtual_offset = Some(target_offset);
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
    use gpui::{Bounds, point, px, size};

    use super::{
        buffer_index_for_line_coord, scroll_offset_for_line_coord_centered,
        scrollbar_marker_y_for_line_coord, search_match_index_for_scrollbar_click,
        search_match_index_for_scrollbar_hover,
    };
    use crate::GridPoint;

    #[test]
    fn scrollbar_marker_y_maps_entire_buffer_top_to_bottom() {
        let track = Bounds {
            origin: point(px(0.0), px(10.0)),
            size: size(px(8.0), px(100.0)),
        };

        // total_lines=3, viewport_lines=1 -> top_line = -2, bottom_line = 0
        let y_top = scrollbar_marker_y_for_line_coord(track, 3, 1, -2).unwrap();
        let y_mid = scrollbar_marker_y_for_line_coord(track, 3, 1, -1).unwrap();
        let y_bot = scrollbar_marker_y_for_line_coord(track, 3, 1, 0).unwrap();

        // Line-band centers: 1/6, 3/6, 5/6 of the track height.
        assert!(((y_top - px(26.666_7)) / px(1.0)).abs() < 0.01);
        assert!(((y_mid - px(60.0)) / px(1.0)).abs() < 0.01);
        assert!(((y_bot - px(93.333_3)) / px(1.0)).abs() < 0.01);
    }

    #[test]
    fn scrollbar_marker_y_clamps_out_of_range_lines() {
        let track = Bounds {
            origin: point(px(0.0), px(0.0)),
            size: size(px(8.0), px(50.0)),
        };

        // total_lines=3, viewport_lines=1 -> valid coords: -2,-1,0.
        let y_above = scrollbar_marker_y_for_line_coord(track, 3, 1, -99).unwrap();
        let y_below = scrollbar_marker_y_for_line_coord(track, 3, 1, 99).unwrap();

        // Clamps to the first/last line-band centers (1/6 and 5/6 of the height).
        assert!(((y_above - px(8.333_3)) / px(1.0)).abs() < 0.01);
        assert!(((y_below - px(41.666_7)) / px(1.0)).abs() < 0.01);
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
    fn search_match_index_for_scrollbar_click_picks_nearest_marker() {
        let track = Bounds {
            origin: point(px(0.0), px(0.0)),
            size: size(px(10.0), px(100.0)),
        };

        // total_lines=3, viewport_lines=1 => coords: -2,-1,0 map to y at ~16.7,50,83.3.
        let matches = vec![
            GridPoint::new(-2, 0)..=GridPoint::new(-2, 1),
            GridPoint::new(-1, 0)..=GridPoint::new(-1, 1),
            GridPoint::new(0, 0)..=GridPoint::new(0, 1),
        ];

        let hit = search_match_index_for_scrollbar_click(track, 3, 1, &matches, px(51.0), px(4.0));
        assert_eq!(hit, Some(1));

        let miss = search_match_index_for_scrollbar_click(track, 3, 1, &matches, px(51.0), px(0.5));
        assert_eq!(miss, None);
    }

    #[test]
    fn search_match_index_for_scrollbar_hover_picks_nearest_marker() {
        let track = Bounds {
            origin: point(px(0.0), px(0.0)),
            size: size(px(10.0), px(100.0)),
        };

        // total_lines=3, viewport_lines=1 => marker y ~ 16.7, 50, 83.3
        let matches = vec![
            GridPoint::new(-2, 0)..=GridPoint::new(-2, 1),
            GridPoint::new(-1, 0)..=GridPoint::new(-1, 1),
            GridPoint::new(0, 0)..=GridPoint::new(0, 1),
        ];

        let hit = search_match_index_for_scrollbar_hover(track, 3, 1, &matches, px(84.0), px(6.0));
        assert_eq!(hit, Some(2));

        let miss = search_match_index_for_scrollbar_hover(track, 3, 1, &matches, px(84.0), px(0.5));
        assert_eq!(miss, None);
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
