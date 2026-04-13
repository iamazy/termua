use std::sync::Arc;

use gpui::{
    Context, Entity, InteractiveElement, IntoElement, MouseButton, ParentElement, Render,
    StatefulInteractiveElement, Styled, StyledImage, Window, div, img, prelude::FluentBuilder, px,
};
use gpui_common::TermuaIcon;
use gpui_component::{
    ActiveTheme, Icon, IconName, Sizable as _, StyledExt, h_flex,
    input::Input,
    list::ListItem,
    menu::{ContextMenu, PopupMenu, PopupMenuItem},
    tree::{TreeEntry, tree},
    v_flex,
};
use rust_i18n::t;

use super::{
    SessionsSidebarEvent, SessionsSidebarView, icons, icons::SessionIconKind, tree as sidebar_tree,
};
use crate::new_session::NewSessionWindow;

impl SessionsSidebarView {
    fn render_tree_row(
        ix: usize,
        entry: &TreeEntry,
        selected: bool,
        entity: &Entity<SessionsSidebarView>,
        session_icon_kinds: &Arc<std::collections::BTreeMap<i64, SessionIconKind>>,
        connecting_ids: &Arc<std::collections::HashSet<i64>>,
        muted_fg: gpui::Hsla,
    ) -> ListItem {
        let item = entry.item();
        let item_id = item.id.clone();
        let label = item.label.clone();
        let depth = entry.depth();
        let is_folder = entry.is_folder();
        let is_expanded = entry.is_expanded();
        let session_id = sidebar_tree::parse_session_id(item_id.as_ref());
        let folder_debug_name =
            is_folder.then(|| sidebar_tree::folder_debug_name(item_id.as_ref()));
        let is_ssh_session = item_id.as_ref().starts_with("session:ssh:");
        let connecting =
            session_id.is_some_and(|id| is_ssh_session && connecting_ids.contains(&id));

        let label_element = Self::render_tree_row_label_element(
            label,
            session_id,
            folder_debug_name.clone(),
            connecting,
            muted_fg,
        );

        let mut row = ListItem::new(ix)
            .selected(selected)
            .text_sm()
            .py_0p5()
            .px_2()
            .pl(px(10.) + px(14.) * depth)
            .child(Self::render_tree_row_wrapper(
                ix,
                is_folder,
                is_expanded,
                session_id,
                entity,
                item_id.clone(),
                folder_debug_name,
                session_icon_kinds,
                label_element,
            ));

        if is_folder {
            row = row.font_medium();
        } else if let Some(id) = session_id {
            let entity = entity.clone();
            row = row.on_click(move |ev, window, cx| {
                let should_open = ev.standard_click() && ev.click_count() >= 2;
                entity.update(cx, |this, cx| {
                    this.selected_item_id = item_id.clone();
                    this.hovered_session_id = Some(id);
                    this.sync_tree_selection(cx);
                    if should_open {
                        if item_id.as_ref().starts_with("session:ssh:") && this.is_connecting(id) {
                            // Prevent hammering the same unreachable host and spawning
                            // many slow connection attempts.
                        } else {
                            if item_id.as_ref().starts_with("session:ssh:") {
                                this.set_connecting(id, true, cx);
                            }
                            cx.emit(SessionsSidebarEvent::OpenSession(id));
                        }
                    }
                    cx.notify();
                });
                window.refresh();
            });
        }

        row
    }

    fn render_tree_row_wrapper(
        ix: usize,
        is_folder: bool,
        is_expanded: bool,
        session_id: Option<i64>,
        entity: &Entity<SessionsSidebarView>,
        item_id: gpui::SharedString,
        folder_debug_name: Option<String>,
        session_icon_kinds: &Arc<std::collections::BTreeMap<i64, SessionIconKind>>,
        label_element: gpui::AnyElement,
    ) -> gpui::AnyElement {
        let mut wrapper = div()
            .w_full()
            .id(format!("termua-sessions-tree-row-wrapper-{ix}"));
        if is_folder {
            let entity = entity.clone();
            wrapper = wrapper.on_mouse_down(MouseButton::Right, move |_ev, _window, app| {
                // Right-click on folders uses the "new session" menu.
                entity.update(app, |this, cx| {
                    this.hovered_session_id = None;
                    cx.notify();
                });
            });
        } else if let Some(id) = session_id {
            let entity_for_hover = entity.clone();
            let item_id_for_click = item_id.clone();
            wrapper = wrapper.on_hover(move |hovered, _window, app| {
                entity_for_hover.update(app, |this, cx| {
                    if *hovered {
                        this.hovered_session_id = Some(id);
                    } else if this.hovered_session_id == Some(id) {
                        this.hovered_session_id = None;
                    }
                    cx.notify();
                });
            });
            let entity_for_right_click = entity.clone();
            wrapper = wrapper.on_mouse_down(MouseButton::Right, move |_ev, window, app| {
                entity_for_right_click.update(app, |this, cx| {
                    this.selected_item_id = item_id_for_click.clone();
                    this.hovered_session_id = Some(id);
                    this.sync_tree_selection(cx);
                    cx.notify();
                });
                window.refresh();
            });
        }

        wrapper
            .child(
                h_flex()
                    .items_center()
                    .gap_1()
                    .child(Self::render_tree_row_chevron(is_folder, is_expanded))
                    .child(Self::render_tree_row_icon(
                        item_id.as_ref(),
                        is_folder,
                        is_expanded,
                        folder_debug_name,
                        session_id,
                        session_icon_kinds,
                    ))
                    .child(label_element),
            )
            .into_any_element()
    }

