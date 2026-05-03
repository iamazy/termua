use gpui::{AppContext, Context, Entity, SharedString, Window};
use gpui_component::{
    input::{InputEvent, InputState},
    tree::TreeState,
};
use rust_i18n::t;

use super::{SessionsSidebarError, SessionsSidebarView, tree};
use crate::store::{delete_session, load_all_sessions};

fn set_input_placeholder(
    input: &Entity<InputState>,
    placeholder: String,
    window: &mut Window,
    cx: &mut Context<SessionsSidebarView>,
) {
    input.update(cx, |state, cx| {
        state.set_placeholder(placeholder, window, cx);
    });
}

impl SessionsSidebarView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        crate::settings::ensure_language_state_with_default(crate::settings::Language::English, cx);

        let search_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("SessionsSidebar.Placeholder.Search").to_string())
        });

        let (sessions, error) = match load_all_sessions() {
            Ok(sessions) => (sessions, None),
            Err(err) => {
                log::error!("SessionsSidebar: failed to load sessions: {err:#}");
                (Vec::new(), Some(SessionsSidebarError::LoadSessions))
            }
        };
        let session_summaries = sessions
            .iter()
            .map(tree::SessionTreeSummary::from_session)
            .collect::<Vec<_>>();
        let tree_items = tree::build_tree_items_from_summaries(&session_summaries, "");
        let tree_state = cx.new(|cx| TreeState::new(cx).items(tree_items.clone()));
        let selected_item_id: SharedString = "".into();

        let mut subscriptions = Vec::new();
        subscriptions.push(cx.observe_global_in::<crate::settings::LanguageSettings>(
            window,
            |this, window, cx| {
                set_input_placeholder(
                    &this.search_input,
                    t!("SessionsSidebar.Placeholder.Search").to_string(),
                    window,
                    cx,
                );
                cx.notify();
                window.refresh();
            },
        ));
        subscriptions.push(cx.subscribe_in(&search_input, window, {
            move |this, input: &Entity<InputState>, ev: &InputEvent, window, cx| {
                if !matches!(ev, InputEvent::Change) {
                    return;
                }
                this.query = input.read(cx).value().to_string();
                this.rebuild_tree(window, cx);
                cx.notify();
            }
        }));

        let mut this = Self {
            focus_handle: cx.focus_handle(),
            search_input,
            query: String::new(),
            error,
            reload_epoch: 0,
            reload_in_flight: false,
            reload_pending: false,
            selected_item_id,
            hovered_session_id: None,
            connecting_session_ids: Default::default(),
            deleting_session_ids: Default::default(),
            tree_items,
            tree_state,
            sessions,
            session_summaries,
            _subscriptions: subscriptions,
        };

        this.sync_tree_selection(cx);
        this
    }

    pub fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.start_reload_sessions_async(window, cx);
    }

    pub fn show_error(
        &mut self,
        message: impl Into<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.error = Some(SessionsSidebarError::Operation(message.into()));
        cx.notify();
        window.refresh();
    }

    pub fn clear_operation_error(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.error, Some(SessionsSidebarError::Operation(_))) {
            return;
        }

        self.error = None;
        cx.notify();
        window.refresh();
    }

    pub(super) fn delete_session_by_id(
        &mut self,
        id: i64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.deleting_session_ids.insert(id) {
            return;
        }
        self.connecting_session_ids.remove(&id);

        // Clear selection if it points at a deleted item.
        if tree::parse_session_id(self.selected_item_id.as_ref()) == Some(id) {
            self.selected_item_id = "".into();
        }
        cx.notify();

        let background = cx.background_executor().clone();
        cx.spawn_in(window, async move |this, window| {
            let result = background.spawn(async move { delete_session(id) }).await;
            let _ = this.update_in(window, |this, window, cx| {
                this.deleting_session_ids.remove(&id);
                if let Err(err) = result {
                    let message = format!("Failed to delete session {id}: {err:#}");
                    log::error!("SessionsSidebar: failed to delete session {id}: {err:#}");
                    this.error = Some(SessionsSidebarError::Operation(message));
                }
                this.start_reload_sessions_async(window, cx);
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn is_connecting(&self, session_id: i64) -> bool {
        self.connecting_session_ids.contains(&session_id)
    }

    pub(crate) fn set_connecting(
        &mut self,
        session_id: i64,
        connecting: bool,
        cx: &mut Context<Self>,
    ) {
        let changed = if connecting {
            self.connecting_session_ids.insert(session_id)
        } else {
            self.connecting_session_ids.remove(&session_id)
        };
        if changed {
            cx.notify();
        }
    }

    pub(crate) fn handle_session_click(
        &mut self,
        item_id: SharedString,
        session_id: i64,
        should_open: bool,
        cx: &mut Context<Self>,
    ) {
        self.selected_item_id = item_id.clone();
        self.hovered_session_id = Some(session_id);
        self.sync_tree_selection(cx);

        if should_open {
            if item_id.as_ref().starts_with("session:ssh:") && self.is_connecting(session_id) {
                // Prevent hammering the same unreachable host and spawning many slow
                // connection attempts.
            } else {
                if item_id.as_ref().starts_with("session:ssh:") {
                    self.set_connecting(session_id, true, cx);
                }
                cx.emit(super::SessionsSidebarEvent::OpenSession(session_id));
            }
        }

        cx.notify();
    }

    pub(crate) fn handle_session_context_click(
        &mut self,
        item_id: SharedString,
        session_id: i64,
        cx: &mut Context<Self>,
    ) {
        self.selected_item_id = item_id;
        self.hovered_session_id = Some(session_id);
        self.sync_tree_selection(cx);
        cx.notify();
    }

    pub(crate) fn handle_background_context_click(&mut self, cx: &mut Context<Self>) {
        self.hovered_session_id = None;
        cx.notify();
    }

    fn rebuild_tree(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let items =
            tree::build_tree_items_from_summaries(&self.session_summaries, self.query.trim());
        self.tree_items = items.clone();
        self.tree_state.update(cx, |tree, cx| {
            tree.set_items(items, cx);
        });
        self.sync_tree_selection(cx);
        cx.notify();
        window.refresh();
    }

    fn start_reload_sessions_async(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.reload_in_flight {
            self.reload_pending = true;
            return;
        }

        self.reload_epoch = self.reload_epoch.saturating_add(1);
        let epoch = self.reload_epoch;
        self.reload_in_flight = true;
        self.reload_pending = false;
        let background = cx.background_executor().clone();

        cx.spawn_in(window, async move |this, window| {
            let result = background.spawn(async move { load_all_sessions() }).await;
            let _ = this.update_in(window, |this, window, cx| {
                if this.reload_epoch != epoch {
                    return;
                }

                this.reload_in_flight = false;
                match result {
                    Ok(sessions) => {
                        if matches!(this.error, Some(SessionsSidebarError::LoadSessions)) {
                            this.error = None;
                        }
                        this.session_summaries = sessions
                            .iter()
                            .map(tree::SessionTreeSummary::from_session)
                            .collect();
                        this.sessions = sessions;
                        this.rebuild_tree(window, cx);
                    }
                    Err(err) => {
                        this.error = Some(SessionsSidebarError::LoadSessions);
                        log::error!("SessionsSidebar: failed to load sessions: {err:#}");
                        cx.notify();
                        window.refresh();
                    }
                }

                if this.reload_pending {
                    this.start_reload_sessions_async(window, cx);
                }
            });
        })
        .detach();
    }

    pub(super) fn sync_tree_selection(&mut self, cx: &mut Context<Self>) {
        if self.selected_item_id.as_ref().is_empty() {
            self.tree_state
                .update(cx, |tree, cx| tree.set_selected_item(None, cx));
            return;
        }

        let Some(item) =
            tree::find_tree_item_by_id(&self.tree_items, self.selected_item_id.as_ref())
        else {
            self.tree_state
                .update(cx, |tree, cx| tree.set_selected_item(None, cx));
            return;
        };

        self.tree_state.update(cx, |tree, cx| {
            tree.set_selected_item(Some(item), cx);
        });
    }

    #[cfg(test)]
    pub(crate) fn selected_item_id_for_test(&self) -> &str {
        self.selected_item_id.as_ref()
    }

    #[cfg(test)]
    pub(crate) fn hovered_session_id_for_test(&self) -> Option<i64> {
        self.hovered_session_id
    }
}
