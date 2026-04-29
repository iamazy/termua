use std::collections::BTreeMap;

use gpui::{
    AnyElement, InteractiveElement, IntoElement, ParentElement, Styled, StyledImage, div, img, px,
};
use gpui_common::TermuaIcon;
use gpui_component::Icon;

use crate::store::{Session, SessionType};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SessionIconKind {
    Terminal,
    Pwsh,
}

impl SessionIconKind {
    fn icon_path(self) -> TermuaIcon {
        match self {
            Self::Terminal => TermuaIcon::Terminal,
            Self::Pwsh => TermuaIcon::Pwsh,
        }
    }

    pub(super) fn into_element_for_session_id(self, session_id: i64) -> AnyElement {
        match self {
            Self::Terminal => div()
                .w(px(16.))
                .h(px(16.))
                .flex_shrink_0()
                .debug_selector(move || match self {
                    Self::Terminal => format!("termua-sessions-session-icon-local-{session_id}"),
                    Self::Pwsh => unreachable!(),
                })
                .child(Icon::default().path(self.icon_path()).size_4())
                .into_any_element(),
            Self::Pwsh => img(TermuaIcon::Pwsh)
                .w(px(16.))
                .h(px(16.))
                .flex_shrink_0()
                .object_fit(gpui::ObjectFit::Contain)
                .debug_selector(move || format!("termua-sessions-session-icon-pwsh-{session_id}"))
                .into_any_element(),
        }
    }
}

pub(super) fn build_session_icon_kinds(sessions: &[Session]) -> BTreeMap<i64, SessionIconKind> {
    let mut out = BTreeMap::new();
    for session in sessions {
        if session.protocol != SessionType::Local {
            continue;
        }

        let kind = session
            .shell_program
            .as_deref()
            .and_then(shell_program_basename)
            .map(|program| program.to_ascii_lowercase())
            .as_deref()
            .map(strip_exe_suffix)
            .map(|program| match program {
                "pwsh" | "powershell" => SessionIconKind::Pwsh,
                _ => SessionIconKind::Terminal,
            })
            .unwrap();

        out.insert(session.id, kind);
    }
    out
}

fn shell_program_basename(program: &str) -> Option<&str> {
    let trimmed = program.trim();
    if trimmed.is_empty() {
        return None;
    }

    let last_slash = trimmed.rfind('/');
    let last_backslash = trimmed.rfind('\\');
    let start = match (last_slash, last_backslash) {
        (Some(a), Some(b)) => a.max(b) + 1,
        (Some(a), None) => a + 1,
        (None, Some(b)) => b + 1,
        (None, None) => 0,
    };

    Some(&trimmed[start..])
}

fn strip_exe_suffix(program: &str) -> &str {
    program.strip_suffix(".exe").unwrap_or(program)
}
