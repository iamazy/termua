use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, ReadGlobal, Styled, Window, div, point,
    px,
};
use gpui_component::ActiveTheme;
use unicode_segmentation::UnicodeSegmentation;

use super::{
    ImeState, TerminalView,
    scrollbar::{
        SCROLLBAR_WIDTH, scroll_offset_for_line_coord_centered, scroll_offset_for_thumb_center_y,
        scrollbar_bounds_for_terminal, scrollbar_track_bounds,
        search_match_index_for_scrollbar_click, search_match_index_for_scrollbar_hover,
        thumb_bounds_for_track,
    },
};
use crate::{settings::TerminalSettings, terminal::SearchClose};

fn search_match_counter(view: &TerminalView, cx: &mut Context<TerminalView>) -> String {
    let terminal = view.terminal.read(cx);
    let match_count = terminal.matches().len();
    let active = terminal
        .active_match_index()
        .map(|ix| ix.saturating_add(1))
        .unwrap_or(0)
        .min(match_count);
    format!("{active}/{match_count}")
}

fn search_panel_width(window: &Window) -> Pixels {
    let viewport = window.viewport_size();
    px(520.0)
        .min((viewport.width - px(24.0)).max(Pixels::ZERO))
        .max(px(320.0).min(viewport.width.max(Pixels::ZERO)))
}

fn marked_text(view: &TerminalView) -> String {
    view.search
        .search_ime_state
        .as_ref()
        .map(|s| s.marked_text.clone())
        .unwrap_or_default()
}

fn show_placeholder(view: &TerminalView, marked: &str) -> bool {
    view.search.search.text().is_empty() && marked.is_empty()
}

fn on_search_backdrop_left_mouse_down(
    this: &mut TerminalView,
    e: &MouseDownEvent,
    window: &mut Window,
    cx: &mut Context<TerminalView>,
) {
    if TerminalSettings::global(cx).show_scrollbar {
        let term_bounds = {
            let terminal = this.terminal.read(cx);
            terminal.last_content().terminal_bounds.bounds
        };
        let sb_bounds = scrollbar_bounds_for_terminal(term_bounds, SCROLLBAR_WIDTH);
        if sb_bounds.contains(&e.position) {
            // Allow scrollbar interaction while searching; do not dismiss.
            let track = scrollbar_track_bounds(sb_bounds);
            let (total_lines, viewport_lines, current_offset) = {
                let terminal = this.terminal.read(cx);
                let content = terminal.last_content();
                (
                    terminal.total_lines(),
                    terminal.viewport_lines(),
                    content.display_offset,
                )
            };

            this.set_scrollbar_hovered(true, cx);
            this.begin_scrollbar_drag(e.position.y, cx);
            this.set_mouse_left_down_in_terminal(false);

            let thumb_bounds =
                thumb_bounds_for_track(track, total_lines, viewport_lines, current_offset);

            if !thumb_bounds.contains(&e.position) {
                let marker_hit_radius = px(7.0);
                let match_idx = {
                    let terminal = this.terminal.read(cx);
                    search_match_index_for_scrollbar_click(
                        track,
                        total_lines,
                        viewport_lines,
                        terminal.matches(),
                        e.position.y,
                        marker_hit_radius,
                    )
                };

                let target_offset = if let Some(match_idx) = match_idx {
                    let line = {
                        let terminal = this.terminal.read(cx);
                        terminal.matches()[match_idx].start().line
                    };
                    let target_offset =
                        scroll_offset_for_line_coord_centered(total_lines, viewport_lines, line);
                    this.terminal.update(cx, |term, _| {
                        term.activate_match(match_idx);
                    });
                    target_offset
                } else {
                    scroll_offset_for_thumb_center_y(
                        track,
                        e.position.y,
                        total_lines,
                        viewport_lines,
                    )
                };

                this.apply_scrollbar_target_offset(target_offset, cx);
                this.set_scrollbar_drag_origin(e.position.y, target_offset);
            }

            cx.stop_propagation();
            return;
        }
    }

    this.close_search(&SearchClose, window, cx);
    cx.stop_propagation();
}

