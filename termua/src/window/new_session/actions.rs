use gpui::{Context, Window};
use gpui_term::{Authentication, SshOptions, TerminalType};

use super::{
    NewSessionWindow, Protocol, SshAuthType, TermBackend, new_proxy_env_row_state,
    new_proxy_jump_row_state, set_input_value, ssh,
};
use crate::{SerialParams, SshParams, env::build_local_terminal_env, store::SshProxyMode};

struct SshFormValues {
    backend: TermBackend,
    auth_type: SshAuthType,
    user_raw: String,
    host_raw: String,
    port_raw: String,
    password: String,
    tcp_nodelay: bool,
    tcp_keepalive: bool,
    term: gpui::SharedString,
    charset: gpui::SharedString,
    label: String,
    group: String,
}

enum SessionStoreOp {
    SaveLocal {
        group: String,
        label: String,
        backend: crate::settings::TerminalBackend,
        shell_program: String,
        term: String,
        charset: String,
    },
    UpdateLocal {
        session_id: i64,
        group: String,
        label: String,
        backend: crate::settings::TerminalBackend,
        shell_program: String,
        term: String,
        charset: String,
    },
    SaveSshPassword {
        group: String,
        label: String,
        backend: crate::settings::TerminalBackend,
        host: String,
        port: u16,
        user: String,
        password: String,
        term: String,
        charset: String,
        tcp_nodelay: bool,
        tcp_keepalive: bool,
        proxy_mode: SshProxyMode,
        proxy_command: Option<String>,
        proxy_workdir: Option<String>,
        proxy_env: Vec<crate::store::SshProxyEnvVar>,
        proxy_jump: Vec<crate::store::SshJumpHop>,
    },
    UpdateSshPassword {
        session_id: i64,
        group: String,
        label: String,
        backend: crate::settings::TerminalBackend,
        host: String,
        port: u16,
        user: String,
        password: String,
        term: String,
        charset: String,
        tcp_nodelay: bool,
        tcp_keepalive: bool,
        proxy_mode: SshProxyMode,
        proxy_command: Option<String>,
        proxy_workdir: Option<String>,
        proxy_env: Vec<crate::store::SshProxyEnvVar>,
        proxy_jump: Vec<crate::store::SshJumpHop>,
    },
    SaveSshConfig {
        group: String,
        label: String,
        backend: crate::settings::TerminalBackend,
        host: String,
        port: u16,
        term: String,
        charset: String,
        tcp_nodelay: bool,
        tcp_keepalive: bool,
        proxy_mode: SshProxyMode,
        proxy_command: Option<String>,
        proxy_workdir: Option<String>,
        proxy_env: Vec<crate::store::SshProxyEnvVar>,
        proxy_jump: Vec<crate::store::SshJumpHop>,
    },
    UpdateSshConfig {
        session_id: i64,
        group: String,
        label: String,
        backend: crate::settings::TerminalBackend,
        host: String,
        port: u16,
        term: String,
        charset: String,
        tcp_nodelay: bool,
        tcp_keepalive: bool,
        proxy_mode: SshProxyMode,
        proxy_command: Option<String>,
        proxy_workdir: Option<String>,
        proxy_env: Vec<crate::store::SshProxyEnvVar>,
        proxy_jump: Vec<crate::store::SshJumpHop>,
    },
    SaveSerial {
        group: String,
        label: String,
        backend: crate::settings::TerminalBackend,
        port: String,
        baud: u32,
        data_bits: u8,
        parity: crate::store::SerialParity,
        stop_bits: crate::store::SerialStopBits,
        flow_control: crate::store::SerialFlowControl,
        term: String,
        charset: String,
    },
    UpdateSerial {
        session_id: i64,
        group: String,
        label: String,
        backend: crate::settings::TerminalBackend,
        port: String,
        baud: u32,
        data_bits: u8,
        parity: crate::store::SerialParity,
        stop_bits: crate::store::SerialStopBits,
        flow_control: crate::store::SerialFlowControl,
        term: String,
        charset: String,
    },
}

