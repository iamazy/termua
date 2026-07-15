use std::time::{Duration, Instant};

use rust_i18n::t;

use super::*;

#[derive(Clone, Debug)]
struct PlannedUpload {
    local: PathBuf,
    file_name: String,
    remote_path: String,
    total: u64,
    epoch: usize,
    cancel: Arc<AtomicBool>,
    group_id: Option<String>,
    group_total: Option<usize>,
}

#[derive(Clone, Debug)]
struct UploadTaskCtx {
    epoch: usize,
    file_name: String,
    remote_path: SharedString,
    total: u64,
    cancel: Arc<AtomicBool>,
    group_id: Option<String>,
    group_total: Option<usize>,
}
impl UploadTaskCtx {
    fn task_id(&self) -> String {
        format!("sftp-transfer-{}", self.epoch)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum UploadFailureKind {
    OpenLocalFile,
    ReadLocalFile,
    OpenRemoteFile,
    WriteRemoteFile,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct UploadFailure {
    kind: UploadFailureKind,
    cause: String,
}

impl UploadFailure {
    fn new(kind: UploadFailureKind, cause: impl Into<String>) -> Self {
        Self {
            kind,
            cause: cause.into(),
        }
    }

    fn summary(&self) -> String {
        match self.kind {
            UploadFailureKind::OpenLocalFile => t!(
                "Sftp.Transfer.OpenLocalFileFailed",
                err = self.cause.clone()
            )
            .to_string(),
            UploadFailureKind::ReadLocalFile => t!(
                "Sftp.Transfer.ReadLocalFileFailed",
                err = self.cause.clone()
            )
            .to_string(),
            UploadFailureKind::OpenRemoteFile => t!(
                "Sftp.Transfer.OpenRemoteFileFailed",
                err = self.cause.clone()
            )
            .to_string(),
            UploadFailureKind::WriteRemoteFile => t!(
                "Sftp.Transfer.WriteRemoteFileFailed",
                err = self.cause.clone()
            )
            .to_string(),
        }
    }

    fn task_detail(&self, ctx: &UploadTaskCtx) -> String {
        if self.kind == UploadFailureKind::OpenRemoteFile {
            return t!(
                "Sftp.Transfer.OpenRemotePathFailed",
                path = ctx.remote_path.to_string(),
                err = self.cause.clone()
            )
            .to_string();
        }
        self.summary()
    }
}

#[derive(Clone, Debug)]
struct UploadFailureGroup {
    failure: UploadFailure,
    files: Vec<String>,
}

#[derive(Clone, Debug, Default)]
struct UploadBatchResult {
    uploaded: usize,
    failed: usize,
    cancelled: usize,
    failure_groups: Vec<UploadFailureGroup>,
}

impl UploadBatchResult {
    fn record_failure(&mut self, failure: UploadFailure, file_name: String) {
        self.failed = self.failed.saturating_add(1);
        if let Some(group) = self
            .failure_groups
            .iter_mut()
            .find(|group| group.failure == failure)
        {
            group.files.push(file_name);
            return;
        }
        self.failure_groups.push(UploadFailureGroup {
            failure,
            files: vec![file_name],
        });
    }

    fn failure_detail(&self) -> Option<String> {
        const MAX_VISIBLE_FILES: usize = 5;

        let details = self
            .failure_groups
            .iter()
            .map(|group| {
                let files = group
                    .files
                    .iter()
                    .take(MAX_VISIBLE_FILES)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ");
                let remaining = group.files.len().saturating_sub(MAX_VISIBLE_FILES);
                if remaining > 0 {
                    t!(
                        "Sftp.Toast.UploadFailureGroupMore",
                        reason = group.failure.summary(),
                        files = files,
                        remaining = remaining
                    )
                    .to_string()
                } else {
                    t!(
                        "Sftp.Toast.UploadFailureGroup",
                        reason = group.failure.summary(),
                        files = files
                    )
                    .to_string()
                }
            })
            .collect::<Vec<_>>();
        (!details.is_empty()).then(|| details.join("\n"))
    }
}

#[derive(Clone, Debug)]
struct DownloadTaskCtx {
    epoch: usize,
    file_name: String,
    dst: SharedString,
    total: Option<u64>,
    cancel: Arc<AtomicBool>,
}

impl DownloadTaskCtx {
    fn task_id(&self) -> String {
        format!("sftp-transfer-{}", self.epoch)
    }
}

#[derive(Clone, Debug)]
enum UploadMsg {
    Progress {
        epoch: usize,
        sent: u64,
        total: u64,
    },
    Finished {
        epoch: usize,
    },
    Cancelled {
        epoch: usize,
    },
    Failed {
        epoch: usize,
        failure: UploadFailure,
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
    sent: u64,
    total: u64,
) {
    let _ = tx.send(UploadMsg::Progress { epoch, sent, total }).await;
}

async fn upload_send_cancelled(tx: &smol::channel::Sender<UploadMsg>, epoch: usize) {
    let _ = tx.send(UploadMsg::Cancelled { epoch }).await;
}

async fn upload_send_failed(
    tx: &smol::channel::Sender<UploadMsg>,
    epoch: usize,
    failure: UploadFailure,
) {
    let _ = tx.send(UploadMsg::Failed { epoch, failure }).await;
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
            upload_send_cancelled(tx, epoch).await;
            return UploadOutcome::Cancelled;
        }

        let n = match local_f.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => n,
            Err(err) => {
                upload_send_failed(
                    tx,
                    epoch,
                    UploadFailure::new(UploadFailureKind::ReadLocalFile, err.to_string()),
                )
                .await;
                return UploadOutcome::Failed;
            }
        };

        if cancel.load(Ordering::Relaxed) {
            upload_send_cancelled(tx, epoch).await;
            return UploadOutcome::Cancelled;
        }

        if let Err(err) = remote_f.write_all(&buf[..n]).await {
            upload_send_failed(
                tx,
                epoch,
                UploadFailure::new(UploadFailureKind::WriteRemoteFile, err.to_string()),
            )
            .await;
            return UploadOutcome::Failed;
        }

        sent = sent.saturating_add(n as u64);

        let now = Instant::now();
        if now.duration_since(last_emit_at) >= Duration::from_millis(200) {
            last_emit_at = now;
            upload_send_progress(tx, epoch, sent, total).await;
        }
    }

    let _ = remote_f.flush().await;
    UploadOutcome::Finished
}

fn is_remote_no_such_file(err: &SftpChannelError) -> bool {
    match err {
        SftpChannelError::Sftp(SftpError::NoSuchFile) => true,
        SftpChannelError::LibSsh(_) => err.to_string().contains("Sftp error code 2"),
        _ => false,
    }
}

async fn run_upload_worker(
    sftp: wezterm_ssh::Sftp,
    pool: gpui_common::PermitPool,
    file: PlannedUpload,
    tx: smol::channel::Sender<UploadMsg>,
) {
    let PlannedUpload {
        local,
        remote_path,
        total,
        epoch,
        cancel,
        ..
    } = file;

    let _permit = pool.acquire().await;

    let mut local_f = match smol::fs::File::open(&local).await {
        Ok(f) => f,
        Err(err) => {
            upload_send_failed(
                &tx,
                epoch,
                UploadFailure::new(UploadFailureKind::OpenLocalFile, err.to_string()),
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
            if is_remote_no_such_file(&err) {
                match open_create().await {
                    Ok(f) => f,
                    Err(err2) => {
                        upload_send_failed(
                            &tx,
                            epoch,
                            UploadFailure::new(UploadFailureKind::OpenRemoteFile, err2.to_string()),
                        )
                        .await;
                        return;
                    }
                }
            } else {
                upload_send_failed(
                    &tx,
                    epoch,
                    UploadFailure::new(UploadFailureKind::OpenRemoteFile, err.to_string()),
                )
                .await;
                return;
            }
        }
    };

    match upload_copy_loop(&mut local_f, &mut remote_f, &cancel, epoch, total, &tx).await {
        UploadOutcome::Finished => {
            upload_send_progress(&tx, epoch, total, total).await;
            let _ = tx.send(UploadMsg::Finished { epoch }).await;
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
    let mut planned = plan_uploads(&this, cx, &remote_dir, locals).await;
    if planned.is_empty() {
        show_upload_nothing_to_upload(&this, cx);
        return;
    }

    let total_files = planned.len();
    let batch_id = next_transfer_epoch();
    let group_id = format!("sftp-upload-batch-{batch_id}");

    for f in &mut planned {
        f.group_id = Some(group_id.clone());
        f.group_total = Some(total_files);
    }

    begin_upload_transfers(cx, &planned, &group_id, total_files);
    let result = run_upload_workers(cx, &sftp, &pool, planned, total_files).await;
    finish_upload_batch(&this, cx, remote_dir, total_files, result);
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
                this.delegate_mut().show_toast(
                    PromptLevel::Warning,
                    t!("Sftp.Toast.InvalidFilename").to_string(),
                    None,
                    cx,
                );
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
            group_id: None,
            group_total: None,
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
            t!("Sftp.Toast.NothingToUpload").to_string(),
            Some(t!("Sftp.Toast.NoValidFilesSelected").to_string()),
            cx,
        );
    });
}

fn begin_upload_transfers(
    cx: &mut gpui::AsyncApp,
    planned: &[PlannedUpload],
    group_id: &str,
    total_files: usize,
) {
    if !cx.has_global::<TransferCenterState>() {
        warn!("TransferCenterState global not installed; sftp transfer UI update skipped");
        return;
    }
    cx.update_global::<TransferCenterState, _>(|state, _cx| {
        for f in planned {
            let task = build_upload_task(
                &UploadTaskCtx {
                    epoch: f.epoch,
                    file_name: f.file_name.clone(),
                    remote_path: f.remote_path.clone().into(),
                    total: f.total,
                    cancel: Arc::clone(&f.cancel),
                    group_id: Some(group_id.to_string()),
                    group_total: Some(total_files),
                },
                TransferStatus::InProgress,
                TransferProgress::Indeterminate,
                Some(0),
                (f.total > 0).then_some(f.total),
                None,
            );
            state.upsert(task);
        }
    });
}

async fn run_upload_workers(
    cx: &mut gpui::AsyncApp,
    sftp: &wezterm_ssh::Sftp,
    pool: &gpui_common::PermitPool,
    planned: Vec<PlannedUpload>,
    total_files: usize,
) -> UploadBatchResult {
    let (tx, rx) = smol::channel::unbounded::<UploadMsg>();
    let mut completed: usize = 0;
    let mut result = UploadBatchResult::default();

    let ctx_by_epoch: HashMap<usize, UploadTaskCtx> = planned
        .iter()
        .map(|f| {
            let ctx = UploadTaskCtx {
                epoch: f.epoch,
                file_name: f.file_name.clone(),
                remote_path: f.remote_path.clone().into(),
                total: f.total,
                cancel: Arc::clone(&f.cancel),
                group_id: f.group_id.clone(),
                group_total: f.group_total,
            };
            (f.epoch, ctx)
        })
        .collect();

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
                epoch, sent, total, ..
            } => {
                if let Some(ctx) = ctx_by_epoch.get(&epoch) {
                    publish_upload_progress_to_center(cx, ctx, sent, total);
                }
            }
            UploadMsg::Finished { epoch, .. } => {
                completed = completed.saturating_add(1);
                result.uploaded = result.uploaded.saturating_add(1);
                if let Some(ctx) = ctx_by_epoch.get(&epoch) {
                    finish_upload_task_in_center(cx, ctx);
                }
            }
            UploadMsg::Cancelled { epoch, .. } => {
                completed = completed.saturating_add(1);
                result.cancelled = result.cancelled.saturating_add(1);
                if let Some(ctx) = ctx_by_epoch.get(&epoch) {
                    cancel_upload_task_in_center(cx, ctx);
                }
            }
            UploadMsg::Failed { epoch, failure } => {
                completed = completed.saturating_add(1);
                let file_name = ctx_by_epoch
                    .get(&epoch)
                    .map(|ctx| ctx.file_name.clone())
                    .unwrap_or_else(|| format!("#{epoch}"));
                result.record_failure(failure.clone(), file_name);
                if let Some(ctx) = ctx_by_epoch.get(&epoch) {
                    fail_upload_task_in_center(cx, ctx, failure.task_detail(ctx));
                }
            }
        }
    }

    result
}

