use gpui::{
    App, Context, FocusHandle, Focusable, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, Styled, WeakEntity, Window, div,
};
use gpui_common::TermuaIcon;
use gpui_component::{ActiveTheme as _, v_flex};
use gpui_dock::{Panel, PanelEvent, PanelInfo, PanelState, TabPanel};

use super::TerminalPanelState;

pub(crate) struct SshErrorPanel {
    id: usize,
    tab_label: SharedString,
    tab_tooltip: Option<SharedString>,
    message: SharedString,
    terminal_state: Option<TerminalPanelState>,
    parent_tab: Option<WeakEntity<TabPanel>>,
    focus_handle: FocusHandle,
}

impl SshErrorPanel {
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
            parent_tab: None,
            focus_handle: cx.focus_handle(),
        }
    }

    pub(crate) fn terminal_state(&self) -> Option<TerminalPanelState> {
        self.terminal_state.clone()
    }

    pub(crate) fn parent_tab(&self) -> Option<WeakEntity<TabPanel>> {
        self.parent_tab.clone()
    }

    pub(crate) fn set_message(&mut self, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.message = message.into();
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
        "SshErrorPanel"
    }

    fn tab_icon(&self, _cx: &App) -> Option<gpui_dock::TabIcon> {
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
            panel_name: "TerminalPanel".to_string(),
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