impl SessionStoreOp {
    fn run(self) -> anyhow::Result<()> {
        match self {
            Self::SaveLocal {
                group,
                label,
                backend,
                shell_program,
                term,
                charset,
            } => {
                crate::store::save_local_session(
                    group.as_str(),
                    label.as_str(),
                    backend,
                    shell_program.as_str(),
                    term.as_str(),
                    charset.as_str(),
                )?;
            }
            Self::UpdateLocal {
                session_id,
                group,
                label,
                backend,
                shell_program,
                term,
                charset,
            } => {
                crate::store::update_local_session(
                    session_id,
                    group.as_str(),
                    label.as_str(),
                    backend,
                    shell_program.as_str(),
                    term.as_str(),
                    charset.as_str(),
                )?;
            }
            Self::SaveSshPassword {
                group,
                label,
                backend,
                host,
                port,
                user,
                password,
                term,
                charset,
                tcp_nodelay,
                tcp_keepalive,
                proxy_mode,
                proxy_command,
                proxy_workdir,
                proxy_env,
                proxy_jump,
            } => {
                crate::store::save_ssh_session_password_with_proxy(
                    group.as_str(),
                    label.as_str(),
                    backend,
                    host.as_str(),
                    port,
                    user.as_str(),
                    password.as_str(),
                    term.as_str(),
                    charset.as_str(),
                    tcp_nodelay,
                    tcp_keepalive,
                    proxy_mode,
                    proxy_command.as_deref(),
                    proxy_workdir.as_deref(),
                    proxy_env,
                    proxy_jump,
                )?;
            }
            Self::UpdateSshPassword {
                session_id,
                group,
                label,
                backend,
                host,
                port,
                user,
                password,
                term,
                charset,
                tcp_nodelay,
                tcp_keepalive,
                proxy_mode,
                proxy_command,
                proxy_workdir,
                proxy_env,
                proxy_jump,
            } => {
                crate::store::update_ssh_session_password_with_proxy(
                    session_id,
                    group.as_str(),
                    label.as_str(),
                    backend,
                    host.as_str(),
                    port,
                    user.as_str(),
                    password.as_str(),
                    term.as_str(),
                    charset.as_str(),
                    tcp_nodelay,
                    tcp_keepalive,
                    proxy_mode,
                    proxy_command.as_deref(),
                    proxy_workdir.as_deref(),
                    proxy_env,
                    proxy_jump,
                )?;
            }
            Self::SaveSshConfig {
                group,
                label,
                backend,
                host,
                port,
                term,
                charset,
                tcp_nodelay,
                tcp_keepalive,
                proxy_mode,
                proxy_command,
                proxy_workdir,
                proxy_env,
                proxy_jump,
            } => {
                crate::store::save_ssh_session_config_with_proxy(
                    group.as_str(),
                    label.as_str(),
                    backend,
                    host.as_str(),
                    port,
                    term.as_str(),
                    charset.as_str(),
                    tcp_nodelay,
                    tcp_keepalive,
                    proxy_mode,
                    proxy_command.as_deref(),
                    proxy_workdir.as_deref(),
                    proxy_env,
                    proxy_jump,
                )?;
            }
            Self::UpdateSshConfig {
                session_id,
                group,
                label,
                backend,
                host,
                port,
                term,
                charset,
                tcp_nodelay,
                tcp_keepalive,
                proxy_mode,
                proxy_command,
                proxy_workdir,
                proxy_env,
                proxy_jump,
            } => {
                crate::store::update_ssh_session_config_with_proxy(
                    session_id,
                    group.as_str(),
                    label.as_str(),
                    backend,
                    host.as_str(),
                    port,
                    term.as_str(),
                    charset.as_str(),
                    tcp_nodelay,
                    tcp_keepalive,
                    proxy_mode,
                    proxy_command.as_deref(),
                    proxy_workdir.as_deref(),
                    proxy_env,
                    proxy_jump,
                )?;
            }
            Self::SaveSerial {
                group,
                label,
                backend,
                port,
                baud,
                data_bits,
                parity,
                stop_bits,
                flow_control,
                term,
                charset,
            } => {
                crate::store::save_serial_session(
                    group.as_str(),
                    label.as_str(),
                    backend,
                    port.as_str(),
                    baud,
                    data_bits,
                    parity,
                    stop_bits,
                    flow_control,
                    term.as_str(),
                    charset.as_str(),
                )?;
            }
            Self::UpdateSerial {
                session_id,
                group,
                label,
                backend,
                port,
                baud,
                data_bits,
                parity,
                stop_bits,
                flow_control,
                term,
                charset,
            } => {
                crate::store::update_serial_session(
                    session_id,
                    group.as_str(),
                    label.as_str(),
                    backend,
                    port.as_str(),
                    baud,
                    data_bits,
                    parity,
                    stop_bits,
                    flow_control,
                    term.as_str(),
                    charset.as_str(),
                )?;
            }
        }

        Ok(())
    }
}

