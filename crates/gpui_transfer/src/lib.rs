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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TransferCenterSummary {
    pub done: usize,
    pub total: usize,
    pub bytes_done: u64,
    pub bytes_total: u64,
}

#[derive(Default)]
struct TransferGroupState {
    total: usize,
    completed: HashSet<String>,
    bytes: HashMap<String, (u64, u64)>,
}

impl Global for TransferCenterState {}

impl TransferGroupState {
    fn summary(&self) -> TransferCenterSummary {
        let mut summary = TransferCenterSummary {
            done: self.completed.len().min(self.total),
            total: self.total,
            ..Default::default()
        };

        for (done, total) in self.bytes.values().copied() {
            summary.bytes_done += done.min(total);
            summary.bytes_total += total;
        }

        summary
    }
}

impl TransferCenterState {
    pub fn upsert(&mut self, mut task: TransferTask) {
        let now = Instant::now();
        let task_id = task.id.clone();

        if let Some(existing) = self.tasks.get(task.id.as_str()).cloned() {
            let preserve_transfer_bytes = existing.group_id == task.group_id;
            if preserve_transfer_bytes && task.bytes_done.is_none() {
                task.bytes_done = existing.bytes_done;
            }
            if preserve_transfer_bytes && task.bytes_total.is_none() {
                task.bytes_total = existing.bytes_total;
            }
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
        self.remove_task_from_group_state(&task, false, task.status != TransferStatus::InProgress);
        if self.tasks.is_empty() {
            self.group_tasks.clear();
            self.groups.clear();
        }
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
        self.group_tasks.remove(group_id);
        self.groups.remove(group_id);
    }

    pub fn remove_groups_with_prefix(&mut self, prefix: &str) {
        let mut group_ids: Vec<String> = self
            .group_tasks
            .keys()
            .filter(|group_id| group_id.starts_with(prefix))
            .cloned()
            .collect();
        for group_id in self
            .groups
            .keys()
            .filter(|group_id| group_id.starts_with(prefix))
        {
            if !group_ids.iter().any(|id| id == group_id) {
                group_ids.push(group_id.clone());
            }
        }

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

    pub fn summary(&self) -> TransferCenterSummary {
        let mut summary = TransferCenterSummary::default();

        for group in self.groups.values() {
            summary.add(group.summary());
        }

        for task in self.tasks.values().filter(|task| task.group_id.is_none()) {
            summary.total += 1;
            if task.status == TransferStatus::Finished {
                summary.done += 1;
            }
            if let (Some(done), Some(total)) = (task.bytes_done, task.bytes_total)
                && total > 0
            {
                summary.bytes_done += done.min(total);
                summary.bytes_total += total;
            }
        }

        summary
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

        if let Some(bytes_total) = task.bytes_total
            && bytes_total > 0
        {
            let bytes_done = task
                .bytes_done
                .unwrap_or_else(|| {
                    if task.status == TransferStatus::Finished {
                        bytes_total
                    } else {
                        0
                    }
                })
                .min(bytes_total);
            g.bytes.insert(task.id.clone(), (bytes_done, bytes_total));
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
                self.group_tasks.remove(group_id);
                if !preserve_group_when_empty {
                    self.groups.remove(group_id);
                    return;
                }
            }
        }

        if let Some(group_state) = self.groups.get_mut(group_id) {
            if remove_completion {
                group_state.completed.remove(task.id.as_str());
            }
            if remove_completion || task.status == TransferStatus::InProgress {
                group_state.bytes.remove(task.id.as_str());
            }
        }
    }
}

impl TransferCenterSummary {
    pub fn progress(self) -> TransferProgress {
        if self.bytes_total > 0 {
            TransferProgress::Determinate(
                (self.bytes_done as f32 / self.bytes_total as f32).clamp(0.0, 1.0),
            )
        } else {
            TransferProgress::Indeterminate
        }
    }

