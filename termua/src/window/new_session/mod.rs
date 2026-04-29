use gpui::{
    App, AppContext, Bounds, Context, Entity, FocusHandle, Focusable, ScrollHandle, SharedString,
    Subscription, Window, WindowBounds, WindowDecorations, WindowHandle, WindowOptions, px, size,
};
use gpui_component::{
    IndexPath, TitleBar,
    input::{InputEvent, InputState},
    select::{SearchableVec, SelectEvent, SelectItem, SelectState},
    tree::{TreeItem, TreeState},
};
use rust_i18n::t;

const SHELL_SESSION_ID: &str = "shell.session";
const SSH_SESSION_ID: &str = "ssh.session";
const SERIAL_SESSION_ID: &str = "serial.session";
const DEFAULT_COLORTERM: &str = "truecolor";

use nav::{
    Page, build_nav_tree_items, default_selected_item_id, find_tree_item_by_id,
    page_for_tree_item_id,
};

use crate::store::{SerialFlowControl, SerialParity, SerialStopBits, SshProxyMode};

mod actions;
mod nav;
mod render;
mod ssh;
mod state;

pub use ssh::connect_enabled;
pub use state::Protocol;
use state::{
    BackendSelectItem, EnvRowState, ProxyEnvRowState, ProxyJumpRowState, SerialDataBitsSelectItem,
    SerialFlowControlSelectItem, SerialParitySelectItem, SerialSessionState,
    SerialStopBitsSelectItem, SessionCommonState, SessionEditorMode, ShellSessionState,
    SshAuthSelectItem, SshAuthType, SshProxySelectItem, SshSessionState, TermBackend,
    shell_program_title,
};

pub struct NewSessionWindow {
    focus_handle: FocusHandle,
    mode: SessionEditorMode,
    protocol: Protocol,
    submit_in_flight: bool,

    lock_overlay: crate::lock_screen::overlay::LockOverlayState,

    selected_item_id: SharedString,
    nav_tree_items: Vec<TreeItem>,
    nav_tree_state: Entity<TreeState>,

    right_scroll_handle: ScrollHandle,

    shell: ShellSessionState,
    ssh: SshSessionState,
    serial: SerialSessionState,

    _subscriptions: Vec<Subscription>,
}

fn row_index(row: usize) -> IndexPath {
    IndexPath::default().row(row)
}

fn new_select<I>(
    window: &mut Window,
    cx: &mut Context<NewSessionWindow>,
    items: Vec<I>,
    selected_row: Option<usize>,
) -> Entity<SelectState<SearchableVec<I>>>
where
    I: SelectItem + 'static,
    I::Value: 'static,
{
    cx.new(|cx| {
        SelectState::new(
            SearchableVec::new(items),
            selected_row.map(row_index),
            window,
            cx,
        )
    })
}

fn new_input(
    window: &mut Window,
    cx: &mut Context<NewSessionWindow>,
    placeholder: String,
) -> Entity<InputState> {
    cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
}

fn new_configured_input<F>(
    window: &mut Window,
    cx: &mut Context<NewSessionWindow>,
    placeholder: String,
    configure: F,
) -> Entity<InputState>
where
    F: FnOnce(InputState) -> InputState,
{
    cx.new(|cx| configure(InputState::new(window, cx).placeholder(placeholder)))
}

fn set_input_value(
    input: &Entity<InputState>,
    value: &str,
    window: &mut Window,
    cx: &mut Context<NewSessionWindow>,
) {
    let value = value.to_string();
    input.update(cx, move |state, cx| state.set_value(&value, window, cx));
}

fn set_input_placeholder(
    input: &Entity<InputState>,
    placeholder: String,
    window: &mut Window,
    cx: &mut Context<NewSessionWindow>,
) {
    input.update(cx, |state, cx| {
        state.set_placeholder(placeholder, window, cx);
    });
}

fn sync_input_placeholders(
    window: &mut Window,
    cx: &mut Context<NewSessionWindow>,
    placeholders: &[(Entity<InputState>, String)],
) {
    for (input, placeholder) in placeholders {
        set_input_placeholder(input, placeholder.clone(), window, cx);
    }
}

fn new_input_with_value(
    window: &mut Window,
    cx: &mut Context<NewSessionWindow>,
    placeholder: String,
    value: &str,
) -> Entity<InputState> {
    let input = new_input(window, cx, placeholder);
    set_input_value(&input, value, window, cx);
    input
}

fn colorterm_options() -> Vec<SharedString> {
    vec![
        SharedString::from(DEFAULT_COLORTERM),
        SharedString::from("24bit"),
    ]
}

fn normalize_colorterm(value: &str) -> SharedString {
    let value = value.trim();
    if value.is_empty() {
        SharedString::from(DEFAULT_COLORTERM)
    } else {
        SharedString::from(value.to_string())
    }
}

fn new_env_row_state(
    id: u64,
    window: &mut Window,
    cx: &mut Context<NewSessionWindow>,
    name: Option<&str>,
    value: Option<&str>,
) -> EnvRowState {
    let name_input = new_input(window, cx, t!("NewSession.Placeholder.EnvVar").to_string());
    if let Some(name) = name {
        set_input_value(&name_input, name, window, cx);
    }

    let value_input = new_input(
        window,
        cx,
        t!("NewSession.Placeholder.EnvValue").to_string(),
    );
    if let Some(value) = value {
        set_input_value(&value_input, value, window, cx);
    }

    EnvRowState {
        id,
        name_input,
        value_input,
    }
}

