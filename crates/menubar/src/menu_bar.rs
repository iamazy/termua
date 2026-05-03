use gpui::{
    App, AppContext, ClickEvent, Context, DismissEvent, Entity, Focusable, InteractiveElement,
    IntoElement, KeyBinding, MouseButton, OwnedMenu, OwnedMenuItem, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, Subscription, Window, actions, anchored,
    deferred, div, prelude::FluentBuilder, px,
};
use gpui_component::{
    IconName, Selectable, Sizable,
    button::{Button, ButtonVariants},
    h_flex,
    kbd::Kbd,
    menu::{PopupMenu, PopupMenuItem},
};
use rust_i18n::t;

use crate::state::MenuBarState;

const CONTEXT: &str = "FoldableAppMenuBar";

actions!(gpui_menubar, [Cancel, SelectLeft, SelectRight]);

fn owned_menu_signature(menu: &OwnedMenu) -> u64 {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    menu.name.hash(&mut hasher);
    owned_menu_items_signature(&menu.items, &mut hasher);
    hasher.finish()
}

fn owned_menu_items_signature(items: &[OwnedMenuItem], hasher: &mut impl std::hash::Hasher) {
    use std::hash::Hash;

    for item in items {
        match item {
            OwnedMenuItem::Separator => {
                0u8.hash(hasher);
            }
            OwnedMenuItem::Action { name, checked, .. } => {
                1u8.hash(hasher);
                name.hash(hasher);
                checked.hash(hasher);
            }
            OwnedMenuItem::Submenu(menu) => {
                2u8.hash(hasher);
                owned_menu_signature(menu).hash(hasher);
            }
            OwnedMenuItem::SystemMenu(menu) => {
                3u8.hash(hasher);
                menu.name.hash(hasher);
            }
        }
    }
}

pub(crate) fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("escape", Cancel, Some(CONTEXT)),
        KeyBinding::new("left", SelectLeft, Some(CONTEXT)),
        KeyBinding::new("right", SelectRight, Some(CONTEXT)),
    ]);
}

/// A Zed-like, foldable app menubar.
///
/// - macOS: renders nothing (use the native OS menubar via `cx.set_menus(...)`).
/// - Linux/Windows: renders an in-window menubar intended to be placed inside a custom titlebar.
///
/// Menu convention: `cx.get_menus()` top-level menus, where `menus[0]` is the fold/app menu.
pub struct FoldableAppMenuBar {
    fold_menu: Option<OwnedMenu>,
    fold_menu_signature: Option<u64>,
    fold_popup: Option<Entity<PopupMenu>>,
    fold_popup_subscription: Option<Subscription>,

    menus: Vec<Entity<FoldableAppMenu>>, // indices 1.. in the original menu order
    state: MenuBarState,
}

impl FoldableAppMenuBar {
    pub fn new(window: &mut Window, cx: &mut App) -> Entity<Self> {
        cx.new(|cx| {
            let menu_bar = cx.entity();
            let app_menus = cx.get_menus().unwrap_or_default();

            let fold_menu = app_menus.first().cloned();
            let fold_menu_signature = fold_menu.as_ref().map(owned_menu_signature);
            let menus = app_menus
                .iter()
                .enumerate()
                .skip(1)
                .map(|(ix, menu)| FoldableAppMenu::new(ix, menu, menu_bar.clone(), window, cx))
                .collect();

            Self {
                fold_menu,
                fold_menu_signature,
                fold_popup: None,
                fold_popup_subscription: None,
                menus,
                state: MenuBarState::default(),
            }
        })
    }

    fn sync_menus_from_owned(
        &mut self,
        app_menus: Vec<OwnedMenu>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.fold_menu = app_menus.first().cloned();
        let new_fold_signature = self.fold_menu.as_ref().map(owned_menu_signature);
        if self.fold_menu_signature != new_fold_signature {
            // Menu labels can change at runtime (e.g. locale changes). If a popup menu was built
            // previously, it cached the labels at that time. Drop the cached popup so the next
            // open rebuilds from the updated `OwnedMenuItem`s.
            self.fold_popup_subscription.take();
            self.fold_popup.take();
            self.fold_menu_signature = new_fold_signature;
        }

        let desired_len = app_menus.len().saturating_sub(1);
        if self.menus.len() != desired_len {
            let menu_bar = cx.entity();
            self.menus = app_menus
                .iter()
                .enumerate()
                .skip(1)
                .map(|(ix, menu)| FoldableAppMenu::new(ix, menu, menu_bar.clone(), window, cx))
                .collect();

            // Keep selection in range (menu indices include the fold/app menu at ix=0).
            if let Some(selected) = self.state.selected_ix {
                if selected >= app_menus.len() {
                    self.state.on_cancel();
                }
            }

            return;
        }

        for (entity, (ix, menu)) in self.menus.iter().zip(app_menus.iter().enumerate().skip(1)) {
            let menu = menu.clone();
            let signature = owned_menu_signature(&menu);
            entity.update(cx, |m, _cx| {
                if m.signature != signature {
                    m._subscription.take();
                    m.popup_menu.take();
                    m.signature = signature;
                }
                m.ix = ix;
                m.name = menu.name.clone();
                m.menu = menu.clone();
            });
        }
    }

