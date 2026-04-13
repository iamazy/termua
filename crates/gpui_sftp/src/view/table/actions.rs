use super::*;

#[derive(Clone, Debug)]
struct PlannedUpload {
    local: PathBuf,
    file_name: String,
    remote_path: String,
    total: u64,
    epoch: usize,
    cancel: Arc<AtomicBool>,
}

#[derive(Clone, Debug)]
enum UploadMsg {
    Progress {
        epoch: usize,
        file_name: String,
        sent: u64,
        total: u64,
    },
    Finished {
        epoch: usize,
        file_name: String,
    },
    Cancelled {
        epoch: usize,
        file_name: String,
    },
    Failed {
        epoch: usize,
        file_name: String,
        error: String,
    },
}

fn spawn_upload_worker(
    sftp: wezterm_ssh::Sftp,
    pool: gpui_common::PermitPool,
    file: PlannedUpload,
    tx: smol::channel::Sender<UploadMsg>,
) {
    smol::spawn(run_upload_worker(sftp, pool, file, tx)).detach();
}

async fn upload_send_progress(
    tx: &smol::channel::Sender<UploadMsg>,
    epoch: usize,
    file_name: String,
    sent: u64,
    total: u64,
) {
    let _ = tx
        .send(UploadMsg::Progress {
            epoch,
            file_name,
            sent,
            total,
        })
        .await;
}

async fn upload_send_cancelled(
    tx: &smol::channel::Sender<UploadMsg>,
    epoch: usize,
    file_name: String,
) {
    let _ = tx.send(UploadMsg::Cancelled { epoch, file_name }).await;
}

async fn upload_send_failed(
    tx: &smol::channel::Sender<UploadMsg>,
    epoch: usize,
    file_name: String,
    error: String,
) {
    let _ = tx
        .send(UploadMsg::Failed {
            epoch,
            file_name,
            error,
        })
        .await;
}

enum UploadOutcome {
    Finished,
    Cancelled,
    Failed,
}

async fn upload_copy_loop(
    local_f: &mut (impl smol::io::AsyncRead + Unpin),
    remote_f: &mut (impl smol::io::AsyncWrite + Unpin),
    cancel: &Arc<AtomicBool>,
    epoch: usize,
    file_name: &str,
    total: u64,
    tx: &smol::channel::Sender<UploadMsg>,
) -> UploadOutcome {
    let mut sent: u64 = 0;
    let mut last_emit_at = Instant::now();
    let mut buf = vec![0u8; 64 * 1024];

    loop {
        if cancel.load(Ordering::Relaxed) {
            if total != 0 && sent >= total {
                break;
            }
            upload_send_cancelled(tx, epoch, file_name.to_string()).await;
            return UploadOutcome::Cancelled;
        }

        let n = match local_f.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => n,
            Err(err) => {
                upload_send_failed(
                    tx,
                    epoch,
                    file_name.to_string(),
                    format!("Read local file failed: {err}"),
                )
                .await;
                return UploadOutcome::Failed;
            }
        };

        if cancel.load(Ordering::Relaxed) {
            upload_send_cancelled(tx, epoch, file_name.to_string()).await;
            return UploadOutcome::Cancelled;
        }

        if let Err(err) = remote_f.write_all(&buf[..n]).await {
            upload_send_failed(
                tx,
                epoch,
                file_name.to_string(),
                format!("Write remote file failed: {err}"),
            )
            .await;
            return UploadOutcome::Failed;
        }

        sent = sent.saturating_add(n as u64);

        let now = Instant::now();
        if now.duration_since(last_emit_at) >= Duration::from_millis(200) {
            last_emit_at = now;
            upload_send_progress(tx, epoch, file_name.to_string(), sent, total).await;
        }
    }

    let _ = remote_f.flush().await;
    UploadOutcome::Finished
}

