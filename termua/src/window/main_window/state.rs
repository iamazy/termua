//! TermuaWindow state and construction.

use std::{collections::HashMap, sync::Arc, time::Duration};

use gpui::{App, AppContext, Context, Focusable, Styled, Subscription, Window};
use gpui_common::TermuaIcon;
use gpui_component::{ActiveTheme, Icon, IconName};
use gpui_dock::{
    DockArea, DockAreaState, DockEvent, DockItem, DockPlacement, DockState, PanelState, PanelView,
    register_panel,
};
use gpui_term::{
    Clear, Copy as CopyAction, CursorShape, Paste, PtySource, SelectAll, SshOptions,
    TerminalBuilder, TerminalType, TerminalView, ToggleCastRecording,
};
use gpui_transfer::TransferCenterState;
use rust_i18n::t;

use crate::{
    OpenSftp, ShareTerminalWeb, TermuaAppState,
    footbar::FootbarView,
    globals::{ensure_ctx_global, ensure_ctx_global_with},
    lock_screen, notification,
    panel::{
        PanelKind, RightSidebarView, SessionsSidebarEvent, SessionsSidebarView, TerminalLaunchState,
    },
    right_sidebar,
    settings::{ThemeMode, set_theme_mode, theme_mode},
    ssh::{SshTerminalBuilderFn, SshTerminalFactory},
};

type RestoredTerminalBuilderFn = Arc<
    dyn Fn(&TerminalLaunchState, usize) -> anyhow::Result<(PanelKind, Box<dyn SshTerminalFactory>)>
        + Send
        + Sync,
>;

fn default_restored_terminal_builder(
    launch: &TerminalLaunchState,
    id: usize,
) -> anyhow::Result<(PanelKind, Box<dyn SshTerminalFactory>)> {
    let (kind, builder) = match launch {
        TerminalLaunchState::Local { backend_type, env } => (
            PanelKind::Local,
            TerminalBuilder::new(
                *backend_type,
                env.clone(),
                CursorShape::default(),
                None,
                id as u64,
            )?,
        ),
        TerminalLaunchState::Serial {
            backend_type,
            params,
            ..
        } => (
            PanelKind::Serial,
            TerminalBuilder::new_with_pty(
                *backend_type,
                PtySource::Serial {
                    opts: params.to_options(),
                },
                CursorShape::default(),
                None,
            )?,
        ),
        TerminalLaunchState::Recorder {
            cast_path,
            playback_speed,
        } => (
            PanelKind::Recorder,
            TerminalBuilder::new(
                TerminalType::WezTerm,
                crate::env::cast_player_child_env(cast_path, *playback_speed),
                CursorShape::default(),
                None,
                id as u64,
            )?,
        ),
        TerminalLaunchState::Ssh { .. } => anyhow::bail!("SSH terminals reconnect separately"),
    };
    Ok((kind, Box::new(builder)))
}
pub(crate) struct TermuaWindow {
    pub(crate) dock_area: gpui::Entity<DockArea>,
    pub(crate) sessions_sidebar: gpui::Entity<SessionsSidebarView>,
    pub(super) right_sidebar: gpui::Entity<RightSidebarView>,
    pub(super) footbar: gpui::Entity<FootbarView>,
    pub(super) lock_overlay: lock_screen::overlay::LockOverlayState,
    pub(super) last_observed_locked: Option<bool>,
    pub(super) focused_terminal_view: Option<gpui::WeakEntity<TerminalView>>,
    pub(super) next_terminal_id: usize,
    pub(super) local_tab_label_counts: HashMap<String, usize>,
    pub(super) ssh_tab_label_counts: HashMap<String, usize>,
    pub(super) ssh_terminal_builder: SshTerminalBuilderFn,
    pub(super) terminal_context_menu_provider: Arc<dyn gpui_term::ContextMenuProvider>,
    pub(super) workspace_save_task: Option<gpui::Task<()>>,
    pub(super) web_share: Option<Arc<crate::web_terminal::WebShareServer>>,
    pub(super) web_share_starting: bool,
    pub(super) web_share_subscription: Option<Subscription>,
    pub(super) _subscriptions: Vec<Subscription>,
}

#[cfg(test)]
const WORKSPACE_SAVE_DEBOUNCE: Duration = Duration::from_millis(10);
#[cfg(not(test))]
const WORKSPACE_SAVE_DEBOUNCE: Duration = Duration::from_millis(500);