    fn sync_menus_from_app(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Note: GPUI's test platform doesn't currently implement `set_menus`/`get_menus`.
        // We keep the sync logic in `sync_menus_from_owned` so it can be tested without relying
        // on the platform menu implementation.
        let Some(app_menus) = cx.get_menus() else {
            return;
        };
        self.sync_menus_from_owned(app_menus, window, cx);
    }

    fn menu_count(&self) -> usize {
        self.fold_menu.is_some() as usize + self.menus.len()
    }

    fn set_selected_index(&mut self, ix: Option<usize>, _: &mut Window, cx: &mut Context<Self>) {
        self.state.selected_ix = ix;

        // If switching away from the fold menu, drop its popup so the next open is fresh.
        if ix != Some(0) {
            self.fold_popup_subscription.take();
            self.fold_popup.take();
        }

        cx.notify();
    }

    fn on_move_left(&mut self, _: &SelectLeft, window: &mut Window, cx: &mut Context<Self>) {
        if !self.state.expanded {
            return;
        }
        let Some(selected_ix) = self.state.selected_ix else {
            return;
        };
        let count = self.menu_count();
        if count == 0 {
            return;
        }

        let new_ix = if selected_ix == 0 {
            count.saturating_sub(1)
        } else {
            selected_ix.saturating_sub(1)
        };
        self.set_selected_index(Some(new_ix), window, cx);
    }

    fn on_move_right(&mut self, _: &SelectRight, window: &mut Window, cx: &mut Context<Self>) {
        if !self.state.expanded {
            return;
        }
        let Some(selected_ix) = self.state.selected_ix else {
            return;
        };
        let count = self.menu_count();
        if count == 0 {
            return;
        }

        let new_ix = if selected_ix + 1 >= count {
            0
        } else {
            selected_ix + 1
        };
        self.set_selected_index(Some(new_ix), window, cx);
    }

    fn on_cancel(&mut self, _: &Cancel, window: &mut Window, cx: &mut Context<Self>) {
        self.state.on_cancel();
        self.set_selected_index(None, window, cx);
    }

    fn on_fold_mouse_down(
        &mut self,
        _: &gpui::MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Stop propagation to avoid dragging the window (titlebar drag region).
        window.prevent_default();
        cx.stop_propagation();

        self.state.on_fold_click();
        self.set_selected_index(self.state.selected_ix, window, cx);
    }

    fn on_fold_hover(&mut self, hovered: &bool, window: &mut Window, cx: &mut Context<Self>) {
        if !*hovered {
            return;
        }
        // Don't expand/open from hover when folded.
        if !self.state.expanded {
            return;
        }
        // Switch from other top-level menus back to the fold/app menu when the menubar is active.
        if self.state.selected_ix != Some(0) {
            self.set_selected_index(Some(0), window, cx);
        }
    }

    fn build_fold_popup_menu(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<PopupMenu> {
        let Some(fold_menu) = self.fold_menu.clone() else {
            // No menus configured; render nothing.
            return PopupMenu::build(window, cx, |menu, _, _| menu);
        };

        let popup_menu = match self.fold_popup.as_ref() {
            None => {
                let items = fold_menu.items;
                let popup = PopupMenu::build(window, cx, |menu, window, cx| {
                    let action_context = window.focused(cx);
                    let menu = menu.when_some(action_context.clone(), |this, handle| {
                        this.action_context(handle)
                    });
                    popup_with_owned_items(menu, items.clone(), action_context, window, cx)
                });
                popup.read(cx).focus_handle(cx).focus(window, cx);
                self.fold_popup_subscription =
                    Some(cx.subscribe_in(&popup, window, Self::handle_fold_dismiss));
                self.fold_popup = Some(popup.clone());
                popup
            }
            Some(menu) => menu.clone(),
        };

        let focus_handle = popup_menu.read(cx).focus_handle(cx);
        if !focus_handle.contains_focused(window, cx) {
            focus_handle.focus(window, cx);
        }

        popup_menu
    }

    fn handle_fold_dismiss(
        &mut self,
        _: &Entity<PopupMenu>,
        _: &DismissEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.fold_popup_subscription.take();
        self.fold_popup.take();
        self.state.on_dismiss();
        self.set_selected_index(None, window, cx);
    }
}

impl Render for FoldableAppMenuBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if cfg!(target_os = "macos") {
            return div().id("foldable-app-menu-bar");
        }

