use gpui::{
    App, Context, FocusHandle, Focusable, InteractiveElement, IntoElement, ParentElement, Render,
    SharedString, Styled, Window, div,
};
use gpui_common::TermuaIcon;
use gpui_component::{ActiveTheme as _, v_flex};
use gpui_dock::{Panel, PanelEvent};

pub(crate) struct SshErrorPanel {
    id: usize,
    tab_label: SharedString,
    tab_tooltip: Option<SharedString>,
    message: SharedString,
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
            focus_handle: cx.focus_handle(),
        }
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
