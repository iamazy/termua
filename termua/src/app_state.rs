use std::collections::HashMap;

use gpui::px;
use gpui_term::{SshOptions, TerminalType};

use crate::store::{SerialFlowControl, SerialParity, SerialStopBits};

#[derive(Clone, Debug)]
pub(crate) struct SerialParams {
    pub(crate) name: String,
    pub(crate) port: String,
    pub(crate) baud: u32,
    pub(crate) data_bits: u8,
    pub(crate) parity: SerialParity,
    pub(crate) stop_bits: SerialStopBits,
    pub(crate) flow_control: SerialFlowControl,
}

pub(crate) struct TermuaAppState {
    pub(crate) main_window: Option<gpui::WindowHandle<gpui_component::Root>>,
    pub(crate) settings_window: Option<gpui::WindowHandle<gpui_component::Root>>,
    pub(crate) multi_exec_enabled: bool,
    pub(crate) sessions_sidebar_visible: bool,
    pub(crate) sessions_sidebar_width: gpui::Pixels,
    pub(crate) pending_commands: Vec<PendingCommand>,
}

impl Default for TermuaAppState {
    fn default() -> Self {
        Self {
            main_window: None,
            settings_window: None,
            multi_exec_enabled: false,
            sessions_sidebar_visible: true,
            sessions_sidebar_width: px(280.0),
            pending_commands: Vec::new(),
        }
    }
}

impl gpui::Global for TermuaAppState {}

impl TermuaAppState {
    pub(crate) fn pending_command(&mut self, command: PendingCommand) {
        if self
            .pending_commands
            .iter()
            .any(|existing| existing.coalesces_with(&command))
        {
            return;
        }

        self.pending_commands.push(command);
    }
}

#[derive(Clone, Debug)]
pub(crate) enum PendingCommand {
    OpenLocalTerminal {
        backend_type: TerminalType,
        env: HashMap<String, String>,
    },
    OpenSshTerminal {
        backend_type: TerminalType,
        env: HashMap<String, String>,
        name: String,
        opts: SshOptions,
    },
    OpenSerialTerminal {
        backend_type: TerminalType,
        params: SerialParams,
        session_id: Option<i64>,
    },
    ReloadSessionsSidebar,
    OpenCastPicker,
    OpenJoinSharingDialog,
    JoinRelaySharing {
        relay_url: String,
        room_id: String,
        join_key: String,
    },
}

impl PendingCommand {
    fn coalesces_with(&self, other: &Self) -> bool {
        matches!(
            (self, other),
            (Self::ReloadSessionsSidebar, Self::ReloadSessionsSidebar)
                | (Self::OpenCastPicker, Self::OpenCastPicker)
                | (Self::OpenJoinSharingDialog, Self::OpenJoinSharingDialog)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enqueue_pending_command_coalesces_singleton_commands() {
        let mut state = TermuaAppState::default();

        state.pending_command(PendingCommand::ReloadSessionsSidebar);
        state.pending_command(PendingCommand::ReloadSessionsSidebar);
        state.pending_command(PendingCommand::OpenCastPicker);
        state.pending_command(PendingCommand::OpenCastPicker);
        state.pending_command(PendingCommand::OpenJoinSharingDialog);
        state.pending_command(PendingCommand::OpenJoinSharingDialog);

        assert_eq!(state.pending_commands.len(), 3);
        assert!(matches!(
            state.pending_commands[0],
            PendingCommand::ReloadSessionsSidebar
        ));
        assert!(matches!(
            state.pending_commands[1],
            PendingCommand::OpenCastPicker
        ));
        assert!(matches!(
            state.pending_commands[2],
            PendingCommand::OpenJoinSharingDialog
        ));
    }

    #[test]
    fn enqueue_pending_command_keeps_repeatable_commands() {
        let mut state = TermuaAppState::default();

        state.pending_command(PendingCommand::OpenLocalTerminal {
            backend_type: TerminalType::WezTerm,
            env: HashMap::new(),
        });
        state.pending_command(PendingCommand::OpenLocalTerminal {
            backend_type: TerminalType::WezTerm,
            env: HashMap::new(),
        });

        assert_eq!(state.pending_commands.len(), 2);
    }
}
