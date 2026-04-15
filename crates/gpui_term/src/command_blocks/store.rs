use std::collections::VecDeque;

#[derive(Clone, Debug)]
pub struct CommandBlock {
    pub id: u64,
    pub started_at: std::time::Instant,
    pub ended_at: Option<std::time::Instant>,
    pub exit_code: Option<i32>,
    pub command: Option<String>,
    pub output_start_line: i64,
    pub output_end_line: Option<i64>,
}

#[derive(Debug)]
pub struct CommandBlockStore {
    blocks: VecDeque<CommandBlock>,
    next_id: u64,
    capacity: usize,
}

impl CommandBlockStore {
    pub fn new(capacity: usize) -> Self {
        Self {
            blocks: VecDeque::with_capacity(capacity.max(1)),
            next_id: 1,
            capacity: capacity.max(1),
        }
    }

    pub fn push(&mut self, block: CommandBlock) {
        while self.blocks.len() >= self.capacity {
            self.blocks.pop_front();
        }
        self.blocks.push_back(block);
    }

    pub fn blocks(&self) -> Vec<CommandBlock> {
        self.blocks.iter().cloned().collect()
    }

    pub fn next_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        id
    }

    pub fn last_mut(&mut self) -> Option<&mut CommandBlock> {
        self.blocks.back_mut()
    }

    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut CommandBlock> {
        self.blocks.iter_mut()
    }

    pub fn get_mut(&mut self, index: usize) -> Option<&mut CommandBlock> {
        self.blocks.get_mut(index)
    }
}
