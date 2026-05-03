use gpui::{Context, KeyDownEvent, ReadGlobal, Window};

use super::TerminalView;
use crate::{
    settings::{CursorShape, TerminalSettings},
    terminal::{Event, UserInput},
};

impl TerminalView {
    pub(super) fn forward_keystroke_to_terminal(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) {
        let (handled, vi_mode_enabled) = self.terminal.update(cx, |term, cx| {
            let handled = term.try_keystroke(
                &event.keystroke,
                TerminalSettings::global(cx).option_as_meta,
            );
            (handled, term.vi_mode_enabled())
        });

        if handled {
            cx.stop_propagation();
            // In terminal vi-mode, keystrokes are usually for scrollback/navigation, so don't
            // force the view back to the bottom.
            if !vi_mode_enabled {
                self.snap_to_bottom_on_input(cx);
            }
            cx.emit(Event::UserInput(UserInput::Keystroke(
                event.keystroke.clone(),
            )));
        }
    }

    pub(super) fn focus_in(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.terminal.update(cx, |terminal, _| {
            terminal.set_cursor_shape(self.cursor_shape);
            terminal.focus_in();
        });
        self.blink_cursors(self.blink.epoch, cx);
        window.invalidate_character_coordinates();
        cx.notify();
    }

    pub(super) fn focus_out(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.terminal.update(cx, |terminal, _| {
            terminal.focus_out();
            terminal.set_cursor_shape(CursorShape::Hollow);
        });
        self.suggestions.close();
        cx.notify();
    }
}