fn new_proxy_env_row_state(
    id: u64,
    window: &mut Window,
    cx: &mut Context<NewSessionWindow>,
    name: Option<&str>,
    value: Option<&str>,
) -> ProxyEnvRowState {
    let row = new_env_row_state(id, window, cx, name, value);
    ProxyEnvRowState {
        id: row.id,
        name_input: row.name_input,
        value_input: row.value_input,
    }
}

fn new_proxy_jump_row_state(
    id: u64,
    window: &mut Window,
    cx: &mut Context<NewSessionWindow>,
    host: Option<&str>,
    user: Option<&str>,
    port: Option<u16>,
) -> ProxyJumpRowState {
    let host_input = new_input(
        window,
        cx,
        t!("NewSession.Placeholder.JumpHost").to_string(),
    );
    if let Some(host) = host {
        set_input_value(&host_input, host, window, cx);
    }

    let user_input = new_input(
        window,
        cx,
        t!("NewSession.Placeholder.JumpUser").to_string(),
    );
    if let Some(user) = user {
        set_input_value(&user_input, user, window, cx);
    }

    let port_input = new_input(
        window,
        cx,
        t!("NewSession.Placeholder.JumpPort").to_string(),
    );
    if let Some(port) = port {
        set_input_value(&port_input, &port.to_string(), window, cx);
    }

    ProxyJumpRowState {
        id,
        host_input,
        user_input,
        port_input,
    }
}

