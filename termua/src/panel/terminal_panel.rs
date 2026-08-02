use std::{collections::HashMap, path::PathBuf, time::Duration};

use gpui::{
    AnyElement, App, Context, ExternalPaths, FocusHandle, InteractiveElement, IntoElement,
    KeyDownEvent, MouseButton, ParentElement, Pixels, Render, SharedString, Styled, Window, div,
    prelude::FluentBuilder, px,
};
use gpui_common::TermuaIcon;
use gpui_component::{ActiveTheme, scroll::ScrollableElement};
use gpui_dock::{Panel, PanelEvent, PanelInfo, PanelState};
use gpui_term::{TerminalMode, TerminalShutdownPolicy, TerminalView};

use crate::notification::{self, MessageKind};

#[derive(Clone, Debug)]
struct PendingSftpUpload {
    paths: Vec<PathBuf>,
}

fn collect_dropped_upload_paths(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut upload_paths: Vec<PathBuf> = paths
        .iter()
        .filter(|path| path.is_file())
        .cloned()
        .collect();
    upload_paths.sort();
    upload_paths
}

fn supports_sftp_file_drop(kind: PanelKind, has_sftp: bool, paths: &[PathBuf]) -> bool {
    kind == PanelKind::Ssh
        && has_sftp
        && !paths.is_empty()
        && paths.iter().all(|path| path.is_file())
}

fn sftp_upload_file_count_label(count: usize) -> String {
    match count {
        1 => "1 file".to_string(),
        n => format!("{n} files"),
    }
}

