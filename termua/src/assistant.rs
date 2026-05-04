use std::{
    borrow::{Borrow, BorrowMut},
    collections::HashMap,
};

use gpui::{App, AppContext, Context, SharedString};
use gpui_term::Terminal;

pub const ASSISTANT_SYSTEM_PROMPT: &str = r#"You are a terminal assistant embedded in a GUI app.

Rules:
- Do NOT output tool-call annotations like `to=functions.exec_command`, `to=shell`, JSON tool payloads, or any "agent framework" metadata.
- When suggesting terminal commands, put them in a single fenced code block (```sh ... ```).
- Otherwise, reply in plain text. Keep it concise."#;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssistantRole {
    User,
    Assistant,
    System,
}

#[derive(Clone, Debug)]
pub struct AssistantMessage {
    pub role: AssistantRole,
    pub content: SharedString,
}

pub struct AssistantState {
    pub messages: Vec<AssistantMessage>,
    pub in_flight: bool,
    pub active_request_id: Option<u64>,
    next_request_id: u64,
    pub target_panel_id: Option<usize>,
    pub target_follows_focus: bool,
    pub attach_selection: bool,
}

impl Default for AssistantState {
    fn default() -> Self {
        Self {
            messages: Vec::new(),
            in_flight: false,
            active_request_id: None,
            next_request_id: 1,
            target_panel_id: None,
            target_follows_focus: true,
            attach_selection: false,
        }
    }
}

impl gpui::Global for AssistantState {}

impl AssistantState {
    pub fn clear(&mut self) {
        self.messages.clear();
        self.in_flight = false;
        self.active_request_id = None;
    }

    pub fn push(&mut self, role: AssistantRole, content: impl Into<SharedString>) {
        let s: SharedString = content.into();
        let trimmed = s.as_ref().trim();
        if trimmed.is_empty() {
            return;
        }
        self.messages.push(AssistantMessage {
            role,
            content: trimmed.to_string().into(),
        });
    }

    pub fn begin_request(&mut self) -> u64 {
        let id = self.next_request_id;
        self.next_request_id = self.next_request_id.saturating_add(1);
        self.in_flight = true;
        self.active_request_id = Some(id);
        id
    }

    pub fn finish_request(&mut self, request_id: u64) -> bool {
        if self.active_request_id != Some(request_id) {
            return false;
        }
        self.in_flight = false;
        self.active_request_id = None;
        true
    }

    pub fn cancel_request(&mut self) -> bool {
        if !self.in_flight {
            return false;
        }
        self.in_flight = false;
        self.active_request_id = None;
        true
    }
}

#[derive(Default)]
pub(crate) struct FocusedTerminalState {
    pub focused_panel_id: Option<usize>,
    pub focused: Option<gpui::WeakEntity<Terminal>>,
}

impl gpui::Global for FocusedTerminalState {}

impl FocusedTerminalState {
    pub fn set_focused(
        &mut self,
        panel_id: Option<usize>,
        terminal: Option<gpui::WeakEntity<Terminal>>,
    ) {
        self.focused_panel_id = panel_id;
        self.focused = terminal;
    }
}

#[derive(Clone, Debug)]
pub struct TerminalTarget {
    pub panel_id: usize,
    pub label: SharedString,
    pub terminal: gpui::WeakEntity<Terminal>,
}

#[derive(Default)]
pub(crate) struct TerminalRegistryState {
    targets: HashMap<usize, TerminalTarget>,
}

impl gpui::Global for TerminalRegistryState {}

impl TerminalRegistryState {
    pub fn upsert_target(
        &mut self,
        panel_id: usize,
        label: SharedString,
        terminal: gpui::WeakEntity<Terminal>,
    ) {
        self.targets.insert(
            panel_id,
            TerminalTarget {
                panel_id,
                label,
                terminal,
            },
        );
    }

    pub fn remove_target(&mut self, panel_id: usize) {
        self.targets.remove(&panel_id);
    }