        self.sync_menus_from_app(window, cx);

        let fold_name: SharedString = self
            .fold_menu
            .as_ref()
            .map(|m| m.name.clone())
            .unwrap_or_else(|| t!("Menubar.FoldMenuFallback").to_string().into());

        let mut row = h_flex()
            .id("foldable-app-menu-bar")
            .key_context(CONTEXT)
            .items_center()
            .gap_x_1()
            .on_action(cx.listener(Self::on_move_left))
            .on_action(cx.listener(Self::on_move_right))
            .on_action(cx.listener(Self::on_cancel))
            .child(
                div()
                    .id("foldable-app-menu-bar-fold")
                    .relative()
                    .on_hover(cx.listener(Self::on_fold_hover))
                    .child({
                        let mut btn = Button::new("foldable-app-menu-bar-fold-trigger")
                            .ghost()
                            .compact()
                            .xsmall();

                        // When expanded, show the app name next to the fold icon.
                        if self.state.expanded {
                            btn = btn.label(fold_name);
                        } else {
                            btn = btn.icon(IconName::Menu);
                        }

                        btn.debug_selector(|| "menubar-fold-trigger".to_string())
                            // Keep the fold button from looking "selected" while another menu
                            // is active.
                            .selected(self.state.selected_ix == Some(0))
                            // Handle on mouse down (not click) so we don't re-open after a
                            // popup dismiss consumes the click.
                            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_fold_mouse_down))
                    })
                    .when(self.state.selected_ix == Some(0), |this| {
                        this.child(deferred(
                            anchored()
                                .anchor(gpui::Anchor::TopLeft)
                                .snap_to_window_with_margin(px(8.))
                                .child(
                                    div()
                                        .size_full()
                                        .occlude()
                                        .top_1()
                                        .child(self.build_fold_popup_menu(window, cx)),
                                ),
                        ))
                    }),
            );

        if self.state.expanded {
            row = row.children(self.menus.iter().cloned());
        }

        row
    }
}

struct FoldableAppMenu {
    menu_bar: Entity<FoldableAppMenuBar>,
    ix: usize,
    name: SharedString,
    menu: OwnedMenu,
    signature: u64,
    popup_menu: Option<Entity<PopupMenu>>,
    _subscription: Option<Subscription>,
}

impl FoldableAppMenu {
    fn new(
        ix: usize,
        menu: &OwnedMenu,
        menu_bar: Entity<FoldableAppMenuBar>,
        _: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        let name = menu.name.clone();
        let signature = owned_menu_signature(menu);
        cx.new(|_| Self {
            ix,
            menu_bar,
            name,
            menu: menu.clone(),
            signature,
            popup_menu: None,
            _subscription: None,
        })
    }

    fn build_popup_menu(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<PopupMenu> {
        let popup_menu = match self.popup_menu.as_ref() {
            None => {
                let items = self.menu.items.clone();
                let popup = PopupMenu::build(window, cx, |menu, window, cx| {
                    let action_context = window.focused(cx);
                    let menu = menu.when_some(action_context.clone(), |this, handle| {
                        this.action_context(handle)
                    });
                    popup_with_owned_items(menu, items.clone(), action_context, window, cx)
                });
                popup.read(cx).focus_handle(cx).focus(window, cx);
                self._subscription = Some(cx.subscribe_in(&popup, window, Self::handle_dismiss));
                self.popup_menu = Some(popup.clone());
                popup
            }
            Some(menu) => menu.clone(),
        };

        let focus_handle = popup_menu.read(cx).focus_handle(cx);
        if !focus_handle.contains_focused(window, cx) {
            focus_handle.focus(window, cx);
        }

        popup_menu
    }

    fn handle_dismiss(
        &mut self,
        _: &Entity<PopupMenu>,
        _: &DismissEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self._subscription.take();
        self.popup_menu.take();

        self.menu_bar.update(cx, |state, cx| {
            state.state.on_dismiss();
            state.set_selected_index(None, window, cx);
        });
    }

    fn handle_trigger_click(
        &mut self,
        _: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.menu_bar.update(cx, |state, cx| {
            state.state.on_menu_click(self.ix);
            state.set_selected_index(state.state.selected_ix, window, cx);
        });
    }

    fn handle_hover(&mut self, hovered: &bool, window: &mut Window, cx: &mut Context<Self>) {
        if !*hovered {
            return;
        }

        self.menu_bar.update(cx, |state, cx| {
            state.state.on_menu_hover(self.ix);
            state.set_selected_index(state.state.selected_ix, window, cx);
        });
    }
}

impl Render for FoldableAppMenu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let menu_bar = self.menu_bar.read(cx);
        let is_selected = menu_bar.state.selected_ix == Some(self.ix);
        let ix = self.ix;

