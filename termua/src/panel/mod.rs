pub mod assistant_panel;
pub mod message_panel;
pub mod right_sidebar;
pub mod sessions_sidebar;
pub mod sftp_panel;
pub mod ssh_error_panel;
pub mod terminal_panel;

pub(crate) use right_sidebar::RightSidebarView;
pub(crate) use sessions_sidebar::{SessionsSidebarEvent, SessionsSidebarView};
pub(crate) use ssh_error_panel::SshErrorPanel;
pub(crate) use terminal_panel::{
    PanelKind, TerminalPanel, local_terminal_panel_tab_name, terminal_panel_tab_name,
};
