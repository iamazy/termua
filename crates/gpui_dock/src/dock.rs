//! Dock is a fixed container that places at left, bottom, right of the Windows.

use std::{ops::Deref, sync::Arc};

use gpui::{
    App, AppContext, Axis, Context, Element, Empty, Entity, IntoElement, MouseMoveEvent,
    MouseUpEvent, ParentElement, Pixels, Point, Render, Style, StyleRefinement, Styled, WeakEntity,
    Window, div, prelude::FluentBuilder, px,
};
use gpui_component::StyledExt;
use serde::{Deserialize, Serialize};

use super::{DockArea, DockItem, Panel, PanelView, TabPanel};
use crate::resizable::{PANEL_MIN_SIZE, resize_handle};

#[derive(Clone)]
struct ResizePanel;

impl Render for ResizePanel {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum DockPlacement {
    #[serde(rename = "center")]
    Center,
    #[serde(rename = "left")]
    Left,
    #[serde(rename = "bottom")]
    Bottom,
    #[serde(rename = "right")]
    Right,
}

impl DockPlacement {
    fn axis(&self) -> Axis {
        match self {
            Self::Left | Self::Right => Axis::Horizontal,
            Self::Bottom => Axis::Vertical,
            Self::Center => unreachable!(),
        }
    }

    pub fn is_left(&self) -> bool {
        matches!(self, Self::Left)
    }

    pub fn is_bottom(&self) -> bool {
        matches!(self, Self::Bottom)
    }

    pub fn is_right(&self) -> bool {
        matches!(self, Self::Right)
    }
}

/// The Dock is a fixed container that places at left, bottom, right of the Windows.
///
/// This is unlike Panel, it can't be move or add any other panel.
pub struct Dock {
    pub(super) placement: DockPlacement,
    dock_area: WeakEntity<DockArea>,
    pub(crate) panel: DockItem,
    /// The size is means the width or height of the Dock, if the placement is left or right, the
    /// size is width, otherwise the size is height.
    pub(super) size: Pixels,
    /// The minimum size of the dock (width/height depending on placement).
    pub(super) min_size: Pixels,
    /// The maximum size of the dock (width/height depending on placement).
    pub(super) max_size: Option<Pixels>,
    pub(super) open: bool,
    /// Whether the Dock is collapsible, default: true
    pub(super) collapsible: bool,

    // Runtime state
    /// Whether the Dock is resizing
    resizing: bool,
}

impl Dock {
    pub(crate) fn new(
        dock_area: WeakEntity<DockArea>,
        placement: DockPlacement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let panel = cx.new(|cx| {
            let mut tab = TabPanel::new(None, dock_area.clone(), window, cx);
            tab.closable = false;
            tab
        });

        let panel = DockItem::Tabs {
            size: None,
            active_ix: 0,
            view: panel,
        };

        Self::subscribe_panel_events(dock_area.clone(), &panel, window, cx);

        Self {
            placement,
            dock_area,
            panel,
            open: true,
            collapsible: true,
            size: px(200.0),
            min_size: PANEL_MIN_SIZE,
            max_size: None,
            resizing: false,
        }
    }

    pub fn left(
        dock_area: WeakEntity<DockArea>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new(dock_area, DockPlacement::Left, window, cx)
    }

    pub fn bottom(
        dock_area: WeakEntity<DockArea>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new(dock_area, DockPlacement::Bottom, window, cx)
    }

    pub fn right(
        dock_area: WeakEntity<DockArea>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new(dock_area, DockPlacement::Right, window, cx)
    }

    /// Update the Dock to be collapsible or not.
    ///
    /// And if the Dock is not collapsible, it will be open.
    pub fn set_collapsible(&mut self, collapsible: bool, _: &mut Window, cx: &mut Context<Self>) {
        self.collapsible = collapsible;
        if !collapsible {
            self.open = true
        }
        cx.notify();
    }

