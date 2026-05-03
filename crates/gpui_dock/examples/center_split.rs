use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use gpui::{
    App, Context, EventEmitter, FocusHandle, Focusable, InteractiveElement, IntoElement,
    MouseButton, Window, WindowOptions, div, prelude::*,
};
use gpui_component::{
    ActiveTheme, IconName, Root, Sizable,
    button::{Button, ButtonVariants},
    menu::{PopupMenu, PopupMenuItem},
};
use gpui_component_assets::Assets;
use gpui_dock::{DockArea, DockItem, Panel, PanelEvent, PanelView, TabPanel};
use smol::Timer;

static NEXT_TAB_ID: AtomicUsize = AtomicUsize::new(1);

struct DemoPanel {
    focus: FocusHandle,
    label: gpui::SharedString,
    tab_panel: Option<gpui::WeakEntity<TabPanel>>,
    toast: Option<String>,
    toast_epoch: usize,
}

impl DemoPanel {
    fn new(label: impl Into<gpui::SharedString>, cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
            label: label.into(),
            tab_panel: None,
            toast: None,
            toast_epoch: 0,
        }
    }

    fn show_toast(&mut self, title: impl Into<String>, cx: &mut Context<Self>) {
        let epoch = self.toast_epoch.wrapping_add(1);
        self.toast_epoch = epoch;
        self.toast = Some(title.into());
        cx.notify();

        // Auto-hide; newer toasts supersede older ones via epoch.
        cx.spawn(async move |this, cx| {
            Timer::after(Duration::from_secs(3)).await;
            let _ = this.update(cx, |this, cx| {
                if this.toast_epoch != epoch {
                    return;
                }
                this.toast = None;
                cx.notify();
            });
        })
        .detach();
    }

    fn add_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab_panel) = self
            .tab_panel
            .as_ref()
            .and_then(|tab_panel| tab_panel.upgrade())
        else {
            return;
        };

        let id = NEXT_TAB_ID.fetch_add(1, Ordering::Relaxed);
        cx.spawn_in(window, async move |_, cx| {
            _ = cx.update(|window, cx| {
                let panel: Arc<dyn PanelView> =
                    Arc::new(cx.new(|cx| DemoPanel::new(format!("Tab {id}"), cx)));
                _ = tab_panel.update(cx, |tab_panel, cx| {
                    tab_panel.add_panel(panel, window, cx);
                });
            });
        })
        .detach();
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

    fn title_suffix(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement> {
        Some(
            div().child(
                Button::new(("center-split-add-tab", cx.entity().entity_id()))
                    // Used by the example tests (noop in release builds).
                    .debug_selector(|| "center-split-add-tab".to_string())
                    .icon(IconName::Plus)
                    .xsmall()
                    .ghost()
                    .tab_stop(false)
                    .tooltip("New Tab")
                    .on_click(cx.listener(|this, _, window, cx| this.add_tab(window, cx))),
            ),
        )
    }

    fn closable(&self, _cx: &App) -> bool {
        true
    }

    fn on_added_to(
        &mut self,
        tab_panel: gpui::WeakEntity<TabPanel>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        self.tab_panel = Some(tab_panel);
    }

    fn tab_context_menu(
        &mut self,
        menu: PopupMenu,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        let panel = cx.entity();
        menu.item(
            PopupMenuItem::element(|_window, _cx| div().child("Show toast")).on_click(
                move |_e, _window, cx| {
                    panel.update(cx, |this, cx| {
                        this.show_toast("Hello from tab context menu", cx);
                    });
                },
            ),
        )
    }
}

impl gpui::Render for DemoPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut root = div()
            .size_full()
            .track_focus(&self.focus_handle(cx))
            .p(gpui::px(10.0))
            .child(format!("Demo panel: {}", self.label));

        if let Some(title) = self.toast.clone() {
            let t = cx.theme();
            root = root.child(
                div()
                    .id("demo-toast")
                    .absolute()
                    .right(gpui::px(12.0))
                    .bottom(gpui::px(12.0))
                    .max_w(gpui::px(420.0))
                    .rounded_md()
                    .border_1()
                    .border_color(t.border.opacity(0.9))
                    .bg(t.popover.opacity(0.98))
                    .text_color(t.popover_foreground)
                    .p(gpui::px(10.0))
                    .text_sm()
                    .child(title)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.toast = None;
                            cx.notify();
                        }),
                    ),
            );
        }

        root
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

                let left: Arc<dyn PanelView> =
                    Arc::new(cx.new(|cx| DemoPanel::new("Editor (Left)", cx)));
                let right: Arc<dyn PanelView> =
                    Arc::new(cx.new(|cx| DemoPanel::new("Outline (Right)", cx)));
                let bottom: Arc<dyn PanelView> =
                    Arc::new(cx.new(|cx| DemoPanel::new("Logs (Bottom)", cx)));

                dock_area.update(cx, |area, cx| {
                    let top = DockItem::h_split(
                        vec![
                            DockItem::tabs(vec![left], &weak, window, cx).size(gpui::px(820.)),
                            DockItem::tabs(vec![right], &weak, window, cx),
                        ],
                        &weak,
                        window,
                        cx,
                    );

                    let bottom =
                        DockItem::tabs(vec![bottom], &weak, window, cx).size(gpui::px(260.));

                    area.set_center(
                        DockItem::v_split(vec![top, bottom], &weak, window, cx),
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
