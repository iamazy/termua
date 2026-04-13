use std::collections::HashMap;

use gpui::{Context, Window, px, rems};
use gpui_component::Size;
use gpui_term::Authentication;

use super::{NewSessionWindow, Protocol, SshAuthType, SshSessionState, new_proxy_jump_row_state};
use crate::{
    ssh::expand_home_dir_placeholders,
    store::{SshJumpHop, SshProxyEnvVar, SshProxyMode},
};

pub(super) fn ssh_user_input_box_width(
    window: &Window,
    cx: &mut Context<NewSessionWindow>,
    user_text: &str,
) -> gpui::Pixels {
    let text_style = window.text_style();
    let font_id = cx.text_system().resolve_font(&text_style.font());
    // gpui-component `Input` default size is `Size::Medium`, which uses `text_sm` (0.875rem).
    let font_size = rems(0.875).to_pixels(window.rem_size());

    let user_text_width = measure_text_advance_width(cx, font_id, font_size, user_text.trim());

    // gpui-component input horizontal padding + bordered width (1px each side).
    let padding_x = Size::Medium.input_px() * 2;
    let border_x = px(2.0);
    let content_width = user_text_width + padding_x + border_x;
    content_width.max(px(120.0)).min(px(200.0))
}

fn measure_text_advance_width(
    cx: &mut Context<NewSessionWindow>,
    font_id: gpui::FontId,
    font_size: gpui::Pixels,
    text: &str,
) -> gpui::Pixels {
    if text.is_empty() {
        return gpui::Pixels::ZERO;
    }

    let fallback = cx
        .text_system()
        .em_advance(font_id, font_size)
        .unwrap_or(px(0.0));

    let mut width = gpui::Pixels::ZERO;
    for ch in text.chars() {
        let adv = cx
            .text_system()
            .advance(font_id, font_size, ch)
            .map(|size| size.width)
            .unwrap_or(fallback);
        width = width + adv;
    }
    width
}

pub(super) fn ssh_port_is_valid(port: &str) -> bool {
    let trimmed = port.trim();
    if trimmed.is_empty() {
        return false;
    }

    let Ok(n) = trimmed.parse::<u16>() else {
        return false;
    };

    n >= 1
}

pub fn connect_enabled(protocol: Protocol, ssh_host: &str, ssh_port: &str) -> bool {
    match protocol {
        Protocol::Ssh => !ssh_host.trim().is_empty() && ssh_port_is_valid(ssh_port),
        Protocol::Shell => true,
        Protocol::Serial => true,
    }
}

pub(super) fn ssh_host_is_valid_for_config_auth(input: &str) -> anyhow::Result<()> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(anyhow::anyhow!("host is required"));
    }
    if trimmed.contains('@') {
        return Err(anyhow::anyhow!("host must not include user@"));
    }
    Ok(())
}

pub(super) fn ssh_user_and_host_from_password_inputs(
    user: &str,
    host: &str,
) -> anyhow::Result<(String, String)> {
    let host = host.trim();
    if host.is_empty() {
        return Err(anyhow::anyhow!("host is required"));
    }

    let user = user.trim();
    let user = if user.is_empty() { "root" } else { user };
    Ok((user.to_string(), host.to_string()))
}

pub(super) fn ssh_password_auth_from_inputs(
    user: &str,
    host: &str,
    password: &str,
) -> anyhow::Result<(String, Authentication)> {
    let password = password.trim();
    if password.is_empty() {
        return Err(anyhow::anyhow!("password is required"));
    }

    let (user, host) = ssh_user_and_host_from_password_inputs(user, host)?;
    Ok((host, Authentication::Password(user, password.to_string())))
}

pub(super) fn ssh_connect_enabled_for_values(
    auth_type: SshAuthType,
    ssh_host: &str,
    ssh_port: &str,
    ssh_password: &str,
) -> bool {
    if !connect_enabled(Protocol::Ssh, ssh_host, ssh_port) {
        return false;
    }

    match auth_type {
        SshAuthType::Password => !ssh_password.trim().is_empty(),
        SshAuthType::Config => ssh_host_is_valid_for_config_auth(ssh_host).is_ok(),
    }
}

impl SshSessionState {
    pub(super) fn proxy_settings_for_store(
        &self,
        cx: &Context<NewSessionWindow>,
    ) -> (
        SshProxyMode,
        Option<String>,
        Option<String>,
        Vec<SshProxyEnvVar>,
        Vec<SshJumpHop>,
    ) {
        let mode = self.proxy_mode;

        if mode != SshProxyMode::Command && mode != SshProxyMode::JumpServer {
            return (mode, None, None, Vec::new(), Vec::new());
        }

        let cmd = if mode == SshProxyMode::Command {
            let cmd = self.proxy_command_input.read(cx).value().trim().to_string();
            (!cmd.is_empty()).then_some(cmd)
        } else {
            None
        };

        let dir = self.proxy_workdir_input.read(cx).value().trim().to_string();
        let dir = (!dir.is_empty()).then_some(dir);

        let mut env = Vec::new();
        for row in self.proxy_env_rows.iter() {
            let name = row.name_input.read(cx).value().trim().to_string();
            if name.is_empty() {
                continue;
            }
            let value = row.value_input.read(cx).value().to_string();
            env.push(SshProxyEnvVar { name, value });
        }

        let mut hops = Vec::new();
        if mode == SshProxyMode::JumpServer {
            for row in self.proxy_jump_rows.iter() {
                let host = row.host_input.read(cx).value().trim().to_string();
                if host.is_empty() {
                    continue;
                }

                let user = row.user_input.read(cx).value().trim().to_string();
                let user = (!user.is_empty()).then_some(user);

                let port_s = row.port_input.read(cx).value().trim().to_string();
                let port = (!port_s.is_empty())
                    .then(|| port_s.parse::<u16>().ok())
                    .flatten();

                hops.push(SshJumpHop { host, user, port });
            }
        }

        (mode, cmd, dir, env, hops)
    }