impl NewSessionWindow {
    pub fn open(app: &mut App) -> anyhow::Result<WindowHandle<gpui_component::Root>> {
        use gpui_component::Root;

        let initial_size = size(px(860.), px(640.));
        let min_size = size(px(720.), px(480.));
        let initial_bounds = Bounds::centered(None, initial_size, app);

        let handle = app.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(initial_bounds)),
                titlebar: Some(TitleBar::title_bar_options()),
                window_decorations: cfg!(target_os = "linux").then_some(WindowDecorations::Client),
                window_min_size: Some(min_size),
                ..Default::default()
            },
            |window, cx| {
                let view = cx.new(|cx| Self::new(window, cx));
                cx.new(|cx| Root::new(view, window, cx))
            },
        )?;

        Ok(handle)
    }

    pub fn open_edit(
        session_id: i64,
        app: &mut App,
    ) -> anyhow::Result<WindowHandle<gpui_component::Root>> {
        use gpui_component::Root;

        let session = crate::store::load_session(session_id)?
            .ok_or_else(|| anyhow::anyhow!("session {session_id} not found"))?;

        let initial_size = size(px(860.), px(640.));
        let min_size = size(px(720.), px(480.));
        let initial_bounds = Bounds::centered(None, initial_size, app);

        let handle = app.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(initial_bounds)),
                titlebar: Some(TitleBar::title_bar_options()),
                window_decorations: cfg!(target_os = "linux").then_some(WindowDecorations::Client),
                window_min_size: Some(min_size),
                ..Default::default()
            },
            move |window, cx| {
                let view = cx.new(|cx| Self::new_for_edit(session, window, cx));
                cx.new(|cx| Root::new(view, window, cx))
            },
        )?;

        Ok(handle)
    }

    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::new_with_mode(SessionEditorMode::New, Protocol::Shell, window, cx)
    }

    fn new_for_edit(
        session: crate::store::Session,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let protocol = match session.protocol {
            crate::store::SessionType::Local => Protocol::Shell,
            crate::store::SessionType::Ssh => Protocol::Ssh,
            crate::store::SessionType::Serial => Protocol::Serial,
        };
        let mut this = Self::new_with_mode(
            SessionEditorMode::Edit {
                session_id: session.id,
            },
            protocol,
            window,
            cx,
        );
        this.apply_session_for_edit(&session, window, cx);
        this
    }

    fn new_with_mode(
        mode: SessionEditorMode,
        protocol: Protocol,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::set_window_title_for_mode(mode, window);
        Self::ensure_globals(cx);

        let settings = crate::settings::load_settings_from_disk().unwrap_or_default();
        let default_backend = match settings.terminal.default_backend {
            crate::settings::TerminalBackend::Alacritty => TermBackend::Alacritty,
            crate::settings::TerminalBackend::Wezterm => TermBackend::Wezterm,
        };

        let lock_overlay = crate::lock_screen::overlay::LockOverlayState::new(window, cx);

        let nav_tree_items = build_nav_tree_items(protocol);
        let nav_tree_state = cx.new(|cx| TreeState::new(cx).items(nav_tree_items.clone()));

        let shell = ShellSessionState::new(default_backend, window, cx);
        let mut ssh = SshSessionState::new(default_backend, window, cx);
        if mode.is_edit() {
            ssh.password_edit_unlocked = false;
        }
        let serial = SerialSessionState::new(default_backend, window, cx);

        let mut this = Self {
            focus_handle: cx.focus_handle(),
            mode,
            protocol,
            submit_in_flight: false,

            lock_overlay,

            selected_item_id: default_selected_item_id(protocol),
            nav_tree_items,
            nav_tree_state,

            right_scroll_handle: ScrollHandle::default(),

            shell,
            ssh,
            serial,

            _subscriptions: Vec::new(),
        };

        this.install_subscriptions(window, cx);
        this.sync_nav_tree_selection(cx);
        this
    }

    fn set_window_title_for_mode(mode: SessionEditorMode, window: &mut Window) {
        let title = if mode.is_edit() {
            t!("NewSession.WindowTitle.Edit")
        } else {
            t!("NewSession.WindowTitle.New")
        };
        window.set_window_title(title.as_ref());
    }

    fn ensure_globals(cx: &mut Context<Self>) {
        if cx.try_global::<crate::TermuaAppState>().is_none() {
            cx.set_global(crate::TermuaAppState::default());
        }
        if cx
            .try_global::<crate::notification::NotifyState>()
            .is_none()
        {
            cx.set_global(crate::notification::NotifyState::default());
        }
        crate::settings::ensure_language_state_with_default(crate::settings::Language::English, cx);
    }

    fn install_subscriptions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.install_language_subscription(window, cx);
        self.install_shell_subscriptions(window, cx);
        self.install_ssh_subscriptions(window, cx);
        self.install_serial_subscriptions(window, cx);
    }

    fn install_language_subscription(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self._subscriptions
            .push(cx.observe_global_in::<crate::settings::LanguageSettings>(
                window,
                |this, window, cx| {
                    this.sync_localized_strings(window, cx);
                    cx.notify();
                    window.refresh();
                },
            ));
    }

    fn subscribe_numeric_input_filter(
        subscriptions: &mut Vec<Subscription>,
        input: &Entity<InputState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        subscriptions.push(cx.subscribe_in(input, window, {
            move |_this, input, ev, window, cx| {
                if !matches!(ev, InputEvent::Change) {
                    return;
                }

                let value = input.read(cx).value();
                let filtered: String = value
                    .as_ref()
                    .chars()
                    .filter(|c| c.is_ascii_digit())
                    .collect();

                if filtered != value.as_ref() {
                    input.update(cx, |state, cx| state.set_value(&filtered, window, cx));
                }

                cx.notify();
                window.refresh();
            }
        }));
    }

    fn subscribe_refresh_input(
        subscriptions: &mut Vec<Subscription>,
        input: &Entity<InputState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        subscriptions.push(cx.subscribe_in(input, window, {
            move |_this, _input, ev, window, cx| {
                if matches!(ev, InputEvent::Change) {
                    cx.notify();
                    window.refresh();
                }
            }
        }));
    }

    fn subscribe_env_row_inputs(
        &mut self,
        row: &EnvRowState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        Self::subscribe_refresh_input(&mut self._subscriptions, &row.name_input, window, cx);
        Self::subscribe_refresh_input(&mut self._subscriptions, &row.value_input, window, cx);
    }

    fn subscribe_proxy_env_row_inputs(
        &mut self,
        row: &ProxyEnvRowState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        Self::subscribe_refresh_input(&mut self._subscriptions, &row.name_input, window, cx);
        Self::subscribe_refresh_input(&mut self._subscriptions, &row.value_input, window, cx);
    }

    fn push_shell_env_row(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        name: Option<&str>,
        value: Option<&str>,
    ) {
        let id = self.shell.env_next_id;
        self.shell.env_next_id += 1;

        let row = new_env_row_state(id, window, cx, None, None);
        self.subscribe_env_row_inputs(&row, window, cx);
        if let Some(name) = name {
            set_input_value(&row.name_input, name, window, cx);
        }
        if let Some(value) = value {
            set_input_value(&row.value_input, value, window, cx);
        }
        self.shell.env_rows.push(row);
    }

    fn push_ssh_env_row(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        name: Option<&str>,
        value: Option<&str>,
    ) {
        let id = self.ssh.env_next_id;
        self.ssh.env_next_id += 1;

        let row = new_env_row_state(id, window, cx, None, None);
        self.subscribe_env_row_inputs(&row, window, cx);
        if let Some(name) = name {
            set_input_value(&row.name_input, name, window, cx);
        }
        if let Some(value) = value {
            set_input_value(&row.value_input, value, window, cx);
        }
        self.ssh.env_rows.push(row);
    }

    fn push_ssh_proxy_env_row(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        name: Option<&str>,
        value: Option<&str>,
    ) {
        let id = self.ssh.proxy_env_next_id;
        self.ssh.proxy_env_next_id += 1;

        let row = new_proxy_env_row_state(id, window, cx, None, None);
        self.subscribe_proxy_env_row_inputs(&row, window, cx);
        if let Some(name) = name {
            set_input_value(&row.name_input, name, window, cx);
        }
        if let Some(value) = value {
            set_input_value(&row.value_input, value, window, cx);
        }
        self.ssh.proxy_env_rows.push(row);
    }

    fn install_shell_subscriptions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self._subscriptions
            .push(cx.subscribe_in(&self.shell.common.type_select, window, {
                move |this,
                      _select,
                      ev: &SelectEvent<SearchableVec<BackendSelectItem>>,
                      window,
                      cx| {
                    if let SelectEvent::Confirm(Some(backend)) = ev {
                        this.shell.common.set_type(*backend, window, cx);
                        cx.notify();
                        window.refresh();
                    }
                }
            }));
        self._subscriptions
            .push(cx.subscribe_in(&self.shell.common.term_select, window, {
                move |this, _select, ev: &SelectEvent<SearchableVec<SharedString>>, window, cx| {
                    if let SelectEvent::Confirm(Some(term)) = ev {
                        this.shell.common.set_term(term.clone(), window, cx);
                        cx.notify();
                        window.refresh();
                    }
                }
            }));
        self._subscriptions
            .push(cx.subscribe_in(&self.shell.common.charset_select, window, {
                move |this, _select, ev: &SelectEvent<SearchableVec<SharedString>>, window, cx| {
                    if let SelectEvent::Confirm(Some(charset)) = ev {
                        this.shell.common.set_charset(charset.clone(), window, cx);
                        cx.notify();
                        window.refresh();
                    }
                }
            }));
        self._subscriptions.push(
            cx.subscribe_in(&self.shell.common.colorterm_select, window, {
                move |this, _select, ev: &SelectEvent<SearchableVec<SharedString>>, window, cx| {
                    if let SelectEvent::Confirm(Some(colorterm)) = ev {
                        this.shell
                            .common
                            .set_colorterm(colorterm.as_ref(), window, cx);
                        cx.notify();
                        window.refresh();
                    }
                }
            }),
        );
    }

    fn install_ssh_subscriptions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self._subscriptions
            .push(cx.subscribe_in(&self.ssh.host_input, window, {
                move |_this, _input, ev, window, cx| {
                    if matches!(ev, InputEvent::Change) {
                        cx.notify();
                        window.refresh();
                    }
                }
            }));
        self._subscriptions
            .push(cx.subscribe_in(&self.ssh.user_input, window, {
                move |_this, _input, ev, window, cx| {
                    if matches!(ev, InputEvent::Change) {
                        cx.notify();
                        window.refresh();
                    }
                }
            }));
        Self::subscribe_numeric_input_filter(
            &mut self._subscriptions,
            &self.ssh.port_input,
            window,
            cx,
        );

        self._subscriptions
            .push(cx.subscribe_in(&self.ssh.common.type_select, window, {
                move |this,
                      _select,
                      ev: &SelectEvent<SearchableVec<BackendSelectItem>>,
                      window,
                      cx| {
                    if let SelectEvent::Confirm(Some(backend)) = ev {
                        this.ssh.common.set_type(*backend, window, cx);
                        cx.notify();
                        window.refresh();
                    }
                }
            }));
        self._subscriptions
            .push(cx.subscribe_in(&self.ssh.auth_select, window, {
                move |this,
                      _select,
                      ev: &SelectEvent<SearchableVec<SshAuthSelectItem>>,
                      window,
                      cx| {
                    if let SelectEvent::Confirm(Some(auth_type)) = ev {
                        this.ssh.set_auth_type(*auth_type, window, cx);
                        cx.notify();
                        window.refresh();
                    }
                }
            }));
        self._subscriptions
            .push(cx.subscribe_in(&self.ssh.proxy_select, window, {
                move |this,
                      _select,
                      ev: &SelectEvent<SearchableVec<SshProxySelectItem>>,
                      window,
                      cx| {
                    if let SelectEvent::Confirm(Some(mode)) = ev {
                        this.ssh.set_proxy_mode(*mode, window, cx);
                        cx.notify();
                        window.refresh();
                    }
                }
            }));
        self._subscriptions
            .push(cx.subscribe_in(&self.ssh.common.term_select, window, {
                move |this, _select, ev: &SelectEvent<SearchableVec<SharedString>>, window, cx| {
                    if let SelectEvent::Confirm(Some(term)) = ev {
                        this.ssh.common.set_term(term.clone(), window, cx);
                        cx.notify();
                        window.refresh();
                    }
                }
            }));
        self._subscriptions
            .push(cx.subscribe_in(&self.ssh.common.charset_select, window, {
                move |this, _select, ev: &SelectEvent<SearchableVec<SharedString>>, window, cx| {
                    if let SelectEvent::Confirm(Some(charset)) = ev {
                        this.ssh.common.set_charset(charset.clone(), window, cx);
                        cx.notify();
                        window.refresh();
                    }
                }
            }));
        self._subscriptions
            .push(cx.subscribe_in(&self.ssh.common.colorterm_select, window, {
                move |this, _select, ev: &SelectEvent<SearchableVec<SharedString>>, window, cx| {
                    if let SelectEvent::Confirm(Some(colorterm)) = ev {
                        this.ssh
                            .common
                            .set_colorterm(colorterm.as_ref(), window, cx);
                        cx.notify();
                        window.refresh();
                    }
                }
            }));
    }

    fn install_serial_subscriptions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self._subscriptions
            .push(cx.subscribe_in(&self.serial.common.type_select, window, {
                move |this,
                      _select,
                      ev: &SelectEvent<SearchableVec<BackendSelectItem>>,
                      window,
                      cx| {
                    if let SelectEvent::Confirm(Some(backend)) = ev {
                        this.serial.common.set_type(*backend, window, cx);
                        cx.notify();
                        window.refresh();
                    }
                }
            }));
        self._subscriptions
            .push(cx.subscribe_in(&self.serial.common.term_select, window, {
                move |this, _select, ev: &SelectEvent<SearchableVec<SharedString>>, window, cx| {
                    if let SelectEvent::Confirm(Some(term)) = ev {
                        this.serial.common.set_term(term.clone(), window, cx);
                        cx.notify();
                        window.refresh();
                    }
                }
            }));
        self._subscriptions.push(
            cx.subscribe_in(&self.serial.common.charset_select, window, {
                move |this, _select, ev: &SelectEvent<SearchableVec<SharedString>>, window, cx| {
                    if let SelectEvent::Confirm(Some(charset)) = ev {
                        this.serial.common.set_charset(charset.clone(), window, cx);
                        cx.notify();
                        window.refresh();
                    }
                }
            }),
        );
        Self::subscribe_numeric_input_filter(
            &mut self._subscriptions,
            &self.serial.baud_input,
            window,
            cx,
        );
        self._subscriptions
            .push(cx.subscribe_in(&self.serial.port_select, window, {
                move |_this, _select, ev: &SelectEvent<SearchableVec<SharedString>>, window, cx| {
                    if let SelectEvent::Confirm(Some(_port)) = ev {
                        cx.notify();
                        window.refresh();
                    }
                }
            }));
    }
}

