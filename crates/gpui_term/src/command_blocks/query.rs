use super::CommandBlock;

pub fn block_at_stable_row(blocks: &[CommandBlock], stable_row: i64) -> Option<&CommandBlock> {
    blocks.iter().rev().find(|b| match b.output_end_line {
        Some(end) => stable_row >= b.output_start_line && stable_row <= end,
        None => stable_row >= b.output_start_line,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(id: u64, start: i64, end: Option<i64>) -> CommandBlock {
        CommandBlock {
            id,
            started_at: std::time::Instant::now(),
            ended_at: None,
            exit_code: None,
            command: None,
            output_start_line: start,
            output_end_line: end,
        }
    }

    #[test]
    fn picks_latest_finished_block_containing_stable_row() {
        let blocks = vec![block(1, 10, Some(20)), block(2, 30, Some(40))];

        assert!(block_at_stable_row(&blocks, 9).is_none());
        assert_eq!(block_at_stable_row(&blocks, 10).unwrap().id, 1);
        assert_eq!(block_at_stable_row(&blocks, 15).unwrap().id, 1);
        assert_eq!(block_at_stable_row(&blocks, 20).unwrap().id, 1);
        assert!(block_at_stable_row(&blocks, 21).is_none());

        assert!(block_at_stable_row(&blocks, 29).is_none());
        assert_eq!(block_at_stable_row(&blocks, 30).unwrap().id, 2);
        assert_eq!(block_at_stable_row(&blocks, 39).unwrap().id, 2);
        assert_eq!(block_at_stable_row(&blocks, 40).unwrap().id, 2);
        assert!(block_at_stable_row(&blocks, 41).is_none());
    }

    #[test]
    fn running_block_matches_everything_after_start() {
        let blocks = vec![block(1, 10, Some(20)), block(2, 30, None)];

        assert!(block_at_stable_row(&blocks, 29).is_none());
        assert_eq!(block_at_stable_row(&blocks, 30).unwrap().id, 2);
        assert_eq!(block_at_stable_row(&blocks, 10_000).unwrap().id, 2);
    }

    #[test]
    fn prefers_latest_block_when_ranges_overlap() {
        let blocks = vec![block(1, 10, Some(50)), block(2, 40, Some(60))];
        assert_eq!(block_at_stable_row(&blocks, 45).unwrap().id, 2);
    }
}
