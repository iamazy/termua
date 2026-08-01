use std::sync::Arc;

use gpui::{
    App, AppContext, Context, EventEmitter, FocusHandle, Focusable, InteractiveElement,
    IntoElement, ParentElement, Render, Styled, Subscription, Window, div,
};
use gpui_common::TermuaIcon;
use gpui_dock::{Panel, PanelEvent, PanelInfo, PanelState, PanelView, TabIcon};
use gpui_sftp::{SftpEvent, SftpView};
use gpui_term::{Event as TerminalEvent, Terminal, TerminalView};

use crate::{lock_screen::LockState, notification};

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct SftpPanelState {
    pub(crate) version: usize,
    pub(crate) tab_label: String,
    pub(crate) terminal_id: usize,
    pub(crate) current_dir: Option<String>,
}

pub struct SftpDockPanel {
    tab_label: gpui::SharedString,
    terminal_id: usize,
    focus_handle: FocusHandle,
    sftp_view: Option<gpui::Entity<SftpView>>,
    restored_current_dir: Option<String>,
    _subscriptions: Vec<Subscription>,
}

fn sftp_event_message(event: &SftpEvent) -> Option<(notification::MessageKind, String)> {
    match event {
        SftpEvent::Toast {
            level,
            title,
            detail,
        } => {
            let kind = match level {
                gpui::PromptLevel::Info => notification::MessageKind::Info,
                gpui::PromptLevel::Warning => notification::MessageKind::Warning,
                gpui::PromptLevel::Critical => notification::MessageKind::Error,
            };
            let message = match detail.as_deref() {
                Some(detail) if !detail.trim().is_empty() => format!("{title}\n{detail}"),
                _ => title.clone(),
            };
            Some((kind, message))
        }
    }
}

impl SftpDockPanel {
    fn subscribe_footbar_status(
        sftp_view: &gpui::Entity<SftpView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> [Subscription; 2] {
        let status_sub = cx.observe_in(sftp_view, window, |_this, view, _window, cx| {
            let status = view.read(cx).status(cx);
            crate::footbar::update_sftp_status(cx.entity_id(), status, cx);
        });
        let sftp_focus = sftp_view.read(cx).focus_handle(cx);
        let focus_in_sub = cx.on_focus_in(&sftp_focus, window, {
            let sftp_view = sftp_view.clone();
            move |_this, _window, cx| {
                let status = sftp_view.read(cx).status(cx);
                crate::footbar::focus_sftp_status(cx.entity_id(), status, cx);
            }
        });

        [status_sub, focus_in_sub]
    }

    pub fn open_for_terminal_view<T: 'static>(
        terminal_view: gpui::Entity<TerminalView>,
        tab_label: gpui::SharedString,
        terminal_id: usize,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> anyhow::Result<Arc<dyn PanelView>> {
        let Some(sftp) = terminal_view.read(cx).terminal.read(cx).sftp() else {
            anyhow::bail!("SFTP is only available for SSH terminals");
        };

        let terminal: gpui::Entity<Terminal> = terminal_view.read(cx).terminal.clone();

        let panel = cx.new(|cx: &mut Context<Self>| {
            let focus_handle = cx.focus_handle();
            let sftp_view = cx.new(|cx| SftpView::new(sftp, window, cx));

            let terminal_sub = cx.subscribe_in(&terminal, window, {
                let sftp_view = sftp_view.clone();
                move |_, _terminal, ev, _window, cx| {
                    if matches!(ev, TerminalEvent::CloseTerminal) {
                        sftp_view.update(cx, |view, cx| view.disconnect(cx));
                    }
                }
            });
            let toast_sub = cx.subscribe_in(&sftp_view, window, {
                move |_, _sftp_view, ev, _window, cx| {
                    let Some((kind, message)) = sftp_event_message(ev) else {
                        return;
                    };
                    notification::record(kind, message, cx);
                }
            });
            let footbar_subs = Self::subscribe_footbar_status(&sftp_view, window, cx);

            let mut subscriptions = vec![terminal_sub, toast_sub];
            subscriptions.extend(footbar_subs);

            Self {
                tab_label,
                terminal_id,
                focus_handle,
                sftp_view: Some(sftp_view),
                restored_current_dir: None,
                _subscriptions: subscriptions,
            }
        });

        Ok(Arc::new(panel) as Arc<dyn PanelView>)
    }

    pub(crate) fn restoring(state: SftpPanelState, cx: &mut Context<Self>) -> Self {
        Self {
            tab_label: state.tab_label.into(),
            terminal_id: state.terminal_id,
            focus_handle: cx.focus_handle(),
            sftp_view: None,
            restored_current_dir: state.current_dir,
            _subscriptions: Vec::new(),
        }
    }

    pub(crate) fn terminal_id(&self) -> usize {
        self.terminal_id
    }

    pub(crate) fn is_connected(&self) -> bool {
        self.sftp_view.is_some()
    }

    pub(crate) fn connect_to_terminal_view(
        &mut self,
        terminal_view: gpui::Entity<TerminalView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        let Some(sftp) = terminal_view.read(cx).terminal.read(cx).sftp() else {
            anyhow::bail!("SFTP is only available for SSH terminals");
        };
        let terminal: gpui::Entity<Terminal> = terminal_view.read(cx).terminal.clone();
        let sftp_view = cx.new(|cx| SftpView::new(sftp, window, cx));
        if let Some(dir) = self.restored_current_dir.take() {
            sftp_view.update(cx, |view, cx| view.change_dir(dir, cx));
        }

        self._subscriptions
            .push(cx.subscribe_in(&terminal, window, {
                let sftp_view = sftp_view.clone();
                move |_, _terminal, ev, _window, cx| {
                    if matches!(ev, TerminalEvent::CloseTerminal) {
                        sftp_view.update(cx, |view, cx| view.disconnect(cx));
                    }
                }
            }));
        self._subscriptions.push(cx.subscribe_in(
            &sftp_view,
            window,
            move |_, _sftp_view, ev, _window, cx| {
                let Some((kind, message)) = sftp_event_message(ev) else {
                    return;
                };
                notification::record(kind, message, cx);
            },
        ));
        self._subscriptions
            .extend(Self::subscribe_footbar_status(&sftp_view, window, cx));
        self.sftp_view = Some(sftp_view);
        cx.notify();
        Ok(())
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
        super::SFTP_PANEL_NAME
    }

    fn persistable(&self, _cx: &App) -> bool {
        false
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
            if let Some(sftp_view) = &self.sftp_view {
                let status = sftp_view.read(cx).status(cx);
                crate::footbar::focus_sftp_status(cx.entity_id(), status, cx);
                let focus = sftp_view.read(cx).focus_handle(cx);
                window.focus(&focus, cx);
            }
        } else {
            crate::footbar::blur_sftp_status(cx.entity_id(), cx);
        }
    }

