use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
};

use gpui::Global;

use crate::{TerminalContent, command_blocks::CommandBlock};

pub trait SuggestionHistoryProvider: Send + Sync + 'static {
    fn seed(&self) -> Vec<String>;
    fn append(&self, command: &str);
}

pub trait SuggestionStaticProvider: Send + Sync + 'static {
    fn for_each_candidate(&self, first_word: &str, f: &mut dyn FnMut(&str, Option<&str>));
}

#[derive(Default)]
pub struct SuggestionHistoryConfig {
    pub provider: Option<Arc<dyn SuggestionHistoryProvider>>,
}

impl Global for SuggestionHistoryConfig {}

#[derive(Default)]
pub struct SuggestionStaticConfig {
    pub provider: Option<Arc<dyn SuggestionStaticProvider>>,
    pub epoch: u64,
}

impl Global for SuggestionStaticConfig {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SuggestionItem {
    pub full_text: String,
    pub score: i32,
    pub description: Option<String>,
}

#[derive(Debug)]
pub struct HistoryStore {
    capacity: usize,
    entries: VecDeque<String>,
}

impl HistoryStore {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            entries: VecDeque::new(),
        }
    }

    pub fn push(&mut self, command: String) -> bool {
        let command = command.trim().to_string();
        if command.is_empty() {
            return false;
        }

        if self.entries.back().is_some_and(|v| v == &command) {
            return false;
        }

        self.entries.push_back(command);
        while self.entries.len() > self.capacity.max(1) {
            self.entries.pop_front();
        }
        true
    }
}

pub struct SuggestionEngine {
    pub history: HistoryStore,
    pub max_items: usize,
    static_provider: Option<Arc<dyn SuggestionStaticProvider>>,
}

impl std::fmt::Debug for SuggestionEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SuggestionEngine")
            .field("history", &self.history)
            .field("max_items", &self.max_items)
            .field("has_static_provider", &self.static_provider.is_some())
            .finish()
    }
}

impl SuggestionEngine {
    pub fn new(history_capacity: usize, max_items: usize) -> Self {
        Self {
            history: HistoryStore::new(history_capacity),
            max_items,
            static_provider: None,
        }
    }

    pub fn set_static_provider(&mut self, provider: Option<Arc<dyn SuggestionStaticProvider>>) {
        self.static_provider = provider;
    }

    pub fn suggest(&self, input_prefix: &str) -> Vec<SuggestionItem> {
        let input_prefix = input_prefix.trim_start();
        if input_prefix.is_empty() {
            return Vec::new();
        }

        let mut meta_by_full_text: HashMap<String, SuggestionMeta> =
            HashMap::with_capacity(self.history.entries.len().min(256) + 16);

        // History suggestions: most recent first.
        for (i, candidate) in self.history.entries.iter().rev().enumerate() {
            push_candidate(
                &mut meta_by_full_text,
                input_prefix,
                candidate,
                1000 - i as i32,
                None,
            );
        }

        // Static suggestions.
        let first_word = input_prefix.split_whitespace().next().unwrap_or("");
        if !first_word.is_empty() {
            // Runtime-loaded static templates (e.g. suggestions.d/*.json).
            if let Some(provider) = self.static_provider.as_ref() {
                provider.for_each_candidate(first_word, &mut |candidate, desc| {
                    push_candidate(&mut meta_by_full_text, input_prefix, candidate, 110, desc);
                });
            }
        }

        let mut out: Vec<SuggestionItem> = meta_by_full_text
            .into_iter()
            .map(|(full_text, meta)| SuggestionItem {
                full_text,
                score: meta.score,
                description: meta.description,
            })
            .collect();
        out.sort_by(|a, b| {
            b.score
                .cmp(&a.score)
                .then_with(|| a.full_text.cmp(&b.full_text))
        });
        out.truncate(self.max_items.max(1));
        out
    }
}

#[derive(Debug)]
struct SuggestionMeta {
    score: i32,
    description: Option<String>,
}