impl Focusable for NewSessionWindow {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl NewSessionWindow {
    fn sync_localized_strings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let title = if self.mode.is_edit() {
            t!("NewSession.WindowTitle.Edit")
        } else {
            t!("NewSession.WindowTitle.New")
        };
        window.set_window_title(title.as_ref());

        self.lock_overlay.sync_localized_placeholders(window, cx);

        self.shell.common.sync_localized_placeholders(window, cx);
        self.ssh.common.sync_localized_placeholders(window, cx);
        self.serial.common.sync_localized_placeholders(window, cx);

        self.shell.sync_localized_placeholders(window, cx);
        self.ssh.sync_localized_placeholders(window, cx);
        self.serial.sync_localized_placeholders(window, cx);

        self.nav_tree_items = build_nav_tree_items(self.protocol);
        let items = self.nav_tree_items.clone();
        self.nav_tree_state
            .update(cx, |tree, cx| tree.set_items(items, cx));
        self.sync_nav_tree_selection(cx);
    }

    fn set_protocol(&mut self, protocol: Protocol, cx: &mut Context<Self>) {
        if self.mode.is_edit() && self.protocol != protocol {
            // Editing is scoped to one session type; do not allow switching between Shell/SSH.
            return;
        }
        if self.protocol == protocol {
            return;
        }

        self.protocol = protocol;
        self.selected_item_id = default_selected_item_id(protocol);
        self.right_scroll_handle
            .set_offset(gpui::point(px(0.), px(0.)));
        self.nav_tree_items = build_nav_tree_items(protocol);
        let items = self.nav_tree_items.clone();
        self.nav_tree_state
            .update(cx, |tree, cx| tree.set_items(items, cx));
        self.sync_nav_tree_selection(cx);
        cx.notify();
    }