impl NewSessionWindow {
    fn read_ssh_form_values(&self, cx: &Context<Self>) -> SshFormValues {
        SshFormValues {
            backend: self.ssh.common.ty,
            auth_type: self.ssh.auth_type,
            user_raw: self.ssh.user_input.read(cx).value().to_string(),
            host_raw: self.ssh.host_input.read(cx).value().to_string(),
            port_raw: self.ssh.port_input.read(cx).value().to_string(),
            password: self.ssh.password_input.read(cx).value().to_string(),
            tcp_nodelay: self.ssh.tcp_nodelay,
            tcp_keepalive: self.ssh.tcp_keepalive,
            term: self.ssh.common.term.clone(),
            charset: self.ssh.common.charset.clone(),
            label: self.ssh.common.label_input.read(cx).value().to_string(),
            group: self.ssh.common.group_input.read(cx).value().to_string(),
        }
    }

    fn backend_for_store(backend: TermBackend) -> crate::settings::TerminalBackend {
        match backend {
            TermBackend::Alacritty => crate::settings::TerminalBackend::Alacritty,
            TermBackend::Wezterm => crate::settings::TerminalBackend::Wezterm,
        }
    }

    fn backend_for_terminal_type(backend: TermBackend) -> TerminalType {
        match backend {
            TermBackend::Alacritty => TerminalType::Alacritty,
            TermBackend::Wezterm => TerminalType::WezTerm,
        }
    }

    fn trimmed_or_value(raw: &str, default: String) -> String {
        let raw = raw.trim();
        if raw.is_empty() {
            default
        } else {
            raw.to_string()
        }
    }

    fn ssh_auth_and_host_from_inputs(
        auth_type: SshAuthType,
        user_raw: &str,
        host_trimmed: &str,
        password: &str,
    ) -> anyhow::Result<(String, Authentication)> {
        match auth_type {
            SshAuthType::Password => {
                let Ok((host, auth)) =
                    ssh::ssh_password_auth_from_inputs(user_raw, host_trimmed, password)
                else {
                    return Err(anyhow::anyhow!("Invalid SSH password credentials."));
                };
                Ok((host, auth))
            }
            SshAuthType::Config => {
                if ssh::ssh_host_is_valid_for_config_auth(host_trimmed).is_err() {
                    return Err(anyhow::anyhow!(
                        "Host must not include user@ in SSH Config mode."
                    ));
                }
                Ok((host_trimmed.to_string(), Authentication::Config))
            }
        }
    }

    pub(super) fn unlock_from_overlay(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.lock_overlay.unlock_with_password(window, cx);
    }

    pub(super) fn save_edit_session(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        let Some(session_id) = self.mode.session_id() else {
            return Ok(());
        };
        if self.submit_in_flight {
            return Ok(());
        }

        let op = match self.protocol {
            Protocol::Shell => self.save_shell_edit_session(session_id, cx)?,
            Protocol::Ssh => self.save_ssh_edit_session(session_id, cx)?,
            Protocol::Serial => self.save_serial_edit_session(session_id, cx)?,
        };
        self.submit_in_flight = true;
        cx.notify();

        let background = cx.background_executor().clone();
        cx.spawn_in(window, async move |this, window| {
            let result = background.spawn(async move { op.run() }).await;
            let _ = this.update_in(window, |this, window, cx| match result {
                Ok(()) => {
                    cx.global_mut::<crate::TermuaAppState>()
                        .pending_command(crate::PendingCommand::ReloadSessionsSidebar);
                    cx.refresh_windows();
                    window.remove_window();
                }
                Err(err) => {
                    this.submit_in_flight = false;
                    cx.notify();
                    crate::notification::notify(
                        crate::notification::MessageKind::Info,
                        format!("Failed to save session: {err:#}"),
                        window,
                        cx,
                    );
                }
            });
        })
        .detach();

        Ok(())
    }

    fn save_shell_edit_session(
        &self,
        session_id: i64,
        cx: &Context<Self>,
    ) -> anyhow::Result<SessionStoreOp> {
        let (backend, shell_program, term, charset, label, group) = (
            self.shell.common.ty,
            self.shell.program.clone(),
            self.shell.common.term.clone(),
            self.shell.common.charset.clone(),
            self.shell.common.label_input.read(cx).value().to_string(),
            self.shell.common.group_input.read(cx).value().to_string(),
        );

        let group = {
            let group = group.trim();
            if group.is_empty() {
                "local".to_string()
            } else {
                group.to_string()
            }
        };
        let label = {
            let label = label.trim();
            if label.is_empty() {
                shell_program.to_string()
            } else {
                label.to_string()
            }
        };

        let backend_for_store = match backend {
            TermBackend::Alacritty => crate::settings::TerminalBackend::Alacritty,
            TermBackend::Wezterm => crate::settings::TerminalBackend::Wezterm,
        };

        Ok(SessionStoreOp::UpdateLocal {
            session_id,
            group,
            label,
            backend: backend_for_store,
            shell_program: shell_program.to_string(),
            term: term.to_string(),
            charset: charset.to_string(),
        })
    }