pub fn extract_cursor_line_prefix(content: &TerminalContent) -> String {
    let cursor_line = content.cursor.point.line;
    let cursor_col = content.cursor.point.column;
    if cursor_col == 0 {
        return String::new();
    }

    let Some(line_cells) = cells_for_line(&content.cells, cursor_line) else {
        // Fallback: if the backend ever produces an unexpected ordering, prefer correctness.
        return extract_cursor_line_prefix_slow(content);
    };

    let mut out = String::with_capacity(cursor_col);
    let mut expected_col = 0usize;
    for ic in line_cells.iter() {
        let col = ic.point.column;
        if col >= cursor_col {
            break;
        }

        while expected_col < col && expected_col < cursor_col {
            out.push(' ');
            expected_col += 1;
        }

        if expected_col >= cursor_col {
            break;
        }

        out.push(ic.c);
        if let Some(zw) = ic.zerowidth() {
            out.extend(zw.iter().copied());
        }
        expected_col += 1;
    }

    while expected_col < cursor_col {
        out.push(' ');
        expected_col += 1;
    }

    out
}

pub fn extract_cursor_line_suffix(content: &TerminalContent) -> String {
    let cursor_line = content.cursor.point.line;
    let cursor_col = content.cursor.point.column;

    let Some(line_cells) = cells_for_line(&content.cells, cursor_line) else {
        return extract_cursor_line_suffix_slow(content);
    };

    let mut out = String::new();
    let mut expected_col = cursor_col;
    for ic in line_cells.iter() {
        let col = ic.point.column;
        if col < cursor_col {
            continue;
        }

        while expected_col < col {
            out.push(' ');
            expected_col += 1;
        }

        out.push(ic.c);
        if let Some(zw) = ic.zerowidth() {
            out.extend(zw.iter().copied());
        }
        expected_col = col.saturating_add(1);
    }

    out
}

#[cfg(test)]
pub fn cursor_at_eol(content: &TerminalContent) -> bool {
    let cursor_line = content.cursor.point.line;
    let cursor_col = content.cursor.point.column;
    let Some(line_cells) = cells_for_line(&content.cells, cursor_line) else {
        return cursor_at_eol_slow(content);
    };

    for ic in line_cells.iter() {
        if ic.point.column < cursor_col {
            continue;
        }

        let is_non_space = ic.c != ' ' || ic.zerowidth().is_some_and(|zw| !zw.is_empty());
        if is_non_space {
            return false;
        }
    }

    true
}

pub fn compute_insert_suffix_for_line(
    line_prefix: &str,
    prompt_prefix: Option<&str>,
    full_text: &str,
) -> Option<(String, String)> {
    if let Some(prompt_prefix) = prompt_prefix {
        let input_prefix = line_prefix.strip_prefix(prompt_prefix).unwrap_or("");
        if let Some(suffix) = compute_insert_suffix(input_prefix, full_text) {
            return Some((input_prefix.to_string(), suffix));
        }
    }

    for start in line_prefix
        .char_indices()
        .map(|(idx, _)| idx)
        .chain(std::iter::once(line_prefix.len()))
    {
        let candidate = &line_prefix[start..];
        if let Some(suffix) = compute_insert_suffix(candidate, full_text) {
            return Some((candidate.trim_start().to_string(), suffix));
        }
    }

    None
}

pub fn line_is_suggestion_prefix(line: &str, full_text: &str) -> bool {
    loose_prefix_match_end(full_text, line.trim_start()).is_some()
}

fn cells_for_line(cells: &[crate::IndexedCell], line: i32) -> Option<&[crate::IndexedCell]> {
    if cells.is_empty() {
        return None;
    }

    // `build_plan` relies on display-order (non-decreasing line numbers). Suggestions use the
    // same cell stream, so we can binary-search by line to avoid scanning the entire grid.
    let start = cells.partition_point(|c| c.point.line < line);
    if start >= cells.len() || cells[start].point.line != line {
        return None;
    }
    let end = cells.partition_point(|c| c.point.line <= line);
    Some(&cells[start..end])
}