fn publish_upload_progress_to_center(
    cx: &gpui::AsyncApp,
    ctx: &UploadTaskCtx,
    sent: u64,
    total: u64,
) {
    if !cx.has_global::<TransferCenterState>() {
        warn!("TransferCenterState global not installed; sftp transfer UI update skipped");
        return;
    }
    let progress = if total > 0 {
        TransferProgress::Determinate((sent as f32 / total as f32).clamp(0.0, 1.0))
    } else {
        TransferProgress::Indeterminate
    };
    let task = build_upload_task(
        ctx,
        TransferStatus::InProgress,
        progress,
        Some(sent),
        Some(total),
        None,
    );
    cx.update_global::<TransferCenterState, _>(|state, _cx| state.upsert(task));
}

fn finish_upload_task_in_center(cx: &gpui::AsyncApp, ctx: &UploadTaskCtx) {
    set_upload_terminal_in_center(cx, ctx, TransferStatus::Finished, None);
}

fn cancel_upload_task_in_center(cx: &gpui::AsyncApp, ctx: &UploadTaskCtx) {
    set_upload_terminal_in_center(cx, ctx, TransferStatus::Cancelled, None);
}

fn fail_upload_task_in_center(cx: &gpui::AsyncApp, ctx: &UploadTaskCtx, error: String) {
    set_upload_terminal_in_center(cx, ctx, TransferStatus::Failed, Some(error));
}

