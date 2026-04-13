use std::{
    any::Any,
    borrow::Cow,
    ops::RangeInclusive,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use gpui::{
    Keystroke, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels,
    ScrollWheelEvent, Window, px,
};

use crate::{
    CursorShape, GridPoint, TerminalBackend, TerminalBounds, TerminalContent,
    TerminalShutdownPolicy,
    remote::{
        RemoteFrame, RemoteGridPoint, RemoteInputEvent, RemoteSelectionRange,
        RemoteSelectionUpdate, RemoteSnapshot, RemoteTerminalBounds, RemoteTerminalContent,
    },
    terminal::Event,
};

pub struct RemoteBackend {
    content: TerminalContent,
    total_lines: usize,
    viewport_lines: usize,
    scroll_px: Pixels,
    controlled: Arc<AtomicBool>,
    on_input: Arc<dyn Send + Sync + Fn(RemoteInputEvent)>,
    selection_anchor: Option<GridPoint>,
    mirrored_bounds: Option<RemoteTerminalBounds>,
    viewport_line_numbers: Vec<Option<usize>>,
}

impl RemoteBackend {
    pub fn new(
        controlled: Arc<AtomicBool>,
        on_input: Arc<dyn Send + Sync + Fn(RemoteInputEvent)>,
    ) -> Self {
        Self {
            content: TerminalContent::default(),
            total_lines: 0,
            viewport_lines: 0,
            scroll_px: px(0.0),
            controlled,
            on_input,
            selection_anchor: None,
            mirrored_bounds: None,
            viewport_line_numbers: Vec::new(),
        }
    }

    fn is_controlled(&self) -> bool {
        self.controlled.load(Ordering::Relaxed)
    }

    fn grid_point_for_position(&self, position: gpui::Point<Pixels>) -> GridPoint {
        let bounds = self.content.terminal_bounds.bounds;
        let pos = position - bounds.origin;
        let cur_size = self.content.terminal_bounds;

        let last_col = cur_size.last_column();
        let mut col = (pos.x / cur_size.cell_width) as usize;
        if col > last_col {
            col = last_col;
        }

        let bottom_line = cur_size.num_lines().saturating_sub(1) as i32;
        let mut line = (pos.y / cur_size.line_height) as i32;
        if line > bottom_line {
            line = bottom_line;
        }
        let line = line - self.content.display_offset as i32;

        GridPoint::new(line, col)
    }

    fn send_selection_range(&self, start: GridPoint, end: GridPoint) {
        (self.on_input)(RemoteInputEvent::SetSelectionRange {
            range: Some(RemoteSelectionRange {
                start: RemoteGridPoint::from(start),
                end: RemoteGridPoint::from(end),
            }),
        });
    }
}

#[derive(Debug)]
pub enum RemoteBackendEvent {
    ApplySnapshot(RemoteSnapshot),
    ApplyFrame(RemoteFrame),
    ApplySelectionUpdate(RemoteSelectionUpdate),
    SetControlled(bool),
}

impl TerminalBackend for RemoteBackend {
    fn backend_name(&self) -> &'static str {
        "remote"
    }

    fn is_remote_mirror(&self) -> bool {
        true
    }

    fn handle_backend_event(
        &mut self,
        event: Box<dyn Any + Send>,
        cx: &mut gpui::Context<crate::Terminal>,
    ) {
        let Ok(ev) = event.downcast::<RemoteBackendEvent>() else {
            return;
        };
        match *ev {
            RemoteBackendEvent::ApplySnapshot(snapshot) => {
                let RemoteTerminalContent {
                    total_lines,
                    viewport_lines,
                    ref terminal_bounds,
                    ref viewport_line_numbers,
                    ..
                } = snapshot.content;
                self.mirrored_bounds = Some(terminal_bounds.clone());
                self.viewport_line_numbers = viewport_line_numbers.clone();
                snapshot.content.apply_to(&mut self.content);
                self.total_lines = total_lines;
                self.viewport_lines = viewport_lines;
                cx.emit(Event::Wakeup);
            }
            RemoteBackendEvent::ApplyFrame(frame) => {
                let RemoteTerminalContent {
                    total_lines,
                    viewport_lines,
                    ref terminal_bounds,
                    ref viewport_line_numbers,
                    ..
                } = frame.content;
                self.mirrored_bounds = Some(terminal_bounds.clone());
                self.viewport_line_numbers = viewport_line_numbers.clone();
                frame.content.apply_to(&mut self.content);
                self.total_lines = total_lines;
                self.viewport_lines = viewport_lines;
                cx.emit(Event::Wakeup);
            }
            RemoteBackendEvent::ApplySelectionUpdate(update) => {
                update.apply_to(&mut self.content);
                cx.emit(Event::SelectionsChanged);
            }
            RemoteBackendEvent::SetControlled(value) => {
                self.controlled.store(value, Ordering::Relaxed);
                cx.emit(Event::Wakeup);
            }
        }
    }

    fn sync(&mut self, _window: &mut Window, _cx: &mut gpui::Context<crate::Terminal>) {}

    fn shutdown(
        &mut self,
        _policy: TerminalShutdownPolicy,
        _cx: &mut gpui::Context<crate::Terminal>,
    ) {
    }

    fn last_content(&self) -> &TerminalContent {
        &self.content
    }

    fn matches(&self) -> &[RangeInclusive<GridPoint>] {
        &[]
    }

    fn last_clicked_line(&self) -> Option<i32> {
        None
    }

    fn vi_mode_enabled(&self) -> bool {
        false
    }

    fn mouse_mode(&self, _shift: bool) -> bool {
        false
    }

    fn selection_started(&self) -> bool {
        false
    }

    fn set_cursor_shape(&mut self, _cursor_shape: CursorShape) {}

    fn total_lines(&self) -> usize {
        self.total_lines
    }

    fn viewport_lines(&self) -> usize {
        self.viewport_lines
    }

    fn logical_line_numbers_from_top(&self, start_line: usize, count: usize) -> Vec<Option<usize>> {
        if count == 0 {
            return Vec::new();
        }

        let viewport_top = self
            .total_lines
            .saturating_sub(self.viewport_lines.max(1))
            .saturating_sub(self.content.display_offset);
        if start_line < viewport_top {
            return (start_line..start_line.saturating_add(count))
                .map(|i| Some(i.saturating_add(1)))
                .collect();
        }

        let local_start = start_line.saturating_sub(viewport_top);
        if local_start >= self.viewport_line_numbers.len() {
            return Vec::new();
        }

        let local_end = local_start
            .saturating_add(count)
            .min(self.viewport_line_numbers.len());
        self.viewport_line_numbers[local_start..local_end].to_vec()
    }

    fn activate_match(&mut self, _index: usize) {}
    fn select_matches(&mut self, _matches: &[RangeInclusive<GridPoint>]) {}
    fn select_all(&mut self) {}

    fn copy(&mut self, _keep_selection: Option<bool>, cx: &mut gpui::Context<crate::Terminal>) {
        if !self.is_controlled() {
            return;
        }
        if let Some(text) = self.content.selection_text.clone() {
            crate::terminal::write_clipboard(cx, text);
        }
    }

    fn clear(&mut self) {}

    fn scroll_line_up(&mut self) {
        self.scroll_up_by(1);
    }

    fn scroll_up_by(&mut self, lines: usize) {
        if !self.is_controlled() {
            return;
        }
        let delta = i32::try_from(lines.min(i32::MAX as usize)).unwrap_or(i32::MAX);
        (self.on_input)(RemoteInputEvent::ScrollLines { delta });
    }

    fn scroll_line_down(&mut self) {
        self.scroll_down_by(1);
    }

    fn scroll_down_by(&mut self, lines: usize) {
        if !self.is_controlled() {
            return;
        }
        let delta = i32::try_from(lines.min(i32::MAX as usize)).unwrap_or(i32::MAX);
        (self.on_input)(RemoteInputEvent::ScrollLines { delta: -delta });
    }

    fn scroll_page_up(&mut self) {
        self.scroll_up_by(self.viewport_lines.max(1));
    }

    fn scroll_page_down(&mut self) {
        self.scroll_down_by(self.viewport_lines.max(1));
    }

    fn scroll_to_top(&mut self) {
        if self.is_controlled() {
            (self.on_input)(RemoteInputEvent::ScrollToTop);
        }
    }

    fn scroll_to_bottom(&mut self) {
        if self.is_controlled() {
            (self.on_input)(RemoteInputEvent::ScrollToBottom);
        }
    }

    fn scrolled_to_top(&self) -> bool {
        self.content.scrolled_to_top
    }

    fn scrolled_to_bottom(&self) -> bool {
        self.content.scrolled_to_bottom
    }

    fn set_size(&mut self, new_bounds: TerminalBounds) {
        if let Some(bounds) = &self.mirrored_bounds {
            bounds.apply_to(&mut self.content, new_bounds.bounds.origin);
        } else {
            self.content.terminal_bounds = new_bounds;
        }
    }

    fn input(&mut self, input: Cow<'static, [u8]>) {
        if !self.is_controlled() {
            return;
        }
        if input.is_empty() {
            return;
        }

        let text = match input {
            Cow::Borrowed(bytes) => String::from_utf8_lossy(bytes).to_string(),
            Cow::Owned(bytes) => String::from_utf8(bytes)
                .unwrap_or_else(|err| String::from_utf8_lossy(err.as_bytes()).to_string()),
        };
        if text.is_empty() {
            return;
        }
        (self.on_input)(RemoteInputEvent::Text { text });
    }

    fn paste(&mut self, text: &str) {
        if self.is_controlled() {
            (self.on_input)(RemoteInputEvent::Paste {
                text: text.to_string(),
            });
        }
    }

    fn focus_in(&self) {}
    fn focus_out(&mut self) {}
    fn toggle_vi_mode(&mut self) {}

    fn try_keystroke(&mut self, keystroke: &Keystroke, _alt_is_meta: bool) -> bool {
        if !self.is_controlled() {
            return false;
        }
        (self.on_input)(RemoteInputEvent::Keystroke {
            keystroke: keystroke_for_remote(keystroke),
        });
        true
    }

    fn try_modifiers_change(
        &mut self,
        _modifiers: &Modifiers,
        _window: &Window,
        _cx: &mut gpui::Context<crate::Terminal>,
    ) {
    }

    fn mouse_move(&mut self, _e: &MouseMoveEvent, _cx: &mut gpui::Context<crate::Terminal>) {}
    fn select_word_at_event_position(&mut self, _e: &MouseDownEvent) {}

    fn mouse_down(&mut self, e: &MouseDownEvent, _cx: &mut gpui::Context<crate::Terminal>) {
        if !self.is_controlled() {
            return;
        }
        if e.button != MouseButton::Left {
            return;
        }
        let p = self.grid_point_for_position(e.position);
        self.selection_anchor = Some(p);
        self.send_selection_range(p, p);
    }

    fn mouse_up(&mut self, e: &MouseUpEvent, _cx: &gpui::Context<crate::Terminal>) {
        if e.button == MouseButton::Left {
            self.selection_anchor = None;
        }
    }

    fn mouse_drag(
        &mut self,
        e: &MouseMoveEvent,
        _region: gpui::Bounds<gpui::Pixels>,
        _cx: &mut gpui::Context<crate::Terminal>,
    ) {
        if !self.is_controlled() {
            return;
        }
        let Some(anchor) = self.selection_anchor else {
            return;
        };
        let end = self.grid_point_for_position(e.position);
        self.send_selection_range(anchor, end);
    }

    fn scroll_wheel(&mut self, e: &ScrollWheelEvent) {
        if !self.is_controlled() {
            return;
        }
        let Some(scroll_lines) = super::determine_scroll_lines(
            &mut self.scroll_px,
            e,
            self.content.terminal_bounds.line_height,
            false,
            self.content.terminal_bounds.height(),
        ) else {
            return;
        };
        if scroll_lines != 0 {
            (self.on_input)(RemoteInputEvent::ScrollLines {
                delta: scroll_lines,
            });
        }
    }

    fn get_content(&self) -> String {
        self.content
            .cells
            .iter()
            .map(|c| c.cell.c)
            .collect::<String>()
    }

    fn last_n_non_empty_lines(&self, _n: usize) -> Vec<String> {
        Vec::new()
    }
}