    pub(super) fn from_state(
        dock_area: WeakEntity<DockArea>,
        placement: DockPlacement,
        size: Pixels,
        panel: DockItem,
        open: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::subscribe_panel_events(dock_area.clone(), &panel, window, cx);

        if !open {
            match panel.clone() {
                DockItem::Tabs { view, .. } => {
                    view.update(cx, |panel, cx| {
                        panel.set_collapsed(true, window, cx);
                    });
                }
                DockItem::Split { items, .. } => {
                    for item in items {
                        item.set_collapsed(true, window, cx);
                    }
                }
                _ => {}
            }
        }

        Self {
            placement,
            dock_area,
            panel,
            open,
            size,
            min_size: PANEL_MIN_SIZE,
            max_size: None,
            collapsible: true,
            resizing: false,
        }
    }

    fn subscribe_panel_events(
        dock_area: WeakEntity<DockArea>,
        panel: &DockItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match panel {
            DockItem::Tabs { view, .. } => {
                window.defer(cx, {
                    let view = view.clone();
                    move |window, cx| {
                        _ = dock_area.update(cx, |this, cx| {
                            this.subscribe_panel(&view, window, cx);
                        });
                    }
                });
            }
            DockItem::Split { items, view, .. } => {
                for item in items {
                    Self::subscribe_panel_events(dock_area.clone(), item, window, cx);
                }
                window.defer(cx, {
                    let view = view.clone();
                    move |window, cx| {
                        _ = dock_area.update(cx, |this, cx| {
                            this.subscribe_panel(&view, window, cx);
                        });
                    }
                });
            }
            DockItem::Tiles { view, .. } => {
                window.defer(cx, {
                    let view = view.clone();
                    move |window, cx| {
                        _ = dock_area.update(cx, |this, cx| {
                            this.subscribe_panel(&view, window, cx);
                        });
                    }
                });
            }
            DockItem::Panel { .. } => {
                // Not supported
            }
        }
    }

    pub fn set_panel(&mut self, panel: DockItem, _: &mut Window, cx: &mut Context<Self>) {
        self.panel = panel;
        cx.notify();
    }

    pub fn panel(&self) -> &DockItem {
        &self.panel
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn toggle_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_open(!self.open, window, cx);
    }

    /// Returns the size of the Dock, the size is means the width or height of
    /// the Dock, if the placement is left or right, the size is width,
    /// otherwise the size is height.
    pub fn size(&self) -> Pixels {
        self.size
    }

    pub fn set_min_size(&mut self, min_size: Pixels, _: &mut Window, cx: &mut Context<Self>) {
        self.min_size = min_size.max(PANEL_MIN_SIZE);
        if self.size < self.min_size {
            self.size = self.min_size;
        }
        cx.notify();
    }

    pub fn set_max_size(&mut self, max_size: Pixels, _: &mut Window, cx: &mut Context<Self>) {
        self.max_size = Some(max_size.max(self.min_size));
        if self.size > max_size {
            self.size = max_size;
        }
        cx.notify();
    }

    /// Set the size of the Dock.
    pub fn set_size(&mut self, size: Pixels, _: &mut Window, cx: &mut Context<Self>) {
        let mut size = size.max(self.min_size);
        if let Some(max) = self.max_size {
            size = size.min(max);
        }
        self.size = size;
        cx.notify();
    }

    /// Set the open state of the Dock.
    pub fn set_open(&mut self, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.open = open;
        let item = self.panel.clone();
        cx.defer_in(window, move |_, window, cx| {
            item.set_collapsed(!open, window, cx);
        });
        cx.notify();
    }

    /// Add item to the Dock.
    pub fn add_panel(
        &mut self,
        panel: Arc<dyn PanelView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.panel
            .add_panel(panel, &self.dock_area, None, window, cx);
        cx.notify();
    }

    /// Remove item from the Dock.
    pub fn remove_panel(
        &mut self,
        panel: Arc<dyn PanelView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.panel.remove_panel(panel, window, cx);
        cx.notify();
    }