async fn run_upload_worker(
    sftp: wezterm_ssh::Sftp,
    pool: gpui_common::PermitPool,
    file: PlannedUpload,
    tx: smol::channel::Sender<UploadMsg>,
) {
    let PlannedUpload {
        local,
        file_name,
        remote_path,
        total,
        epoch,
        cancel,
    } = file;

    let _permit = pool.acquire().await;

    let mut local_f = match smol::fs::File::open(&local).await {
        Ok(f) => f,
        Err(err) => {
            upload_send_failed(
                &tx,
                epoch,
                file_name,
                format!("Open local file failed: {err}"),
            )
            .await;
            return;
        }
    };

    let open_write = || {
        sftp.open_with_mode(
            &remote_path,
            OpenOptions {
                read: false,
                write: Some(WriteMode::Write),
                mode: 0o666,
                ty: OpenFileType::File,
            },
        )
    };

    let open_create = || {
        sftp.open_with_mode(
            &remote_path,
            OpenOptions {
                read: false,
                write: Some(WriteMode::Append),
                mode: 0o666,
                ty: OpenFileType::File,
            },
        )
    };

    let mut remote_f = match open_write().await {
        Ok(f) => f,
        Err(err) => {
            let err_str = err.to_string();
            if err_str.contains("Sftp error code 2") {
                match open_create().await {
                    Ok(f) => f,
                    Err(err2) => {
                        upload_send_failed(
                            &tx,
                            epoch,
                            file_name,
                            format!("Open remote file failed: {remote_path}: {err2}"),
                        )
                        .await;
                        return;
                    }
                }
            } else {
                upload_send_failed(
                    &tx,
                    epoch,
                    file_name,
                    format!("Open remote file failed: {remote_path}: {err}"),
                )
                .await;
                return;
            }
        }
    };

    match upload_copy_loop(
        &mut local_f,
        &mut remote_f,
        &cancel,
        epoch,
        &file_name,
        total,
        &tx,
    )
    .await
    {
        UploadOutcome::Finished => {
            upload_send_progress(&tx, epoch, file_name.clone(), total, total).await;
            let _ = tx.send(UploadMsg::Finished { epoch, file_name }).await;
        }
        UploadOutcome::Cancelled | UploadOutcome::Failed => {}
    }
}

async fn upload_local_files_to_dir_task(
    this: gpui::WeakEntity<TableState<SftpTable>>,
    cx: &mut gpui::AsyncApp,
    sftp: wezterm_ssh::Sftp,
    pool: gpui_common::PermitPool,
    remote_dir: String,
    locals: Vec<PathBuf>,
) {
    let planned = plan_uploads(&this, cx, &remote_dir, locals).await;
    if planned.is_empty() {
        show_upload_nothing_to_upload(&this, cx);
        return;
    }

    let total_files = planned.len();
    let batch_id = next_transfer_epoch();
    let group_id = format!("sftp-upload-batch-{batch_id}");

    begin_upload_transfers(&this, cx, &planned, &group_id, total_files);
    let uploaded = run_upload_workers(&this, cx, &sftp, &pool, planned, total_files).await;
    finish_upload_batch(&this, cx, remote_dir, total_files, uploaded);
}

async fn plan_uploads(
    this: &gpui::WeakEntity<TableState<SftpTable>>,
    cx: &mut gpui::AsyncApp,
    remote_dir: &str,
    locals: Vec<PathBuf>,
) -> Vec<PlannedUpload> {
    let mut planned: Vec<PlannedUpload> = Vec::new();
    for local in locals {
        let Some(file_name) = local
            .file_name()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string())
        else {
            let _ = this.update(cx, |this, cx| {
                this.delegate_mut()
                    .show_toast(PromptLevel::Warning, "Invalid filename", None, cx);
            });
            continue;
        };

        let remote_path = join_remote(remote_dir, &file_name);
        let total: u64 = smol::fs::metadata(&local)
            .await
            .ok()
            .map(|m| m.len())
            .unwrap_or(0);

        planned.push(PlannedUpload {
            local,
            file_name,
            remote_path,
            total,
            epoch: next_transfer_epoch(),
            cancel: Arc::new(AtomicBool::new(false)),
        });
    }
    planned
}

fn show_upload_nothing_to_upload(
    this: &gpui::WeakEntity<TableState<SftpTable>>,
    cx: &mut gpui::AsyncApp,
) {
    let _ = this.update(cx, |this, cx| {
        this.delegate_mut().show_toast(
            PromptLevel::Info,
            "Nothing to upload",
            Some("No valid files were selected.".to_string()),
            cx,
        );
    });
}

fn begin_upload_transfers(
    this: &gpui::WeakEntity<TableState<SftpTable>>,
    cx: &mut gpui::AsyncApp,
    planned: &[PlannedUpload],
    group_id: &str,
    total_files: usize,
) {
    let group_id = group_id.to_string();
    let _ = this.update(cx, |this, cx| {
        for f in planned {
            this.delegate_mut().begin_transfer(
                f.epoch,
                Transfer::Upload {
                    name: f.file_name.clone(),
                    sent: 0,
                    total: f.total,
                },
                Arc::clone(&f.cancel),
                Some(f.remote_path.clone().into()),
                Some(group_id.clone()),
                Some(total_files),
                cx,
            );
        }
    });
}

