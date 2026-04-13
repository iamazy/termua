use super::*;

impl SftpTable {
    pub(in crate::view) fn new(sftp: wezterm_ssh::Sftp) -> Self {
        let tree = TreeState::new(Entry::new(".", "~", EntryKind::Dir));
        let sort = SortSpec::default();
        let visible = apply_hidden_filter(
            tree.visible_rows_sorted(sort).into_iter().skip(1).collect(),
            false,
        );
        Self {
            sftp: Some(sftp),
            tree: Some(tree),
            loading: HashSet::new(),
            show_hidden: false,
            selected_ids: HashSet::new(),
            selection_anchor_id: None,
            columns: sftp_table_columns(),
            sort,
            visible,
            context_row: None,
            pending_toast: None,
            pending_toast_epoch: 0,
            transfers: std::collections::HashMap::new(),
            op: None,
        }
    }

    pub(in crate::view) fn rebuild_visible(&mut self) {
        let rows = self
            .tree
            .as_ref()
            .map(|t| t.visible_rows_sorted(self.sort))
            .unwrap_or_default();

        // We hide the root row (it is already represented by the breadcrumbs/path UI),
        // but keep the nested tree structure by showing expanded descendants.
        let rows = rows.into_iter().skip(1).collect::<Vec<_>>();
        self.visible = apply_hidden_filter(rows, self.show_hidden);

        let visible_ids: HashSet<&str> = self.visible.iter().map(|r| r.id.as_str()).collect();
        self.selected_ids
            .retain(|id| visible_ids.contains(id.as_str()));
        if let Some(anchor) = self.selection_anchor_id.as_deref()
            && !visible_ids.contains(anchor)
        {
            self.selection_anchor_id = None;
        }
    }

    pub(in crate::view) fn selected_ids_sorted(&self) -> Vec<String> {
        let mut out = self.selected_ids.iter().cloned().collect::<Vec<_>>();
        out.sort();
        out
    }

    pub(in crate::view) fn click_row_local(&mut self, row_ix: usize, modifiers: gpui::Modifiers) {
        let Some(row) = self.row(row_ix) else {
            return;
        };
        let target_id = row.id.clone();

        let extend = modifiers.secondary();
        let range = modifiers.shift;

        if range {
            if let Some(anchor_id) = self.selection_anchor_id.clone()
                && let Some(anchor_ix) = self.visible.iter().position(|r| r.id == anchor_id)
            {
                let (a, b) = if anchor_ix <= row_ix {
                    (anchor_ix, row_ix)
                } else {
                    (row_ix, anchor_ix)
                };

                if !extend {
                    self.selected_ids.clear();
                }
                for r in self.visible.iter().skip(a).take(b - a + 1) {
                    self.selected_ids.insert(r.id.clone());
                }
            } else {
                self.selected_ids.clear();
                self.selected_ids.insert(target_id.clone());
            }
        } else if extend {
            if !self.selected_ids.insert(target_id.clone()) {
                self.selected_ids.remove(target_id.as_str());
            }
        } else {
            self.selected_ids.clear();
            self.selected_ids.insert(target_id.clone());
        }

        self.selection_anchor_id = Some(target_id);
    }

    pub(in crate::view) fn set_context_menu_target(&mut self, row_ix: Option<usize>) {
        self.context_row = row_ix;

        let Some(row_ix) = row_ix else {
            return;
        };
        let Some(target_id) = self.row(row_ix).map(|row| row.id.clone()) else {
            return;
        };

        // Common file-explorer behavior: right-clicking a non-selected row selects it.
        if !self.selected_ids.contains(target_id.as_str()) {
            self.selected_ids.clear();
            self.selected_ids.insert(target_id.clone());
        }
        self.selection_anchor_id = Some(target_id);
    }

    pub(in crate::view) fn selection_summary(&self) -> (usize, bool, bool) {
        let mut has_file = false;
        let mut has_dir = false;
        let mut count = 0usize;

        for row in &self.visible {
            if !self.selected_ids.contains(row.id.as_str()) {
                continue;
            }
            count += 1;
            match row.kind {
                EntryKind::Dir => has_dir = true,
                EntryKind::File => has_file = true,
                _ => has_file = true,
            }
        }

        (count, has_file, has_dir)
    }

    pub(in crate::view) fn delete_target_ids(&self, target_row: Option<usize>) -> Vec<String> {
        if !self.selected_ids.is_empty() {
            return self.selected_ids_sorted();
        }
        target_row
            .and_then(|ix| self.row(ix))
            .map(|row| vec![row.id.clone()])
            .unwrap_or_default()
    }

    pub(in crate::view) fn toggle_dir_local(&mut self, row_ix: usize) -> Option<String> {
        let Some(row) = self.visible.get(row_ix).cloned() else {
            return None;
        };
        if row.kind != EntryKind::Dir {
            return None;
        }
        let Some(tree) = self.tree.as_mut() else {
            return None;
        };

        let id = row.id.clone();
        let next = !tree.is_expanded(&id);
        tree.set_expanded(&id, next);
        self.rebuild_visible();

        if next && !row.loaded_children {
            Some(id)
        } else {
            None
        }
    }

    pub(super) fn toggle_dir(&mut self, row_ix: usize, cx: &mut Context<TableState<Self>>) {
        let refresh = self.toggle_dir_local(row_ix);
        if let Some(dir) = refresh {
            self.refresh_dir(dir, cx);
        } else {
            cx.notify();
        }
    }