fn extract_cursor_line_prefix_slow(content: &TerminalContent) -> String {
    let cursor_line = content.cursor.point.line;
    let cursor_col = content.cursor.point.column;

    let mut by_col: Vec<Option<(char, Vec<char>)>> = vec![None; cursor_col];
    for ic in content.cells.iter() {
        if ic.point.line != cursor_line {
            continue;
        }
        if ic.point.column >= cursor_col {
            continue;
        }
        by_col[ic.point.column] = Some((ic.c, ic.zerowidth.clone()));
    }

    let mut out = String::new();
    for cell in &by_col {
        match cell.as_ref() {
            Some((ch, zw)) => {
                out.push(*ch);
                out.extend(zw.iter().copied());
            }
            None => out.push(' '),
        }
    }
    out
}

fn extract_cursor_line_suffix_slow(content: &TerminalContent) -> String {
    let cursor_line = content.cursor.point.line;
    let cursor_col = content.cursor.point.column;

    let max_col = content
        .cells
        .iter()
        .filter(|ic| ic.point.line == cursor_line)
        .map(|ic| ic.point.column)
        .max()
        .map(|v| v.saturating_add(1))
        .unwrap_or(cursor_col);

    let mut by_col: Vec<Option<(char, Vec<char>)>> = vec![None; max_col.saturating_sub(cursor_col)];
    for ic in content.cells.iter() {
        if ic.point.line != cursor_line || ic.point.column < cursor_col {
            continue;
        }
        by_col[ic.point.column - cursor_col] = Some((ic.c, ic.zerowidth.clone()));
    }

    let mut out = String::new();
    for cell in &by_col {
        match cell.as_ref() {
            Some((ch, zw)) => {
                out.push(*ch);
                out.extend(zw.iter().copied());
            }
            None => out.push(' '),
        }
    }
    out
}

#[cfg(test)]
fn cursor_at_eol_slow(content: &TerminalContent) -> bool {
    let cursor_line = content.cursor.point.line;
    let cursor_col = content.cursor.point.column;

    for ic in content.cells.iter() {
        if ic.point.line != cursor_line {
            continue;
        }
        if ic.point.column < cursor_col {
            continue;
        }

        let is_non_space = ic.c != ' ' || !ic.zerowidth.is_empty();
        if is_non_space {
            return false;
        }
    }

    true
}

