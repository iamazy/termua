use gpui::{
    Context, Entity, InteractiveElement, IntoElement, MouseButton, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, StyledImage, Window, div, img,
    prelude::FluentBuilder, px,
};
use gpui_common::TermuaIcon;
use gpui_component::{
    ActiveTheme, Disableable, Icon, IconName, Selectable, Sizable, StyledExt, TitleBar,
    button::{Button, ButtonVariants},
    h_flex,
    input::Input,
    scroll::{Scrollbar, ScrollbarShow},
    select::Select,
    switch::Switch,
    tab::Tab,
    text::TextView,
    theme::Colorize,
    tree::{TreeEntry, tree},
    v_flex,
};
use rust_i18n::t;

use super::{
    EnvRowState, NewSessionWindow, Page, Protocol, SerialSessionState, ShellSessionState,
    SshAuthType, SshSessionState, new_proxy_jump_row_state, ssh, ssh::ssh_user_input_box_width,
};
use crate::store::SshProxyMode;

fn backend_icon(backend: crate::settings::TerminalBackend) -> TermuaIcon {
    match backend {
        crate::settings::TerminalBackend::Alacritty => TermuaIcon::Alacritty,
        crate::settings::TerminalBackend::Wezterm => TermuaIcon::Wezterm,
    }
}

#[derive(Clone, Copy)]
enum EnvEditorKind {
    Shell,
    Ssh,
    Serial,
}

fn render_env_editor(
    rows: &[EnvRowState],
    view: Entity<NewSessionWindow>,
    kind: EnvEditorKind,
) -> impl IntoElement + use<> {
    let (selector, add_id, delete_prefix) = match kind {
        EnvEditorKind::Shell => (
            "termua-new-session-shell-env",
            "termua-new-session-shell-env-add",
            "termua-new-session-shell-env-del",
        ),
        EnvEditorKind::Ssh => (
            "termua-new-session-ssh-env",
            "termua-new-session-ssh-env-add",
            "termua-new-session-ssh-env-del",
        ),
        EnvEditorKind::Serial => (
            "termua-new-session-serial-env",
            "termua-new-session-serial-env-add",
            "termua-new-session-serial-env-del",
        ),
    };
    let mut editor = v_flex()
        .w_full()
        .gap_1()
        .debug_selector(move || selector.to_string());

    for row in rows {
        let row_id = row.id;
        let view = view.clone();
        editor = editor.child(
            h_flex()
                .w_full()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .min_w(px(120.))
                        .child(Input::new(&row.name_input)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(160.))
                        .child(Input::new(&row.value_input)),
                )
                .child(
                    Button::new(format!("{delete_prefix}-{row_id}"))
                        .icon(IconName::Minus)
                        .xsmall()
                        .ghost()
                        .tab_stop(false)
                        .on_click(move |_, window, app| {
                            view.update(app, |this, cx| {
                                match kind {
                                    EnvEditorKind::Shell => {
                                        this.shell.env_rows.retain(|row| row.id != row_id)
                                    }
                                    EnvEditorKind::Ssh => {
                                        this.ssh.env_rows.retain(|row| row.id != row_id)
                                    }
                                    EnvEditorKind::Serial => {
                                        this.serial.env_rows.retain(|row| row.id != row_id)
                                    }
                                }
                                cx.notify();
                            });
                            window.refresh();
                        }),
                ),
        );
    }

    editor.child(
        h_flex().justify_end().child(
            Button::new(add_id)
                .icon(IconName::Plus)
                .xsmall()
                .ghost()
                .tab_stop(false)
                .on_click(move |_, window, app| {
                    view.update(app, |this, cx| {
                        match kind {
                            EnvEditorKind::Shell => this.push_shell_env_row(window, cx, None, None),
                            EnvEditorKind::Ssh => this.push_ssh_env_row(window, cx, None, None),
                            EnvEditorKind::Serial => {
                                this.push_serial_env_row(window, cx, None, None)
                            }
                        }
                        cx.notify();
                    });
                    window.refresh();
                }),
        ),
    )
}

