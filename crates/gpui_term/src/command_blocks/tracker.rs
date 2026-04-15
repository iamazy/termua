use std::time::Instant;

use super::{CommandBlock, CommandBlockStore};

#[derive(Debug)]
pub struct CommandBlockTracker {
    store: CommandBlockStore,
    active_id: Option<u64>,
}

impl CommandBlockTracker {
    pub fn new(capacity: usize) -> Self {
        Self {
            store: CommandBlockStore::new(capacity),
            active_id: None,
        }
    }

    pub fn blocks(&self) -> Vec<CommandBlock> {
        self.store.blocks()
    }

    pub fn remap_after_rewrap(&mut self, lines: &[(i64, String)], cursor_stable: i64) {
        let mut next_search_idx = 0usize;
        let mut remapped_starts: Vec<Option<i64>> = Vec::with_capacity(self.store.len());

        for block in self.store.iter_mut() {
            let Some(command) = block.command.as_deref() else {
                remapped_starts.push(None);
                continue;
            };

            let Some((start_idx, end_idx)) = find_command_span(lines, next_search_idx, command)
            else {
                remapped_starts.push(None);
                continue;
            };

            block.output_start_line = lines[start_idx].0;
            remapped_starts.push(Some(lines[start_idx].0));
            next_search_idx = end_idx.saturating_add(1);
        }

        for idx in 0..self.store.len() {
            let Some(start) = remapped_starts.get(idx).and_then(|v| *v) else {
                continue;
            };
            let next_start = remapped_starts
                .iter()
                .skip(idx + 1)
                .flatten()
                .copied()
                .next();
            let active = self.active_id;
            let block = self.store.get_mut(idx).expect("block index should exist");
            block.output_start_line = start;

            if block.ended_at.is_some() {
                block.output_end_line = next_start
                    .map(prev_stable_row)
                    .or_else(|| (active != Some(block.id)).then(|| prev_stable_row(cursor_stable)));
            }
        }
    }

    pub fn block_id_for_range(&self, start_line: i64, end_line: i64) -> Option<u64> {
        let start_line = start_line.min(end_line);
        let end_line = start_line.max(end_line);
        self.store.blocks().into_iter().find_map(|block| {
            (block.output_start_line == start_line && block.output_end_line == Some(end_line))
                .then_some(block.id)
        })
    }

    pub fn range_for_block_id(&self, block_id: u64) -> Option<(i64, i64)> {
        self.store.blocks().into_iter().find_map(|block| {
            (block.id == block_id).then(|| {
                let end = block.output_end_line.unwrap_or(block.output_start_line);
                (block.output_start_line, end)
            })
        })
    }

    pub fn apply_osc133(
        &mut self,
        payload: &str,
        now: Instant,
        bottom_line: i64,
        command: Option<String>,
    ) {
        let Some(ev) = Osc133Event::parse(payload) else {
            return;
        };

        match ev {
            Osc133Event::PromptStart => {
                if let Some(active_id) = self.active_id.take() {
                    self.finalize_block(active_id, now, bottom_line, None);
                }
            }
            Osc133Event::PromptEnd => {}
            Osc133Event::CommandStart => self.start_block(now, bottom_line, command),
            Osc133Event::CommandEnd { exit_code } => self.end_block(now, bottom_line, exit_code),
            Osc133Event::Unknown(_) => {}
        }
    }

    fn start_block(&mut self, now: Instant, output_start_line: i64, command: Option<String>) {
        if let Some(active_id) = self.active_id.take() {
            self.finalize_block(active_id, now, output_start_line, None);
        }

        let id = self.store.next_id();
        self.store.push(CommandBlock {
            id,
            started_at: now,
            ended_at: None,
            exit_code: None,
            command: normalize_command_text(command),
            output_start_line,
            output_end_line: None,
        });
        self.active_id = Some(id);
    }

    fn end_block(&mut self, now: Instant, output_end_line: i64, exit_code: Option<i32>) {
        let Some(active_id) = self.active_id.take() else {
            return;
        };
        self.finalize_block(active_id, now, output_end_line, exit_code);
    }

    fn finalize_block(
        &mut self,
        block_id: u64,
        now: Instant,
        output_end_line: i64,
        exit_code: Option<i32>,
    ) {
        if let Some(block) = self
            .store
            .last_mut()
            .filter(|b| b.id == block_id && b.ended_at.is_none())
        {
            block.ended_at = Some(now);
            block.exit_code = exit_code;
            block.output_end_line = Some(output_end_line);
        }
    }
}