    fn save_ssh_edit_session(
        &self,
        session_id: i64,
        cx: &Context<Self>,
    ) -> anyhow::Result<SessionStoreOp> {
        let values = self.read_ssh_form_values(cx);
        let backend_for_store = Self::backend_for_store(values.backend);
        let group = Self::trimmed_or_value(values.group.as_str(), "ssh".to_string());

        let host_trimmed = values.host_raw.trim();
        if host_trimmed.is_empty() {
            return Err(anyhow::anyhow!("Host is required."));
        }

        let Ok(port) = values.port_raw.trim().parse::<u16>() else {
            return Err(anyhow::anyhow!("Invalid port."));
        };

        let name = Self::trimmed_or_value(values.label.as_str(), host_trimmed.to_string());

        let (proxy_mode, proxy_command, proxy_workdir, proxy_env, proxy_jump) =
            self.ssh.proxy_settings_for_store(cx);

        match values.auth_type {
            SshAuthType::Password => {
                let Ok((_host, _auth)) = ssh::ssh_password_auth_from_inputs(
                    values.user_raw.as_ref(),
                    host_trimmed,
                    values.password.as_ref(),
                ) else {
                    return Err(anyhow::anyhow!("Invalid SSH password credentials."));
                };

                let user = values.user_raw.trim();
                let user = if user.is_empty() { "root" } else { user };
                let pw = values.password.trim();
                if pw.is_empty() {
                    return Err(anyhow::anyhow!("Password is required."));
                }

                Ok(SessionStoreOp::UpdateSshPassword {
                    session_id,
                    group,
                    label: name,
                    backend: backend_for_store,
                    host: host_trimmed.to_string(),
                    port,
                    user: user.to_string(),
                    password: pw.to_string(),
                    term: values.term.to_string(),
                    charset: values.charset.to_string(),
                    tcp_nodelay: values.tcp_nodelay,
                    tcp_keepalive: values.tcp_keepalive,
                    proxy_mode,
                    proxy_command,
                    proxy_workdir,
                    proxy_env,
                    proxy_jump,
                })
            }
            SshAuthType::Config => {
                if ssh::ssh_host_is_valid_for_config_auth(host_trimmed).is_err() {
                    return Err(anyhow::anyhow!(
                        "Host must not include user@ in SSH Config mode."
                    ));
                }

                Ok(SessionStoreOp::UpdateSshConfig {
                    session_id,
                    group,
                    label: name,
                    backend: backend_for_store,
                    host: host_trimmed.to_string(),
                    port,
                    term: values.term.to_string(),
                    charset: values.charset.to_string(),
                    tcp_nodelay: values.tcp_nodelay,
                    tcp_keepalive: values.tcp_keepalive,
                    proxy_mode,
                    proxy_command,
                    proxy_workdir,
                    proxy_env,
                    proxy_jump,
                })
            }
        }
    }

    pub(super) fn connect_new_session(&mut self, cx: &mut Context<Self>) -> anyhow::Result<()> {
        if self.submit_in_flight {
            return Ok(());
        }

        match self.protocol {
            Protocol::Shell => self.connect_new_local_shell(cx),
            Protocol::Ssh => self.connect_new_ssh(cx),
            Protocol::Serial => self.connect_new_serial(cx),
        }
    }

