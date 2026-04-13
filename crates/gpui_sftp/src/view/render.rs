use gpui::{AnyElement, Hsla, Pixels};
use gpui_common::TermuaIcon;

use super::{format::human_bytes, *};

impl SftpView {
    fn preview_panel_width(&self, viewport: gpui::Size<Pixels>) -> Pixels {
        if self.show_preview {
            px((viewport.width.as_f32() * 0.38).clamp(280.0, 520.0))
        } else {
            px(0.0)
        }
    }

    fn render_body_root(table_panel: AnyElement, preview_panel: Option<AnyElement>) -> AnyElement {
        div()
            .id("sftp-body")
            .flex_1()
            .min_h_0()
            .flex()
            .items_stretch()
            .child(table_panel)
            .when_some(preview_panel, |this, panel| this.child(panel))
            .into_any_element()
    }

    fn render_table_panel(
        view_handle: Entity<SftpView>,
        table: Entity<TableState<SftpTable>>,
        row_h: Pixels,
        cx: &mut Context<SftpView>,
    ) -> AnyElement {
        div()
            .id("sftp-table-panel")
            .flex_1()
            .min_w_0()
            .relative()
            .can_drop(|any, _window, _cx| {
                any.downcast_ref::<ExternalPaths>()
                    .is_some_and(|paths| accept_external_file_drop_paths(paths.paths()))
            })
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _window, cx| {
                let table = this.table.clone();
                let remote_dir = table
                    .read(cx)
                    .delegate()
                    .tree
                    .as_ref()
                    .map(|t| t.root.clone())
                    .unwrap_or_default();
                if remote_dir.trim().is_empty() {
                    return;
                }

                table.update(cx, |state, cx| {
                    state.delegate_mut().upload_local_files_to_dir(
                        remote_dir,
                        paths.paths().to_vec(),
                        cx,
                    );
                });
            }))
            .on_any_mouse_down({
                let view_handle = view_handle.clone();
                let table = table.clone();
                move |_ev, window, cx| {
                    let bounds = view_handle.read(cx).table_bounds;
                    if bounds.size.width <= px(0.0) || bounds.size.height <= px(0.0) {
                        return;
                    }

                    // If the mouse is over a row, stop propagation so the root "click
                    // outside" handler doesn't close the preview.
                    let mouse = window.mouse_position();
                    let rel_y = mouse.y - bounds.origin.y;
                    let body_y = rel_y - row_h;
                    if body_y < px(0.0) {
                        return;
                    }

                    let scroll_y = table
                        .read(cx)
                        .vertical_scroll_handle
                        .0
                        .borrow()
                        .base_handle
                        .offset()
                        .y;
                    let ix = table_row_ix_from_mouse_y(body_y, scroll_y, row_h);
                    if ix.is_some_and(|ix| table.read(cx).delegate().row(ix).is_some()) {
                        cx.stop_propagation();
                    }
                }
            })
            .child(
                canvas(
                    {
                        let view_handle = view_handle.clone();
                        move |bounds, _window, cx| {
                            view_handle.update(cx, |this, _| this.table_bounds = bounds)
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .context_menu({
                let table_for_menu = table.clone();
                let view_handle_for_menu = view_handle;
                move |menu: PopupMenu, window: &mut Window, cx: &mut Context<PopupMenu>| {
                    SftpView::build_table_context_menu(
                        menu,
                        &table_for_menu,
                        &view_handle_for_menu,
                        row_h,
                        window,
                        cx,
                    )
                }
            })
            .child(Table::new(&table).bordered(false))
            .into_any_element()
    }

    fn maybe_push_pending_toast(
        &mut self,
        pending_toast: Option<PendingToast>,
        pending_toast_epoch: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(toast) = pending_toast else {
            return;
        };

        if pending_toast_epoch == self.last_pushed_notification_epoch {
            return;
        }

        struct SftpToastNotification;

        self.last_pushed_notification_epoch = pending_toast_epoch;

        let title = toast.title;
        let note = match (toast.level, toast.detail) {
            (PromptLevel::Info, Some(detail)) => Notification::info(detail).title(title),
            (PromptLevel::Warning, Some(detail)) => Notification::warning(detail).title(title),
            (PromptLevel::Critical, Some(detail)) => Notification::error(detail).title(title),
            (PromptLevel::Info, None) => Notification::info(title),
            (PromptLevel::Warning, None) => Notification::warning(title),
            (PromptLevel::Critical, None) => Notification::error(title),
        }
        .id::<SftpToastNotification>();

        window.push_notification(note, cx);

        // Best-effort cleanup so we don't hold stale notifications in the delegate.
        let table = self.table.clone();
        window.defer(cx, move |_window, cx| {
            table.update(cx, |state, cx| {
                let d = state.delegate_mut();
                if d.pending_toast_epoch == pending_toast_epoch {
                    d.pending_toast = None;
                }
                cx.notify();
            });
        });
    }

    fn render_breadcrumb_bar(
        &mut self,
        popover: Hsla,
        border: Hsla,
        cwd: String,
        show_hidden: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let crumbs = breadcrumbs_for_path(&cwd);
        let total = crumbs.len();
        let view_handle = cx.entity();

        let mut bc = Breadcrumb::new();
        for (ix, (label, path)) in crumbs.into_iter().enumerate() {
            let is_last = ix + 1 == total;
            if is_last {
                bc = bc.child(BreadcrumbItem::new(label).disabled(true));
            } else {
                let table = self.table.clone();
                bc = bc.child(BreadcrumbItem::new(label).on_click(move |_e, _window, cx| {
                    table.update(cx, |state, cx| state.delegate_mut().cd(path.clone(), cx));
                }));
            }
        }

        let table = self.table.clone();
        let path_input = self.path_input.clone();
        let search_button = Button::new("sftp-path-search")
            .icon(IconName::Search)
            .ghost()
            .compact()
            .on_click(move |_e, window, cx| {
                let was_editing = view_handle.read(cx).path_editing;
                view_handle.update(cx, |this, cx| {
                    this.path_editing = !was_editing;
                    cx.notify();
                });

                if was_editing {
                    window.focus(&view_handle.read(cx).focus_handle(cx), cx);
                    return;
                }

                let cwd = table
                    .read(cx)
                    .delegate()
                    .tree
                    .as_ref()
                    .map(|t| t.root.clone())
                    .unwrap_or_default();
                path_input.update(cx, |input, cx| {
                    input.set_value(cwd, window, cx);
                });
                window.focus(&path_input.read(cx).focus_handle(cx), cx);
            });

        let table = self.table.clone();
        let menu_button = Button::new("sftp-options-menu")
            .icon(IconName::EllipsisVertical)
            .ghost()
            .compact()
            .dropdown_menu(move |menu, _window, _cx| {
                menu.item(
                    // PopupMenu renders left icons at a fixed `.xsmall()` size.
                    // Render our own icon in the label so it can be bigger.
                    PopupMenuItem::element(move |_window, _cx| {
                        let (label, icon) = if show_hidden {
                            ("Hide Hidden Files", IconName::EyeOff)
                        } else {
                            ("Show Hidden Files", IconName::Eye)
                        };

                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(Icon::new(icon))
                            .text_sm()
                            .child(label)
                    })
                    .on_click({
                        let table = table.clone();
                        move |_e, _window, cx| {
                            table.update(cx, |state, cx| {
                                let d = state.delegate_mut();
                                d.show_hidden = !d.show_hidden;
                                d.rebuild_visible();
                                cx.notify();
                            });
                        }
                    }),
                )
            });

        div()
            .id("sftp-breadcrumb")
            .flex()
            .items_center()
            .justify_between()
            .px(px(8.0))
            .py(px(6.0))
            .bg(popover.opacity(0.55))
            .border_b_1()
            .border_color(border.opacity(0.9))
            .child(div().flex_1().child(if self.path_editing {
                Input::new(&self.path_input)
                    .cleanable(true)
                    .into_any_element()
            } else {
                bc.into_any_element()
            }))
            .child(search_button)
            .child(menu_button)
            .into_any_element()
    }

    fn build_table_context_menu(
        mut menu: PopupMenu,
        table: &Entity<TableState<SftpTable>>,
        view_handle: &Entity<SftpView>,
        row_h: Pixels,
        window: &mut Window,
        cx: &mut Context<PopupMenu>,
    ) -> PopupMenu {
        menu = menu.action_context(table.focus_handle(cx));

        let bounds = view_handle.read(cx).table_bounds;
        let mouse = window.mouse_position();
        let rel_y = mouse.y - bounds.origin.y;
        let body_y = rel_y - row_h;

        let mut row_ix = None;
        if body_y >= px(0.0) {
            let scroll_y = table
                .read(cx)
                .vertical_scroll_handle
                .0
                .borrow()
                .base_handle
                .offset()
                .y;
            if let Some(ix) = table_row_ix_from_mouse_y(body_y, scroll_y, row_h)
                && table.read(cx).delegate().row(ix).is_some()
            {
                row_ix = Some(ix);
            }
        }
        table.update(cx, |state, cx| {
            state.delegate_mut().set_context_menu_target(row_ix);
            cx.notify();
        });

        let spec_target = match row_ix {
            None => ContextMenuTarget::Background,
            Some(ix) => {
                let (count, has_file, has_dir) = table.read(cx).delegate().selection_summary();
                if count > 1 {
                    ContextMenuTarget::Multi { has_file, has_dir }
                } else {
                    let kind = table
                        .read(cx)
                        .delegate()
                        .row(ix)
                        .map(|row| row.kind)
                        .unwrap_or(EntryKind::File);
                    ContextMenuTarget::Single(kind)
                }
            }
        };

        let spec = sftp_context_menu(spec_target);
        for item in spec {
            menu = match item {
                ContextMenu::Separator => menu.separator(),
                ContextMenu::Action(action) => match action {
                    ContextMenuAction::Refresh => menu.menu_with_icon(
                        "Refresh",
                        Icon::default().path(TermuaIcon::Refresh),
                        Box::new(Refresh),
                    ),
                    ContextMenuAction::Upload => menu.menu_with_icon(
                        "Upload",
                        Icon::default().path(TermuaIcon::Upload),
                        Box::new(Upload),
                    ),
                    ContextMenuAction::Download => menu.menu_with_icon(
                        "Download",
                        Icon::default().path(TermuaIcon::Download),
                        Box::new(Download),
                    ),
                    ContextMenuAction::NewFolder => menu.menu_with_icon(
                        "New Folder",
                        Icon::default().path(TermuaIcon::FolderPlus),
                        Box::new(NewFolder),
                    ),
                    ContextMenuAction::Rename => menu.menu_with_icon(
                        "Rename",
                        Icon::default().path(TermuaIcon::SquarePen),
                        Box::new(Rename),
                    ),
                    ContextMenuAction::Delete => menu.menu_with_icon(
                        "Delete",
                        Icon::default().path(TermuaIcon::Trash),
                        Box::new(Delete),
                    ),
                },
            };
        }

        menu
    }

    fn render_preview_panel(
        &mut self,
        popover: Hsla,
        border: Hsla,
        preview_w: Pixels,
        row_h: Pixels,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.show_preview {
            return None;
        }

        let (preview_title, preview_size) = self
            .preview
            .target
            .as_ref()
            .map(|t| (SharedString::from(t.name.clone()), t.size))
            .unwrap_or_else(|| ("Preview".into(), None));

        let preview_body = self.render_preview_body(preview_w, window, cx);
        let muted_fg = cx.theme().muted_foreground.opacity(0.85);

        let (table_head_bg, table_head_fg, theme_border) = {
            let t = cx.theme();
            (t.table_head, t.table_head_foreground, t.border)
        };

        let preview_header = div()
            .id("sftp-preview-header")
            .flex()
            .items_center()
            .justify_between()
            // Match the Table header row ("Name", "Type", ...) height.
            .h(row_h)
            .px(px(10.0))
            .bg(table_head_bg)
            .text_color(table_head_fg)
            .border_b_1()
            .border_color(theme_border)
            .child(
                div()
                    .min_w_0()
                    .text_sm()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .child(preview_title),
            )
            .when_some(preview_size, |this, size| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(muted_fg)
                        .child(human_bytes(size)),
                )
            });

        Some(
            div()
                .id("sftp-preview-panel")
                .w(preview_w)
                .min_w(px(240.0))
                .flex_shrink_0()
                .flex()
                .flex_col()
                .min_h_0()
                .bg(popover.opacity(0.32))
                .border_l_1()
                .border_color(border.opacity(0.9))
                .on_any_mouse_down(|_ev, _window, cx| {
                    cx.stop_propagation();
                })
                .child(preview_header)
                .child(
                    div()
                        .id("sftp-preview-body")
                        .flex_1()
                        .min_h_0()
                        .p(px(8.0))
                        .child(preview_body),
                )
                .into_any_element(),
        )
    }

    fn render_preview_body(
        &mut self,
        preview_w: Pixels,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let muted_fg = cx.theme().muted_foreground.opacity(0.85);
        match &self.preview.content {
            PreviewContent::Empty => div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(8.0))
                .text_color(muted_fg)
                .child(Icon::new(IconName::Search).size_6())
                .child("Select a file to preview")
                .into_any_element(),
            PreviewContent::Loading => div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(8.0))
                .text_color(muted_fg)
                .child(Icon::new(IconName::LoaderCircle).size_6())
                .child("Loading preview...")
                .into_any_element(),
            PreviewContent::Binary => div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(8.0))
                .text_color(muted_fg)
                .child(Icon::new(IconName::TriangleAlert).size_6())
                .child("Binary file (preview unsupported)")
                .into_any_element(),
            PreviewContent::Error { message } => div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(8.0))
                .text_color(muted_fg)
                .child(Icon::new(IconName::Info).size_6())
                .child(message.clone())
                .into_any_element(),
            PreviewContent::Image { image } => {
                // Images are rendered "fit to width" so tall images can be scrolled.
                // (Using `h_full + ObjectFit::Contain` would scale them to fit height,
                // leaving nothing to scroll.)
                let content_w = (preview_w - px(16.0)).max(px(80.0));
                let render_size = image
                    .clone()
                    .get_render_image(window, cx)
                    .map(|r| r.size(0));

                let img_el = if let Some(size) = render_size
                    && size.width.0 > 0
                    && size.height.0 > 0
                {
                    let aspect = size.height.0 as f32 / size.width.0 as f32;
                    let h = px((content_w.as_f32() * aspect).max(1.0));
                    img(ImageSource::Image(image.clone()))
                        .w(content_w)
                        .h(h)
                        .object_fit(gpui::ObjectFit::Fill)
                } else {
                    // Fallback: let GPUI pick intrinsic sizing.
                    img(ImageSource::Image(image.clone()))
                        .w(content_w)
                        .object_fit(gpui::ObjectFit::Fill)
                };

                div()
                    .size_full()
                    .overflow_y_scrollbar()
                    .child(img_el)
                    .into_any_element()
            }
            PreviewContent::Text { fenced_markdown } => {
                TextView::markdown("sftp-preview-text", fenced_markdown.clone())
                    .selectable(true)
                    .scrollable(true)
                    .w_full()
                    .h_full()
                    .into_any_element()
            }
        }
    }

    fn render_body_panel(
        &mut self,
        popover: Hsla,
        border: Hsla,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let row_h = UiSize::default().table_row_height();

        let viewport = window.viewport_size();
        let preview_w = self.preview_panel_width(viewport);

        let view_handle = cx.entity();
        let table_panel = Self::render_table_panel(view_handle, self.table.clone(), row_h, cx);

        let preview_panel =
            self.render_preview_panel(popover, border, preview_w, row_h, window, cx);

        Self::render_body_root(table_panel, preview_panel)
    }

    fn render_op_modal(
        &mut self,
        op: &SftpOp,
        overlay: Hsla,
        popover: Hsla,
        border: Hsla,
        muted: Hsla,
        accent: Hsla,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let viewport = window.viewport_size();
        let panel_w = px(480.0).min(viewport.width.max(px(360.0)));
        let x = (viewport.width - panel_w).max(px(0.0)) / 2.0;
        let y = px(140.0);

        let title = match op.kind {
            SftpOpKind::NewFolder { .. } => "New Folder",
            SftpOpKind::Rename { .. } => "Rename",
        };

        div()
            .id("sftp-backdrop")
            .absolute()
            .left(px(0.0))
            .top(px(0.0))
            .size_full()
            .bg(overlay.opacity(0.35))
            .child(
                div()
                    .id("sftp-view")
                    .absolute()
                    .left(x)
                    .top(y)
                    .w(panel_w)
                    .bg(popover.opacity(0.98))
                    .border_1()
                    .border_color(border.opacity(0.9))
                    .rounded_md()
                    .shadow_lg()
                    .p(px(12.0))
                    .child(div().text_sm().child(title))
                    .child(
                        div()
                            .mt(px(10.0))
                            .child(Input::new(&op.input).cleanable(true)),
                    )
                    .child(
                        div()
                            .mt(px(12.0))
                            .flex()
                            .justify_end()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .cursor_pointer()
                                    .rounded_md()
                                    .px(px(10.0))
                                    .py(px(6.0))
                                    .bg(muted.opacity(0.2))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, window, cx| {
                                            this.close(window, cx);
                                            cx.stop_propagation();
                                        }),
                                    )
                                    .child(div().text_xs().child("Cancel")),
                            )
                            .child(
                                div()
                                    .cursor_pointer()
                                    .rounded_md()
                                    .px(px(10.0))
                                    .py(px(6.0))
                                    .bg(accent.opacity(0.3))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, window, cx| {
                                            this.confirm(window, cx);
                                            cx.stop_propagation();
                                        }),
                                    )
                                    .child(div().text_xs().child("OK")),
                            ),
                    ),
            )
            .into_any_element()
    }
}

