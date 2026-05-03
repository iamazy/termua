use std::sync::Arc;

use gpui::{
    App, AppContext, Context, DismissEvent, Div, DragMoveEvent, Empty, Entity, EventEmitter,
    FocusHandle, Focusable, InteractiveElement, IntoElement, ParentElement, Pixels, Render,
    ScrollHandle, SharedString, StatefulInteractiveElement, StyleRefinement, Styled, StyledImage,
    Subscription, WeakEntity, Window, div, img, prelude::FluentBuilder, relative,
};
use gpui_component::{
    ActiveTheme, AxisExt, Disableable, Icon, IconName, Placement, Selectable, Sizable,
    button::{Button, ButtonVariants},
    h_flex,
    menu::{ContextMenuExt, PopupMenu, PopupMenuItem},
    tab::{Tab, TabBar},
    tooltip::Tooltip,
    v_flex,
};
use rust_i18n::t;

use super::{
    ClosePanel, DockArea, DockPlacement, Panel, PanelControl, PanelEvent, PanelInfo, PanelState,
    PanelView, StackPanel, TabIcon, ToggleZoom,
};

#[derive(Clone)]
struct TabState {
    zoomable: Option<PanelControl>,
    draggable: bool,
    droppable: bool,
    active_panel: Option<Arc<dyn PanelView>>,
}

#[derive(Clone)]
pub(crate) struct DragPanel {
    pub(crate) panel: Arc<dyn PanelView>,
    pub(crate) tab_panel: Entity<TabPanel>,
}

impl DragPanel {
    pub(crate) fn new(panel: Arc<dyn PanelView>, tab_panel: Entity<TabPanel>) -> Self {
        Self { panel, tab_panel }
    }
}

impl Render for DragPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("drag-panel")
            .cursor_grab()
            .py_1()
            .px_3()
            .overflow_hidden()
            .whitespace_nowrap()
            .border_1()
            .border_color(cx.theme().border)
            .rounded(cx.theme().radius)
            .text_color(cx.theme().tab_foreground)
            .bg(cx.theme().tab_active)
            .opacity(0.75)
            .child(self.panel.title(window, cx))
    }
}

pub struct TabPanel {
    focus_handle: FocusHandle,
    dock_area: WeakEntity<DockArea>,
    /// The stock_panel can be None, if is None, that means the panels can't be split or move
    stack_panel: Option<WeakEntity<StackPanel>>,
    pub(crate) panels: Vec<Arc<dyn PanelView>>,
    pub(crate) active_ix: usize,
    /// If this is true, the Panel closable will follow the active panel's closable,
    /// otherwise this TabPanel will not able to close
    ///
    /// This is used for Dock to limit the last TabPanel not able to close, see
    /// [`super::Dock::new`].
    pub(crate) closable: bool,

    tab_bar_scroll_handle: ScrollHandle,
    tab_overflow: bool,
    scroll_active_tab_next_frame: bool,
    zoomed: bool,
    collapsed: bool,
    /// When drag move, will get the placement of the panel to be split
    will_split_placement: Option<Placement>,
    /// Is TabPanel used in Tiles.
    in_tiles: bool,
    _subscriptions: Vec<Subscription>,
}

impl Panel for TabPanel {
    fn panel_name(&self) -> &'static str {
        "TabPanel"
    }

    fn title(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.active_panel(cx)
            .map(|panel| panel.title(window, cx))
            .unwrap_or("Empty Tab".into_any_element())
    }

    fn closable(&self, cx: &App) -> bool {
        if !self.closable {
            return false;
        }

        self.active_panel(cx)
            .map(|panel| panel.closable(cx))
            .unwrap_or(false)
    }

    fn zoomable(&self, cx: &App) -> Option<PanelControl> {
        self.active_panel(cx).and_then(|panel| panel.zoomable(cx))
    }

    fn visible(&self, cx: &App) -> bool {
        self.visible_panels(cx).next().is_some()
    }

    fn dropdown_menu(
        &mut self,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        if let Some(panel) = self.active_panel(cx) {
            panel.dropdown_menu(menu, window, cx)
        } else {
            menu
        }
    }

    fn toolbar_buttons(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Vec<Button>> {
        self.active_panel(cx)
            .and_then(|panel| panel.toolbar_buttons(window, cx))
    }

    fn dump(&self, cx: &App) -> PanelState {
        let mut state = PanelState::new(self);
        for panel in self.panels.iter() {
            state.add_child(panel.dump(cx));
            state.info = PanelInfo::tabs(self.active_ix);
        }
        state
    }

    fn inner_padding(&self, cx: &App) -> bool {
        self.active_panel(cx)
            .map_or(true, |panel| panel.inner_padding(cx))
    }
}

impl TabPanel {
    pub fn new(
        stack_panel: Option<WeakEntity<StackPanel>>,
        dock_area: WeakEntity<DockArea>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut subscriptions = Vec::new();
        subscriptions.push(cx.observe_window_bounds(window, |this, _window, cx| {
            // Scroll after the resized frame updates scroll handle bounds; see `render_title_bar`
            // where we apply the scroll request from a deferred callback.
            this.scroll_active_tab_next_frame = true;
            cx.notify();
        }));

        Self {
            focus_handle: cx.focus_handle(),
            dock_area,
            stack_panel,
            panels: Vec::new(),
            active_ix: 0,
            tab_bar_scroll_handle: ScrollHandle::new(),
            tab_overflow: false,
            scroll_active_tab_next_frame: false,
            will_split_placement: None,
            zoomed: false,
            collapsed: false,
            closable: true,
            in_tiles: false,
            _subscriptions: subscriptions,
        }
    }

    fn scroll_active_tab_into_view(&mut self, cx: &App) {
        let Some(active_panel) = self.active_panel(cx) else {
            return;
        };

        let visible_panels = self.visible_panels(cx).collect::<Vec<_>>();
        let Some(active_pos) = visible_panels.iter().position(|p| p == &active_panel) else {
            return;
        };

        self.tab_bar_scroll_handle.scroll_to_item(active_pos);
    }

    /// Mark the TabPanel as being used in Tiles.
    pub(super) fn set_in_tiles(&mut self, in_tiles: bool) {
        self.in_tiles = in_tiles;
    }

    pub(super) fn set_parent(&mut self, view: WeakEntity<StackPanel>) {
        self.stack_panel = Some(view);
    }

    /// Return current active_panel View
    pub fn active_panel(&self, cx: &App) -> Option<Arc<dyn PanelView>> {
        let panel = self.panels.get(self.active_ix);

        if let Some(panel) = panel {
            if panel.visible(cx) {
                Some(panel.clone())
            } else {
                // Return the first visible panel
                self.visible_panels(cx).next()
            }
        } else {
            None
        }
    }

    pub fn active_ix(&self) -> usize {
        self.active_ix
    }

    pub fn is_collapsed(&self) -> bool {
        self.collapsed
    }

