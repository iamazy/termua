use std::sync::Arc;

use gpui::{
    App, AppContext, Context, InteractiveElement, IntoElement, ParentElement, Styled, Window, div,
    px,
};
use gpui_common::TermuaIcon;
use gpui_component::{
    Icon,
    button::{Button, ButtonVariants},
    dialog::DialogFooter,
    h_flex, v_flex,
};
use gpui_dock::{DockPlacement, PanelView};
use gpui_term::{Authentication, SshOptions, TerminalBuilder, TerminalType};
use rust_i18n::t;

use super::TermuaWindow;
use crate::{
    PendingCommand, SshParams, TermuaAppState,
    env::build_terminal_env,
    notification,
    panel::SshErrorPanel,
    ssh::{
        SshHostKeyMismatchDetails, dedupe_tab_label, default_known_hosts_path,
        parse_ssh_host_key_mismatch, remove_known_host_entry, ssh_connect_failure_message,
        ssh_proxy_from_session, ssh_tab_tooltip, ssh_target_label,
    },
};

impl TermuaWindow {
    fn open_ssh_host_verification_dialog(
        &mut self,
        opts: SshOptions,
        message: String,
        decision_tx: smol::channel::Sender<bool>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(Some(root)) = window.root::<gpui_component::Root>() else {
            log::warn!("termua: dialog requested but window root is not gpui_component::Root");
            let _ = decision_tx.try_send(false);
            return;
        };

        let target = ssh_target_label(&opts);

        root.update(cx, |root, cx| {
            root.open_dialog(
                move |dialog, _window, app| {
                    let decision_tx_ok = decision_tx.clone();
                    let decision_tx_cancel = decision_tx.clone();

                    dialog
                        .title(Self::ssh_host_verification_dialog_title(app))
                        .w(px(720.))
                        .child(Self::ssh_host_verification_dialog_body(&target, &message))
                        .button_props(
                            gpui_component::dialog::DialogButtonProps::default()
                                .ok_text(t!("SshHostVerify.Button.TrustContinue").to_string())
                                .cancel_text(t!("SshHostVerify.Button.Reject").to_string())
                                .show_cancel(true),
                        )
                        .on_ok(move |_, _window, _app| {
                            let _ = decision_tx_ok.try_send(true);
                            true
                        })
                        .on_cancel(move |_, _window, _app| {
                            let _ = decision_tx_cancel.try_send(false);
                            true
                        })
                },
                window,
                cx,
            );
        });
    }

    fn ssh_host_verification_dialog_title(app: &App) -> gpui::AnyElement {
        use gpui_component::ActiveTheme as _;

        h_flex()
            .gap_2()
            .items_center()
            .child(
                Icon::default()
                    .path(TermuaIcon::AlertCircle)
                    .text_color(app.theme().warning),
            )
            .child(t!("SshHostVerify.Title").to_string())
            .into_any_element()
    }

