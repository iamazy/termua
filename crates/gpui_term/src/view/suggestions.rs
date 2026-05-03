use std::time::Duration;

use gpui::{App, Context, KeyDownEvent, Keystroke, ReadGlobal};
use smol::Timer;

use super::TerminalView;
use crate::{
    TerminalContent, TerminalMode,
    settings::TerminalSettings,
    snippet::{SnippetSession, parse_snippet_suffix},
    suggestions::{
        SelectionMove, SuggestionItem, SuggestionStaticConfig, compute_insert_suffix_for_line,
        extract_cursor_line_prefix, extract_cursor_line_suffix, line_is_suggestion_prefix,
        move_selection_opt,
    },
};

impl TerminalView {
    pub(crate) fn suggestions_snapshot(&self) -> Option<(Vec<SuggestionItem>, Option<usize>)> {
        self.suggestions.open.then(|| {
            let highlighted = self
                .suggestions
                .hovered
                .or(self.suggestions.selected)
                .and_then(|highlighted| {
                    let last = self.suggestions.items.len().saturating_sub(1);
                    (!self.suggestions.items.is_empty()).then_some(highlighted.min(last))
                });
            (self.suggestions.items.clone(), highlighted)
        })
    }

    pub(crate) fn close_suggestions(&mut self, cx: &mut Context<Self>) {
        if !self.suggestions.open {
            return;
        }
        self.suggestions.close();
        cx.notify();
    }

    pub(crate) fn set_suggestions_hovered(
        &mut self,
        hovered: Option<usize>,
        cx: &mut Context<Self>,
    ) {
        if !self.suggestions.open {
            if self.suggestions.hovered.take().is_some() {
                cx.notify();
            }
            return;
        }

        let hovered = hovered.and_then(|idx| {
            let last = self.suggestions.items.len().saturating_sub(1);
            (!self.suggestions.items.is_empty()).then_some(idx.min(last))
        });

        if self.suggestions.hovered != hovered {
            self.suggestions.hovered = hovered;
            cx.notify();
        }
    }

    pub(super) fn handle_suggestions_key_down(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        // Suggestions are intentionally conservative: remote/SSH sessions have no shell
        // integration, so we only show/accept append-only hints in shell-like contexts.
        if !TerminalSettings::global(cx).suggestions_enabled || self.snippet.is_some() {
            return false;
        }

        let Some(prompt) = self.prompt_context(cx) else {
            return false;
        };
        if !self.suggestions_eligible_for_content(&prompt.content, cx) {
            self.suggestions.prompt_prefix = None;
            self.suggestions.close();
            return false;
        }

        match event.keystroke.key.as_str() {
            "escape" if self.suggestions.open => {
                self.suggestions.close();
                cx.notify();
                cx.stop_propagation();
                true
            }
            "up" if self.suggestions.open => {
                self.suggestions.selected = move_selection_opt(
                    self.suggestions.selected,
                    self.suggestions.items.len(),
                    SelectionMove::Up,
                );
                cx.notify();
                cx.stop_propagation();
                true
            }
            "down" if self.suggestions.open => {
                self.suggestions.selected = move_selection_opt(
                    self.suggestions.selected,
                    self.suggestions.items.len(),
                    SelectionMove::Down,
                );
                cx.notify();
                cx.stop_propagation();
                true
            }
            "enter" => {
                if self.accept_selected_suggestion(&prompt.content, prompt.cursor_line_id, cx) {
                    cx.stop_propagation();
                    return true;
                }

                if let Some(prompt_prefix) = self.suggestions.prompt_prefix.take() {
                    let line_prefix = extract_cursor_line_prefix(&prompt.content);
                    let input = line_prefix
                        .strip_prefix(&prompt_prefix)
                        .unwrap_or("")
                        .trim()
                        .to_string();
                    if !input.is_empty() {
                        self.queue_command_for_history(input, cx);
                    }
                }
                self.suggestions.close();
                false
            }
            "right" => {
                if self.accept_selected_suggestion(&prompt.content, prompt.cursor_line_id, cx) {
                    cx.stop_propagation();
                    return true;
                }
                false
            }
            "backspace" => {
                if self.suggestions.prompt_prefix.is_none() {
                    self.suggestions.prompt_prefix =
                        Some(extract_cursor_line_prefix(&prompt.content));
                }
                self.schedule_suggestions_update(cx);
                false
            }
            _ => {
                let is_plain_text = event.keystroke.key_char.as_ref().is_some_and(|ch| {
                    !ch.is_empty()
                        && !event.keystroke.is_ime_in_progress()
                        && !event.keystroke.modifiers.control
                        && !event.keystroke.modifiers.platform
                        && !event.keystroke.modifiers.function
                        && !event.keystroke.modifiers.alt
                });

                if is_plain_text {
                    if self.suggestions.prompt_prefix.is_none() {
                        self.suggestions.prompt_prefix =
                            Some(extract_cursor_line_prefix(&prompt.content));
                    }
                    self.schedule_suggestions_update(cx);
                } else if self.suggestions.open {
                    self.suggestions.close();
                }
                false
            }
        }
    }