    fn sync_nav_tree_selection(&mut self, cx: &mut Context<Self>) {
        let Some(item) = find_tree_item_by_id(&self.nav_tree_items, self.selected_item_id.as_ref())
        else {
            return;
        };

        self.nav_tree_state.update(cx, |tree, cx| {
            tree.set_selected_item(Some(item), cx);
        });
    }
}

impl SessionCommonState {
    fn sync_localized_placeholders(&self, window: &mut Window, cx: &mut Context<NewSessionWindow>) {
        sync_input_placeholders(
            window,
            cx,
            &[
                (
                    self.label_input.clone(),
                    t!("NewSession.Placeholder.Label").to_string(),
                ),
                (
                    self.group_input.clone(),
                    t!("NewSession.Placeholder.Group").to_string(),
                ),
            ],
        );
    }

    fn new(
        default_backend: TermBackend,
        window: &mut Window,
        cx: &mut Context<NewSessionWindow>,
        debug_icon_prefix: &'static str,
    ) -> Self {
        let term_select = new_select(
            window,
            cx,
            vec![
                SharedString::from("xterm-256color"),
                SharedString::from("screen-256color"),
                SharedString::from("tmux-256color"),
            ],
            Some(0),
        );

        let charset_select = new_select(
            window,
            cx,
            vec![SharedString::from("UTF-8"), SharedString::from("ASCII")],
            Some(0),
        );

        let type_select = new_select(
            window,
            cx,
            vec![
                BackendSelectItem::new(TermBackend::Alacritty, debug_icon_prefix),
                BackendSelectItem::new(TermBackend::Wezterm, debug_icon_prefix),
            ],
            Some(match default_backend {
                TermBackend::Alacritty => 0,
                TermBackend::Wezterm => 1,
            }),
        );

        let label_input = new_input(window, cx, t!("NewSession.Placeholder.Label").to_string());
        let group_input = new_input(window, cx, t!("NewSession.Placeholder.Group").to_string());
        let colorterm_options = colorterm_options();
        let colorterm_select = new_select(window, cx, colorterm_options.clone(), Some(0));
        Self {
            ty: default_backend,
            term: "xterm-256color".into(),
            colorterm: DEFAULT_COLORTERM.into(),
            charset: "UTF-8".into(),
            label_input,
            group_input,
            type_select,
            term_select,
            colorterm_options,
            colorterm_select,
            charset_select,
        }
    }

    fn set_type(
        &mut self,
        ty: TermBackend,
        window: &mut Window,
        cx: &mut Context<NewSessionWindow>,
    ) {
        self.ty = ty;
        self.type_select.update(cx, |select, cx| {
            select.set_selected_value(&ty, window, cx);
        });
    }

