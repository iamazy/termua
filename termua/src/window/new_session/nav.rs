use gpui::SharedString;
use gpui_component::tree::TreeItem;
use rust_i18n::t;

use super::{Protocol, SERIAL_SESSION_ID, SHELL_SESSION_ID, SSH_SESSION_ID};

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(super) enum Page {
    ShellSession,
    SshSession,
    SshConnection,
    SshProxy,
    SerialSession,
    SerialConnection,
    SerialFrameSettings,
}

pub(super) fn page_for_tree_item_id(protocol: Protocol, id: &str) -> Page {
    match (protocol, id) {
        (Protocol::Shell, SHELL_SESSION_ID) => Page::ShellSession,
        (Protocol::Ssh, SSH_SESSION_ID) => Page::SshSession,
        (Protocol::Ssh, "ssh.connection") => Page::SshConnection,
        (Protocol::Ssh, "ssh.proxy") => Page::SshProxy,
        (Protocol::Serial, SERIAL_SESSION_ID) => Page::SerialSession,
        (Protocol::Serial, "serial.connection") => Page::SerialConnection,
        (Protocol::Serial, "serial.frame") => Page::SerialFrameSettings,
        // Allow clicking the leaf even after tab switching.
        (_, SHELL_SESSION_ID) => Page::ShellSession,
        (_, SSH_SESSION_ID) => Page::SshSession,
        (_, SERIAL_SESSION_ID) => Page::SerialSession,
        (_, "ssh.connection") => Page::SshConnection,
        (_, "ssh.proxy") => Page::SshProxy,
        (_, "serial.connection") => Page::SerialConnection,
        (_, "serial.frame") => Page::SerialFrameSettings,
        (Protocol::Shell, _) => Page::ShellSession,
        (Protocol::Ssh, _) => Page::SshSession,
        (Protocol::Serial, _) => Page::SerialSession,
    }
}

pub(super) fn default_selected_item_id(protocol: Protocol) -> SharedString {
    match protocol {
        Protocol::Shell => SHELL_SESSION_ID.into(),
        Protocol::Ssh => SSH_SESSION_ID.into(),
        Protocol::Serial => SERIAL_SESSION_ID.into(),
    }
}

pub(super) fn build_nav_tree_items(protocol: Protocol) -> Vec<TreeItem> {
    match protocol {
        Protocol::Shell => vec![TreeItem::new(
            SHELL_SESSION_ID,
            t!("NewSession.Nav.Session").to_string(),
        )],
        Protocol::Ssh => vec![
            TreeItem::new(SSH_SESSION_ID, t!("NewSession.Nav.Session").to_string()),
            TreeItem::new("ssh.ssh", t!("NewSession.Nav.Ssh.Name").to_string())
                .expanded(true)
                .children([
                    TreeItem::new(
                        "ssh.connection",
                        t!("NewSession.Nav.Ssh.Connection").to_string(),
                    ),
                    TreeItem::new("ssh.proxy", t!("NewSession.Nav.Ssh.Proxy").to_string()),
                ]),
        ],
        Protocol::Serial => vec![
            TreeItem::new(SERIAL_SESSION_ID, t!("NewSession.Nav.Session").to_string()),
            TreeItem::new(
                "serial.serial",
                t!("NewSession.Nav.Serial.Name").to_string(),
            )
            .expanded(true)
            .children([
                TreeItem::new(
                    "serial.connection",
                    t!("NewSession.Nav.Serial.Connection").to_string(),
                ),
                TreeItem::new(
                    "serial.frame",
                    t!("NewSession.Nav.Serial.Frame").to_string(),
                ),
            ]),
        ],
    }
}

pub(super) fn find_tree_item_by_id<'a>(items: &'a [TreeItem], id: &str) -> Option<&'a TreeItem> {
    for item in items {
        if item.id.as_ref() == id {
            return Some(item);
        }
        if let Some(found) = find_tree_item_by_id(&item.children, id) {
            return Some(found);
        }
    }
    None
}