fn find_panel_state<'a>(state: &'a PanelState, panel_name: &str) -> Option<&'a PanelState> {
    if state.panel_name == panel_name {
        return Some(state);
    }
    state
        .children
        .iter()
        .find_map(|child| find_panel_state(child, panel_name))
}

fn unwrap_fixed_dock_panel(dock: &mut Option<DockState>, panel_name: &str) {
    let Some(dock) = dock else {
        return;
    };
    if let Some(panel) = find_panel_state(dock.panel_state(), panel_name).cloned() {
        dock.set_panel_state(panel);
    }
}

fn normalize_fixed_sidebar_panels(state: &mut DockAreaState) {
    unwrap_fixed_dock_panel(
        &mut state.left_dock,
        crate::panel::SESSIONS_SIDEBAR_PANEL_NAME,
    );
    unwrap_fixed_dock_panel(
        &mut state.right_dock,
        crate::panel::RIGHT_SIDEBAR_PANEL_NAME,
    );
}

struct TermuaContextMenuProvider;

pub(super) struct RecorderContextMenuProvider;

impl RecorderContextMenuProvider {
    pub(super) fn new_terminal_view(
        terminal: gpui::Entity<gpui_term::Terminal>,
        window: &mut Window,
        cx: &mut Context<TerminalView>,
    ) -> TerminalView {
        TerminalView::new_with_context_menu_provider(
            terminal,
            window,
            cx,
            true,
            Some(Arc::new(Self)),
        )
    }
}

impl gpui_term::ContextMenuProvider for RecorderContextMenuProvider {
    fn context_menu(
        &self,
        menu: gpui_component::menu::PopupMenu,
        _terminal: gpui::Entity<gpui_term::Terminal>,
        terminal_view: gpui::Entity<TerminalView>,
        window: &mut Window,
        cx: &mut App,
    ) -> gpui_component::menu::PopupMenu {
        let focus = terminal_view.read(cx).focus_handle.clone();
        window.focus(&focus, cx);

        menu.menu_with_icon(
            t!("Terminal.ContextMenu.Copy").to_string(),
            IconName::Copy,
            Box::new(CopyAction),
        )
        .separator()
        .menu(
            t!("Terminal.ContextMenu.SelectAll").to_string(),
            Box::new(SelectAll),
        )
    }
}

impl gpui_term::ContextMenuProvider for TermuaContextMenuProvider {
    fn context_menu(
        &self,
        menu: gpui_component::menu::PopupMenu,
        terminal: gpui::Entity<gpui_term::Terminal>,
        terminal_view: gpui::Entity<TerminalView>,
        window: &mut Window,
        cx: &mut App,
    ) -> gpui_component::menu::PopupMenu {
        // Ensure context-menu actions target the terminal the user interacted with.
        let focus = terminal_view.read(cx).focus_handle.clone();
        window.focus(&focus, cx);

        let recording_active = terminal.read(cx).cast_recording_active();
        let record_icon_color = if recording_active {
            cx.theme().danger
        } else {
            cx.theme().muted_foreground
        };
        let record_icon = Icon::default()
            .path(TermuaIcon::Record)
            .text_color(record_icon_color);
        let recording_label_key = if recording_active {
            "Terminal.ContextMenu.RecordingActive"
        } else {
            "Terminal.ContextMenu.Recording"
        };

        let has_sftp = terminal.read(cx).sftp().is_some();

        let mut menu = if has_sftp {
            menu.menu(
                t!("MainWindow.ContextMenu.OpenSftp").to_string(),
                Box::new(OpenSftp),
            )
            .separator()
        } else {
            menu
        };

        menu = menu
            .item(
                gpui_component::menu::PopupMenuItem::new(t!(recording_label_key).to_string())
                    .icon(record_icon)
                    .checked(recording_active)
                    .action(Box::new(ToggleCastRecording)),
            )
            .separator();

        menu = menu
            .menu(
                t!("Terminal.ContextMenu.ShareWeb").to_string(),
                Box::new(ShareTerminalWeb),
            )
            .separator()
            .menu_with_icon(
                t!("Terminal.ContextMenu.Copy").to_string(),
                IconName::Copy,
                Box::new(CopyAction),
            )
            .menu(
                t!("Terminal.ContextMenu.Paste").to_string(),
                Box::new(Paste),
            )
            .separator()
            .menu(
                t!("Terminal.ContextMenu.SelectAll").to_string(),
                Box::new(SelectAll),
            )
            .separator()
            .menu(
                t!("Terminal.ContextMenu.Clear").to_string(),
                Box::new(Clear),
            );

        menu
    }
}

