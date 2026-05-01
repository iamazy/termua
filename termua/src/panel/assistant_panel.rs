use gpui::{
    AnyElement, App, AppContext as _, ClipboardItem, Context, Entity, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement as _, ParentElement as _, Pixels, Render, ScrollHandle,
    SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, div,
    prelude::FluentBuilder as _, px,
};
use gpui_common::TermuaIcon;
use gpui_component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputState},
    menu::{DropdownMenu as _, PopupMenu, PopupMenuItem},
    scroll::{Scrollbar, ScrollbarShow},
    spinner::Spinner,
    text::TextView,
    v_flex,
};
use rust_i18n::t;
use serde::Deserialize;
use termua_zeroclaw::{Client as ZeroclawClient, ClientOptions as ZeroclawClientOptions};

use crate::assistant::{
    ASSISTANT_SYSTEM_PROMPT, AssistantMessage, AssistantRole, AssistantState,
    DEFAULT_TERMINAL_CONTEXT_MAX_LINES, extract_terminal_command_snippets, focused_selection_text,
    sanitize_assistant_reply, strip_fenced_code_blocks, tail_text_for_panel,
};

const PROMPT_KEY_CONTEXT: &str = "termua_assistant_prompt";

#[derive(gpui::Action, Clone, PartialEq, Eq, Deserialize)]
#[action(namespace = termua, no_json)]
pub(crate) struct AssistantSend;

pub(crate) fn bind_keybindings(cx: &mut App) {
    cx.bind_keys([gpui::KeyBinding::new(
        "enter",
        AssistantSend,
        Some("termua_assistant_prompt > Input"),
    )]);
}

pub struct AssistantPanelView {
    focus_handle: FocusHandle,
    scroll_handle: ScrollHandle,
    remaining_scroll_to_bottom_passes: u8,
    prompt_input: gpui::Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

impl Focusable for AssistantPanelView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn set_input_placeholder(
    input: &Entity<InputState>,
    placeholder: String,
    window: &mut Window,
    cx: &mut Context<AssistantPanelView>,
) {
    input.update(cx, |state, cx| {
        state.set_placeholder(placeholder, window, cx);
    });
}

impl AssistantPanelView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        crate::assistant::ensure_globals(cx);

        let prompt_input = cx.new(|cx| {
            InputState::new(window, cx)
                .auto_grow(3, 10)
                .placeholder(t!("Assistant.Placeholder.Ask").to_string())
        });
        let subs = vec![
            cx.observe_global::<AssistantState>(|_, cx| cx.notify()),
            cx.observe_global_in::<crate::settings::LanguageSettings>(
                window,
                |this, window, cx| {
                    set_input_placeholder(
                        &this.prompt_input,
                        t!("Assistant.Placeholder.Ask").to_string(),
                        window,
                        cx,
                    );
                    cx.notify();
                    window.refresh();
                },
            ),
            cx.observe_window_activation(window, |_, _, cx| cx.notify()),
        ];