    fn connect_new_local_shell(&mut self, cx: &mut Context<Self>) -> anyhow::Result<()> {
        let (backend, shell_program, term, charset) = (
            self.shell.common.ty,
            self.shell.program.clone(),
            self.shell.common.term.clone(),
            self.shell.common.charset.clone(),
        );

        let backend_type = match backend {
            TermBackend::Alacritty => TerminalType::Alacritty,
            TermBackend::Wezterm => TerminalType::WezTerm,
        };
        let env = build_local_terminal_env(shell_program.as_ref(), term.as_ref(), charset.as_ref());

        // Best-effort persistence.
        let group = self.shell.common.group_input.read(cx).value().to_string();
        let group = {
            let group = group.trim();
            if group.is_empty() {
                "local".to_string()
            } else {
                group.to_string()
            }
        };
        let label = self.shell.common.label_input.read(cx).value().to_string();
        let label = {
            let label = label.trim();
            if label.is_empty() {
                shell_program.as_ref().trim().to_string()
            } else {
                label.to_string()
            }
        };

        let backend_for_store = match backend {
            TermBackend::Alacritty => crate::settings::TerminalBackend::Alacritty,
            TermBackend::Wezterm => crate::settings::TerminalBackend::Wezterm,
        };

        if cx.global::<crate::TermuaAppState>().main_window.is_none() {
            return Err(anyhow::anyhow!("Main window not ready yet."));
        };
        self.submit_in_flight = true;
        cx.notify();

        self.spawn_store_op_detached(
            SessionStoreOp::SaveLocal {
                group,
                label,
                backend: backend_for_store,
                shell_program: shell_program.to_string(),
                term: term.to_string(),
                charset: charset.to_string(),
            },
            "persist local session",
            cx,
        );

        cx.global_mut::<crate::TermuaAppState>()
            .pending_command(crate::PendingCommand::OpenLocalTerminal { backend_type, env });
        cx.refresh_windows();

        Ok(())
    }

    fn connect_new_ssh(&mut self, cx: &mut Context<Self>) -> anyhow::Result<()> {
        let values = self.read_ssh_form_values(cx);
        let backend_type = Self::backend_for_terminal_type(values.backend);

        let Ok(port) = values.port_raw.trim().parse::<u16>() else {
            return Err(anyhow::anyhow!("Invalid port."));
        };

        let host_trimmed = values.host_raw.trim();
        if host_trimmed.is_empty() {
            return Err(anyhow::anyhow!("Host is required."));
        }

        let env = build_local_terminal_env("", values.term.as_ref(), values.charset.as_ref());
        let (host, auth) = Self::ssh_auth_and_host_from_inputs(
            values.auth_type,
            values.user_raw.as_str(),
            host_trimmed,
            values.password.as_str(),
        )?;

        let name = Self::trimmed_or_value(values.label.as_str(), host.clone());
        let group = Self::trimmed_or_value(values.group.as_str(), "ssh".to_string());
        let backend_for_store = Self::backend_for_store(values.backend);

        let (proxy_mode, proxy_command, proxy_workdir, proxy_env, proxy_jump) =
            self.ssh.proxy_settings_for_store(cx);
        let proxy_for_opts = self.ssh.proxy_settings_for_opts(cx);

        let persist_op = match &auth {
            Authentication::Password(user, pw) => SessionStoreOp::SaveSshPassword {
                group: group.clone(),
                label: name.clone(),
                backend: backend_for_store,
                host: host.clone(),
                port,
                user: user.clone(),
                password: pw.clone(),
                term: values.term.to_string(),
                charset: values.charset.to_string(),
                tcp_nodelay: values.tcp_nodelay,
                tcp_keepalive: values.tcp_keepalive,
                proxy_mode,
                proxy_command,
                proxy_workdir,
                proxy_env,
                proxy_jump,
            },
            Authentication::Config => SessionStoreOp::SaveSshConfig {
                group: group.clone(),
                label: name.clone(),
                backend: backend_for_store,
                host: host.clone(),
                port,
                term: values.term.to_string(),
                charset: values.charset.to_string(),
                tcp_nodelay: values.tcp_nodelay,
                tcp_keepalive: values.tcp_keepalive,
                proxy_mode,
                proxy_command,
                proxy_workdir,
                proxy_env,
                proxy_jump,
            },
        };

        let opts = SshOptions {
            host,
            port: Some(port),
            auth,
            proxy: proxy_for_opts,
            backend: cx
                .try_global::<crate::settings::SshBackendPreference>()
                .map(|pref| pref.backend)
                .unwrap_or_default(),
            tcp_nodelay: values.tcp_nodelay,
            tcp_keepalive: values.tcp_keepalive,
        };

        if cx.global::<crate::TermuaAppState>().main_window.is_none() {
            return Err(anyhow::anyhow!("Main window not ready yet."));
        };
        self.submit_in_flight = true;
        cx.notify();

        self.spawn_store_op_detached(persist_op, "persist ssh session", cx);

        cx.global_mut::<crate::TermuaAppState>().pending_command(
            crate::PendingCommand::OpenSshTerminal {
                backend_type,
                params: SshParams { env, name, opts },
            },
        );
        cx.refresh_windows();

        Ok(())
    }