    fn set_term(
        &mut self,
        term: SharedString,
        window: &mut Window,
        cx: &mut Context<NewSessionWindow>,
    ) {
        self.term = term.clone();
        self.term_select.update(cx, |select, cx| {
            select.set_selected_value(&term, window, cx);
        });
    }

    fn set_charset(
        &mut self,
        charset: SharedString,
        window: &mut Window,
        cx: &mut Context<NewSessionWindow>,
    ) {
        self.charset = charset.clone();
        self.charset_select.update(cx, |select, cx| {
            select.set_selected_value(&charset, window, cx);
        });
    }

    fn set_colorterm(
        &mut self,
        colorterm: &str,
        window: &mut Window,
        cx: &mut Context<NewSessionWindow>,
    ) {
        let colorterm = normalize_colorterm(colorterm);
        self.colorterm = colorterm.clone();

        if !self.colorterm_options.iter().any(|item| item == &colorterm) {
            self.colorterm_options.push(colorterm.clone());
            let items = SearchableVec::new(self.colorterm_options.clone());
            self.colorterm_select.update(cx, |select, cx| {
                select.set_items(items, window, cx);
            });
        }

        self.colorterm_select.update(cx, |select, cx| {
            select.set_selected_value(&colorterm, window, cx);
        });
    }
}

impl ShellSessionState {
    fn program_default_value() -> SharedString {
        gpui_term::shell::ui_default_shell_program().into()
    }

    fn new(
        default_backend: TermBackend,
        window: &mut Window,
        cx: &mut Context<NewSessionWindow>,
    ) -> Self {
        let common = SessionCommonState::new(
            default_backend,
            window,
            cx,
            "termua-new-session-shell-type-icon",
        );

        let program = Self::program_default_value();

        let this = Self {
            program,
            env_rows: Vec::new(),
            env_next_id: 1,
            common,
        };

        // Initialize label to match the selected program.
        let program = this.program.clone();
        let label = shell_program_title(program.as_ref());
        this.common.label_input.update(cx, move |input, cx| {
            input.set_value(label.clone(), window, cx);
        });

        this
    }

    fn set_program(
        &mut self,
        program: &str,
        window: &mut Window,
        cx: &mut Context<NewSessionWindow>,
    ) {
        let old_program = self.program.clone();
        let current_label = self.common.label_input.read(cx).value().to_string();
        let old_display_label = shell_program_title(old_program.as_ref());

        let program: SharedString = program.to_string().into();
        self.program = program.clone();

        // Keep the label in sync with the selected shell program, but don't override
        // user-customized labels.
        let should_update_label = {
            let label = current_label.trim();
            label.is_empty() || label == old_display_label.as_ref() || label == old_program.as_ref()
        };
        if should_update_label {
            let new_label = shell_program_title(program.as_ref());
            self.common.label_input.update(cx, move |input, cx| {
                input.set_value(new_label.clone(), window, cx);
            });
        }
    }
}

impl ShellSessionState {
    fn sync_localized_placeholders(&self, window: &mut Window, cx: &mut Context<NewSessionWindow>) {
        for row in &self.env_rows {
            sync_input_placeholders(
                window,
                cx,
                &[
                    (
                        row.name_input.clone(),
                        t!("NewSession.Placeholder.EnvVar").to_string(),
                    ),
                    (
                        row.value_input.clone(),
                        t!("NewSession.Placeholder.EnvValue").to_string(),
                    ),
                ],
            );
        }
    }
}

impl SshSessionState {
    fn new(
        default_backend: TermBackend,
        window: &mut Window,
        cx: &mut Context<NewSessionWindow>,
    ) -> Self {
        let common = SessionCommonState::new(
            default_backend,
            window,
            cx,
            "termua-new-session-ssh-type-icon",
        );

        let auth_select = new_select(
            window,
            cx,
            vec![
                SshAuthSelectItem::new(SshAuthType::Password),
                SshAuthSelectItem::new(SshAuthType::Config),
            ],
            Some(0),
        );

        let user_input = new_input(window, cx, t!("NewSession.Placeholder.SshUser").to_string());
        let host_input = new_input(window, cx, t!("NewSession.Placeholder.SshHost").to_string());
        let port_input = new_input_with_value(
            window,
            cx,
            t!("NewSession.Placeholder.SshPort").to_string(),
            "22",
        );
        let password_input = new_configured_input(
            window,
            cx,
            t!("NewSession.Placeholder.SshPassword").to_string(),
            |input| input.masked(true),
        );

        let proxy_select = new_select(
            window,
            cx,
            vec![
                SshProxySelectItem::new(SshProxyMode::Inherit),
                SshProxySelectItem::new(SshProxyMode::Disabled),
                SshProxySelectItem::new(SshProxyMode::Command),
                SshProxySelectItem::new(SshProxyMode::JumpServer),
            ],
            Some(1),
        );

        let proxy_command_input = new_input(
            window,
            cx,
            t!("NewSession.Placeholder.ProxyCommand").to_string(),
        );
        let proxy_workdir_input = new_input(
            window,
            cx,
            t!("NewSession.Placeholder.ProxyWorkdir").to_string(),
        );

        Self {
            common,
            env_rows: Vec::new(),
            env_next_id: 1,
            auth_type: SshAuthType::Password,
            auth_select,
            user_input,
            host_input,
            port_input,
            password_input,
            password_edit_unlocked: true,
            sftp: true,
            tcp_nodelay: true,
            tcp_keepalive: false,

            proxy_mode: SshProxyMode::Disabled,
            proxy_select,
            proxy_command_input,
            proxy_workdir_input,
            proxy_env_rows: Vec::new(),
            proxy_env_next_id: 1,
            proxy_jump_rows: Vec::new(),
            proxy_jump_next_id: 1,
        }
    }

