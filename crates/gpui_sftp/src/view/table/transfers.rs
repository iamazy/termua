use super::*;

impl SftpTable {
    fn transfer_task_id(epoch: usize) -> String {
        format!("sftp-transfer-{epoch}")
    }

    fn publish_transfer_to_center(
        &self,
        epoch: usize,
        transfer: &Transfer,
        cancel: Option<&Arc<AtomicBool>>,
        detail: Option<&SharedString>,
        group_id: Option<&str>,
        group_total: Option<usize>,
        cx: &mut Context<TableState<Self>>,
    ) {
        if cx.try_global::<TransferCenterState>().is_none() {
            return;
        }

        let id = Self::transfer_task_id(epoch);
        let (title, pct, kind) = match transfer {
            Transfer::Upload { name, sent, total } => {
                let pct = if *total == 0 {
                    0.0
                } else {
                    (*sent as f64 / *total as f64).clamp(0.0, 1.0) as f32
                };
                (name.clone(), pct, TransferKind::Upload)
            }
            Transfer::Download {
                name,
                received,
                total,
            } => {
                let pct = match total {
                    Some(total) if *total > 0 => {
                        (*received as f64 / *total as f64).clamp(0.0, 1.0) as f32
                    }
                    _ => 0.0,
                };
                (name.clone(), pct, TransferKind::Download)
            }
            Transfer::Finished { title } => (title.clone(), 1.0, TransferKind::Other),
        };
        let progress = match transfer {
            Transfer::Upload { total, .. } if *total > 0 => {
                TransferProgress::Determinate(pct.clamp(0.0, 1.0))
            }
            Transfer::Download { total, .. } if total.is_some_and(|t| t > 0) => {
                TransferProgress::Determinate(pct.clamp(0.0, 1.0))
            }
            Transfer::Finished { .. } => TransferProgress::Determinate(1.0),
            _ => TransferProgress::Indeterminate,
        };

        let status = match transfer {
            Transfer::Finished { .. } => TransferStatus::Finished,
            _ => TransferStatus::InProgress,
        };

        let mut task = TransferTask::new(id, title)
            .with_kind(kind)
            .with_status(status)
            .with_progress(progress)
            .with_bytes(
                match transfer {
                    Transfer::Upload { sent, .. } => Some(*sent),
                    Transfer::Download { received, .. } => Some(*received),
                    Transfer::Finished { .. } => None,
                },
                match transfer {
                    Transfer::Upload { total, .. } if *total > 0 => Some(*total),
                    Transfer::Download { total, .. } => total.filter(|t| *t > 0),
                    Transfer::Finished { .. } => None,
                    _ => None,
                },
            );
        if let Some(token) = cancel {
            task = task.with_cancel_token(Arc::clone(token));
        }
        if let Some(detail) = detail
            && !detail.as_ref().trim().is_empty()
        {
            task = task.with_detail(detail.clone());
        }
        if let Some(group_id) = group_id {
            task = task.with_group(group_id.to_string(), group_total);
        }

        cx.global_mut::<TransferCenterState>().upsert(task);
    }

    fn remove_transfer_from_center(&self, epoch: usize, cx: &mut Context<TableState<Self>>) {
        if cx.try_global::<TransferCenterState>().is_none() {
            return;
        }
        let id = Self::transfer_task_id(epoch);
        cx.global_mut::<TransferCenterState>().remove(id.as_str());
    }

    fn begin_transfer_local(
        &mut self,
        epoch: usize,
        transfer: Transfer,
        cancel: Arc<AtomicBool>,
        detail: Option<SharedString>,
        group_id: Option<String>,
        group_total: Option<usize>,
    ) {
        self.transfers.insert(
            epoch,
            TransferEntry {
                transfer,
                cancel: Some(cancel),
                detail,
                group_id,
                group_total,
            },
        );
    }

    fn set_transfer_progress_local(&mut self, epoch: usize, transfer: Transfer) {
        let Some(entry) = self.transfers.get_mut(&epoch) else {
            return;
        };
        // If we're showing "finished", ignore any late progress for this transfer.
        // This avoids flicker where a late update re-opens the footer and prevents auto-hide.
        if matches!(entry.transfer, Transfer::Finished { .. }) {
            return;
        }
        entry.transfer = transfer;
    }

    fn finish_transfer_local(&mut self, epoch: usize) -> bool {
        self.transfers.remove(&epoch).is_some()
    }

    fn finish_transfer_with_auto_hide_local(&mut self, epoch: usize, title: impl Into<String>) {
        let Some(entry) = self.transfers.get_mut(&epoch) else {
            return;
        };
        entry.transfer = Transfer::Finished {
            title: title.into(),
        };
        entry.cancel = None;
    }

    pub(super) fn begin_transfer(
        &mut self,
        epoch: usize,
        transfer: Transfer,
        cancel: Arc<AtomicBool>,
        detail: Option<SharedString>,
        group_id: Option<String>,
        group_total: Option<usize>,
        cx: &mut Context<TableState<Self>>,
    ) {
        self.begin_transfer_local(epoch, transfer, cancel, detail, group_id, group_total);

        if let Some(entry) = self.transfers.get(&epoch) {
            self.publish_transfer_to_center(
                epoch,
                &entry.transfer,
                entry.cancel.as_ref(),
                entry.detail.as_ref(),
                entry.group_id.as_deref(),
                entry.group_total,
                cx,
            );
        }
        cx.notify();
    }

