use std::collections::HashSet;

use gpui::{App, Entity, EventEmitter, FocusHandle, Focusable, SharedString, Subscription};
use gpui_component::{
    input::InputState,
    tree::{TreeItem, TreeState},
};
use gpui_dock::{Panel, PanelEvent, PanelInfo, PanelState};

use super::tree::SessionTreeSummary;
use crate::store::Session;

#[derive(Clone, Debug)]
pub enum SessionsSidebarEvent {
    OpenSession(i64),
}

#[derive(Clone, Debug)]
pub(crate) enum SessionsSidebarError {
    LoadSessions,
    Operation(String),
}

impl SessionsSidebarError {
    pub(super) fn debug_selector(&self) -> &'static str {
        match self {
            Self::LoadSessions => "termua-sessions-sidebar-load-error",
            Self::Operation(_) => "termua-sessions-sidebar-operation-error",
        }
    }
}

pub struct SessionsSidebarView {
    pub(super) focus_handle: FocusHandle,
    pub(super) search_input: Entity<InputState>,
    pub(super) query: String,
    pub(super) error: Option<SessionsSidebarError>,
    pub(super) reload_epoch: usize,
    pub(super) reload_in_flight: bool,
    pub(super) reload_pending: bool,

    pub(super) selected_item_id: SharedString,
    pub(super) hovered_session_id: Option<i64>,
    pub(super) connecting_session_ids: HashSet<i64>,
    pub(super) deleting_session_ids: HashSet<i64>,
    pub(super) tree_items: Vec<TreeItem>,
    pub(super) tree_state: Entity<TreeState>,

    pub(super) sessions: Vec<Session>,
    pub(super) session_summaries: Vec<SessionTreeSummary>,
    pub(super) _subscriptions: Vec<Subscription>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct SessionsSidebarPanelState {
    pub(crate) version: usize,
    pub(crate) query: String,
}

impl EventEmitter<SessionsSidebarEvent> for SessionsSidebarView {}
impl EventEmitter<PanelEvent> for SessionsSidebarView {}

impl Focusable for SessionsSidebarView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for SessionsSidebarView {
    fn panel_name(&self) -> &'static str {
        "termua.sessions_sidebar"
    }

    fn dump(&self, _cx: &App) -> PanelState {
        let mut state = PanelState::new(self);
        state.info = PanelInfo::panel(
            serde_json::to_value(SessionsSidebarPanelState {
                version: 1,
                query: self.query.clone(),
            })
            .expect("sessions sidebar state should serialize"),
        );
        state
    }
}