    fn add(&mut self, other: Self) {
        self.done += other.done;
        self.total += other.total;
        self.bytes_done += other.bytes_done;
        self.bytes_total += other.bytes_total;
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
    fn summary_keeps_original_total_even_when_tasks_auto_dismiss() {
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

        assert_summary(&s, 1, 3, 0, 0);

        // Simulate auto-dismiss removing finished tasks: the group total should not change.
        s.remove("t1");
        assert_summary(&s, 1, 3, 0, 0);

        // When the remaining task finishes, done count should advance.
        s.upsert(
            TransferTask::new("t2", "two")
                .with_group("g1", Some(3))
                .with_status(TransferStatus::Finished),
        );
        assert_summary(&s, 2, 3, 0, 0);
    }

    #[test]
    fn summary_progress_uses_bytes() {
        assert_eq!(
            TransferCenterSummary {
                done: 1,
                total: 2,
                bytes_done: 25,
                bytes_total: 100,
            }
            .progress(),
            TransferProgress::Determinate(0.25)
        );
    }

    #[test]
    fn summary_keeps_finished_task_bytes_after_auto_dismiss() {
        let mut s = TransferCenterState::default();

        s.upsert(
            TransferTask::new("t1", "one")
                .with_group("g1", Some(2))
                .with_status(TransferStatus::Finished)
                .with_bytes(Some(10), Some(10)),
        );
        s.upsert(
            TransferTask::new("t2", "two")
                .with_group("g1", Some(2))
                .with_status(TransferStatus::InProgress)
                .with_bytes(Some(5), Some(20)),
        );

        assert_summary(&s, 1, 2, 15, 30);

        s.remove("t1");
        assert_summary(&s, 1, 2, 15, 30);
    }

    #[test]
    fn summary_resets_after_all_tasks_auto_dismiss() {
        let mut s = TransferCenterState::default();

        s.upsert(
            TransferTask::new("old-1", "old one")
                .with_group("old", Some(2))
                .with_status(TransferStatus::Finished)
                .with_bytes(Some(10), Some(10)),
        );
        s.upsert(
            TransferTask::new("old-2", "old two")
                .with_group("old", Some(2))
                .with_status(TransferStatus::Finished)
                .with_bytes(Some(20), Some(20)),
        );

        s.remove("old-1");
        s.remove("old-2");

        assert_eq!(s.summary(), TransferCenterSummary::default());

        s.upsert(
            TransferTask::new("new-1", "new one")
                .with_group("new", Some(2))
                .with_status(TransferStatus::InProgress)
                .with_bytes(Some(5), Some(50)),
        );

        assert_summary(&s, 0, 2, 5, 50);
    }

    #[test]
    fn summary_keeps_finished_group_while_another_group_is_active() {
        let mut s = TransferCenterState::default();

        s.upsert(
            TransferTask::new("old-1", "old one")
                .with_group("old", Some(1))
                .with_status(TransferStatus::Finished)
                .with_bytes(Some(10), Some(10)),
        );
        s.upsert(
            TransferTask::new("new-1", "new one")
                .with_group("new", Some(2))
                .with_status(TransferStatus::InProgress)
                .with_bytes(Some(5), Some(50)),
        );
        s.remove("old-1");

        assert_summary(&s, 1, 3, 15, 60);
    }

    #[test]
    fn terminal_update_without_bytes_preserves_existing_group_bytes() {
        let mut s = TransferCenterState::default();

        s.upsert(
            TransferTask::new("t1", "one")
                .with_group("g1", Some(1))
                .with_status(TransferStatus::InProgress)
                .with_bytes(Some(4), Some(10)),
        );
        s.upsert(
            TransferTask::new("t1", "one")
                .with_group("g1", Some(1))
                .with_status(TransferStatus::Cancelled),
        );

        assert_summary(&s, 1, 1, 4, 10);
    }

    #[test]
    fn moving_task_to_another_group_does_not_reuse_previous_bytes() {
        let mut s = TransferCenterState::default();

        s.upsert(
            TransferTask::new("t1", "one")
                .with_group("g1", Some(1))
                .with_status(TransferStatus::InProgress)
                .with_bytes(Some(4), Some(10)),
        );
        s.upsert(
            TransferTask::new("t1", "one")
                .with_group("g2", Some(1))
                .with_status(TransferStatus::InProgress),
        );

        assert_summary(&s, 0, 1, 0, 0);
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
        assert_summary(&s, 0, 1, 0, 0);
    }

    #[test]
    fn upsert_replaces_previous_group_membership_and_completion_state() {
        let mut s = TransferCenterState::default();

        s.upsert(
            TransferTask::new("t1", "one")
                .with_group("g1", Some(2))
                .with_status(TransferStatus::Finished),
        );
        assert_summary(&s, 1, 2, 0, 0);

        s.upsert(
            TransferTask::new("t1", "one")
                .with_group("g2", Some(1))
                .with_status(TransferStatus::InProgress),
        );

        assert_summary(&s, 0, 1, 0, 0);
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
        assert_summary(&s, 0, 1, 0, 0);
    }

    fn assert_summary(
        state: &TransferCenterState,
        done: usize,
        total: usize,
        bytes_done: u64,
        bytes_total: u64,
    ) {
        assert_eq!(
            state.summary(),
            TransferCenterSummary {
                done,
                total,
                bytes_done,
                bytes_total,
            }
        );
    }
}