impl Render for NewSessionWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let is_edit = self.mode.is_edit();
        let title = Self::window_title(self.mode, self.protocol);

        // Apply async serial port refresh results while we have access to `Window`.
        if let Some(ports) = self.serial.ports_pending.take() {
            self.serial.apply_ports(ports, window, cx);
        }

        // Initial serial port scan: do this lazily (only once) when the Serial tab is opened.
        if self.protocol == Protocol::Serial
            && !self.serial.ports_auto_started
            && !self.serial.ports_loading
        {
            self.serial.ports_auto_started = true;
            self.serial.start_refresh_ports_async(cx);
        }

        let connect_enabled = match self.protocol {
            Protocol::Ssh => {
                let host = self.ssh.host_input.read(cx).value();
                let port = self.ssh.port_input.read(cx).value();
                let password = self.ssh.password_input.read(cx).value();
                ssh::ssh_connect_enabled_for_values(
                    self.ssh.auth_type,
                    host.as_ref(),
                    port.as_ref(),
                    password.as_ref(),
                )
            }
            Protocol::Shell => true,
            Protocol::Serial => {
                let port = self.serial.port_select.read(cx).selected_value().cloned();
                let baud = self.serial.baud_input.read(cx).value();
                port.is_some_and(|p| !p.trim().is_empty()) && baud.trim().parse::<u32>().is_ok()
            }
        };

        let lock_overlay = self
            .lock_overlay
            .render_overlay_if_locked(Self::unlock_from_overlay, cx);

        let backend = match self.protocol {
            Protocol::Shell => self.shell.common.backend,
            Protocol::Ssh => self.ssh.common.backend,
            Protocol::Serial => self.serial.common.backend,
        };

        v_flex()
            .id("termua-new-session-window")
            .size_full()
            .bg(cx.theme().background)
            .relative()
            // Treat any interaction in this window as activity for the lock timer.
            .on_any_mouse_down(|_ev, _window, cx| {
                cx.global::<crate::lock_screen::LockState>()
                    .report_activity();
            })
            .on_mouse_move(|_ev, _window, cx| {
                cx.global::<crate::lock_screen::LockState>()
                    .report_activity();
            })
            .on_key_down(cx.listener(|_this, ev: &gpui::KeyDownEvent, _window, cx| {
                if ev.is_held {
                    return;
                }
                cx.global::<crate::lock_screen::LockState>()
                    .report_activity();
            }))
            .child(
                TitleBar::new().child(
                    h_flex()
                        .id("termua-new-session-titlebar-left")
                        .items_center()
                        .gap_x_1()
                        .child(
                            div()
                                .debug_selector(|| "termua-new-session-titlebar-icon".to_string())
                                .child(
                                    img(backend_icon(backend))
                                        .w(px(16.))
                                        .h(px(16.))
                                        .flex_shrink_0()
                                        .object_fit(gpui::ObjectFit::Contain),
                                ),
                        )
                        .child(div().text_sm().child(title)),
                ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .when(!is_edit, |this| {
                        this.child(self.render_protocol_tabs(window, cx))
                    })
                    .child(
                        h_flex()
                            .id("termua-new-session-main")
                            .flex_1()
                            .min_h_0()
                            .items_stretch()
                            .child(self.render_left_pane(window, cx))
                            .child(self.render_right_pane(window, cx)),
                    )
                    .child(self.render_footer(connect_enabled, window, cx))
                    .when_some(lock_overlay, |this, overlay| this.child(overlay)),
            )
            .children(gpui_component::Root::render_sheet_layer(window, cx))
            .children(gpui_component::Root::render_dialog_layer(window, cx))
            .children(gpui_component::Root::render_notification_layer(window, cx))
    }
}

impl NewSessionWindow {
    fn render_protocol_tab(
        this: Entity<Self>,
        label: String,
        protocol: Protocol,
        selected_ix: usize,
        selected_tab_tint: gpui::Hsla,
        tab_divider_color: gpui::Hsla,
        show_divider: bool,
    ) -> gpui::AnyElement {
        div()
            .relative()
            .flex_1()
            .debug_selector(move || format!("termua-new-session-tab-{}", protocol.debug_id()))
            .child(
                Tab::new()
                    .label(label)
                    .selected(selected_ix == protocol.tab_index())
                    .w_full()
                    .justify_center(),
            )
            .child(
                div()
                    .absolute()
                    .top_0()
                    .right_0()
                    .bottom_0()
                    .left_0()
                    .when(selected_ix == protocol.tab_index(), |this| {
                        this.bg(selected_tab_tint)
                    })
                    // `gpui-component::Tab` stops mouse-down propagation, so the click target
                    // needs to live above the visual tab instead of on a parent container.
                    .on_mouse_down(MouseButton::Left, move |_, window, app| {
                        this.update(app, |this, cx| this.set_protocol(protocol, cx));
                        window.refresh();
                    }),
            )
            .when(show_divider, |this| {
                this.child(
                    div()
                        .absolute()
                        .top_0()
                        .right_0()
                        .bottom_0()
                        .w(px(1.0))
                        .bg(tab_divider_color),
                )
            })
            .into_any_element()
    }

