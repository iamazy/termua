use gpui::{
    App, Context, FocusHandle, Focusable, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, Styled, WeakEntity, Window, div,
};
use gpui_common::TermuaIcon;
use gpui_component::{ActiveTheme as _, v_flex};
use gpui_dock::{Panel, PanelEvent, PanelInfo, PanelState, TabPanel};

use super::{PanelKind, TerminalLaunchState, TerminalPanelState};
use crate::panel::terminal_panel::tab_icon_for_terminal_panel_with_launch;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SshErrorPanelStatus {
    Restoring,
    Error,
}

pub(crate) struct SshErrorPanel {
    id: usize,
    tab_label: SharedString,
    tab_tooltip: Option<SharedString>,
    message: SharedString,
    terminal_state: Option<TerminalPanelState>,
    status: SshErrorPanelStatus,
    parent_tab: Option<WeakEntity<TabPanel>>,
    focus_handle: FocusHandle,
}

impl SshErrorPanel {
    pub(crate) fn id(&self) -> usize {
        self.id
    }

    pub(crate) fn new(
        id: usize,
        tab_label: SharedString,
        tab_tooltip: Option<SharedString>,
        message: SharedString,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            id,
            tab_label,
            tab_tooltip,
            message,
            terminal_state: None,
            status: SshErrorPanelStatus::Error,
            parent_tab: None,
            focus_handle: cx.focus_handle(),
        }
    }

    pub(crate) fn restoring(
        state: TerminalPanelState,
        message: SharedString,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            id: state.id,
            tab_label: state.tab_label.clone().into(),
            tab_tooltip: state.tab_tooltip.clone().map(Into::into),
            message,
            terminal_state: Some(state),
            status: SshErrorPanelStatus::Restoring,
            parent_tab: None,
            focus_handle: cx.focus_handle(),
        }
    }

    pub(crate) fn with_terminal_error(
        state: TerminalPanelState,
        message: SharedString,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut panel = Self::restoring(state, message, cx);
        panel.status = SshErrorPanelStatus::Error;
        panel
    }

    pub(crate) fn terminal_state(&self) -> Option<TerminalPanelState> {
        self.terminal_state.clone()
    }

    pub(crate) fn parent_tab(&self) -> Option<WeakEntity<TabPanel>> {
        self.parent_tab.clone()
    }

    pub(crate) fn set_message(&mut self, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.message = message.into();
        self.status = SshErrorPanelStatus::Error;
        cx.notify();
    }
}

impl Drop for SshErrorPanel {
    fn drop(&mut self) {
        log::debug!("termua: SshErrorPanel drop (id={})", self.id);
    }
}

impl gpui::EventEmitter<PanelEvent> for SshErrorPanel {}

impl Focusable for SshErrorPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for SshErrorPanel {
    fn panel_name(&self) -> &'static str {
        super::SSH_ERROR_PANEL_NAME
    }

    fn tab_icon(&self, _cx: &App) -> Option<gpui_dock::TabIcon> {
        if self.status == SshErrorPanelStatus::Restoring
            && let Some(state) = &self.terminal_state
        {
            let kind = match state.launch {
                TerminalLaunchState::Local { .. } => PanelKind::Local,
                TerminalLaunchState::Ssh { .. } => PanelKind::Ssh,
                TerminalLaunchState::Serial { .. } => PanelKind::Serial,
                TerminalLaunchState::Recorder { .. } => PanelKind::Recorder,
            };
            return Some(tab_icon_for_terminal_panel_with_launch(
                kind,
                Some(&state.launch),
            ));
        }

        Some(gpui_dock::TabIcon::Monochrome {
            path: TermuaIcon::Bug.into(),
            color: Some(gpui::red()),
        })
    }

    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        Some(self.tab_label.clone())
    }

    fn tab_tooltip(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<impl IntoElement> {
        let tooltip = self.tab_tooltip.clone()?;
        Some(div().child(tooltip))
    }

    fn title(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.tab_name(cx).unwrap_or_else(|| "ssh".into())
    }

    fn on_added_to(
        &mut self,
        tab_panel: WeakEntity<TabPanel>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        self.parent_tab = Some(tab_panel);
    }

    fn dump(&self, _cx: &App) -> PanelState {
        let Some(terminal_state) = self.terminal_state.clone() else {
            return PanelState::new(self);
        };
        PanelState {
            panel_name: super::TERMINAL_PANEL_NAME.to_string(),
            children: Vec::new(),
            info: PanelInfo::panel(
                serde_json::to_value(terminal_state)
                    .expect("restoring terminal state should serialize"),
            ),
        }
    }
}

