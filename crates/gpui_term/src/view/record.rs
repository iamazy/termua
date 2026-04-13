// Recording-specific UI helpers for `TerminalView`.

use gpui::{AnyElement, InteractiveElement, IntoElement, ParentElement, Styled, div, px};
use gpui_component::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecordingMenuEntry {
    Item { checked: bool },
}

pub(crate) fn recording_context_menu_entry(recording_active: bool) -> RecordingMenuEntry {
    RecordingMenuEntry::Item {
        checked: recording_active,
    }
}

pub(crate) fn recording_indicator_label(recording_active: bool) -> Option<&'static str> {
    recording_active.then_some("REC")
}

pub(crate) fn render_recording_indicator_label(theme: &Theme, label: &'static str) -> AnyElement {
    div()
        .id("terminal-recording-indicator")
        .absolute()
        .top(px(12.0))
        .right(px(12.0))
        .flex()
        .items_center()
        .gap(px(6.0))
        .px(px(8.0))
        .py(px(4.0))
        .bg(theme.background.opacity(0.65))
        .border_1()
        .border_color(theme.danger.opacity(0.8))
        .rounded_full()
        .child(div().w(px(6.0)).h(px(6.0)).rounded_full().bg(theme.danger))
        .child(div().text_xs().text_color(theme.danger).child(label))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::{RecordingMenuEntry, recording_context_menu_entry, recording_indicator_label};

    #[test]
    fn recording_indicator_label_is_some_when_recording() {
        assert_eq!(recording_indicator_label(true), Some("REC"));
        assert_eq!(recording_indicator_label(false), None);
    }

    #[test]
    fn recording_context_menu_entry_is_a_single_togglable_item() {
        assert_eq!(
            recording_context_menu_entry(false),
            RecordingMenuEntry::Item { checked: false }
        );

        assert_eq!(
            recording_context_menu_entry(true),
            RecordingMenuEntry::Item { checked: true }
        );
    }
}