        Self {
            focus_handle: cx.focus_handle(),
            scroll_handle: ScrollHandle::default(),
            remaining_scroll_to_bottom_passes: 0,
            prompt_input,
            _subscriptions: subs,
        }
    }

    fn send_on_enter(&mut self, _: &AssistantSend, window: &mut Window, cx: &mut Context<Self>) {
        self.send(window, cx);
        cx.refresh_windows();
        window.refresh();
    }

    fn cancel(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if cx.global_mut::<AssistantState>().cancel_request() {
            cx.global_mut::<AssistantState>().push(
                AssistantRole::System,
                t!("Assistant.Message.Cancelled").to_string(),
            );
        }
        cx.notify();
    }

    fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let prompt = self.prompt_input.read(cx).value().to_string();
        let prompt = prompt.trim().to_string();
        if prompt.is_empty() {
            return;
        }

        // Best-effort clear the input immediately to keep the UI responsive.
        self.prompt_input
            .update(cx, |state, cx| state.set_value("", window, cx));

        self.send_prompt_text(prompt, cx);
    }

    fn send_prompt_text(&mut self, prompt: String, cx: &mut Context<Self>) {
        if cx.global::<AssistantState>().in_flight {
            return;
        }

        let prompt = prompt.trim().to_string();
        if prompt.is_empty() {
            return;
        }

        let attach_selection = cx.global::<AssistantState>().attach_selection;
        let attach_terminal_context = cx.global::<AssistantState>().attach_terminal_context;
        let terminal_context_panel_id = cx
            .global::<AssistantState>()
            .target_panel_id
            .or(crate::assistant::focused_panel_id(cx));

        let attachments = Self::gather_prompt_attachments(
            cx,
            attach_selection,
            attach_terminal_context,
            terminal_context_panel_id,
        );

        let request_id = {
            let state = cx.global_mut::<AssistantState>();
            state.push(AssistantRole::User, prompt.clone());
            state.begin_request()
        };

        // Ensure the newly-added message is visible without manual scrolling.
        self.scroll_messages_to_bottom(cx);
        cx.notify();

        let full_prompt = Self::compose_full_prompt(&prompt, &attachments);
        let full_prompt = format!("{ASSISTANT_SYSTEM_PROMPT}\n\n{full_prompt}");

        let assistant_settings = cx
            .try_global::<crate::settings::AssistantSettings>()
            .cloned()
            .unwrap_or_default();

        if !assistant_settings.enabled {
            self.finish_disabled_request(request_id, cx);
            return;
        }

        Self::spawn_assistant_turn(request_id, full_prompt, assistant_settings, cx);
    }

    fn gather_prompt_attachments(
        cx: &mut Context<Self>,
        attach_selection: bool,
        attach_terminal_context: bool,
        terminal_context_panel_id: Option<usize>,
    ) -> PromptAttachments {
        let selection = if attach_selection {
            focused_selection_text(cx).unwrap_or_default()
        } else {
            String::new()
        };

        let (terminal_context, last_command_output) = if attach_terminal_context {
            let terminal_context = terminal_context_panel_id
                .and_then(|panel_id| {
                    crate::assistant::terminal_context_snapshot_text(cx, panel_id)
                        .map(|s| s.to_string())
                        .or_else(|| {
                            tail_text_for_panel(cx, panel_id, DEFAULT_TERMINAL_CONTEXT_MAX_LINES)
                        })
                })
                .unwrap_or_default();

            let last_command_output = terminal_context_panel_id
                .and_then(|panel_id| {
                    crate::assistant::command_output_snapshot_text(cx, panel_id)
                        .map(|s| s.to_string())
                })
                .unwrap_or_default();

            (terminal_context, last_command_output)
        } else {
            (String::new(), String::new())
        };

        PromptAttachments {
            selection,
            terminal_context,
            last_command_output,
        }
    }

    fn compose_full_prompt(prompt: &str, attachments: &PromptAttachments) -> String {
        if attachments.last_command_output.is_empty()
            && attachments.terminal_context.is_empty()
            && attachments.selection.is_empty()
        {
            return prompt.to_string();
        }

        let mut out = String::new();
        if !attachments.last_command_output.is_empty() {
            out.push_str("Last command output:\n");
            out.push_str(&attachments.last_command_output);
            out.push_str("\n\n");
        }
        if !attachments.terminal_context.is_empty() {
            out.push_str("Terminal context (tail):\n");
            out.push_str(&attachments.terminal_context);
            out.push_str("\n\n");
        }
        if !attachments.selection.is_empty() {
            out.push_str("Selected text:\n");
            out.push_str(&attachments.selection);
            out.push_str("\n\n");
        }
        out.push_str("User:\n");
        out.push_str(prompt);
        out
    }

    fn finish_disabled_request(&mut self, request_id: u64, cx: &mut Context<Self>) {
        let state = cx.global_mut::<AssistantState>();
        state.push(
            AssistantRole::Assistant,
            t!("Assistant.Message.Disabled").to_string(),
        );
        state.finish_request(request_id);
        self.scroll_messages_to_bottom(cx);
        cx.notify();
    }

    fn scroll_messages_to_bottom(&mut self, cx: &mut Context<Self>) {
        self.remaining_scroll_to_bottom_passes = self.remaining_scroll_to_bottom_passes.max(2);
        self.scroll_handle.scroll_to_bottom();
        cx.notify();
    }

    fn spawn_assistant_turn(
        request_id: u64,
        full_prompt: String,
        assistant_settings: crate::settings::AssistantSettings,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            let first_prompt = full_prompt.clone();
            let assistant_settings_first = assistant_settings.clone();
            let result = smol::unblock(move || {
                let api_key = crate::keychain::load_zeroclaw_api_key().ok().flatten();
                let opts = ZeroclawClientOptions {
                    provider: assistant_settings_first.provider.clone(),
                    model: assistant_settings_first.model.clone(),
                    api_key,
                    api_url: assistant_settings_first.api_url.clone(),
                    api_path: assistant_settings_first.api_path.clone(),
                    temperature: assistant_settings_first.temperature,
                    provider_timeout_secs: assistant_settings_first.provider_timeout_secs,
                    extra_headers: assistant_settings_first.extra_headers.clone(),
                };
                ZeroclawClient::turn_blocking_with_options(first_prompt, opts)
            })
            .await;

            let reply: SharedString = match result {
                Ok(s) => sanitize_assistant_reply(&s).into(),
                Err(err) => format!("Error: {err:#}").into(),
            };
            let _ = this.update(cx, |_this, cx| {
                if !cx.global_mut::<AssistantState>().finish_request(request_id) {
                    return;
                }
                cx.global_mut::<AssistantState>()
                    .push(AssistantRole::Assistant, reply);
                _this.scroll_messages_to_bottom(cx);
                cx.notify();
            });
        })
        .detach();
    }
}

struct PromptAttachments {
    selection: String,
    terminal_context: String,
    last_command_output: String,
}

#[derive(Clone)]
struct AssistantMessageStyle {
    label: SharedString,
    bg: gpui::Hsla,
    border: gpui::Hsla,
    role_icon: TermuaIcon,
}

impl AssistantPanelView {
    fn sync_target_selection(&mut self, cx: &mut Context<Self>) {
        let focused_id = crate::assistant::focused_panel_id(cx);
        let follow_focus = cx.global::<AssistantState>().target_follows_focus;
        let current_target_id = cx.global::<AssistantState>().target_panel_id;

        if follow_focus {
            if focused_id.is_some() {
                cx.global_mut::<AssistantState>().target_panel_id = focused_id;
            }
        } else if let Some(current_target_id) = current_target_id {
            let current_exists = crate::assistant::target_is_available(cx, current_target_id);
            if !current_exists {
                let state = cx.global_mut::<AssistantState>();
                state.target_panel_id = focused_id;
                state.target_follows_focus = true;
            }
        }
    }

    fn compute_can_rerun_user_messages(entries: &[AssistantMessage], in_flight: bool) -> Vec<bool> {
        let mut can_rerun = entries
            .iter()
            .map(|m| m.role == AssistantRole::User)
            .collect::<Vec<_>>();

        // Avoid showing "Resend" for the latest user message while we're still awaiting
        // the assistant reply, but keep it available for older prompts (even if their
        // assistant replies were deleted).
        if in_flight {
            if let Some(last_user_ix) = entries.iter().rposition(|m| m.role == AssistantRole::User)
            {
                let has_assistant_after = entries[last_user_ix + 1..]
                    .iter()
                    .any(|m| m.role == AssistantRole::Assistant);
                if !has_assistant_after {
                    can_rerun[last_user_ix] = false;
                }
            }
        }

        can_rerun
    }