fn set_upload_terminal_in_center(
    cx: &gpui::AsyncApp,
    ctx: &UploadTaskCtx,
    status: TransferStatus,
    detail_override: Option<String>,
) {
    if !cx.has_global::<TransferCenterState>() {
        warn!("TransferCenterState global not installed; sftp transfer UI update skipped");
        return;
    }
    let task = build_upload_task(
        ctx,
        status,
        TransferProgress::Determinate(1.0),
        (status == TransferStatus::Finished).then_some(ctx.total),
        (ctx.total > 0).then_some(ctx.total),
        detail_override,
    );
    cx.update_global::<TransferCenterState, _>(|state, _cx| state.upsert(task));
    schedule_transfer_auto_dismiss(cx, ctx.task_id());
}

fn remove_transfer_if_terminal(state: &mut TransferCenterState, task_id: &str) {
    let should_remove = state
        .tasks_sorted()
        .iter()
        .find(|task| task.id == task_id)
        .is_some_and(|task| task.status != TransferStatus::InProgress);
    if should_remove {
        state.remove(task_id);
    }
}

fn schedule_transfer_auto_dismiss(cx: &gpui::AsyncApp, task_id: String) {
    cx.spawn(async move |cx| {
        Timer::after(AUTO_DISMISS_AFTER).await;
        if !cx.has_global::<TransferCenterState>() {
            return;
        }
        cx.update_global::<TransferCenterState, _>(|state, _cx| {
            remove_transfer_if_terminal(state, &task_id)
        });
    })
    .detach();
}