fn sftp_upload_destination_label(current_dir: Option<&str>) -> String {
    match current_dir {
        Some(path) if !path.trim().is_empty() => format!("Destination: {path}"),
        _ => "Destination: current remote directory".to_string(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PanelKind {
    Local,
    Ssh,
    Serial,
    Recorder,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum TerminalLaunchState {
    Local {
        backend_type: gpui_term::TerminalType,
        env: HashMap<String, String>,
    },
    Ssh {
        backend_type: gpui_term::TerminalType,
        session_id: Option<i64>,
    },
    Serial {
        backend_type: gpui_term::TerminalType,
        params: crate::SerialParams,
        session_id: Option<i64>,
    },
    Recorder {
        cast_path: PathBuf,
        playback_speed: f64,
    },
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct TerminalPanelState {
    pub(crate) version: usize,
    pub(crate) id: usize,
    pub(crate) tab_label: String,
    pub(crate) tab_tooltip: Option<String>,
    pub(crate) launch: TerminalLaunchState,
}

pub(crate) fn terminal_panel_tab_name(kind: PanelKind, id: usize) -> SharedString {
    match kind {
        PanelKind::Local => format!("local {id}").into(),
        PanelKind::Ssh => format!("ssh {id}").into(),
        PanelKind::Serial => format!("serial {id}").into(),
        PanelKind::Recorder => format!("recorder {id}").into(),
    }
}

pub(crate) fn local_terminal_panel_tab_name(
    env: &HashMap<String, String>,
    id: usize,
    counts: &mut HashMap<String, usize>,
) -> SharedString {
    let Some(base) = local_shell_display_name_from_env(env) else {
        return terminal_panel_tab_name(PanelKind::Local, id);
    };

    let count = counts.entry(base.clone()).or_insert(0);
    *count += 1;

    if *count == 1 {
        base.into()
    } else {
        format!("{base} {count}").into()
    }
}

pub(crate) fn local_shell_display_name_from_env(env: &HashMap<String, String>) -> Option<String> {
    gpui_term::shell::pick_shell_program_from_env(env)
        .map(gpui_term::shell::shell_display_name)
        .filter(|name| !name.trim().is_empty())
}

pub(crate) fn shell_icon_for_program(program: Option<&str>) -> TermuaIcon {
    match program.map(gpui_term::shell::shell_kind) {
        Some(gpui_term::shell::ShellKind::Fish) => TermuaIcon::Fish,
        Some(gpui_term::shell::ShellKind::Nu) => TermuaIcon::Nushell,
        Some(gpui_term::shell::ShellKind::Pwsh | gpui_term::shell::ShellKind::PowerShell) => {
            TermuaIcon::Pwsh
        }
        _ => TermuaIcon::Terminal,
    }
}

pub(crate) fn tab_icon_for_terminal_panel(kind: PanelKind) -> gpui_dock::TabIcon {
    match kind {
        PanelKind::Recorder => gpui_dock::TabIcon::Monochrome {
            path: TermuaIcon::Record.into(),
            color: Some(gpui::red()),
        },
        PanelKind::Local => gpui_dock::TabIcon::Monochrome {
            path: TermuaIcon::Terminal.into(),
            color: None,
        },
        PanelKind::Ssh => gpui_dock::TabIcon::Monochrome {
            path: TermuaIcon::Ssh.into(),
            color: None,
        },
        PanelKind::Serial => gpui_dock::TabIcon::Monochrome {
            path: TermuaIcon::Usb.into(),
            color: None,
        },
    }
}

pub(crate) fn tab_icon_for_terminal_panel_with_launch(
    kind: PanelKind,
    launch_state: Option<&TerminalLaunchState>,
) -> gpui_dock::TabIcon {
    if kind == PanelKind::Local {
        let program = match launch_state {
            Some(TerminalLaunchState::Local { env, .. }) => {
                gpui_term::shell::pick_shell_program_from_env(env)
            }
            _ => None,
        };
        return gpui_dock::TabIcon::Monochrome {
            path: shell_icon_for_program(program).into(),
            color: None,
        };
    }

    tab_icon_for_terminal_panel(kind)
}

pub(crate) struct TerminalPanel {
    id: usize,
    kind: PanelKind,
    tab_label: SharedString,
    tab_tooltip: Option<SharedString>,
    launch_state: Option<TerminalLaunchState>,
    terminal_view: gpui::Entity<TerminalView>,
    pending_sftp_upload: Option<PendingSftpUpload>,
}

impl TerminalPanel {
    #[cfg(test)]
    pub(crate) fn new(
        id: usize,
        kind: PanelKind,
        tab_label: SharedString,
        tab_tooltip: Option<SharedString>,
        terminal_view: gpui::Entity<TerminalView>,
    ) -> Self {
        Self::new_with_launch_state(id, kind, tab_label, tab_tooltip, None, terminal_view)
    }

    pub(crate) fn new_with_launch_state(
        id: usize,
        kind: PanelKind,
        tab_label: SharedString,
        tab_tooltip: Option<SharedString>,
        launch_state: Option<TerminalLaunchState>,
        terminal_view: gpui::Entity<TerminalView>,
    ) -> Self {
        Self {
            id,
            kind,
            tab_label,
            tab_tooltip,
            launch_state,
            terminal_view,
            pending_sftp_upload: None,
        }
    }

    pub(crate) fn id(&self) -> usize {
        self.id
    }

    pub(crate) fn kind(&self) -> PanelKind {
        self.kind
    }

    pub(crate) fn terminal_view(&self) -> gpui::Entity<TerminalView> {
        self.terminal_view.clone()
    }

    pub(crate) fn tab_label(&self) -> SharedString {
        self.tab_label.clone()
    }

    pub(crate) fn local_shell_display_name(&self) -> Option<String> {
        match self.launch_state.as_ref() {
            Some(TerminalLaunchState::Local { env, .. }) => local_shell_display_name_from_env(env),
            _ => None,
        }
    }

    pub(crate) fn cleanup_runtime_state<T>(id: usize, cx: &mut Context<T>) {
        crate::assistant::unregister_terminal_target(cx, id);
        crate::footbar::blur_terminal_backend(id, cx);
    }

    fn terminal_has_sftp(&self, cx: &App) -> bool {
        self.terminal_view
            .read(cx)
            .terminal
            .read(cx)
            .sftp()
            .is_some()
    }

    fn current_remote_dir(&self, cx: &App) -> Option<String> {
        self.terminal_view.read(cx).terminal.read(cx).current_dir()
    }

    fn notify(
        &self,
        kind: MessageKind,
        title: &str,
        detail: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let message = match detail {
            Some(detail) if !detail.trim().is_empty() => format!("{title}\n{detail}"),
            _ => title.to_string(),
        };
        notification::notify_deferred(kind, message, window, cx);
    }

    fn handle_sftp_file_drop(
        &mut self,
        paths: &ExternalPaths,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let upload_paths = collect_dropped_upload_paths(paths.paths());
        if upload_paths.is_empty() {
            self.notify(
                MessageKind::Info,
                "Only files are supported",
                Some("Dropped items did not include any files."),
                window,
                cx,
            );
            return;
        }

        if self.kind != PanelKind::Ssh || !self.terminal_has_sftp(cx) {
            return;
        }

        let terminal = self.terminal_view.read(cx).terminal.clone();
        let terminal = terminal.read(cx);

        if terminal
            .last_content()
            .mode
            .contains(TerminalMode::ALT_SCREEN)
        {
            self.notify(
                MessageKind::Warning,
                "Exit the full-screen app first",
                Some("Upload requires a shell prompt (ALT_SCREEN is active)."),
                window,
                cx,
            );
            return;
        }

        let _ = terminal;

        self.pending_sftp_upload = Some(PendingSftpUpload {
            paths: upload_paths,
        });
        cx.notify();
    }

    fn cancel_sftp_file_drop_upload(&mut self, cx: &mut Context<Self>) {
        if self.pending_sftp_upload.take().is_some() {
            cx.notify();
        }
    }

    fn confirm_sftp_file_drop_upload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.pending_sftp_upload.take() else {
            return;
        };

        let has_sftp = self.terminal_has_sftp(cx);
        if !has_sftp {
            self.notify(
                MessageKind::Error,
                "SFTP is unavailable",
                Some("This SSH terminal no longer has an active SFTP session."),
                window,
                cx,
            );
            cx.notify();
            return;
        }

        self.terminal_view.update(cx, |terminal_view, cx| {
            terminal_view.terminal.update(cx, |terminal, cx| {
                terminal.start_sftp_upload(dialog.paths, cx);
            });
        });
        cx.notify();
    }

    fn handle_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.pending_sftp_upload.is_none() {
            return;
        }

        match event.keystroke.key.as_str() {
            "escape" => {
                self.cancel_sftp_file_drop_upload(cx);
                cx.stop_propagation();
            }
            "enter" => {
                self.confirm_sftp_file_drop_upload(window, cx);
                cx.stop_propagation();
            }
            _ => {}
        }
    }

    fn render_pending_sftp_upload_overlay(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let dialog = self.pending_sftp_upload.clone()?;
        let destination = sftp_upload_destination_label(self.current_remote_dir(cx).as_deref());
        let file_count = sftp_upload_file_count_label(dialog.paths.len());
        let theme = cx.theme();
        let viewport = window.viewport_size();
        let panel_w = px(680.0)
            .min((viewport.width - px(24.0)).max(Pixels::ZERO))
            .max(px(360.0).min(viewport.width.max(Pixels::ZERO)));
        let row_bg = theme.muted.opacity(0.2);
        let backdrop = theme.overlay.opacity(0.35);
        let panel_bg = theme.popover.opacity(0.98);
        let panel_border = theme.border.opacity(0.9);
        let hint_fg = theme.muted_foreground;
        let accent = theme.accent;
        let accent_fg = theme.accent_foreground;

        let mut list = div()
            .mt(px(10.0))
            .h(px(280.0))
            .rounded_md()
            .border_1()
            .border_color(panel_border)
            .bg(theme.background.opacity(0.05))
            .p(px(8.0))
            .overflow_y_scrollbar();

        for path in &dialog.paths {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .to_string();
            list = list.child(
                div()
                    .bg(row_bg)
                    .rounded_md()
                    .p(px(8.0))
                    .mb(px(6.0))
                    .child(div().text_sm().child(name))
                    .child(
                        div()
                            .mt(px(2.0))
                            .text_xs()
                            .text_color(hint_fg)
                            .whitespace_normal()
                            .child(path.display().to_string()),
                    ),
            );
        }

        Some(
            div()
                .id("termua-terminal-panel-sftp-drop")
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .bottom_0()
                .bg(backdrop)
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.cancel_sftp_file_drop_upload(cx);
                        cx.stop_propagation();
                    }),
                )
                .child(
                    div()
                        .w(panel_w)
                        .max_w(px(720.0))
                        .bg(panel_bg)
                        .text_color(theme.popover_foreground)
                        .border_1()
                        .border_color(panel_border)
                        .rounded_lg()
                        .shadow_lg()
                        .p(px(14.0))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|_, _, _, cx| cx.stop_propagation()),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(div().text_sm().child("Upload via SFTP"))
                                .child(
                                    div()
                                        .cursor_pointer()
                                        .rounded_md()
                                        .w(px(34.0))
                                        .h(px(34.0))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .bg(theme.muted.opacity(0.25))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _, _, cx| {
                                                this.cancel_sftp_file_drop_upload(cx);
                                                cx.stop_propagation();
                                            }),
                                        )
                                        .child(gpui_component::Icon::new(
                                            gpui_component::IconName::Close,
                                        )),
                                ),
                        )
                        .child(
                            div()
                                .mt(px(8.0))
                                .text_xs()
                                .text_color(hint_fg)
                                .child(destination),
                        )
                        .child(list)
                        .child(
                            div()
                                .mt(px(10.0))
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(hint_fg)
                                        .child(format!("{file_count}  Press Enter to upload")),
                                )
                                .child(
                                    div()
                                        .cursor_pointer()
                                        .rounded_md()
                                        .bg(accent)
                                        .text_color(accent_fg)
                                        .w(px(38.0))
                                        .h(px(38.0))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(|this, _, window, cx| {
                                                this.confirm_sftp_file_drop_upload(window, cx);
                                                cx.stop_propagation();
                                            }),
                                        )
                                        .child(gpui_component::Icon::new(
                                            gpui_component::IconName::ArrowUp,
                                        )),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }
}

