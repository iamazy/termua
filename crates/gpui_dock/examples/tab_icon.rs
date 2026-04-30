use std::sync::Arc;

use gpui::{
    App, Context, EventEmitter, FocusHandle, Focusable, IntoElement, Window, WindowOptions, div,
    prelude::*,
};
use gpui_component::{Icon, IconName, Root, h_flex};
use gpui_component_assets::Assets;
use gpui_dock::{DockArea, DockItem, Panel, PanelEvent, PanelView};

struct IconPanel {
    focus: FocusHandle,
    icon: IconName,
    label: gpui::SharedString,
}

impl IconPanel {
    fn new(icon: IconName, label: impl Into<gpui::SharedString>, cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
            icon,
            label: label.into(),
        }
    }
}

impl EventEmitter<PanelEvent> for IconPanel {}

impl Focusable for IconPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for IconPanel {
    fn panel_name(&self) -> &'static str {
        "gpui_dock.example.icon_panel"
    }

    // Option 1: leave tab_name as None so the Tab uses `title()` (which can include an icon).
    fn tab_name(&self, _cx: &App) -> Option<gpui::SharedString> {
        None
    }

    fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .gap_1()
            .items_center()
            .child(Icon::new(self.icon.clone()).size_3())
            .child(self.label.clone())
    }

    fn closable(&self, _cx: &App) -> bool {
        true
    }
}

impl gpui::Render for IconPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .track_focus(&self.focus_handle(cx))
            .p(gpui::px(10.))
            .child(format!("Icon panel: {}", self.label))
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
    gpui_platform::application()
        .with_assets(Assets)
        .run(|cx: &mut App| {
            gpui_component::init(cx);
            gpui_dock::init(cx);
            cx.activate(true);

            cx.open_window(WindowOptions::default(), |window, cx| {
                let dock_area = cx.new(|cx| DockArea::new("dock", Some(1), window, cx));
                let weak = dock_area.downgrade();

                let file: Arc<dyn PanelView> =
                    Arc::new(cx.new(|cx| IconPanel::new(IconName::File, "File.rs", cx)));
                let search: Arc<dyn PanelView> =
                    Arc::new(cx.new(|cx| IconPanel::new(IconName::Search, "Search", cx)));
                let settings: Arc<dyn PanelView> =
                    Arc::new(cx.new(|cx| IconPanel::new(IconName::Settings, "Settings", cx)));

                dock_area.update(cx, |area, cx| {
                    area.set_center(
                        DockItem::tabs(vec![file, search, settings], &weak, window, cx),
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