pub(crate) fn drain_successful_history_commands(
    pending: &mut VecDeque<String>,
    last_seen_block_id: &mut u64,
    blocks: &[CommandBlock],
) -> Vec<String> {
    let old_last_seen = *last_seen_block_id;
    let mut out = Vec::<String>::new();

    let mut new_last_seen = old_last_seen;
    for block in blocks {
        if block.id <= old_last_seen {
            continue;
        }
        if block.ended_at.is_none() {
            continue;
        }

        new_last_seen = new_last_seen.max(block.id);
        let Some(cmd) = pending.pop_front() else {
            continue;
        };

        if block.exit_code == Some(0) {
            out.push(cmd);
        }
    }

    *last_seen_block_id = new_last_seen;
    out
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum SelectionMove {
    Up,
    Down,
}

pub fn move_selection_opt(
    selected: Option<usize>,
    items_len: usize,
    dir: SelectionMove,
) -> Option<usize> {
    if items_len == 0 {
        return None;
    }

    let last = items_len.saturating_sub(1);
    match (selected, dir) {
        (None, SelectionMove::Down) => Some(0),
        (None, SelectionMove::Up) => Some(last),
        (Some(selected), dir) => Some(move_selection(selected, items_len, dir)),
    }
}

pub fn move_selection(selected: usize, items_len: usize, dir: SelectionMove) -> usize {
    if items_len == 0 {
        return 0;
    }

    let last = items_len.saturating_sub(1);
    match dir {
        SelectionMove::Up => selected.saturating_sub(1).min(last),
        SelectionMove::Down => (selected.saturating_add(1)).min(last),
    }
}

pub fn compute_insert_suffix(current_prefix: &str, full_text: &str) -> Option<String> {
    let current_prefix = current_prefix.trim_start();
    if current_prefix.is_empty() {
        return None;
    }

    let end = loose_prefix_match_end(full_text, current_prefix)?;
    if end >= full_text.len() {
        return None;
    }

    let suffix = full_text[end..].to_string();
    (!suffix.is_empty()).then_some(suffix)
}

fn push_candidate(
    meta_by_full_text: &mut HashMap<String, SuggestionMeta>,
    input_prefix: &str,
    candidate: &str,
    score: i32,
    description: Option<&str>,
) {
    let Some(end) = loose_prefix_match_end(candidate, input_prefix) else {
        return;
    };

    if end >= candidate.len() {
        return;
    }

    let description = description.map(str::trim).filter(|s| !s.is_empty());

    match meta_by_full_text.get_mut(candidate) {
        Some(existing) => {
            if existing.score >= score {
                if existing.description.is_none()
                    && let Some(description) = description
                {
                    existing.description = Some(description.to_string());
                }
                return;
            }

            existing.score = score;
            if let Some(description) = description {
                let should_replace = match existing.description.as_deref() {
                    None => true,
                    Some(v) => description.len() > v.len(),
                };
                if should_replace {
                    existing.description = Some(description.to_string());
                }
            }
        }
        None => {
            meta_by_full_text.insert(
                candidate.to_string(),
                SuggestionMeta {
                    score,
                    description: description.map(|s| s.to_string()),
                },
            );
        }
    }
}

fn loose_prefix_match_end(full_text: &str, prefix: &str) -> Option<usize> {
    if full_text.is_ascii() && prefix.is_ascii() {
        return loose_prefix_match_end_ascii(full_text.as_bytes(), prefix.as_bytes());
    }

    let mut full = full_text.char_indices().peekable();
    let mut prefix = prefix.chars().peekable();

    let mut matched_end = 0usize;

    while let Some(&p) = prefix.peek() {
        if p.is_whitespace() {
            while prefix.peek().is_some_and(|c| c.is_whitespace()) {
                prefix.next();
            }

            let mut consumed_any = false;
            while let Some(&(idx, f)) = full.peek() {
                if !f.is_whitespace() {
                    break;
                }
                consumed_any = true;
                matched_end = idx + f.len_utf8();
                full.next();
            }

            if !consumed_any {
                return None;
            }
            continue;
        }

        let (idx, f) = full.next()?;
        if f != p {
            return None;
        }

        matched_end = idx + f.len_utf8();
        prefix.next();
    }

    Some(matched_end)
}

fn loose_prefix_match_end_ascii(full: &[u8], prefix: &[u8]) -> Option<usize> {
    let mut i = 0usize;
    let mut j = 0usize;
    let mut matched_end = 0usize;

    while j < prefix.len() {
        if prefix[j].is_ascii_whitespace() {
            while j < prefix.len() && prefix[j].is_ascii_whitespace() {
                j += 1;
            }

            let start_i = i;
            while i < full.len() && full[i].is_ascii_whitespace() {
                i += 1;
            }
            if i == start_i {
                return None;
            }

            matched_end = i;
            continue;
        }

        if i >= full.len() {
            return None;
        }
        if full[i] != prefix[j] {
            return None;
        }
        i += 1;
        j += 1;
        matched_end = i;
    }

    Some(matched_end)
}

#[cfg(test)]
mod history_drain_tests {
    use super::*;

    fn block(id: u64, exit_code: Option<i32>, ended: bool) -> CommandBlock {
        CommandBlock {
            id,
            started_at: std::time::Instant::now(),
            ended_at: ended.then_some(std::time::Instant::now()),
            exit_code,
            command: None,
            output_start_line: 0,
            output_end_line: None,
        }
    }

    #[test]
    fn drains_only_successful_commands_in_order() {
        let mut pending = VecDeque::from(["ok".to_string(), "bad".to_string(), "ok2".to_string()]);
        let mut last_seen = 0u64;
        let blocks = vec![
            block(1, Some(0), true),
            block(2, Some(1), true),
            block(3, Some(0), true),
        ];

        let out = drain_successful_history_commands(&mut pending, &mut last_seen, &blocks);
        assert_eq!(out, vec!["ok".to_string(), "ok2".to_string()]);
        assert!(pending.is_empty());
        assert_eq!(last_seen, 3);
    }

    #[test]
    fn does_not_drain_for_running_blocks() {
        let mut pending = VecDeque::from(["x".to_string()]);
        let mut last_seen = 0u64;
        let blocks = vec![block(1, None, false)];

        let out = drain_successful_history_commands(&mut pending, &mut last_seen, &blocks);
        assert!(out.is_empty());
        assert_eq!(pending.len(), 1);
        assert_eq!(last_seen, 0);
    }

    #[test]
    fn drains_when_running_block_later_finishes() {
        let mut pending = VecDeque::from(["x".to_string()]);
        let mut last_seen = 0u64;

        let out = drain_successful_history_commands(
            &mut pending,
            &mut last_seen,
            &[block(1, None, false)],
        );
        assert!(out.is_empty());
        assert_eq!(pending.len(), 1);
        assert_eq!(last_seen, 0);

        let out = drain_successful_history_commands(
            &mut pending,
            &mut last_seen,
            &[block(1, Some(0), true)],
        );
        assert_eq!(out, vec!["x".to_string()]);
        assert!(pending.is_empty());
        assert_eq!(last_seen, 1);
    }
}

#[cfg(test)]
mod selection_tests {
    use super::*;

    #[test]
    fn move_selection_opt_none_down_selects_first() {
        assert_eq!(move_selection_opt(None, 3, SelectionMove::Down), Some(0));
    }

    #[test]
    fn move_selection_opt_none_up_selects_last() {
        assert_eq!(move_selection_opt(None, 3, SelectionMove::Up), Some(2));
    }

    #[test]
    fn move_selection_opt_saturates_bounds() {
        assert_eq!(move_selection_opt(Some(0), 3, SelectionMove::Up), Some(0));
        assert_eq!(move_selection_opt(Some(2), 3, SelectionMove::Down), Some(2));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestStaticProvider;

    impl SuggestionStaticProvider for TestStaticProvider {
        fn for_each_candidate(&self, first_word: &str, f: &mut dyn FnMut(&str, Option<&str>)) {
            if first_word == "ls" {
                f("ls -al", Some("List directory contents"));
            }
        }
    }

    #[test]
    fn static_provider_is_used_when_set() {
        let mut engine = SuggestionEngine::new(50, 8);
        engine.set_static_provider(Some(std::sync::Arc::new(TestStaticProvider)));
        let out = engine.suggest("ls");
        let item = out
            .iter()
            .find(|s| s.full_text == "ls -al")
            .expect("expected ls -al");
        assert_eq!(item.description.as_deref(), Some("List directory contents"));
    }

    #[test]
    fn no_builtin_hints_when_no_static_provider() {
        let engine = SuggestionEngine::new(50, 8);
        let out = engine.suggest("ls");
        assert!(
            out.is_empty(),
            "expected no built-in suggestions by default"
        );
    }

    #[test]
    fn history_extends_prefix_append_only() {
        let mut engine = SuggestionEngine::new(50, 8);
        engine.history.push("git status".to_string());

        let out = engine.suggest("g");
        assert!(
            out.iter().any(|s| s.full_text == "git status"),
            "expected `git status` to extend `g`"
        );

        let out = engine.suggest("git status");
        assert!(
            !out.iter().any(|s| s.full_text == "git status"),
            "expected exact matches to not be suggested (append-only)"
        );
    }

    #[test]
    fn history_suggests_when_prefix_has_extra_spaces() {
        let mut engine = SuggestionEngine::new(50, 8);
        engine.history.push("git status".to_string());

        let out = engine.suggest("git    st");
        assert!(
            out.iter().any(|s| s.full_text == "git status"),
            "expected `git    st` to match `git status`"
        );
    }

    #[test]
    fn dedup_prefers_higher_score() {
        let mut engine = SuggestionEngine::new(50, 8);
        engine.history.push("ls --all".to_string());
        engine.set_static_provider(Some(std::sync::Arc::new(TestStaticProvider)));

        let out = engine.suggest("ls");
        let item = out
            .into_iter()
            .find(|s| s.full_text == "ls --all")
            .expect("expected ls --all");
        assert!(item.score >= 900, "expected history to win dedup by score");
    }

    #[test]
    fn history_is_ranked_by_recency() {
        let mut engine = SuggestionEngine::new(50, 8);
        engine.history.push("echo one".to_string());
        engine.history.push("echo two".to_string());

        let out = engine.suggest("e");
        assert_eq!(out.first().map(|s| s.full_text.as_str()), Some("echo two"));
    }

    mod prefix_extract {
        use super::*;

        #[test]
        fn extracts_cursor_line_prefix_from_cells() {
            let mut content = TerminalContent::default();
            content.cursor.point = crate::GridPoint::new(0, 4);
            content.cells = vec![
                crate::IndexedCell {
                    point: crate::GridPoint::new(0, 0),
                    cell: crate::Cell {
                        c: '$',
                        ..Default::default()
                    },
                },
                crate::IndexedCell {
                    point: crate::GridPoint::new(0, 1),
                    cell: crate::Cell {
                        c: ' ',
                        ..Default::default()
                    },
                },
                crate::IndexedCell {
                    point: crate::GridPoint::new(0, 2),
                    cell: crate::Cell {
                        c: 'l',
                        ..Default::default()
                    },
                },
                crate::IndexedCell {
                    point: crate::GridPoint::new(0, 3),
                    cell: crate::Cell {
                        c: 's',
                        ..Default::default()
                    },
                },
            ];

            assert_eq!(extract_cursor_line_prefix(&content), "$ ls");
        }
    }

    mod cursor_eol {
        use super::*;

        #[test]
        fn cursor_is_at_eol_when_no_non_space_to_the_right() {
            let mut content = TerminalContent::default();
            content.cursor.point = crate::GridPoint::new(0, 4);
            content.cells = vec![
                crate::IndexedCell {
                    point: crate::GridPoint::new(0, 0),
                    cell: crate::Cell {
                        c: '$',
                        ..Default::default()
                    },
                },
                crate::IndexedCell {
                    point: crate::GridPoint::new(0, 2),
                    cell: crate::Cell {
                        c: 'l',
                        ..Default::default()
                    },
                },
                crate::IndexedCell {
                    point: crate::GridPoint::new(0, 3),
                    cell: crate::Cell {
                        c: 's',
                        ..Default::default()
                    },
                },
            ];
            assert!(cursor_at_eol(&content));
        }

        #[test]
        fn cursor_is_not_at_eol_when_text_exists_to_the_right() {
            let mut content = TerminalContent::default();
            content.cursor.point = crate::GridPoint::new(0, 4);
            content.cells = vec![
                crate::IndexedCell {
                    point: crate::GridPoint::new(0, 0),
                    cell: crate::Cell {
                        c: '$',
                        ..Default::default()
                    },
                },
                crate::IndexedCell {
                    point: crate::GridPoint::new(0, 5),
                    cell: crate::Cell {
                        c: 'x',
                        ..Default::default()
                    },
                },
            ];
            assert!(!cursor_at_eol(&content));
        }
    }

    mod selection_move {
        use super::*;

        #[test]
        fn down_clamps_at_last() {
            assert_eq!(move_selection(0, 3, SelectionMove::Down), 1);
            assert_eq!(move_selection(2, 3, SelectionMove::Down), 2);
            assert_eq!(move_selection(0, 0, SelectionMove::Down), 0);
        }

        #[test]
        fn up_clamps_at_zero() {
            assert_eq!(move_selection(2, 3, SelectionMove::Up), 1);
            assert_eq!(move_selection(0, 3, SelectionMove::Up), 0);
            assert_eq!(move_selection(0, 0, SelectionMove::Up), 0);
        }
    }

    mod insert_suffix {
        use super::*;

        #[test]
        fn computes_non_empty_suffix_when_prefix_matches() {
            assert_eq!(
                compute_insert_suffix("ls", "ls --all").as_deref(),
                Some(" --all")
            );
        }

        #[test]
        fn rejects_when_not_a_prefix_or_empty_suffix() {
            assert_eq!(compute_insert_suffix("git", "ls --all"), None);
            assert_eq!(compute_insert_suffix("ls --all", "ls --all"), None);
        }

        #[test]
        fn ignores_extra_spaces_in_prefix() {
            assert_eq!(
                compute_insert_suffix("git    st", "git status").as_deref(),
                Some("atus")
            );
            assert_eq!(
                compute_insert_suffix("ls    -a", "ls -al").as_deref(),
                Some("l")
            );
        }
    }
}
