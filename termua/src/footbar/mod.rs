use gpui::{
    App, Context, InteractiveElement, IntoElement, ParentElement, Render, SharedString, Styled,
    Subscription, Window, div, prelude::FluentBuilder as _, px,
};
use gpui_common::{TermuaIcon, format_bytes};
use gpui_component::{
    ActiveTheme as _, Icon, Selectable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
};
use gpui_transfer::TransferCenterState;
use rust_i18n::t;

use crate::{
    TermuaAppState,
    globals::{ensure_ctx_global, ensure_ctx_global_with},
    lock_screen, notification,
    right_sidebar::{RightSidebarState, RightSidebarTab},
};

mod transfers;

pub(crate) struct FootbarView {
    _observe_app_state: Subscription,
    _observe_messages: Subscription,
    _observe_right_sidebar: Subscription,
    _observe_transfers: Subscription,
    transfers_open: bool,
}

impl FootbarView {
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        ensure_ctx_global_with(cx, lock_screen::LockState::new_default);
        ensure_ctx_global::<notification::NotifyState, _>(cx);
        ensure_ctx_global::<RightSidebarState, _>(cx);
        ensure_ctx_global::<TransferCenterState, _>(cx);

        // Keep the footbar reactive to global state changes.
        let app_state_sub = cx.observe_global::<TermuaAppState>(|_, cx| cx.notify());
        let messages_sub = cx.observe_global::<notification::NotifyState>(|_, cx| cx.notify());
        let right_sidebar_sub = cx.observe_global::<RightSidebarState>(|_, cx| cx.notify());
        let transfers_sub = cx.observe_global::<TransferCenterState>(|_, cx| cx.notify());
        Self {
            _observe_app_state: app_state_sub,
            _observe_messages: messages_sub,
            _observe_right_sidebar: right_sidebar_sub,
            _observe_transfers: transfers_sub,
            transfers_open: false,
        }
    }

    fn colors_for_theme(theme: &gpui_component::Theme) -> (gpui::Hsla, gpui::Hsla) {
        (theme.title_bar_border.opacity(0.7), theme.title_bar)
    }

    fn multi_exec_icon_path(enabled: bool) -> TermuaIcon {
        if enabled {
            TermuaIcon::Dice4
        } else {
            TermuaIcon::Dice1
        }
    }

    fn set_transfers_open(&mut self, open: bool, cx: &mut Context<Self>) {
        self.transfers_open = open;
        cx.notify();
    }

    fn render_controls_left(&self, sessions_visible: bool) -> gpui::AnyElement {
        h_flex()
            .items_center()
            .gap_1()
            .child(
                Button::new("termua-footbar-sessions-button")
                    .xsmall()
                    .compact()
                    .ghost()
                    .icon(Icon::default().path(TermuaIcon::List))
                    .tooltip(t!("Footbar.Tooltip.Sessions").to_string())
                    .selected(sessions_visible)
                    .debug_selector(|| "termua-footbar-sessions".to_string())
                    .on_click(|_, _, cx| {
                        crate::menu::toggle_sessions_sidebar(&crate::ToggleSessionsSidebar, cx)
                    }),
            )
            .into_any_element()
    }

    fn render_controls_right(
        &self,
        enabled: bool,
        icon_path: TermuaIcon,
        lock_enabled: bool,
        lock_tooltip: SharedString,
        messages_selected: bool,
        assistant_selected: bool,
    ) -> gpui::AnyElement {
        h_flex()
            .items_center()
            .gap_1()
            .child(
                Button::new("termua-footbar-issues-link")
                    .xsmall()
                    .compact()
                    .link()
                    .icon(Icon::default().path(TermuaIcon::Bug))
                    .label(t!("Footbar.Issues.Label").to_string())
                    .tooltip(t!("Footbar.Issues.Tooltip").to_string())
                    .debug_selector(|| "termua-footbar-issues".to_string())
                    .on_click(|_, _, cx| {
                        cx.open_url("https://github.com/iamazy/termua/issues");
                    }),
            )
            .child(
                Button::new("termua-footbar-multi-exec-button")
                    .xsmall()
                    .compact()
                    .ghost()
                    .icon(Icon::default().path(icon_path))
                    .tooltip(t!("Footbar.Tooltip.MultiExecute").to_string())
                    .selected(enabled)
                    .debug_selector(|| "termua-footbar-multi-exec".to_string())
                    .on_click(|_, _, cx| {
                        crate::menu::toggle_multi_exec(&crate::ToggleMultiExec, cx)
                    }),
            )
            .when(lock_enabled, |this| {
                this.child(
                    Button::new("termua-footbar-lock-button")
                        .xsmall()
                        .compact()
                        .ghost()
                        .icon(Icon::default().path(TermuaIcon::LockOpen))
                        .tooltip(lock_tooltip)
                        .debug_selector(|| "termua-footbar-lock".to_string())
                        .on_click(|_, _window: &mut Window, cx: &mut App| {
                            cx.global_mut::<lock_screen::LockState>().lock_now();
                            cx.refresh_windows();
                        }),
                )
            })
            .child(
                Button::new("termua-footbar-messages-button")
                    .xsmall()
                    .compact()
                    .ghost()
                    .icon(Icon::default().path(TermuaIcon::Message))
                    .tooltip(t!("Footbar.Tooltip.Messages").to_string())
                    .selected(messages_selected)
                    .debug_selector(|| "termua-footbar-messages".to_string())
                    .on_click(|_, _, cx| {
                        crate::menu::toggle_messages_sidebar(&crate::ToggleMessagesSidebar, cx)
                    }),
            )
            .child(
                Button::new("termua-footbar-assistant-button")
                    .xsmall()
                    .compact()
                    .ghost()
                    .icon(Icon::default().path(TermuaIcon::Bot))
                    .tooltip(t!("Footbar.Tooltip.Assistant").to_string())
                    .selected(assistant_selected)
                    .debug_selector(|| "termua-footbar-assistant".to_string())
                    .on_click(|_, _, cx| {
                        crate::menu::toggle_assistant_sidebar(&crate::ToggleAssistantSidebar, cx)
                    }),
            )
            .into_any_element()
    }
}