    fn set_active_ix(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix == self.active_ix {
            return;
        }

        let last_active_ix = self.active_ix;

        self.active_ix = ix;
        self.tab_bar_scroll_handle.scroll_to_item(ix);
        self.focus_active_panel(window, cx);

        // Sync the active state to all panels
        cx.spawn_in(window, async move |view, cx| {
            _ = cx.update(|window, cx| {
                _ = view.update(cx, |view, cx| {
                    if let Some(last_active) = view.panels.get(last_active_ix) {
                        last_active.set_active(false, window, cx);
                    }
                    if let Some(active) = view.panels.get(view.active_ix) {
                        active.set_active(true, window, cx);
                    }
                });
            });
        })
        .detach();

        cx.emit(PanelEvent::LayoutChanged);
        cx.notify();
    }

    fn activate_adjacent_tab(
        &mut self,
        direction: isize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(active_panel) = self.active_panel(cx) else {
            return;
        };

        // Navigate within *visible* tabs so the buttons reflect the actual tab bar order.
        let visible_ixs: Vec<usize> = self
            .panels
            .iter()
            .enumerate()
            .filter(|(_, p)| p.visible(cx))
            .map(|(ix, _)| ix)
            .collect();
        if visible_ixs.len() < 2 {
            return;
        }

        let Some(pos) = visible_ixs
            .iter()
            .position(|&ix| self.panels.get(ix).is_some_and(|p| p == &active_panel))
        else {
            return;
        };

        let new_pos = match direction {
            -1 => pos.checked_sub(1),
            1 => (pos + 1 < visible_ixs.len()).then_some(pos + 1),
            _ => None,
        };
        let Some(new_pos) = new_pos else {
            return;
        };

        let to_ix = visible_ixs[new_pos];
        self.set_active_ix(to_ix, window, cx);
    }

    /// Add a panel to the end of the tabs
    pub fn add_panel(
        &mut self,
        panel: Arc<dyn PanelView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.add_panel_with_active(panel, true, window, cx);
    }

    fn add_panel_with_active(
        &mut self,
        panel: Arc<dyn PanelView>,
        active: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        assert_ne!(
            panel.panel_name(cx),
            "StackPanel",
            "can not allows add `StackPanel` to `TabPanel`"
        );

        if self
            .panels
            .iter()
            .any(|p| p.view().entity_id() == panel.view().entity_id())
        {
            return;
        }

        panel.on_added_to(cx.entity().downgrade(), window, cx);
        self.panels.push(panel);
        // set the active panel to the new panel
        if active {
            self.set_active_ix(self.panels.len() - 1, window, cx);
        }
        cx.emit(PanelEvent::LayoutChanged);
        cx.notify();
    }

    /// Add panel to try to split
    pub fn add_panel_at(
        &mut self,
        panel: Arc<dyn PanelView>,
        placement: Placement,
        size: Option<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.spawn_in(window, async move |view, cx| {
            cx.update(|window, cx| {
                view.update(cx, |view, cx| {
                    view.will_split_placement = Some(placement);
                    view.split_panel(panel, placement, size, window, cx)
                })
                .ok()
            })
            .ok()
        })
        .detach();
        cx.emit(PanelEvent::LayoutChanged);
        cx.notify();
    }

    fn insert_panel_at(
        &mut self,
        panel: Arc<dyn PanelView>,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .panels
            .iter()
            .any(|p| p.view().entity_id() == panel.view().entity_id())
        {
            return;
        }

        panel.on_added_to(cx.entity().downgrade(), window, cx);
        self.panels.insert(ix, panel);
        self.set_active_ix(ix, window, cx);
        cx.emit(PanelEvent::LayoutChanged);
        cx.notify();
    }

    /// Remove a panel from the tab panel
    pub fn remove_panel(
        &mut self,
        panel: Arc<dyn PanelView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.detach_panel(panel, window, cx);
        self.remove_self_if_empty(window, cx);
        cx.emit(PanelEvent::ZoomOut);
        cx.emit(PanelEvent::LayoutChanged);
        cx.notify();
    }

    fn detach_panel(
        &mut self,
        panel: Arc<dyn PanelView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        panel.on_removed(window, cx);
        let panel_view = panel.view();
        self.panels.retain(|p| p.view() != panel_view);
        if self.active_ix >= self.panels.len() {
            self.set_active_ix(self.panels.len().saturating_sub(1), window, cx)
        }
        cx.notify();
    }

    /// Check to remove self from the parent StackPanel, if there is no panel left
    fn remove_self_if_empty(&self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.panels.is_empty() {
            return;
        }

        let tab_view = cx.entity();
        if let Some(stack_panel) = self.stack_panel.as_ref() {
            _ = stack_panel.update(cx, |view, cx| {
                view.remove_panel(Arc::new(tab_view), window, cx);
            });
        }

        // In tiles mode, an empty TabPanel should be removed from the Tiles container too;
        // otherwise the tile's bounds remain and you get an empty background block.
        if self.in_tiles {
            let tab_panel = Arc::new(cx.entity());
            window.defer(cx, {
                let dock_area = self.dock_area.clone();
                move |window, cx| {
                    _ = dock_area.update(cx, |this, cx| {
                        this.remove_panel_from_all_docks(tab_panel, window, cx);
                    });
                }
            });
        }
    }

    pub(super) fn set_collapsed(
        &mut self,
        collapsed: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.collapsed = collapsed;
        if let Some(panel) = self.panels.get(self.active_ix) {
            panel.set_active(!collapsed, window, cx);
        }
        cx.notify();
    }

    fn is_locked(&self, cx: &App) -> bool {
        let Some(dock_area) = self.dock_area.upgrade() else {
            return true;
        };

        if dock_area.read(cx).is_locked() {
            return true;
        }

        if self.zoomed {
            return true;
        }

        self.stack_panel.is_none()
    }

    /// Return true if self or parent only have last panel.
    fn is_last_panel(&self, cx: &App) -> bool {
        if let Some(parent) = &self.stack_panel {
            if let Some(stack_panel) = parent.upgrade() {
                if !stack_panel.read(cx).is_last_panel(cx) {
                    return false;
                }
            }
        }

        self.panels.len() <= 1
    }

    /// Return all visible panels
    fn visible_panels<'a>(&'a self, cx: &'a App) -> impl Iterator<Item = Arc<dyn PanelView>> + 'a {
        self.panels.iter().filter_map(|panel| {
            if panel.visible(cx) {
                Some(panel.clone())
            } else {
                None
            }
        })
    }

    /// Return true if the tab panel is draggable.
    ///
    /// E.g. if the parent and self only have one panel, it is not draggable.
    fn draggable(&self, cx: &App) -> bool {
        !self.is_locked(cx) && !self.is_last_panel(cx)
    }

    /// Return true if the tab panel is droppable.
    ///
    /// E.g. if the tab panel is locked, it is not droppable.
    fn droppable(&self, cx: &App) -> bool {
        !self.is_locked(cx)
    }

    fn can_close_panel(&self, panel: &Arc<dyn PanelView>, cx: &App) -> bool {
        self.closable && panel.closable(cx)
    }

    fn close_other_panels(
        &mut self,
        keep_panel_id: gpui::EntityId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let to_close: Vec<Arc<dyn PanelView>> = self
            .panels
            .iter()
            .filter(|p| p.panel_id(cx) != keep_panel_id && p.closable(cx))
            .cloned()
            .collect();

        for panel in to_close {
            self.close_panel(panel, window, cx);
        }
    }

