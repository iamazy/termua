use std::fmt::Write as _;

use gpui::{App, Bounds, Pixels, TextAlign, TextRun, TextStyle, Window, point, px};
use gpui_component::ActiveTheme;

use crate::{
    TerminalMode, point_to_viewport,
    terminal::{Terminal, TerminalBounds},
};

pub(crate) fn should_show_line_numbers(
    show_line_numbers_setting: bool,
    mode: TerminalMode,
) -> bool {
    // TUI apps like vim/top/htop typically use the alternate screen. When active, we hide the
    // line-number gutter to avoid stealing columns and disturbing layout-sensitive UIs.
    show_line_numbers_setting && !mode.contains(TerminalMode::ALT_SCREEN)
}

pub(crate) fn reserve_left_padding_without_line_numbers(
    show_line_numbers_setting: bool,
    mode: TerminalMode,
) -> bool {
    // When the line-number gutter is disabled, we still want a small "breathing room" on the
    // left edge. But for ALT_SCREEN TUIs, do not reserve space (avoid stealing columns).
    !show_line_numbers_setting && !mode.contains(TerminalMode::ALT_SCREEN)
}

pub(crate) fn should_relayout_for_mode_change(
    show_line_numbers_setting: bool,
    previous_mode: TerminalMode,
    mode_after_sync: TerminalMode,
) -> bool {
    // We compute layout once using the previous snapshot mode as a hint, then sync the backend
    // (which updates mode). If either the line number gutter or the left padding policy changes
    // across the sync boundary, do a second layout+sync so the user doesn't see a one-frame shift.
    should_show_line_numbers(show_line_numbers_setting, previous_mode)
        != should_show_line_numbers(show_line_numbers_setting, mode_after_sync)
        || reserve_left_padding_without_line_numbers(show_line_numbers_setting, previous_mode)
            != reserve_left_padding_without_line_numbers(show_line_numbers_setting, mode_after_sync)
}

#[derive(Copy, Clone)]
pub(crate) struct LineNumberState {
    pub(crate) gutter: Pixels,
    pub(crate) line_number_width: Pixels,
    pub(crate) line_number_digits: usize,
}

pub(crate) struct LineNumberPaintData {
    pub(crate) line_numbers: Vec<Option<usize>>,
    pub(crate) last_row_to_number: usize,
}

pub(crate) fn compute_line_number_layout(
    cell_width: Pixels,
    show_line_numbers: bool,
    reserve_left_padding_without_line_numbers: bool,
    total_lines_for_digits: usize,
) -> LineNumberState {
    let line_number_digits = if show_line_numbers {
        digit_count(total_lines_for_digits.max(1))
    } else {
        0
    };

    let line_number_width = if show_line_numbers {
        cell_width * (line_number_digits as f32 + 1.0)
    } else {
        Pixels::ZERO
    };

    // "Gutter" is the horizontal space reserved for line numbers + a small padding.
    // If line numbers are hidden (e.g. ALT_SCREEN apps like vim), do not reserve any space
    // so TUIs can use the full terminal width.
    let gutter = if show_line_numbers {
        cell_width / 3.0 + line_number_width
    } else if reserve_left_padding_without_line_numbers {
        // When line numbers are hidden, still reserve enough space for interaction affordances
        // in the left gutter (e.g. command block selection).
        (cell_width / 3.0).max(px(8.0)).max(px(14.0))
    } else {
        Pixels::ZERO
    };

    LineNumberState {
        gutter,
        line_number_width,
        line_number_digits,
    }
}

fn digit_count(mut n: usize) -> usize {
    // At least 1 digit for 0.
    let mut digits = 1;
    while n >= 10 {
        n /= 10;
        digits += 1;
    }
    digits
}

fn format_line_number(buf: &mut String, line_no: usize, digits: usize) {
    buf.clear();
    let padding = digits.saturating_sub(digit_count(line_no));
    buf.extend(std::iter::repeat_n(' ', padding));
    let _ = write!(buf, "{line_no} ");
}

