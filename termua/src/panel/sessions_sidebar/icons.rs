use std::collections::BTreeMap;

use gpui::{AnyElement, InteractiveElement, IntoElement, ParentElement, Styled, div, px};
use gpui_common::TermuaIcon;
use gpui_component::Icon;

use crate::store::{Session, SessionType};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SessionIconKind {
    Terminal,
    Fish,
    Nushell,
    Pwsh,
}

impl SessionIconKind {
    fn icon_path(self) -> TermuaIcon {
        match self {
            Self::Terminal => TermuaIcon::Terminal,
            Self::Fish => TermuaIcon::Fish,
            Self::Nushell => TermuaIcon::Nushell,
            Self::Pwsh => TermuaIcon::Pwsh,
        }
    }

    pub(super) fn into_element_for_session_id(self, session_id: i64) -> AnyElement {
        div()
            .w(px(16.))
            .h(px(16.))
            .flex_shrink_0()
            .debug_selector(move || format!("termua-sessions-session-icon-local-{session_id}"))
            .child(Icon::default().path(self.icon_path()).size_4())
            .into_any_element()
    }
}

fn icon_kind_for_shell_program(program: Option<&str>) -> SessionIconKind {
    match crate::panel::terminal_panel::shell_icon_for_program(program) {
        TermuaIcon::Fish => SessionIconKind::Fish,
        TermuaIcon::Nushell => SessionIconKind::Nushell,
        TermuaIcon::Pwsh => SessionIconKind::Pwsh,
        _ => SessionIconKind::Terminal,
    }
}

pub(super) fn build_session_icon_kinds(sessions: &[Session]) -> BTreeMap<i64, SessionIconKind> {
    let mut out = BTreeMap::new();
    for session in sessions {
        if session.protocol != SessionType::Local {
            continue;
        }

        let shell_program = session.env.as_deref().and_then(|env| {
            env.iter()
                .find(|var| var.name == gpui_term::shell::TERMUA_SHELL_ENV_KEY)
                .or_else(|| {
                    env.iter()
                        .find(|var| var.name == gpui_term::shell::SHELL_ENV_KEY)
                })
                .map(|var| var.value.as_str())
        });
        out.insert(session.id, icon_kind_for_shell_program(shell_program));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_programs_map_to_session_sidebar_icons() {
        for (program, expected) in [
            ("bash", SessionIconKind::Terminal),
            ("zsh", SessionIconKind::Terminal),
            ("fish", SessionIconKind::Fish),
            ("nu", SessionIconKind::Nushell),
            ("pwsh", SessionIconKind::Pwsh),
            ("powershell", SessionIconKind::Pwsh),
            ("cmd", SessionIconKind::Terminal),
        ] {
            assert_eq!(icon_kind_for_shell_program(Some(program)), expected);
        }
    }
}
