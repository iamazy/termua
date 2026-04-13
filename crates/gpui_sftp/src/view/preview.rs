use super::{format::fenced_text_as_markdown, *};
use crate::preview::{PreviewGate, PreviewKind, gate_preview, read_bytes_with_limit};

#[derive(Clone, Debug)]
pub(super) struct PreviewTarget {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) kind: EntryKind,
    pub(super) size: Option<u64>,
}

#[derive(Clone)]
pub(super) enum PreviewContent {
    Empty,
    Loading,
    Binary,
    Image { image: Arc<Image> },
    Text { fenced_markdown: SharedString },
    Error { message: SharedString },
}

#[derive(Clone)]
pub(super) struct PreviewPane {
    pub(super) target: Option<PreviewTarget>,
    pub(super) content: PreviewContent,
}

impl SftpView {
    pub(super) fn close_preview(&mut self, cx: &mut Context<Self>) {
        if !self.show_preview && self.preview.target.is_none() {
            return;
        }

        self.show_preview = false;
        // Cancel any in-flight preview tasks.
        self.preview_epoch = self.preview_epoch.wrapping_add(1);

        self.preview = PreviewPane {
            target: None,
            content: PreviewContent::Empty,
        };
        cx.notify();
    }

    pub(super) fn request_preview(
        &mut self,
        target: Option<PreviewTarget>,
        cx: &mut Context<Self>,
    ) {
        enum PreviewOutcome {
            Hide,
            Content(PreviewContent),
            Error(SharedString),
        }

        async fn load_preview_outcome(
            sftp: wezterm_ssh::Sftp,
            target: &PreviewTarget,
            kind: PreviewKind,
            limit_bytes: usize,
        ) -> PreviewOutcome {
            let mut remote_f = match sftp.open(&target.id).await {
                Ok(f) => f,
                Err(err) => {
                    return PreviewOutcome::Error(format!("Open failed: {err}").into());
                }
            };

            let (bytes, truncated) = match read_bytes_with_limit(&mut remote_f, limit_bytes).await {
                Ok(res) => res,
                Err(err) => {
                    return PreviewOutcome::Error(format!("Read failed: {err}").into());
                }
            };

            if truncated {
                return PreviewOutcome::Hide;
            }

            let content = match kind {
                PreviewKind::Image(format) => {
                    let image = Arc::new(Image::from_bytes(format, bytes));
                    PreviewContent::Image { image }
                }
                PreviewKind::Text => {
                    if bytes.contains(&0) {
                        return PreviewOutcome::Content(PreviewContent::Binary);
                    }

                    let s = String::from_utf8_lossy(&bytes).to_string();
                    let fenced = fenced_text_as_markdown(&target.name, &s);
                    PreviewContent::Text {
                        fenced_markdown: fenced.into(),
                    }
                }
            };

            PreviewOutcome::Content(content)
        }

        if !self.show_preview {
            return;
        }

        let gate = target
            .as_ref()
            .map(|t| gate_preview(self.show_preview, &t.name, t.kind, t.size))
            .unwrap_or(PreviewGate::Hidden);

        let PreviewGate::Allowed { kind, limit_bytes } = gate else {
            // If the target isn't previewable, keep the UI clean by hiding the preview pane
            // instead of showing an "unsupported" placeholder panel.
            self.close_preview(cx);
            return;
        };

        let epoch = self.preview_epoch.wrapping_add(1);
        self.preview_epoch = epoch;

        self.preview.target = target.clone();
        self.preview.content = PreviewContent::Loading;
        cx.notify();

        let Some(target) = target else {
            return;
        };
        let sftp = self.table.read(cx).delegate().sftp.clone();

        cx.spawn(async move |this, cx| {
            let outcome = match sftp {
                Some(sftp) => load_preview_outcome(sftp, &target, kind, limit_bytes).await,
                None => PreviewOutcome::Error("Disconnected".into()),
            };

            let _ = this.update(cx, |this, cx| {
                if this.preview_epoch != epoch {
                    return;
                }

                match outcome {
                    PreviewOutcome::Hide => this.close_preview(cx),
                    PreviewOutcome::Content(content) => {
                        if matches!(content, PreviewContent::Binary) {
                            // Don't show a preview pane when the content isn't previewable.
                            this.close_preview(cx);
                            return;
                        }
                        this.preview = PreviewPane {
                            target: Some(target.clone()),
                            content,
                        };
                        cx.notify();
                    }
                    PreviewOutcome::Error(message) => {
                        this.preview = PreviewPane {
                            target: Some(target.clone()),
                            content: PreviewContent::Error { message },
                        };
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }
}