async fn run_upload_workers(
    this: &gpui::WeakEntity<TableState<SftpTable>>,
    cx: &mut gpui::AsyncApp,
    sftp: &wezterm_ssh::Sftp,
    pool: &gpui_common::PermitPool,
    planned: Vec<PlannedUpload>,
    total_files: usize,
) -> usize {
    let (tx, rx) = smol::channel::unbounded::<UploadMsg>();
    let mut completed: usize = 0;
    let mut uploaded: usize = 0;

    for f in planned {
        spawn_upload_worker(sftp.clone(), pool.clone(), f, tx.clone());
    }

    while completed < total_files {
        let msg = match rx.recv().await {
            Ok(m) => m,
            Err(_) => break,
        };

        match msg {
            UploadMsg::Progress {
                epoch,
                file_name,
                sent,
                total,
            } => {
                let _ = this.update(cx, |this, cx| {
                    this.delegate_mut().set_transfer_progress(
                        epoch,
                        Transfer::Upload {
                            name: file_name,
                            sent,
                            total,
                        },
                        cx,
                    );
                });
            }
            UploadMsg::Finished { epoch, file_name } => {
                completed = completed.saturating_add(1);
                uploaded = uploaded.saturating_add(1);
                let _ = this.update(cx, |this, cx| {
                    this.delegate_mut()
                        .finish_transfer_with_auto_hide(epoch, file_name, cx);
                });
            }
            UploadMsg::Cancelled { epoch, file_name } => {
                completed = completed.saturating_add(1);
                let _ = this.update(cx, |this, cx| {
                    this.delegate_mut().finish_transfer(epoch, cx);
                    this.delegate_mut().show_toast(
                        PromptLevel::Info,
                        format!("Upload canceled: {file_name}"),
                        None,
                        cx,
                    );
                });
            }
            UploadMsg::Failed {
                epoch,
                file_name,
                error,
            } => {
                completed = completed.saturating_add(1);
                let _ = this.update(cx, |this, cx| {
                    this.delegate_mut().finish_transfer(epoch, cx);
                    this.delegate_mut().show_toast(
                        PromptLevel::Warning,
                        format!("Upload failed: {file_name}"),
                        Some(error),
                        cx,
                    );
                });
            }
        }
    }

    uploaded
}

fn finish_upload_batch(
    this: &gpui::WeakEntity<TableState<SftpTable>>,
    cx: &mut gpui::AsyncApp,
    remote_dir: String,
    total_files: usize,
    uploaded: usize,
) {
    let _ = this.update(cx, |this, cx| {
        let title = if total_files == 1 {
            "Upload finished".to_string()
        } else if uploaded == total_files {
            format!("Upload finished ({uploaded} files)")
        } else {
            format!("Upload finished ({uploaded}/{total_files} files)")
        };
        this.delegate_mut()
            .show_toast(PromptLevel::Info, title, None, cx);
        this.delegate_mut().refresh_dir(remote_dir.clone(), cx);
    });
}

fn begin_download_transfer(
    this: &gpui::WeakEntity<TableState<SftpTable>>,
    cx: &mut gpui::AsyncApp,
    file_name: String,
    total: Option<u64>,
    dst: &PathBuf,
) -> (usize, Arc<AtomicBool>) {
    let cancel = Arc::new(AtomicBool::new(false));
    let epoch = next_transfer_epoch();
    let dst_label = dst.display().to_string();
    let _ = this.update(cx, |this, cx| {
        this.delegate_mut().begin_transfer(
            epoch,
            Transfer::Download {
                name: file_name,
                received: 0,
                total,
            },
            cancel.clone(),
            Some(dst_label.into()),
            None,
            None,
            cx,
        );
    });
    (epoch, cancel)
}

fn finish_download_cancelled(
    this: &gpui::WeakEntity<TableState<SftpTable>>,
    cx: &mut gpui::AsyncApp,
    epoch: usize,
) {
    let _ = this.update(cx, |this, cx| {
        this.delegate_mut().finish_transfer(epoch, cx);
        this.delegate_mut()
            .show_toast(PromptLevel::Info, "Download canceled", None, cx);
    });
}

