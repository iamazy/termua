//! TermuaWindow state and construction.

use std::{collections::HashMap, sync::Arc, time::Duration};

use gpui::{App, AppContext, Context, Focusable, Styled, Subscription, Window};
use gpui_common::TermuaIcon;
use gpui_component::{ActiveTheme, Icon, IconName};
use gpui_dock::{DockArea, DockItem, DockPlacement, PanelView};
use gpui_term::{
    Clear, Copy as CopyAction, CursorShape, Paste, PtySource, SelectAll, SshOptions,
    TerminalBuilder, TerminalType, TerminalView, ToggleCastRecording,
};
use gpui_transfer::TransferCenterState;
use rust_i18n::t;

use crate::{
    OpenSftp, TermuaAppState,
    footbar::FootbarView,
    globals::{ensure_ctx_global, ensure_ctx_global_with},
    lock_screen, notification,
    panel::{RightSidebarView, SessionsSidebarEvent, SessionsSidebarView},
    right_sidebar,
    settings::{ThemeMode, set_theme_mode, theme_mode},
    ssh::SshTerminalBuilderFn,
};
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
    pub(super) _subscriptions: Vec<Subscription>,
}

struct TermuaContextMenuProvider;

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
            },
        );

        Self::new_with_ssh_terminal_builder(window, ssh_terminal_builder, cx)
    }

    pub(crate) fn new_with_ssh_terminal_builder(
        window: &mut Window,
        ssh_terminal_builder: SshTerminalBuilderFn,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::ensure_globals(cx);

        let dock_area = cx.new(|cx| DockArea::new("termua", None, window, cx));
        let sessions_sidebar = cx.new(|cx| SessionsSidebarView::new(window, cx));
        let right_sidebar = cx.new(|cx| RightSidebarView::new(window, cx));
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
                    dock.set_min_size(gpui::px(320.0), window, cx);
                    dock.set_max_size(gpui::px(400.0), window, cx);
                });
            }
        });

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
}