    fn save_serial_edit_session(
        &self,
        session_id: i64,
        cx: &Context<Self>,
    ) -> anyhow::Result<SessionStoreOp> {
        let (
            backend,
            port,
            baud_raw,
            data_bits,
            parity,
            stop_bits,
            flow_control,
            term,
            charset,
            label,
            group,
        ) = (
            self.serial.common.ty,
            self.serial.selected_port(cx).unwrap_or_default(),
            self.serial.baud_input.read(cx).value().to_string(),
            self.serial.selected_data_bits(cx),
            self.serial.selected_parity(cx),
            self.serial.selected_stop_bits(cx),
            self.serial.selected_flow_control(cx),
            self.serial.common.term.clone(),
            self.serial.common.charset.clone(),
            self.serial.common.label_input.read(cx).value().to_string(),
            self.serial.common.group_input.read(cx).value().to_string(),
        );

        if port.trim().is_empty() {
            return Err(anyhow::anyhow!("Port is required."));
        }
        let Ok(baud) = baud_raw.trim().parse::<u32>() else {
            return Err(anyhow::anyhow!("Invalid baud rate."));
        };

        let group = {
            let group = group.trim();
            if group.is_empty() {
                "serial".to_string()
            } else {
                group.to_string()
            }
        };
        let label = {
            let label = label.trim();
            if label.is_empty() {
                port.to_string()
            } else {
                label.to_string()
            }
        };

        let backend_for_store = match backend {
            TermBackend::Alacritty => crate::settings::TerminalBackend::Alacritty,
            TermBackend::Wezterm => crate::settings::TerminalBackend::Wezterm,
        };

        Ok(SessionStoreOp::UpdateSerial {
            session_id,
            group,
            label,
            backend: backend_for_store,
            port: port.trim().to_string(),
            baud,
            data_bits,
            parity,
            stop_bits,
            flow_control,
            term: term.to_string(),
            charset: charset.to_string(),
        })
    }

    fn connect_new_serial(&mut self, cx: &mut Context<Self>) -> anyhow::Result<()> {
        let (
            backend,
            port,
            baud_raw,
            data_bits,
            parity,
            stop_bits,
            flow_control,
            term,
            charset,
            label,
            group,
        ) = (
            self.serial.common.ty,
            self.serial.selected_port(cx).unwrap_or_default(),
            self.serial.baud_input.read(cx).value().to_string(),
            self.serial.selected_data_bits(cx),
            self.serial.selected_parity(cx),
            self.serial.selected_stop_bits(cx),
            self.serial.selected_flow_control(cx),
            self.serial.common.term.clone(),
            self.serial.common.charset.clone(),
            self.serial.common.label_input.read(cx).value().to_string(),
            self.serial.common.group_input.read(cx).value().to_string(),
        );

        let backend_type = match backend {
            TermBackend::Alacritty => TerminalType::Alacritty,
            TermBackend::Wezterm => TerminalType::WezTerm,
        };

        if port.trim().is_empty() {
            return Err(anyhow::anyhow!("Port is required."));
        }
        let Ok(baud) = baud_raw.trim().parse::<u32>() else {
            return Err(anyhow::anyhow!("Invalid baud rate."));
        };

        let group = {
            let group = group.trim();
            if group.is_empty() {
                "serial".to_string()
            } else {
                group.to_string()
            }
        };
        let label = {
            let label = label.trim();
            if label.is_empty() {
                port.trim().to_string()
            } else {
                label.to_string()
            }
        };

        let backend_for_store = match backend {
            TermBackend::Alacritty => crate::settings::TerminalBackend::Alacritty,
            TermBackend::Wezterm => crate::settings::TerminalBackend::Wezterm,
        };

        if cx.global::<crate::TermuaAppState>().main_window.is_none() {
            return Err(anyhow::anyhow!("Main window not ready yet."));
        };
        self.submit_in_flight = true;
        cx.notify();

        self.spawn_store_op_detached(
            SessionStoreOp::SaveSerial {
                group,
                label: label.clone(),
                backend: backend_for_store,
                port: port.trim().to_string(),
                baud,
                data_bits,
                parity,
                stop_bits,
                flow_control,
                term: term.to_string(),
                charset: charset.to_string(),
            },
            "persist serial session",
            cx,
        );

        cx.global_mut::<crate::TermuaAppState>().pending_command(
            crate::PendingCommand::OpenSerialTerminal {
                backend_type,
                params: SerialParams {
                    name: label,
                    port: port.trim().to_string(),
                    baud,
                    data_bits,
                    parity,
                    stop_bits,
                    flow_control,
                },
                session_id: None,
            },
        );
        cx.refresh_windows();

        Ok(())
    }