    fn close_all_panels(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let to_close: Vec<Arc<dyn PanelView>> = self
            .panels
            .iter()
            .filter(|p| p.closable(cx))
            .cloned()
            .collect();

        for panel in to_close {
            self.close_panel(panel, window, cx);
        }
    }

    fn close_panel(
        &mut self,
        panel: Arc<dyn PanelView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.can_close_panel(&panel, cx) {
            return;
        }

        panel.on_close(window, cx);
        self.remove_panel(panel, window, cx);
    }

    fn render_toolbar(
        &mut self,
        state: &TabState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        if self.collapsed {
            return div();
        }

        let zoomed = self.zoomed;
        // Show the zoom toggle icon in the toolbar whenever zoom is supported (even if the
        // Panel only opted into menu zoom controls previously).
        let zoom_supported = state.zoomable.is_some();

        h_flex()
            .gap_1()
            .occlude()
            .when_some(self.toolbar_buttons(window, cx), |this, buttons| {
                this.children(
                    buttons
                        .into_iter()
                        .map(|btn| btn.xsmall().ghost().tab_stop(false)),
                )
            })
            .map(|this| {
                let value = if zoomed {
                    Some(("zoom-out", IconName::Minimize, t!("Dock.Zoom Out")))
                } else if zoom_supported {
                    Some(("zoom-in", IconName::Maximize, t!("Dock.Zoom In")))
                } else {
                    None
                };

                if let Some((id, icon, tooltip)) = value {
                    this.child(
                        Button::new(id)
                            .icon(icon)
                            .xsmall()
                            .ghost()
                            .tab_stop(false)
                            .tooltip_with_action(tooltip, &ToggleZoom, None)
                            .when(zoomed, |this| this.selected(true))
                            .on_click(cx.listener(|view, _, window, cx| {
                                view.on_action_toggle_zoom(&ToggleZoom, window, cx)
                            })),
                    )
                } else {
                    this
                }
            })
    }

    fn render_dock_toggle_button(
        &self,
        placement: DockPlacement,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Button> {
        if self.zoomed {
            return None;
        }

        let dock_area = self.dock_area.upgrade()?.read(cx);
        if !dock_area.toggle_button_visible_for(placement) {
            return None;
        }
        if !dock_area.is_dock_collapsible(placement, cx) {
            return None;
        }

        let view_entity_id = cx.entity().entity_id();
        let toggle_button_panels = dock_area.toggle_button_panels;

        // Check if current TabPanel's entity_id matches the one stored in DockArea for this
        // placement
        if !match placement {
            DockPlacement::Left => {
                dock_area.left_dock.is_some() && toggle_button_panels.left == Some(view_entity_id)
            }
            DockPlacement::Right => {
                dock_area.right_dock.is_some() && toggle_button_panels.right == Some(view_entity_id)
            }
            DockPlacement::Bottom => {
                dock_area.bottom_dock.is_some()
                    && toggle_button_panels.bottom == Some(view_entity_id)
            }
            DockPlacement::Center => unreachable!(),
        } {
            return None;
        }

        let is_open = dock_area.is_dock_open(placement, cx);

        let icon = match placement {
            DockPlacement::Left => {
                if is_open {
                    IconName::PanelLeft
                } else {
                    IconName::PanelLeftOpen
                }
            }
            DockPlacement::Right => {
                if is_open {
                    IconName::PanelRight
                } else {
                    IconName::PanelRightOpen
                }
            }
            DockPlacement::Bottom => {
                if is_open {
                    IconName::PanelBottom
                } else {
                    IconName::PanelBottomOpen
                }
            }
            DockPlacement::Center => unreachable!(),
        };

        let debug_selector: &'static str = match placement {
            DockPlacement::Left => "gpui-dock-toggle-left",
            DockPlacement::Right => "gpui-dock-toggle-right",
            DockPlacement::Bottom => "gpui-dock-toggle-bottom",
            DockPlacement::Center => unreachable!(),
        };

        Some(
            Button::new(SharedString::from(format!("toggle-dock:{:?}", placement)))
                .icon(icon)
                .xsmall()
                .ghost()
                .tab_stop(false)
                .debug_selector(move || debug_selector.to_string())
                .tooltip(match is_open {
                    true => t!("Dock.Collapse"),
                    false => t!("Dock.Expand"),
                })
                .on_click(cx.listener({
                    let dock_area = self.dock_area.clone();
                    move |_, _, window, cx| {
                        _ = dock_area.update(cx, |dock_area, cx| {
                            dock_area.toggle_dock(placement, window, cx);
                        });
                    }
                })),
        )
    }