    pub(in crate::view) fn disconnect(&mut self, cx: &mut Context<TableState<Self>>) {
        self.sftp = None;
        self.loading.clear();
        self.finish_transfer_force(cx);
        self.op = None;
        self.show_toast(
            PromptLevel::Warning,
            "SSH terminal closed",
            Some("SFTP session disconnected.".to_string()),
            cx,
        );
    }

    pub(in crate::view) fn bootstrap_root(&mut self, cx: &mut Context<TableState<Self>>) {
        let Some(sftp) = self.sftp.clone() else {
            return;
        };

        // Canonicalize "." so we can show a stable, real absolute path as the root label.
        cx.spawn(async move |this, cx| {
            let root = match sftp.canonicalize(".").await {
                Ok(p) => p.to_string(),
                Err(_) => ".".to_string(),
            };

            let _ = this.update(cx, |this, cx| {
                let d = this.delegate_mut();
                d.tree = Some(TreeState::new(Entry::new(
                    &root,
                    display_name_for_dir(&root),
                    EntryKind::Dir,
                )));
                d.rebuild_visible();
                d.refresh_dir(root, cx);
            });
        })
        .detach();
    }

    pub(in crate::view) fn row(&self, row_ix: usize) -> Option<&VisibleRow> {
        self.visible.get(row_ix)
    }

    pub(in crate::view) fn drop_upload_target_dir(
        &self,
        target_row: Option<usize>,
    ) -> Option<String> {
        let tree = self.tree.as_ref()?;
        let Some(row) = target_row.and_then(|ix| self.row(ix)) else {
            return Some(tree.root.clone());
        };

        match row.kind {
            EntryKind::Dir => Some(row.id.clone()),
            _ => Some(tree.root.clone()),
        }
    }

    pub(super) fn selected_dir_for_new_entries(&self, target_row: Option<usize>) -> Option<String> {
        let tree = self.tree.as_ref()?;
        let Some(row) = target_row.and_then(|ix| self.row(ix)) else {
            return Some(tree.root.clone());
        };

        match row.kind {
            EntryKind::Dir => Some(row.id.clone()),
            _ => parent_dir(&row.id).or_else(|| Some(tree.root.clone())),
        }
    }

    pub(in crate::view) fn refresh_at(
        &mut self,
        target_row: Option<usize>,
        cx: &mut Context<TableState<Self>>,
    ) {
        let Some(tree) = self.tree.as_ref() else {
            return;
        };

        let dir = match target_row.and_then(|ix| self.row(ix)) {
            Some(row) if row.kind == EntryKind::Dir => row.id.clone(),
            Some(row) => parent_dir(&row.id).unwrap_or_else(|| tree.root.clone()),
            None => tree.root.clone(),
        };

        self.refresh_dir(dir, cx);
    }

    pub(super) fn refresh_dir(&mut self, dir: String, cx: &mut Context<TableState<Self>>) {
        let Some(sftp) = self.sftp.clone() else {
            self.show_toast(PromptLevel::Warning, "Disconnected", None, cx);
            return;
        };
        let Some(tree) = self.tree.as_mut() else {
            return;
        };

        self.loading.insert(dir.clone());
        tree.set_expanded(&dir, true);
        self.rebuild_visible();
        cx.notify();

        cx.spawn(async move |this, cx| {
            let res = sftp.read_dir(dir.clone()).await;
            let _ = this.update(cx, |this, cx| {
                this.delegate_mut().loading.remove(&dir);
                match res {
                    Ok(entries) => {
                        let children = entries
                            .into_iter()
                            .filter_map(|(path, meta)| {
                                // Different SFTP backends can return either absolute or relative
                                // paths here. We normalize all ids to be `dir/<name>` so that
                                // subsequent operations address consistent remote paths.
                                let name = path.file_name()?;
                                if name == "." || name == ".." {
                                    return None;
                                }
                                let path = Utf8PathBuf::from(join_remote(&dir, name));
                                Some(entry_from_meta(path, meta))
                            })
                            .collect();
                        if let Some(tree) = this.delegate_mut().tree.as_mut() {
                            tree.upsert_children(&dir, children);
                        }
                        this.delegate_mut().rebuild_visible();
                        cx.notify();
                    }
                    Err(err) => {
                        this.delegate_mut().show_toast(
                            PromptLevel::Warning,
                            "Failed to read directory",
                            Some(err.to_string()),
                            cx,
                        );
                        this.delegate_mut().rebuild_visible();
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    pub(in crate::view) fn cd(&mut self, dir: String, cx: &mut Context<TableState<Self>>) {
        if self.sftp.is_none() {
            self.show_toast(PromptLevel::Warning, "Disconnected", None, cx);
            return;
        }

        self.loading.clear();
        self.tree = Some(TreeState::new(Entry::new(
            &dir,
            display_name_for_dir(&dir),
            EntryKind::Dir,
        )));
        self.rebuild_visible();
        cx.notify();

        self.refresh_dir(dir, cx);
    }

    pub(super) fn set_sort(&mut self, col: SortColumn, cx: &mut Context<TableState<Self>>) {
        if self.sort.column == col {
            self.sort.direction = match self.sort.direction {
                SortDirection::Asc => SortDirection::Desc,
                SortDirection::Desc => SortDirection::Asc,
            };
        } else {
            self.sort.column = col;
            self.sort.direction = SortDirection::Asc;
        }
        self.rebuild_visible();
        cx.notify();
    }
}