pub(crate) fn compute_line_number_paint_data(
    terminal: &Terminal,
    display_offset: usize,
    rows: usize,
) -> Option<LineNumberPaintData> {
    let rows = rows.max(1);

    let total_lines = terminal.total_lines();
    let viewport_lines = terminal.viewport_lines().max(1);
    let max_offset = total_lines.saturating_sub(viewport_lines);
    let viewport_top_idx = max_offset.saturating_sub(display_offset);

    let line_numbers = terminal.logical_line_numbers_from_top(viewport_top_idx, rows);
    if line_numbers.is_empty() {
        return None;
    }

    let last_row_to_number = if display_offset == 0 {
        terminal
            .cursor_line_id()
            .and_then(|cursor_id| {
                let top_id = terminal.scrollback_top_line_id();
                let diff = cursor_id - top_id;
                if diff < 0 {
                    return None;
                }
                let cursor_abs_idx = diff as usize;
                cursor_abs_idx.checked_sub(viewport_top_idx)
            })
            .or_else(|| {
                let content = terminal.last_content();
                point_to_viewport(content.display_offset, content.cursor.point).map(|p| p.line)
            })
            .map(|cursor_row| cursor_row.min(rows.saturating_sub(1)))
            .unwrap_or(rows.saturating_sub(1))
    } else {
        rows.saturating_sub(1)
    };

    Some(LineNumberPaintData {
        line_numbers,
        last_row_to_number,
    })
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_line_numbers(
    bounds: Bounds<Pixels>,
    scroll_top: Pixels,
    line_number_state: LineNumberState,
    mode: TerminalMode,
    dimensions: &TerminalBounds,
    line_number_data: Option<&LineNumberPaintData>,
    base_text_style: &TextStyle,
    window: &mut Window,
    cx: &mut App,
) {
    if line_number_state.line_number_width <= Pixels::ZERO
        || line_number_state.line_number_digits == 0
        || mode.contains(TerminalMode::ALT_SCREEN)
    {
        return;
    }

    let ln_origin = bounds.origin
        + point(
            line_number_state.gutter - line_number_state.line_number_width,
            px(0.0),
        )
        - point(px(0.0), scroll_top);

    let font_px = base_text_style.font_size.to_pixels(window.rem_size());
    let fg = cx.theme().foreground.opacity(0.40);
    let font = base_text_style.font();

    let Some(line_number_data) = line_number_data else {
        return;
    };
    let line_numbers = &line_number_data.line_numbers;
    let last_row_to_number = line_number_data.last_row_to_number;
    let rows = dimensions.num_lines().max(1);

    let last_row_to_number = last_row_to_number
        .min(line_numbers.len().saturating_sub(1))
        .min(rows.saturating_sub(1));

    // Only the first visual row of a logical (soft-wrapped) line is numbered.
    for row in 0..=last_row_to_number {
        if let Some(Some(line_no)) = line_numbers.get(row).copied() {
            let mut line_text = String::with_capacity(line_number_state.line_number_digits + 1);
            format_line_number(
                &mut line_text,
                line_no,
                line_number_state.line_number_digits,
            );
            let len = line_text.len();
            let shaped = window.text_system().shape_line(
                line_text.into(),
                font_px,
                &[TextRun {
                    len,
                    font: font.clone(),
                    color: fg,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                }],
                None,
            );

            let pos = ln_origin + point(Pixels::ZERO, dimensions.line_height * row as f32);
            let _ = shaped.paint(
                pos,
                dimensions.line_height,
                TextAlign::Left,
                None,
                window,
                cx,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserves_minimum_gutter_without_line_numbers() {
        // Even when line numbers are hidden, we still want enough left gutter space for
        // interaction affordances (e.g. command block selection).
        let cell_width = px(9.0);
        let state = compute_line_number_layout(cell_width, false, true, 10_000);
        assert!(state.gutter >= px(14.0));
        assert_eq!(state.line_number_width, Pixels::ZERO);
        assert_eq!(state.line_number_digits, 0);
    }

    #[test]
    fn formats_line_number_with_left_padding_and_trailing_space() {
        let mut buf = String::new();
        format_line_number(&mut buf, 42, 4);
        assert_eq!(buf, "  42 ");
    }
}