fn build_upload_task(
    ctx: &UploadTaskCtx,
    status: TransferStatus,
    progress: TransferProgress,
    bytes_done: Option<u64>,
    bytes_total: Option<u64>,
    detail_override: Option<String>,
) -> TransferTask {
    let mut task = TransferTask::new(ctx.task_id(), ctx.file_name.clone())
        .with_kind(TransferKind::Upload)
        .with_status(status)
        .with_progress(progress)
        .with_cancel_token(Arc::clone(&ctx.cancel))
        .with_bytes(bytes_done, bytes_total);
    let detail = detail_override
        .filter(|d| !d.trim().is_empty())
        .map(SharedString::from)
        .or_else(|| (!ctx.remote_path.as_ref().trim().is_empty()).then(|| ctx.remote_path.clone()));
    if let Some(detail) = detail {
        task = task.with_detail(detail);
    }
    if let Some(group_id) = ctx.group_id.as_deref() {
        task = task.with_group(group_id.to_string(), ctx.group_total);
    }
    task
}

fn classify_upload_batch(total_files: usize, result: &UploadBatchResult) -> PendingToast {
    if result.uploaded == total_files {
        let title = if total_files == 1 {
            t!("Sftp.Toast.UploadFinished").to_string()
        } else {
            t!("Sftp.Toast.UploadFinishedFiles", count = result.uploaded).to_string()
        };
        return PendingToast {
            level: PromptLevel::Info,
            title,
            detail: None,
        };
    }

    if result.failed == total_files {
        return PendingToast {
            level: PromptLevel::Critical,
            title: t!("Sftp.Toast.UploadFailed").to_string(),
            detail: result.failure_detail(),
        };
    }

    PendingToast {
        level: PromptLevel::Warning,
        title: t!(
            "Sftp.Toast.UploadIncomplete",
            uploaded = result.uploaded,
            failed = result.failed,
            cancelled = result.cancelled
        )
        .to_string(),
        detail: result.failure_detail(),
    }
}