    fn render_protocol_tabs(
        &self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let this = cx.entity();
        let selected_ix = self.protocol.tab_index();
        let selected_tab_tint = cx.theme().tab_active.darken(0.08).opacity(0.32);
        let tab_divider_color = cx.theme().border.opacity(0.7);

        div()
            .w_full()
            .debug_selector(|| "termua-new-session-protocol-tabbar".to_string())
            .child(
                h_flex()
                    .id("termua-new-session-protocol-tabs")
                    .w_full()
                    .border_b_1()
                    .border_color(cx.theme().border.opacity(0.6))
                    .children([
                        Self::render_protocol_tab(
                            this.clone(),
                            t!("NewSession.Tabs.Shell").to_string(),
                            Protocol::Shell,
                            selected_ix,
                            selected_tab_tint,
                            tab_divider_color,
                            true,
                        ),
                        Self::render_protocol_tab(
                            this.clone(),
                            t!("NewSession.Tabs.Ssh").to_string(),
                            Protocol::Ssh,
                            selected_ix,
                            selected_tab_tint,
                            tab_divider_color,
                            true,
                        ),
                        Self::render_protocol_tab(
                            this,
                            t!("NewSession.Tabs.Serial").to_string(),
                            Protocol::Serial,
                            selected_ix,
                            selected_tab_tint,
                            tab_divider_color,
                            false,
                        ),
                    ]),
            )
    }

    fn render_footer(
        &self,
        connect_enabled: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let view = cx.entity();
        let is_edit = self.mode.is_edit();

        h_flex()
            .id("termua-new-session-footer")
            .items_center()
            .justify_end()
            .p_3()
            .border_t_1()
            .border_color(gpui::transparent_white())
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .when(is_edit, |this| {
                        let view = view.clone();
                        this.child(
                            Button::new("termua-edit-session-save")
                                .label(t!("NewSession.Button.Save").to_string())
                                .primary()
                                .disabled(!connect_enabled || self.submit_in_flight)
                                .on_click({
                                    move |_, window, app| {
                                        let result = view.update(app, |this, cx| {
                                            this.save_edit_session(window, cx)
                                        });
                                        if let Err(err) = result {
                                            crate::notification::notify_app(
                                                crate::notification::MessageKind::Info,
                                                format!("{err:#}"),
                                                window,
                                                app,
                                            );
                                            return;
                                        }
                                    }
                                }),
                        )
                    })
                    .when(!is_edit, |this| {
                        this.child(
                            Button::new("termua-new-session-connect")
                                .icon(Icon::default().path(TermuaIcon::Send))
                                .label(t!("NewSession.Button.Connect").to_string())
                                .primary()
                                .disabled(!connect_enabled || self.submit_in_flight)
                                .on_click({
                                    let view = view.clone();
                                    move |_, window, app| {
                                        let result = view
                                            .update(app, |this, cx| this.connect_new_session(cx));
                                        if let Err(err) = result {
                                            crate::notification::notify_app(
                                                crate::notification::MessageKind::Info,
                                                format!("{err:#}"),
                                                window,
                                                app,
                                            );
                                            return;
                                        }

                                        window.remove_window();
                                    }
                                }),
                        )
                    })
                    .child(
                        Button::new("termua-new-session-cancel")
                            .icon(IconName::Close)
                            .label(t!("NewSession.Button.Cancel").to_string())
                            .ghost()
                            .on_click(move |_, window, _app| window.remove_window()),
                    ),
            )
    }

    fn render_left_pane(&self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();

        v_flex()
            .id("termua-new-session-left-pane")
            .debug_selector(|| "termua-new-session-left-pane".to_string())
            .w(px(260.))
            .flex_shrink_0()
            .min_h_0()
            .border_r_1()
            .border_color(cx.theme().border.opacity(0.6))
            .child(div().flex_1().min_h_0().child(tree(
                &self.nav_tree_state,
                move |ix, entry: &TreeEntry, selected, _window, _cx| {
                    let is_folder = entry.is_folder();
                    let item_id = entry.item().id.clone();
                    let mut row = crate::window::nav_tree::nav_tree_row(ix, entry, selected);

                    if is_folder {
                        row = row.font_medium();
                    } else {
                        let entity = entity.clone();
                        row = row.on_click(move |_, window, app| {
                            entity.update(app, |this, cx| {
                                if this.selected_item_id.as_ref() != item_id.as_ref() {
                                    this.right_scroll_handle
                                        .set_offset(gpui::point(px(0.), px(0.)));
                                }
                                this.selected_item_id = item_id.clone();
                                this.sync_nav_tree_selection(cx);
                                cx.notify();
                            });
                            window.refresh();
                        });
                    }

                    row
                },
            )))
    }

    fn render_right_pane(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let page = super::page_for_tree_item_id(self.protocol, self.selected_item_id.as_ref());
        let view = cx.entity();

        h_flex()
            .id("termua-new-session-right-pane")
            .flex_1()
            .min_w(px(0.))
            .min_h_0()
            .items_stretch()
            .child(
                div()
                    .id("termua-new-session-right-scroll-area")
                    .flex_1()
                    .min_w(px(0.))
                    .min_h_0()
                    .flex_col()
                    .track_scroll(&self.right_scroll_handle)
                    .overflow_y_scroll()
                    .overflow_x_hidden()
                    .child(match page {
                        Page::ShellSession => div()
                            .w_full()
                            .p_4()
                            .flex_none()
                            .child(self.shell.render(view, window, cx))
                            .into_any_element(),
                        Page::SshSession => div()
                            .w_full()
                            .p_4()
                            .flex_none()
                            .child(self.ssh.render_session(view, window, cx))
                            .into_any_element(),
                        Page::SshConnection => div()
                            .w_full()
                            .p_4()
                            .flex_none()
                            .child(self.ssh.render_connection(view, window, cx))
                            .into_any_element(),
                        Page::SshProxy => div()
                            .w_full()
                            .p_4()
                            .flex_none()
                            .child(self.ssh.render_proxy(view, window, cx))
                            .into_any_element(),
                        Page::SerialSession => div()
                            .w_full()
                            .p_4()
                            .flex_none()
                            .child(self.serial.render_session(view, window, cx))
                            .into_any_element(),
                        Page::SerialConnection => div()
                            .w_full()
                            .p_4()
                            .flex_none()
                            .child(self.serial.render_connection(view, window, cx))
                            .into_any_element(),
                        Page::SerialFrameSettings => div()
                            .w_full()
                            .p_4()
                            .flex_none()
                            .child(self.serial.render_line_settings(view, window, cx))
                            .into_any_element(),
                    }),
            )
            .child(
                div()
                    .w(px(16.0))
                    .flex_shrink_0()
                    .relative()
                    .h_full()
                    .min_h_0()
                    .child(
                        Scrollbar::vertical(&self.right_scroll_handle)
                            .id("termua-new-session-right-scrollbar")
                            .scrollbar_show(ScrollbarShow::Scrolling),
                    ),
            )
    }
}