    fn set_auth_type(
        &mut self,
        auth_type: SshAuthType,
        window: &mut Window,
        cx: &mut Context<NewSessionWindow>,
    ) {
        self.auth_type = auth_type;
        self.auth_select.update(cx, |select, cx| {
            select.set_selected_value(&auth_type, window, cx);
        });
    }

    #[cfg(test)]
    fn set_auth_type_for_test_only(
        &mut self,
        auth_type: SshAuthType,
        window: &mut Window,
        cx: &mut Context<NewSessionWindow>,
    ) {
        self.set_auth_type(auth_type, window, cx);
    }

    fn sync_localized_placeholders(&self, window: &mut Window, cx: &mut Context<NewSessionWindow>) {
        sync_input_placeholders(
            window,
            cx,
            &[
                (
                    self.user_input.clone(),
                    t!("NewSession.Placeholder.SshUser").to_string(),
                ),
                (
                    self.host_input.clone(),
                    t!("NewSession.Placeholder.SshHost").to_string(),
                ),
                (
                    self.port_input.clone(),
                    t!("NewSession.Placeholder.SshPort").to_string(),
                ),
                (
                    self.password_input.clone(),
                    t!("NewSession.Placeholder.SshPassword").to_string(),
                ),
                (
                    self.proxy_command_input.clone(),
                    t!("NewSession.Placeholder.ProxyCommand").to_string(),
                ),
                (
                    self.proxy_workdir_input.clone(),
                    t!("NewSession.Placeholder.ProxyWorkdir").to_string(),
                ),
            ],
        );

        for row in &self.env_rows {
            sync_input_placeholders(
                window,
                cx,
                &[
                    (
                        row.name_input.clone(),
                        t!("NewSession.Placeholder.EnvVar").to_string(),
                    ),
                    (
                        row.value_input.clone(),
                        t!("NewSession.Placeholder.EnvValue").to_string(),
                    ),
                ],
            );
        }

        for row in &self.proxy_env_rows {
            sync_input_placeholders(
                window,
                cx,
                &[
                    (
                        row.name_input.clone(),
                        t!("NewSession.Placeholder.EnvVar").to_string(),
                    ),
                    (
                        row.value_input.clone(),
                        t!("NewSession.Placeholder.EnvValue").to_string(),
                    ),
                ],
            );
        }

        for row in &self.proxy_jump_rows {
            sync_input_placeholders(
                window,
                cx,
                &[
                    (
                        row.host_input.clone(),
                        t!("NewSession.Placeholder.JumpHost").to_string(),
                    ),
                    (
                        row.user_input.clone(),
                        t!("NewSession.Placeholder.JumpUser").to_string(),
                    ),
                    (
                        row.port_input.clone(),
                        t!("NewSession.Placeholder.JumpPort").to_string(),
                    ),
                ],
            );
        }
    }
}

impl SerialSessionState {
    fn new(
        default_backend: TermBackend,
        window: &mut Window,
        cx: &mut Context<NewSessionWindow>,
    ) -> Self {
        let common = SessionCommonState::new(
            default_backend,
            window,
            cx,
            "termua-new-session-serial-type-icon",
        );

        let ports = Vec::<SharedString>::new();
        let port_select = new_select(window, cx, ports.clone(), None);

        let baud_input = new_input_with_value(
            window,
            cx,
            t!("NewSession.Placeholder.SerialBaud").to_string(),
            "9600",
        );

        let data_bits_select = new_select(
            window,
            cx,
            vec![
                SerialDataBitsSelectItem::new(5),
                SerialDataBitsSelectItem::new(6),
                SerialDataBitsSelectItem::new(7),
                SerialDataBitsSelectItem::new(8),
            ],
            Some(3),
        );

        let parity_select = new_select(
            window,
            cx,
            vec![
                SerialParitySelectItem::new(SerialParity::None),
                SerialParitySelectItem::new(SerialParity::Even),
                SerialParitySelectItem::new(SerialParity::Odd),
            ],
            Some(0),
        );

        let stop_bits_select = new_select(
            window,
            cx,
            vec![
                SerialStopBitsSelectItem::new(SerialStopBits::One),
                SerialStopBitsSelectItem::new(SerialStopBits::Two),
            ],
            Some(0),
        );

        let flow_control_select = new_select(
            window,
            cx,
            vec![
                SerialFlowControlSelectItem::new(SerialFlowControl::None),
                SerialFlowControlSelectItem::new(SerialFlowControl::Software),
                SerialFlowControlSelectItem::new(SerialFlowControl::Hardware),
            ],
            Some(0),
        );

        Self {
            common,
            ports,
            port_select,
            baud_input,
            data_bits_select,
            parity_select,
            stop_bits_select,
            flow_control_select,
            ports_auto_started: false,
            ports_loading: false,
            ports_refresh_epoch: 0,
            ports_pending: None,
        }
    }