fn finish_upload_batch(
    this: &gpui::WeakEntity<TableState<SftpTable>>,
    cx: &mut gpui::AsyncApp,
    remote_dir: String,
    total_files: usize,
    result: UploadBatchResult,
) {
    let toast = classify_upload_batch(total_files, &result);
    let _ = this.update(cx, |this, cx| {
        this.delegate_mut()
            .show_toast(toast.level, toast.title, toast.detail, cx);
        this.delegate_mut().refresh_dir(remote_dir.clone(), cx);
    });
}

fn begin_download_task(
    cx: &mut gpui::AsyncApp,
    file_name: String,
    total: Option<u64>,
    dst: &PathBuf,
) -> DownloadTaskCtx {
    let ctx = DownloadTaskCtx {
        epoch: next_transfer_epoch(),
        file_name,
        dst: dst.display().to_string().into(),
        total,
        cancel: Arc::new(AtomicBool::new(false)),
    };
    publish_download_progress_to_center(cx, &ctx, 0);
    ctx
}

fn publish_download_progress_to_center(cx: &gpui::AsyncApp, ctx: &DownloadTaskCtx, received: u64) {
    if !cx.has_global::<TransferCenterState>() {
        warn!("TransferCenterState global not installed; sftp transfer UI update skipped");
        return;
    }
    let progress = match ctx.total {
        Some(total) if total > 0 => {
            TransferProgress::Determinate((received as f32 / total as f32).clamp(0.0, 1.0))
        }
        _ => TransferProgress::Indeterminate,
    };
    let task = build_download_task(
        ctx,
        TransferStatus::InProgress,
        progress,
        Some(received),
        None,
    );
    cx.update_global::<TransferCenterState, _>(|state, _cx| state.upsert(task));
}

fn finish_download_task_in_center(cx: &gpui::AsyncApp, ctx: &DownloadTaskCtx) {
    set_download_terminal_in_center(cx, ctx, TransferStatus::Finished, None);
}

fn cancel_download_task_in_center(cx: &gpui::AsyncApp, ctx: &DownloadTaskCtx) {
    set_download_terminal_in_center(cx, ctx, TransferStatus::Cancelled, None);
}