    pub(super) fn apply_session_for_edit(
        &mut self,
        session: &crate::store::Session,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let backend = match session.backend {
            crate::settings::TerminalBackend::Alacritty => TermBackend::Alacritty,
            crate::settings::TerminalBackend::Wezterm => TermBackend::Wezterm,
        };

        self.apply_common_fields_for_edit(backend, session, window, cx);
        match session.protocol {
            crate::store::SessionType::Local => {
                self.apply_local_session_for_edit(session, window, cx);
            }
            crate::store::SessionType::Ssh => {
                self.apply_ssh_session_for_edit(session, window, cx);
            }
            crate::store::SessionType::Serial => {
                self.apply_serial_session_for_edit(session, window, cx);
            }
        }

        cx.notify();
        window.refresh();
    }

    fn apply_common_fields_for_edit(
        &mut self,
        backend: TermBackend,
        session: &crate::store::Session,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let term: gpui::SharedString = session.term.clone().into();
        let charset: gpui::SharedString = session.charset.clone().into();
        let label = session.label.as_str();
        let group = session.group_path.as_str();

        Self::apply_common_state_fields(
            &mut self.shell.common,
            backend,
            &term,
            &charset,
            label,
            group,
            window,
            cx,
        );
        Self::apply_common_state_fields(
            &mut self.ssh.common,
            backend,
            &term,
            &charset,
            label,
            group,
            window,
            cx,
        );
        Self::apply_common_state_fields(
            &mut self.serial.common,
            backend,
            &term,
            &charset,
            label,
            group,
            window,
            cx,
        );
    }

    fn apply_common_state_fields(
        common: &mut super::state::SessionCommonState,
        backend: TermBackend,
        term: &gpui::SharedString,
        charset: &gpui::SharedString,
        label: &str,
        group: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        common.set_type(backend, window, cx);
        common.set_term(term.clone(), window, cx);
        common.set_charset(charset.clone(), window, cx);
        set_input_value(&common.label_input, label, window, cx);
        set_input_value(&common.group_input, group, window, cx);
    }

    fn apply_local_session_for_edit(
        &mut self,
        session: &crate::store::Session,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let program = session
            .shell_program
            .as_deref()
            .unwrap_or(gpui_term::shell::fallback_shell_program());
        self.shell.set_program(program, window, cx);

        // The shell program may auto-sync the label; restore the persisted label/group.
        set_input_value(
            &self.shell.common.label_input,
            session.label.as_str(),
            window,
            cx,
        );
        set_input_value(
            &self.shell.common.group_input,
            session.group_path.as_str(),
            window,
            cx,
        );
    }

    fn apply_ssh_session_for_edit(
        &mut self,
        session: &crate::store::Session,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let auth_type = match session.ssh_auth_type {
            Some(crate::store::SshAuthType::Password) => SshAuthType::Password,
            Some(crate::store::SshAuthType::Config) | None => SshAuthType::Config,
        };
        self.ssh.set_auth_type(auth_type, window, cx);

        if let Some(host) = session.ssh_host.clone() {
            set_input_value(&self.ssh.host_input, &host, window, cx);
        }
        if let Some(port) = session.ssh_port {
            set_input_value(&self.ssh.port_input, port.to_string().as_str(), window, cx);
        }
        if let Some(user) = session.ssh_user.clone() {
            set_input_value(&self.ssh.user_input, &user, window, cx);
        }
        if let Some(pw) = session.ssh_password.clone() {
            set_input_value(&self.ssh.password_input, &pw, window, cx);
        }

        self.ssh.tcp_nodelay = session.ssh_tcp_nodelay;
        self.ssh.tcp_keepalive = session.ssh_tcp_keepalive;

        let proxy_mode = session.ssh_proxy_mode.unwrap_or(SshProxyMode::Disabled);
        self.ssh.set_proxy_mode(proxy_mode, window, cx);
        self.apply_ssh_proxy_command_workdir_for_edit(session, window, cx);
        self.apply_ssh_proxy_env_rows_for_edit(session, window, cx);
        self.apply_ssh_proxy_jump_rows_for_edit(session, proxy_mode, window, cx);
    }

    fn apply_ssh_proxy_command_workdir_for_edit(
        &mut self,
        session: &crate::store::Session,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(cmd) = session.ssh_proxy_command.clone() {
            set_input_value(&self.ssh.proxy_command_input, &cmd, window, cx);
        }
        if let Some(dir) = session.ssh_proxy_workdir.clone() {
            set_input_value(&self.ssh.proxy_workdir_input, &dir, window, cx);
        }
    }

