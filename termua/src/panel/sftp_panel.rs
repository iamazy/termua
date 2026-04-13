use std::sync::Arc;

use gpui::{
    App, AppContext, Context, EventEmitter, FocusHandle, Focusable, InteractiveElement,
    IntoElement, ParentElement, Render, Styled, Subscription, Window, div,
};
use gpui_common::TermuaIcon;
use gpui_dock::{Panel, PanelEvent, PanelView, TabIcon};
use gpui_sftp::SftpView;
use gpui_term::{Event as TerminalEvent, Terminal, TerminalView};

use crate::lock_screen::LockState;

pub struct SftpDockPanel {
    tab_label: gpui::SharedString,
    focus_handle: FocusHandle,
    sftp_view: gpui::Entity<SftpView>,
    _subscriptions: Vec<Subscription>,
}

impl SftpDockPanel {
    pub fn open_for_terminal_view<T: 'static>(
        terminal_view: gpui::Entity<TerminalView>,
        tab_label: gpui::SharedString,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> anyhow::Result<Arc<dyn PanelView>> {
        let Some(sftp) = terminal_view.read(cx).terminal.read(cx).sftp() else {
            anyhow::bail!("SFTP is only available for SSH terminals");
        };

        let terminal: gpui::Entity<Terminal> = terminal_view.read(cx).terminal.clone();

        let panel = cx.new(|cx| {
            let focus_handle = cx.focus_handle();
            let sftp_view = cx.new(|cx| SftpView::new(sftp, window, cx));

            let sub = cx.subscribe_in(&terminal, window, {
                let sftp_view = sftp_view.clone();
                move |_, _terminal, ev, _window, cx| {
                    if matches!(ev, TerminalEvent::CloseTerminal) {
                        sftp_view.update(cx, |view, cx| view.disconnect(cx));
                    }
                }
            });

            Self {
                tab_label,
                focus_handle,
                sftp_view,
                _subscriptions: vec![sub],
            }
        });

        Ok(Arc::new(panel) as Arc<dyn PanelView>)
    }
}

impl EventEmitter<PanelEvent> for SftpDockPanel {}

impl Focusable for SftpDockPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for SftpDockPanel {
    fn panel_name(&self) -> &'static str {
        "termua.sftp_dock_panel"
    }

    fn tab_icon(&self, _cx: &App) -> Option<TabIcon> {
        Some(TabIcon::ColoredSvg {
            path: TermuaIcon::FolderClosedBlue.into(),
        })
    }

    fn tab_name(&self, _cx: &App) -> Option<gpui::SharedString> {
        Some(self.tab_label.clone())
    }

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().child(self.tab_label.clone())
    }

    fn set_active(&mut self, active: bool, window: &mut Window, cx: &mut Context<Self>) {
        if active {
            let focus = self.sftp_view.read(cx).focus_handle(cx);
            window.focus(&focus, cx);
        }
    }
}

impl Render for SftpDockPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .track_focus(&self.focus_handle(cx))
            .on_any_mouse_down(move |_, _window, cx| {
                if cx.try_global::<LockState>().is_some() {
                    cx.global::<LockState>().report_activity();
                }
            })
            .on_mouse_move(move |_ev, _window, cx| {
                if cx.try_global::<LockState>().is_some() {
                    cx.global::<LockState>().report_activity();
                }
            })
            .on_key_down(cx.listener(move |_, ev: &gpui::KeyDownEvent, _window, cx| {
                if ev.is_held {
                    return;
                }
                if cx.try_global::<LockState>().is_some() {
                    cx.global::<LockState>().report_activity();
                }
            }))
            .child(self.sftp_view.clone())
    }
}
