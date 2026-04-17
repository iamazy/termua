//! TermuaWindow state and construction.

use std::{collections::HashMap, sync::Arc, time::Duration};

use gpui::{App, AppContext, ClipboardItem, Context, Focusable, Styled, Subscription, Window};
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
    sharing::{
        ReleaseControl, RequestControl, RevokeControl, StartSharing, StopSharing,
        host_controller_present, host_sharing, is_remote_terminal, viewer_can_copy_paste,
        viewer_controlled, viewer_sharing,
    },
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

        let has_sftp = terminal.read(cx).sftp().is_some();

        let terminal_view_id = terminal_view.entity_id();
        let is_remote = is_remote_terminal(&terminal, cx);
        let is_viewer = viewer_sharing(terminal_view_id, cx);
        let is_host = host_sharing(terminal_view_id, cx);
        let viewer_has_control = viewer_controlled(terminal_view_id, cx);
        let sharing_enabled = crate::sharing::sharing_feature_enabled(cx);

        // Viewer terminals should only expose sharing-related actions in the context menu.
        if is_viewer {
            let mut menu = menu;
            if viewer_has_control {
                menu = menu
                    .menu_with_icon("Copy", IconName::Copy, Box::new(CopyAction))
                    .menu("Paste", Box::new(Paste))
                    .separator();
            }

            menu = if viewer_has_control {
                menu.item(
                    gpui_component::menu::PopupMenuItem::new("Release Control")
                        .action(Box::new(ReleaseControl)),
                )
            } else {
                menu.item(
                    gpui_component::menu::PopupMenuItem::new("Request Control")
                        .action(Box::new(RequestControl)),
                )
            };

            return menu;
        }

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
                gpui_component::menu::PopupMenuItem::new("Recording")
                    .icon(record_icon)
                    .checked(recording_active)
                    .action(Box::new(ToggleCastRecording)),
            )
            .separator();

        if !is_remote || viewer_can_copy_paste(&terminal, terminal_view_id, cx) {
            menu = menu
                .menu_with_icon("Copy", IconName::Copy, Box::new(CopyAction))
                .menu("Paste", Box::new(Paste))
                .separator()
                .menu("SelectAll", Box::new(SelectAll))
                .separator()
                .menu("Clear", Box::new(Clear));
        } else {
            // Viewer (not controlling): hide copy/paste/select/clear per sharing UX rules.
            menu = menu.menu("Clear", Box::new(Clear));
        }

        // Sharing / Control (mutually exclusive entries by role/state).
        let show_sharing_section = is_host || (!is_remote && sharing_enabled);
        if show_sharing_section {
            menu = menu.separator();

            if is_host {
                menu = menu.item(
                    gpui_component::menu::PopupMenuItem::new("Stop Sharing")
                        .action(Box::new(StopSharing)),
                );
                if host_controller_present(terminal_view_id, cx) {
                    menu = menu.item(
                        gpui_component::menu::PopupMenuItem::new("Revoke Control")
                            .action(Box::new(RevokeControl)),
                    );
                }
            } else if !is_remote && sharing_enabled {
                menu = menu.item(
                    gpui_component::menu::PopupMenuItem::new("Sharing")
                        .action(Box::new(StartSharing)),
                );
            }
        }

        if is_viewer {
            if viewer_has_control {
                menu = menu.item(
                    gpui_component::menu::PopupMenuItem::new("Release Control")
                        .action(Box::new(ReleaseControl)),
                );
            } else {
                menu = menu.item(
                    gpui_component::menu::PopupMenuItem::new("Request Control")
                        .action(Box::new(RequestControl)),
                );
            }
        }

        if !cfg!(debug_assertions) {
            return menu;
        }

        let Some(block) = command_block_at_current_selection(&terminal, cx) else {
            return menu;
        };

        let Some(output_end) = block.output_end_line else {
            // The block is still running (no end marker yet). Skip export actions.
            return menu;
        };
        let mut output_start = block.output_start_line;

        const MAX_EXPORT_LINES: i64 = 2_000;
        if output_end.saturating_sub(output_start).saturating_add(1) > MAX_EXPORT_LINES {
            output_start = output_end.saturating_sub(MAX_EXPORT_LINES.saturating_sub(1));
        }

        menu = menu
            .separator()
            .item(
                gpui_component::menu::PopupMenuItem::new(
                    t!("MainWindow.ContextMenu.CopyCommandBlockOutput").to_string(),
                )
                .on_click({
                    let terminal = terminal.clone();
                    move |_, _window, cx| {
                        let text = terminal
                            .read(cx)
                            .text_for_lines(output_start, output_end)
                            .unwrap_or_default();
                        terminal.update(cx, |_terminal, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(text));
                        });
                    }
                }),
            )
            .item(
                gpui_component::menu::PopupMenuItem::new(
                    t!("MainWindow.ContextMenu.CopyCommandBlockId").to_string(),
                )
                .on_click({
                    let id_text = format!("block_id={}", block.id);
                    move |_, _window, cx| {
                        terminal.update(cx, |_terminal, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(id_text.clone()));
                        });
                    }
                }),
            );

        menu
    }
}

