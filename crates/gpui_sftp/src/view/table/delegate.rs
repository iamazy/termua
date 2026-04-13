use super::*;

impl TableDelegate for SftpTable {
    fn columns_count(&self, _cx: &App) -> usize {
        self.columns.len()
    }

    fn rows_count(&self, _cx: &App) -> usize {
        self.visible.len()
    }

    fn column(&self, col_ix: usize, _cx: &App) -> Column {
        self.columns[col_ix].clone()
    }

    fn render_th(
        &mut self,
        col_ix: usize,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let (label, sort_col) = match col_ix {
            0 => ("Name", SortColumn::Name),
            1 => ("Size", SortColumn::Size),
            2 => ("Modified", SortColumn::Modified),
            3 => ("Perms", SortColumn::Perms),
            _ => ("", SortColumn::Name),
        };

        let active = self.sort.column == sort_col;
        let icon = if active {
            Some(if self.sort.direction == SortDirection::Asc {
                IconName::ArrowUp
            } else {
                IconName::ArrowDown
            })
        } else {
            None
        };

        let mut row = div().flex().items_center().gap(px(6.0)).child(label);
        if let Some(icon) = icon {
            row = row.child(Icon::new(icon).xsmall());
        }

        row.on_mouse_down(
            MouseButton::Left,
            cx.listener(move |table, _e: &MouseDownEvent, _window, cx| {
                table.delegate_mut().set_sort(sort_col, cx);
                cx.stop_propagation();
            }),
        )
    }

    fn render_tr(
        &mut self,
        row_ix: usize,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> Stateful<Div> {
        // Remove per-row separator line.
        let Some(row) = self.visible.get(row_ix) else {
            return div().id(("row", row_ix)).border_0();
        };

        let is_selected = self.selected_ids.contains(row.id.as_str());
        let tr = div()
            .id(("row", row_ix))
            .border_0()
            .when(is_selected, |this| {
                this.bg(cx.theme().table_active.opacity(0.65))
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |table, ev: &MouseDownEvent, _window, cx| {
                    table.delegate_mut().click_row_local(row_ix, ev.modifiers);
                    // Keep Table's internal focus row in sync for keyboard navigation / preview.
                    table.set_selected_row(row_ix, cx);
                    cx.notify();
                }),
            );

        if row.kind != EntryKind::Dir {
            return tr;
        }

        tr.can_drop(|any, _window, _cx| {
            any.downcast_ref::<ExternalPaths>()
                .is_some_and(|paths| accept_external_file_drop_paths(paths.paths()))
        })
        .drag_over::<ExternalPaths>(|mut style, _paths, _window, cx| {
            let t = cx.theme();
            style = style
                .bg(t.table_active)
                .border_1()
                .border_color(t.table_active_border)
                .rounded_md();
            style.style().box_shadow = Some(folder_drop_ring_shadow(t.table_active_border));
            style
        })
        .on_drop(
            cx.listener(move |table, paths: &ExternalPaths, _window, cx| {
                let Some(remote_dir) = table.delegate().drop_upload_target_dir(Some(row_ix)) else {
                    return;
                };
                table.delegate_mut().upload_local_files_to_dir(
                    remote_dir,
                    paths.paths().to_vec(),
                    cx,
                );
            }),
        )
    }

    fn context_menu(
        &mut self,
        row_ix: usize,
        menu: PopupMenu,
        _window: &mut Window,
        _cx: &mut Context<TableState<Self>>,
    ) -> PopupMenu {
        self.set_context_menu_target(Some(row_ix));
        // SftpView provides a table-wide context menu (including blank area). Keep the row menu
        // empty here to avoid duplicate/competing menus.
        menu
    }

    fn render_empty(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let muted = cx.theme().muted_foreground.opacity(0.75);
        div()
            .id("sftp-empty")
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(8.0))
            .text_color(muted)
            .child(Icon::new(IconName::Inbox).size_6())
            .child("Empty directory")
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let Some(row) = self.visible.get(row_ix) else {
            return div();
        };

        let (muted, fg, accent) = {
            let t = cx.theme();
            (t.muted_foreground, t.foreground, t.accent)
        };

        let file_icon_color = match row.kind {
            EntryKind::Dir => accent.opacity(0.95),
            _ => muted.opacity(0.95),
        };

        let size_text = format_size(row.kind, row.size);
        let modified_text = format_modified(row.modified);
        let perms_text = row.perms.clone().unwrap_or_else(|| "---------".to_string());

        match col_ix {
            0 => {
                let depth = row.depth.saturating_sub(1);
                let pad = px(8.0) + px(depth as f32 * 14.0);

                let chevron = if row.is_expanded {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                };

                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .pl(pad)
                    .child(if row.kind == EntryKind::Dir {
                        div()
                            .w(px(16.0))
                            .h(px(16.0))
                            .flex_shrink_0()
                            .cursor_pointer()
                            .child(Icon::new(chevron).small().text_color(muted))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _ev: &MouseDownEvent, _window, cx| {
                                    this.delegate_mut().toggle_dir(row_ix, cx);
                                    cx.stop_propagation();
                                }),
                            )
                            .into_any_element()
                    } else {
                        div()
                            .w(px(16.0))
                            .h(px(16.0))
                            .flex_shrink_0()
                            .into_any_element()
                    })
                    .child(if row.kind == EntryKind::Dir {
                        div()
                            .w(px(16.0))
                            .h(px(16.0))
                            .flex_shrink_0()
                            .child(
                                img(sftp_dir_icon_path(row.is_expanded))
                                    .w_full()
                                    .h_full()
                                    .object_fit(gpui::ObjectFit::Contain),
                            )
                            .into_any_element()
                    } else {
                        if let Some(path) = file_icons::icon_path_for_file_name(&row.name) {
                            Icon::default()
                                .path(path)
                                .small()
                                .text_color(file_icon_color)
                                .into_any_element()
                        } else {
                            Icon::new(IconName::File)
                                .small()
                                .text_color(file_icon_color)
                                .into_any_element()
                        }
                    })
                    .child(
                        div()
                            .text_sm()
                            .text_color(if row.kind == EntryKind::Dir {
                                fg
                            } else {
                                fg.opacity(0.95)
                            })
                            .child(row.name.clone()),
                    )
                    .when(self.loading.contains(&row.id), |this| {
                        this.child(
                            div()
                                .ml(px(6.0))
                                .child(Icon::new(IconName::LoaderCircle).small().text_color(muted)),
                        )
                    })
            }
            1 => div().text_xs().text_color(muted).child(size_text),
            2 => div().text_xs().text_color(muted).child(modified_text),
            3 => div().text_xs().text_color(muted).child(perms_text),
            _ => div(),
        }
    }
}