impl TermuaWindow {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let ssh_terminal_builder: SshTerminalBuilderFn = Arc::new(
            move |backend_type: TerminalType, env: HashMap<String, String>, opts: SshOptions| {
                TerminalBuilder::new_with_pty(
                    backend_type,
                    PtySource::Ssh { env, opts },
                    CursorShape::default(),
                    None,
                )
                .map(|builder| Box::new(builder) as Box<dyn crate::ssh::SshTerminalFactory>)
            },
        );

        Self::new_with_ssh_terminal_builder(window, ssh_terminal_builder, cx)
    }

    pub(crate) fn new_with_ssh_terminal_builder(
        window: &mut Window,
        ssh_terminal_builder: SshTerminalBuilderFn,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_with_terminal_builders(
            window,
            ssh_terminal_builder,
            Arc::new(default_restored_terminal_builder),
            cx,
        )
    }

    pub(crate) fn new_with_terminal_builders(
        window: &mut Window,
        ssh_terminal_builder: SshTerminalBuilderFn,
        restored_terminal_builder: RestoredTerminalBuilderFn,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::ensure_globals(cx);

        let dock_area =
            cx.new(|cx| DockArea::new("termua", Some(crate::workspace::STATE_VERSION), window, cx));
        let sessions_sidebar = cx.new(|cx| SessionsSidebarView::new(window, cx));
        let right_sidebar = cx.new(|cx| RightSidebarView::new(window, cx));
        register_panel(cx, crate::panel::SESSIONS_SIDEBAR_PANEL_NAME, {
            let sessions_sidebar = sessions_sidebar.clone();
            move |_, _, info, window, cx| {
                if let gpui_dock::PanelInfo::Panel(value) = info
                    && let Ok(state) = serde_json::from_value::<
                        crate::panel::sessions_sidebar::SessionsSidebarPanelState,
                    >(value.clone())
                {
                    sessions_sidebar.update(cx, |sidebar, cx| {
                        sidebar.restore_persisted_state(state, window, cx)
                    });
                }
                Box::new(sessions_sidebar.clone())
            }
        });
        register_panel(cx, crate::panel::RIGHT_SIDEBAR_PANEL_NAME, {
            let right_sidebar = right_sidebar.clone();
            move |_, _, info, window, cx| {
                if let gpui_dock::PanelInfo::Panel(value) = info
                    && let Ok(state) = serde_json::from_value::<
                        crate::panel::right_sidebar::RightSidebarPanelState,
                    >(value.clone())
                {
                    right_sidebar.update(cx, |sidebar, cx| {
                        sidebar.restore_persisted_state(state, window, cx)
                    });
                }
                Box::new(right_sidebar.clone())
            }
        });
        let footbar = cx.new(FootbarView::new);
        let lock_overlay = lock_screen::overlay::LockOverlayState::new(window, cx);
        let mut this = Self {
            dock_area: dock_area.clone(),
            sessions_sidebar: sessions_sidebar.clone(),
            right_sidebar,
            footbar,
            lock_overlay,
            last_observed_locked: Some(cx.global::<lock_screen::LockState>().locked()),
            focused_terminal_view: None,
            next_terminal_id: 1,
            local_tab_label_counts: HashMap::new(),
            ssh_tab_label_counts: HashMap::new(),
            ssh_terminal_builder,
            terminal_context_menu_provider: Arc::new(TermuaContextMenuProvider),
            workspace_save_task: None,
            web_share: None,
            web_share_starting: false,
            web_share_subscription: None,
            _subscriptions: Vec::new(),
        };

        this.install_language_subscription(window, cx);
        Self::spawn_lock_state_monitor(cx);
        this.install_app_state_subscription(window, cx);
        this.install_lock_state_subscription(window, cx);
        this.install_sessions_sidebar_subscription(window, cx);
        this.install_window_appearance_subscription(window, cx);

        let sessions_sidebar_open = cx.global::<TermuaAppState>().sessions_sidebar_visible;
        let sessions_sidebar_width = cx.global::<TermuaAppState>().sessions_sidebar_width;
        let right_sidebar_open = cx.global::<right_sidebar::RightSidebarState>().visible;
        let right_sidebar_width = cx.global::<right_sidebar::RightSidebarState>().width;

        let sessions_panel = Arc::new(sessions_sidebar) as Arc<dyn PanelView>;
        let right_panel = Arc::new(this.right_sidebar.clone()) as Arc<dyn PanelView>;

        let dock_weak = dock_area.downgrade();
        dock_area.update(cx, |dock, cx| {
            // Important: make the center a StackPanel even when there's a single TabPanel.
            // This allows TabPanel to have a parent StackPanel, enabling tab drag/drop.
            let center = DockItem::v_split(
                vec![DockItem::tabs(vec![], &dock_weak, window, cx)],
                &dock_weak,
                window,
                cx,
            );
            dock.set_center(center, window, cx);

            // Termua already provides its own sidebar toggles/actions, so the DockArea's
            // title-bar toggle buttons are redundant for left/right sidebars.
            dock.set_toggle_button_visible_for(DockPlacement::Left, false, cx);
            dock.set_toggle_button_visible_for(DockPlacement::Right, false, cx);

            dock.set_left_dock(
                DockItem::panel(sessions_panel.clone()),
                Some(sessions_sidebar_width),
                sessions_sidebar_open,
                window,
                cx,
            );
            if let Some(left) = dock.left_dock().cloned() {
                left.update(cx, |dock, cx| {
                    dock.set_min_size(gpui::px(220.0), window, cx);
                    dock.set_max_size(gpui::px(400.0), window, cx);
                });
            }
            dock.set_right_dock(
                DockItem::panel(right_panel.clone()),
                Some(right_sidebar_width),
                right_sidebar_open,
                window,
                cx,
            );
            if let Some(right) = dock.right_dock().cloned() {
                right.update(cx, |dock, cx| {
                    dock.set_min_size(gpui::px(220.0), window, cx);
                    dock.set_max_size(gpui::px(400.0), window, cx);
                });
            }
        });

        let terminal_context_menu_provider = this.terminal_context_menu_provider.clone();
        register_panel(cx, crate::panel::TERMINAL_PANEL_NAME, {
            let restored_terminal_builder = restored_terminal_builder.clone();
            move |_, _, info, window, cx| {
                let panel_state = match info {
                    gpui_dock::PanelInfo::Panel(value) => {
                        serde_json::from_value::<crate::panel::TerminalPanelState>(value.clone())
                    }
                    _ => unreachable!("terminal factory received non-panel state"),
                };
                let panel_state = match panel_state {
                    Ok(state) if state.version == 1 => state,
                    Ok(state) => {
                        return Box::new(cx.new(|cx| {
                            crate::panel::SshErrorPanel::new(
                                state.id,
                                state.tab_label.into(),
                                state.tab_tooltip.map(Into::into),
                                "This saved terminal state is from an unsupported version.".into(),
                                cx,
                            )
                        }));
                    }
                    Err(err) => {
                        return Box::new(cx.new(|cx| {
                            crate::panel::SshErrorPanel::new(
                                0,
                                "Terminal".into(),
                                None,
                                format!("Failed to restore terminal state: {err}").into(),
                                cx,
                            )
                        }));
                    }
                };

                let id = panel_state.id;
                let builder = match &panel_state.launch {
                    crate::panel::TerminalLaunchState::Ssh { .. } => {
                        return Box::new(cx.new(|cx| {
                            crate::panel::SshErrorPanel::restoring(
                                panel_state,
                                "Reconnecting SSH session...".into(),
                                cx,
                            )
                        }));
                    }
                    launch => restored_terminal_builder(launch, id),
                };
                let (kind, builder) = match builder {
                    Ok(builder) => builder,
                    Err(err) => {
                        return Box::new(cx.new(|cx| {
                            crate::panel::SshErrorPanel::with_terminal_error(
                                panel_state,
                                format!("Failed to restore terminal: {err:#}").into(),
                                cx,
                            )
                        }));
                    }
                };
                let terminal = cx.new(move |cx| builder.build(cx));
                let terminal_view = if kind == crate::panel::PanelKind::Recorder {
                    cx.new(|cx| {
                        RecorderContextMenuProvider::new_terminal_view(terminal, window, cx)
                    })
                } else {
                    cx.new(|cx| {
                        TerminalView::new_with_context_menu_provider(
                            terminal,
                            window,
                            cx,
                            true,
                            Some(terminal_context_menu_provider.clone()),
                        )
                    })
                };
                Box::new(cx.new(|_| {
                    crate::panel::TerminalPanel::new_with_launch_state(
                        id,
                        kind,
                        panel_state.tab_label.into(),
                        panel_state.tab_tooltip.map(Into::into),
                        Some(panel_state.launch),
                        terminal_view,
                    )
                }))
            }
        });
        register_panel(cx, crate::panel::SFTP_PANEL_NAME, |_, _, info, _, cx| {
            let state = match info {
                gpui_dock::PanelInfo::Panel(value) => serde_json::from_value::<
                    crate::panel::sftp_panel::SftpPanelState,
                >(value.clone()),
                _ => unreachable!("sftp factory received non-panel state"),
            };
            match state {
                Ok(state) if state.version == 1 => Box::new(
                    cx.new(|cx| crate::panel::sftp_panel::SftpDockPanel::restoring(state, cx)),
                ),
                Ok(_) | Err(_) => Box::new(cx.new(|cx| {
                    crate::panel::SshErrorPanel::new(
                        0,
                        "SFTP".into(),
                        None,
                        "Failed to restore SFTP panel state.".into(),
                        cx,
                    )
                })),
            }
        });

        match crate::workspace::load_from_path(&crate::workspace::state_path()) {
            Ok(mut state) if state.version == Some(crate::workspace::STATE_VERSION) => {
                normalize_fixed_sidebar_panels(&mut state);
                if let Err(err) = dock_area.update(cx, |dock, cx| dock.load(state, window, cx)) {
                    log::warn!("termua: failed to restore dock workspace: {err:#}");
                }
            }
            Ok(state) => log::info!(
                "termua: ignoring dock workspace version {:?}, expected {}",
                state.version,
                crate::workspace::STATE_VERSION
            ),
            Err(err) if crate::workspace::state_path().exists() => {
                log::warn!("termua: failed to read dock workspace: {err:#}");
            }
            Err(_) => {}
        }
        let (left_dock, right_dock) = {
            let dock = dock_area.read(cx);
            (dock.left_dock().cloned(), dock.right_dock().cloned())
        };
        if let Some(left) = left_dock {
            left.update(cx, |dock, cx| {
                dock.set_min_size(gpui::px(220.0), window, cx);
                dock.set_max_size(gpui::px(400.0), window, cx);
            });
        }
        if let Some(right) = right_dock {
            right.update(cx, |dock, cx| {
                dock.set_min_size(gpui::px(220.0), window, cx);
                dock.set_max_size(gpui::px(400.0), window, cx);
            });
        }
        this.wire_restored_terminal_panels(window, cx);
        this.restore_pending_ssh_panels(window, cx);
        this.restore_pending_sftp_panels(window, cx);

        let (left_state, right_state) = {
            let dock = dock_area.read(cx);
            let left = dock
                .left_dock()
                .map(|left| (left.read(cx).is_open(), left.read(cx).size()));
            let right = dock
                .right_dock()
                .map(|right| (right.read(cx).is_open(), right.read(cx).size()));
            (left, right)
        };
        if let Some((visible, width)) = left_state {
            let state = cx.global_mut::<TermuaAppState>();
            state.sessions_sidebar_visible = visible;
            state.sessions_sidebar_width = width;
        }
        if let Some((visible, width)) = right_state {
            let state = cx.global_mut::<right_sidebar::RightSidebarState>();
            state.visible = visible;
            state.width = width;
        }

        this.install_workspace_persistence(window, cx);

        this
    }

    fn ensure_globals(cx: &mut Context<Self>) {
        ensure_ctx_global_with::<lock_screen::LockState, _>(
            cx,
            lock_screen::LockState::new_default,
        );
        ensure_ctx_global::<notification::NotifyState, _>(cx);
        ensure_ctx_global::<right_sidebar::RightSidebarState, _>(cx);
        crate::assistant::ensure_globals(cx);
        ensure_ctx_global::<TransferCenterState, _>(cx);
        crate::settings::ensure_language_state_with_default(crate::settings::Language::English, cx);
    }

    fn install_language_subscription(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self._subscriptions
            .push(cx.observe_global_in::<crate::settings::LanguageSettings>(
                window,
                |this, window, cx| {
                    this.lock_overlay.sync_localized_placeholders(window, cx);
                    cx.notify();
                    window.refresh();
                },
            ));
    }

    fn spawn_lock_state_monitor(cx: &mut Context<Self>) {
        if !cx
            .global_mut::<lock_screen::LockState>()
            .start_monitor_once()
        {
            return;
        }

        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let _ = this.update(cx, |_this, cx| {
                    if cx.global::<lock_screen::LockState>().should_lock()
                        && cx.global_mut::<lock_screen::LockState>().lock_now()
                    {
                        cx.refresh_windows();
                    }
                });
            }
        })
        .detach();
    }

    fn install_app_state_subscription(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self._subscriptions
            .push(
                cx.observe_global_in::<TermuaAppState>(window, |this, window, cx| {
                    this.process_pending_commands(window, cx);
                    cx.notify();
                    window.refresh();
                }),
            );
    }

    fn install_lock_state_subscription(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self._subscriptions
            .push(
                cx.observe_global_in::<lock_screen::LockState>(window, |this, window, cx| {
                    let locked = cx.global::<lock_screen::LockState>().locked();
                    if this.last_observed_locked == Some(locked) {
                        return;
                    }
                    this.last_observed_locked = Some(locked);

                    cx.notify();
                    window.refresh();

                    if locked {
                        if let Some(server) = this.web_share.take() {
                            server.shutdown();
                        }
                        this.web_share_starting = false;
                        this.web_share_subscription = None;
                        this.lock_overlay.password_input.update(cx, |state, cx| {
                            state.set_masked(true, window, cx);
                        });
                        let focus = this.lock_overlay.password_input.read(cx).focus_handle(cx);
                        window.defer(cx, move |window, cx| window.focus(&focus, cx));
                    } else {
                        this.lock_overlay.error = None;
                    }
                }),
            );
    }

    fn install_sessions_sidebar_subscription(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let sessions_sidebar = self.sessions_sidebar.clone();
        self._subscriptions.push(cx.subscribe_in(
            &sessions_sidebar,
            window,
            |this, _sidebar, ev: &SessionsSidebarEvent, window, cx| {
                let SessionsSidebarEvent::OpenSession(id) = ev;
                cx.global::<lock_screen::LockState>().report_activity();
                this.open_session_by_id(*id, window, cx);
            },
        ));
    }

    fn install_window_appearance_subscription(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self._subscriptions
            .push(cx.observe_window_appearance(window, |_, window, cx| {
                if theme_mode(cx) == ThemeMode::System {
                    set_theme_mode(ThemeMode::System, Some(window), cx);
                }
            }));
    }

    fn install_workspace_persistence(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let dock_area = self.dock_area.clone();
        self._subscriptions.push(cx.subscribe_in(
            &dock_area,
            window,
            |this, dock_area, event: &DockEvent, window, cx| {
                if matches!(event, DockEvent::LayoutChanged) {
                    this.schedule_workspace_save(dock_area.clone(), window, cx);
                }
            },
        ));

        let dock_area = self.dock_area.clone();
        cx.on_app_quit(move |_, cx| {
            let state = dock_area.read(cx).dump(cx);
            let path = crate::workspace::state_path();
            cx.background_executor().spawn(async move {
                if let Err(err) = crate::workspace::save_to_path(&path, state) {
                    log::warn!("termua: failed to save dock workspace on quit: {err:#}");
                }
            })
        })
        .detach();
    }

    fn schedule_workspace_save(
        &mut self,
        dock_area: gpui::Entity<DockArea>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.workspace_save_task = Some(cx.spawn_in(window, async move |view, window| {
            window
                .background_executor()
                .timer(WORKSPACE_SAVE_DEBOUNCE)
                .await;
            let Ok(state) = view.update_in(window, move |_, _, cx| dock_area.read(cx).dump(cx))
            else {
                return;
            };
            let path = crate::workspace::state_path();
            if let Err(err) = window
                .background_executor()
                .spawn(async move { crate::workspace::save_to_path(&path, state) })
                .await
            {
                log::warn!("termua: failed to save dock workspace: {err:#}");
            }
        }));
    }
}