    fn render_title_bar(
        &mut self,
        state: &TabState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let view = cx.entity();

        let left_dock_button = self.render_dock_toggle_button(DockPlacement::Left, window, cx);
        let bottom_dock_button = self.render_dock_toggle_button(DockPlacement::Bottom, window, cx);
        let right_dock_button = self.render_dock_toggle_button(DockPlacement::Right, window, cx);
        let has_extend_dock_button = left_dock_button.is_some() || bottom_dock_button.is_some();

        let is_bottom_dock = bottom_dock_button.is_some();

        let visible_panels = self.visible_panels(cx).collect::<Vec<_>>();
        if visible_panels.is_empty() {
            // No tabs: hide the title bar entirely (including dock toggle buttons).
            return div().into_any_element();
        }

        // Update whether the tab bar overflows after layout has been computed (prepaint updates the
        // scroll handle). We keep this in the entity so we can show/hide the move buttons
        // when the window is resized.
        let view_for_defer = cx.entity();
        let scroll_handle = self.tab_bar_scroll_handle.clone();
        window.defer(cx, move |_, cx| {
            view_for_defer.update(cx, |this, cx| {
                let max_overflow_x = scroll_handle.max_offset().x;
                // Use a small hysteresis band to avoid oscillation when we're right at the
                // overflow boundary (which can happen while dragging due to transient layout).
                let overflow = if this.tab_overflow {
                    max_overflow_x > gpui::px(1.)
                } else {
                    max_overflow_x > gpui::px(8.)
                };
                if this.tab_overflow != overflow {
                    this.tab_overflow = overflow;
                    cx.notify();
                }

                if this.scroll_active_tab_next_frame {
                    this.scroll_active_tab_next_frame = false;
                    this.scroll_active_tab_into_view(cx);
                    cx.notify();
                }
            });
        });

        let show_move_buttons = self.tab_overflow && visible_panels.len() > 1;

        let view_entity_id = cx.entity().entity_id();
        let (can_move_left, can_move_right) = if show_move_buttons {
            let active_pos = state
                .active_panel
                .as_ref()
                .and_then(|active| visible_panels.iter().position(|p| p == active));
            (
                active_pos.is_some_and(|pos| pos > 0),
                active_pos.is_some_and(|pos| pos + 1 < visible_panels.len()),
            )
        } else {
            (false, false)
        };

        let move_buttons = show_move_buttons.then(|| {
            h_flex()
                .flex_shrink_0()
                .items_center()
                .gap_1()
                .child(
                    Button::new(("tab-move-left", view_entity_id))
                        .icon(IconName::ChevronLeft)
                        .xsmall()
                        .ghost()
                        .tab_stop(false)
                        .debug_selector(|| "gpui-dock-tab-move-left".to_string())
                        .disabled(!can_move_left)
                        .tooltip("Previous tab")
                        .on_click(cx.listener(|view, _, window, cx| {
                            view.activate_adjacent_tab(-1, window, cx)
                        })),
                )
                .child(
                    Button::new(("tab-move-right", view_entity_id))
                        .icon(IconName::ChevronRight)
                        .xsmall()
                        .ghost()
                        .tab_stop(false)
                        .debug_selector(|| "gpui-dock-tab-move-right".to_string())
                        .disabled(!can_move_right)
                        .tooltip("Next tab")
                        .on_click(cx.listener(|view, _, window, cx| {
                            view.activate_adjacent_tab(1, window, cx)
                        })),
                )
        });

        // Always render a TabBar, even when there is only a single visible tab.
        // This keeps styling consistent (tab bar background + active tab highlight) and ensures
        // controls (dock toggles / move buttons) appear uniformly across split tab groups.

        let tabs_count = self.panels.len();

        // Make the tab bar id unique per TabPanel; otherwise split tab groups would share
        // the same element id and fight over internal TabBar state (scroll/interaction).
        TabBar::new(("tab-bar", view_entity_id))
            .track_scroll(&self.tab_bar_scroll_handle)
            .when(show_move_buttons || has_extend_dock_button, |this| {
                this.prefix(
                    h_flex()
                        .items_center()
                        .top_0()
                        // Right -1 for avoid border overlap with the first tab
                        .right(-gpui::px(1.))
                        .border_r_1()
                        .border_b_1()
                        .h_full()
                        .border_color(cx.theme().border)
                        .bg(cx.theme().tab_bar)
                        .px_2()
                        .when_some(move_buttons, |this, btns| this.child(btns))
                        .when(has_extend_dock_button, |this| {
                            this.child(
                                h_flex()
                                    .flex_shrink_0()
                                    .ml_1()
                                    .gap_1()
                                    .children(left_dock_button)
                                    .children(bottom_dock_button),
                            )
                        }),
                )
            })
            .children(self.panels.iter().enumerate().filter_map(|(ix, panel)| {
                let mut active = state.active_panel.as_ref() == Some(panel);
                let droppable = self.collapsed;

                if !panel.visible(cx) {
                    return None;
                }

                // Always not show active tab style, if the panel is collapsed
                if self.collapsed {
                    active = false;
                }

                let panel_id = panel.panel_id(cx);
                let tab_closable = self.closable && panel.closable(cx);
                let panel_for_close = panel.clone();
                let tab_panel_for_close = view.clone();
                let panel_for_menu = panel.clone();
                let tab_panel_for_menu = view.clone();

                let tab_icon = panel.tab_icon(cx);
                let has_tab_tooltip = panel.tab_tooltip(window, cx).is_some();

                Some(
                    Tab::new()
                        .when(has_tab_tooltip, |this| {
                            let panel_for_tooltip = panel.clone();
                            this.tooltip(move |window, cx| {
                                let panel_for_tooltip = panel_for_tooltip.clone();
                                Tooltip::element(move |window, cx| {
                                    panel_for_tooltip
                                        .tab_tooltip(window, cx)
                                        .unwrap_or_else(|| Empty.into_any_element())
                                })
                                .build(window, cx)
                            })
                        })
                        .child(
                            h_flex()
                                .w_full()
                                .h_full()
                                .items_center()
                                .gap_1()
                                // Use group hover rather than stateful `.on_hover` so the close
                                // button continues to work when tabs are removed under the cursor.
                                .group("tab")
                                .when_some(tab_icon, |this, icon| {
                                    // NOTE: GPUI's `svg()` element is monochrome (alpha mask),
                                    // so multi-color SVGs must use `img()` to preserve colors.
                                    let icon_el = match icon {
                                        TabIcon::Monochrome { path, color } => Icon::default()
                                            .path(path)
                                            .text_color(
                                                color.unwrap_or_else(|| cx.theme().tab_foreground),
                                            )
                                            .into_any_element(),
                                        TabIcon::ColoredSvg { path } => img(path)
                                            // Match the tab header's text size (like Icon does by
                                            // default).
                                            .w(window
                                                .text_style()
                                                .font_size
                                                .to_pixels(window.rem_size()))
                                            .h(window
                                                .text_style()
                                                .font_size
                                                .to_pixels(window.rem_size()))
                                            .flex_shrink_0()
                                            .object_fit(gpui::ObjectFit::Contain)
                                            .into_any_element(),
                                    };

                                    this.child(
                                        div()
                                            // Used by tests (noop in release builds).
                                            .debug_selector(|| "dock-tab-icon".to_string())
                                            .child(icon_el),
                                    )
                                })
                                .child(if let Some(tab_name) = panel.tab_name(cx) {
                                    tab_name.into_any_element()
                                } else {
                                    panel.title(window, cx)
                                })
                                .child(
                                    div()
                                        .w_4()
                                        .flex_shrink_0()
                                        .items_center()
                                        .justify_end()
                                        .when(tab_closable, |this| {
                                            this.child(
                                                Button::new((
                                                    "tab-close",
                                                    panel_for_close.panel_id(cx),
                                                ))
                                                .icon(IconName::Close)
                                                .xsmall()
                                                .ghost()
                                                .tab_stop(false)
                                                .invisible()
                                                .group_hover("tab", |this| this.visible())
                                                .on_click(
                                                    move |_: &gpui::ClickEvent, window, cx| {
                                                        cx.stop_propagation();
                                                        window.prevent_default();

                                                        tab_panel_for_close.update(
                                                            cx,
                                                            |this, cx| {
                                                                this.close_panel(
                                                                    panel_for_close.clone(),
                                                                    window,
                                                                    cx,
                                                                )
                                                            },
                                                        );
                                                    },
                                                ),
                                            )
                                        }),
                                )
                                .context_menu(move |menu: PopupMenu, window: &mut Window, cx| {
                                    // Mirror common UX: right-clicking a tab makes it active.
                                    tab_panel_for_menu.update(cx, |this, cx| {
                                        this.set_active_ix(ix, window, cx);
                                    });

                                    let menu = menu.action_context(panel_for_menu.focus_handle(cx));

                                    let can_close = tab_panel_for_menu
                                        .read(cx)
                                        .can_close_panel(&panel_for_menu, cx);

                                    let tab_panel_for_close = tab_panel_for_menu.clone();
                                    let panel_for_close = panel_for_menu.clone();
                                    let menu =
                                        menu.item(
                                            PopupMenuItem::element(|_window, _cx| {
                                                div().child(t!("Dock.Close"))
                                            })
                                            .on_click(move |_e, window, cx| {
                                                if !can_close {
                                                    return;
                                                }
                                                tab_panel_for_close.update(cx, |this, cx| {
                                                    this.close_panel(
                                                        panel_for_close.clone(),
                                                        window,
                                                        cx,
                                                    )
                                                });
                                            }),
                                        );

                                    let tab_panel_for_close_others = tab_panel_for_menu.clone();
                                    let menu =
                                        menu.item(
                                            PopupMenuItem::element(|_window, _cx| {
                                                div().child(t!("Dock.Close Others"))
                                            })
                                            .on_click(move |_e, window, cx| {
                                                tab_panel_for_close_others.update(
                                                    cx,
                                                    |this, cx| {
                                                        this.close_other_panels(
                                                            panel_id, window, cx,
                                                        );
                                                    },
                                                );
                                            }),
                                        );

                                    let tab_panel_for_close_all = tab_panel_for_menu.clone();
                                    let menu =
                                        menu.item(
                                            PopupMenuItem::element(|_window, _cx| {
                                                div().child(t!("Dock.Close All"))
                                            })
                                            .on_click(move |_e, window, cx| {
                                                tab_panel_for_close_all.update(cx, |this, cx| {
                                                    this.close_all_panels(window, cx)
                                                });
                                            }),
                                        );

                                    // Allow the panel to customize the tab's context menu.
                                    let menu = menu.separator();
                                    panel_for_menu.tab_context_menu(menu, window, cx)
                                }),
                        )
                        .selected(active)
                        .on_click(cx.listener({
                            let is_collapsed = self.collapsed;
                            let dock_area = self.dock_area.clone();
                            move |view, _, window, cx| {
                                view.set_active_ix(ix, window, cx);

                                // Open dock if clicked on the collapsed bottom dock
                                if is_bottom_dock && is_collapsed {
                                    _ = dock_area.update(cx, |dock_area, cx| {
                                        dock_area.toggle_dock(DockPlacement::Bottom, window, cx);
                                    });
                                }
                            }
                        }))
                        .when(!droppable, |this| {
                            this.when(state.draggable, |this| {
                                this.on_drag(
                                    DragPanel::new(panel.clone(), view.clone()),
                                    |drag, _, _, cx| {
                                        cx.stop_propagation();
                                        cx.new(|_| drag.clone())
                                    },
                                )
                            })
                            .when(state.droppable, |this| {
                                this.drag_over::<DragPanel>(|this, _, _, cx| {
                                    this.rounded_l_none()
                                        .border_l_2()
                                        .border_r_0()
                                        .border_color(cx.theme().drag_border)
                                })
                                .on_drop(cx.listener(
                                    move |this, drag: &DragPanel, window, cx| {
                                        this.will_split_placement = None;
                                        this.on_drop(drag, Some(ix), true, window, cx)
                                    },
                                ))
                            })
                        }),
                )
            }))
            .last_empty_space(
                // empty space to allow move to last tab right
                div()
                    .id("tab-bar-empty-space")
                    .h_full()
                    .flex_grow()
                    .min_w_16()
                    .when(state.droppable, |this| {
                        this.drag_over::<DragPanel>(|this, _, _, cx| {
                            this.bg(cx.theme().drop_target)
                        })
                        .on_drop(cx.listener(
                            move |this, drag: &DragPanel, window, cx| {
                                this.will_split_placement = None;

                                let ix = if drag.tab_panel == view {
                                    tabs_count.checked_sub(1)
                                } else {
                                    None
                                };

                                this.on_drop(drag, ix, false, window, cx)
                            },
                        ))
                    }),
            )
            .when(!self.collapsed, |this| {
                this.suffix(
                    h_flex()
                        .items_center()
                        .top_0()
                        .right_0()
                        .border_l_1()
                        .border_b_1()
                        .h_full()
                        .border_color(cx.theme().border)
                        .bg(cx.theme().tab_bar)
                        .px_2()
                        .gap_1()
                        .children(
                            self.active_panel(cx)
                                .and_then(|panel| panel.title_suffix(window, cx)),
                        )
                        .child(self.render_toolbar(state, window, cx))
                        .when_some(right_dock_button, |this, btn| this.child(btn)),
                )
            })
            .into_any_element()
    }

