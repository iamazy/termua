use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, atomic::AtomicBool},
    time::{Duration, Instant},
};

use gpui::{Global, SharedString};

pub const AUTO_DISMISS_AFTER: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferKind {
    Upload,
    Download,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferStatus {
    InProgress,
    Finished,
    Cancelled,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TransferProgress {
    Indeterminate,
    /// 0.0..=1.0
    Determinate(f32),
}

#[derive(Clone, Debug)]
pub struct TransferTask {
    pub id: String,
    pub title: SharedString,
    pub detail: Option<SharedString>,
    /// Optional group id for representing a multi-item transfer (e.g. multi-file upload).
    ///
    /// Used by the footbar summary to show stable `done/total` counts even when individual
    /// tasks auto-dismiss from the list.
    pub group_id: Option<String>,
    /// Optional group total, in "items" (usually files).
    pub group_total: Option<usize>,
    pub kind: TransferKind,
    pub status: TransferStatus,
    pub progress: TransferProgress,
    pub bytes_done: Option<u64>,
    pub bytes_total: Option<u64>,
    pub cancel: Option<Arc<AtomicBool>>,
    pub created_at: Instant,
    pub updated_at: Instant,
}

impl TransferTask {
    pub fn new(id: impl Into<String>, title: impl Into<SharedString>) -> Self {
        let now = Instant::now();
        Self {
            id: id.into(),
            title: title.into(),
            detail: None,
            group_id: None,
            group_total: None,
            kind: TransferKind::Other,
            status: TransferStatus::InProgress,
            progress: TransferProgress::Indeterminate,
            bytes_done: None,
            bytes_total: None,
            cancel: None,
            created_at: now,
            updated_at: now,
        }
    }

    pub fn with_detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn with_group(mut self, group_id: impl Into<String>, total: Option<usize>) -> Self {
        self.group_id = Some(group_id.into());
        self.group_total = total;
        self
    }

    pub fn with_kind(mut self, kind: TransferKind) -> Self {
        self.kind = kind;
        self
    }

    pub fn with_progress(mut self, progress: TransferProgress) -> Self {
        self.progress = progress;
        self
    }

    pub fn with_bytes(mut self, done: Option<u64>, total: Option<u64>) -> Self {
        self.bytes_done = done;
        self.bytes_total = total;
        self
    }

    pub fn with_status(mut self, status: TransferStatus) -> Self {
        self.status = status;
        self
    }

    pub fn with_cancel_token(mut self, token: Arc<AtomicBool>) -> Self {
        self.cancel = Some(token);
        self
    }
}

#[derive(Default)]
pub struct TransferCenterState {
    tasks: HashMap<String, TransferTask>,
    order: Vec<String>,
    groups: HashMap<String, TransferGroupState>,
    group_tasks: HashMap<String, HashSet<String>>,
}

#[derive(Default)]
struct TransferGroupState {
    total: usize,
    completed: HashSet<String>,
}

impl Global for TransferCenterState {}

impl TransferCenterState {
    pub fn upsert(&mut self, mut task: TransferTask) {
        let now = Instant::now();
        let task_id = task.id.clone();

        if let Some(existing) = self.tasks.get(task.id.as_str()).cloned() {
            let preserve_group = existing.group_id == task.group_id;
            self.remove_task_from_group_state(&existing, true, preserve_group);
            task.created_at = existing.created_at;
            task.updated_at = now;
            self.apply_task_to_group_state(&task);
            self.tasks.insert(task_id, task);
            return;
        }

        task.created_at = now;
        task.updated_at = now;
        self.apply_task_to_group_state(&task);
        self.order.push(task_id.clone());
        self.tasks.insert(task_id, task);
    }

    pub fn remove(&mut self, id: &str) {
        let Some(task) = self.tasks.remove(id) else {
            return;
        };

        if let Some(pos) = self.order.iter().position(|k| k == id) {
            self.order.remove(pos);
        }
        self.remove_task_from_group_state(&task, false, false);
    }

    pub fn remove_group(&mut self, group_id: &str) {
        let ids: Vec<String> = self
            .group_tasks
            .get(group_id)
            .map(|ids| ids.iter().cloned().collect())
            .unwrap_or_default();

        for id in ids {
            self.remove(id.as_str());
        }
    }

    pub fn remove_groups_with_prefix(&mut self, prefix: &str) {
        let group_ids: Vec<String> = self
            .group_tasks
            .keys()
            .filter(|group_id| group_id.starts_with(prefix))
            .cloned()
            .collect();

        for group_id in group_ids {
            self.remove_group(group_id.as_str());
        }
    }

    pub fn tasks_sorted(&self) -> Vec<TransferTask> {
        self.order
            .iter()
            .filter_map(|id| self.tasks.get(id))
            .cloned()
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }

    pub fn group_counts(&self, group_id: &str) -> Option<(usize, usize)> {
        let g = self.groups.get(group_id)?;
        if g.total == 0 {
            return None;
        }
        Some((g.completed.len(), g.total))
    }

    fn apply_task_to_group_state(&mut self, task: &TransferTask) {
        let Some(group_id) = task.group_id.as_deref() else {
            return;
        };

        self.group_tasks
            .entry(group_id.to_string())
            .or_default()
            .insert(task.id.clone());

        let g = self.groups.entry(group_id.to_string()).or_default();
        if let Some(total) = task.group_total {
            g.total = g.total.max(total);
        }

        match task.status {
            TransferStatus::InProgress => {}
            TransferStatus::Finished | TransferStatus::Cancelled | TransferStatus::Failed => {
                g.completed.insert(task.id.clone());
            }
        }
    }

    fn remove_task_from_group_state(
        &mut self,
        task: &TransferTask,
        remove_completion: bool,
        preserve_group_when_empty: bool,
    ) {
        let Some(group_id) = task.group_id.as_deref() else {
            return;
        };

        if let Some(group_tasks) = self.group_tasks.get_mut(group_id) {
            group_tasks.remove(task.id.as_str());
            if group_tasks.is_empty() {
                if preserve_group_when_empty {
                    group_tasks.clear();
                } else {
                    self.group_tasks.remove(group_id);
                    self.groups.remove(group_id);
                    return;
                }
            }
        }

        if remove_completion && let Some(group_state) = self.groups.get_mut(group_id) {
            group_state.completed.remove(task.id.as_str());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn tasks_sorted_preserves_insertion_order_even_when_tasks_update() {
        let mut s = TransferCenterState::default();

        s.upsert(TransferTask::new("1", "one"));
        s.upsert(TransferTask::new("2", "two"));

        assert_eq!(
            s.tasks_sorted()
                .iter()
                .map(|t| t.id.as_str())
                .collect::<Vec<_>>(),
            vec!["1", "2"]
        );

        // Updating "2" used to move it to the top because tasks were sorted by updated_at.
        std::thread::sleep(Duration::from_millis(2));
        s.upsert(
            TransferTask::new("2", "two")
                .with_progress(TransferProgress::Determinate(0.5))
                .with_bytes(Some(5), Some(10)),
        );

        assert_eq!(
            s.tasks_sorted()
                .iter()
                .map(|t| t.id.as_str())
                .collect::<Vec<_>>(),
            vec!["1", "2"]
        );
    }

    #[test]
    fn group_counts_keep_original_total_even_when_tasks_auto_dismiss() {
        let mut s = TransferCenterState::default();

        s.upsert(
            TransferTask::new("t1", "one")
                .with_group("g1", Some(3))
                .with_status(TransferStatus::Finished),
        );
        s.upsert(
            TransferTask::new("t2", "two")
                .with_group("g1", Some(3))
                .with_status(TransferStatus::InProgress),
        );

        assert_eq!(s.group_counts("g1"), Some((1, 3)));

        // Simulate auto-dismiss removing finished tasks: the group total should not change.
        s.remove("t1");
        assert_eq!(s.group_counts("g1"), Some((1, 3)));

        // When the remaining task finishes, done count should advance.
        s.upsert(
            TransferTask::new("t2", "two")
                .with_group("g1", Some(3))
                .with_status(TransferStatus::Finished),
        );
        assert_eq!(s.group_counts("g1"), Some((2, 3)));
    }

    #[test]
    fn remove_group_removes_all_tasks_and_group_state() {
        let mut s = TransferCenterState::default();

        s.upsert(
            TransferTask::new("t1", "one")
                .with_group("g1", Some(2))
                .with_status(TransferStatus::Finished),
        );
        s.upsert(
            TransferTask::new("t2", "two")
                .with_group("g1", Some(2))
                .with_status(TransferStatus::InProgress),
        );
        s.upsert(TransferTask::new("t3", "three").with_group("g2", Some(1)));

        s.remove_group("g1");

        assert_eq!(
            s.tasks_sorted()
                .iter()
                .map(|t| t.id.as_str())
                .collect::<Vec<_>>(),
            vec!["t3"]
        );
        assert_eq!(s.group_counts("g1"), None);
        assert_eq!(s.group_counts("g2"), Some((0, 1)));
    }

    #[test]
    fn upsert_replaces_previous_group_membership_and_completion_state() {
        let mut s = TransferCenterState::default();

        s.upsert(
            TransferTask::new("t1", "one")
                .with_group("g1", Some(2))
                .with_status(TransferStatus::Finished),
        );
        assert_eq!(s.group_counts("g1"), Some((1, 2)));

        s.upsert(
            TransferTask::new("t1", "one")
                .with_group("g2", Some(1))
                .with_status(TransferStatus::InProgress),
        );

        assert_eq!(s.group_counts("g1"), None);
        assert_eq!(s.group_counts("g2"), Some((0, 1)));
    }

    #[test]
    fn remove_groups_with_prefix_removes_matching_groups_only() {
        let mut s = TransferCenterState::default();

        s.upsert(TransferTask::new("t1", "one").with_group("sftp-1", Some(1)));
        s.upsert(TransferTask::new("t2", "two").with_group("sftp-2", Some(1)));
        s.upsert(TransferTask::new("t3", "three").with_group("http-1", Some(1)));

        s.remove_groups_with_prefix("sftp-");

        assert_eq!(
            s.tasks_sorted()
                .iter()
                .map(|t| t.id.as_str())
                .collect::<Vec<_>>(),
            vec!["t3"]
        );
        assert_eq!(s.group_counts("sftp-1"), None);
        assert_eq!(s.group_counts("sftp-2"), None);
        assert_eq!(s.group_counts("http-1"), Some((0, 1)));
    }
}