fn finish_download_failed(
    this: &gpui::WeakEntity<TableState<SftpTable>>,
    cx: &mut gpui::AsyncApp,
    epoch: usize,
    title: &'static str,
    detail: String,
) {
    let _ = this.update(cx, |this, cx| {
        this.delegate_mut()
            .show_toast(PromptLevel::Warning, title, Some(detail), cx);
        this.delegate_mut().finish_transfer(epoch, cx);
    });
}

async fn download_copy_loop(
    this: &gpui::WeakEntity<TableState<SftpTable>>,
    cx: &mut gpui::AsyncApp,
    remote_f: &mut (impl smol::io::AsyncRead + Unpin),
    local_f: &mut (impl smol::io::AsyncWrite + Unpin),
    epoch: usize,
    file_name: &str,
    total: Option<u64>,
    cancel: &Arc<AtomicBool>,
) -> Option<u64> {
    let mut received: u64 = 0;
    let mut buf = vec![0u8; 64 * 1024];

    loop {
        if cancel.load(Ordering::Relaxed) {
            if total.is_some_and(|t| t != 0 && received >= t) {
                break;
            }
            finish_download_cancelled(this, cx, epoch);
            return None;
        }

        let n = match remote_f.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => n,
            Err(err) => {
                finish_download_failed(this, cx, epoch, "Read remote file failed", err.to_string());
                return None;
            }
        };

        if cancel.load(Ordering::Relaxed) {
            finish_download_cancelled(this, cx, epoch);
            return None;
        }

        if let Err(err) = local_f.write_all(&buf[..n]).await {
            finish_download_failed(this, cx, epoch, "Write local file failed", err.to_string());
            return None;
        }

        received = received.saturating_add(n as u64);
        if cancel.load(Ordering::Relaxed) {
            finish_download_cancelled(this, cx, epoch);
            return None;
        }

        let file_name = file_name.to_string();
        let _ = this.update(cx, |this, cx| {
            this.delegate_mut().set_transfer_progress(
                epoch,
                Transfer::Download {
                    name: file_name,
                    received,
                    total,
                },
                cx,
            );
        });
    }

    Some(received)
}

async fn finish_download_success(
    this: &gpui::WeakEntity<TableState<SftpTable>>,
    cx: &mut gpui::AsyncApp,
    epoch: usize,
    file_name: String,
    received: u64,
    total: Option<u64>,
) {
    // Ensure the last chunk has a chance to render as 100%.
    let received = total.unwrap_or(received);
    let _ = this.update(cx, |this, cx| {
        this.delegate_mut().set_transfer_progress(
            epoch,
            Transfer::Download {
                name: file_name.clone(),
                received,
                total,
            },
            cx,
        );
    });
    Timer::after(Duration::from_millis(150)).await;

    let _ = this.update(cx, |this, cx| {
        this.delegate_mut()
            .finish_transfer_with_auto_hide(epoch, file_name, cx);
        this.delegate_mut()
            .show_toast(PromptLevel::Info, "Download finished", None, cx);
    });
}

async fn download_to_path(
    this: gpui::WeakEntity<TableState<SftpTable>>,
    cx: &mut gpui::AsyncApp,
    sftp: wezterm_ssh::Sftp,
    id: String,
    file_name: String,
    dst: PathBuf,
) {
    let total = sftp.metadata(&id).await.ok().and_then(|m| m.size);
    let mut remote_f = match sftp.open(&id).await {
        Ok(f) => f,
        Err(err) => {
            let _ = this.update(cx, |this, cx| {
                this.delegate_mut().show_toast(
                    PromptLevel::Warning,
                    "Open remote file failed",
                    Some(err.to_string()),
                    cx,
                );
            });
            return;
        }
    };

    let mut local_f = match smol::fs::File::create(&dst).await {
        Ok(f) => f,
        Err(err) => {
            let _ = this.update(cx, |this, cx| {
                this.delegate_mut().show_toast(
                    PromptLevel::Warning,
                    "Create local file failed",
                    Some(err.to_string()),
                    cx,
                );
            });
            return;
        }
    };

    let (epoch, cancel) = begin_download_transfer(&this, cx, file_name.clone(), total, &dst);
    let Some(received) = download_copy_loop(
        &this,
        cx,
        &mut remote_f,
        &mut local_f,
        epoch,
        &file_name,
        total,
        &cancel,
    )
    .await
    else {
        return;
    };

    finish_download_success(&this, cx, epoch, file_name, received, total).await;
}