    fn render_active_panel(
        &self,
        state: &TabState,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        if self.collapsed {
            return Empty {}.into_any_element();
        }

        let Some(active_panel) = state.active_panel.as_ref() else {
            return Empty {}.into_any_element();
        };

        let is_render_in_tabs = self.panels.len() > 1 && self.inner_padding(cx);

        v_flex()
            .id("active-panel")
            .group("")
            .flex_1()
            .when(is_render_in_tabs, |this| this.pt_2())
            .child(
                div()
                    .id("tab-content")
                    .overflow_y_scroll()
                    .overflow_x_hidden()
                    .flex_1()
                    .child(
                        active_panel
                            .view()
                            .cached(StyleRefinement::default().absolute().size_full()),
                    ),
            )
            .when(state.droppable, |this| {
                this.on_drag_move(cx.listener(Self::on_panel_drag_move))
                    .child(
                        div()
                            .invisible()
                            .absolute()
                            .bg(cx.theme().drop_target)
                            .map(|this| match self.will_split_placement {
                                Some(placement) => {
                                    let size = relative(0.5);
                                    match placement {
                                        Placement::Left => this.left_0().top_0().bottom_0().w(size),
                                        Placement::Right => {
                                            this.right_0().top_0().bottom_0().w(size)
                                        }
                                        Placement::Top => this.top_0().left_0().right_0().h(size),
                                        Placement::Bottom => {
                                            this.bottom_0().left_0().right_0().h(size)
                                        }
                                    }
                                }
                                None => this.top_0().left_0().size_full(),
                            })
                            .group_drag_over::<DragPanel>("", |this| this.visible())
                            .on_drop(cx.listener(|this, drag: &DragPanel, window, cx| {
                                this.on_drop(drag, None, true, window, cx)
                            })),
                    )
            })
            .into_any_element()
    }

    /// Calculate the split direction based on the current mouse position
    fn on_panel_drag_move(
        &mut self,
        drag: &DragMoveEvent<DragPanel>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let bounds = drag.bounds;
        let position = drag.event.position;

        // Check the mouse position to determine the split direction
        if position.x < bounds.left() + bounds.size.width * 0.35 {
            self.will_split_placement = Some(Placement::Left);
        } else if position.x > bounds.left() + bounds.size.width * 0.65 {
            self.will_split_placement = Some(Placement::Right);
        } else if position.y < bounds.top() + bounds.size.height * 0.35 {
            self.will_split_placement = Some(Placement::Top);
        } else if position.y > bounds.top() + bounds.size.height * 0.65 {
            self.will_split_placement = Some(Placement::Bottom);
        } else {
            // center to merge into the current tab
            self.will_split_placement = None;
        }
        cx.notify()
    }