fn fail_download_task_in_center(cx: &gpui::AsyncApp, ctx: &DownloadTaskCtx, error: String) {
    set_download_terminal_in_center(cx, ctx, TransferStatus::Failed, Some(error));
}

fn set_download_terminal_in_center(
    cx: &gpui::AsyncApp,
    ctx: &DownloadTaskCtx,
    status: TransferStatus,
    detail_override: Option<String>,
) {
    if !cx.has_global::<TransferCenterState>() {
        warn!("TransferCenterState global not installed; sftp transfer UI update skipped");
        return;
    }
    let task = build_download_task(
        ctx,
        status,
        TransferProgress::Determinate(1.0),
        None,
        detail_override,
    );
    cx.update_global::<TransferCenterState, _>(|state, _cx| state.upsert(task));
    schedule_transfer_auto_dismiss(cx, ctx.task_id());
}

fn build_download_task(
    ctx: &DownloadTaskCtx,
    status: TransferStatus,
    progress: TransferProgress,
    bytes_done: Option<u64>,
    detail_override: Option<String>,
) -> TransferTask {
    let mut task = TransferTask::new(ctx.task_id(), ctx.file_name.clone())
        .with_kind(TransferKind::Download)
        .with_status(status)
        .with_progress(progress)
        .with_cancel_token(Arc::clone(&ctx.cancel))
        .with_bytes(bytes_done, ctx.total.filter(|t| *t > 0));
    let detail = detail_override
        .filter(|d| !d.trim().is_empty())
        .map(SharedString::from)
        .or_else(|| (!ctx.dst.as_ref().trim().is_empty()).then(|| ctx.dst.clone()));
    if let Some(detail) = detail {
        task = task.with_detail(detail);
    }
    task
}

async fn download_copy_loop(
    cx: &mut gpui::AsyncApp,
    remote_f: &mut (impl smol::io::AsyncRead + Unpin),
    local_f: &mut (impl smol::io::AsyncWrite + Unpin),
    ctx: &DownloadTaskCtx,
) -> Option<u64> {
    let mut received: u64 = 0;
    let mut buf = vec![0u8; 64 * 1024];

    loop {
        if ctx.cancel.load(Ordering::Relaxed) {
            if ctx.total.is_some_and(|t| t != 0 && received >= t) {
                break;
            }
            cancel_download_task_in_center(cx, ctx);
            return None;
        }

        let n = match remote_f.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => n,
            Err(err) => {
                fail_download_task_in_center(
                    cx,
                    ctx,
                    t!("Sftp.Transfer.ReadRemoteFileFailed", err = err.to_string()).to_string(),
                );
                return None;
            }
        };

        if ctx.cancel.load(Ordering::Relaxed) {
            cancel_download_task_in_center(cx, ctx);
            return None;
        }

        if let Err(err) = local_f.write_all(&buf[..n]).await {
            fail_download_task_in_center(
                cx,
                ctx,
                t!("Sftp.Transfer.WriteLocalFileFailed", err = err.to_string()).to_string(),
            );
            return None;
        }

        received = received.saturating_add(n as u64);
        if ctx.cancel.load(Ordering::Relaxed) {
            cancel_download_task_in_center(cx, ctx);
            return None;
        }

        publish_download_progress_to_center(cx, ctx, received);
    }

    Some(received)
}

async fn finish_download_success(cx: &mut gpui::AsyncApp, ctx: &DownloadTaskCtx, received: u64) {
    publish_download_progress_to_center(cx, ctx, ctx.total.unwrap_or(received));
    Timer::after(Duration::from_millis(150)).await;
    finish_download_task_in_center(cx, ctx);
}

