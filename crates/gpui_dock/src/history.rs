use std::{
    fmt::Debug,
    time::{Duration, Instant},
};

/// A HistoryItem represents a single change in the history.
/// It must implement Clone and PartialEq to be used in the History.
pub trait HistoryItem: Clone + PartialEq {
    fn version(&self) -> usize;
    fn set_version(&mut self, version: usize);
}

/// The History is used to keep track of changes to a model and to allow undo and redo operations.
#[derive(Debug)]
pub struct History<I: HistoryItem> {
    undos: Vec<I>,
    redos: Vec<I>,
    last_changed_at: Instant,
    version: usize,
    pub(crate) ignore: bool,
    max_undos: usize,
    group_interval: Option<Duration>,
    grouping: bool,
    unique: bool,
}

impl<I> History<I>
where
    I: HistoryItem,
{
    pub fn new() -> Self {
        Self {
            undos: Default::default(),
            redos: Default::default(),
            ignore: false,
            last_changed_at: Instant::now(),
            version: 0,
            max_undos: 1000,
            group_interval: None,
            grouping: false,
            unique: false,
        }
    }

    /// Set the maximum number of undo steps to keep, defaults to 1000.
    pub fn max_undos(mut self, max_undos: usize) -> Self {
        self.max_undos = max_undos;
        self
    }

    /// Set the history to be unique, defaults to false.
    /// If set to true, the history will only keep unique changes.
    pub fn unique(mut self) -> Self {
        self.unique = true;
        self
    }

    /// Set the interval to group changes, defaults to None.
    pub fn group_interval(mut self, group_interval: Duration) -> Self {
        self.group_interval = Some(group_interval);
        self
    }

    /// Start grouping changes; version will not increment until `end_grouping` is called.
    pub fn start_grouping(&mut self) {
        self.grouping = true;
    }

    /// End grouping changes; version can increment again.
    pub fn end_grouping(&mut self) {
        self.grouping = false;
    }

    fn inc_version(&mut self) -> usize {
        let t = Instant::now();
        if !self.grouping && Some(self.last_changed_at.elapsed()) > self.group_interval {
            self.version += 1;
        }

        self.last_changed_at = t;
        self.version
    }

    pub fn version(&self) -> usize {
        self.version
    }

    pub fn push(&mut self, item: I) {
        let version = self.inc_version();

        if self.undos.len() >= self.max_undos {
            self.undos.remove(0);
        }

        if self.unique {
            self.undos.retain(|c| *c != item);
            self.redos.retain(|c| *c != item);
        }

        let mut item = item;
        item.set_version(version);
        self.undos.push(item);
    }

    pub fn undos(&self) -> &Vec<I> {
        &self.undos
    }

    pub fn redos(&self) -> &Vec<I> {
        &self.redos
    }

    pub fn clear(&mut self) {
        self.undos.clear();
        self.redos.clear();
    }

    pub fn undo(&mut self) -> Option<Vec<I>> {
        if let Some(first_change) = self.undos.pop() {
            let mut changes = vec![first_change.clone()];
            while self
                .undos
                .iter()
                .filter(|c| c.version() == first_change.version())
                .count()
                > 0
            {
                let change = self.undos.pop().unwrap();
                changes.push(change);
            }

            self.redos.extend(changes.clone());
            Some(changes)
        } else {
            None
        }
    }

    pub fn redo(&mut self) -> Option<Vec<I>> {
        if let Some(first_change) = self.redos.pop() {
            let mut changes = vec![first_change.clone()];
            while self
                .redos
                .iter()
                .filter(|c| c.version() == first_change.version())
                .count()
                > 0
            {
                let change = self.redos.pop().unwrap();
                changes.push(change);
            }
            self.undos.extend(changes.clone());
            Some(changes)
        } else {
            None
        }
    }
}