fn render_form_row(
    label: impl Into<SharedString>,
    control: impl IntoElement,
    cx: &mut Context<NewSessionWindow>,
) -> impl IntoElement {
    h_flex()
        .w_full()
        .items_center()
        .gap_3()
        .child(
            div()
                .w(px(110.))
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(label.into()),
        )
        .child(div().flex_1().min_w(px(280.)).child(control))
}

fn render_backend_type_row(
    common: &super::SessionCommonState,
    selector: &'static str,
    cx: &mut Context<NewSessionWindow>,
) -> gpui::AnyElement {
    render_form_row(
        t!("NewSession.Field.Type").to_string(),
        div()
            .w_full()
            .debug_selector(move || selector.to_string())
            .child(Select::new(&common.backend_select)),
        cx,
    )
    .into_any_element()
}

impl ShellSessionState {
    pub(super) fn render(
        &self,
        view: Entity<NewSessionWindow>,
        _window: &mut Window,
        cx: &mut Context<NewSessionWindow>,
    ) -> impl IntoElement {
        let env_editor = render_env_editor(&self.env_rows, view, EnvEditorKind::Shell);

        v_flex()
            .id("termua-new-session-shell-session")
            .gap_3()
            .child(render_backend_type_row(
                &self.common,
                "termua-new-session-shell-backend-type-select",
                cx,
            ))
            .child(render_form_row(
                t!("NewSession.Field.Shell").to_string(),
                div()
                    .w_full()
                    .debug_selector(|| "termua-new-session-shell-type".to_string())
                    .child(
                        div()
                            .w_full()
                            .debug_selector(|| "termua-new-session-shell-type-select".to_string())
                            .child(Select::new(&self.program_select)),
                    ),
                cx,
            ))
            .child(render_form_row(
                t!("NewSession.Field.Label").to_string(),
                div().w_full().child(Input::new(&self.common.label_input)),
                cx,
            ))
            .child(render_form_row(
                t!("NewSession.Field.Group").to_string(),
                div()
                    .w_full()
                    .debug_selector(|| "termua-new-session-shell-group-input".to_string())
                    .child(Input::new(&self.common.group_input)),
                cx,
            ))
            .child(render_form_row(
                t!("NewSession.Field.EnvironmentVariables").to_string(),
                env_editor,
                cx,
            ))
    }
}

impl SshSessionState {
    fn render_proxy_command_input(&self) -> impl IntoElement {
        div()
            .w_full()
            .debug_selector(|| "termua-new-session-ssh-proxy-command".to_string())
            .child(Input::new(&self.proxy_command_input))
    }

    fn render_proxy_workdir_input(&self) -> impl IntoElement {
        div()
            .w_full()
            .debug_selector(|| "termua-new-session-ssh-proxy-working-dir".to_string())
            .child(Input::new(&self.proxy_workdir_input))
    }