    pub fn list_targets(&mut self) -> Vec<TerminalTarget> {
        self.targets.retain(|_, t| t.terminal.upgrade().is_some());
        let mut out = self.targets.values().cloned().collect::<Vec<_>>();
        out.sort_by(|a, b| {
            a.label
                .as_ref()
                .cmp(b.label.as_ref())
                .then_with(|| a.panel_id.cmp(&b.panel_id))
        });
        out
    }

    pub fn get(&self, panel_id: usize) -> Option<&TerminalTarget> {
        self.targets.get(&panel_id)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct TerminalTargetInfo {
    pub panel_id: usize,
    pub label: SharedString,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SendInputError {
    RegistryUnavailable,
    TargetUnavailable,
}

pub(crate) fn ensure_app_globals(app: &mut App) {
    if app.try_global::<AssistantState>().is_none() {
        app.set_global(AssistantState::default());
    }
    if app.try_global::<FocusedTerminalState>().is_none() {
        app.set_global(FocusedTerminalState::default());
    }
    if app.try_global::<TerminalRegistryState>().is_none() {
        app.set_global(TerminalRegistryState::default());
    }
}

pub(crate) fn ensure_globals<T>(cx: &mut Context<T>) {
    crate::globals::ensure_ctx_global::<AssistantState, _>(cx);
    crate::globals::ensure_ctx_global::<FocusedTerminalState, _>(cx);
    crate::globals::ensure_ctx_global::<TerminalRegistryState, _>(cx);
}

pub(crate) fn focused_panel_id(cx: &impl Borrow<App>) -> Option<usize> {
    cx.borrow()
        .try_global::<FocusedTerminalState>()
        .and_then(|s| s.focused_panel_id)
}

pub(crate) fn target_label(cx: &impl Borrow<App>, panel_id: usize) -> Option<SharedString> {
    cx.borrow()
        .try_global::<TerminalRegistryState>()
        .and_then(|r| r.get(panel_id).map(|t| t.label.clone()))
}

pub(crate) fn target_is_available(cx: &impl Borrow<App>, panel_id: usize) -> bool {
    cx.borrow()
        .try_global::<TerminalRegistryState>()
        .and_then(|r| r.get(panel_id))
        .and_then(|t| t.terminal.upgrade())
        .is_some()
}

pub(crate) fn list_targets(cx: &mut impl BorrowMut<App>) -> Vec<TerminalTargetInfo> {
    let app = cx.borrow_mut();
    if app.try_global::<TerminalRegistryState>().is_none() {
        return Vec::new();
    }

    app.global_mut::<TerminalRegistryState>()
        .list_targets()
        .into_iter()
        .map(|t| TerminalTargetInfo {
            panel_id: t.panel_id,
            label: t.label,
        })
        .collect()
}

pub(crate) fn send_input_to_target<C: AppContext + Borrow<App>>(
    cx: &mut C,
    panel_id: usize,
    input: Vec<u8>,
) -> Result<(), SendInputError> {
    let app: &App = (*cx).borrow();
    let terminal = app
        .try_global::<TerminalRegistryState>()
        .ok_or(SendInputError::RegistryUnavailable)?
        .get(panel_id)
        .and_then(|t| t.terminal.upgrade())
        .ok_or(SendInputError::TargetUnavailable)?;

    terminal.update(cx, move |terminal: &mut Terminal, _| terminal.input(input));

    Ok(())
}

pub(crate) fn register_terminal_target<T>(
    cx: &mut Context<T>,
    panel_id: usize,
    label: SharedString,
    terminal: gpui::WeakEntity<Terminal>,
) {
    if cx.try_global::<TerminalRegistryState>().is_none() {
        return;
    }

    cx.global_mut::<TerminalRegistryState>()
        .upsert_target(panel_id, label, terminal);
}

pub(crate) fn unregister_terminal_target<T>(cx: &mut Context<T>, panel_id: usize) {
    if cx.try_global::<TerminalRegistryState>().is_none() {
        return;
    }

    cx.global_mut::<TerminalRegistryState>()
        .remove_target(panel_id);
}

pub(crate) fn set_focused_terminal<T>(
    cx: &mut Context<T>,
    panel_id: Option<usize>,
    terminal: Option<gpui::WeakEntity<Terminal>>,
) {
    if cx.try_global::<FocusedTerminalState>().is_none() {
        return;
    }

    cx.global_mut::<FocusedTerminalState>()
        .set_focused(panel_id, terminal);
}

pub fn focused_selection_text<C>(cx: &C) -> Option<String>
where
    C: AppContext + Borrow<App>,
{
    let weak = cx
        .borrow()
        .try_global::<FocusedTerminalState>()?
        .focused
        .clone()?;
    let terminal = weak.upgrade()?;
    cx.read_entity(&terminal, |terminal, _app| {
        terminal.last_content().selection_text.clone()
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FencedCodeBlock {
    pub lang: Option<String>,
    pub body: String,
}

pub fn extract_fenced_code_blocks(text: &str) -> Vec<FencedCodeBlock> {
    let mut out = Vec::new();
    if text.trim().is_empty() {
        return out;
    }

    let mut ix = 0usize;
    while ix < text.len() {
        let Some(open_rel) = text[ix..].find("```") else {
            break;
        };
        let open_ix = ix + open_rel;

        // Find end-of-line for optional language tag.
        let mut lang_end = open_ix + 3;
        while lang_end < text.len() {
            if text.as_bytes()[lang_end] == b'\n' {
                break;
            }
            lang_end += 1;
        }
        if lang_end >= text.len() {
            break;
        }

        let lang_raw = text[open_ix + 3..lang_end].trim();
        let lang = (!lang_raw.is_empty()).then(|| lang_raw.to_string());

        let body_start = lang_end + 1;
        let Some(close_rel) = text[body_start..].find("```") else {
            break;
        };
        let close_ix = body_start + close_rel;

        let body = text[body_start..close_ix].trim();
        if !body.is_empty() {
            out.push(FencedCodeBlock {
                lang,
                body: body.to_string(),
            });
        }

        ix = close_ix + 3;
    }

    out
}

pub fn strip_fenced_code_blocks(text: &str) -> String {
    if text.trim().is_empty() {
        return String::new();
    }

    let mut out = String::new();
    let mut ix = 0usize;
    while ix < text.len() {
        let Some(open_rel) = text[ix..].find("```") else {
            out.push_str(&text[ix..]);
            break;
        };
        let open_ix = ix + open_rel;
        out.push_str(&text[ix..open_ix]);

        // Find end-of-line for optional language tag.
        let mut lang_end = open_ix + 3;
        while lang_end < text.len() {
            if text.as_bytes()[lang_end] == b'\n' {
                break;
            }
            lang_end += 1;
        }
        if lang_end >= text.len() {
            break;
        }

        let body_start = lang_end + 1;
        let Some(close_rel) = text[body_start..].find("```") else {
            break;
        };
        let close_ix = body_start + close_rel;
        ix = close_ix + 3;
    }

    out.trim().to_string()
}

pub fn extract_terminal_command_snippets(text: &str) -> Vec<String> {
    fn lang_allows_terminal_run(lang: Option<&str>) -> bool {
        let Some(lang) = lang else { return true };
        let lang = lang.trim();
        if lang.is_empty() {
            return true;
        }
        let lang = lang.to_ascii_lowercase();
        matches!(
            lang.as_str(),
            "sh" | "bash" | "zsh" | "shell" | "console" | "terminal"
        )
    }

    fn looks_like_multi_line_script(body: &str) -> bool {
        if body.contains("<<") {
            return true;
        }
        for line in body.lines() {
            let trimmed = line.trim_end();
            if trimmed.ends_with('\\') {
                return true;
            }
            if line.starts_with(' ') || line.starts_with('\t') {
                return true;
            }
        }
        let lower = body.to_ascii_lowercase();
        lower.contains("\nif ")
            || lower.contains("\nfor ")
            || lower.contains("\nwhile ")
            || lower.contains("\ncase ")
            || lower.contains("\nfunction ")
            || lower.contains("\ndo\n")
            || lower.contains("\nthen\n")
    }

    let mut out = Vec::new();
    for block in extract_fenced_code_blocks(text) {
        if !lang_allows_terminal_run(block.lang.as_deref()) {
            continue;
        }

        let body = block.body.trim();
        if body.is_empty() {
            continue;
        }

        let has_blank_lines = body.lines().any(|l| l.trim().is_empty());
        let script_like = looks_like_multi_line_script(body);

        if has_blank_lines || script_like {
            let mut buf = String::new();
            for line in body.lines() {
                if line.trim().is_empty() {
                    let snippet = buf.trim().to_string();
                    if !snippet.is_empty() {
                        out.push(snippet);
                    }
                    buf.clear();
                    continue;
                }
                buf.push_str(line);
                buf.push('\n');
            }
            let snippet = buf.trim().to_string();
            if !snippet.is_empty() {
                out.push(snippet);
            }
        } else {
            for line in body.lines() {
                let snippet = line.trim();
                if snippet.is_empty() {
                    continue;
                }
                if snippet.starts_with('#') {
                    continue;
                }
                out.push(snippet.to_string());
            }
        }
    }

    out
}

pub fn sanitize_assistant_reply(text: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            out.push(line);
            continue;
        }

        // Some models occasionally hallucinate internal "tool call" transcripts.
        // Strip those so the Assistant panel only shows user-facing content.
        if trimmed.contains("to=functions.") || trimmed.contains("to=shell") {
            continue;
        }
        if looks_like_tool_json_payload(trimmed) {
            continue;
        }

        out.push(line);
    }

    out.join("\n").trim().to_string()
}

fn looks_like_tool_json_payload(trimmed_line: &str) -> bool {
    let s = trimmed_line;
    if !(s.starts_with('{') && s.ends_with('}')) {
        return false;
    }

    // Best-effort filter: these are common wrappers for tool calls in chat transcripts.
    s.contains("\"cmd\"") || s.contains("\"command\"") || s.contains("\"justification\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_fenced_code_blocks_returns_multiple_blocks() {
        let text = "A\n```sh\necho hi\n```\nB\n```bash\nls\n```\nC";
        let blocks = extract_fenced_code_blocks(text);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].lang.as_deref(), Some("sh"));
        assert_eq!(blocks[0].body, "echo hi");
        assert_eq!(blocks[1].lang.as_deref(), Some("bash"));
        assert_eq!(blocks[1].body, "ls");
    }

    #[test]
    fn strip_fenced_code_blocks_removes_blocks_but_keeps_surrounding_text() {
        let text = "A\n```sh\necho hi\n```\nB";
        assert_eq!(strip_fenced_code_blocks(text), "A\n\nB");
    }

    #[test]
    fn extract_terminal_command_snippets_splits_simple_lines() {
        let text = "```sh\nls -la\npwd\n```";
        assert_eq!(
            extract_terminal_command_snippets(text),
            vec!["ls -la".to_string(), "pwd".to_string()]
        );
    }

    #[test]
    fn assistant_state_request_lifecycle_is_tracked_and_cancellable() {
        let mut s = AssistantState::default();
        assert!(!s.in_flight);
        assert_eq!(s.active_request_id, None);

        let id = s.begin_request();
        assert!(s.in_flight);
        assert_eq!(s.active_request_id, Some(id));

        assert!(!s.finish_request(id.saturating_add(1)));
        assert!(s.in_flight);
        assert_eq!(s.active_request_id, Some(id));

        assert!(s.cancel_request());
        assert!(!s.in_flight);
        assert_eq!(s.active_request_id, None);

        assert!(!s.finish_request(id));
    }

    #[test]
    fn sanitize_assistant_reply_strips_tool_call_transcript_lines() {
        let raw = r#"
Ok, here's what I'll do.
to=functions.exec_command {"cmd":"ls -la ~/vmware"}
{"cmd":"bash -lc 'ls -la ~/vmware'"}
Run:
```sh
ls -la ~/vmware
```
Done.
"#;

        let cleaned = sanitize_assistant_reply(raw);
        assert!(!cleaned.contains("to=functions.exec_command"));
        assert!(!cleaned.contains("\"cmd\""));
        assert!(cleaned.contains("```sh"));
        assert!(cleaned.contains("ls -la ~/vmware"));
    }
}