    /// Handle the drop event when dragging a panel
    ///
    /// - `active` - When true, the panel will be active after the drop
    fn on_drop(
        &mut self,
        drag: &DragPanel,
        ix: Option<usize>,
        active: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let panel = drag.panel.clone();
        let is_same_tab = drag.tab_panel == cx.entity();

        // If target is same tab, and it is only one panel, do nothing.
        if is_same_tab && ix.is_none() {
            if self.will_split_placement.is_none() {
                return;
            } else {
                if self.panels.len() == 1 {
                    return;
                }
            }
        }

        // Here is looks like remove_panel on a same item, but it difference.
        //
        // We must to split it to remove_panel, unless it will be crash by error:
        // Cannot update ui::dock::tab_panel::TabPanel while it is already being updated
        if is_same_tab {
            self.detach_panel(panel.clone(), window, cx);
        } else {
            let _ = drag.tab_panel.update(cx, |view, cx| {
                view.detach_panel(panel.clone(), window, cx);
                view.remove_self_if_empty(window, cx);
            });
        }

        // Insert into new tabs
        if let Some(placement) = self.will_split_placement {
            self.split_panel(panel, placement, None, window, cx);
        } else {
            if let Some(ix) = ix {
                self.insert_panel_at(panel, ix, window, cx)
            } else {
                self.add_panel_with_active(panel, active, window, cx)
            }
        }

        self.remove_self_if_empty(window, cx);
        cx.emit(PanelEvent::LayoutChanged);
    }

    /// Add panel with split placement
    fn split_panel(
        &self,
        panel: Arc<dyn PanelView>,
        placement: Placement,
        size: Option<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let dock_area = self.dock_area.clone();
        // wrap the panel in a TabPanel
        let new_tab_panel = cx.new(|cx| Self::new(None, dock_area.clone(), window, cx));
        new_tab_panel.update(cx, |view, cx| {
            view.add_panel(panel, window, cx);
        });

        let stack_panel = match self.stack_panel.as_ref().and_then(|panel| panel.upgrade()) {
            Some(panel) => panel,
            None => return,
        };

        let parent_axis = stack_panel.read(cx).axis;

        let ix = stack_panel
            .read(cx)
            .index_of_panel(Arc::new(cx.entity()))
            .unwrap_or_default();

        if parent_axis.is_vertical() && placement.is_vertical() {
            stack_panel.update(cx, |view, cx| {
                view.insert_panel_at(
                    Arc::new(new_tab_panel),
                    ix,
                    placement,
                    size,
                    dock_area.clone(),
                    window,
                    cx,
                );
            });
        } else if parent_axis.is_horizontal() && placement.is_horizontal() {
            stack_panel.update(cx, |view, cx| {
                view.insert_panel_at(
                    Arc::new(new_tab_panel),
                    ix,
                    placement,
                    size,
                    dock_area.clone(),
                    window,
                    cx,
                );
            });
        } else {
            // 1. Create new StackPanel with new axis
            // 2. Move cx.entity() from parent StackPanel to the new StackPanel
            // 3. Add the new TabPanel to the new StackPanel at the correct index
            // 4. Add new StackPanel to the parent StackPanel at the correct index
            let tab_panel = cx.entity();

            // Try to use the old stack panel, not just create a new one, to avoid too many nested
            // stack panels
            let new_stack_panel = if stack_panel.read(cx).panels_len() <= 1 {
                stack_panel.update(cx, |view, cx| {
                    view.remove_all_panels(window, cx);
                    view.set_axis(placement.axis(), window, cx);
                });
                stack_panel.clone()
            } else {
                cx.new(|cx| {
                    let mut panel = StackPanel::new(placement.axis(), window, cx);
                    panel.parent = Some(stack_panel.downgrade());
                    panel
                })
            };

            new_stack_panel.update(cx, |view, cx| match placement {
                Placement::Left | Placement::Top => {
                    view.add_panel(Arc::new(new_tab_panel), size, dock_area.clone(), window, cx);
                    view.add_panel(
                        Arc::new(tab_panel.clone()),
                        None,
                        dock_area.clone(),
                        window,
                        cx,
                    );
                }
                Placement::Right | Placement::Bottom => {
                    view.add_panel(
                        Arc::new(tab_panel.clone()),
                        None,
                        dock_area.clone(),
                        window,
                        cx,
                    );
                    view.add_panel(Arc::new(new_tab_panel), size, dock_area.clone(), window, cx);
                }
            });

            if stack_panel != new_stack_panel {
                stack_panel.update(cx, |view, cx| {
                    view.replace_panel(
                        Arc::new(tab_panel.clone()),
                        new_stack_panel.clone(),
                        window,
                        cx,
                    );
                });
            }

            cx.spawn_in(window, async move |_, cx| {
                cx.update(|window, cx| {
                    tab_panel.update(cx, |view, cx| view.remove_self_if_empty(window, cx))
                })
            })
            .detach()
        }

        cx.emit(PanelEvent::LayoutChanged);
    }

    fn focus_active_panel(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(active_panel) = self.active_panel(cx) {
            active_panel.focus_handle(cx).focus(window, cx);
        }
    }

    fn on_action_toggle_zoom(
        &mut self,
        _: &ToggleZoom,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.zoomable(cx).is_none() {
            return;
        }

        if !self.zoomed {
            cx.emit(PanelEvent::ZoomIn)
        } else {
            cx.emit(PanelEvent::ZoomOut)
        }
        self.zoomed = !self.zoomed;

        cx.spawn_in(window, {
            let zoomed = self.zoomed;
            async move |view, cx| {
                _ = cx.update(|window, cx| {
                    _ = view.update(cx, |view, cx| {
                        view.set_zoomed(zoomed, window, cx);
                    });
                });
            }
        })
        .detach();
    }

    fn on_action_close_panel(
        &mut self,
        _: &ClosePanel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(panel) = self.active_panel(cx) {
            // Only treat explicit close actions (tab close button, context menu Close, etc) as a
            // "close". Other flows like drag/drop will temporarily detach panels but should not
            // trigger close semantics.
            self.close_panel(panel, window, cx);
        }

        // Remove self from the parent DockArea.
        // This is ensure to remove from Tiles
        if self.panels.is_empty() && self.in_tiles {
            let tab_panel = Arc::new(cx.entity());
            window.defer(cx, {
                let dock_area = self.dock_area.clone();
                move |window, cx| {
                    _ = dock_area.update(cx, |this, cx| {
                        this.remove_panel_from_all_docks(tab_panel, window, cx);
                    });
                }
            });
        }
    }

    // Bind actions to the tab panel, only when the tab panel is not collapsed.
    fn bind_actions(&self, cx: &mut Context<Self>) -> Div {
        v_flex().when(!self.collapsed, |this| {
            this.on_action(cx.listener(Self::on_action_toggle_zoom))
                .on_action(cx.listener(Self::on_action_close_panel))
        })
    }
}