    pub(super) fn suggestions_eligible_for_content(
        &self,
        content: &TerminalContent,
        cx: &App,
    ) -> bool {
        TerminalSettings::global(cx).suggestions_enabled
            && content.display_offset == 0
            && !content.mode.contains(TerminalMode::ALT_SCREEN)
            && content.selection.is_none()
            && self.scroll.block_below_cursor.is_none()
    }

    fn schedule_suggestions_update(&mut self, cx: &mut Context<Self>) {
        let epoch = self.suggestions.epoch.wrapping_add(1);
        self.suggestions.epoch = epoch;
        cx.spawn(async move |this, cx| {
            Timer::after(Duration::from_millis(200)).await;
            let _ = this.update(cx, |this, cx| {
                if this.suggestions.epoch != epoch {
                    return;
                }

                let Some(prompt) = this.prompt_context(cx) else {
                    return;
                };
                if !this.suggestions_eligible_for_content(&prompt.content, cx) {
                    this.suggestions.prompt_prefix = None;
                    this.suggestions.close();
                    cx.notify();
                    return;
                }

                let Some(prompt_prefix) = this.suggestions.prompt_prefix.clone() else {
                    this.suggestions.close();
                    cx.notify();
                    return;
                };

                let line_prefix = extract_cursor_line_prefix(&prompt.content);
                let input_prefix = line_prefix.strip_prefix(&prompt_prefix).unwrap_or("");

                this.suggestions.engine.max_items =
                    TerminalSettings::global(cx).suggestions_max_items;

                if let Some(cfg) = cx.try_global::<SuggestionStaticConfig>()
                    && cfg.epoch != this.suggestions.static_epoch_seen
                {
                    this.suggestions.static_epoch_seen = cfg.epoch;
                    this.suggestions
                        .engine
                        .set_static_provider(cfg.provider.clone());
                }

                let items = this.suggestions.engine.suggest(input_prefix);
                this.suggestions.open_with_items(items);
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn accept_selected_suggestion(
        &mut self,
        content: &TerminalContent,
        cursor_line_id: Option<i64>,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.suggestions.open {
            return false;
        }

        let Some(selected) = self.suggestions.selected else {
            return false;
        };
        self.accept_suggestion_at_index(selected, content, cursor_line_id, cx)
    }

    pub(crate) fn accept_suggestion_at_index(
        &mut self,
        index: usize,
        content: &TerminalContent,
        cursor_line_id: Option<i64>,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.suggestions.open {
            return false;
        }

        let Some(item) = self.suggestions.items.get(index).cloned() else {
            return false;
        };

        let line_prefix = extract_cursor_line_prefix(content);
        let Some((input_prefix, suffix_template)) = compute_insert_suffix_for_line(
            &line_prefix,
            self.suggestions.prompt_prefix.as_deref(),
            &item.full_text,
        ) else {
            return false;
        };

        let line_suffix = extract_cursor_line_suffix(content);
        if !line_suffix.trim().is_empty() {
            let combined_line = format!("{input_prefix}{line_suffix}");
            if line_is_suggestion_prefix(&combined_line, &item.full_text) {
                return false;
            }
        }

        let mut suffix_rendered = suffix_template.clone();
        let mut snippet_session: Option<SnippetSession> = None;
        let mut initial_move_left = 0usize;

        if let Some(snippet) = parse_snippet_suffix(&suffix_template) {
            suffix_rendered = snippet.rendered;

            // `$0` is a cursor position, not an editable placeholder. Avoid entering snippet mode
            // if there are no non-zero tabstops.
            if snippet.tabstops.iter().any(|t| t.index != 0) {
                let mut session = SnippetSession::new(suffix_rendered.clone(), snippet.tabstops);
                session.cursor_line_id = cursor_line_id;
                session.start_point = content.cursor.point;
                session.active = 0;

                let target_end = session
                    .tabstops
                    .first()
                    .map(|t| t.range_chars.end)
                    .unwrap_or(session.inserted_len_chars);

                initial_move_left = session.inserted_len_chars.saturating_sub(target_end);
                session.cursor_offset_chars = target_end;
                session.selected = true;
                snippet_session = Some(session);
            }
        }

        self.snap_to_bottom_on_input(cx);
        let alt_is_meta = TerminalSettings::global(cx).option_as_meta;
        let left = Keystroke::parse("left").unwrap();

        let suffix = suffix_rendered.into_bytes();
        self.terminal.update(cx, move |term, _| {
            term.input(suffix);
            for _ in 0..initial_move_left {
                term.try_keystroke(&left, alt_is_meta);
            }
        });
        self.snippet = snippet_session;
        self.suggestions.close();
        true
    }

    pub(crate) fn snippet_snapshot_for_content(
        &self,
        content: &TerminalContent,
        cursor_line_id: Option<i64>,
        cx: &App,
    ) -> Option<SnippetSession> {
        let snippet = self.snippet.clone()?;
        if !self.suggestions_eligible_for_content(content, cx) {
            return None;
        }

        if let Some(expected) = snippet.cursor_line_id
            && cursor_line_id != Some(expected)
        {
            return None;
        }

        Some(snippet)
    }

    pub(super) fn handle_snippet_key_down(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.snippet.is_none() {
            return false;
        }

        let eligible = self
            .prompt_context(cx)
            .is_some_and(|prompt| self.snippet_prompt_is_eligible(&prompt, cx));

        if !eligible {
            self.snippet = None;
            return false;
        }

        // Any newline ends the snippet session.
        if event.keystroke.key.as_str() == "enter" {
            self.snippet = None;
            return false;
        }

        let is_plain_text = event.keystroke.key_char.as_ref().is_some_and(|ch| {
            !ch.is_empty()
                && !event.keystroke.is_ime_in_progress()
                && !event.keystroke.modifiers.control
                && !event.keystroke.modifiers.platform
                && !event.keystroke.modifiers.function
                && !event.keystroke.modifiers.alt
        });
        if is_plain_text
            && !matches!(event.keystroke.key.as_str(), "tab" | "escape")
            && let Some(ch) = event.keystroke.key_char.as_deref()
            && !ch.is_empty()
        {
            // Snippet placeholder "selection" is local UI state; terminal line editors do not
            // support replacing highlighted ranges. Treat character input as an explicit
            // commit so we can delete/replace the active placeholder and keep placeholder
            // highlight ranges in sync.
            self.commit_text(ch, cx);
            cx.notify();
            cx.stop_propagation();
            return true;
        }

        let Some(session) = self.snippet.as_mut() else {
            return false;
        };

        if event.keystroke.key.as_str() == "backspace" {
            if session.selected {
                let deleted_chars = session.delete_active_placeholder();
                session.selected = false;

                if deleted_chars > 0 {
                    let alt_is_meta = TerminalSettings::global(cx).option_as_meta;
                    let backspace = Keystroke::parse("backspace").unwrap();
                    self.terminal.update(cx, move |term, _| {
                        for _ in 0..deleted_chars {
                            term.try_keystroke(&backspace, alt_is_meta);
                        }
                    });
                    cx.notify();
                    cx.stop_propagation();
                    return true;
                }
            } else {
                let deleted = session.backspace_one_in_active_placeholder();
                if deleted {
                    let alt_is_meta = TerminalSettings::global(cx).option_as_meta;
                    let backspace = Keystroke::parse("backspace").unwrap();
                    self.terminal.update(cx, move |term, _| {
                        term.try_keystroke(&backspace, alt_is_meta);
                    });
                    cx.notify();
                    cx.stop_propagation();
                    return true;
                }
            }
            return false;
        }

        // Any unexpected navigation/editing key cancels snippet mode to avoid
        // desync with the remote line editor state.
        let key = event.keystroke.key.as_str();
        let cancel = event.keystroke.modifiers.control
            || event.keystroke.modifiers.platform
            || event.keystroke.modifiers.function
            || event.keystroke.modifiers.alt
            || matches!(
                key,
                "left"
                    | "right"
                    | "up"
                    | "down"
                    | "home"
                    | "end"
                    | "pageup"
                    | "pagedown"
                    | "delete"
            );
        if cancel {
            self.snippet = None;
        }

        false
    }
}