impl SftpTable {
    pub(in crate::view) fn open_new_folder(
        &mut self,
        target_row: Option<usize>,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        let Some(parent) = self.selected_dir_for_new_entries(target_row) else {
            return;
        };
        let input = new_input(window, cx, "Folder name");
        self.op = Some(SftpOp {
            kind: SftpOpKind::NewFolder { parent },
            input: input.clone(),
        });
        window.focus(&input.read(cx).focus_handle(cx), cx);
        cx.notify();
    }

    pub(in crate::view) fn open_rename(
        &mut self,
        target_row: Option<usize>,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        let Some(row_ix) = target_row else {
            return;
        };
        let Some(row) = self.row(row_ix) else {
            return;
        };
        let Some(parent) = parent_dir(&row.id) else {
            return;
        };

        let input = new_configured_input(window, cx, "New name", |input| {
            input.default_value(row.name.clone())
        });

        self.op = Some(SftpOp {
            kind: SftpOpKind::Rename {
                target: row.id.clone(),
                parent,
            },
            input: input.clone(),
        });
        window.focus(&input.read(cx).focus_handle(cx), cx);
        cx.notify();
    }

    pub(in crate::view) fn close(&mut self, cx: &mut Context<TableState<Self>>) {
        self.op = None;
        cx.notify();
    }

    pub(in crate::view) fn confirm(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        let Some(op) = self.op.clone() else {
            return;
        };
        let Some(sftp) = self.sftp.clone() else {
            self.show_toast(PromptLevel::Warning, "Disconnected", None, cx);
            return;
        };

        let name = op.input.read(cx).value().to_string();
        let name = name.trim().to_string();
        if name.is_empty() {
            self.show_toast(PromptLevel::Info, "Name is required", None, cx);
            return;
        }

        self.op = None;
        cx.notify();

        match op.kind {
            SftpOpKind::NewFolder { parent } => {
                let dir = join_remote(&parent, &name);
                cx.spawn(async move |this, cx| {
                    let res = sftp.create_dir(dir.clone(), 0o755).await;
                    let _ = this.update(cx, |this, cx| match res {
                        Ok(()) => {
                            this.delegate_mut().show_toast(
                                PromptLevel::Info,
                                "Folder created",
                                None,
                                cx,
                            );
                            this.delegate_mut().refresh_dir(parent.clone(), cx);
                        }
                        Err(err) => this.delegate_mut().show_toast(
                            PromptLevel::Warning,
                            "Create folder failed",
                            Some(err.to_string()),
                            cx,
                        ),
                    });
                })
                .detach();
            }
            SftpOpKind::Rename { target, parent } => {
                let dst = join_remote(&parent, &name);
                cx.spawn(async move |this, cx| {
                    let res = sftp
                        .rename(&target, &dst, wezterm_ssh::RenameOptions::default())
                        .await;
                    let _ = this.update(cx, |this, cx| match res {
                        Ok(()) => {
                            this.delegate_mut()
                                .show_toast(PromptLevel::Info, "Renamed", None, cx);
                            this.delegate_mut().refresh_dir(parent.clone(), cx);
                        }
                        Err(err) => this.delegate_mut().show_toast(
                            PromptLevel::Warning,
                            "Rename failed",
                            Some(err.to_string()),
                            cx,
                        ),
                    });
                })
                .detach();
            }
        }
    }