    fn render_resize_handle(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let axis = self.placement.axis();
        let view = cx.entity();

        resize_handle("resize-handle", axis)
            .placement(self.placement)
            .on_drag(ResizePanel {}, move |info, _, _, cx| {
                cx.stop_propagation();
                view.update(cx, |view, _| {
                    view.resizing = true;
                });
                cx.new(|_| info.deref().clone())
            })
    }
    fn resize(
        &mut self,
        mouse_position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.resizing {
            return;
        }

        if !self.open {
            self.set_open(true, window, cx);
        }

        let dock_area = self
            .dock_area
            .upgrade()
            .expect("DockArea is missing")
            .read(cx);
        let area_bounds = dock_area.bounds;
        let mut left_dock_size = px(0.0);
        let mut right_dock_size = px(0.0);

        // Get the size of the left dock if it's open and not the current dock
        if let Some(left_dock) = &dock_area.left_dock {
            if left_dock.entity_id() != cx.entity().entity_id() {
                let left_dock_read = left_dock.read(cx);
                if left_dock_read.is_open() {
                    left_dock_size = left_dock_read.size;
                }
            }
        }

        // Get the size of the right dock if it's open and not the current dock
        if let Some(right_dock) = &dock_area.right_dock {
            if right_dock.entity_id() != cx.entity().entity_id() {
                let right_dock_read = right_dock.read(cx);
                if right_dock_read.is_open() {
                    right_dock_size = right_dock_read.size;
                }
            }
        }

        let size = match self.placement {
            DockPlacement::Left => mouse_position.x - area_bounds.left(),
            DockPlacement::Right => area_bounds.right() - mouse_position.x,
            DockPlacement::Bottom => area_bounds.bottom() - mouse_position.y,
            DockPlacement::Center => unreachable!(),
        };
        let min_size = self.min_size;
        let max_size = match self.placement {
            DockPlacement::Left => {
                (area_bounds.size.width - PANEL_MIN_SIZE - right_dock_size).max(min_size)
            }
            DockPlacement::Right => {
                (area_bounds.size.width - PANEL_MIN_SIZE - left_dock_size).max(min_size)
            }
            DockPlacement::Bottom => (area_bounds.size.height - PANEL_MIN_SIZE).max(min_size),
            DockPlacement::Center => unreachable!(),
        };
        let max_size = self
            .max_size
            .map_or(max_size, |self_max| max_size.min(self_max));
        self.size = size.clamp(min_size, max_size);

        cx.notify();
    }

    fn done_resizing(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.resizing = false;
    }
}

impl Render for Dock {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        // If the dock has no visible tabs, hide it completely (including resize handle / split
        // line). This keeps the layout clean after closing the last tab in examples like
        // four_docks.
        if let DockItem::Tabs { view, .. } = &self.panel {
            if !view.read(cx).visible(cx) {
                return div();
            }
        }

        if !self.open && !self.placement.is_bottom() {
            return div();
        }

        let cache_style = StyleRefinement::default().absolute().size_full();
        let has_tabs = matches!(&self.panel, DockItem::Tabs { .. })
            && match &self.panel {
                DockItem::Tabs { view, .. } => !view.read(cx).panels.is_empty(),
                _ => false,
            };

        div()
            .relative()
            .overflow_hidden()
            .map(|this| match self.placement {
                DockPlacement::Left | DockPlacement::Right => this.h_flex().h_full().w(self.size),
                DockPlacement::Bottom => this.w_full().h(self.size),
                DockPlacement::Center => unreachable!(),
            })
            // Bottom dock collapses to a "title bar" height when closed, but if it has no tabs,
            // hide it entirely (no toggle buttons to show).
            .when(!self.open && self.placement.is_bottom(), |this| {
                if has_tabs {
                    this.h(px(29.))
                } else {
                    this.h(px(0.))
                }
            })
            .map(|this| match &self.panel {
                DockItem::Split { view, .. } => this.child(view.clone()),
                DockItem::Tabs { view, .. } => this.child(view.clone()),
                DockItem::Panel { view, .. } => this.child(view.clone().view().cached(cache_style)),
                // Not support to render Tiles and Tile into Dock
                DockItem::Tiles { .. } => this,
            })
            .child(self.render_resize_handle(window, cx))
            .child(DockElement { view: cx.entity() })
    }
}