    fn render_proxy_env_editor(&self, view: Entity<NewSessionWindow>) -> impl IntoElement {
        let mut rows = v_flex()
            .w_full()
            .gap_1()
            .debug_selector(|| "termua-new-session-ssh-proxy-env".to_string());

        for row in self.proxy_env_rows.iter() {
            let row_id = row.id;
            let view = view.clone();
            rows = rows.child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(120.))
                            .child(Input::new(&row.name_input)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(160.))
                            .child(Input::new(&row.value_input)),
                    )
                    .child(
                        Button::new(format!("termua-new-session-ssh-proxy-env-del-{row_id}"))
                            .icon(IconName::Minus)
                            .xsmall()
                            .ghost()
                            .tab_stop(false)
                            .on_click(move |_, window, app| {
                                view.update(app, |this, cx| {
                                    this.ssh.proxy_env_rows.retain(|r| r.id != row_id);
                                    cx.notify();
                                });
                                window.refresh();
                            }),
                    ),
            );
        }

        rows.child(
            h_flex().justify_end().child(
                Button::new("termua-new-session-ssh-proxy-env-add")
                    .icon(IconName::Plus)
                    .xsmall()
                    .ghost()
                    .tab_stop(false)
                    .on_click(move |_, window, app| {
                        view.update(app, |this, cx| {
                            this.push_ssh_proxy_env_row(window, cx, None, None);
                            cx.notify();
                        });
                        window.refresh();
                    }),
            ),
        )
    }

    fn render_proxy_jump_editor(&self, view: Entity<NewSessionWindow>) -> impl IntoElement {
        let mut rows = v_flex()
            .w_full()
            .gap_1()
            .debug_selector(|| "termua-new-session-ssh-proxy-jump-chain".to_string());

        for row in self.proxy_jump_rows.iter() {
            let row_id = row.id;
            let view = view.clone();
            rows = rows.child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(180.))
                            .child(Input::new(&row.host_input)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(120.))
                            .child(Input::new(&row.user_input)),
                    )
                    .child(div().w(px(90.)).child(Input::new(&row.port_input)))
                    .child(
                        Button::new(format!("termua-new-session-ssh-proxy-jump-del-{row_id}"))
                            .icon(IconName::Minus)
                            .xsmall()
                            .ghost()
                            .tab_stop(false)
                            .on_click(move |_, window, app| {
                                view.update(app, |this, cx| {
                                    this.ssh.proxy_jump_rows.retain(|r| r.id != row_id);
                                    cx.notify();
                                });
                                window.refresh();
                            }),
                    ),
            );
        }

        rows.child(
            h_flex().justify_end().child(
                Button::new("termua-new-session-ssh-proxy-jump-add")
                    .debug_selector(|| "termua-new-session-ssh-proxy-jump-add".to_string())
                    .icon(IconName::Plus)
                    .xsmall()
                    .ghost()
                    .tab_stop(false)
                    .on_click(move |_, window, app| {
                        view.update(app, |this, cx| {
                            let id = this.ssh.proxy_jump_next_id;
                            this.ssh.proxy_jump_next_id += 1;

                            this.ssh
                                .proxy_jump_rows
                                .push(new_proxy_jump_row_state(id, window, cx, None, None, None));
                            cx.notify();
                        });
                        window.refresh();
                    }),
            ),
        )
    }

    fn render_host_row(
        &self,
        host_error: Option<String>,
        port_error: Option<String>,
        window: &mut Window,
        cx: &mut Context<NewSessionWindow>,
    ) -> gpui::AnyElement {
        let inline_port = |host_input: gpui::AnyElement,
                           this: &Self,
                           port_error: Option<String>,
                           cx: &mut Context<NewSessionWindow>| {
            v_flex()
                .gap_1()
                .child(
                    h_flex()
                        .w_full()
                        .items_center()
                        .gap_1()
                        .child(host_input)
                        .child(
                            div()
                                .debug_selector(|| {
                                    "termua-new-session-ssh-host-port-colon".to_string()
                                })
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(":"),
                        )
                        .child(
                            div()
                                .w(px(80.))
                                .debug_selector(|| {
                                    "termua-new-session-ssh-port-inline-input".to_string()
                                })
                                .child(
                                    Input::new(&this.port_input)
                                        .when(port_error.is_some(), |this| {
                                            this.border_color(cx.theme().danger)
                                        }),
                                ),
                        ),
                )
                .when_some(port_error, |this, msg| {
                    this.child(
                        div()
                            .debug_selector(|| "termua-new-session-ssh-port-error".to_string())
                            .text_xs()
                            .text_color(cx.theme().danger)
                            .child(msg),
                    )
                })
        };

        let control = match self.auth_type {
            SshAuthType::Password => {
                let user_value = self.user_input.read(cx).value();
                let user_w = ssh_user_input_box_width(window, cx, user_value.as_ref());

                inline_port(
                    h_flex()
                        .flex_1()
                        .min_w(px(160.))
                        .items_center()
                        .gap_1()
                        .child(
                            div()
                                .w(user_w)
                                .flex_shrink_0()
                                .debug_selector(|| "termua-new-session-ssh-user-input".to_string())
                                .child(Input::new(&self.user_input)),
                        )
                        .child(
                            div()
                                .debug_selector(|| "termua-new-session-ssh-at-label".to_string())
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child("@"),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(160.))
                                .debug_selector(|| "termua-new-session-ssh-host-input".to_string())
                                .child(Input::new(&self.host_input)),
                        )
                        .into_any_element(),
                    self,
                    port_error,
                    cx,
                )
                .into_any_element()
            }
            SshAuthType::Config => v_flex()
                .gap_1()
                .child(inline_port(
                    div()
                        .flex_1()
                        .min_w(px(160.))
                        .debug_selector(|| "termua-new-session-ssh-host-input".to_string())
                        .child(
                            Input::new(&self.host_input).when(host_error.is_some(), |this| {
                                this.border_color(cx.theme().danger)
                            }),
                        )
                        .into_any_element(),
                    self,
                    port_error,
                    cx,
                ))
                .when_some(host_error, |this, msg| {
                    this.child(
                        div()
                            .debug_selector(|| "termua-new-session-ssh-host-error".to_string())
                            .text_xs()
                            .text_color(cx.theme().danger)
                            .child(msg),
                    )
                })
                .into_any_element(),
        };

        render_form_row(t!("NewSession.Field.Host").to_string(), control, cx).into_any_element()
    }

    fn render_auth_type_row(&self, cx: &mut Context<NewSessionWindow>) -> gpui::AnyElement {
        let control = div()
            .w_full()
            .debug_selector(|| "termua-new-session-ssh-auth-type".to_string())
            .child(
                div()
                    .w_full()
                    .debug_selector(|| "termua-new-session-ssh-auth-type-select".to_string())
                    .child(Select::new(&self.auth_select)),
            );

        render_form_row(t!("NewSession.Ssh.Field.AuthType").to_string(), control, cx)
            .into_any_element()
    }

    fn render_password_row(
        &self,
        view: Entity<NewSessionWindow>,
        password_locked: bool,
        cx: &mut Context<NewSessionWindow>,
    ) -> Option<gpui::AnyElement> {
        if self.auth_type != SshAuthType::Password {
            return None;
        }

        let password_control = if password_locked {
            Input::new(&self.password_input)
                .disabled(true)
                .suffix(
                    Button::new("termua-edit-session-password-edit")
                        .debug_selector(|| "termua-edit-session-password-edit".to_string())
                        .icon(Icon::default().path(TermuaIcon::SquarePen))
                        .xsmall()
                        .ghost()
                        .tab_stop(false)
                        .on_click(move |_, window, app| {
                            view.update(app, |this, cx| {
                                this.ssh.password_edit_unlocked = true;
                                this.ssh.password_input.update(cx, |input, cx| {
                                    input.set_value("", window, cx);
                                    input.set_masked(true, window, cx);
                                    input.focus(window, cx);
                                });
                                cx.notify();
                            });
                            window.refresh();
                        }),
                )
                .into_any_element()
        } else {
            Input::new(&self.password_input)
                .mask_toggle()
                .into_any_element()
        };

        Some(
            render_form_row(
                t!("NewSession.Ssh.Field.Password").to_string(),
                div()
                    .w_full()
                    .debug_selector(|| "termua-new-session-ssh-password-input".to_string())
                    .child(password_control),
                cx,
            )
            .into_any_element(),
        )
    }

    fn render_config_auth_hint_row(
        &self,
        cx: &mut Context<NewSessionWindow>,
    ) -> Option<gpui::AnyElement> {
        if self.auth_type != SshAuthType::Config {
            return None;
        }

        Some(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(t!("NewSession.Ssh.AuthHint").to_string())
                .into_any_element(),
        )
    }

    fn render_common_rows(
        &self,
        view: Entity<NewSessionWindow>,
        cx: &mut Context<NewSessionWindow>,
    ) -> Vec<gpui::AnyElement> {
        let env_editor = render_env_editor(&self.env_rows, view, EnvEditorKind::Ssh);
        vec![
            render_form_row(
                t!("NewSession.Field.Label").to_string(),
                Input::new(&self.common.label_input),
                cx,
            )
            .into_any_element(),
            render_form_row(
                t!("NewSession.Field.Group").to_string(),
                div()
                    .w_full()
                    .debug_selector(|| "termua-new-session-ssh-group-input".to_string())
                    .child(Input::new(&self.common.group_input)),
                cx,
            )
            .into_any_element(),
            render_form_row(
                t!("NewSession.Field.EnvironmentVariables").to_string(),
                env_editor,
                cx,
            )
            .into_any_element(),
        ]
    }

    pub(super) fn render_session(
        &self,
        view: Entity<NewSessionWindow>,
        window: &mut Window,
        cx: &mut Context<NewSessionWindow>,
    ) -> impl IntoElement {
        let host_value = self.host_input.read(cx).value();
        let host_error = (self.auth_type == SshAuthType::Config && host_value.contains('@'))
            .then_some(t!("NewSession.Ssh.Error.HostNoUserAtInConfigMode").to_string());

        let port_value = self.port_input.read(cx).value();
        let port_error = (!ssh::ssh_port_is_valid(port_value.as_ref()))
            .then_some(t!("NewSession.Ssh.Error.PortRange").to_string());

        let mut rows = Vec::new();
        rows.push(render_backend_type_row(
            &self.common,
            "termua-new-session-ssh-backend-type-select",
            cx,
        ));
        rows.push(self.render_host_row(host_error, port_error, window, cx));
        rows.push(self.render_auth_type_row(cx));
        if let Some(row) = self.render_password_row(view.clone(), !self.password_edit_unlocked, cx)
        {
            rows.push(row);
        }
        if let Some(hint) = self.render_config_auth_hint_row(cx) {
            rows.push(hint);
        }
        rows.extend(self.render_common_rows(view, cx));

        v_flex()
            .id("termua-new-session-ssh-connection")
            .gap_3()
            .children(rows)
    }

    pub(super) fn render_connection(
        &self,
        view: Entity<NewSessionWindow>,
        _window: &mut Window,
        _cx: &mut Context<NewSessionWindow>,
    ) -> impl IntoElement {
        let view_tcp_nodelay = view.clone();
        let view_tcp_keepalive = view;

        v_flex()
            .id("termua-new-session-ssh-connection-settings")
            .gap_3()
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .debug_selector(|| "termua-new-session-ssh-tcp-nodelay".to_string())
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .text_sm()
                            .child(TextView::markdown(
                                "termua-new-session-ssh-tcp-nodelay-label",
                                t!("NewSession.Ssh.ConnectionSettings.TcpNoDelay").to_string(),
                            )),
                    )
                    .child(
                        h_flex().justify_end().child(
                            Switch::new("termua-new-session-ssh-tcp-nodelay")
                                .checked(self.tcp_nodelay)
                                .on_click(move |checked, window, app| {
                                    view_tcp_nodelay.update(app, |this, cx| {
                                        this.ssh.tcp_nodelay = *checked;
                                        cx.notify();
                                    });
                                    window.refresh();
                                }),
                        ),
                    ),
            )
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .debug_selector(|| "termua-new-session-ssh-tcp-keepalive".to_string())
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .text_sm()
                            .child(TextView::markdown(
                                "termua-new-session-ssh-tcp-keepalive-label",
                                t!("NewSession.Ssh.ConnectionSettings.TcpKeepalive").to_string(),
                            )),
                    )
                    .child(
                        h_flex().justify_end().child(
                            Switch::new("termua-new-session-ssh-tcp-keepalive")
                                .checked(self.tcp_keepalive)
                                .on_click(move |checked, window, app| {
                                    view_tcp_keepalive.update(app, |this, cx| {
                                        this.ssh.tcp_keepalive = *checked;
                                        cx.notify();
                                    });
                                    window.refresh();
                                }),
                        ),
                    ),
            )
    }

    pub(super) fn render_proxy(
        &self,
        view: Entity<NewSessionWindow>,
        _window: &mut Window,
        cx: &mut Context<NewSessionWindow>,
    ) -> impl IntoElement {
        let proxy_type = div()
            .w_full()
            .debug_selector(|| "termua-new-session-ssh-proxy-type".to_string())
            .child(Select::new(&self.proxy_select));

        let command = self.render_proxy_command_input();
        let workdir_for_command = self.render_proxy_workdir_input();
        let env_editor_for_command = self.render_proxy_env_editor(view.clone());

        let jump_editor = self.render_proxy_jump_editor(view.clone());
        let workdir_for_jumpserver = self.render_proxy_workdir_input();
        let env_editor_for_jumpserver = self.render_proxy_env_editor(view);

        v_flex()
            .id("termua-new-session-ssh-proxy")
            .gap_3()
            .child(render_form_row(
                t!("NewSession.Ssh.Proxy.Field.ProxyType").to_string(),
                proxy_type,
                cx,
            ))
            .when(
                self.proxy_mode == SshProxyMode::Command,
                |this: gpui::Stateful<gpui::Div>| {
                    this.child(render_form_row(
                        t!("NewSession.Ssh.Proxy.Field.Command").to_string(),
                        command,
                        cx,
                    ))
                    .child(render_form_row(
                        t!("NewSession.Ssh.Proxy.Field.WorkingDirectory").to_string(),
                        workdir_for_command,
                        cx,
                    ))
                    .child(render_form_row(
                        t!("NewSession.Ssh.Proxy.Field.EnvironmentVariables").to_string(),
                        env_editor_for_command,
                        cx,
                    ))
                },
            )
            .when(
                self.proxy_mode == SshProxyMode::JumpServer,
                |this: gpui::Stateful<gpui::Div>| {
                    this.child(render_form_row(
                        t!("NewSession.Ssh.Proxy.Field.JumpChain").to_string(),
                        jump_editor,
                        cx,
                    ))
                    .child(render_form_row(
                        t!("NewSession.Ssh.Proxy.Field.WorkingDirectory").to_string(),
                        workdir_for_jumpserver,
                        cx,
                    ))
                    .child(render_form_row(
                        t!("NewSession.Ssh.Proxy.Field.EnvironmentVariables").to_string(),
                        env_editor_for_jumpserver,
                        cx,
                    ))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(t!("NewSession.Ssh.Proxy.JumpServerNote").to_string()),
                    )
                },
            )
            .when(
                self.proxy_mode != SshProxyMode::Command
                    && self.proxy_mode != SshProxyMode::JumpServer,
                |this: gpui::Stateful<gpui::Div>| {
                    this.child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(t!("NewSession.Ssh.ProxyHint").to_string()),
                    )
                },
            )
    }
}