        div()
            .id(("foldable-app-menu", self.ix))
            .relative()
            .child(
                Button::new(("foldable-app-menu-trigger", ix))
                    .xsmall()
                    .py_0p5()
                    .compact()
                    .ghost()
                    .label(self.name.clone())
                    .debug_selector(move || format!("menubar-menu-trigger-{ix}"))
                    .selected(is_selected)
                    .on_mouse_down(
                        MouseButton::Left,
                        |_: &gpui::MouseDownEvent, window: &mut Window, cx: &mut App| {
                            window.prevent_default();
                            cx.stop_propagation();
                        },
                    )
                    .on_click(cx.listener(Self::handle_trigger_click)),
            )
            .on_hover(cx.listener(Self::handle_hover))
            .when(is_selected, |this| {
                this.child(deferred(
                    anchored()
                        .anchor(gpui::Anchor::TopLeft)
                        .snap_to_window_with_margin(px(8.))
                        .child(
                            div()
                                .size_full()
                                .occlude()
                                .top_1()
                                .child(self.build_popup_menu(window, cx)),
                        ),
                ))
            })
    }
}

fn popup_with_owned_items(
    mut menu: PopupMenu,
    items: Vec<OwnedMenuItem>,
    action_context: Option<gpui::FocusHandle>,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    for (_item_ix, item) in items.into_iter().enumerate() {
        match item {
            OwnedMenuItem::Action {
                name,
                action,
                checked,
                ..
            } => {
                // Linux/Windows: our in-window menubar renders a check indicator for checked
                // items. macOS uses the native menubar, which already supports checkmarks via
                // `MenuItem::checked(...)`.
                let label: SharedString = name.into();
                let action_for_kbd = action.boxed_clone();
                let action_context = action_context.clone();

                let item = PopupMenuItem::element(move |window, _cx| {
                    let label_el = div().text_xs().child(label.clone());

                    let key = action_context
                        .as_ref()
                        .and_then(|handle| {
                            Kbd::binding_for_action_in(action_for_kbd.as_ref(), handle, window)
                        })
                        .or_else(|| Kbd::binding_for_action(action_for_kbd.as_ref(), None, window))
                        .map(|this| {
                            this.p_0()
                                .flex_nowrap()
                                .border_0()
                                .bg(gpui::transparent_white())
                        });

                    h_flex()
                        .w_full()
                        .gap_3()
                        .items_center()
                        .justify_between()
                        .child(label_el)
                        .when_some(key, |this, kbd| this.child(kbd))
                })
                .checked(checked)
                .action(action);

                menu = menu.item(item);
            }
            OwnedMenuItem::Separator => {
                menu = menu.separator();
            }
            OwnedMenuItem::Submenu(submenu) => {
                let name = submenu.name.clone();
                let sub_items = submenu.items.clone();
                let action_context = action_context.clone();
                menu = menu.submenu(name, window, cx, move |menu, window, cx| {
                    popup_with_owned_items(
                        menu,
                        sub_items.clone(),
                        action_context.clone(),
                        window,
                        cx,
                    )
                });
            }
            OwnedMenuItem::SystemMenu(_) => {}
        }
    }

    menu
}

