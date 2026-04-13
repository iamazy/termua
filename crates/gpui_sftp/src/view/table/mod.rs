use super::*;

mod actions;
mod delegate;
mod navigation;
mod transfers;

impl SftpTable {
    fn show_toast(
        &mut self,
        level: PromptLevel,
        title: impl Into<String>,
        detail: Option<String>,
        cx: &mut Context<TableState<Self>>,
    ) {
        self.pending_toast_epoch = self.pending_toast_epoch.wrapping_add(1);
        self.pending_toast = Some(PendingToast {
            level,
            title: title.into(),
            detail,
        });
        cx.notify();
    }
}