    fn render_tree_row_label_element(
        label: gpui::SharedString,
        session_id: Option<i64>,
        folder_debug_name: Option<String>,
        connecting: bool,
        muted_fg: gpui::Hsla,
    ) -> gpui::AnyElement {
        let label_text = if let Some(id) = session_id {
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .debug_selector(move || format!("termua-sessions-session-item-{id}"))
                .child(label)
                .into_any_element()
        } else {
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .when_some(folder_debug_name, |this, folder_debug_name| {
                    this.debug_selector(move || {
                        format!("termua-sessions-folder-row-{folder_debug_name}")
                    })
                })
                .child(label)
                .into_any_element()
        };

        h_flex()
            .items_center()
            .gap_x_1()
            .flex_1()
            .min_w_0()
            .child(label_text)
            .when(connecting, move |this| {
                let selector = session_id.map(|id| format!("termua-sessions-ssh-connecting-{id}"));
                this.child(
                    div()
                        .text_xs()
                        .text_color(muted_fg)
                        .when_some(selector, |this, selector| {
                            this.debug_selector(move || selector)
                        })
                        .child(t!("SessionsSidebar.Status.Connecting").to_string()),
                )
            })
            .into_any_element()
    }

    fn render_tree_row_chevron(is_folder: bool, is_expanded: bool) -> gpui::AnyElement {
        if !is_folder {
            return div().w(px(16.)).into_any_element();
        }

        gpui_component::Icon::new(IconName::ChevronRight)
            .size_4()
            .when(is_expanded, |this| {
                this.rotate(gpui::percentage(90. / 360.))
            })
            .into_any_element()
    }

    fn render_tree_row_icon(
        item_id: &str,
        is_folder: bool,
        is_expanded: bool,
        folder_debug_name: Option<String>,
        session_id: Option<i64>,
        session_icon_kinds: &Arc<std::collections::BTreeMap<i64, SessionIconKind>>,
    ) -> gpui::AnyElement {
        if is_folder {
            let icon_path = sidebar_tree::folder_icon_asset_path(is_expanded);
            let folder_debug_name = folder_debug_name.unwrap_or_else(|| "unknown".to_string());
            let state: &'static str = if is_expanded { "open" } else { "closed" };
            return div()
                .w(px(16.))
                .h(px(16.))
                .flex_shrink_0()
                .debug_selector(move || {
                    format!("termua-sessions-folder-icon-{state}-{folder_debug_name}")
                })
                .child(
                    img(icon_path)
                        .w_full()
                        .h_full()
                        .object_fit(gpui::ObjectFit::Contain),
                )
                .into_any_element();
        }

        if item_id.starts_with("session:local:") {
            return session_id
                .map(|id| {
                    session_icon_kinds
                        .get(&id)
                        .copied()
                        .unwrap_or(SessionIconKind::Terminal)
                        .into_element_for_session_id(id)
                })
                .unwrap_or_else(|| {
                    Icon::default()
                        .path(TermuaIcon::Terminal)
                        .size_4()
                        .into_any_element()
                });
        }

        if item_id.starts_with("session:ssh:") {
            return Icon::default()
                .path(TermuaIcon::Ssh)
                .size_4()
                .into_any_element();
        }

        if item_id.starts_with("session:serial:") {
            return Icon::default()
                .path(TermuaIcon::Usb)
                .size_4()
                .into_any_element();
        }

        div().w(px(16.)).into_any_element()
    }

    fn build_context_menu(
        menu: PopupMenu,
        menu_entity: &Entity<SessionsSidebarView>,
        action_context: gpui::FocusHandle,
        cx: &mut Context<PopupMenu>,
    ) -> PopupMenu {
        let hovered_session_id = menu_entity.read(cx).hovered_session_id;
        let menu = menu.action_context(action_context);

        match hovered_session_id {
            Some(session_id) => Self::build_context_menu_for_session(menu, menu_entity, session_id),
            None => Self::build_context_menu_for_background(menu),
        }
    }

    fn build_context_menu_for_session(
        menu: PopupMenu,
        menu_entity: &Entity<SessionsSidebarView>,
        session_id: i64,
    ) -> PopupMenu {
        menu.item(Self::context_menu_edit_item(menu_entity, session_id))
            .item(Self::context_menu_delete_item(menu_entity, session_id))
    }