fn on_search_backdrop_mouse_move(
    this: &mut TerminalView,
    e: &MouseMoveEvent,
    window: &mut Window,
    cx: &mut Context<TerminalView>,
) {
    let panel_dragging = this.search.search_panel_dragging;
    if !panel_dragging {
        if TerminalSettings::global(cx).show_scrollbar {
            let term_bounds = {
                let terminal = this.terminal.read(cx);
                terminal.last_content().terminal_bounds.bounds
            };
            let sb_bounds = scrollbar_bounds_for_terminal(term_bounds, SCROLLBAR_WIDTH);
            if sb_bounds.contains(&e.position) {
                let track = scrollbar_track_bounds(sb_bounds);
                let match_idx = {
                    let terminal = this.terminal.read(cx);
                    search_match_index_for_scrollbar_hover(
                        track,
                        terminal.total_lines(),
                        terminal.viewport_lines(),
                        terminal.matches(),
                        e.position.y,
                        px(7.0),
                    )
                };
                if let Some(match_idx) = match_idx {
                    this.set_scrollbar_preview_for_match(match_idx, e.position, cx);
                } else {
                    this.clear_scrollbar_preview(cx);
                }
            } else {
                this.clear_scrollbar_preview(cx);
            }
        } else {
            this.clear_scrollbar_preview(cx);
        }
    } else {
        // Don't show preview while dragging the search panel.
        this.clear_scrollbar_preview(cx);
    }

    if !panel_dragging {
        return;
    }

    let search = &mut this.search;
    if !e.dragging() {
        search.search_panel_dragging = false;
        search.search_panel_drag_start_mouse = None;
        search.search_panel_drag_start_pos = None;
        return;
    }

    let (Some(start_mouse), Some(start_pos)) = (
        search.search_panel_drag_start_mouse,
        search.search_panel_drag_start_pos,
    ) else {
        return;
    };

    let dx = e.position.x - start_mouse.x;
    let dy = e.position.y - start_mouse.y;
    let mut next = point(start_pos.x + dx, start_pos.y + dy);

    let viewport = window.viewport_size();
    let keep = px(32.0);
    let panel_w = search_panel_width(window);

    // Allow the panel to be dragged mostly offscreen, but keep a small grab area
    // visible so it can't be lost.
    next.x = next
        .x
        .clamp((Pixels::ZERO - panel_w) + keep, viewport.width - keep);
    next.y = next.y.clamp(px(0.0), viewport.height - keep);

    search.search_panel_pos = next;
    cx.notify();
    cx.stop_propagation();
}

fn on_search_backdrop_left_mouse_up(
    this: &mut TerminalView,
    _: &MouseUpEvent,
    _window: &mut Window,
    cx: &mut Context<TerminalView>,
) {
    let search = &mut this.search;
    search.search_panel_dragging = false;
    search.search_panel_drag_start_mouse = None;
    search.search_panel_drag_start_pos = None;
    this.end_scrollbar_drag();
    cx.stop_propagation();
}

fn on_search_backdrop_right_mouse_down(
    this: &mut TerminalView,
    e: &MouseDownEvent,
    window: &mut Window,
    cx: &mut Context<TerminalView>,
) {
    if TerminalSettings::global(cx).show_scrollbar {
        let term_bounds = {
            let terminal = this.terminal.read(cx);
            terminal.last_content().terminal_bounds.bounds
        };
        let sb_bounds = scrollbar_bounds_for_terminal(term_bounds, SCROLLBAR_WIDTH);
        if sb_bounds.contains(&e.position) {
            // Do not dismiss on scrollbar right-click either.
            cx.stop_propagation();
            return;
        }
    }

    this.close_search(&SearchClose, window, cx);
    cx.stop_propagation();
}