impl Render for SshErrorPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("termua-ssh-error-panel")
            .debug_selector(|| "termua-ssh-error-panel".to_string())
            .size_full()
            .justify_center()
            .items_center()
            .gap_2()
            .text_color(cx.theme().muted_foreground)
            .child(self.message.clone())
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, path::PathBuf};

    use gpui::AppContext as _;
    use gpui_dock::{Panel as _, PanelInfo};

    use super::*;
    use crate::panel::{
        PanelKind, TerminalLaunchState, terminal_panel::tab_icon_for_terminal_panel,
    };

    fn terminal_state(launch: TerminalLaunchState) -> TerminalPanelState {
        TerminalPanelState {
            version: 1,
            id: 7,
            tab_label: "saved terminal".to_string(),
            tab_tooltip: Some("saved tooltip".to_string()),
            launch,
        }
    }

    #[gpui::test]
    fn restoring_ssh_panel_uses_ssh_terminal_icon(cx: &mut gpui::TestAppContext) {
        let panel = cx.new(|cx| {
            SshErrorPanel::restoring(
                TerminalPanelState {
                    version: 1,
                    id: 1,
                    tab_label: "ssh".to_string(),
                    tab_tooltip: None,
                    launch: TerminalLaunchState::Ssh {
                        backend_type: gpui_term::TerminalType::WezTerm,
                        session_id: Some(1),
                    },
                },
                "Restoring...".into(),
                cx,
            )
        });

        assert_eq!(
            panel.read_with(cx, |panel, app| panel.tab_icon(app)),
            Some(tab_icon_for_terminal_panel(PanelKind::Ssh))
        );
    }

    #[gpui::test]
    fn restoring_powershell_panel_uses_powershell_terminal_icon(cx: &mut gpui::TestAppContext) {
        let panel = cx.new(|cx| {
            SshErrorPanel::restoring(
                terminal_state(TerminalLaunchState::Local {
                    backend_type: gpui_term::TerminalType::WezTerm,
                    env: HashMap::from([("TERMUA_SHELL".to_string(), "pwsh".to_string())]),
                }),
                "Restoring...".into(),
                cx,
            )
        });

        assert!(matches!(
            panel.read_with(cx, |panel, app| panel.tab_icon(app)),
            Some(gpui_dock::TabIcon::Monochrome { path, color: None })
                if path.as_ref() == TermuaIcon::Pwsh.path()
        ));
    }

    #[gpui::test]
    fn restoring_panels_use_their_terminal_icons(cx: &mut gpui::TestAppContext) {
        let cases = [
            (
                TerminalLaunchState::Local {
                    backend_type: gpui_term::TerminalType::Alacritty,
                    env: HashMap::new(),
                },
                PanelKind::Local,
            ),
            (
                TerminalLaunchState::Serial {
                    backend_type: gpui_term::TerminalType::Alacritty,
                    params: crate::SerialParams {
                        name: "serial".to_string(),
                        port: "test".to_string(),
                        baud: 9600,
                        data_bits: 8,
                        parity: crate::store::SerialParity::None,
                        stop_bits: crate::store::SerialStopBits::One,
                        flow_control: crate::store::SerialFlowControl::None,
                    },
                    session_id: None,
                },
                PanelKind::Serial,
            ),
            (
                TerminalLaunchState::Recorder {
                    cast_path: PathBuf::from("recording.cast"),
                    playback_speed: 1.0,
                },
                PanelKind::Recorder,
            ),
        ];

        for (launch, kind) in cases {
            let panel = cx.new(|cx| {
                SshErrorPanel::restoring(terminal_state(launch), "Restoring...".into(), cx)
            });
            assert_eq!(
                panel.read_with(cx, |panel, app| panel.tab_icon(app)),
                Some(tab_icon_for_terminal_panel(kind))
            );
        }
    }

    #[gpui::test]
    fn setting_message_changes_restoring_panel_to_error(cx: &mut gpui::TestAppContext) {
        let panel = cx.new(|cx| {
            SshErrorPanel::restoring(
                terminal_state(TerminalLaunchState::Ssh {
                    backend_type: gpui_term::TerminalType::WezTerm,
                    session_id: Some(1),
                }),
                "Restoring...".into(),
                cx,
            )
        });

        panel.update(cx, |panel, cx| panel.set_message("failed", cx));

        assert_eq!(
            panel.read_with(cx, |panel, app| panel.tab_icon(app)),
            Some(gpui_dock::TabIcon::Monochrome {
                path: TermuaIcon::Bug.into(),
                color: Some(gpui::red()),
            })
        );
    }

    #[gpui::test]
    fn dump_preserves_restoring_terminal_state(cx: &mut gpui::TestAppContext) {
        let state = terminal_state(TerminalLaunchState::Ssh {
            backend_type: gpui_term::TerminalType::WezTerm,
            session_id: Some(12),
        });
        let panel = cx.new(|cx| SshErrorPanel::restoring(state.clone(), "Restoring...".into(), cx));

        let dumped = panel.read_with(cx, |panel, app| panel.dump(app));

        assert_eq!(dumped.panel_name, super::super::TERMINAL_PANEL_NAME);
        let PanelInfo::Panel(value) = dumped.info else {
            panic!("expected terminal panel state");
        };
        assert_eq!(
            serde_json::from_value::<TerminalPanelState>(value).unwrap(),
            state
        );
    }

    #[gpui::test]
    fn new_error_panel_dumps_its_own_panel_state(cx: &mut gpui::TestAppContext) {
        let panel = cx.new(|cx| {
            SshErrorPanel::new(
                3,
                "failed".into(),
                Some("details".into()),
                "connection failed".into(),
                cx,
            )
        });

        let dumped = panel.read_with(cx, |panel, app| panel.dump(app));

        assert_eq!(dumped.panel_name, super::super::SSH_ERROR_PANEL_NAME);
        assert_eq!(panel.read_with(cx, |panel, _| panel.terminal_state()), None);
        assert!(panel.read_with(cx, |panel, _| panel.parent_tab()).is_none());
    }
}