    pub(super) fn set_transfer_progress(
        &mut self,
        epoch: usize,
        transfer: Transfer,
        cx: &mut Context<TableState<Self>>,
    ) {
        if !self.transfers.contains_key(&epoch) {
            return;
        }

        self.set_transfer_progress_local(epoch, transfer);
        if let Some(entry) = self.transfers.get(&epoch) {
            self.publish_transfer_to_center(
                epoch,
                &entry.transfer,
                entry.cancel.as_ref(),
                entry.detail.as_ref(),
                entry.group_id.as_deref(),
                entry.group_total,
                cx,
            );
        }
        cx.notify();
    }

    pub(super) fn finish_transfer(&mut self, epoch: usize, cx: &mut Context<TableState<Self>>) {
        if !self.finish_transfer_local(epoch) {
            return;
        }
        self.remove_transfer_from_center(epoch, cx);
        cx.notify();
    }

    pub(super) fn finish_transfer_with_auto_hide(
        &mut self,
        epoch: usize,
        title: impl Into<String>,
        cx: &mut Context<TableState<Self>>,
    ) {
        if !self.transfers.contains_key(&epoch) {
            return;
        }

        self.finish_transfer_with_auto_hide_local(epoch, title);
        if let Some(entry) = self.transfers.get(&epoch) {
            self.publish_transfer_to_center(
                epoch,
                &entry.transfer,
                entry.cancel.as_ref(),
                entry.detail.as_ref(),
                entry.group_id.as_deref(),
                entry.group_total,
                cx,
            );
        }
        cx.notify();

        // Auto-hide after a short delay to keep the transfer list tidy.
        cx.spawn(async move |this, cx| {
            Timer::after(AUTO_DISMISS_AFTER).await;
            let _ = this.update(cx, |this, cx| {
                let still_finished = this
                    .delegate()
                    .transfers
                    .get(&epoch)
                    .is_some_and(|e| matches!(e.transfer, Transfer::Finished { .. }));
                if !still_finished {
                    return;
                }
                this.delegate_mut().transfers.remove(&epoch);
                this.delegate_mut().remove_transfer_from_center(epoch, cx);
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn finish_transfer_force(&mut self, cx: &mut Context<TableState<Self>>) {
        let epochs = self.transfers.keys().copied().collect::<Vec<_>>();
        for epoch in &epochs {
            if let Some(entry) = self.transfers.get(epoch) {
                if let Some(cancel) = entry.cancel.as_ref() {
                    cancel.store(true, Ordering::Relaxed);
                }
            }
            self.remove_transfer_from_center(*epoch, cx);
        }
        self.transfers.clear();
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn delegate_for_transfer_tests() -> SftpTable {
        let sort = SortSpec::default();
        SftpTable {
            sftp: None,
            tree: None,
            loading: HashSet::new(),
            show_hidden: false,
            selected_ids: HashSet::new(),
            selection_anchor_id: None,
            columns: sftp_table_columns(),
            sort,
            visible: Vec::new(),
            context_row: None,
            pending_toast: None,
            pending_toast_epoch: 0,
            transfers: std::collections::HashMap::new(),
            op: None,
        }
    }

    #[test]
    fn sftp_transfers_do_not_cancel_previous_transfers() {
        let mut d = delegate_for_transfer_tests();

        let c1 = Arc::new(AtomicBool::new(false));
        let c2 = Arc::new(AtomicBool::new(false));

        d.begin_transfer_local(
            1,
            Transfer::Download {
                name: "a.txt".to_string(),
                received: 0,
                total: Some(10),
            },
            Arc::clone(&c1),
            None,
            None,
            None,
        );
        d.begin_transfer_local(
            2,
            Transfer::Download {
                name: "b.txt".to_string(),
                received: 0,
                total: Some(20),
            },
            Arc::clone(&c2),
            None,
            None,
            None,
        );

        assert!(!c1.load(Ordering::Relaxed));
        assert!(!c2.load(Ordering::Relaxed));
        assert_eq!(d.transfers.len(), 2);
    }

    #[test]
    fn sftp_transfers_can_finish_out_of_order() {
        let mut d = delegate_for_transfer_tests();

        d.begin_transfer_local(
            1,
            Transfer::Upload {
                name: "a.bin".to_string(),
                sent: 0,
                total: 100,
            },
            Arc::new(AtomicBool::new(false)),
            None,
            None,
            None,
        );
        d.begin_transfer_local(
            2,
            Transfer::Upload {
                name: "b.bin".to_string(),
                sent: 0,
                total: 100,
            },
            Arc::new(AtomicBool::new(false)),
            None,
            None,
            None,
        );

        assert!(d.finish_transfer_local(1));
        assert_eq!(d.transfers.len(), 1);
        assert!(d.transfers.contains_key(&2));
    }

    #[test]
    fn sftp_transfer_ignores_late_progress_after_finished() {
        let mut d = delegate_for_transfer_tests();

        d.begin_transfer_local(
            1,
            Transfer::Upload {
                name: "a.bin".to_string(),
                sent: 0,
                total: 100,
            },
            Arc::new(AtomicBool::new(false)),
            None,
            None,
            None,
        );
        d.finish_transfer_with_auto_hide_local(1, "a.bin");

        d.set_transfer_progress_local(
            1,
            Transfer::Upload {
                name: "a.bin".to_string(),
                sent: 50,
                total: 100,
            },
        );

        let entry = d.transfers.get(&1).unwrap();
        assert!(matches!(entry.transfer, Transfer::Finished { .. }));
    }
}