    fn render_header_bar(
        &self,
        tab_header_height: Pixels,
        target_label: SharedString,
        follow_focus: bool,
        target_panel_id: Option<usize>,
        entries_empty: bool,
        in_flight: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        h_flex()
            .items_center()
            .justify_between()
            .h(tab_header_height)
            .text_sm()
            .px_2()
            .border_b_1()
            .border_color(cx.theme().border.opacity(0.8))
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .debug_selector(|| "termua-assistant-header-icon".to_string())
                            .child(Icon::default().path(TermuaIcon::Bot).size_4()),
                    )
                    .child(div().child(t!("Assistant.Title").to_string()))
                    .child(
                        Button::new("termua-assistant-target")
                            .xsmall()
                            .ghost()
                            .label(target_label)
                            .dropdown_menu(move |menu, _window, cx| {
                                let menu = menu.item(
                                    PopupMenuItem::new(
                                        t!("Assistant.Menu.FollowActiveTab").to_string(),
                                    )
                                    .checked(follow_focus)
                                    .on_click(
                                        move |_, window, cx| {
                                            let focused_id = crate::assistant::focused_panel_id(cx);
                                            let state = cx.global_mut::<AssistantState>();
                                            state.target_follows_focus = true;
                                            state.target_panel_id = focused_id;
                                            cx.refresh_windows();
                                            window.refresh();
                                        },
                                    ),
                                );

                                let mut menu = menu.item(
                                    PopupMenuItem::new(t!("Assistant.Menu.NoTarget").to_string())
                                        .checked(!follow_focus && target_panel_id.is_none())
                                        .on_click(move |_, window, cx| {
                                            let state = cx.global_mut::<AssistantState>();
                                            state.target_follows_focus = false;
                                            state.target_panel_id = None;
                                            cx.refresh_windows();
                                            window.refresh();
                                        }),
                                );

                                for target in crate::assistant::list_targets(cx) {
                                    let panel_id = target.panel_id;
                                    let label = target.label.to_string();
                                    let checked =
                                        !follow_focus && target_panel_id == Some(panel_id);
                                    menu = menu.item(
                                        PopupMenuItem::new(label).checked(checked).on_click(
                                            move |_, window, cx| {
                                                let state = cx.global_mut::<AssistantState>();
                                                state.target_follows_focus = false;
                                                state.target_panel_id = Some(panel_id);
                                                cx.refresh_windows();
                                                window.refresh();
                                            },
                                        ),
                                    );
                                }

                                menu
                            }),
                    ),
            )
            .child(
                Button::new("termua-assistant-clear")
                    .xsmall()
                    .ghost()
                    .icon(Icon::default().path(TermuaIcon::Trash))
                    .disabled(entries_empty || in_flight)
                    .on_click(|_, window: &mut Window, cx: &mut App| {
                        cx.global_mut::<AssistantState>().clear();
                        cx.refresh_windows();
                        window.refresh();
                    }),
            )
            .into_any_element()
    }

    fn render_prompt_bar(
        &self,
        in_flight: bool,
        assistant_enabled: bool,
        attach_selection: bool,
        attach_terminal_context: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        h_flex()
            .gap_2()
            .p_2()
            .border_t_1()
            .border_color(cx.theme().border.opacity(0.8))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .key_context(PROMPT_KEY_CONTEXT)
                    .on_action(cx.listener(Self::send_on_enter))
                    .child(
                        Input::new(&self.prompt_input).suffix(
                            h_flex()
                                .items_center()
                                .gap_1()
                                .child(
                                    div()
                                        .debug_selector(|| {
                                            "termua-assistant-send-dropdown".to_string()
                                        })
                                        .child(
                                            h_flex()
                                                .items_center()
                                                .gap_0()
                                                .child(
                                                    Button::new("termua-assistant-send")
                                                        .primary()
                                                        .compact()
                                                        .disabled(!in_flight && !assistant_enabled)
                                                        .rounded_tr(px(0.))
                                                        .rounded_br(px(0.))
                                                        .tooltip(if in_flight { "Cancel" } else { "Send" })
                                                        .debug_selector(|| {
                                                            "termua-assistant-send-button".to_string()
                                                        })
                                                        .icon(
                                                            Icon::default()
                                                                .path(if in_flight {
                                                                    TermuaIcon::Stop
                                                                } else {
                                                                    TermuaIcon::Send
                                                                })
                                                                .size_4(),
                                                        )
                                                        .on_click(cx.listener(|this, _ev, window, cx| {
                                                            if cx.global::<AssistantState>().in_flight {
                                                                this.cancel(window, cx);
                                                            } else {
                                                                this.send(window, cx);
                                                            }
                                                            cx.refresh_windows();
                                                            window.refresh();
                                                        })),
                                                )
                                                .child(
                                                    Button::new("termua-assistant-send-menu")
                                                        .primary()
                                                        .compact()
                                                        .w(px(12.))
                                                        .px(px(0.))
                                                        .ml(px(-1.))
                                                        .rounded_tl(px(0.))
                                                        .rounded_bl(px(0.))
                                                        .icon(
                                                            Icon::new(gpui_component::IconName::ChevronDown)
                                                                .xsmall(),
                                                        )
                                                        .dropdown_menu(move |menu: PopupMenu, _window, _cx| {
                                                            menu.item(
                                                                PopupMenuItem::new("Include selection")
                                                                    .checked(attach_selection)
                                                                    .on_click(|_, window, cx| {
                                                                        let state = cx.global_mut::<AssistantState>();
                                                                        state.attach_selection = !state.attach_selection;
                                                                        cx.refresh_windows();
                                                                        window.refresh();
                                                                    }),
                                                            )
                                                            .item(
                                                                PopupMenuItem::new("Include terminal context")
                                                                    .checked(attach_terminal_context)
                                                                    .on_click(|_, window, cx| {
                                                                        let state = cx.global_mut::<AssistantState>();
                                                                        state.attach_terminal_context = !state.attach_terminal_context;
                                                                        cx.refresh_windows();
                                                                        window.refresh();
                                                                    }),
                                                            )
                                                        }),
                                                ),
                                        ),
                                ),
                        ),
                    ),
            )
            .into_any_element()
    }

    fn render_messages_area(
        &self,
        this: Entity<Self>,
        entries: Vec<AssistantMessage>,
        can_rerun_user_messages: Vec<bool>,
        in_flight: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let cx = &*cx;

        let list = v_flex()
            .w_full()
            .p_2()
            .gap_2()
            .children(entries.into_iter().enumerate().map(|(ix, msg)| {
                let can_rerun_user_message =
                    can_rerun_user_messages.get(ix).copied().unwrap_or(false);
                self.render_message_card(this.clone(), ix, msg, can_rerun_user_message, cx)
            }))
            .when(in_flight, |this| this.child(Self::render_in_flight_row(cx)));

        h_flex()
            .flex_1()
            .min_h_0()
            .items_stretch()
            .child(
                div()
                    .id("termua-assistant-scroll-area")
                    .flex_1()
                    .min_h_0()
                    .flex_col()
                    .track_scroll(&self.scroll_handle)
                    .overflow_y_scroll()
                    .overflow_x_hidden()
                    .child(list),
            )
            .child(
                div()
                    .w(px(16.0))
                    .flex_shrink_0()
                    .relative()
                    .h_full()
                    .min_h_0()
                    .child(
                        Scrollbar::vertical(&self.scroll_handle)
                            .id("termua-assistant-scrollbar")
                            .scrollbar_show(ScrollbarShow::Always),
                    ),
            )
            .into_any_element()
    }

    fn render_message_card(
        &self,
        this: Entity<Self>,
        ix: usize,
        msg: AssistantMessage,
        can_rerun_user_message: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let copy_text = msg.content.to_string();
        let style = Self::message_style(msg.role, cx);

        let command_snippets = if msg.role == AssistantRole::Assistant {
            extract_terminal_command_snippets(msg.content.as_ref())
        } else {
            Vec::new()
        };

        let display_content: SharedString =
            if msg.role == AssistantRole::Assistant && !command_snippets.is_empty() {
                strip_fenced_code_blocks(msg.content.as_ref()).into()
            } else {
                msg.content.clone()
            };

        let user_prompt = (msg.role == AssistantRole::User && can_rerun_user_message)
            .then_some(copy_text.clone());

        v_flex()
            .id(format!("termua-assistant-msg-{ix}"))
            .debug_selector(move || format!("termua-assistant-msg-card-{ix}"))
            .relative()
            .w_full()
            .px_2()
            .py_1()
            .gap_0p5()
            .rounded_md()
            .border_1()
            .border_color(style.border)
            .bg(style.bg)
            .child(Self::render_message_header(style, cx))
            .child(
                div()
                    .debug_selector(move || format!("termua-assistant-msg-text-{ix}"))
                    .w_full()
                    .min_w_0()
                    .child(
                        TextView::markdown(
                            gpui::ElementId::named_usize("termua-assistant-msg-textview", ix),
                            display_content,
                        )
                        .selectable(true)
                        .w_full()
                        .min_w_0()
                        .text_sm(),
                    ),
            )
            .when(!command_snippets.is_empty(), |this_row| {
                this_row.child(Self::render_command_snippets(
                    this.clone(),
                    ix,
                    command_snippets,
                    cx,
                ))
            })
            .child(Self::render_message_actions_overlay(
                this,
                ix,
                user_prompt,
                copy_text,
            ))
            .into_any_element()
    }

    fn message_style(role: AssistantRole, cx: &Context<Self>) -> AssistantMessageStyle {
        match role {
            AssistantRole::User => AssistantMessageStyle {
                label: SharedString::from(t!("Assistant.Role.You").to_string()),
                bg: cx.theme().accent.opacity(0.10),
                border: cx.theme().accent.opacity(0.20),
                role_icon: TermuaIcon::User,
            },
            AssistantRole::Assistant => AssistantMessageStyle {
                label: SharedString::from(t!("Assistant.Role.Assistant").to_string()),
                bg: cx.theme().background.opacity(0.35),
                border: cx.theme().border.opacity(0.8),
                role_icon: TermuaIcon::Bot,
            },
            AssistantRole::System => AssistantMessageStyle {
                label: SharedString::from(t!("Assistant.Role.Context").to_string()),
                bg: cx.theme().warning.opacity(0.08),
                border: cx.theme().warning.opacity(0.25),
                role_icon: TermuaIcon::AlertCircle,
            },
        }
    }

    fn render_message_header(style: AssistantMessageStyle, cx: &Context<Self>) -> AnyElement {
        h_flex()
            .items_center()
            .gap_1()
            .child(
                Icon::default()
                    .path(style.role_icon)
                    .size_3()
                    .text_color(cx.theme().muted_foreground),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(style.label),
            )
            .into_any_element()
    }

    fn render_command_snippets(
        this: Entity<Self>,
        ix: usize,
        command_snippets: Vec<String>,
        cx: &Context<Self>,
    ) -> AnyElement {
        v_flex()
            .w_full()
            .gap_1()
            .children(
                command_snippets
                    .into_iter()
                    .enumerate()
                    .map(|(cmd_ix, command)| {
                        Self::render_command_snippet_card(this.clone(), ix, cmd_ix, command, cx)
                    }),
            )
            .into_any_element()
    }

    fn render_command_snippet_card(
        this: Entity<Self>,
        ix: usize,
        cmd_ix: usize,
        command: String,
        cx: &Context<Self>,
    ) -> AnyElement {
        let command_md = format!("```sh\n{command}\n```");

        v_flex()
            .id(format!("termua-assistant-msg-command-{ix}-{cmd_ix}"))
            .w_full()
            .gap_0p5()
            .p_1()
            .rounded_sm()
            .border_1()
            .border_color(cx.theme().border.opacity(0.8))
            .bg(cx.theme().background.opacity(0.2))
            .child(
                h_flex().justify_end().child(
                    Button::new(format!("termua-assistant-command-run-{ix}-{cmd_ix}"))
                        .xsmall()
                        .ghost()
                        .label(t!("Assistant.Button.Run").to_string())
                        .debug_selector(move || {
                            format!("termua-assistant-msg-command-run-{ix}-{cmd_ix}")
                        })
                        .on_click(move |_, window, cx| {
                            AssistantPanelView::open_run_command_dialog(
                                this.clone(),
                                command.clone(),
                                window,
                                cx,
                            );
                        }),
                ),
            )
            .child(
                TextView::markdown(
                    gpui::ElementId::named_usize(
                        "termua-assistant-msg-command-textview",
                        ix.saturating_mul(1000).saturating_add(cmd_ix),
                    ),
                    command_md,
                )
                .selectable(true)
                .w_full()
                .min_w_0()
                .text_sm(),
            )
            .into_any_element()
    }

    fn render_message_actions_overlay(
        this: Entity<Self>,
        ix: usize,
        user_prompt: Option<String>,
        copy_text: String,
    ) -> AnyElement {
        h_flex()
            .absolute()
            .top(px(4.0))
            .right(px(4.0))
            .justify_end()
            .gap_1()
            .when_some(user_prompt, |this_row, prompt| {
                this_row.child(
                    Button::new(format!("termua-assistant-rerun-{ix}"))
                        .xsmall()
                        .ghost()
                        .debug_selector(move || format!("termua-assistant-msg-rerun-{ix}"))
                        .icon(Icon::default().path(TermuaIcon::Refresh))
                        .tooltip(t!("Assistant.Tooltip.Resend").to_string())
                        .on_click({
                            let this = this.clone();
                            move |_, window, cx| {
                                let prompt = prompt.clone();
                                let _ = this.update(cx, move |this_panel, cx| {
                                    this_panel.send_prompt_text(prompt, cx);
                                });
                                cx.refresh_windows();
                                window.refresh();
                            }
                        }),
                )
            })
            .child(
                Button::new(format!("termua-assistant-copy-{ix}"))
                    .xsmall()
                    .ghost()
                    .debug_selector(move || format!("termua-assistant-msg-copy-{ix}"))
                    .icon(Icon::new(IconName::Copy))
                    .tooltip(t!("Assistant.Tooltip.Copy").to_string())
                    .on_click(move |_, _window, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(copy_text.clone()));
                    }),
            )
            .child(
                Button::new(format!("termua-assistant-delete-{ix}"))
                    .xsmall()
                    .ghost()
                    .debug_selector(move || format!("termua-assistant-msg-delete-{ix}"))
                    .icon(Icon::new(IconName::Close))
                    .tooltip(t!("Assistant.Tooltip.Delete").to_string())
                    .on_click(move |_, window, cx| {
                        let state = cx.global_mut::<AssistantState>();
                        if ix < state.messages.len() {
                            state.messages.remove(ix);
                        }

                        let _ = this.update(cx, |_this_panel, cx| {
                            cx.notify();
                        });
                        cx.refresh_windows();
                        window.refresh();
                    }),
            )
            .into_any_element()
    }

    fn render_in_flight_row(cx: &Context<Self>) -> AnyElement {
        v_flex()
            .id("termua-assistant-in-flight")
            .w_full()
            .px_2()
            .py_1()
            .gap_0p5()
            .rounded_md()
            .border_1()
            .border_color(cx.theme().border.opacity(0.8))
            .bg(cx.theme().background.opacity(0.35))
            .child(
                h_flex()
                    .items_center()
                    .gap_1()
                    .child(
                        div()
                            .debug_selector(|| "termua-assistant-in-flight-icon".to_string())
                            .child(
                                Icon::default()
                                    .path(TermuaIcon::Bot)
                                    .size_3()
                                    .text_color(cx.theme().muted_foreground),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(t!("Assistant.Role.Assistant").to_string()),
                    ),
            )
            .child(
                h_flex()
                    .items_center()
                    .gap_1()
                    .child(Spinner::new().xsmall().color(cx.theme().muted_foreground))
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(t!("Assistant.Status.Thinking").to_string()),
                    ),
            )
            .into_any_element()
    }

    fn open_run_command_dialog(
        this: Entity<Self>,
        command: String,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(target_panel_id) = Self::target_panel_id_or_notify(this.clone(), cx) else {
            return;
        };
        let target_label = Self::target_label_for_panel_id(target_panel_id, cx);

        let Some(Some(root)) = window.root::<gpui_component::Root>() else {
            return;
        };

        root.update(cx, |root, cx| {
            Self::open_run_command_dialog_in_root(
                root,
                this.clone(),
                target_panel_id,
                target_label.clone(),
                command.clone(),
                window,
                cx,
            );
        });
    }

    fn target_panel_id_or_notify(this: Entity<Self>, cx: &mut App) -> Option<usize> {
        let Some(target_panel_id) = cx.global::<AssistantState>().target_panel_id else {
            this.update(cx, |_, cx| {
                cx.global_mut::<AssistantState>().push(
                    AssistantRole::System,
                    t!("Assistant.Message.NoTargetTerminalSelected").to_string(),
                );
                cx.notify();
            });
            return None;
        };
        Some(target_panel_id)
    }

    fn target_label_for_panel_id(target_panel_id: usize, cx: &App) -> String {
        crate::assistant::target_label(cx, target_panel_id)
            .map(|label| label.to_string())
            .unwrap_or_else(|| t!("Assistant.Target.Fallback", id = target_panel_id).to_string())
    }

    fn open_run_command_dialog_in_root(
        root: &mut gpui_component::Root,
        this: Entity<Self>,
        target_panel_id: usize,
        target_label: String,
        command: String,
        window: &mut Window,
        cx: &mut Context<gpui_component::Root>,
    ) {
        root.open_dialog(
            move |dialog, _window, _app| {
                dialog
                    .title(t!("Assistant.Dialog.RunInTerminalTitle").to_string())
                    .w(px(720.))
                    .child(Self::run_command_dialog_body(
                        target_label.clone(),
                        &command,
                    ))
                    .button_props(
                        gpui_component::dialog::DialogButtonProps::default()
                            .ok_text(t!("Assistant.Dialog.RunInTerminalOk").to_string())
                            .cancel_text(t!("Assistant.Dialog.RunInTerminalCancel").to_string())
                            .show_cancel(true),
                    )
                    .on_ok({
                        let this = this.clone();
                        let command = command.clone();
                        let target_label = target_label.clone();
                        move |_, _window, cx| {
                            Self::send_command_to_target(
                                this.clone(),
                                target_panel_id,
                                target_label.clone(),
                                command.clone(),
                                cx,
                            );
                            true
                        }
                    })
            },
            window,
            cx,
        );
    }

    fn run_command_dialog_body(target_label: String, command: &str) -> AnyElement {
        let command_md = format!("```sh\n{command}\n```");
        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .items_start()
                    .child(div().child(t!("Assistant.Label.Target").to_string()))
                    .child(
                        div().min_w_0().child(
                            TextView::markdown("termua-assistant-run-target", target_label)
                                .selectable(true),
                        ),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_start()
                    .child(div().child(t!("Assistant.Label.Command").to_string()))
                    .child(
                        div().min_w_0().child(
                            TextView::markdown("termua-assistant-run-command", command_md)
                                .selectable(true),
                        ),
                    ),
            )
            .into_any_element()
    }

    fn send_command_to_target(
        this: Entity<Self>,
        target_panel_id: usize,
        target_label: String,
        command: String,
        cx: &mut App,
    ) {
        let _ = this.update(cx, move |this_panel, cx| {
            let input = format!("{}\n", command.trim_end()).into_bytes();
            let message = match crate::assistant::send_input_to_target(cx, target_panel_id, input) {
                Ok(()) => t!("Assistant.Message.SentCommand", target = target_label).to_string(),
                Err(crate::assistant::SendInputError::RegistryUnavailable) => {
                    t!("Assistant.Message.TerminalRegistryUnavailable").to_string()
                }
                Err(crate::assistant::SendInputError::TargetUnavailable) => {
                    t!("Assistant.Message.TargetTerminalUnavailable").to_string()
                }
            };

            cx.global_mut::<AssistantState>()
                .push(AssistantRole::System, message);
            this_panel.scroll_messages_to_bottom(cx);
            cx.notify();
        });
    }
}