struct SearchPanelColors {
    panel_bg: gpui::Hsla,
    panel_fg: gpui::Hsla,
    panel_border: gpui::Hsla,
    input_bg: gpui::Hsla,
    input_border: gpui::Hsla,
    hint_fg: gpui::Hsla,
    button_bg: gpui::Hsla,
    plain_text_fg: gpui::Hsla,
    composing_fg: gpui::Hsla,
}

fn search_panel_colors(cx: &mut Context<TerminalView>) -> SearchPanelColors {
    // Avoid holding an immutable borrow of `cx` across `cx.listener` calls.
    let theme = cx.theme();
    SearchPanelColors {
        panel_bg: theme.popover,
        panel_fg: theme.popover_foreground,
        panel_border: theme.border.opacity(0.9),
        input_bg: theme.muted.opacity(0.9),
        input_border: theme.input.opacity(0.9),
        hint_fg: theme.muted_foreground,
        button_bg: theme.background.opacity(0.15),
        plain_text_fg: theme.foreground,
        composing_fg: theme.accent_foreground,
    }
}

fn render_search_query_line(
    view: &TerminalView,
    marked: &str,
    show_placeholder: bool,
    composing_fg: gpui::Hsla,
) -> AnyElement {
    if show_placeholder {
        return div()
            .whitespace_nowrap()
            .child("Type to search...")
            .into_any_element();
    }

    // Render the query + composing text + caret on a single baseline-aligned row.
    let (prefix, suffix) = view.search.search.split_at_cursor();
    let mut line = div()
        .flex()
        .items_end()
        .whitespace_nowrap()
        .overflow_hidden()
        .child(prefix.to_string());
    if !marked.is_empty() {
        line = line.child(div().text_color(composing_fg).child(marked.to_string()));
    }
    line.child("|").child(suffix.to_string()).into_any_element()
}

fn render_search_nav_button(
    label: &'static str,
    panel_border: gpui::Hsla,
    button_bg: gpui::Hsla,
    forward: bool,
    cx: &mut Context<TerminalView>,
) -> AnyElement {
    div()
        .cursor_pointer()
        .rounded_md()
        .border_1()
        .border_color(panel_border)
        .bg(button_bg)
        .px(px(10.0))
        .py(px(6.0))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _, _, cx| {
                this.jump_search(forward, cx);
                cx.stop_propagation();
            }),
        )
        .child(label)
        .into_any_element()
}

fn render_search_panel(
    view: &TerminalView,
    counter: String,
    marked: String,
    show_placeholder: bool,
    panel_w: Pixels,
    cx: &mut Context<TerminalView>,
) -> AnyElement {
    let colors = search_panel_colors(cx);

    let pos = view.search.search_panel_pos;

    let query_line = render_search_query_line(view, &marked, show_placeholder, colors.composing_fg);

    div()
        .id("terminal-search-panel")
        .absolute()
        .left(pos.x)
        .top(pos.y)
        .w(panel_w)
        .max_w(px(720.0))
        .bg(colors.panel_bg)
        .text_color(colors.panel_fg)
        .border_1()
        .border_color(colors.panel_border)
        .rounded_lg()
        .shadow_lg()
        .p(px(12.0))
        // Prevent "click outside to close" from firing when clicking the panel.
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|_, _, _, cx| cx.stop_propagation()),
        )
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(|_, _, _, cx| cx.stop_propagation()),
        )
        .child(
            div()
                .mt(px(10.0))
                .bg(colors.input_bg)
                .border_1()
                .border_color(colors.input_border)
                .rounded_md()
                .px(px(10.0))
                .py(px(8.0))
                .text_color(if show_placeholder {
                    colors.hint_fg
                } else {
                    colors.plain_text_fg
                })
                .child(query_line),
        )
        .child(
            div()
                .cursor_move()
                .mt(px(10.0))
                .flex()
                .items_center()
                .justify_between()
                // The cursor indicates this row is draggable; ensure the whole row
                // starts a drag (not just the hint text).
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, e: &MouseDownEvent, _, cx| {
                        let search = &mut this.search;
                        search.search_panel_dragging = true;
                        search.search_panel_drag_start_mouse = Some(e.position);
                        search.search_panel_drag_start_pos = Some(search.search_panel_pos);
                        cx.stop_propagation();
                    }),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(colors.hint_fg)
                        .child("Enter: next  Shift+Enter: prev  Esc: close"),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(render_search_nav_button(
                            "<",
                            colors.panel_border,
                            colors.button_bg,
                            false,
                            cx,
                        ))
                        .child(render_search_nav_button(
                            ">",
                            colors.panel_border,
                            colors.button_bg,
                            true,
                            cx,
                        ))
                        .child(div().text_sm().text_color(colors.hint_fg).child(counter)),
                ),
        )
        .into_any_element()
}

