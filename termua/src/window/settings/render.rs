use gpui::{
    AnyElement, AppContext, Context, InteractiveElement, InteractiveText, IntoElement,
    ParentElement, StatefulInteractiveElement, Styled, StyledImage, StyledText, Window, div, img,
    prelude::FluentBuilder, px,
};
use gpui_common::TermuaIcon;
use gpui_component::{
    ActiveTheme, IconName, Sizable, WindowExt,
    button::{Button, ButtonVariants},
    h_flex,
    link::Link,
    scroll::{Scrollbar, ScrollbarShow},
    tooltip::Tooltip,
    v_flex,
};
use rust_i18n::t;

use super::{
    SettingMeta, SettingsPage, SettingsWindow,
    keybindings::{keybinding_warning_for_setting_id, terminal_keybinding_conflicts},
    search_settings,
    state::page_spec,
};
use crate::window::{settings::state::ssh_backend_docs_url, theme_editor::ThemeEditor};

impl SettingsWindow {
    fn render_terminal_keybindings_table(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let keybinding_conflicts = terminal_keybinding_conflicts(&self.settings);

        let rows = SettingMeta::all()
            .iter()
            .filter(|meta| meta.page == SettingsPage::TerminalKeyBindings)
            .map(|meta| {
                let title = meta.localized_title();
                let description = meta.localized_description();
                let tooltip_selector = format!("termua-settings-keybinding-tooltip-{}", meta.id);
                let warning = keybinding_warning_for_setting_id(
                    meta.id,
                    self.terminal_keybinding_value(meta.id),
                    &keybinding_conflicts,
                );
                h_flex()
                    .debug_selector(move || format!("termua-settings-keybinding-row-{}", meta.id))
                    .w_full()
                    .items_stretch()
                    .border_b_1()
                    .border_color(cx.theme().border.opacity(0.45))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .justify_center()
                            .gap_1()
                            .px_3()
                            .py_2()
                            .child(
                                div()
                                    .debug_selector(move || {
                                        format!("termua-settings-keybinding-title-{}", meta.id)
                                    })
                                    .flex_1()
                                    .min_w_0()
                                    .child(
                                        self.render_setting_title(
                                            InteractiveText::new(
                                                format!(
                                                    "termua-settings-keybinding-title-text-{}",
                                                    meta.id
                                                ),
                                                StyledText::new(title),
                                            )
                                            .tooltip({
                                                move |_ix, window, cx| {
                                                    let tooltip_selector = tooltip_selector.clone();
                                                    let description = description.clone();
                                                    Some(
                                                        Tooltip::element({
                                                            move |_window, cx| {
                                                                div()
                                                                    .debug_selector({
                                                                        let tooltip_selector =
                                                                            tooltip_selector
                                                                                .clone();
                                                                        move || tooltip_selector
                                                                    })
                                                                    .max_w(px(320.))
                                                                    .text_xs()
                                                                    .text_color(
                                                                        cx.theme().foreground,
                                                                    )
                                                                    .whitespace_normal()
                                                                    .child(description.clone())
                                                            }
                                                        })
                                                        .build(window, cx),
                                                    )
                                                }
                                            }),
                                            cx,
                                        ),
                                    ),
                            )
                            .when_some(warning, |this, warning| {
                                this.child(self.render_setting_warning(warning, cx))
                            }),
                    )
                    .child(
                        div()
                            .debug_selector(move || {
                                format!("termua-settings-keybinding-divider-{}", meta.id)
                            })
                            .w(px(1.))
                            .bg(cx.theme().border.opacity(0.6)),
                    )
                    .child(
                        div()
                            .w(px(300.))
                            .flex_shrink_0()
                            .min_h_full()
                            .child(self.render_control_for_setting(meta.id, window, cx)),
                    )
                    .into_any_element()
            })
            .collect::<Vec<_>>();