    fn ssh_host_verification_dialog_body(target: &str, message: &str) -> gpui::AnyElement {
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .items_start()
                    .child(div().child(t!("SshHostVerify.Label.Target").to_string()))
                    .child(
                        div().min_w_0().child(
                            gpui_component::text::TextView::markdown(
                                "termua-ssh-host-verify-text-target",
                                target.to_string(),
                            )
                            .selectable(true),
                        ),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_start()
                    .child(div().child(t!("SshHostVerify.Label.Message").to_string()))
                    .child(
                        div().min_w_0().child(
                            gpui_component::text::TextView::markdown(
                                "termua-ssh-host-verify-text-message",
                                message.to_string(),
                            )
                            .selectable(true),
                        ),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_start()
                    .child(div().child(t!("SshHostVerify.Label.Note").to_string()))
                    .child(
                        div().min_w_0().child(
                            gpui_component::text::TextView::markdown(
                                "termua-ssh-host-verify-text-note",
                                t!("SshHostVerify.NoteText").to_string(),
                            )
                            .selectable(true),
                        ),
                    ),
            )
            .into_any_element()
    }

    fn ssh_host_key_mismatch_dialog_title(app: &App) -> gpui::AnyElement {
        use gpui_component::ActiveTheme as _;

        h_flex()
            .gap_2()
            .items_center()
            .child(
                Icon::default()
                    .path(TermuaIcon::AlertCircle)
                    .text_color(app.theme().danger),
            )
            .child(t!("SshHostKeyMismatch.Title").to_string())
            .into_any_element()
    }

    fn ssh_host_key_mismatch_markdown_row(
        label_selector: &'static str,
        label: String,
        value_selector: &'static str,
        markdown_id: &'static str,
        markdown: String,
    ) -> gpui::AnyElement {
        h_flex()
            .gap_2()
            .items_start()
            .child(
                div()
                    .debug_selector(|| label_selector.to_string())
                    .child(label),
            )
            .child(
                div()
                    .min_w_0()
                    .debug_selector(|| value_selector.to_string())
                    .child(
                        gpui_component::text::TextView::markdown(markdown_id, markdown)
                            .selectable(true),
                    ),
            )
            .into_any_element()
    }

    fn ssh_host_key_mismatch_dialog_body(
        target: String,
        host: String,
        port: u16,
        reason: String,
        got_fingerprint: Option<String>,
        known_hosts_label: String,
        fix_cmd: Option<String>,
    ) -> gpui::AnyElement {
        let mut column = v_flex()
            .gap_2()
            .child(Self::ssh_host_key_mismatch_markdown_row(
                "termua-ssh-hostkey-mismatch-label-target",
                t!("SshHostKeyMismatch.Label.Target").to_string(),
                "termua-ssh-hostkey-mismatch-value-target",
                "termua-ssh-hostkey-mismatch-text-target",
                target,
            ))
            .child(Self::ssh_host_key_mismatch_markdown_row(
                "termua-ssh-hostkey-mismatch-label-server",
                t!("SshHostKeyMismatch.Label.Server").to_string(),
                "termua-ssh-hostkey-mismatch-value-server",
                "termua-ssh-hostkey-mismatch-text-server",
                format!("{host}:{port}"),
            ))
            .child(Self::ssh_host_key_mismatch_markdown_row(
                "termua-ssh-hostkey-mismatch-label-reason",
                t!("SshHostKeyMismatch.Label.Reason").to_string(),
                "termua-ssh-hostkey-mismatch-value-reason",
                "termua-ssh-hostkey-mismatch-text-reason",
                reason,
            ));

        if let Some(fp) = got_fingerprint {
            column = column.child(Self::ssh_host_key_mismatch_markdown_row(
                "termua-ssh-hostkey-mismatch-label-fingerprint",
                t!("SshHostKeyMismatch.Label.GotFingerprint").to_string(),
                "termua-ssh-hostkey-mismatch-value-fingerprint",
                "termua-ssh-hostkey-mismatch-text-fingerprint",
                fp,
            ));
        }

        column = column.child(Self::ssh_host_key_mismatch_markdown_row(
            "termua-ssh-hostkey-mismatch-label-known-hosts",
            t!("SshHostKeyMismatch.Label.KnownHosts").to_string(),
            "termua-ssh-hostkey-mismatch-value-known-hosts",
            "termua-ssh-hostkey-mismatch-text-known-hosts",
            known_hosts_label,
        ));

        if let Some(cmd) = fix_cmd {
            column = column.child(Self::ssh_host_key_mismatch_markdown_row(
                "termua-ssh-hostkey-mismatch-label-manual-fix",
                t!("SshHostKeyMismatch.Label.ManualFix").to_string(),
                "termua-ssh-hostkey-mismatch-value-manual-fix",
                "termua-ssh-hostkey-mismatch-text-manual-fix",
                cmd,
            ));
        }

        column
            .child(Self::ssh_host_key_mismatch_markdown_row(
                "termua-ssh-hostkey-mismatch-label-note",
                t!("SshHostKeyMismatch.Label.Note").to_string(),
                "termua-ssh-hostkey-mismatch-value-note",
                "termua-ssh-hostkey-mismatch-text-note",
                t!("SshHostKeyMismatch.NoteText").to_string(),
            ))
            .into_any_element()
    }

    fn close_dialog(window: &mut Window, app: &mut App) {
        gpui_component::Root::update(window, app, |root, window, cx| {
            root.close_dialog(window, cx);
        });
    }

    fn queue_open_ssh_terminal(backend_type: TerminalType, params: SshParams, app: &mut App) {
        app.global_mut::<TermuaAppState>()
            .pending_commands
            .push(PendingCommand::OpenSshTerminal {
                backend_type,
                params,
            });
        app.refresh_windows();
    }

    fn ssh_host_key_mismatch_dialog_footer(
        backend_type: TerminalType,
        params: SshParams,
        known_hosts_path: Option<std::path::PathBuf>,
        host: String,
        port: u16,
    ) -> DialogFooter {
        let retry_params = params.clone();
        let retry_button = Button::new("termua-ssh-hostkey-mismatch-retry")
            .label(t!("SshHostKeyMismatch.Button.Retry").to_string())
            .on_click(move |_, window, app| {
                Self::close_dialog(window, app);
                Self::queue_open_ssh_terminal(backend_type, retry_params.clone(), app);
            });

        let remove_params = params;
        let remove_and_retry_button = Button::new("termua-ssh-hostkey-mismatch-remove-retry")
            .label(t!("SshHostKeyMismatch.Button.RemoveRetry").to_string())
            .primary()
            .on_click(move |_, window, app| {
                let Some(path) = known_hosts_path.as_ref() else {
                    notification::notify_app(
                        notification::MessageKind::Error,
                        t!("SshHostKeyMismatch.Error.MissingKnownHostsPath").to_string(),
                        window,
                        app,
                    );
                    return;
                };

                let summary = match remove_known_host_entry(path, &host, port) {
                    Ok(summary) => summary,
                    Err(err) => {
                        notification::notify_app(
                            notification::MessageKind::Error,
                            t!(
                                "SshHostKeyMismatch.Error.FailedUpdateKnownHosts",
                                err = format!("{err:#}")
                            )
                            .to_string(),
                            window,
                            app,
                        );
                        return;
                    }
                };

                Self::close_dialog(window, app);

                notification::notify_app(
                    notification::MessageKind::Info,
                    if summary.is_empty() {
                        format!("Updated {}", path.display())
                    } else {
                        summary
                    },
                    window,
                    app,
                );

                Self::queue_open_ssh_terminal(backend_type, remove_params.clone(), app);
            });

        let cancel_button = Button::new("termua-ssh-hostkey-mismatch-cancel")
            .label("Cancel")
            .on_click(|_, window, app| {
                Self::close_dialog(window, app);
            });

        DialogFooter::new()
            .child(cancel_button)
            .child(retry_button)
            .child(remove_and_retry_button)
    }

    pub(crate) fn open_ssh_host_key_mismatch_dialog(
        &mut self,
        backend_type: TerminalType,
        params: SshParams,
        reason: String,
        details: SshHostKeyMismatchDetails,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(Some(root)) = window.root::<gpui_component::Root>() else {
            log::warn!("termua: dialog requested but window root is not gpui_component::Root");
            return;
        };

        let target = ssh_target_label(&params.opts);
        let default_host = params.opts.host.trim().to_string();
        let default_port = params.opts.port.unwrap_or(22);
        let host = details
            .server_host
            .clone()
            .unwrap_or_else(|| default_host.clone());
        let port = details.server_port.unwrap_or(default_port);

        let known_hosts_path = details
            .known_hosts_path
            .clone()
            .or_else(default_known_hosts_path);

        let known_hosts_label = known_hosts_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "~/.ssh/known_hosts".to_string());

        let fix_cmd = known_hosts_path.as_ref().map(|p| {
            if port == 22 {
                format!("ssh-keygen -R \"{host}\" -f \"{}\"", p.display())
            } else {
                format!("ssh-keygen -R \"[{host}]:{port}\" -f \"{}\"", p.display())
            }
        });

        root.update(cx, |root, cx| {
            root.open_dialog(
                move |dialog, _window, app| {
                    let known_hosts_path_for_footer = known_hosts_path.clone();
                    let host_for_footer = host.clone();
                    let port_for_footer = port;
                    let params_for_footer = params.clone();

                    dialog
                        .title(Self::ssh_host_key_mismatch_dialog_title(app))
                        .w(px(720.))
                        .child(Self::ssh_host_key_mismatch_dialog_body(
                            target.clone(),
                            host.clone(),
                            port,
                            reason.clone(),
                            details.got_fingerprint.clone(),
                            known_hosts_label.clone(),
                            fix_cmd.clone(),
                        ))
                        .footer(Self::ssh_host_key_mismatch_dialog_footer(
                            backend_type,
                            params_for_footer,
                            known_hosts_path_for_footer,
                            host_for_footer,
                            port_for_footer,
                        ))
                },
                window,
                cx,
            );
        });
    }

