use std::sync::Arc;

use gpui::{
    App, Application, Context, EventEmitter, FocusHandle, Focusable, IntoElement, Window,
    WindowOptions, div, prelude::*,
};
use gpui_component::Root;
use gpui_component_assets::Assets;
use gpui_dock::{DockArea, DockItem, Panel, PanelEvent, PanelView};

struct DemoPanel {
    focus: FocusHandle,
    label: gpui::SharedString,
}

impl DemoPanel {
    fn new(label: impl Into<gpui::SharedString>, cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
            label: label.into(),
        }
    }
}

impl EventEmitter<PanelEvent> for DemoPanel {}

impl Focusable for DemoPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for DemoPanel {
    fn panel_name(&self) -> &'static str {
        "gpui_dock.example.demo_panel"
    }

    fn tab_name(&self, _cx: &App) -> Option<gpui::SharedString> {
        Some(self.label.clone())
    }

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().child(self.label.clone())
    }
}

impl gpui::Render for DemoPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .track_focus(&self.focus_handle(cx))
            .p(gpui::px(10.))
            .child(format!("Demo panel: {}", self.label))
    }
}

struct MainView {
    dock_area: gpui::Entity<DockArea>,
}

impl gpui::Render for MainView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.dock_area.clone())
    }
}

fn main() {
    Application::new().with_assets(Assets).run(|cx: &mut App| {
        gpui_component::init(cx);
        gpui_dock::init(cx);
        cx.activate(true);

        cx.open_window(WindowOptions::default(), |window, cx| {
            let dock_area = cx.new(|cx| DockArea::new("dock", Some(1), window, cx));
            let weak = dock_area.downgrade();

            let center: Arc<dyn PanelView> = Arc::new(cx.new(|cx| DemoPanel::new("Center", cx)));
            let left: Arc<dyn PanelView> = Arc::new(cx.new(|cx| DemoPanel::new("Left", cx)));
            let right_a: Arc<dyn PanelView> = Arc::new(cx.new(|cx| DemoPanel::new("Right A", cx)));
            let right_b: Arc<dyn PanelView> = Arc::new(cx.new(|cx| DemoPanel::new("Right B", cx)));
            let bottom: Arc<dyn PanelView> = Arc::new(cx.new(|cx| DemoPanel::new("Bottom", cx)));

            dock_area.update(cx, |area, cx| {
                area.set_center(DockItem::tabs(vec![center], &weak, window, cx), window, cx);

                area.set_left_dock(
                    DockItem::tabs(vec![left], &weak, window, cx),
                    Some(gpui::px(280.)),
                    true,
                    window,
                    cx,
                );

                area.set_right_dock(
                    DockItem::tabs(vec![right_a, right_b], &weak, window, cx),
                    Some(gpui::px(320.)),
                    true,
                    window,
                    cx,
                );

                area.set_bottom_dock(
                    DockItem::tabs(vec![bottom], &weak, window, cx),
                    Some(gpui::px(240.)),
                    true,
                    window,
                    cx,
                );

                area.update_toggle_button_tab_panels(window, cx);
            });

            let view = cx.new(|_cx| MainView { dock_area });
            cx.new(|cx| Root::new(view, window, cx))
        })
        .unwrap();
    });
}