/// A small, testable text buffer used by the terminal search.
///
/// - Cursor is stored as a byte index into `text`, always on a grapheme boundary.
/// - Editing operations are grapheme-aware, so emoji/CJK/combining marks behave naturally.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct SearchBuffer {
    text: String,
    cursor: usize,
}

impl SearchBuffer {
    pub(crate) fn new() -> Self {
        Self {
            text: String::new(),
            cursor: 0,
        }
    }

    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    pub(crate) fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }

    pub(crate) fn cursor_utf16(&self) -> usize {
        let cursor = self.cursor.min(self.text.len());
        self.text[..cursor].encode_utf16().count()
    }

    pub(crate) fn split_at_cursor(&self) -> (&str, &str) {
        let cursor = self.cursor.min(self.text.len());
        self.text.split_at(cursor)
    }

    pub(crate) fn move_left(&mut self) {
        self.clamp_cursor();
        if self.cursor == 0 {
            return;
        }

        let mut prev = 0usize;
        for (i, _) in UnicodeSegmentation::grapheme_indices(self.text.as_str(), true) {
            if i >= self.cursor {
                break;
            }
            prev = i;
        }
        self.cursor = prev;
    }

    pub(crate) fn move_right(&mut self) {
        self.clamp_cursor();
        if self.cursor >= self.text.len() {
            self.cursor = self.text.len();
            return;
        }

        let mut next = self.text.len();
        for (i, _) in UnicodeSegmentation::grapheme_indices(self.text.as_str(), true) {
            if i > self.cursor {
                next = i;
                break;
            }
        }
        self.cursor = next;
    }

    pub(crate) fn home(&mut self) {
        self.cursor = 0;
    }

    pub(crate) fn end(&mut self) {
        self.cursor = self.text.len();
    }

    pub(crate) fn insert(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        self.clamp_cursor();
        self.text.insert_str(self.cursor, s);
        self.cursor += s.len();
        self.clamp_cursor();
    }

    pub(crate) fn delete_prev(&mut self) -> bool {
        self.clamp_cursor();
        if self.cursor == 0 || self.text.is_empty() {
            return false;
        }

        let mut prev = 0usize;
        for (i, _) in UnicodeSegmentation::grapheme_indices(self.text.as_str(), true) {
            if i >= self.cursor {
                break;
            }
            prev = i;
        }
        if prev == self.cursor {
            return false;
        }
        self.text.replace_range(prev..self.cursor, "");
        self.cursor = prev;
        true
    }

    pub(crate) fn delete_next(&mut self) -> bool {
        self.clamp_cursor();
        let cursor = self.cursor.min(self.text.len());
        if cursor >= self.text.len() {
            return false;
        }

        let mut next = self.text.len();
        for (i, _) in UnicodeSegmentation::grapheme_indices(self.text.as_str(), true) {
            if i > cursor {
                next = i;
                break;
            }
        }
        if next <= cursor {
            return false;
        }

        self.text.replace_range(cursor..next, "");
        true
    }

    fn clamp_cursor(&mut self) {
        self.cursor = self.cursor.min(self.text.len());
        if self.cursor == self.text.len() {
            return;
        }

        // Move the cursor back to the nearest grapheme boundary.
        let mut last = 0usize;
        for (i, _) in UnicodeSegmentation::grapheme_indices(self.text.as_str(), true) {
            if i > self.cursor {
                break;
            }
            last = i;
        }
        self.cursor = last;
    }
}