        v_flex()
            .debug_selector(|| "termua-settings-keybindings-table".to_string())
            .w_full()
            .border_1()
            .border_color(cx.theme().border.opacity(0.6))
            .rounded_lg()
            .overflow_hidden()
            .children(rows)
            .into_any_element()
    }

    fn render_terminal_ssh_backend_description(
        &self,
        description: impl IntoElement,
    ) -> impl IntoElement {
        let backend = self.settings.terminal.ssh_backend;
        let (selector, link_id, label) = match backend {
            gpui_term::SshBackend::Ssh2 => (
                "termua-settings-terminal-ssh-backend-link-ssh2",
                "settings-terminal-ssh-backend-link-ssh2",
                "docs.rs/ssh2",
            ),
            gpui_term::SshBackend::Libssh => (
                "termua-settings-terminal-ssh-backend-link-libssh",
                "settings-terminal-ssh-backend-link-libssh",
                "docs.rs/libssh-rs",
            ),
        };

        h_flex()
            .items_center()
            .gap_4()
            .debug_selector(|| {
                "termua-settings-terminal-ssh-backend-description-inline".to_string()
            })
            .child(div().flex_1().min_w_0().child(description))
            .child(
                div()
                    .w(px(240.))
                    .flex_shrink_0()
                    .debug_selector(|| {
                        "termua-settings-terminal-ssh-backend-link-column".to_string()
                    })
                    .text_left()
                    .child(
                        div().debug_selector(move || selector.to_string()).child(
                            Link::new(link_id)
                                .href(ssh_backend_docs_url(backend))
                                .text_xs()
                                .child(label),
                        ),
                    ),
            )
    }

    fn open_new_theme_sheet(window: &mut Window, app: &mut gpui::App) {
        let original_theme = gpui_component::Theme::global(app).clone();
        let editor = app.new(|cx| ThemeEditor::new(window, cx));
        let editor_for_footer = editor.clone();

        window.open_sheet(app, move |sheet, _window, _cx| {
            sheet
                .title(t!("Settings.Theme.NewTheme").to_string())
                .size(px(560.))
                .overlay_closable(false)
                .on_close({
                    let original_theme = original_theme.clone();
                    move |_, _window, cx| {
                        *gpui_component::Theme::global_mut(cx) = original_theme.clone();
                        gpui_component::Theme::change(original_theme.mode, None, cx);
                        cx.refresh_windows();
                    }
                })
                .footer(
                    h_flex()
                        .w_full()
                        .justify_end()
                        .gap_2()
                        .debug_selector(|| "termua-theme-editor-footer".to_string())
                        .child(
                            Button::new("termua-theme-editor-cancel-button")
                                .debug_selector(|| "termua-theme-editor-cancel-button".to_string())
                                .label(t!("Settings.Button.Cancel").to_string())
                                .small()
                                .ghost()
                                .on_click({
                                    let original_theme = original_theme.clone();
                                    move |_, window, cx| {
                                        window.close_sheet(cx);
                                        *gpui_component::Theme::global_mut(cx) =
                                            original_theme.clone();
                                        gpui_component::Theme::change(
                                            original_theme.mode,
                                            None,
                                            cx,
                                        );
                                        cx.refresh_windows();
                                    }
                                }),
                        )
                        .child(
                            Button::new("termua-theme-editor-save-button")
                                .debug_selector(|| "termua-theme-editor-save-button".to_string())
                                .label(t!("Settings.Button.Save").to_string())
                                .small()
                                .primary()
                                .on_click({
                                    let editor_for_footer = editor_for_footer.clone();
                                    let original_theme = original_theme.clone();
                                    move |_, window, cx| {
                                        let payload = cx
                                            .read_entity(&editor_for_footer, |editor, cx| {
                                                editor.save_payload(cx)
                                            });
                                        let themes_dir = crate::theme_manager::themes_dir_path();
                                        crate::window::theme_editor::write_theme_set(
                                            &themes_dir,
                                            &payload,
                                        );

                                        window.close_sheet(cx);
                                        *gpui_component::Theme::global_mut(cx) =
                                            original_theme.clone();
                                        gpui_component::Theme::change(
                                            original_theme.mode,
                                            None,
                                            cx,
                                        );
                                        cx.refresh_windows();
                                    }
                                }),
                        ),
                )
                .child(editor.clone())
        });
    }

    fn render_appearance_theme_page_heading(
        &self,
        heading: String,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        h_flex()
            .debug_selector(|| "termua-settings-page-heading".to_string())
            .items_center()
            .gap_2()
            .child(
                div()
                    .debug_selector(|| "termua-settings-appearance-theme-heading-icon".to_string())
                    .w(px(18.))
                    .h(px(18.))
                    .flex_shrink_0()
                    .child(
                        img(TermuaIcon::PaletteRainbow)
                            .w_full()
                            .h_full()
                            .object_fit(gpui::ObjectFit::Contain),
                    ),
            )
            .child(
                div()
                    .debug_selector(|| "termua-settings-page-heading-text".to_string())
                    .text_lg()
                    .text_color(cx.theme().foreground)
                    .child(heading),
            )
            .child(div().flex_1())
            .child(
                Button::new("termua-settings-appearance-theme-new-theme-button")
                    .debug_selector(|| {
                        "termua-settings-appearance-theme-new-theme-button".to_string()
                    })
                    .icon(IconName::Plus)
                    .tooltip(t!("Settings.Theme.NewTheme").to_string())
                    .small()
                    .on_click(|_, window, app| {
                        Self::open_new_theme_sheet(window, app);
                    }),
            )
            .into_any_element()
    }

    fn render_simple_page_heading(&self, heading: String, cx: &mut Context<Self>) -> AnyElement {
        div()
            .debug_selector(|| "termua-settings-page-heading".to_string())
            .child(
                div()
                    .debug_selector(|| "termua-settings-page-heading-text".to_string())
                    .text_lg()
                    .text_color(cx.theme().foreground)
                    .child(heading),
            )
            .into_any_element()
    }

    fn render_setting_rows<'a, I, FTitle, FWarn>(
        &self,
        metas: I,
        window: &mut Window,
        cx: &mut Context<Self>,
        title_for_meta: FTitle,
        warning_for_meta: FWarn,
    ) -> Vec<AnyElement>
    where
        I: IntoIterator<Item = &'a SettingMeta>,
        FTitle: Fn(&SettingMeta) -> String,
        FWarn: Fn(&SettingMeta) -> Option<String>,
    {
        let mut rows: Vec<AnyElement> = Vec::new();
        for meta in metas {
            let control = self.render_control_for_setting(meta.id, window, cx);
            let warning = warning_for_meta(meta);
            if meta.id == "terminal.suggestions_json_dir" {
                let path = crate::static_suggestions::suggestions_dir_path()
                    .display()
                    .to_string();
                let hint = t!(
                    "Settings.Meta.terminal.suggestions_json_dir.Hint",
                    path = path
                )
                .to_string();

                rows.push(
                    self.render_setting_row_with_warning(
                        title_for_meta(meta),
                        v_flex().gap_1().child(meta.localized_description()).child(
                            div()
                                .debug_selector(|| {
                                    "termua-settings-suggestions-json-dir-hint".to_string()
                                })
                                .child(hint),
                        ),
                        control,
                        warning,
                        cx,
                    )
                    .into_any_element(),
                );
            } else if meta.id == "terminal.ssh_backend" {
                rows.push(
                    self.render_setting_row_with_warning(
                        title_for_meta(meta),
                        self.render_terminal_ssh_backend_description(meta.localized_description()),
                        control,
                        warning,
                        cx,
                    )
                    .into_any_element(),
                );
            } else {
                rows.push(
                    self.render_setting_row_with_warning(
                        title_for_meta(meta),
                        meta.localized_description(),
                        control,
                        warning,
                        cx,
                    )
                    .into_any_element(),
                );
            }
        }
        rows
    }

    fn render_page_heading(
        &self,
        page: SettingsPage,
        heading: String,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match page {
            SettingsPage::AppearanceTheme => self.render_appearance_theme_page_heading(heading, cx),
            _ => self.render_simple_page_heading(heading, cx),
        }
    }

    pub(super) fn render_search_results(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let query = self.search_query(cx);
        let entries = SettingMeta::all();
        let results = search_settings(entries, &query);

        let keybinding_conflicts = terminal_keybinding_conflicts(&self.settings);
        let rows = self.render_setting_rows(
            results,
            window,
            cx,
            |meta| meta.localized_title(),
            |meta| {
                keybinding_warning_for_setting_id(
                    meta.id,
                    self.terminal_keybinding_value(meta.id),
                    &keybinding_conflicts,
                )
            },
        );

        v_flex()
            .gap_2()
            .child(
                div().text_lg().text_color(cx.theme().foreground).child(
                    t!(
                        "Settings.Search.ResultsHeading",
                        count = rows.len().to_string()
                    )
                    .to_string(),
                ),
            )
            .when(rows.is_empty(), |this| {
                this.child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(t!("Settings.Search.NoMatches").to_string()),
                )
            })
            .children(rows)
    }

    pub(super) fn render_page(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let selected_page = self.selected_page;
        let spec = page_spec(selected_page);

        let heading_el =
            self.render_page_heading(selected_page, t!(spec.heading_key).to_string(), cx);

        let mut page = v_flex().gap_2().child(heading_el);
        if let Some(hint) = spec.hint_key {
            page = page.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(t!(hint).to_string()),
            );
        }

        if matches!(selected_page, SettingsPage::TerminalKeyBindings) {
            return page.child(self.render_terminal_keybindings_table(window, cx));
        }

        let keybinding_conflicts = terminal_keybinding_conflicts(&self.settings);

        let rows = self.render_setting_rows(
            SettingMeta::all()
                .iter()
                .filter(|m| m.page == selected_page),
            window,
            cx,
            |meta| meta.localized_title(),
            |meta| match meta.id {
                "assistant.status" => self
                    .assistant_service_error
                    .as_ref()
                    .map(ToString::to_string),
                "assistant.model" => self
                    .assistant_model_fetch_error
                    .as_ref()
                    .map(ToString::to_string),
                _ => keybinding_warning_for_setting_id(
                    meta.id,
                    self.terminal_keybinding_value(meta.id),
                    &keybinding_conflicts,
                ),
            },
        );

        page.children(rows)
    }

    pub(super) fn render_right_pane(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let query = self.search_query(cx);
        let content = if query.trim().is_empty() {
            self.render_page(window, cx).into_any_element()
        } else {
            self.render_search_results(window, cx).into_any_element()
        };

        // Right pane is a scrollable content area + a fixed-width scrollbar gutter.
        // Keeping the scrollbar in its own gutter avoids overlapping the controls.
        h_flex()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .items_stretch()
            .child(
                div()
                    .id("termua-settings-right-scroll-area")
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .flex_col()
                    .track_scroll(&self.right_scroll_handle)
                    .overflow_y_scroll()
                    .overflow_x_hidden()
                    .child(
                        // Ensure the content takes its natural height so the scroll container
                        // can compute a non-zero max scroll offset when it overflows.
                        div().w_full().p_3().flex_none().child(
                            // Center the settings page content horizontally (Zed-like),
                            // while keeping it top-aligned.
                            div().w_full().max_w(px(900.0)).mx_auto().child(content),
                        ),
                    ),
            )
            .child(
                div()
                    .w(px(16.0))
                    .flex_shrink_0()
                    .relative()
                    .h_full()
                    .min_h_0()
                    .child(
                        Scrollbar::vertical(&self.right_scroll_handle)
                            .id("termua-settings-right-scrollbar")
                            .scrollbar_show(ScrollbarShow::Always),
                    ),
            )
    }
}
