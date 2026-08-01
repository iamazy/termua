pub mod assistant_panel;
pub mod message_panel;
pub mod right_sidebar;
pub mod sessions_sidebar;
pub mod sftp_panel;
pub mod ssh_error_panel;
pub mod terminal_panel;

pub(crate) const RIGHT_SIDEBAR_PANEL_NAME: &str = "termua.right_sidebar";
pub(crate) const SESSIONS_SIDEBAR_PANEL_NAME: &str = "termua.sessions_sidebar";
pub(crate) const SFTP_PANEL_NAME: &str = "termua.sftp_dock_panel";
pub(crate) const SSH_ERROR_PANEL_NAME: &str = "SshErrorPanel";
pub(crate) const TERMINAL_PANEL_NAME: &str = "TerminalPanel";

pub(crate) use right_sidebar::RightSidebarView;
pub(crate) use sessions_sidebar::{SessionsSidebarEvent, SessionsSidebarView};
pub(crate) use ssh_error_panel::SshErrorPanel;
pub(crate) use terminal_panel::{
    PanelKind, TerminalLaunchState, TerminalPanel, TerminalPanelState,
    local_terminal_panel_tab_name, terminal_panel_tab_name,
};