pub(crate) struct SearchState {
    pub(crate) search_open: bool,
    pub(crate) search: SearchBuffer,
    pub(crate) search_ime_state: Option<ImeState>,
    pub(crate) search_expected_commit: Option<String>,
    pub(crate) search_epoch: usize,
    pub(crate) search_panel_pos: gpui::Point<Pixels>,
    pub(crate) search_panel_pos_initialized: bool,
    pub(crate) search_panel_dragging: bool,
    pub(crate) search_panel_drag_start_mouse: Option<gpui::Point<Pixels>>,
    pub(crate) search_panel_drag_start_pos: Option<gpui::Point<Pixels>>,
}

impl Default for SearchState {
    fn default() -> Self {
        Self {
            search_open: false,
            search: SearchBuffer::new(),
            search_ime_state: None,
            search_expected_commit: None,
            search_epoch: 0,
            search_panel_pos: point(px(0.0), px(0.0)),
            search_panel_pos_initialized: false,
            search_panel_dragging: false,
            search_panel_drag_start_mouse: None,
            search_panel_drag_start_pos: None,
        }
    }
}

pub(crate) fn render_search(
    view: &TerminalView,
    window: &mut Window,
    cx: &mut Context<TerminalView>,
) -> Option<AnyElement> {
    if !view.search.search_open {
        return None;
    }

    let counter = search_match_counter(view, cx);

    let theme = cx.theme();
    // Keep the terminal content visible while searching; the panel itself is the primary visual
    // focus.
    let backdrop = theme.overlay.opacity(0.25);
    let panel_w = search_panel_width(window);
    let marked = marked_text(view);
    let show_placeholder = show_placeholder(view, &marked);

    Some(
        div()
            .id("terminal-search")
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .bottom_0()
            .bg(backdrop)
            .size_full()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(on_search_backdrop_left_mouse_down),
            )
            .on_mouse_move(cx.listener(on_search_backdrop_mouse_move))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(on_search_backdrop_left_mouse_up),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(on_search_backdrop_right_mouse_down),
            )
            .child(render_search_panel(
                view,
                counter,
                marked,
                show_placeholder,
                panel_w,
                cx,
            ))
            .into_any_element(),
    )
}

#[cfg(test)]
mod tests {
    use super::SearchBuffer;

    #[test]
    fn insert_and_move_ascii() {
        let mut b = SearchBuffer::new();
        b.insert("abc");
        assert_eq!(b.text(), "abc");
        b.move_left();
        b.move_left();
        b.insert("X");
        assert_eq!(b.text(), "aXbc");
    }

    #[test]
    fn delete_prev_next_respects_graphemes() {
        // "e\u{301}" is one grapheme; 👍🏽 is also one grapheme cluster.
        let mut b = SearchBuffer::new();
        b.insert("a");
        b.insert("e\u{301}");
        b.insert("👍🏽");
        assert_eq!(b.text(), "ae\u{301}👍🏽");

        assert!(b.delete_prev());
        assert_eq!(b.text(), "ae\u{301}");

        assert!(b.delete_prev());
        assert_eq!(b.text(), "a");

        assert!(!b.delete_next());
    }

    #[test]
    fn cjk_moves_by_character() {
        let mut b = SearchBuffer::new();
        b.insert("中文abc");
        b.home();
        b.move_right();
        b.move_right();
        b.insert("X");
        assert_eq!(b.text(), "中文Xabc");
    }
}