    pub(super) fn proxy_settings_for_opts(
        &self,
        cx: &Context<NewSessionWindow>,
    ) -> gpui_term::SshProxyMode {
        match self.proxy_mode {
            SshProxyMode::Inherit => gpui_term::SshProxyMode::Inherit,
            SshProxyMode::Disabled => gpui_term::SshProxyMode::Disabled,
            SshProxyMode::Command => {
                let (_, cmd, dir, env_vars, _) = self.proxy_settings_for_store(cx);

                let command = cmd.unwrap_or_default();
                let working_dir = dir.map(|d| expand_home_dir_placeholders(d.as_str()));

                let mut env = HashMap::new();
                for var in env_vars {
                    env.insert(var.name, expand_home_dir_placeholders(var.value.as_str()));
                }

                gpui_term::SshProxyMode::Command(gpui_term::SshProxyCommand {
                    command,
                    working_dir,
                    env,
                })
            }
            SshProxyMode::JumpServer => {
                let (_, _, dir, env_vars, hops) = self.proxy_settings_for_store(cx);
                let working_dir = dir.map(|d| expand_home_dir_placeholders(d.as_str()));

                let mut env = HashMap::new();
                for var in env_vars {
                    env.insert(var.name, expand_home_dir_placeholders(var.value.as_str()));
                }

                let hops = hops
                    .into_iter()
                    .map(|hop| gpui_term::SshJumpHop {
                        host: hop.host,
                        user: hop.user,
                        port: hop.port,
                    })
                    .collect();

                gpui_term::SshProxyMode::JumpServer(gpui_term::SshJumpChain {
                    hops,
                    working_dir,
                    env,
                })
            }
        }
    }

    pub(super) fn set_proxy_mode(
        &mut self,
        mode: SshProxyMode,
        window: &mut Window,
        cx: &mut Context<NewSessionWindow>,
    ) {
        self.proxy_mode = mode;
        if mode == SshProxyMode::JumpServer && self.proxy_jump_rows.is_empty() {
            let id = self.proxy_jump_next_id;
            self.proxy_jump_next_id += 1;

            self.proxy_jump_rows
                .push(new_proxy_jump_row_state(id, window, cx, None, None, None));
        }
        self.proxy_select.update(cx, |select, cx| {
            select.set_selected_value(&mode, window, cx);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_host_is_valid_for_config_auth_rejects_user_at() {
        assert!(ssh_host_is_valid_for_config_auth("alice@example.com").is_err());
        assert!(ssh_host_is_valid_for_config_auth("example.com").is_ok());
    }

    #[test]
    fn ssh_user_and_host_from_password_inputs_defaults_user_to_root() {
        let (user, host) = ssh_user_and_host_from_password_inputs("", "example.com").unwrap();
        assert_eq!(user, "root");
        assert_eq!(host, "example.com");
    }

    #[test]
    fn ssh_user_and_host_from_password_inputs_requires_host() {
        assert!(ssh_user_and_host_from_password_inputs("alice", "").is_err());
    }

    #[test]
    fn ssh_password_auth_from_inputs_requires_password() {
        assert!(ssh_password_auth_from_inputs("alice", "example.com", "").is_err());
    }

    #[test]
    fn ssh_password_auth_from_inputs_returns_password_auth() {
        let (host, auth) = ssh_password_auth_from_inputs("alice", "example.com", "pw").unwrap();
        assert_eq!(host, "example.com");
        assert_eq!(auth, Authentication::Password("alice".into(), "pw".into()));
    }

    #[test]
    fn ssh_connect_enabled_for_values_respects_auth_type() {
        assert!(!ssh_connect_enabled_for_values(
            SshAuthType::Password,
            "",
            "22",
            "pw"
        ));
        assert!(ssh_connect_enabled_for_values(
            SshAuthType::Password,
            "example.com",
            "22",
            "pw"
        ));
        assert!(!ssh_connect_enabled_for_values(
            SshAuthType::Password,
            "example.com",
            "22",
            ""
        ));
        assert!(ssh_connect_enabled_for_values(
            SshAuthType::Config,
            "example.com",
            "22",
            ""
        ));
        assert!(!ssh_connect_enabled_for_values(
            SshAuthType::Config,
            "alice@example.com",
            "22",
            ""
        ));
    }
}
