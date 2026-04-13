use std::{collections::HashMap, ops::RangeInclusive};

use gpui::{Pixels, ScrollWheelEvent, TouchPhase, px};
use url::Url;

use crate::{GridPoint, HoveredWord, SerialOptions, SshOptions};

pub mod alacritty;
pub mod remote;
pub(crate) mod search;
pub mod ssh;
pub mod wezterm;

#[derive(Debug, Clone, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum PtySource {
    Local {
        env: HashMap<String, String>,
        window_id: u64,
    },
    Ssh {
        env: HashMap<String, String>,
        opts: SshOptions,
    },
    Serial {
        opts: SerialOptions,
    },
}

// Keep scrollback defaults aligned between backends.
pub(crate) const DEFAULT_SCROLLBACK_LINES: usize = 10_000;
pub(crate) const MAX_SCROLLBACK_LINES: usize = 100_000;

// GPUI can emit high-frequency scroll deltas (especially on trackpads). We scale
// deltas on macOS a bit to match typical terminal feel.
#[cfg(target_os = "macos")]
pub(crate) const SCROLL_MULTIPLIER: f32 = 4.;
#[cfg(not(target_os = "macos"))]
pub(crate) const SCROLL_MULTIPLIER: f32 = 1.;

pub(crate) fn hover_id(p: &GridPoint) -> usize {
    // Stable-ish identifier for hover state; avoids allocating hashes.
    let line = p.line as i64 as u64;
    ((line << 32) ^ (p.column as u64)) as usize
}

pub(crate) fn drag_line_delta(
    cursor_pos: gpui::Point<Pixels>,
    region: gpui::Bounds<Pixels>,
    line_height: Pixels,
) -> Option<i32> {
    let top = region.origin.y;
    let bottom = region.bottom_left().y;

    let scroll_lines = if cursor_pos.y < top {
        let scroll_delta = (top - cursor_pos.y).pow(1.1);
        (scroll_delta / line_height).ceil() as i32
    } else if cursor_pos.y > bottom {
        let scroll_delta = -((cursor_pos.y - bottom).pow(1.1));
        (scroll_delta / line_height).floor() as i32
    } else {
        return None;
    };

    Some(scroll_lines.clamp(-3, 3))
}

/// Normalizes GPUI scroll wheel deltas into terminal "line" deltas.
///
/// - Returns `Some(delta_lines)` only for `TouchPhase::Moved` events.
/// - Positive deltas correspond to scrolling "up" (towards older scrollback).
pub(crate) fn determine_scroll_lines(
    scroll_px: &mut Pixels,
    e: &ScrollWheelEvent,
    line_height: Pixels,
    mouse_mode: bool,
    viewport_height: Pixels,
) -> Option<i32> {
    let scroll_multiplier = if mouse_mode { 1.0 } else { SCROLL_MULTIPLIER };

    match e.touch_phase {
        TouchPhase::Started => {
            *scroll_px = px(0.0);
            None
        }
        TouchPhase::Moved => {
            let old_offset = (*scroll_px / line_height) as i32;
            *scroll_px += e.delta.pixel_delta(line_height).y * scroll_multiplier;
            let new_offset = (*scroll_px / line_height) as i32;
            *scroll_px %= viewport_height;
            Some(new_offset - old_offset)
        }
        TouchPhase::Ended => None,
    }
}