impl Render for AssistantPanelView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        let this = cx.entity();

        if self.remaining_scroll_to_bottom_passes > 0 {
            let this_for_defer = this.clone();
            let scroll_handle = self.scroll_handle.clone();
            window.defer(cx, move |_, cx| {
                let _ = this_for_defer.update(cx, |this, cx| {
                    this.remaining_scroll_to_bottom_passes =
                        this.remaining_scroll_to_bottom_passes.saturating_sub(1);
                    scroll_handle.scroll_to_bottom();
                    cx.notify();
                });
            });
        }

        self.sync_target_selection(cx);

        let state = cx.global::<AssistantState>();
        let in_flight = state.in_flight;
        let entries = state.messages.clone();
        let can_rerun_user_messages = Self::compute_can_rerun_user_messages(&entries, in_flight);
        let attach_selection = state.attach_selection;
        let attach_terminal_context = state.attach_terminal_context;
        let target_panel_id = state.target_panel_id;
        let follow_focus = state.target_follows_focus;
        let assistant_enabled = cx
            .try_global::<crate::settings::AssistantSettings>()
            .map(|s| s.enabled)
            .unwrap_or(true);
        let tab_header_height = px(32.);

        let target_label: SharedString = target_panel_id
            .and_then(|id| crate::assistant::target_label(cx, id))
            .unwrap_or_else(|| t!("Assistant.Target.None").to_string().into());
        let entries_empty = entries.is_empty();

        v_flex()
            .id("termua-assistant-view")
            .size_full()
            .min_h_0()
            .items_stretch()
            .child(self.render_header_bar(
                tab_header_height,
                target_label,
                follow_focus,
                target_panel_id,
                entries_empty,
                in_flight,
                cx,
            ))
            .child(self.render_messages_area(this, entries, can_rerun_user_messages, in_flight, cx))
            .child(self.render_prompt_bar(
                in_flight,
                assistant_enabled,
                attach_selection,
                attach_terminal_context,
                cx,
            ))
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use gpui::{AvailableSpace, point, px, size};

    use super::*;

    fn init_test_app(app: &mut gpui::App) {
        gpui_component::init(app);
        gpui_term::init(app);
        crate::settings::set_language(crate::settings::Language::English, app);
        app.set_global(crate::settings::AssistantSettings {
            enabled: false,
            ..Default::default()
        });
    }

    #[gpui::test]
    fn assistant_message_text_is_rendered_by_textview_and_send_scrolls_to_bottom(
        cx: &mut gpui::TestAppContext,
    ) {
        let assistant_entity_slot: Rc<RefCell<Option<gpui::Entity<AssistantPanelView>>>> =
            Rc::new(RefCell::new(None));

        cx.update(|app| init_test_app(app));

        let slot_for_window = assistant_entity_slot.clone();
        let (root, window_cx) = cx.add_window_view(move |window, cx| {
            let assistant = cx.new(|cx| AssistantPanelView::new(window, cx));
            *slot_for_window.borrow_mut() = Some(assistant.clone());
            gpui_component::Root::new(assistant, window, cx)
        });

        let assistant = assistant_entity_slot
            .borrow()
            .clone()
            .expect("assistant view should be created");

        window_cx.update(|_window, app| {
            // Seed enough content to make the scroll area overflow.
            for ix in 0..80 {
                app.global_mut::<AssistantState>()
                    .push(AssistantRole::Assistant, format!("message {ix}\nline 2"));
            }
        });

        let root_for_draw = root.clone();
        window_cx.draw(
            point(px(0.), px(0.)),
            size(
                AvailableSpace::Definite(px(520.)),
                AvailableSpace::Definite(px(240.)),
            ),
            move |_, _| div().size_full().child(root_for_draw),
        );
        window_cx.run_until_parked();

        window_cx
            .debug_bounds("termua-assistant-msg-text-0")
            .expect("assistant message content should be rendered as a TextView");

        window_cx.update(|window, app| {
            assistant.update(app, |this, cx| {
                this.prompt_input
                    .update(cx, |state, cx| state.set_value("hello", window, cx));
                this.send(window, cx);
            });
        });

        let root_for_redraw = root.clone();
        window_cx.draw(
            point(px(0.), px(0.)),
            size(
                AvailableSpace::Definite(px(520.)),
                AvailableSpace::Definite(px(240.)),
            ),
            move |_, _| div().size_full().child(root_for_redraw),
        );
        window_cx.run_until_parked();

        window_cx.draw(
            point(px(0.), px(0.)),
            size(
                AvailableSpace::Definite(px(520.)),
                AvailableSpace::Definite(px(240.)),
            ),
            move |_, _| div().size_full().child(root),
        );
        window_cx.run_until_parked();

        let (offset_y, max_y) = window_cx.update(|_window, app| {
            let view = assistant.read(app);
            (
                view.scroll_handle.offset().y,
                view.scroll_handle.max_offset().y,
            )
        });

        assert!(max_y > px(0.), "expected overflow content to be scrollable");
        let distance_from_bottom = offset_y + max_y;
        assert!(
            distance_from_bottom >= px(0.) && distance_from_bottom <= px(48.),
            "expected assistant view to scroll close to bottom, got offset_y={offset_y:?} \
             max_y={max_y:?}"
        );
    }

    #[gpui::test]
    fn assistant_prompt_renders_options_and_send_buttons(cx: &mut gpui::TestAppContext) {
        cx.update(|app| init_test_app(app));

        let (root, window_cx) = cx.add_window_view(|window, cx| {
            let assistant = cx.new(|cx| AssistantPanelView::new(window, cx));
            gpui_component::Root::new(assistant, window, cx)
        });

        window_cx.draw(
            point(px(0.), px(0.)),
            size(
                AvailableSpace::Definite(px(520.)),
                AvailableSpace::Definite(px(240.)),
            ),
            move |_, _| div().size_full().child(root),
        );
        window_cx.run_until_parked();

        window_cx
            .debug_bounds("termua-assistant-send-dropdown")
            .expect("expected assistant prompt dropdown send button");
        window_cx
            .debug_bounds("termua-assistant-send-button")
            .expect("expected assistant prompt send button");
        window_cx
            .debug_bounds("termua-assistant-header-icon")
            .expect("expected assistant header icon to render");
    }

    #[gpui::test]
    fn assistant_in_flight_card_renders_bot_icon(cx: &mut gpui::TestAppContext) {
        cx.update(|app| init_test_app(app));

        let (root, window_cx) = cx.add_window_view(|window, cx| {
            let assistant = cx.new(|cx| AssistantPanelView::new(window, cx));
            gpui_component::Root::new(assistant, window, cx)
        });

        window_cx.update(|_window, app| {
            app.global_mut::<AssistantState>().in_flight = true;
        });

        window_cx.draw(
            point(px(0.), px(0.)),
            size(
                AvailableSpace::Definite(px(520.)),
                AvailableSpace::Definite(px(240.)),
            ),
            move |_, _| div().size_full().child(root),
        );
        window_cx.run_until_parked();

        window_cx
            .debug_bounds("termua-assistant-in-flight-icon")
            .expect("expected assistant in-flight card to render a bot icon");
    }

    #[gpui::test]
    fn assistant_message_delete_button_is_rightmost_and_above_body(cx: &mut gpui::TestAppContext) {
        cx.update(|app| init_test_app(app));

        let (root, window_cx) = cx.add_window_view(|window, cx| {
            let assistant = cx.new(|cx| AssistantPanelView::new(window, cx));
            gpui_component::Root::new(assistant, window, cx)
        });

        window_cx.update(|_window, app| {
            app.global_mut::<AssistantState>()
                .push(AssistantRole::Assistant, "hello\nworld");
        });

        window_cx.draw(
            point(px(0.), px(0.)),
            size(
                AvailableSpace::Definite(px(520.)),
                AvailableSpace::Definite(px(240.)),
            ),
            move |_, _| div().size_full().child(root),
        );
        window_cx.run_until_parked();

        let card = window_cx
            .debug_bounds("termua-assistant-msg-card-0")
            .expect("assistant message card should be debuggable");
        let copy_button = window_cx
            .debug_bounds("termua-assistant-msg-copy-0")
            .expect("assistant message copy button should be debuggable");
        let delete_button = window_cx
            .debug_bounds("termua-assistant-msg-delete-0")
            .expect("assistant message delete button should be debuggable");
        let message_text = window_cx
            .debug_bounds("termua-assistant-msg-text-0")
            .expect("assistant message text should be debuggable");

        assert!(
            delete_button.origin.y < message_text.origin.y,
            "expected delete button to be above message body; delete_y={:?}, text_y={:?}",
            delete_button.origin.y,
            message_text.origin.y
        );

        let card_right = card.origin.x + card.size.width;
        let delete_right = delete_button.origin.x + delete_button.size.width;
        assert!(
            delete_right > card_right - px(12.0),
            "expected delete button to be near the card's right edge; card_right={card_right:?}, \
             delete_right={delete_right:?}",
        );

        assert!(
            copy_button.origin.x + copy_button.size.width <= delete_button.origin.x,
            "expected delete button to be right of copy button; copy_right={:?}, delete_x={:?}",
            copy_button.origin.x + copy_button.size.width,
            delete_button.origin.x,
        );
    }

    #[gpui::test]
    fn assistant_message_delete_button_removes_only_that_message(cx: &mut gpui::TestAppContext) {
        cx.update(|app| init_test_app(app));

        let (root, window_cx) = cx.add_window_view(|window, cx| {
            let assistant = cx.new(|cx| AssistantPanelView::new(window, cx));
            gpui_component::Root::new(assistant, window, cx)
        });

        window_cx.update(|_window, app| {
            app.global_mut::<AssistantState>()
                .push(AssistantRole::Assistant, "a");
            app.global_mut::<AssistantState>()
                .push(AssistantRole::Assistant, "b");
        });

        window_cx.draw(
            point(px(0.), px(0.)),
            size(
                AvailableSpace::Definite(px(520.)),
                AvailableSpace::Definite(px(240.)),
            ),
            move |_, _| div().size_full().child(root),
        );
        window_cx.run_until_parked();

        let delete_bounds = window_cx
            .debug_bounds("termua-assistant-msg-delete-0")
            .expect("expected assistant message to render a delete button");

        window_cx.simulate_click(delete_bounds.center(), gpui::Modifiers::none());
        window_cx.run_until_parked();

        let remaining = window_cx.update(|_window, app| {
            app.global::<AssistantState>()
                .messages
                .iter()
                .map(|m| m.content.to_string())
                .collect::<Vec<_>>()
        });
        assert_eq!(remaining, vec!["b".to_string()]);
    }

    #[gpui::test]
    fn assistant_reply_with_multiple_commands_renders_run_button_per_command(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|app| init_test_app(app));

        let (root, window_cx) = cx.add_window_view(|window, cx| {
            let assistant = cx.new(|cx| AssistantPanelView::new(window, cx));
            gpui_component::Root::new(assistant, window, cx)
        });

        window_cx.update(|_window, app| {
            app.global_mut::<AssistantState>().push(
                AssistantRole::Assistant,
                "Try:\n```sh\nls -la\npwd\n```\nDone.",
            );
        });

        window_cx.draw(
            point(px(0.), px(0.)),
            size(
                AvailableSpace::Definite(px(520.)),
                AvailableSpace::Definite(px(240.)),
            ),
            move |_, _| div().size_full().child(root),
        );
        window_cx.run_until_parked();

        window_cx
            .debug_bounds("termua-assistant-msg-command-run-0-0")
            .expect("expected first command in assistant reply to have a Run button");
        window_cx
            .debug_bounds("termua-assistant-msg-command-run-0-1")
            .expect("expected second command in assistant reply to have a Run button");
    }

    // Intentionally no assistant "tool" UI in the panel. Terminal context (if enabled)
    // is injected into the request payload, not exposed as tool calls.

    #[gpui::test]
    fn assistant_user_messages_render_rerun_button_after_assistant_reply(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|app| init_test_app(app));

        let (root, window_cx) = cx.add_window_view(|window, cx| {
            let assistant = cx.new(|cx| AssistantPanelView::new(window, cx));
            gpui_component::Root::new(assistant, window, cx)
        });

        window_cx.update(|_window, app| {
            app.global_mut::<AssistantState>()
                .push(AssistantRole::User, "ls -la");
            app.global_mut::<AssistantState>()
                .push(AssistantRole::Assistant, "ok");
        });

        window_cx.draw(
            point(px(0.), px(0.)),
            size(
                AvailableSpace::Definite(px(520.)),
                AvailableSpace::Definite(px(240.)),
            ),
            move |_, _| div().size_full().child(root),
        );
        window_cx.run_until_parked();

        window_cx
            .debug_bounds("termua-assistant-msg-rerun-0")
            .expect("expected user messages to render a rerun button");
    }

    #[gpui::test]
    fn assistant_user_messages_hide_rerun_button_before_assistant_reply(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|app| init_test_app(app));

        let (root, window_cx) = cx.add_window_view(|window, cx| {
            let assistant = cx.new(|cx| AssistantPanelView::new(window, cx));
            gpui_component::Root::new(assistant, window, cx)
        });

        window_cx.update(|_window, app| {
            let state = app.global_mut::<AssistantState>();
            state.push(AssistantRole::User, "ls -la");
            state.in_flight = true;
        });

        window_cx.draw(
            point(px(0.), px(0.)),
            size(
                AvailableSpace::Definite(px(520.)),
                AvailableSpace::Definite(px(240.)),
            ),
            move |_, _| div().size_full().child(root),
        );
        window_cx.run_until_parked();

        assert!(
            window_cx
                .debug_bounds("termua-assistant-msg-rerun-0")
                .is_none(),
            "expected rerun button to be hidden before assistant reply"
        );
    }

    #[gpui::test]
    fn assistant_only_hides_rerun_for_last_user_message_while_in_flight(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|app| init_test_app(app));

        let (root, window_cx) = cx.add_window_view(|window, cx| {
            let assistant = cx.new(|cx| AssistantPanelView::new(window, cx));
            gpui_component::Root::new(assistant, window, cx)
        });

        window_cx.update(|_window, app| {
            let state = app.global_mut::<AssistantState>();
            state.push(AssistantRole::User, "first");
            state.push(AssistantRole::User, "second");
            state.in_flight = true;
        });

        window_cx.draw(
            point(px(0.), px(0.)),
            size(
                AvailableSpace::Definite(px(520.)),
                AvailableSpace::Definite(px(240.)),
            ),
            move |_, _| div().size_full().child(root),
        );
        window_cx.run_until_parked();

        window_cx
            .debug_bounds("termua-assistant-msg-rerun-0")
            .expect("expected earlier user message to still render a rerun button");
        assert!(
            window_cx
                .debug_bounds("termua-assistant-msg-rerun-1")
                .is_none(),
            "expected the last user message to hide rerun while awaiting assistant reply"
        );
    }

    // Intentionally no "copy selected text" / context-menu copy behavior in the assistant panel.
}