impl Focusable for TabPanel {
    fn focus_handle(&self, cx: &App) -> gpui::FocusHandle {
        if let Some(active_panel) = self.active_panel(cx) {
            active_panel.focus_handle(cx)
        } else {
            self.focus_handle.clone()
        }
    }
}
impl EventEmitter<DismissEvent> for TabPanel {}
impl EventEmitter<PanelEvent> for TabPanel {}
impl Render for TabPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        let focus_handle = self.focus_handle(cx);
        let active_panel = self.active_panel(cx);
        let state = TabState {
            draggable: self.draggable(cx),
            droppable: self.droppable(cx),
            zoomable: self.zoomable(cx),
            active_panel,
        };

        self.bind_actions(cx)
            .id("tab-panel")
            .track_focus(&focus_handle)
            .tab_group()
            .size_full()
            .overflow_hidden()
            .bg(cx.theme().background)
            .child(self.render_title_bar(&state, window, cx))
            .child(self.render_active_panel(&state, window, cx))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use gpui::{
        App, AvailableSpace, Context, EventEmitter, FocusHandle, Focusable, IntoElement, Render,
        SharedString, Window, div, point, px, size,
    };
    use gpui_common::TermuaIcon;

    use super::*;

    struct FakePanelWithIcon {
        focus: FocusHandle,
    }

    impl FakePanelWithIcon {
        fn new(cx: &mut Context<Self>) -> Self {
            Self {
                focus: cx.focus_handle(),
            }
        }
    }

    impl EventEmitter<PanelEvent> for FakePanelWithIcon {}

    impl Focusable for FakePanelWithIcon {
        fn focus_handle(&self, _cx: &App) -> FocusHandle {
            self.focus.clone()
        }
    }

    impl Panel for FakePanelWithIcon {
        fn panel_name(&self) -> &'static str {
            "fake.panel.with_icon"
        }

        fn tab_name(&self, _cx: &App) -> Option<SharedString> {
            Some("tab".into())
        }

        fn tab_icon(&self, _cx: &App) -> Option<TabIcon> {
            Some(TabIcon::ColoredSvg {
                path: TermuaIcon::GitBash.into(),
            })
        }

        fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            "tab"
        }
    }

    impl Render for FakePanelWithIcon {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    struct FakePanelWithoutIcon {
        focus: FocusHandle,
    }

    impl FakePanelWithoutIcon {
        fn new(cx: &mut Context<Self>) -> Self {
            Self {
                focus: cx.focus_handle(),
            }
        }
    }

    impl EventEmitter<PanelEvent> for FakePanelWithoutIcon {}

    impl Focusable for FakePanelWithoutIcon {
        fn focus_handle(&self, _cx: &App) -> FocusHandle {
            self.focus.clone()
        }
    }

    impl Panel for FakePanelWithoutIcon {
        fn panel_name(&self) -> &'static str {
            "fake.panel.without_icon"
        }

        fn tab_name(&self, _cx: &App) -> Option<SharedString> {
            Some("tab".into())
        }

        fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            "tab"
        }
    }

    impl Render for FakePanelWithoutIcon {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    struct FakePanelWithLongName {
        focus: FocusHandle,
        name: SharedString,
    }

    impl FakePanelWithLongName {
        fn new(name: impl Into<SharedString>, cx: &mut Context<Self>) -> Self {
            Self {
                focus: cx.focus_handle(),
                name: name.into(),
            }
        }
    }

    impl EventEmitter<PanelEvent> for FakePanelWithLongName {}

    impl Focusable for FakePanelWithLongName {
        fn focus_handle(&self, _cx: &App) -> FocusHandle {
            self.focus.clone()
        }
    }

    impl Panel for FakePanelWithLongName {
        fn panel_name(&self) -> &'static str {
            "fake.panel.long_name"
        }

        fn tab_name(&self, _cx: &App) -> Option<SharedString> {
            Some(self.name.clone())
        }

        fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div().child(self.name.clone())
        }
    }

    impl Render for FakePanelWithLongName {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    struct FakeClosablePanel {
        focus: FocusHandle,
        close_count: Arc<AtomicUsize>,
    }

    impl FakeClosablePanel {
        fn new(close_count: Arc<AtomicUsize>, cx: &mut Context<Self>) -> Self {
            Self {
                focus: cx.focus_handle(),
                close_count,
            }
        }
    }

    impl EventEmitter<PanelEvent> for FakeClosablePanel {}

    impl Focusable for FakeClosablePanel {
        fn focus_handle(&self, _cx: &App) -> FocusHandle {
            self.focus.clone()
        }
    }

    impl Panel for FakeClosablePanel {
        fn panel_name(&self) -> &'static str {
            "fake.panel.closable"
        }

        fn tab_name(&self, _cx: &App) -> Option<SharedString> {
            Some("tab".into())
        }

        fn title(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            "tab"
        }

        fn on_close(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
            self.close_count.fetch_add(1, Ordering::SeqCst);
        }
    }

    impl Render for FakeClosablePanel {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    #[gpui::test]
    fn tab_panel_close_all_calls_panel_on_close(cx: &mut gpui::TestAppContext) {
        cx.update(|app| {
            gpui_component::init(app);
            crate::init(app);
        });

        let close_count = Arc::new(AtomicUsize::new(0));
        let (tab_panel, window_cx) = cx.add_window_view(|window, cx| {
            let dock_area = cx.new(|cx| DockArea::new("dock", None, window, cx));
            TabPanel::new(None, dock_area.downgrade(), window, cx)
        });

        window_cx.update(|window, cx| {
            for _ in 0..3usize {
                let panel: Arc<dyn PanelView> =
                    Arc::new(cx.new(|cx| FakeClosablePanel::new(close_count.clone(), cx)));
                tab_panel.update(cx, |this, cx| this.add_panel(panel, window, cx));
            }

            tab_panel.update(cx, |this, cx| this.close_all_panels(window, cx));
        });

        assert_eq!(close_count.load(Ordering::SeqCst), 3);
    }

    #[gpui::test]
    fn tab_panel_close_others_calls_panel_on_close(cx: &mut gpui::TestAppContext) {
        cx.update(|app| {
            gpui_component::init(app);
            crate::init(app);
        });

        let close_count = Arc::new(AtomicUsize::new(0));
        let (tab_panel, window_cx) = cx.add_window_view(|window, cx| {
            let dock_area = cx.new(|cx| DockArea::new("dock", None, window, cx));
            TabPanel::new(None, dock_area.downgrade(), window, cx)
        });

        window_cx.update(|window, cx| {
            let keep_panel: Arc<dyn PanelView> =
                Arc::new(cx.new(|cx| FakeClosablePanel::new(close_count.clone(), cx)));
            let keep_id = keep_panel.panel_id(cx);
            tab_panel.update(cx, |this, cx| this.add_panel(keep_panel, window, cx));

            for _ in 0..2usize {
                let panel: Arc<dyn PanelView> =
                    Arc::new(cx.new(|cx| FakeClosablePanel::new(close_count.clone(), cx)));
                tab_panel.update(cx, |this, cx| this.add_panel(panel, window, cx));
            }

            tab_panel.update(cx, |this, cx| this.close_other_panels(keep_id, window, cx));
        });

        assert_eq!(close_count.load(Ordering::SeqCst), 2);
    }

    #[gpui::test]
    fn tab_panel_renders_tab_icon_when_panel_provides_one(cx: &mut gpui::TestAppContext) {
        cx.update(|app| {
            gpui_component::init(app);
            crate::init(app);
        });

        let cx = cx.add_empty_window();

        cx.draw(
            point(px(0.), px(0.)),
            size(
                AvailableSpace::Definite(px(800.)),
                AvailableSpace::Definite(px(600.)),
            ),
            |window, app| {
                let dock_area = app.new(|cx| DockArea::new("dock", None, window, cx));
                let tab_panel =
                    app.new(|cx| TabPanel::new(None, dock_area.downgrade(), window, cx));

                let panel: Arc<dyn PanelView> = Arc::new(app.new(|cx| FakePanelWithIcon::new(cx)));
                tab_panel.update(app, |tab_panel, cx| {
                    tab_panel.add_panel(panel, window, cx);
                });

                div().size_full().child(tab_panel)
            },
        );

        cx.run_until_parked();
        assert!(cx.debug_bounds("dock-tab-icon").is_some());
    }

    #[gpui::test]
    fn tab_panel_omits_tab_icon_when_panel_does_not_provide_one(cx: &mut gpui::TestAppContext) {
        cx.update(|app| {
            gpui_component::init(app);
            crate::init(app);
        });

        let cx = cx.add_empty_window();

        cx.draw(
            point(px(0.), px(0.)),
            size(
                AvailableSpace::Definite(px(800.)),
                AvailableSpace::Definite(px(600.)),
            ),
            |window, app| {
                let dock_area = app.new(|cx| DockArea::new("dock", None, window, cx));
                let tab_panel =
                    app.new(|cx| TabPanel::new(None, dock_area.downgrade(), window, cx));

                let panel: Arc<dyn PanelView> =
                    Arc::new(app.new(|cx| FakePanelWithoutIcon::new(cx)));
                tab_panel.update(app, |tab_panel, cx| {
                    tab_panel.add_panel(panel, window, cx);
                });

                div().size_full().child(tab_panel)
            },
        );

        cx.run_until_parked();
        assert!(cx.debug_bounds("dock-tab-icon").is_none());
    }

    #[gpui::test]
    fn tab_panel_sets_tab_overflow_when_tabs_exceed_width(cx: &mut gpui::TestAppContext) {
        cx.update(|app| {
            gpui_component::init(app);
            crate::init(app);
        });

        let (tab_panel, window_cx) = cx.add_window_view(|window, cx| {
            let dock_area = cx.new(|cx| DockArea::new("dock", None, window, cx));
            TabPanel::new(None, dock_area.downgrade(), window, cx)
        });

        window_cx.update(|window, cx| {
            for ix in 0..24usize {
                let panel: Arc<dyn PanelView> = Arc::new(cx.new(|cx| {
                    FakePanelWithLongName::new(
                        format!("Tab {ix} - This is a very long tab name"),
                        cx,
                    )
                }));
                tab_panel.update(cx, |this, cx| this.add_panel(panel, window, cx));
            }
        });

        for _ in 0..3 {
            let tab_panel_for_draw = tab_panel.clone();
            window_cx.draw(
                point(px(0.), px(0.)),
                size(
                    AvailableSpace::Definite(px(520.)),
                    AvailableSpace::Definite(px(360.)),
                ),
                move |_, _| div().size_full().child(tab_panel_for_draw),
            );
            window_cx.run_until_parked();
        }

        let max_overflow_x =
            window_cx.update(|_, cx| tab_panel.read(cx).tab_bar_scroll_handle.max_offset().x);

        assert!(
            max_overflow_x > px(8.),
            "expected scroll handle to report overflow, got {max_overflow_x:?}"
        );
        assert!(
            window_cx.update(|_, cx| tab_panel.read(cx).tab_overflow),
            "expected TabPanel to set tab_overflow=true when tabs overflow"
        );
    }

    #[gpui::test]
    fn tab_panel_scrolls_active_tab_into_view_on_resize(cx: &mut gpui::TestAppContext) {
        cx.update(|app| {
            gpui_component::init(app);
            crate::init(app);
        });

        let (tab_panel, window_cx) = cx.add_window_view(|window, cx| {
            let dock_area = cx.new(|cx| DockArea::new("dock", None, window, cx));
            TabPanel::new(None, dock_area.downgrade(), window, cx)
        });

        window_cx.update(|window, cx| {
            for ix in 0..24usize {
                let panel: Arc<dyn PanelView> = Arc::new(cx.new(|cx| {
                    FakePanelWithLongName::new(
                        format!("Tab {ix} - This is a very long tab name"),
                        cx,
                    )
                }));
                tab_panel.update(cx, |this, cx| this.add_panel(panel, window, cx));
            }
        });

        // Wide frame: ensure we start from a non-overflowed state so scroll offset clamps to 0.
        let tab_panel_for_draw = tab_panel.clone();
        window_cx.draw(
            point(px(0.), px(0.)),
            size(
                AvailableSpace::Definite(px(4000.)),
                AvailableSpace::Definite(px(360.)),
            ),
            move |_, _| div().size_full().child(tab_panel_for_draw),
        );
        window_cx.run_until_parked();

        let wide_offset_x =
            window_cx.update(|_, cx| tab_panel.read(cx).tab_bar_scroll_handle.offset().x);
        assert_eq!(
            wide_offset_x,
            px(0.),
            "expected no horizontal scroll at wide width"
        );

        // Resize narrower: active tab is the last one, so it must be scrolled into view.
        window_cx.simulate_resize(gpui::size(px(520.), px(360.)));
        // Render two frames: the first updates scroll handle bounds and schedules a scroll request
        // via `window.defer`, the second applies the scroll in prepaint.
        for _ in 0..2 {
            let tab_panel_for_draw = tab_panel.clone();
            window_cx.draw(
                point(px(0.), px(0.)),
                size(
                    AvailableSpace::Definite(px(520.)),
                    AvailableSpace::Definite(px(360.)),
                ),
                move |_, _| div().size_full().child(tab_panel_for_draw),
            );
            window_cx.run_until_parked();
        }

        let (bounds, last_bounds, offset_x) = window_cx.update(|_, cx| {
            let handle = &tab_panel.read(cx).tab_bar_scroll_handle;
            let bounds = handle.bounds();
            let last_bounds = handle
                .bounds_for_item(23)
                .expect("expected last tab bounds to exist");
            let offset_x = handle.offset().x;
            (bounds, last_bounds, offset_x)
        });

        assert!(
            offset_x < px(0.),
            "expected active tab to scroll into view, got offset_x={offset_x:?}"
        );
        assert!(
            last_bounds.left() + offset_x >= bounds.left() - px(1.),
            "expected active tab left edge to be visible"
        );
        if last_bounds.size.width <= bounds.size.width {
            assert!(
                last_bounds.right() + offset_x <= bounds.right() + px(1.),
                "expected active tab right edge to be visible (viewport={:?}, tab={:?}, \
                 offset_x={:?})",
                bounds,
                last_bounds,
                offset_x,
            );
        } else {
            // If the tab itself is wider than the viewport, we can only guarantee that it is
            // scrolled into view (typically aligning its left edge).
            assert!(
                (last_bounds.left() + offset_x - bounds.left()).abs() <= px(1.),
                "expected wide active tab to be aligned into view (viewport={:?}, tab={:?}, \
                 offset_x={:?})",
                bounds,
                last_bounds,
                offset_x,
            );
        }
    }
}