impl Drop for TerminalPanel {
    fn drop(&mut self) {
        log::debug!("termua: TerminalPanel drop (id={})", self.id);
    }
}

impl gpui::EventEmitter<PanelEvent> for TerminalPanel {}

impl gpui::Focusable for TerminalPanel {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.terminal_view.read(cx).focus_handle.clone()
    }
}

impl Panel for TerminalPanel {
    fn panel_name(&self) -> &'static str {
        super::TERMINAL_PANEL_NAME
    }

    fn tab_icon(&self, _cx: &App) -> Option<gpui_dock::TabIcon> {
        Some(tab_icon_for_terminal_panel_with_launch(
            self.kind,
            self.launch_state.as_ref(),
        ))
    }

    fn set_active(&mut self, active: bool, _window: &mut Window, cx: &mut Context<Self>) {
        if active {
            let backend = self.terminal_view.read(cx).terminal.read(cx).backend_type();
            crate::footbar::focus_terminal_backend(self.id, backend, cx);
        } else {
            crate::footbar::blur_terminal_backend(self.id, cx);
        }
    }

    fn on_removed(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        // This may run during tab drag/drop (detach/attach), so it must not terminate the session.
        log::debug!("termua: TerminalPanel on_removed (id={})", self.id);
    }

    fn on_close(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        log::debug!(
            "termua: TerminalPanel on_close (id={}), requesting terminal shutdown",
            self.id
        );

        Self::cleanup_runtime_state(self.id, cx);

        // Ensure the backend releases its PTY/process resources when the tab is explicitly closed.
        self.terminal_view.update(cx, |terminal_view, cx| {
            terminal_view.terminal.update(cx, |terminal, cx| {
                terminal.shutdown(
                    TerminalShutdownPolicy::GracefulThenKill(Duration::from_secs(3)),
                    cx,
                );
            });
        });
    }

    fn tab_name(&self, _cx: &App) -> Option<SharedString> {
        Some(self.tab_label.clone())
    }

    fn tab_tooltip(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<impl IntoElement> {
        let tooltip = self.tab_tooltip.clone()?;
        Some(div().child(tooltip))
    }

    fn title(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.tab_name(cx).unwrap_or_else(|| "local".into())
    }

    fn dump(&self, _cx: &App) -> PanelState {
        let mut state = PanelState::new(self);
        let Some(launch) = self.launch_state.clone() else {
            return state;
        };
        let panel_state = TerminalPanelState {
            version: 1,
            id: self.id,
            tab_label: self.tab_label.to_string(),
            tab_tooltip: self.tab_tooltip.as_ref().map(ToString::to_string),
            launch,
        };
        state.info = PanelInfo::panel(
            serde_json::to_value(panel_state).expect("terminal panel state should serialize"),
        );
        state
    }
}