    fn build_context_menu_for_background(menu: PopupMenu) -> PopupMenu {
        menu.item(
            PopupMenuItem::element(|_window, _cx| {
                div()
                    .id("termua-sessions-context-new-session")
                    .debug_selector(|| "termua-sessions-context-new-session".to_string())
                    .child(
                        h_flex()
                            .items_center()
                            .gap_x_1()
                            .child(
                                div()
                                    .id("termua-sessions-context-new-session-icon")
                                    .debug_selector(|| {
                                        "termua-sessions-context-new-session-icon".to_string()
                                    })
                                    .child(
                                        gpui_component::Icon::default()
                                            .path(TermuaIcon::PlugZap)
                                            .small(),
                                    ),
                            )
                            .child(t!("SessionsSidebar.Context.NewSession").to_string()),
                    )
            })
            .action(Box::new(crate::OpenNewSession)),
        )
    }

    fn context_menu_edit_item(
        menu_entity: &Entity<SessionsSidebarView>,
        session_id: i64,
    ) -> PopupMenuItem {
        let entity_for_click = menu_entity.clone();
        PopupMenuItem::element(|_window, _cx| {
            div()
                .id("termua-sessions-context-edit")
                .debug_selector(|| "termua-sessions-context-edit".to_string())
                .child(
                    h_flex()
                        .items_center()
                        .gap_x_1()
                        .child(
                            div()
                                .id("termua-sessions-context-edit-icon")
                                .debug_selector(|| "termua-sessions-context-edit-icon".to_string())
                                .child(
                                    gpui_component::Icon::default()
                                        .path(TermuaIcon::SquarePen)
                                        .small(),
                                ),
                        )
                        .child(t!("SessionsSidebar.Context.Edit").to_string()),
                )
        })
        .on_click(move |_, _window, cx| {
            if let Err(err) = NewSessionWindow::open_edit(session_id, cx) {
                log::warn!(
                    "SessionsSidebar: failed to open edit window for session {session_id}: {err:#}"
                );
            }

            entity_for_click.update(cx, |this, cx| {
                this.hovered_session_id = Some(session_id);
                cx.notify();
            });
        })
    }

    fn context_menu_delete_item(
        menu_entity: &Entity<SessionsSidebarView>,
        session_id: i64,
    ) -> PopupMenuItem {
        PopupMenuItem::element(|_window, _cx| {
            div()
                .id("termua-sessions-context-delete")
                .debug_selector(|| "termua-sessions-context-delete".to_string())
                .child(
                    h_flex()
                        .items_center()
                        .gap_x_1()
                        .child(
                            div()
                                .id("termua-sessions-context-delete-icon")
                                .debug_selector(|| {
                                    "termua-sessions-context-delete-icon".to_string()
                                })
                                .child(
                                    gpui_component::Icon::default()
                                        .path(TermuaIcon::Trash)
                                        .small(),
                                ),
                        )
                        .child(t!("SessionsSidebar.Context.Delete").to_string()),
                )
        })
        .on_click({
            let entity_for_click = menu_entity.clone();
            move |_, window, cx| {
                entity_for_click.update(cx, |this, cx| {
                    this.delete_session_by_id(session_id, window, cx);
                });
            }
        })
    }
}

impl Render for SessionsSidebarView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        let menu_entity = cx.entity();
        let session_icon_kinds = Arc::new(icons::build_session_icon_kinds(&self.sessions));
        let action_context = self.focus_handle.clone();
        let connecting_ids = Arc::new(self.connecting_session_ids.clone());
        let muted_fg = cx.theme().muted_foreground;

        v_flex()
            .id("termua-sessions-sidebar")
            .debug_selector(|| "termua-sessions-sidebar".to_string())
            .w_full()
            .flex_shrink_0()
            .h_full()
            .min_h_0()
            .bg(cx.theme().background)
            .child(div().p_2().child(Input::new(&self.search_input)))
            .child(
                ContextMenu::new(
                    "termua-sessions-sidebar-context-menu",
                    div()
                        .flex_1()
                        .min_h_0()
                        .child(tree(
                            &self.tree_state,
                            move |ix, entry: &TreeEntry, selected, _window, _app| {
                                SessionsSidebarView::render_tree_row(
                                    ix,
                                    entry,
                                    selected,
                                    &entity,
                                    &session_icon_kinds,
                                    &connecting_ids,
                                    muted_fg,
                                )
                            },
                        ))
                        .when(self.tree_items.is_empty(), |this| {
                            this.child(
                                div()
                                    .p_3()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(t!("SessionsSidebar.Empty").to_string()),
                            )
                        }),
                )
                .menu(move |menu: PopupMenu, _window, cx| {
                    SessionsSidebarView::build_context_menu(
                        menu,
                        &menu_entity,
                        action_context.clone(),
                        cx,
                    )
                }),
            )
    }
}
