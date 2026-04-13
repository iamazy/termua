use gpui::{
    App, AppContext, Context, FocusHandle, Focusable, InteractiveElement as _, IntoElement as _,
    ParentElement as _, Render, Styled as _, Subscription, Window, div,
};
use gpui_component::{ActiveTheme, v_flex};
use gpui_dock::{Panel, PanelControl, PanelEvent};

use crate::{
    globals::ensure_ctx_global,
    panel::{assistant_panel::AssistantPanelView, message_panel::MessageCenterView},
    right_sidebar::{RightSidebarState, RightSidebarTab},
};

pub struct RightSidebarView {
    focus_handle: FocusHandle,
    notifications: gpui::Entity<MessageCenterView>,
    assistant: gpui::Entity<AssistantPanelView>,
    _subscriptions: Vec<Subscription>,
}

impl gpui::EventEmitter<PanelEvent> for RightSidebarView {}

impl Focusable for RightSidebarView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl RightSidebarView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        ensure_ctx_global::<RightSidebarState, _>(cx);

        let notifications = cx.new(|cx| MessageCenterView::new(window, cx));
        let assistant = cx.new(|cx| AssistantPanelView::new(window, cx));

        let subs = vec![
            cx.observe_global::<RightSidebarState>(|_, cx| cx.notify()),
            cx.observe_window_activation(window, |_, _, cx| cx.notify()),
        ];

        Self {
            focus_handle: cx.focus_handle(),
            notifications,
            assistant,
            _subscriptions: subs,
        }
    }

    // Intentionally no local tab bar: switching happens via the app-level toggle actions.
}

impl Panel for RightSidebarView {
    fn panel_name(&self) -> &'static str {
        "termua.right_sidebar"
    }

    fn tab_name(&self, _cx: &App) -> Option<gpui::SharedString> {
        Some("Sidebar".into())
    }

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl gpui::IntoElement {
        "Sidebar"
    }

    fn closable(&self, _cx: &App) -> bool {
        false
    }

    fn zoomable(&self, _cx: &App) -> Option<PanelControl> {
        None
    }

    fn inner_padding(&self, _cx: &App) -> bool {
        false
    }
}

impl Render for RightSidebarView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        let tab = cx.global::<RightSidebarState>().active_tab;

        v_flex()
            .id("termua-right-sidebar")
            .debug_selector(|| "termua-right-sidebar".to_string())
            .size_full()
            .min_h_0()
            .items_stretch()
            .bg(cx.theme().background)
            .child(match tab {
                RightSidebarTab::Notifications => div()
                    .flex_1()
                    .min_h_0()
                    .child(self.notifications.clone())
                    .into_any_element(),
                RightSidebarTab::Assistant => div()
                    .flex_1()
                    .min_h_0()
                    .child(self.assistant.clone())
                    .into_any_element(),
            })
    }
}

#[cfg(test)]
mod tests {
    use gpui::{AvailableSpace, point, px, size};

    use super::*;

    fn init_test_app(app: &mut gpui::App) {
        gpui_component::init(app);
        gpui_term::init(app);
        crate::settings::set_language(crate::settings::Language::English, app);
        crate::assistant::ensure_app_globals(app);
        app.set_global(crate::settings::AssistantSettings {
            enabled: false,
            ..Default::default()
        });
    }

    #[gpui::test]
    fn right_sidebar_does_not_render_outer_tab_title_or_tab_bar(cx: &mut gpui::TestAppContext) {
        cx.update(|app| {
            init_test_app(app);
            app.set_global(crate::right_sidebar::RightSidebarState {
                visible: true,
                width: px(360.),
                active_tab: crate::right_sidebar::RightSidebarTab::Assistant,
            });
        });

        let (root, window_cx) = cx.add_window_view(|window, cx| {
            let sidebar = cx.new(|cx| RightSidebarView::new(window, cx));
            gpui_component::Root::new(sidebar, window, cx)
        });

        window_cx.draw(
            point(px(0.), px(0.)),
            size(
                AvailableSpace::Definite(px(520.)),
                AvailableSpace::Definite(px(240.)),
            ),
            move |_, _| div().size_full().child(root),
        );
        window_cx.run_until_parked();

        assert!(
            window_cx
                .debug_bounds("termua-right-sidebar-active-tab-title")
                .is_none(),
            "expected right sidebar not to render an outer tab title"
        );
        assert!(
            window_cx
                .debug_bounds("termua-right-sidebar-tab-notifications")
                .is_none(),
            "expected right sidebar not to render the full tab bar"
        );
        assert!(
            window_cx
                .debug_bounds("termua-right-sidebar-tab-assistant")
                .is_none(),
            "expected right sidebar not to render the full tab bar"
        );
    }
}