fn command_block_at_current_selection(
    terminal: &gpui::Entity<gpui_term::Terminal>,
    cx: &gpui::App,
) -> Option<gpui_term::command_blocks::CommandBlock> {
    let (stable, blocks) = {
        let terminal = terminal.read(cx);
        let selection_start_line = terminal.last_content().selection.as_ref()?.start.line;
        let stable = terminal.stable_row_for_grid_line(selection_start_line)?;
        let blocks = terminal.command_blocks()?;
        Some((stable, blocks))
    }?;
    blocks.into_iter().rev().find(|b| match b.output_end_line {
        Some(end) => stable >= b.output_start_line && stable <= end,
        None => stable >= b.output_start_line,
    })
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
        Self::spawn_terminal_context_poll(cx);
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
                    dock.set_min_size(gpui::px(220.0), window, cx)
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
                    dock.set_min_size(gpui::px(320.0), window, cx)
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

    fn spawn_terminal_context_poll(cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(
                        crate::assistant::DEFAULT_TERMINAL_CONTEXT_POLL_INTERVAL_MS,
                    ))
                    .await;

                let _ = this.update(cx, |_this, cx| {
                    Self::poll_terminal_context_snapshots(cx);
                });
            }
        })
        .detach();
    }

    fn poll_terminal_context_snapshots(cx: &mut Context<Self>) {
        crate::assistant::poll_terminal_context_snapshots(cx);
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

#[cfg(test)]
mod command_block_context_menu_tests {
    #[test]
    fn command_block_menu_items_are_debug_only_and_no_select_action() {
        // Source-level guardrail: the command-block context menu entries are a debug-only
        // developer feature, and we should not expose a "select block" action.
        //
        // This lives here because the context menu builder depends on GPUI window/app wiring
        // that isn't trivial to instantiate in unit tests.
        let src = include_str!("state.rs");

        let select_label = ["Select", " command", " block"].concat();
        let select_item = format!("PopupMenuItem::new(\"{}\")", select_label);
        assert!(
            !src.contains(&select_item),
            "unexpected command-block select action is still present"
        );

        let debug_gate = "cfg!(debug_assertions)";
        let gate_pos = src
            .find(debug_gate)
            .expect("expected a debug-assertions gate for command-block context menu items");

        let copy_output_item = "t!(\"MainWindow.ContextMenu.CopyCommandBlockOutput\")";
        let copy_output_pos = src
            .find(&copy_output_item)
            .expect("expected a command-block output copy action");
        assert!(
            gate_pos < copy_output_pos,
            "debug-assertions gate should appear before command-block actions"
        );

        assert!(
            {
                let hint_label = ["Command blocks", " (no block", " at cursor)"].concat();
                let hint_item = format!("PopupMenuItem::new(\"{}\")", hint_label);
                !src.contains(&hint_item)
            },
            "no-block hint menu item should not exist"
        );
    }
}