    fn apply_ssh_proxy_env_rows_for_edit(
        &mut self,
        session: &crate::store::Session,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.ssh.proxy_env_rows.clear();
        self.ssh.proxy_env_next_id = 1;
        let Some(vars) = session.ssh_proxy_env.clone() else {
            return;
        };

        for var in vars {
            let id = self.ssh.proxy_env_next_id;
            self.ssh.proxy_env_next_id += 1;

            self.ssh.proxy_env_rows.push(new_proxy_env_row_state(
                id,
                window,
                cx,
                Some(var.name.as_str()),
                Some(var.value.as_str()),
            ));
        }
    }

    fn apply_ssh_proxy_jump_rows_for_edit(
        &mut self,
        session: &crate::store::Session,
        proxy_mode: SshProxyMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.ssh.proxy_jump_rows.clear();
        self.ssh.proxy_jump_next_id = 1;
        if let Some(hops) = session.ssh_proxy_jump.clone() {
            for hop in hops {
                let id = self.ssh.proxy_jump_next_id;
                self.ssh.proxy_jump_next_id += 1;

                self.ssh.proxy_jump_rows.push(new_proxy_jump_row_state(
                    id,
                    window,
                    cx,
                    Some(hop.host.as_str()),
                    hop.user.as_deref(),
                    hop.port,
                ));
            }
        }

        if proxy_mode == SshProxyMode::JumpServer && self.ssh.proxy_jump_rows.is_empty() {
            self.add_empty_proxy_jump_row(window, cx);
        }
    }

    fn add_empty_proxy_jump_row(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let id = self.ssh.proxy_jump_next_id;
        self.ssh.proxy_jump_next_id += 1;

        self.ssh
            .proxy_jump_rows
            .push(new_proxy_jump_row_state(id, window, cx, None, None, None));
    }

    fn apply_serial_session_for_edit(
        &mut self,
        session: &crate::store::Session,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(port) = session.serial_port.clone() {
            self.serial.set_port(&port, window, cx);
        }
        if let Some(baud) = session.serial_baud {
            set_input_value(
                &self.serial.baud_input,
                baud.to_string().as_str(),
                window,
                cx,
            );
        }

        if let Some(bits) = session.serial_data_bits {
            self.serial.set_data_bits(bits, window, cx);
        }
        if let Some(parity) = session.serial_parity {
            self.serial.set_parity(parity, window, cx);
        }
        if let Some(stop_bits) = session.serial_stop_bits {
            self.serial.set_stop_bits(stop_bits, window, cx);
        }
        if let Some(flow) = session.serial_flow_control {
            self.serial.set_flow_control(flow, window, cx);
        }
    }

    fn spawn_store_op_detached(
        &self,
        op: SessionStoreOp,
        action: &'static str,
        cx: &mut Context<Self>,
    ) {
        let background = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            let result = background.spawn(async move { op.run() }).await;
            match result {
                Ok(()) => {
                    let _ = this.update(cx, |_this, cx| {
                        cx.global_mut::<crate::TermuaAppState>()
                            .pending_command(crate::PendingCommand::ReloadSessionsSidebar);
                        cx.refresh_windows();
                    });
                }
                Err(err) => {
                    log::warn!("NewSessionWindow: failed to {action}: {err:#}");
                }
            }
        })
        .detach();
    }
}

#[cfg(test)]
impl NewSessionWindow {
    pub(super) fn persist_new_local_session_for_connect(
        &self,
        app: &gpui::App,
    ) -> anyhow::Result<()> {
        let (backend, shell_program, term, charset, label, group) = (
            self.shell.common.ty,
            self.shell.program.clone(),
            self.shell.common.term.clone(),
            self.shell.common.charset.clone(),
            self.shell.common.label_input.read(app).value().to_string(),
            self.shell.common.group_input.read(app).value().to_string(),
        );

        let group = {
            let group = group.trim();
            if group.is_empty() {
                "local".to_string()
            } else {
                group.to_string()
            }
        };

        let label = {
            let label = label.trim();
            if label.is_empty() {
                shell_program.as_ref().trim().to_string()
            } else {
                label.to_string()
            }
        };

        let backend_for_store = match backend {
            TermBackend::Alacritty => crate::settings::TerminalBackend::Alacritty,
            TermBackend::Wezterm => crate::settings::TerminalBackend::Wezterm,
        };

        crate::store::save_local_session(
            group.as_str(),
            label.as_str(),
            backend_for_store,
            shell_program.as_ref(),
            term.as_ref(),
            charset.as_ref(),
        )?;

        Ok(())
    }
}