struct DockElement {
    view: Entity<Dock>,
}

impl IntoElement for DockElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for DockElement {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<gpui::ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut gpui::Window,
        cx: &mut App,
    ) -> (gpui::LayoutId, Self::RequestLayoutState) {
        (window.request_layout(Style::default(), None, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _: gpui::Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        _window: &mut gpui::Window,
        _cx: &mut App,
    ) -> Self::PrepaintState {
        ()
    }

    fn paint(
        &mut self,
        _: Option<&gpui::GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _: gpui::Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        _: &mut Self::PrepaintState,
        window: &mut gpui::Window,
        cx: &mut App,
    ) {
        window.on_mouse_event({
            let view = self.view.clone();
            let resizing = view.read(cx).resizing;
            move |e: &MouseMoveEvent, phase, window, cx| {
                if !resizing {
                    return;
                }
                if !phase.bubble() {
                    return;
                }

                view.update(cx, |view, cx| view.resize(e.position, window, cx))
            }
        });

        // When any mouse up, stop dragging
        window.on_mouse_event({
            let view = self.view.clone();
            move |_: &MouseUpEvent, phase, window, cx| {
                if phase.bubble() {
                    view.update(cx, |view, cx| view.done_resizing(window, cx));
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use gpui::{AvailableSpace, Bounds, point, px, size};

    use super::*;

    fn init(cx: &mut gpui::TestAppContext) {
        cx.update(|app| {
            gpui_component::init(app);
            crate::init(app);
        });
    }

    #[gpui::test]
    fn resizing_collapsed_bottom_dock_reopens_it(cx: &mut gpui::TestAppContext) {
        init(cx);

        let window_cx = cx.add_empty_window();
        window_cx.draw(
            point(px(0.), px(0.)),
            size(
                AvailableSpace::Definite(px(800.)),
                AvailableSpace::Definite(px(600.)),
            ),
            |window, cx| {
                let dock_area = cx.new(|cx| DockArea::new("dock", None, window, cx));
                dock_area.update(cx, |dock_area, _| {
                    dock_area.bounds = Bounds::new(point(px(0.), px(0.)), size(px(800.), px(600.)));
                });

                let dock = cx.new(|cx| Dock::bottom(dock_area.downgrade(), window, cx));
                dock.update(cx, |dock, cx| {
                    dock.set_open(false, window, cx);
                    dock.resizing = true;
                    dock.resize(point(px(0.), px(420.)), window, cx);
                    assert!(dock.is_open());
                    assert_eq!(dock.size(), px(180.));
                });

                div()
            },
        );
    }

    #[gpui::test]
    fn resizing_with_custom_min_size_does_not_panic_when_area_is_too_small(
        cx: &mut gpui::TestAppContext,
    ) {
        init(cx);

        let window_cx = cx.add_empty_window();
        window_cx.draw(
            point(px(0.), px(0.)),
            size(
                AvailableSpace::Definite(px(150.)),
                AvailableSpace::Definite(px(400.)),
            ),
            |window, cx| {
                let dock_area = cx.new(|cx| DockArea::new("dock", None, window, cx));
                dock_area.update(cx, |dock_area, _| {
                    dock_area.bounds = Bounds::new(point(px(0.), px(0.)), size(px(150.), px(400.)));
                });

                let dock = cx.new(|cx| Dock::left(dock_area.downgrade(), window, cx));
                dock.update(cx, |dock, cx| {
                    dock.set_min_size(px(220.), window, cx);
                    dock.resizing = true;
                    dock.resize(point(px(80.), px(0.)), window, cx);
                    assert_eq!(dock.size(), px(220.));
                });

                div()
            },
        );
    }
}
