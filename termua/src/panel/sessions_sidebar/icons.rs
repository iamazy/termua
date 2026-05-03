use std::collections::BTreeMap;

use gpui::{AnyElement, InteractiveElement, IntoElement, ParentElement, Styled, div, px};
use gpui_common::TermuaIcon;
use gpui_component::Icon;

use crate::store::{Session, SessionType};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SessionIconKind {
    Terminal,
}

impl SessionIconKind {
    fn icon_path(self) -> TermuaIcon {
        TermuaIcon::Terminal
    }

    pub(super) fn into_element_for_session_id(self, session_id: i64) -> AnyElement {
        let _ = self;
        div()
            .w(px(16.))
            .h(px(16.))
            .flex_shrink_0()
            .debug_selector(move || format!("termua-sessions-session-icon-local-{session_id}"))
            .child(Icon::default().path(self.icon_path()).size_4())
            .into_any_element()
    }
}

pub(super) fn build_session_icon_kinds(sessions: &[Session]) -> BTreeMap<i64, SessionIconKind> {
    let mut out = BTreeMap::new();
    for session in sessions {
        if session.protocol != SessionType::Local {
            continue;
        }

        out.insert(session.id, SessionIconKind::Terminal);
    }
    out
}
