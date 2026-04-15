use std::collections::HashMap;

use gpui::px;
use gpui_term::{SshOptions, TerminalType};

use crate::store::{SerialFlowControl, SerialParity, SerialStopBits};

#[derive(Clone, Debug)]
pub(crate) struct SshParams {
    pub(crate) env: HashMap<String, String>,
    pub(crate) name: String,
    pub(crate) opts: SshOptions,
}

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

impl SerialParams {
    pub(crate) fn to_options(&self) -> gpui_term::SerialOptions {
        gpui_term::SerialOptions {
            port: self.port.clone(),
            baud: self.baud,
            data_bits: self.data_bits,
            parity: match self.parity {
                SerialParity::None => gpui_term::SerialParity::None,
                SerialParity::Even => gpui_term::SerialParity::Even,
                SerialParity::Odd => gpui_term::SerialParity::Odd,
            },
            stop_bits: match self.stop_bits {
                SerialStopBits::One => gpui_term::SerialStopBits::One,
                SerialStopBits::Two => gpui_term::SerialStopBits::Two,
            },
            flow_control: match self.flow_control {
                SerialFlowControl::None => gpui_term::SerialFlowControl::None,
                SerialFlowControl::Software => gpui_term::SerialFlowControl::Software,
                SerialFlowControl::Hardware => gpui_term::SerialFlowControl::Hardware,
            },
        }
    }
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
        params: SshParams,
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
    fn serial_params_convert_to_serial_options() {
        let params = SerialParams {
            name: "usb".to_string(),
            port: "/dev/ttyUSB0".to_string(),
            baud: 115_200,
            data_bits: 8,
            parity: SerialParity::Even,
            stop_bits: SerialStopBits::Two,
            flow_control: SerialFlowControl::Hardware,
        };

        let opts = params.to_options();

        assert_eq!(opts.port, "/dev/ttyUSB0");
        assert_eq!(opts.baud, 115_200);
        assert_eq!(opts.data_bits, 8);
        assert_eq!(opts.parity, gpui_term::SerialParity::Even);
        assert_eq!(opts.stop_bits, gpui_term::SerialStopBits::Two);
        assert_eq!(opts.flow_control, gpui_term::SerialFlowControl::Hardware);
    }

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