    pub(in crate::view) fn upload(
        &mut self,
        target_row: Option<usize>,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        if self.sftp.is_none() {
            self.show_toast(PromptLevel::Warning, "Disconnected", None, cx);
            return;
        };
        let Some(remote_dir) = self.selected_dir_for_new_entries(target_row) else {
            return;
        };

        let picker = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Select file to upload".into()),
        });
        let window_handle = _window.window_handle();

        cx.spawn(async move |this, cx| {
            let picked = picker.await;
            // Native dialogs can temporarily deactivate the app. Explicitly re-activate and
            // refresh once the dialog resolves (even if the user cancels).
            let _ = cx.update_window(window_handle, |_, window, app| {
                app.activate(true);
                window.refresh();
            });

            let Ok(Ok(Some(mut paths))) = picked else {
                return;
            };
            let Some(local) = paths.pop() else {
                return;
            };
            let _ = this.update(cx, |this, cx| {
                this.delegate_mut()
                    .upload_local_files_to_dir(remote_dir.clone(), vec![local], cx);
            });
        })
        .detach();
    }

    pub(in crate::view) fn upload_local_files_to_dir(
        &mut self,
        remote_dir: String,
        mut locals: Vec<PathBuf>,
        cx: &mut Context<TableState<Self>>,
    ) {
        let Some(sftp) = self.sftp.clone() else {
            self.show_toast(PromptLevel::Warning, "Disconnected", None, cx);
            return;
        };
        if !accept_external_file_drop_paths(&locals) {
            self.show_toast(
                PromptLevel::Info,
                "Only files are supported",
                Some("Dragging folders is not supported.".to_string()),
                cx,
            );
            return;
        }

        // Prefer stable ordering for readability and reproducibility.
        locals.sort();

        // Use the app-global pool (resized by Termua settings immediately).
        let pool = gpui_common::sftp_upload_permit_pool(cx);

        cx.spawn(async move |this, cx| {
            upload_local_files_to_dir_task(this, cx, sftp, pool, remote_dir, locals).await;
        })
        .detach();
    }

    pub(in crate::view) fn download(
        &mut self,
        target_row: Option<usize>,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        let Some(sftp) = self.sftp.clone() else {
            self.show_toast(PromptLevel::Warning, "Disconnected", None, cx);
            return;
        };
        let Some(row_ix) = target_row else {
            return;
        };
        let Some(row) = self.row(row_ix) else {
            return;
        };
        if row.kind == EntryKind::Dir {
            return;
        }

        let id = row.id.clone();
        let file_name = row.name.clone();
        let dir = default_download_dir();
        let picker = cx.prompt_for_new_path(&dir, Some(file_name.as_str()));
        let window_handle = _window.window_handle();

        cx.spawn(async move |this, cx| {
            let picked = picker.await;
            // Native dialogs can temporarily deactivate the app. Explicitly re-activate and
            // refresh once the dialog resolves (even if the user cancels).
            let _ = cx.update_window(window_handle, |_, window, app| {
                app.activate(true);
                window.refresh();
            });

            let Ok(Ok(Some(dst))) = picked else {
                return;
            };

            download_to_path(this, cx, sftp, id, file_name, dst).await;
        })
        .detach();
    }

    pub(in crate::view) fn delete_selected_ids(
        &mut self,
        mut ids: Vec<String>,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        let Some(sftp) = self.sftp.clone() else {
            self.show_toast(PromptLevel::Warning, "Disconnected", None, cx);
            return;
        };
        let Some(tree) = self.tree.as_ref() else {
            return;
        };

        ids.retain(|id| id != &tree.root);
        if ids.is_empty() {
            return;
        }

        let kinds = self
            .visible
            .iter()
            .map(|row| (row.id.clone(), row.kind))
            .collect::<std::collections::HashMap<String, EntryKind>>();
        let root = tree.root.clone();

        cx.spawn(async move |this, cx| {
            let total = ids.len();
            let mut deleted = 0usize;
            let mut failed = 0usize;
            let mut last_error: Option<String> = None;
            let mut parents = std::collections::HashSet::<String>::new();

            for id in ids {
                let is_dir = kinds.get(&id).is_some_and(|k| *k == EntryKind::Dir);
                let res = if is_dir {
                    sftp.remove_dir(&id).await
                } else {
                    sftp.remove_file(&id).await
                };

                match res {
                    Ok(()) => {
                        deleted = deleted.saturating_add(1);
                        parents.insert(parent_dir(&id).unwrap_or_else(|| root.clone()));
                    }
                    Err(err) => {
                        failed = failed.saturating_add(1);
                        last_error = Some(err.to_string());
                    }
                }
            }

            let _ = this.update(cx, |this, cx| {
                this.delegate_mut().selected_ids.clear();
                this.delegate_mut().selection_anchor_id = None;

                if failed == 0 {
                    let title = if deleted == 1 {
                        "Deleted".to_string()
                    } else {
                        format!("Deleted {deleted} items")
                    };
                    this.delegate_mut()
                        .show_toast(PromptLevel::Info, title, None, cx);
                } else {
                    let title = format!("Deleted {deleted}/{total} items");
                    this.delegate_mut()
                        .show_toast(PromptLevel::Warning, title, last_error, cx);
                }

                for parent in parents {
                    this.delegate_mut().refresh_dir(parent, cx);
                }
            });
        })
        .detach();
    }
}