impl Render for TerminalPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let kind = self.kind;
        let terminal_view = self.terminal_view.clone();
        div()
            .id("termua-terminal-panel")
            .size_full()
            .relative()
            .can_drop(move |any, _window, cx| {
                let has_sftp = terminal_view.read(cx).terminal.read(cx).sftp().is_some();
                any.downcast_ref::<ExternalPaths>()
                    .is_some_and(|paths| supports_sftp_file_drop(kind, has_sftp, paths.paths()))
            })
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                this.handle_sftp_file_drop(paths, window, cx);
            }))
            .on_key_down(cx.listener(Self::handle_key_down))
            .child(self.terminal_view.clone())
            .when_some(
                self.render_pending_sftp_upload_overlay(window, cx),
                |this, overlay| this.child(overlay),
            )
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
    };

    use super::*;

    fn unique_tmp_path(label: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("termua-terminal-panel-{label}-{nanos}"))
    }

    fn touch(path: &Path) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent dirs");
        }
        fs::write(path, b"test").expect("create file");
    }

    #[test]
    fn recorder_terminal_tab_icon_path_is_record_svg() {
        assert!(matches!(
            tab_icon_for_terminal_panel(PanelKind::Recorder),
            gpui_dock::TabIcon::Monochrome { path, color }
                if path.as_ref() == TermuaIcon::Record.path() && color.is_some()
        ));
    }

    #[test]
    fn local_terminal_tab_icons_follow_shell_program() {
        for (program, expected_icon) in [
            ("bash", TermuaIcon::Terminal),
            ("zsh", TermuaIcon::Terminal),
            ("fish", TermuaIcon::Fish),
            ("nu", TermuaIcon::Nushell),
            ("pwsh", TermuaIcon::Pwsh),
            ("powershell", TermuaIcon::Pwsh),
            ("cmd", TermuaIcon::Terminal),
        ] {
            let launch = TerminalLaunchState::Local {
                backend_type: gpui_term::TerminalType::WezTerm,
                env: HashMap::from([("TERMUA_SHELL".to_string(), program.to_string())]),
            };
            assert!(matches!(
                tab_icon_for_terminal_panel_with_launch(PanelKind::Local, Some(&launch)),
                gpui_dock::TabIcon::Monochrome { path, color: None }
                    if path.as_ref() == expected_icon.path()
            ));
        }
    }

    #[test]
    fn recorder_tabs_use_recorder_prefix() {
        assert_eq!(
            terminal_panel_tab_name(PanelKind::Local, 7).as_ref(),
            "local 7"
        );
        assert_eq!(
            terminal_panel_tab_name(PanelKind::Recorder, 7).as_ref(),
            "recorder 7"
        );
    }

    #[test]
    fn terminal_panel_state_round_trips_local_launch_parameters() {
        let state = TerminalPanelState {
            version: 1,
            id: 7,
            tab_label: "bash".to_string(),
            tab_tooltip: None,
            launch: TerminalLaunchState::Local {
                backend_type: gpui_term::TerminalType::WezTerm,
                env: HashMap::from([("TERMUA_SHELL".to_string(), "bash".to_string())]),
            },
        };

        let json = serde_json::to_value(&state).expect("serialize terminal panel state");
        let restored: TerminalPanelState =
            serde_json::from_value(json).expect("deserialize terminal panel state");

        assert_eq!(restored, state);
    }

    #[test]
    fn local_tabs_fall_back_to_local_prefix_without_shell() {
        let mut counts = HashMap::new();
        assert_eq!(
            local_terminal_panel_tab_name(&HashMap::new(), 7, &mut counts).as_ref(),
            "local 7"
        );
    }

    #[test]
    fn duplicate_local_shell_tabs_append_shell_sequence() {
        let mut counts = HashMap::new();
        let mut env = HashMap::new();
        env.insert("TERMUA_SHELL".into(), "bash".into());

        assert_eq!(
            local_terminal_panel_tab_name(&env, 7, &mut counts).as_ref(),
            "bash"
        );
        assert_eq!(
            local_terminal_panel_tab_name(&env, 9, &mut counts).as_ref(),
            "bash 2"
        );
    }

    #[test]
    fn dropped_upload_paths_only_keep_files_and_sort() {
        let base = unique_tmp_path("drop-paths");
        let dir_path = base.join("dir");
        let b_path = base.join("b.txt");
        let a_path = base.join("nested").join("a.txt");
        fs::create_dir_all(&dir_path).expect("create dir");
        touch(&b_path);
        touch(&a_path);

        let paths = collect_dropped_upload_paths(&[dir_path, b_path.clone(), a_path.clone()]);

        assert_eq!(paths, vec![b_path, a_path]);

        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn ssh_panel_drop_support_requires_sftp_files_only() {
        let base = unique_tmp_path("drop-accept");
        let dir_path = base.join("dir");
        let file_path = base.join("file.txt");
        fs::create_dir_all(&dir_path).expect("create dir");
        touch(&file_path);

        assert!(supports_sftp_file_drop(
            PanelKind::Ssh,
            true,
            std::slice::from_ref(&file_path)
        ));
        assert!(!supports_sftp_file_drop(
            PanelKind::Local,
            true,
            std::slice::from_ref(&file_path)
        ));
        assert!(!supports_sftp_file_drop(
            PanelKind::Ssh,
            false,
            std::slice::from_ref(&file_path)
        ));
        assert!(!supports_sftp_file_drop(
            PanelKind::Ssh,
            true,
            std::slice::from_ref(&dir_path)
        ));

        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn sftp_upload_file_count_label_handles_pluralization() {
        assert_eq!(sftp_upload_file_count_label(1), "1 file");
        assert_eq!(sftp_upload_file_count_label(2), "2 files");
    }

    #[test]
    fn sftp_upload_destination_label_uses_fallback_when_unknown() {
        assert_eq!(
            sftp_upload_destination_label(None),
            "Destination: current remote directory"
        );
        assert_eq!(
            sftp_upload_destination_label(Some("")),
            "Destination: current remote directory"
        );
        assert_eq!(
            sftp_upload_destination_label(Some("/srv/app")),
            "Destination: /srv/app"
        );
    }
}