#[cfg(all(test, not(target_os = "macos")))]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use gpui::{App, Menu, MenuItem, actions};

    use super::FoldableAppMenuBar;

    actions!(menubar_tests, [Toggle]);

    fn owned_menus(checked: bool) -> Vec<gpui::OwnedMenu> {
        let menus: Vec<Menu> = vec![
            Menu::new("Menu").items(vec![]),
            Menu::new("Run").items(vec![
                MenuItem::action("Multi Exec", Toggle).checked(checked),
            ]),
        ];
        menus.into_iter().map(|m| m.owned()).collect()
    }

    fn owned_menus_with_labels(
        fold_item_label: &str,
        run_item_label: &str,
    ) -> Vec<gpui::OwnedMenu> {
        let menus: Vec<Menu> = vec![
            Menu::new("Termua").items(vec![
                // Keep the fold/app menu name stable across locales (like "Termua") to ensure
                // our change detection is driven by item labels.
                MenuItem::action(fold_item_label.to_string(), Toggle),
            ]),
            Menu::new("Run").items(vec![MenuItem::action(run_item_label.to_string(), Toggle)]),
        ];
        menus.into_iter().map(|m| m.owned()).collect()
    }

    #[gpui::test]
    fn sync_menus_updates_checked_state(cx: &mut gpui::TestAppContext) {
        cx.update(|app| {
            gpui_component::init(app);
            crate::init(app);
            app.activate(true);
            app.on_action(|_: &Toggle, _: &mut App| {});
        });

        let cx = cx.add_empty_window();
        let menubar_cell: Rc<RefCell<Option<gpui::Entity<FoldableAppMenuBar>>>> =
            Rc::new(RefCell::new(None));

        cx.update(|window, app| {
            let menubar = FoldableAppMenuBar::new(window, app);
            *menubar_cell.borrow_mut() = Some(menubar);
        });

        let menubar = menubar_cell.borrow().clone().expect("menubar created");

        menubar.update_in(cx, |this, window, cx| {
            this.sync_menus_from_owned(owned_menus(false), window, cx)
        });

        cx.update(|_, app| {
            let run_menu = menubar
                .read(app)
                .menus
                .first()
                .cloned()
                .expect("Run menu exists");
            let item = run_menu
                .read(app)
                .menu
                .items
                .first()
                .expect("menu item exists");
            match item {
                gpui::OwnedMenuItem::Action { checked, .. } => assert!(!*checked),
                _ => panic!("expected Action menu item"),
            }
        });

        menubar.update_in(cx, |this, window, cx| {
            this.sync_menus_from_owned(owned_menus(true), window, cx)
        });

        cx.update(|_, app| {
            let run_menu = menubar
                .read(app)
                .menus
                .first()
                .cloned()
                .expect("Run menu exists");
            let item = run_menu
                .read(app)
                .menu
                .items
                .first()
                .expect("menu item exists");
            match item {
                gpui::OwnedMenuItem::Action { checked, .. } => assert!(*checked),
                _ => panic!("expected Action menu item"),
            }
        });
    }

    #[gpui::test]
    fn sync_menus_drops_cached_popups_when_labels_change(cx: &mut gpui::TestAppContext) {
        cx.update(|app| {
            gpui_component::init(app);
            crate::init(app);
            app.activate(true);
            app.on_action(|_: &Toggle, _: &mut App| {});
        });

        let cx = cx.add_empty_window();
        let menubar_cell: Rc<RefCell<Option<gpui::Entity<FoldableAppMenuBar>>>> =
            Rc::new(RefCell::new(None));

        cx.update(|window, app| {
            let menubar = FoldableAppMenuBar::new(window, app);
            *menubar_cell.borrow_mut() = Some(menubar);
        });

        let menubar = menubar_cell.borrow().clone().expect("menubar created");

        menubar.update_in(cx, |this, window, cx| {
            this.sync_menus_from_owned(
                owned_menus_with_labels("Open Settings", "Multi Execute"),
                window,
                cx,
            );

            // Build and cache both the fold menu popup and a top-level menu popup.
            let _ = this.build_fold_popup_menu(window, cx);
            assert!(this.fold_popup.is_some());

            let run_menu = this.menus.first().cloned().expect("Run menu exists");
            run_menu.update(cx, |menu, cx| {
                let _ = menu.build_popup_menu(window, cx);
                assert!(menu.popup_menu.is_some());
            });
        });

        // Changing labels (e.g. locale switch) should invalidate the cached popups so the next
        // open uses the new strings.
        menubar.update_in(cx, |this, window, cx| {
            this.sync_menus_from_owned(owned_menus_with_labels("打开设置", "批量执行"), window, cx);

            assert!(this.fold_popup.is_none());

            let run_menu = this.menus.first().cloned().expect("Run menu exists");
            assert!(run_menu.read(cx).popup_menu.is_none());
        });
    }
}
