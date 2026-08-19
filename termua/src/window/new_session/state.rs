use gpui::{
    AnyElement, App, Entity, IntoElement, ParentElement, SharedString, Styled, StyledImage, Window,
    div, img, px,
};
use gpui_common::TermuaIcon;
use gpui_component::{
    Icon, Sizable, h_flex,
    input::InputState,
    select::{SearchableVec, SelectItem, SelectState},
};
use rust_i18n::t;

use crate::{
    settings::TerminalBackend,
    store::{SerialFlowControl, SerialParity, SerialStopBits, SshProxyMode},
};

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(super) enum SessionEditorMode {
    New,
    Edit { session_id: i64 },
}

impl SessionEditorMode {
    pub(super) fn is_edit(&self) -> bool {
        matches!(self, Self::Edit { .. })
    }

    pub(super) fn session_id(&self) -> Option<i64> {
        match self {
            Self::New => None,
            Self::Edit { session_id } => Some(*session_id),
        }
    }
}

pub(super) struct SessionCommonState {
    pub(super) backend: TerminalBackend,
    pub(super) backend_select: Entity<SelectState<SearchableVec<TerminalBackendSelectItem>>>,
    pub(super) label_input: Entity<InputState>,
    pub(super) group_input: Entity<InputState>,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(super) struct TerminalBackendSelectItem {
    backend: TerminalBackend,
}

impl TerminalBackendSelectItem {
    pub(super) fn new(backend: TerminalBackend) -> Self {
        Self { backend }
    }

    fn label(&self) -> &'static str {
        match self.backend {
            TerminalBackend::Alacritty => "Alacritty",
            TerminalBackend::Wezterm => "WezTerm",
        }
    }

    pub(super) fn icon(&self) -> TermuaIcon {
        match self.backend {
            TerminalBackend::Alacritty => TermuaIcon::Alacritty,
            TerminalBackend::Wezterm => TermuaIcon::Wezterm,
        }
    }

    fn render_title(&self) -> impl IntoElement {
        h_flex()
            .items_center()
            .gap_2()
            .child(
                img(self.icon())
                    .w(px(16.))
                    .h(px(16.))
                    .flex_shrink_0()
                    .object_fit(gpui::ObjectFit::Contain),
            )
            .child(div().child(self.label()))
    }
}

impl SelectItem for TerminalBackendSelectItem {
    type Value = TerminalBackend;

    fn title(&self) -> SharedString {
        self.label().into()
    }

    fn display_title(&self) -> Option<AnyElement> {
        Some(self.render_title().into_any_element())
    }

    fn render(&self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        self.render_title()
    }

    fn value(&self) -> &Self::Value {
        &self.backend
    }
}

pub(super) struct ShellSessionState {
    pub(super) program: SharedString,
    pub(super) program_options: Vec<ShellProgramSelectItem>,
    pub(super) program_select: Entity<SelectState<SearchableVec<ShellProgramSelectItem>>>,
    pub(super) env_rows: Vec<EnvRowState>,
    pub(super) env_next_id: u64,
    pub(super) common: SessionCommonState,
}