fn keystroke_for_remote(keystroke: &Keystroke) -> String {
    // `Keystroke::to_string()` is for *display* (and uppercases single-char keys),
    // while the host side uses `Keystroke::parse()` which expects the unparse format.
    keystroke.unparse()
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use gpui::{Bounds, Modifiers, point, size};

    use super::*;

    #[test]
    fn keystroke_for_remote_does_not_force_shift_for_letters() {
        let k = Keystroke {
            modifiers: Modifiers::none(),
            key: "a".to_string(),
            key_char: Some("a".to_string()),
        };
        assert_eq!(keystroke_for_remote(&k), "a");
    }

    #[test]
    fn keystroke_for_remote_preserves_shift_modifier() {
        let k = Keystroke {
            modifiers: Modifiers {
                shift: true,
                ..Modifiers::none()
            },
            key: "a".to_string(),
            key_char: Some("A".to_string()),
        };
        assert_eq!(keystroke_for_remote(&k), "shift-a");
    }

    #[test]
    fn input_forwards_text_when_controlled() {
        let captured: Arc<Mutex<Vec<RemoteInputEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let captured_for_cb = Arc::clone(&captured);
        let controlled = Arc::new(AtomicBool::new(true));
        let on_input: Arc<dyn Send + Sync + Fn(RemoteInputEvent)> = Arc::new(move |ev| {
            captured_for_cb.lock().unwrap().push(ev);
        });

        let mut backend = RemoteBackend::new(controlled, on_input);
        backend.input(Cow::Owned(b"A".to_vec()));

        assert_eq!(
            captured.lock().unwrap().as_slice(),
            &[RemoteInputEvent::Text {
                text: "A".to_string()
            }]
        );
    }

    #[test]
    fn set_size_preserves_remote_host_grid_when_known() {
        use crate::TerminalBackend as _;

        let controlled = Arc::new(AtomicBool::new(false));
        let on_input: Arc<dyn Send + Sync + Fn(RemoteInputEvent)> = Arc::new(move |_ev| {});
        let mut backend = RemoteBackend::new(controlled, on_input);

        backend.mirrored_bounds =
            Some(crate::remote::RemoteTerminalBounds::new(8.0, 12.0, 120, 40));

        backend.set_size(TerminalBounds::new(
            px(20.0),
            px(10.0),
            Bounds {
                origin: point(px(9.0), px(13.0)),
                size: size(px(300.0), px(200.0)),
            },
        ));

        assert_eq!(
            backend.content.terminal_bounds,
            TerminalBounds::new(
                px(12.0),
                px(8.0),
                Bounds {
                    origin: point(px(9.0), px(13.0)),
                    size: size(px(960.0), px(480.0)),
                },
            )
        );
    }

    #[test]
    fn logical_line_numbers_follow_remote_viewport_line_numbers() {
        use crate::TerminalBackend as _;

        let controlled = Arc::new(AtomicBool::new(false));
        let on_input: Arc<dyn Send + Sync + Fn(RemoteInputEvent)> = Arc::new(move |_ev| {});
        let mut backend = RemoteBackend::new(controlled, on_input);

        backend.total_lines = 6;
        backend.viewport_lines = 3;
        backend.content.display_offset = 0;
        backend.viewport_line_numbers = vec![Some(4), None, Some(5)];

        assert_eq!(
            backend.logical_line_numbers_from_top(3, 3),
            vec![Some(4), None, Some(5)]
        );
    }
}