    fn on_removed(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        crate::footbar::blur_sftp_status(cx.entity_id(), cx);
    }

    fn dump(&self, cx: &App) -> PanelState {
        let current_dir = self
            .sftp_view
            .as_ref()
            .and_then(|view| view.read(cx).current_dir(cx))
            .or_else(|| self.restored_current_dir.clone());
        let mut state = PanelState::new(self);
        state.info = PanelInfo::panel(
            serde_json::to_value(SftpPanelState {
                version: 1,
                tab_label: self.tab_label.to_string(),
                terminal_id: self.terminal_id,
                current_dir,
            })
            .expect("sftp panel state should serialize"),
        );
        state
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
            .child(match &self.sftp_view {
                Some(view) => div().size_full().child(view.clone()),
                None => div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child("Waiting for SSH connection..."),
            })
    }
}

#[cfg(test)]
mod tests {
    use gpui::AppContext as _;
    use gpui_dock::{Panel as _, PanelInfo};

    use super::*;

    #[gpui::test]
    fn sftp_panel_is_not_persistable(cx: &mut gpui::TestAppContext) {
        let panel = cx.new(|cx| {
            SftpDockPanel::restoring(
                SftpPanelState {
                    version: 1,
                    tab_label: "prod".to_string(),
                    terminal_id: 42,
                    current_dir: None,
                },
                cx,
            )
        });

        assert!(!panel.read_with(cx, |panel, app| panel.persistable(app)));
    }

    #[test]
    fn sftp_panel_state_round_trips_terminal_link_and_directory() {
        let state = SftpPanelState {
            version: 1,
            tab_label: "prod".to_string(),
            terminal_id: 42,
            current_dir: Some("/srv/app".to_string()),
        };

        let json = serde_json::to_value(&state).expect("serialize sftp panel state");
        let restored: SftpPanelState =
            serde_json::from_value(json).expect("deserialize sftp panel state");

        assert_eq!(restored, state);
    }

    #[gpui::test]
    fn restoring_sftp_panel_exposes_metadata_and_dumps_directory(cx: &mut gpui::TestAppContext) {
        let panel = cx.new(|cx| {
            SftpDockPanel::restoring(
                SftpPanelState {
                    version: 1,
                    tab_label: "prod".to_string(),
                    terminal_id: 42,
                    current_dir: Some("/srv/app".to_string()),
                },
                cx,
            )
        });

        assert_eq!(panel.read_with(cx, |panel, _| panel.terminal_id()), 42);
        assert!(!panel.read_with(cx, |panel, _| panel.is_connected()));
        assert_eq!(
            panel.read_with(cx, |panel, app| panel.tab_name(app)),
            Some("prod".into())
        );
        assert_eq!(
            panel.read_with(cx, |panel, _| panel.panel_name()),
            super::super::SFTP_PANEL_NAME
        );

        let dumped = panel.read_with(cx, |panel, app| panel.dump(app));
        let PanelInfo::Panel(value) = dumped.info else {
            panic!("expected sftp panel state");
        };
        assert_eq!(
            serde_json::from_value::<SftpPanelState>(value).unwrap(),
            SftpPanelState {
                version: 1,
                tab_label: "prod".to_string(),
                terminal_id: 42,
                current_dir: Some("/srv/app".to_string()),
            }
        );
    }

    #[test]
    fn sftp_toast_event_message_includes_detail_for_message_center() {
        let event = SftpEvent::Toast {
            level: gpui::PromptLevel::Info,
            title: "Moved".to_string(),
            detail: Some("File: a.txt\nFrom: /from/a.txt\nTo: /to/a.txt".to_string()),
        };

        let (kind, message) = sftp_event_message(&event).expect("expected message");

        assert_eq!(kind, notification::MessageKind::Info);
        assert!(message.contains("Moved"), "message={message:?}");
        assert!(message.contains("a.txt"), "message={message:?}");
        assert!(message.contains("/from/a.txt"), "message={message:?}");
        assert!(message.contains("/to/a.txt"), "message={message:?}");
    }

    #[test]
    fn sftp_toast_event_message_maps_levels_and_ignores_blank_detail() {
        let cases = [
            (
                gpui::PromptLevel::Warning,
                notification::MessageKind::Warning,
            ),
            (
                gpui::PromptLevel::Critical,
                notification::MessageKind::Error,
            ),
        ];

        for (level, expected_kind) in cases {
            let event = SftpEvent::Toast {
                level,
                title: "Transfer failed".to_string(),
                detail: Some("   ".to_string()),
            };
            assert_eq!(
                sftp_event_message(&event),
                Some((expected_kind, "Transfer failed".to_string()))
            );
        }
    }
}