impl SerialSessionState {
    pub(super) fn render_session(
        &self,
        view: Entity<NewSessionWindow>,
        _window: &mut Window,
        cx: &mut Context<NewSessionWindow>,
    ) -> impl IntoElement {
        let env_editor = render_env_editor(&self.env_rows, view.clone(), EnvEditorKind::Serial);
        v_flex()
            .id("termua-new-session-serial-session")
            .gap_3()
            .child(render_backend_type_row(
                &self.common,
                "termua-new-session-serial-backend-type-select",
                cx,
            ))
            .child(render_form_row(
                t!("NewSession.Serial.Field.Port").to_string(),
                div()
                    .w_full()
                    .debug_selector(|| "termua-new-session-serial-port-select".to_string())
                    .child(
                        h_flex()
                            .w_full()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .child(Select::new(&self.port_select)),
                            )
                            .child(
                                Button::new("termua-new-session-serial-port-refresh")
                                    .icon(Icon::default().path(TermuaIcon::Refresh))
                                    .tooltip(if self.ports_loading {
                                        t!("NewSession.Serial.Tooltip.Refreshing").to_string()
                                    } else {
                                        t!("NewSession.Serial.Tooltip.RefreshPorts").to_string()
                                    })
                                    .ghost()
                                    .disabled(self.ports_loading)
                                    .on_click(move |_, window, app| {
                                        view.update(app, |this, cx| {
                                            this.serial.refresh_ports(window, cx);
                                        });
                                    }),
                            ),
                    ),
                cx,
            ))
            .child(render_form_row(
                t!("NewSession.Field.Label").to_string(),
                div().w_full().child(Input::new(&self.common.label_input)),
                cx,
            ))
            .child(render_form_row(
                t!("NewSession.Field.Group").to_string(),
                div()
                    .w_full()
                    .debug_selector(|| "termua-new-session-serial-group-input".to_string())
                    .child(Input::new(&self.common.group_input)),
                cx,
            ))
            .child(render_form_row(
                t!("NewSession.Field.EnvironmentVariables").to_string(),
                env_editor,
                cx,
            ))
    }

    pub(super) fn render_connection(
        &self,
        _view: Entity<NewSessionWindow>,
        _window: &mut Window,
        cx: &mut Context<NewSessionWindow>,
    ) -> impl IntoElement {
        v_flex()
            .id("termua-new-session-serial-connection")
            .gap_3()
            .child(render_form_row(
                t!("NewSession.Serial.Field.Baud").to_string(),
                div()
                    .w_full()
                    .debug_selector(|| "termua-new-session-serial-baud-input".to_string())
                    .child(Input::new(&self.baud_input)),
                cx,
            ))
    }

    pub(super) fn render_line_settings(
        &self,
        _view: Entity<NewSessionWindow>,
        _window: &mut Window,
        cx: &mut Context<NewSessionWindow>,
    ) -> impl IntoElement {
        v_flex()
            .id("termua-new-session-serial-line-settings")
            .gap_3()
            .child(render_form_row(
                t!("NewSession.Serial.Frame.DataBits").to_string(),
                div()
                    .w_full()
                    .debug_selector(|| "termua-new-session-serial-data-bits".to_string())
                    .child(Select::new(&self.data_bits_select)),
                cx,
            ))
            .child(render_form_row(
                t!("NewSession.Serial.Frame.Parity").to_string(),
                div()
                    .w_full()
                    .debug_selector(|| "termua-new-session-serial-parity".to_string())
                    .child(Select::new(&self.parity_select)),
                cx,
            ))
            .child(render_form_row(
                t!("NewSession.Serial.Frame.StopBits").to_string(),
                div()
                    .w_full()
                    .debug_selector(|| "termua-new-session-serial-stop-bits".to_string())
                    .child(Select::new(&self.stop_bits_select)),
                cx,
            ))
            .child(render_form_row(
                t!("NewSession.Serial.Frame.FlowControl").to_string(),
                div()
                    .w_full()
                    .debug_selector(|| "termua-new-session-serial-flow-control".to_string())
                    .child(Select::new(&self.flow_control_select)),
                cx,
            ))
    }
}