impl Render for SftpView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Avoid holding an immutable borrow of `cx` across `cx.listener` calls.
        let (popover, border, overlay, muted, accent) = {
            let t = cx.theme();
            (t.popover, t.border, t.overlay, t.muted, t.accent)
        };
        let (pending_toast, pending_toast_epoch, op, _disconnected, cwd, show_hidden) = {
            let d = self.table.read(cx).delegate();
            (
                d.pending_toast.clone(),
                d.pending_toast_epoch,
                d.op.clone(),
                d.sftp.is_none(),
                d.tree.as_ref().map(|t| t.root.clone()).unwrap_or_default(),
                d.show_hidden,
            )
        };

        self.maybe_push_pending_toast(pending_toast, pending_toast_epoch, window, cx);
        let breadcrumb = self.render_breadcrumb_bar(popover, border, cwd, show_hidden, cx);

        let mut root = div()
            .id("sftp-view")
            .size_full()
            .flex()
            .flex_col()
            .track_focus(&self.focus_handle(cx))
            .on_any_mouse_down({
                let view_handle = cx.entity();
                move |_ev, _window, cx| {
                    view_handle.update(cx, |this, cx| this.close_preview(cx));
                }
            })
            .on_action(cx.listener(Self::on_refresh))
            .on_action(cx.listener(Self::on_upload))
            .on_action(cx.listener(Self::on_download))
            .on_action(cx.listener(Self::on_new_folder))
            .on_action(cx.listener(Self::on_rename))
            .on_action(cx.listener(Self::on_delete))
            .child(breadcrumb)
            .child(self.render_body_panel(popover, border, window, cx));

        if let Some(op) = op {
            root = root.child(
                self.render_op_modal(&op, overlay, popover, border, muted, accent, window, cx),
            );
        }

        root
    }
}
