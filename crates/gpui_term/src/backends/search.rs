use unicode_segmentation::UnicodeSegmentation;

use crate::GridPoint;

pub(crate) const MAX_SEARCH_MATCHES: usize = 10_000;

/// A "visible glyph" token on a terminal line.
///
/// Terminals represent some glyphs (eg: CJK, many emoji) as a leading cell with a width > 1,
/// followed by one or more spacer columns. Search matching must operate on these visible glyph
/// tokens rather than raw column adjacency.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SearchToken {
    /// Starting column in the terminal grid.
    pub(crate) start_col: usize,
    /// Display width in terminal columns. Must be >= 1.
    pub(crate) width: usize,
    /// The rendered text for this token (typically a single grapheme cluster).
    pub(crate) text: String,
}

pub(crate) fn push_search_token(
    tokens: &mut Vec<SearchToken>,
    start_col: usize,
    width: usize,
    cols: usize,
    text: impl Into<String>,
) {
    if start_col >= cols {
        return;
    }

    tokens.push(SearchToken {
        start_col,
        width: width.max(1).min(cols - start_col),
        text: text.into(),
    });
}

/// Finds substring matches of `query` within the visible `tokens`.
///
/// Returns a list of inclusive `(start_col, end_col)` ranges in terminal column space.
pub(crate) fn find_search_matches_in_tokens(
    tokens: &[SearchToken],
    query: &str,
    max_matches: usize,
) -> Vec<(usize, usize)> {
    if query.is_empty() || query.chars().all(|c| c.is_whitespace()) {
        return Vec::new();
    }

    // Match using extended grapheme clusters so multi-codepoint glyphs (emoji sequences,
    // combining marks, etc.) behave as the user expects.
    let q_graphemes: Vec<&str> = UnicodeSegmentation::graphemes(query, true).collect();
    if q_graphemes.is_empty() || tokens.is_empty() || q_graphemes.len() > tokens.len() {
        return Vec::new();
    }

    let mut out = Vec::new();
    let max_start = tokens.len() - q_graphemes.len();
    for i in 0..=max_start {
        if out.len() >= max_matches {
            break;
        }

        let mut ok = true;
        for (j, qg) in q_graphemes.iter().enumerate() {
            if tokens[i + j].text != *qg {
                ok = false;
                break;
            }
        }
        if !ok {
            continue;
        }

        let start = tokens[i].start_col;
        let last = &tokens[i + q_graphemes.len() - 1];
        let end = last
            .start_col
            .saturating_add(last.width.max(1).saturating_sub(1));
        out.push((start, end));
    }

    out
}

pub(crate) fn append_search_matches_for_line(
    matches: &mut Vec<std::ops::RangeInclusive<GridPoint>>,
    tokens: &[SearchToken],
    query: &str,
    max_matches: usize,
    line: i32,
) {
    let remaining = max_matches.saturating_sub(matches.len());
    if remaining == 0 {
        return;
    }

    matches.extend(
        find_search_matches_in_tokens(tokens, query, remaining)
            .into_iter()
            .map(|(start_col, end_col)| {
                GridPoint::new(line, start_col)..=GridPoint::new(line, end_col)
            }),
    );
}

pub(crate) fn collect_search_matches_for_lines<T>(
    query: &str,
    lines: impl IntoIterator<Item = T>,
    mut tokens_for_line: impl FnMut(T, &mut Vec<SearchToken>) -> Option<i32>,
) -> Vec<std::ops::RangeInclusive<GridPoint>> {
    let mut matches = Vec::new();
    let mut tokens = Vec::new();
    for line in lines {
        if matches.len() >= MAX_SEARCH_MATCHES {
            break;
        }

        tokens.clear();
        let Some(line_coord) = tokens_for_line(line, &mut tokens) else {
            continue;
        };

        append_search_matches_for_line(
            &mut matches,
            &tokens,
            query,
            MAX_SEARCH_MATCHES,
            line_coord,
        );
    }
    matches
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_multi_cjk_wide_cells() {
        // "中文" typically occupies 4 columns: each glyph is width 2.
        let tokens = vec![
            SearchToken {
                start_col: 0,
                width: 2,
                text: "中".to_string(),
            },
            SearchToken {
                start_col: 2,
                width: 2,
                text: "文".to_string(),
            },
        ];

        assert_eq!(
            find_search_matches_in_tokens(&tokens, "中文", 10),
            vec![(0, 3)]
        );
    }

    #[test]
    fn matches_single_grapheme_with_combining_mark() {
        // "e\u{301}" is a single grapheme cluster (e + combining acute).
        let tokens = vec![SearchToken {
            start_col: 5,
            width: 1,
            text: "e\u{301}".to_string(),
        }];

        assert_eq!(
            find_search_matches_in_tokens(&tokens, "e\u{301}", 10),
            vec![(5, 5)]
        );
    }

    #[test]
    fn matches_single_grapheme_emoji_modifier_sequence() {
        let tokens = vec![SearchToken {
            start_col: 10,
            width: 2,
            text: "👍🏽".to_string(),
        }];

        assert_eq!(
            find_search_matches_in_tokens(&tokens, "👍🏽", 10),
            vec![(10, 11)]
        );
    }
}