fn truncate_shared(s: &SharedString, max_chars: usize) -> SharedString {
    if max_chars == 0 {
        return "".into();
    }

    let mut it = s.as_ref().chars();
    let head: String = it.by_ref().take(max_chars).collect();
    if it.next().is_some() {
        format!("{head}...").into()
    } else {
        s.clone()
    }
}

impl Render for FootbarView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let enabled = cx.global::<TermuaAppState>().multi_exec_enabled;
        let sessions_visible = cx.global::<TermuaAppState>().sessions_sidebar_visible;
        let right = cx.global::<RightSidebarState>();
        let messages_selected = right.visible && right.active_tab == RightSidebarTab::Notifications;
        let assistant_selected = right.visible && right.active_tab == RightSidebarTab::Assistant;

        let icon_path = Self::multi_exec_icon_path(enabled);
        let lock_state = cx.global::<lock_screen::LockState>();
        let lock_supported = lock_state.locking_supported();
        let lock_enabled = lock_state.locking_enabled();
        let lock_tooltip: SharedString = if !lock_supported {
            t!("Footbar.Tooltip.LockUnsupported").to_string().into()
        } else if !lock_enabled {
            t!("Footbar.Tooltip.LockDisabled").to_string().into()
        } else {
            t!("Footbar.Tooltip.Lock").to_string().into()
        };

        let (border, background) = Self::colors_for_theme(cx.theme());

        let transfers = cx.global::<TransferCenterState>().tasks_sorted();
        self.sync_transfers_popup_state(&transfers);

        let transfers_summary = self.render_transfers_summary(&transfers, cx);

        let left_controls = self.render_controls_left(sessions_visible);
        let right_controls = self.render_controls_right(
            enabled,
            icon_path,
            lock_enabled,
            lock_tooltip,
            messages_selected,
            assistant_selected,
        );

        div()
            .id("termua-footbar")
            .relative()
            .h(px(34.0))
            .px(px(10.0))
            .border_t_1()
            .border_color(border)
            .bg(background)
            .child(
                h_flex()
                    .size_full()
                    .items_center()
                    .justify_between()
                    .child(left_controls)
                    .child(transfers_summary)
                    .child(right_controls),
            )
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use gpui::{AppContext, Context, IntoElement, ParentElement, Render, Styled, Window, div};
    use gpui_component::Colorize;
    use gpui_transfer::{TransferKind, TransferProgress, TransferStatus, TransferTask};

    use super::*;
    use crate::{TermuaAppState, lock_screen};

    #[test]
    fn footbar_multi_exec_icon_paths_match_spec() {
        assert_eq!(FootbarView::multi_exec_icon_path(false), TermuaIcon::Dice1);
        assert_eq!(FootbarView::multi_exec_icon_path(true), TermuaIcon::Dice4);
    }

    #[gpui::test]
    fn footbar_colors_match_titlebar(cx: &mut gpui::TestAppContext) {
        let mut app = cx.app.borrow_mut();
        gpui_component::init(&mut app);

        let theme_set = serde_json::from_str::<gpui_component::ThemeSet>(
            r##"
{
  "name": "Footbar Test",
  "themes": [
    {
      "name": "Footbar Test Dark",
      "mode": "dark",
      "colors": {
        "title_bar.background": "#010203ff",
        "title_bar.border": "#0a0b0cff",
        "popover.background": "#111213ff",
        "border": "#212223ff"
      }
    }
  ]
}
"##,
        )
        .unwrap();

        let dark = theme_set
            .themes
            .into_iter()
            .find(|t| t.mode.is_dark())
            .unwrap();

        gpui_component::Theme::global_mut(&mut app).dark_theme = std::rc::Rc::new(dark);
        gpui_component::Theme::change(gpui_component::ThemeMode::Dark, None, &mut app);

        let t = app.theme();
        let (border, background) = FootbarView::colors_for_theme(t);
        assert_eq!(background.to_hex(), t.title_bar.to_hex());
        assert_eq!(border.to_hex(), t.title_bar_border.opacity(0.7).to_hex());
    }

    #[gpui::test]
    fn footbar_multi_exec_button_toggles_global(cx: &mut gpui::TestAppContext) {
        cx.update(|app| {
            gpui_component::init(app);
            app.activate(true);

            app.set_global(TermuaAppState::default());
            crate::menu::register(app);
        });

        struct Root {
            footbar: gpui::Entity<FootbarView>,
        }

        impl Render for Root {
            fn render(
                &mut self,
                _window: &mut Window,
                _cx: &mut Context<Self>,
            ) -> impl IntoElement {
                div().size_full().child(self.footbar.clone())
            }
        }

        let (root, cx) = cx.add_window_view(|_window, cx| Root {
            footbar: cx.new(FootbarView::new),
        });

        cx.draw(
            gpui::point(gpui::px(0.), gpui::px(0.)),
            gpui::size(
                gpui::AvailableSpace::Definite(gpui::px(800.)),
                gpui::AvailableSpace::Definite(gpui::px(200.)),
            ),
            move |_, _| div().size_full().child(root),
        );
        cx.run_until_parked();

        let bounds = cx
            .debug_bounds("termua-footbar-multi-exec")
            .expect("expected footbar Multi Execute button to exist");
        cx.simulate_click(bounds.center(), gpui::Modifiers::none());

        cx.update(|_, app| {
            assert!(app.global::<TermuaAppState>().multi_exec_enabled);
        });
    }

    #[gpui::test]
    fn footbar_sessions_button_is_left_aligned(cx: &mut gpui::TestAppContext) {
        cx.update(|app| {
            gpui_component::init(app);
            app.activate(true);

            app.set_global(TermuaAppState::default());
        });

        struct Root {
            footbar: gpui::Entity<FootbarView>,
        }

        impl Render for Root {
            fn render(
                &mut self,
                _window: &mut Window,
                _cx: &mut Context<Self>,
            ) -> impl IntoElement {
                div().size_full().child(self.footbar.clone())
            }
        }

        let (root, cx) = cx.add_window_view(|_window, cx| Root {
            footbar: cx.new(FootbarView::new),
        });

        cx.draw(
            gpui::point(gpui::px(0.), gpui::px(0.)),
            gpui::size(
                gpui::AvailableSpace::Definite(gpui::px(800.)),
                gpui::AvailableSpace::Definite(gpui::px(200.)),
            ),
            move |_, _| div().size_full().child(root),
        );
        cx.run_until_parked();

        let sessions = cx
            .debug_bounds("termua-footbar-sessions")
            .expect("expected footbar Sessions button to exist");
        assert!(
            sessions.origin.x < gpui::px(150.),
            "expected Sessions button to be on the left side of the footbar"
        );
    }

    #[gpui::test]
    fn footbar_renders_issues_link_on_right(cx: &mut gpui::TestAppContext) {
        cx.update(|app| {
            gpui_component::init(app);
            app.activate(true);

            app.set_global(TermuaAppState::default());
            app.set_global(lock_screen::LockState::new_for_test(Duration::from_secs(
                60,
            )));
        });

        struct Root {
            footbar: gpui::Entity<FootbarView>,
        }

        impl Render for Root {
            fn render(
                &mut self,
                _window: &mut Window,
                _cx: &mut Context<Self>,
            ) -> impl IntoElement {
                div().size_full().child(self.footbar.clone())
            }
        }

        let (root, cx) = cx.add_window_view(|_window, cx| Root {
            footbar: cx.new(FootbarView::new),
        });

        cx.draw(
            gpui::point(gpui::px(0.), gpui::px(0.)),
            gpui::size(
                gpui::AvailableSpace::Definite(gpui::px(800.)),
                gpui::AvailableSpace::Definite(gpui::px(200.)),
            ),
            move |_, _| div().size_full().child(root),
        );
        cx.run_until_parked();

        let sessions = cx
            .debug_bounds("termua-footbar-sessions")
            .expect("expected footbar Sessions button to exist");
        let issues = cx
            .debug_bounds("termua-footbar-issues")
            .expect("expected footbar Issues link to exist");
        assert!(
            issues.origin.x > sessions.origin.x,
            "expected Issues link to be on the right side of the footbar"
        );
    }

    #[gpui::test]
    fn footbar_lock_button_locks_app(cx: &mut gpui::TestAppContext) {
        cx.update(|app| {
            gpui_component::init(app);
            app.activate(true);

            app.set_global(TermuaAppState::default());
            app.set_global(lock_screen::LockState::new_for_test(Duration::from_secs(
                60,
            )));
        });

        struct Root {
            footbar: gpui::Entity<FootbarView>,
        }

        impl Render for Root {
            fn render(
                &mut self,
                _window: &mut Window,
                _cx: &mut Context<Self>,
            ) -> impl IntoElement {
                div().size_full().child(self.footbar.clone())
            }
        }

        let (root, cx) = cx.add_window_view(|_window, cx| Root {
            footbar: cx.new(FootbarView::new),
        });

        cx.draw(
            gpui::point(gpui::px(0.), gpui::px(0.)),
            gpui::size(
                gpui::AvailableSpace::Definite(gpui::px(800.)),
                gpui::AvailableSpace::Definite(gpui::px(200.)),
            ),
            move |_, _| div().size_full().child(root),
        );
        cx.run_until_parked();

        assert!(
            !cx.update(|_window, app| app.global::<lock_screen::LockState>().locked()),
            "sanity: lock should start unlocked"
        );

        let lock_bounds = cx
            .debug_bounds("termua-footbar-lock")
            .expect("expected footbar Lock button to exist");
        cx.simulate_click(lock_bounds.center(), gpui::Modifiers::none());
        cx.run_until_parked();

        assert!(
            cx.update(|_window, app| app.global::<lock_screen::LockState>().locked()),
            "expected footbar lock button to lock the app"
        );
    }

    #[gpui::test]
    fn footbar_hides_lock_button_when_locking_disabled(cx: &mut gpui::TestAppContext) {
        cx.update(|app| {
            gpui_component::init(app);
            app.activate(true);

            app.set_global(TermuaAppState::default());
            app.set_global(lock_screen::LockState::new_for_test(Duration::from_secs(
                60,
            )));
            app.global_mut::<lock_screen::LockState>()
                .set_user_enabled(false);
        });

        struct Root {
            footbar: gpui::Entity<FootbarView>,
        }

        impl Render for Root {
            fn render(
                &mut self,
                _window: &mut Window,
                _cx: &mut Context<Self>,
            ) -> impl IntoElement {
                div().size_full().child(self.footbar.clone())
            }
        }

        let (root, cx) = cx.add_window_view(|_window, cx| Root {
            footbar: cx.new(FootbarView::new),
        });

        cx.draw(
            gpui::point(gpui::px(0.), gpui::px(0.)),
            gpui::size(
                gpui::AvailableSpace::Definite(gpui::px(800.)),
                gpui::AvailableSpace::Definite(gpui::px(200.)),
            ),
            move |_, _| div().size_full().child(root),
        );
        cx.run_until_parked();

        assert!(
            cx.debug_bounds("termua-footbar-lock").is_none(),
            "expected lock button to be hidden when lock screen is disabled"
        );
    }

    #[gpui::test]
    fn footbar_transfers_popover_opens_and_closes(cx: &mut gpui::TestAppContext) {
        use std::sync::{Arc, Mutex};

        cx.update(|app| {
            gpui_component::init(app);
            app.activate(true);

            app.set_global(TermuaAppState::default());
            app.set_global(lock_screen::LockState::new_for_test(Duration::from_secs(
                60,
            )));

            app.set_global(TransferCenterState::default());
            app.global_mut::<TransferCenterState>().upsert(
                TransferTask::new("test-transfer-1", "test.bin")
                    .with_kind(TransferKind::Upload)
                    .with_status(TransferStatus::InProgress)
                    .with_progress(TransferProgress::Determinate(0.5)),
            );
        });

        struct Root {
            footbar: gpui::Entity<FootbarView>,
        }

        impl Render for Root {
            fn render(
                &mut self,
                _window: &mut Window,
                _cx: &mut Context<Self>,
            ) -> impl IntoElement {
                div().size_full().child(self.footbar.clone())
            }
        }

        let footbar_handle: Arc<Mutex<Option<gpui::Entity<FootbarView>>>> =
            Arc::new(Mutex::new(None));
        let footbar_handle_for_view = footbar_handle.clone();

        let (root, cx) = cx.add_window_view(|_window, cx| {
            let footbar = cx.new(FootbarView::new);
            *footbar_handle_for_view.lock().unwrap() = Some(footbar.clone());
            Root { footbar }
        });
        let footbar = footbar_handle
            .lock()
            .unwrap()
            .clone()
            .expect("expected footbar handle to be captured");

        let root_for_draw = root.clone();
        cx.draw(
            gpui::point(gpui::px(0.), gpui::px(0.)),
            gpui::size(
                gpui::AvailableSpace::Definite(gpui::px(800.)),
                gpui::AvailableSpace::Definite(gpui::px(200.)),
            ),
            move |_, _| div().size_full().child(root_for_draw),
        );
        cx.run_until_parked();

        cx.debug_bounds("termua-footbar-transfers-trigger")
            .expect("expected transfers trigger to exist");

        cx.update(|_window, app| {
            footbar.update(app, |this, cx| this.set_transfers_open(true, cx));
        });
        cx.update(|_, app| {
            assert!(
                footbar.read(app).transfers_open,
                "expected transfers popover state to be open"
            );
        });

        cx.update(|_window, app| {
            footbar.update(app, |this, cx| this.set_transfers_open(false, cx));
        });

        cx.update(|_, app| {
            assert!(
                !footbar.read(app).transfers_open,
                "expected transfers popover state to close"
            );
        });

        cx.draw(
            gpui::point(gpui::px(0.), gpui::px(0.)),
            gpui::size(
                gpui::AvailableSpace::Definite(gpui::px(800.)),
                gpui::AvailableSpace::Definite(gpui::px(200.)),
            ),
            move |_, _| div().size_full().child(root),
        );
        cx.run_until_parked();
    }
}
