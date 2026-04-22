use gpui::{
    App, Entity, InteractiveElement, IntoElement, ParentElement, SharedString, Styled, StyledImage,
    Window, div, img, prelude::FluentBuilder, px,
};
use gpui_common::TermuaIcon;
use gpui_component::{
    Icon, h_flex,
    input::InputState,
    select::{SearchableVec, SelectItem, SelectState},
};
use rust_i18n::t;

use crate::store::{SerialFlowControl, SerialParity, SerialStopBits, SshProxyMode};

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
    pub(super) ty: TermBackend,
    pub(super) term: SharedString,
    pub(super) colorterm: SharedString,
    pub(super) charset: SharedString,
    pub(super) label_input: Entity<InputState>,
    pub(super) group_input: Entity<InputState>,
    pub(super) type_select: Entity<SelectState<SearchableVec<BackendSelectItem>>>,
    pub(super) term_select: Entity<SelectState<SearchableVec<SharedString>>>,
    pub(super) colorterm_options: Vec<SharedString>,
    pub(super) colorterm_select: Entity<SelectState<SearchableVec<SharedString>>>,
    pub(super) charset_select: Entity<SelectState<SearchableVec<SharedString>>>,
}

pub(super) struct ShellSessionState {
    pub(super) program: SharedString,
    pub(super) program_options: Vec<ShellProgramSelectItem>,
    pub(super) program_select: Entity<SelectState<SearchableVec<ShellProgramSelectItem>>>,
    pub(super) env_rows: Vec<EnvRowState>,
    pub(super) env_next_id: u64,
    pub(super) common: SessionCommonState,
}

#[derive(Clone)]
pub(super) struct ShellProgramSelectItem {
    pub(super) program: SharedString,
    label: SharedString,
}

impl ShellProgramSelectItem {
    pub(super) fn new(program: SharedString) -> Self {
        let label = match program.as_ref() {
            "nu" => "nushell".to_string(),
            "pwsh" => "powershell".to_string(),
            _ => program.to_string(),
        };

        Self {
            program,
            label: SharedString::from(label),
        }
    }

    pub(super) fn icon_path(&self) -> Option<TermuaIcon> {
        match self.program.as_ref() {
            "sh" => Some(TermuaIcon::Sh),
            "bash" | "zsh" => Some(TermuaIcon::Terminal),
            "nu" | "nushell" => Some(TermuaIcon::Nushell),
            "pwsh" | "powershell" => Some(TermuaIcon::Pwsh),
            "fish" => Some(TermuaIcon::Fish),
            _ => None,
        }
    }

    pub(super) fn uses_themed_icon(&self) -> bool {
        !matches!(self.program.as_ref(), "pwsh" | "powershell") && self.icon_path().is_some()
    }

    fn icon_element(&self) -> Option<gpui::AnyElement> {
        let path = self.icon_path()?;
        if self.uses_themed_icon() {
            Some(Icon::default().path(path).size_4().into_any_element())
        } else {
            Some(
                img(path)
                    .w(px(16.))
                    .h(px(16.))
                    .flex_shrink_0()
                    .object_fit(gpui::ObjectFit::Contain)
                    .into_any_element(),
            )
        }
    }
}

impl gpui_component::select::SelectItem for ShellProgramSelectItem {
    type Value = SharedString;

    fn title(&self) -> SharedString {
        self.label.clone()
    }

    fn display_title(&self) -> Option<gpui::AnyElement> {
        // The selected value shown in the input should be left-aligned; do not reserve icon
        // space unless the item actually has an icon.
        let icon = self.icon_element();

        Some(
            h_flex()
                .w_full()
                .justify_start()
                .items_center()
                .gap_2()
                .debug_selector(|| "termua-new-session-shell-program-display-title".to_string())
                .when_some(icon, |this, icon| this.child(icon))
                .child(self.label.clone())
                .into_any_element(),
        )
    }

    fn render(&self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let icon = if let Some(icon) = self.icon_element() {
            icon
        } else {
            div()
                .w(px(16.))
                .h(px(16.))
                .flex_shrink_0()
                .into_any_element()
        };

        h_flex()
            .items_center()
            .gap_2()
            .child(icon)
            .child(self.label.clone())
    }

    fn value(&self) -> &Self::Value {
        &self.program
    }

    fn matches(&self, query: &str) -> bool {
        let query = query.to_lowercase();
        self.label.to_lowercase().contains(&query) || self.program.to_lowercase().contains(&query)
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
    pub(super) sftp: bool,
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
pub(super) enum TermBackend {
    Alacritty,
    Wezterm,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub(super) struct BackendSelectItem {
    backend: TermBackend,
    debug_icon_prefix: &'static str,
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

impl BackendSelectItem {
    pub(super) fn new(backend: TermBackend, debug_icon_prefix: &'static str) -> Self {
        Self {
            backend,
            debug_icon_prefix,
        }
    }
}

impl SelectItem for BackendSelectItem {
    type Value = TermBackend;

    fn title(&self) -> SharedString {
        SharedString::from(self.backend.label().to_string())
    }

    fn display_title(&self) -> Option<gpui::AnyElement> {
        Some(backend_label_with_icon(self.backend, Some(self.debug_icon_prefix)).into_any_element())
    }

    fn render(&self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        backend_label_with_icon(self.backend, None)
    }

    fn value(&self) -> &Self::Value {
        &self.backend
    }
}

impl TermBackend {
    pub(super) fn label(self) -> &'static str {
        match self {
            TermBackend::Alacritty => "Alacritty",
            TermBackend::Wezterm => "Wezterm",
        }
    }

    pub(super) fn icon_path(self) -> TermuaIcon {
        match self {
            TermBackend::Alacritty => TermuaIcon::Alacritty,
            TermBackend::Wezterm => TermuaIcon::Wezterm,
        }
    }

    fn id_suffix(self) -> &'static str {
        match self {
            TermBackend::Alacritty => "alacritty",
            TermBackend::Wezterm => "wezterm",
        }
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

fn backend_label_with_icon(
    backend: TermBackend,
    debug_icon_prefix: Option<&'static str>,
) -> impl IntoElement {
    let icon = img(backend.icon_path())
        .w(px(16.))
        .h(px(16.))
        .flex_shrink_0()
        .object_fit(gpui::ObjectFit::Contain);

    let icon = if let Some(prefix) = debug_icon_prefix {
        let selector = format!("{prefix}-{}", backend.id_suffix());
        div()
            .debug_selector(move || selector)
            .child(icon)
            .into_any_element()
    } else {
        icon.into_any_element()
    };

    let content = h_flex()
        .items_center()
        .gap_2()
        .child(icon)
        .child(div().child(backend.label()));

    if let Some(prefix) = debug_icon_prefix {
        let selector = format!("{prefix}-content-{}", backend.id_suffix());
        content.debug_selector(move || selector).into_any_element()
    } else {
        content.into_any_element()
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

    pub(super) fn from_tab_index(ix: usize) -> Self {
        match ix {
            0 => Protocol::Shell,
            1 => Protocol::Ssh,
            2 => Protocol::Serial,
            _ => Protocol::Shell,
        }
    }
}