    fn selected_port(&self, cx: &App) -> Option<String> {
        self.port_select
            .read(cx)
            .selected_value()
            .map(|s| s.to_string())
            .filter(|s| !s.trim().is_empty())
    }

    fn refresh_ports(&mut self, window: &mut Window, cx: &mut Context<NewSessionWindow>) {
        self.start_refresh_ports_async(cx);
        cx.notify();
        window.refresh();
    }

    fn apply_ports(
        &mut self,
        ports: Vec<String>,
        window: &mut Window,
        cx: &mut Context<NewSessionWindow>,
    ) {
        let current = self.selected_port(cx);

        let mut next_ports = ports
            .into_iter()
            .map(SharedString::from)
            .collect::<Vec<_>>();

        // Preserve the currently selected port even if it is not discoverable right now (e.g.
        // unplugged device, permission issues).
        if let Some(current) = current.as_deref()
            && !next_ports.iter().any(|p| p.as_ref() == current)
        {
            next_ports.push(current.to_string().into());
        }

        self.ports = next_ports;

        let selected = current
            .as_deref()
            .and_then(|c| self.ports.iter().find(|p| p.as_ref() == c).cloned())
            .or_else(|| self.ports.first().cloned());

        let items = SearchableVec::new(self.ports.clone());
        self.port_select.update(cx, |select, cx| {
            select.set_items(items, window, cx);
            if let Some(selected) = &selected {
                select.set_selected_value(selected, window, cx);
            } else {
                select.set_selected_index(None, window, cx);
            }
        });
    }

    fn start_refresh_ports_async(&mut self, cx: &mut Context<NewSessionWindow>) {
        self.ports_loading = true;
        self.ports_refresh_epoch = self.ports_refresh_epoch.saturating_add(1);
        let epoch = self.ports_refresh_epoch;

        cx.spawn(async move |this, cx| {
            let ports = smol::unblock(crate::serial::list_ports).await;
            let _ = this.update(cx, |this, cx| {
                if this.serial.ports_refresh_epoch != epoch {
                    return;
                }
                this.serial.ports_pending = Some(ports);
                this.serial.ports_loading = false;
                cx.notify();
            });
        })
        .detach();
    }

    fn set_port(&mut self, port: &str, window: &mut Window, cx: &mut Context<NewSessionWindow>) {
        let port: SharedString = port.trim().to_string().into();
        if port.as_ref().is_empty() {
            return;
        }

        if !self.ports.iter().any(|p| p.as_ref() == port.as_ref()) {
            self.ports.push(port.clone());
            let items = SearchableVec::new(self.ports.clone());
            self.port_select.update(cx, |select, cx| {
                select.set_items(items, window, cx);
            });
        }

        self.port_select.update(cx, |select, cx| {
            select.set_selected_value(&port, window, cx);
        });
    }

    pub(super) fn selected_data_bits(&self, cx: &App) -> u8 {
        self.data_bits_select
            .read(cx)
            .selected_value()
            .copied()
            .unwrap_or(8)
    }

    pub(super) fn selected_parity(&self, cx: &App) -> SerialParity {
        self.parity_select
            .read(cx)
            .selected_value()
            .copied()
            .unwrap_or(SerialParity::None)
    }

    pub(super) fn selected_stop_bits(&self, cx: &App) -> SerialStopBits {
        self.stop_bits_select
            .read(cx)
            .selected_value()
            .copied()
            .unwrap_or(SerialStopBits::One)
    }

    pub(super) fn selected_flow_control(&self, cx: &App) -> SerialFlowControl {
        self.flow_control_select
            .read(cx)
            .selected_value()
            .copied()
            .unwrap_or(SerialFlowControl::None)
    }

    pub(super) fn set_data_bits(
        &mut self,
        data_bits: u8,
        window: &mut Window,
        cx: &mut Context<NewSessionWindow>,
    ) {
        self.data_bits_select.update(cx, |select, cx| {
            select.set_selected_value(&data_bits, window, cx);
        });
    }

    pub(super) fn set_parity(
        &mut self,
        parity: SerialParity,
        window: &mut Window,
        cx: &mut Context<NewSessionWindow>,
    ) {
        self.parity_select.update(cx, |select, cx| {
            select.set_selected_value(&parity, window, cx);
        });
    }

    pub(super) fn set_stop_bits(
        &mut self,
        stop_bits: SerialStopBits,
        window: &mut Window,
        cx: &mut Context<NewSessionWindow>,
    ) {
        self.stop_bits_select.update(cx, |select, cx| {
            select.set_selected_value(&stop_bits, window, cx);
        });
    }

    pub(super) fn set_flow_control(
        &mut self,
        flow_control: SerialFlowControl,
        window: &mut Window,
        cx: &mut Context<NewSessionWindow>,
    ) {
        self.flow_control_select.update(cx, |select, cx| {
            select.set_selected_value(&flow_control, window, cx);
        });
    }

    fn sync_localized_placeholders(&self, window: &mut Window, cx: &mut Context<NewSessionWindow>) {
        set_input_placeholder(
            &self.baud_input,
            t!("NewSession.Placeholder.SerialBaud").to_string(),
            window,
            cx,
        );
    }
}

#[cfg(test)]
mod tests;