    pub(crate) fn add_ssh_terminal_with_params(
        &mut self,
        backend_type: TerminalType,
        params: SshParams,
        session_id: Option<i64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Building the SSH PTY involves a blocking login handshake. Run that work in a background
        // thread and only attach the terminal panel on success.
        let builder_fn = self.ssh_terminal_builder.clone();
        let env_for_thread = params.env.clone();
        let opts_for_thread = params.opts.clone();
        let params_for_finish = params.clone();
        let opts_for_prompt = params.opts;
        let background = cx.background_executor().clone();

        let (verify_tx, verify_rx) =
            smol::channel::unbounded::<gpui_term::SshHostVerificationPrompt>();
        // Keep a sender alive on the UI thread so closing the prompt channel can't happen on the
        // background thread.
        let verify_tx_keepalive = verify_tx.clone();

        // Handle host verification prompts while the background handshake is running.
        cx.spawn_in(window, async move |view, window| {
            while let Ok(req) = verify_rx.recv().await {
                let (decision_tx, decision_rx) = smol::channel::bounded::<bool>(1);
                let message = req.message;
                let _ = view.update_in(window, |this, window, cx| {
                    this.open_ssh_host_verification_dialog(
                        opts_for_prompt.clone(),
                        message,
                        decision_tx.clone(),
                        window,
                        cx,
                    );
                });

                let decision = decision_rx.recv().await.unwrap_or(false);
                let _ = req.reply.send(decision).await;
            }
        })
        .detach();

        cx.spawn_in(window, async move |view, window| {
            let session_id = session_id;
            let verify_tx_for_task = verify_tx.clone();
            let task = background.spawn(async move {
                // Route SSH host verification prompts (unknown host keys) back to the UI thread.
                // If no UI consumes them, gpui_term will time out and treat the host as untrusted.
                let _guard = gpui_term::set_thread_ssh_host_verification_prompt_sender(Some(
                    verify_tx_for_task,
                ));
                (builder_fn)(backend_type, env_for_thread, opts_for_thread)
            });

            let result = task.await;

            let _ = view.update_in(window, |this, window, cx| {
                this.finish_add_ssh_terminal_task(
                    result,
                    backend_type,
                    params_for_finish,
                    session_id,
                    window,
                    cx,
                );
            });

            // Keep the sender alive on the UI thread until the handshake completes.
            drop(verify_tx_keepalive);
        })
        .detach();
    }