async fn download_to_path(
    cx: &mut gpui::AsyncApp,
    sftp: wezterm_ssh::Sftp,
    id: String,
    file_name: String,
    dst: PathBuf,
) {
    let total = sftp.metadata(&id).await.ok().and_then(|m| m.size);
    let ctx = begin_download_task(cx, file_name, total, &dst);

    let mut remote_f = match sftp.open(&id).await {
        Ok(f) => f,
        Err(err) => {
            fail_download_task_in_center(
                cx,
                &ctx,
                t!("Sftp.Transfer.OpenRemoteFileFailed", err = err.to_string()).to_string(),
            );
            return;
        }
    };

    let mut local_f = match smol::fs::File::create(&dst).await {
        Ok(f) => f,
        Err(err) => {
            fail_download_task_in_center(
                cx,
                &ctx,
                t!("Sftp.Transfer.CreateLocalFileFailed", err = err.to_string()).to_string(),
            );
            return;
        }
    };

    let Some(received) = download_copy_loop(cx, &mut remote_f, &mut local_f, &ctx).await else {
        return;
    };

    finish_download_success(cx, &ctx, received).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upload_batch_all_success_is_info() {
        let toast = classify_upload_batch(
            2,
            &UploadBatchResult {
                uploaded: 2,
                ..Default::default()
            },
        );

        assert!(matches!(toast.level, PromptLevel::Info));
        assert_eq!(toast.detail, None);
    }

    #[test]
    fn upload_batch_all_failed_is_critical_with_aggregated_error() {
        let mut result = UploadBatchResult::default();
        result.record_failure(
            UploadFailure::new(UploadFailureKind::OpenRemoteFile, "permission denied"),
            "a.txt".to_string(),
        );
        result.record_failure(
            UploadFailure::new(UploadFailureKind::OpenRemoteFile, "permission denied"),
            "b.txt".to_string(),
        );

        let toast = classify_upload_batch(2, &result);

        assert!(matches!(toast.level, PromptLevel::Critical));
        assert!(toast.detail.as_deref().is_some_and(|detail| {
            detail.contains("permission denied")
                && detail.contains("a.txt")
                && detail.contains("b.txt")
        }));
    }

    #[test]
    fn upload_batch_groups_failures_by_operation_and_cause() {
        let mut result = UploadBatchResult::default();
        let permission_denied =
            UploadFailure::new(UploadFailureKind::OpenRemoteFile, "permission denied");
        result.record_failure(permission_denied.clone(), "a.txt".to_string());
        result.record_failure(permission_denied, "b.txt".to_string());
        result.record_failure(
            UploadFailure::new(UploadFailureKind::WriteRemoteFile, "permission denied"),
            "c.txt".to_string(),
        );

        assert_eq!(result.failed, 3);
        assert_eq!(result.failure_groups.len(), 2);
        assert_eq!(result.failure_groups[0].files, ["a.txt", "b.txt"]);
        assert_eq!(result.failure_groups[1].files, ["c.txt"]);
    }

    #[test]
    fn upload_batch_partial_failure_is_warning() {
        let mut result = UploadBatchResult {
            uploaded: 1,
            cancelled: 1,
            ..Default::default()
        };
        result.record_failure(
            UploadFailure::new(UploadFailureKind::WriteRemoteFile, "disk full"),
            "large.bin".to_string(),
        );

        let toast = classify_upload_batch(3, &result);

        assert!(matches!(toast.level, PromptLevel::Warning));
        assert!(
            toast
                .detail
                .as_deref()
                .is_some_and(|detail| detail.contains("disk full") && detail.contains("large.bin"))
        );
    }

    #[test]
    fn upload_batch_cancellation_is_incomplete_not_failed() {
        let toast = classify_upload_batch(
            2,
            &UploadBatchResult {
                uploaded: 1,
                cancelled: 1,
                ..Default::default()
            },
        );

        assert!(matches!(toast.level, PromptLevel::Warning));
        assert_eq!(toast.detail, None);
    }

    #[test]
    fn terminal_auto_dismiss_removes_failed_task_only() {
        let mut state = TransferCenterState::default();
        state.upsert(TransferTask::new("failed", "failed").with_status(TransferStatus::Failed));
        state.upsert(TransferTask::new("active", "active"));

        remove_transfer_if_terminal(&mut state, "failed");
        remove_transfer_if_terminal(&mut state, "active");

        assert_eq!(
            state
                .tasks_sorted()
                .iter()
                .map(|task| task.id.as_str())
                .collect::<Vec<_>>(),
            vec!["active"]
        );
    }
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
        let input = new_input(window, cx, t!("Sftp.Placeholder.FolderName").to_string());
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

        let input = new_configured_input(
            window,
            cx,
            t!("Sftp.Placeholder.NewName").to_string(),
            |input| input.default_value(row.name.clone()),
        );

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
            self.show_toast(
                PromptLevel::Warning,
                t!("Sftp.Toast.Disconnected").to_string(),
                None,
                cx,
            );
            return;
        };

        let name = op.input.read(cx).value().to_string();
        let name = name.trim().to_string();
        if name.is_empty() {
            self.show_toast(
                PromptLevel::Info,
                t!("Sftp.Toast.NameRequired").to_string(),
                None,
                cx,
            );
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
                                t!("Sftp.Toast.FolderCreated").to_string(),
                                None,
                                cx,
                            );
                            this.delegate_mut().refresh_dir(parent.clone(), cx);
                        }
                        Err(err) => this.delegate_mut().show_toast(
                            PromptLevel::Warning,
                            t!("Sftp.Toast.CreateFolderFailed").to_string(),
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
                            this.delegate_mut().show_toast(
                                PromptLevel::Info,
                                t!("Sftp.Toast.Renamed").to_string(),
                                None,
                                cx,
                            );
                            this.delegate_mut().refresh_dir(parent.clone(), cx);
                        }
                        Err(err) => this.delegate_mut().show_toast(
                            PromptLevel::Warning,
                            t!("Sftp.Toast.RenameFailed").to_string(),
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
            self.show_toast(
                PromptLevel::Warning,
                t!("Sftp.Toast.Disconnected").to_string(),
                None,
                cx,
            );
            return;
        };
        let Some(remote_dir) = self.selected_dir_for_new_entries(target_row) else {
            return;
        };

        let picker = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some(t!("Sftp.Dialog.SelectFilesToUpload").to_string().into()),
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

            let Ok(Ok(Some(paths))) = picked else {
                return;
            };
            if paths.is_empty() {
                return;
            }
            let _ = this.update(cx, |this, cx| {
                this.delegate_mut()
                    .upload_local_files_to_dir(remote_dir.clone(), paths, cx);
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
            self.show_toast(
                PromptLevel::Warning,
                t!("Sftp.Toast.Disconnected").to_string(),
                None,
                cx,
            );
            return;
        };
        if !accept_external_file_drop_paths(&locals) {
            self.show_toast(
                PromptLevel::Info,
                t!("Sftp.Toast.OnlyFilesSupported").to_string(),
                Some(t!("Sftp.Toast.DraggingFoldersUnsupported").to_string()),
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
            self.show_toast(
                PromptLevel::Warning,
                t!("Sftp.Toast.Disconnected").to_string(),
                None,
                cx,
            );
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

        cx.spawn(async move |_this, cx| {
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

            download_to_path(cx, sftp, id, file_name, dst).await;
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
            self.show_toast(
                PromptLevel::Warning,
                t!("Sftp.Toast.Disconnected").to_string(),
                None,
                cx,
            );
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
                        t!("Sftp.Toast.Deleted").to_string()
                    } else {
                        t!("Sftp.Toast.DeletedItems", count = deleted).to_string()
                    };
                    this.delegate_mut()
                        .show_toast(PromptLevel::Info, title, None, cx);
                } else {
                    let title = t!(
                        "Sftp.Toast.DeletedPartial",
                        deleted = deleted,
                        total = total
                    )
                    .to_string();
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