/// Extract a conservative URL token from a line of optional characters.
///
/// The `chars` slice is indexed by column; `None` entries are treated as "part of the token"
/// (this allows wide-char spacer cells to be included without breaking the run), but they are
/// not included in the produced token string.
pub(crate) fn url_from_line_chars(
    chars: &[Option<char>],
    hover_col: usize,
) -> Option<(String, RangeInclusive<usize>)> {
    if chars.is_empty() {
        return None;
    }

    let cols = chars.len();
    let mut col = hover_col.min(cols - 1);

    if chars[col].is_none() {
        // If the pointer landed on a spacer cell, prefer the nearest preceding real cell.
        if let Some(prev) = (0..=col).rev().find(|&c| chars[c].is_some()) {
            col = prev;
        } else {
            return None;
        }
    }

    let is_sep = |c: Option<char>| matches!(c, Some(ch) if ch.is_whitespace());
    if is_sep(chars[col]) {
        return None;
    }

    let mut start = col;
    while start > 0 && !is_sep(chars[start - 1]) {
        start -= 1;
    }

    let mut end = col;
    while end + 1 < cols && !is_sep(chars[end + 1]) {
        end += 1;
    }

    // Keep only actual characters (skip spacers).
    let mut token: Vec<(usize, char)> = Vec::with_capacity(end - start + 1);
    for (offset, ch) in chars[start..=end].iter().enumerate() {
        if let Some(ch) = *ch {
            token.push((start + offset, ch));
        }
    }

    // Trim common leading/trailing punctuation to avoid capturing parens/quotes around URLs.
    while matches!(
        token.first().map(|(_, ch)| *ch),
        Some('(' | '[' | '{' | '<' | '"' | '\'')
    ) {
        token.remove(0);
    }
    while matches!(
        token.last().map(|(_, ch)| *ch),
        Some('.' | ',' | ';' | ':' | '!' | '?' | ')' | ']' | '}' | '>' | '"' | '\'')
    ) {
        token.pop();
    }

    if token.is_empty() {
        return None;
    }

    let url_text: String = token.iter().map(|(_, ch)| *ch).collect();
    let parsed = Url::parse(&url_text).ok().or_else(|| {
        if url_text.starts_with("www.") {
            Url::parse(&format!("http://{url_text}")).ok()
        } else {
            None
        }
    })?;

    let start = token.first()?.0;
    let end = token.last()?.0;
    Some((parsed.to_string(), start..=end))
}

pub(crate) fn hovered_url_from_line_chars(
    chars: &[Option<char>],
    hover_col: usize,
    line: i32,
) -> Option<(String, RangeInclusive<GridPoint>)> {
    let (url, col_range) = url_from_line_chars(chars, hover_col)?;
    let start = GridPoint::new(line, *col_range.start());
    let end = GridPoint::new(line, *col_range.end());
    Some((url, start..=end))
}

pub(crate) fn collect_line_chars(
    cols: usize,
    cells: impl IntoIterator<Item = (usize, char)>,
) -> Vec<Option<char>> {
    let mut chars = vec![None; cols];
    for (idx, ch) in cells {
        if idx < cols {
            chars[idx] = Some(ch);
        }
    }
    chars
}

pub(crate) fn sync_search_matches(
    dirty: &mut bool,
    query: Option<&str>,
    matches: &mut Vec<RangeInclusive<GridPoint>>,
    active_match: &mut Option<usize>,
    compute_matches: impl FnOnce(&str) -> Vec<RangeInclusive<GridPoint>>,
) {
    if !*dirty {
        return;
    }
    *dirty = false;

    let Some(query) = query else {
        matches.clear();
        *active_match = None;
        return;
    };

    let new_matches = compute_matches(query);
    *active_match = (!new_matches.is_empty()).then_some(0);
    *matches = new_matches;
}

pub(crate) fn update_hovered_word(
    last_hovered_word: &mut Option<HoveredWord>,
    hovered: Option<(String, RangeInclusive<GridPoint>)>,
) -> Option<Option<String>> {
    match hovered {
        Some((url, range)) => {
            let changed = last_hovered_word
                .as_ref()
                .map(|word| (&word.word, &word.word_match))
                != Some((&url, &range));

            if !changed {
                return None;
            }

            let id = hover_id(range.start());
            *last_hovered_word = Some(HoveredWord {
                word: url.clone(),
                word_match: range,
                id,
            });
            Some(Some(url))
        }
        None => last_hovered_word.take().map(|_| None),
    }
}