    fn finish_add_ssh_terminal_task(
        &mut self,
        result: anyhow::Result<TerminalBuilder>,
        backend_type: TerminalType,
        params: SshParams,
        session_id: Option<i64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(builder) => {
                let panel = self.build_ssh_panel_from_builder(
                    builder,
                    params.name,
                    params.opts,
                    window,
                    cx,
                );
                self.dock_area.update(cx, |dock, cx| {
                    dock.add_panel(
                        Arc::new(panel) as Arc<dyn PanelView>,
                        DockPlacement::Center,
                        None,
                        window,
                        cx,
                    );
                });
                self.clear_connecting_session(session_id, cx);
                cx.notify();
            }
            Err(err) => {
                let root_reason = err.root_cause().to_string();
                if let Some(details) = parse_ssh_host_key_mismatch(&root_reason) {
                    self.open_ssh_host_key_mismatch_dialog(
                        backend_type,
                        params,
                        root_reason,
                        details,
                        window,
                        cx,
                    );
                    self.clear_connecting_session(session_id, cx);
                    return;
                }
                let id = self.next_terminal_id;
                self.next_terminal_id += 1;

                let tab_label =
                    dedupe_tab_label(&mut self.ssh_tab_label_counts, params.name.as_str());
                let tab_tooltip = ssh_tab_tooltip(&params.opts);
                let message = ssh_connect_failure_message(&params.opts, &err);

                let panel = cx.new(|cx| {
                    SshErrorPanel::new(id, tab_label, Some(tab_tooltip), message.into(), cx)
                });

                self.dock_area.update(cx, |dock, cx| {
                    dock.add_panel(
                        Arc::new(panel) as Arc<dyn PanelView>,
                        DockPlacement::Center,
                        None,
                        window,
                        cx,
                    );
                });
                self.clear_connecting_session(session_id, cx);
                cx.notify();
            }
        }
    }

    fn clear_connecting_session(&mut self, session_id: Option<i64>, cx: &mut Context<Self>) {
        let Some(session_id) = session_id else {
            return;
        };
        self.sessions_sidebar.update(cx, |sidebar, cx| {
            sidebar.set_connecting(session_id, false, cx);
        });
    }

    pub(super) fn open_saved_ssh_session(
        &mut self,
        backend_type: TerminalType,
        session: crate::store::Session,
        session_id: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(host) = session.ssh_host.as_deref() else {
            return;
        };
        let port = session.ssh_port.unwrap_or(22);

        let session_env = session.env.clone().unwrap_or_default();
        let env = build_terminal_env(
            "",
            session.term(),
            session.colorterm(),
            session.charset(),
            &session_env,
        );
        let proxy = ssh_proxy_from_session(&session);
        let name = session.label;

        let auth = match session.ssh_auth_type {
            Some(crate::store::SshAuthType::Config) => Authentication::Config,
            Some(crate::store::SshAuthType::Password) => {
                let user = session.ssh_user.unwrap_or_else(|| "root".to_string());
                let password = session.ssh_password.unwrap_or_default();
                if password.trim().is_empty() {
                    notification::notify_deferred(
                        notification::MessageKind::Error,
                        "Missing saved SSH password for this session.",
                        window,
                        cx,
                    );
                    return;
                }
                Authentication::Password(user, password)
            }
            None => {
                // Back-compat: default to config auth if not recorded.
                Authentication::Config
            }
        };

        let opts = SshOptions {
            host: host.to_string(),
            port: Some(port),
            auth,
            proxy,
            backend: cx
                .try_global::<crate::settings::SshBackendPreference>()
                .map(|pref| pref.backend)
                .unwrap_or_default(),
            tcp_nodelay: session.ssh_tcp_nodelay,
            tcp_keepalive: session.ssh_tcp_keepalive,
        };
        self.add_ssh_terminal_with_params(
            backend_type,
            SshParams { env, name, opts },
            Some(session_id),
            window,
            cx,
        );
    }
}