fn normalize_command_text(command: Option<String>) -> Option<String> {
    let command = command?;
    let trimmed = command.trim_end().to_string();
    if trimmed.trim().is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

fn prev_stable_row(stable: i64) -> i64 {
    if stable > 0 { stable - 1 } else { 0 }
}

fn find_command_span(
    lines: &[(i64, String)],
    start_idx: usize,
    command: &str,
) -> Option<(usize, usize)> {
    let command = command.trim_end();
    if command.is_empty() {
        return None;
    }

    for idx in start_idx..lines.len() {
        let mut combined = String::new();
        for (end, (_, line)) in lines.iter().enumerate().skip(idx) {
            combined.push_str(line.trim_end());
            if combined == command {
                return Some((idx, end));
            }
            if combined.len() >= command.len() || !command.starts_with(&combined) {
                break;
            }
        }
    }

    None
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
enum Osc133Event<'a> {
    PromptStart,
    PromptEnd,
    CommandStart,
    CommandEnd { exit_code: Option<i32> },
    Unknown(&'a str),
}

impl<'a> Osc133Event<'a> {
    fn parse(payload: &'a str) -> Option<Self> {
        let mut parts = payload.split(';');
        let kind = parts.next()?.trim();

        match kind {
            "A" => Some(Self::PromptStart),
            "B" => Some(Self::PromptEnd),
            "C" => Some(Self::CommandStart),
            "D" => {
                let exit_code = parts.next().and_then(|v| v.trim().parse::<i32>().ok());
                Some(Self::CommandEnd { exit_code })
            }
            other if !other.is_empty() => Some(Self::Unknown(other)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_start_creates_active_block() {
        let mut t = CommandBlockTracker::new(10);
        let now = Instant::now();
        t.apply_osc133("C", now, 10, None);

        let blocks = t.blocks();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].output_start_line, 10);
        assert_eq!(blocks[0].ended_at, None);
        assert_eq!(blocks[0].output_end_line, None);
    }

    #[test]
    fn command_start_stores_command_text() {
        let mut t = CommandBlockTracker::new(10);
        let now = Instant::now();
        t.apply_osc133("C", now, 10, Some("echo hello".to_string()));

        let blocks = t.blocks();
        assert_eq!(blocks[0].command.as_deref(), Some("echo hello"));
    }

    #[test]
    fn command_end_finalizes_block() {
        let mut t = CommandBlockTracker::new(10);
        let now = Instant::now();
        t.apply_osc133("C", now, 10, None);
        t.apply_osc133("D;0", now, 42, None);

        let blocks = t.blocks();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].exit_code, Some(0));
        assert_eq!(blocks[0].output_end_line, Some(42));
        assert!(blocks[0].ended_at.is_some());
    }

    #[test]
    fn duplicate_command_start_finalizes_previous_block_best_effort() {
        let mut t = CommandBlockTracker::new(10);
        let now = Instant::now();
        t.apply_osc133("C", now, 10, Some("echo one".to_string()));
        t.apply_osc133("C", now, 20, Some("echo two".to_string()));

        let blocks = t.blocks();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].output_end_line, Some(20));
        assert!(blocks[0].ended_at.is_some());
        assert_eq!(blocks[0].command.as_deref(), Some("echo one"));
        assert_eq!(blocks[1].output_start_line, 20);
        assert_eq!(blocks[1].ended_at, None);
        assert_eq!(blocks[1].command.as_deref(), Some("echo two"));
    }

    #[test]
    fn prompt_start_finalizes_active_block_best_effort() {
        let mut t = CommandBlockTracker::new(10);
        let now = Instant::now();
        t.apply_osc133("C", now, 10, None);
        t.apply_osc133("A", now, 19, None);

        let blocks = t.blocks();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].output_end_line, Some(19));
        assert!(blocks[0].ended_at.is_some());
        assert_eq!(blocks[0].exit_code, None);
    }

    #[test]
    fn command_end_without_active_block_is_ignored() {
        let mut t = CommandBlockTracker::new(10);
        let now = Instant::now();
        t.apply_osc133("D;0", now, 10, None);
        assert!(t.blocks().is_empty());
    }

    #[test]
    fn remap_after_rewrap_updates_block_ranges_from_command_order() {
        let mut t = CommandBlockTracker::new(10);
        let now = Instant::now();
        t.apply_osc133("C", now, 10, Some("$ echo 123456".to_string()));
        t.apply_osc133("D;0", now, 12, None);
        t.apply_osc133("C", now, 13, Some("$ pwd".to_string()));
        t.apply_osc133("D;0", now, 14, None);

        let lines = vec![
            (100, "$ echo 1".to_string()),
            (101, "23456".to_string()),
            (102, "out".to_string()),
            (103, "$ pwd".to_string()),
            (104, "/tmp".to_string()),
            (105, "% ".to_string()),
        ];

        t.remap_after_rewrap(&lines, 105);

        let blocks = t.blocks();
        assert_eq!(blocks[0].output_start_line, 100);
        assert_eq!(blocks[0].output_end_line, Some(102));
        assert_eq!(blocks[1].output_start_line, 103);
        assert_eq!(blocks[1].output_end_line, Some(104));
    }

    #[test]
    fn maps_exact_selected_range_to_block_and_back_after_remap() {
        let mut t = CommandBlockTracker::new(10);
        let now = Instant::now();
        t.apply_osc133("C", now, 10, Some("$ echo 123456".to_string()));
        t.apply_osc133("D;0", now, 12, None);

        let block_id = t
            .block_id_for_range(10, 12)
            .expect("selection should match the command block exactly");

        let lines = vec![
            (100, "$ echo 1".to_string()),
            (101, "23456".to_string()),
            (102, "out".to_string()),
            (103, "% ".to_string()),
        ];
        t.remap_after_rewrap(&lines, 103);

        assert_eq!(t.range_for_block_id(block_id), Some((100, 102)));
    }
}