pub(super) fn shell_program_title(program: &str) -> SharedString {
    gpui_term::shell::shell_display_name(program).into()
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ShellProgramSelectItem {
    program: SharedString,
}

impl ShellProgramSelectItem {
    pub(super) fn new(program: impl Into<SharedString>) -> Self {
        Self {
            program: program.into(),
        }
    }

    pub(super) fn icon(&self) -> TermuaIcon {
        use gpui_term::shell::ShellKind;

        match gpui_term::shell::shell_kind(self.program.as_ref()) {
            ShellKind::Bash => TermuaIcon::Terminal,
            ShellKind::Fish => TermuaIcon::Fish,
            ShellKind::Nu => TermuaIcon::Nushell,
            ShellKind::Pwsh | ShellKind::PowerShell => TermuaIcon::Pwsh,
            ShellKind::Zsh | ShellKind::Cmd | ShellKind::Other => TermuaIcon::Terminal,
        }
    }

    fn render_title(&self) -> impl IntoElement {
        h_flex()
            .items_center()
            .gap_2()
            .child(Icon::empty().path(self.icon().path()).small())
            .child(div().child(self.title()))
    }
}

pub(super) struct SshSessionState {
    pub(super) common: SessionCommonState,
    pub(super) env_rows: Vec<EnvRowState>,
    pub(super) env_next_id: u64,
    pub(super) auth_type: SshAuthType,
    pub(super) auth_select: Entity<SelectState<SearchableVec<SshAuthSelectItem>>>,
    pub(super) user_input: Entity<InputState>,
    pub(super) host_input: Entity<InputState>,
    pub(super) port_input: Entity<InputState>,
    pub(super) password_input: Entity<InputState>,
    pub(super) password_edit_unlocked: bool,
    pub(super) tcp_nodelay: bool,
    pub(super) tcp_keepalive: bool,

    pub(super) proxy_mode: SshProxyMode,
    pub(super) proxy_select: Entity<SelectState<SearchableVec<SshProxySelectItem>>>,
    pub(super) proxy_command_input: Entity<InputState>,
    pub(super) proxy_workdir_input: Entity<InputState>,
    pub(super) proxy_env_rows: Vec<ProxyEnvRowState>,
    pub(super) proxy_env_next_id: u64,
    pub(super) proxy_jump_rows: Vec<ProxyJumpRowState>,
    pub(super) proxy_jump_next_id: u64,
}

pub(super) struct SerialSessionState {
    pub(super) common: SessionCommonState,
    pub(super) env_rows: Vec<EnvRowState>,
    pub(super) env_next_id: u64,
    pub(super) ports: Vec<SharedString>,
    pub(super) port_select: Entity<SelectState<SearchableVec<SharedString>>>,
    pub(super) baud_input: Entity<InputState>,
    pub(super) data_bits_select: Entity<SelectState<SearchableVec<SerialDataBitsSelectItem>>>,
    pub(super) parity_select: Entity<SelectState<SearchableVec<SerialParitySelectItem>>>,
    pub(super) stop_bits_select: Entity<SelectState<SearchableVec<SerialStopBitsSelectItem>>>,
    pub(super) flow_control_select: Entity<SelectState<SearchableVec<SerialFlowControlSelectItem>>>,

    pub(super) ports_auto_started: bool,
    pub(super) ports_loading: bool,
    pub(super) ports_refresh_epoch: u64,
    pub(super) ports_pending: Option<Vec<String>>,
}

#[derive(Debug)]
pub(super) struct EnvRowState {
    pub(super) id: u64,
    pub(super) name_input: Entity<InputState>,
    pub(super) value_input: Entity<InputState>,
}

#[derive(Debug)]
pub(super) struct ProxyEnvRowState {
    pub(super) id: u64,
    pub(super) name_input: Entity<InputState>,
    pub(super) value_input: Entity<InputState>,
}

#[derive(Debug)]
pub(super) struct ProxyJumpRowState {
    pub(super) id: u64,
    pub(super) host_input: Entity<InputState>,
    pub(super) user_input: Entity<InputState>,
    pub(super) port_input: Entity<InputState>,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Protocol {
    Shell,
    Ssh,
    Serial,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(super) enum SshAuthType {
    Password,
    Config,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(super) struct SshAuthSelectItem {
    auth_type: SshAuthType,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(super) struct SshProxySelectItem {
    mode: SshProxyMode,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(super) struct SerialDataBitsSelectItem {
    bits: u8,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(super) struct SerialParitySelectItem {
    parity: SerialParity,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(super) struct SerialStopBitsSelectItem {
    bits: SerialStopBits,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(super) struct SerialFlowControlSelectItem {
    flow: SerialFlowControl,
}

impl SshAuthSelectItem {
    pub(super) fn new(auth_type: SshAuthType) -> Self {
        Self { auth_type }
    }
}

impl SshProxySelectItem {
    pub(super) fn new(mode: SshProxyMode) -> Self {
        Self { mode }
    }
}

impl SerialDataBitsSelectItem {
    pub(super) fn new(bits: u8) -> Self {
        Self { bits }
    }
}

impl SerialParitySelectItem {
    pub(super) fn new(parity: SerialParity) -> Self {
        Self { parity }
    }
}

impl SerialStopBitsSelectItem {
    pub(super) fn new(bits: SerialStopBits) -> Self {
        Self { bits }
    }
}

impl SerialFlowControlSelectItem {
    pub(super) fn new(flow: SerialFlowControl) -> Self {
        Self { flow }
    }
}

impl SelectItem for SshAuthSelectItem {
    type Value = SshAuthType;

    fn title(&self) -> SharedString {
        self.auth_type.label()
    }

    fn render(&self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        div().child(self.auth_type.label())
    }

    fn value(&self) -> &Self::Value {
        &self.auth_type
    }
}

impl SelectItem for ShellProgramSelectItem {
    type Value = SharedString;

    fn title(&self) -> SharedString {
        shell_program_title(self.program.as_ref())
    }

    fn display_title(&self) -> Option<AnyElement> {
        Some(self.render_title().into_any_element())
    }

    fn render(&self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        self.render_title()
    }

    fn value(&self) -> &Self::Value {
        &self.program
    }
}

impl SelectItem for SshProxySelectItem {
    type Value = SshProxyMode;

    fn title(&self) -> SharedString {
        ssh_proxy_mode_label(self.mode)
    }

    fn render(&self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        div().child(ssh_proxy_mode_label(self.mode))
    }

    fn value(&self) -> &Self::Value {
        &self.mode
    }
}

impl SelectItem for SerialDataBitsSelectItem {
    type Value = u8;

    fn title(&self) -> SharedString {
        self.bits.to_string().into()
    }

    fn render(&self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        div().child(self.bits.to_string())
    }

    fn value(&self) -> &Self::Value {
        &self.bits
    }
}

impl SelectItem for SerialParitySelectItem {
    type Value = SerialParity;

    fn title(&self) -> SharedString {
        serial_parity_label(self.parity)
    }

    fn render(&self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        div().child(serial_parity_label(self.parity))
    }

    fn value(&self) -> &Self::Value {
        &self.parity
    }
}

impl SelectItem for SerialStopBitsSelectItem {
    type Value = SerialStopBits;

    fn title(&self) -> SharedString {
        SharedString::from(serial_stop_bits_label(self.bits))
    }

    fn render(&self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        div().child(serial_stop_bits_label(self.bits))
    }

    fn value(&self) -> &Self::Value {
        &self.bits
    }
}

impl SelectItem for SerialFlowControlSelectItem {
    type Value = SerialFlowControl;

    fn title(&self) -> SharedString {
        serial_flow_control_label(self.flow)
    }

    fn render(&self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        div().child(serial_flow_control_label(self.flow))
    }

    fn value(&self) -> &Self::Value {
        &self.flow
    }
}

impl SshAuthType {
    pub(super) fn label(self) -> SharedString {
        match self {
            SshAuthType::Password => t!("NewSession.Select.SshAuth.Password").to_string().into(),
            SshAuthType::Config => t!("NewSession.Select.SshAuth.Config").to_string().into(),
        }
    }
}

fn ssh_proxy_mode_label(mode: SshProxyMode) -> SharedString {
    match mode {
        SshProxyMode::Inherit => t!("NewSession.Select.SshProxy.Inherit").to_string().into(),
        SshProxyMode::Disabled => t!("NewSession.Select.SshProxy.Disabled").to_string().into(),
        SshProxyMode::Command => t!("NewSession.Select.SshProxy.Command").to_string().into(),
        SshProxyMode::JumpServer => t!("NewSession.Select.SshProxy.JumpServer")
            .to_string()
            .into(),
    }
}

fn serial_parity_label(parity: SerialParity) -> SharedString {
    match parity {
        SerialParity::None => t!("NewSession.Select.SerialParity.None").to_string().into(),
        SerialParity::Even => t!("NewSession.Select.SerialParity.Even").to_string().into(),
        SerialParity::Odd => t!("NewSession.Select.SerialParity.Odd").to_string().into(),
    }
}

fn serial_stop_bits_label(bits: SerialStopBits) -> &'static str {
    match bits {
        SerialStopBits::One => "1",
        SerialStopBits::Two => "2",
    }
}

fn serial_flow_control_label(flow: SerialFlowControl) -> SharedString {
    match flow {
        SerialFlowControl::None => t!("NewSession.Select.SerialFlow.None").to_string().into(),
        SerialFlowControl::Software => t!("NewSession.Select.SerialFlow.Software")
            .to_string()
            .into(),
        SerialFlowControl::Hardware => t!("NewSession.Select.SerialFlow.Hardware")
            .to_string()
            .into(),
    }
}

impl Protocol {
    pub(super) fn tab_index(self) -> usize {
        match self {
            Protocol::Shell => 0,
            Protocol::Ssh => 1,
            Protocol::Serial => 2,
        }
    }

    pub(super) fn debug_id(self) -> &'static str {
        match self {
            Protocol::Shell => "shell",
            Protocol::Ssh => "ssh",
            Protocol::Serial => "serial",
        }
    }
}
