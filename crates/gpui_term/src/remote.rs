use serde::{Deserialize, Serialize};

use crate::{
    Cell, CellFlags, Cursor, CursorRenderShape, GridPoint, IndexedCell, NamedColor, SelectionRange,
    TermColor, TerminalContent, TerminalMode,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteGridPoint {
    pub line: i32,
    pub column: usize,
}

impl From<GridPoint> for RemoteGridPoint {
    fn from(p: GridPoint) -> Self {
        Self {
            line: p.line,
            column: p.column,
        }
    }
}

impl From<RemoteGridPoint> for GridPoint {
    fn from(p: RemoteGridPoint) -> Self {
        GridPoint::new(p.line, p.column)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteSelectionRange {
    pub start: RemoteGridPoint,
    pub end: RemoteGridPoint,
}

impl From<SelectionRange> for RemoteSelectionRange {
    fn from(r: SelectionRange) -> Self {
        Self {
            start: r.start.into(),
            end: r.end.into(),
        }
    }
}

impl From<RemoteSelectionRange> for SelectionRange {
    fn from(r: RemoteSelectionRange) -> Self {
        SelectionRange {
            start: r.start.into(),
            end: r.end.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteCursorRenderShape {
    Hidden,
    Block,
    Underline,
    Bar,
    Hollow,
}

impl From<CursorRenderShape> for RemoteCursorRenderShape {
    fn from(s: CursorRenderShape) -> Self {
        match s {
            CursorRenderShape::Hidden => Self::Hidden,
            CursorRenderShape::Block => Self::Block,
            CursorRenderShape::Underline => Self::Underline,
            CursorRenderShape::Bar => Self::Bar,
            CursorRenderShape::Hollow => Self::Hollow,
        }
    }
}

impl From<RemoteCursorRenderShape> for CursorRenderShape {
    fn from(s: RemoteCursorRenderShape) -> Self {
        match s {
            RemoteCursorRenderShape::Hidden => Self::Hidden,
            RemoteCursorRenderShape::Block => Self::Block,
            RemoteCursorRenderShape::Underline => Self::Underline,
            RemoteCursorRenderShape::Bar => Self::Bar,
            RemoteCursorRenderShape::Hollow => Self::Hollow,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteCursor {
    pub shape: RemoteCursorRenderShape,
    pub point: RemoteGridPoint,
}

impl From<Cursor> for RemoteCursor {
    fn from(c: Cursor) -> Self {
        Self {
            shape: c.shape.into(),
            point: c.point.into(),
        }
    }
}

impl From<RemoteCursor> for Cursor {
    fn from(c: RemoteCursor) -> Self {
        Cursor {
            shape: c.shape.into(),
            point: c.point.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RemoteTermColor {
    Named { name: NamedColor },
    Indexed { index: u8 },
    Rgb { r: u8, g: u8, b: u8 },
}

impl From<TermColor> for RemoteTermColor {
    fn from(c: TermColor) -> Self {
        match c {
            TermColor::Named(name) => Self::Named { name },
            TermColor::Indexed(index) => Self::Indexed { index },
            TermColor::Rgb(r, g, b) => Self::Rgb { r, g, b },
        }
    }
}

impl From<RemoteTermColor> for TermColor {
    fn from(c: RemoteTermColor) -> Self {
        match c {
            RemoteTermColor::Named { name } => TermColor::Named(name),
            RemoteTermColor::Indexed { index } => TermColor::Indexed(index),
            RemoteTermColor::Rgb { r, g, b } => TermColor::Rgb(r, g, b),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteCell {
    pub c: char,
    pub fg: RemoteTermColor,
    pub bg: RemoteTermColor,
    pub flags: u32,
}

impl From<&Cell> for RemoteCell {
    fn from(cell: &Cell) -> Self {
        Self {
            c: cell.c,
            fg: cell.fg.into(),
            bg: cell.bg.into(),
            flags: cell.flags.bits(),
        }
    }
}

impl From<RemoteCell> for Cell {
    fn from(cell: RemoteCell) -> Self {
        Cell {
            c: cell.c,
            fg: cell.fg.into(),
            bg: cell.bg.into(),
            flags: CellFlags::from_bits_truncate(cell.flags),
            hyperlink: None,
            zerowidth: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteIndexedCell {
    pub point: RemoteGridPoint,
    pub cell: RemoteCell,
}

impl From<&IndexedCell> for RemoteIndexedCell {
    fn from(cell: &IndexedCell) -> Self {
        Self {
            point: cell.point.into(),
            cell: RemoteCell::from(&cell.cell),
        }
    }
}

impl From<RemoteIndexedCell> for IndexedCell {
    fn from(c: RemoteIndexedCell) -> Self {
        IndexedCell {
            point: c.point.into(),
            cell: c.cell.into(),
        }
    }
}

/// Minimal, backend-neutral content payload suitable for remote mirroring.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemoteTerminalBounds {
    pub cell_width: f32,
    pub line_height: f32,
    pub columns: usize,
    pub lines: usize,
}

impl RemoteTerminalBounds {
    pub fn new(cell_width: f32, line_height: f32, columns: usize, lines: usize) -> Self {
        Self {
            cell_width,
            line_height,
            columns,
            lines,
        }
    }

    pub fn from_local(bounds: crate::terminal::TerminalBounds) -> Self {
        Self::new(
            f32::from(bounds.cell_width),
            f32::from(bounds.line_height),
            bounds.num_columns(),
            bounds.num_lines(),
        )
    }

    pub fn apply_to(&self, content: &mut TerminalContent, origin: gpui::Point<gpui::Pixels>) {
        let cell_width = gpui::px(self.cell_width);
        let line_height = gpui::px(self.line_height);
        content.terminal_bounds = crate::terminal::TerminalBounds::new(
            line_height,
            cell_width,
            gpui::Bounds {
                origin,
                size: gpui::size(
                    cell_width * self.columns as f32,
                    line_height * self.lines as f32,
                ),
            },
        );
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemoteTerminalContent {
    pub cells: Vec<RemoteIndexedCell>,
    pub mode_bits: u32,
    pub terminal_bounds: RemoteTerminalBounds,
    pub viewport_line_numbers: Vec<Option<usize>>,
    pub display_offset: usize,
    pub selection_text: Option<String>,
    pub selection: Option<RemoteSelectionRange>,
    pub cursor: RemoteCursor,
    pub cursor_char: char,
    pub scrolled_to_top: bool,
    pub scrolled_to_bottom: bool,
    pub total_lines: usize,
    pub viewport_lines: usize,
}

impl RemoteTerminalContent {
    pub fn from_local(
        content: &TerminalContent,
        total_lines: usize,
        viewport_lines: usize,
        viewport_line_numbers: Vec<Option<usize>>,
    ) -> Self {
        Self {
            cells: content.cells.iter().map(RemoteIndexedCell::from).collect(),
            mode_bits: content.mode.bits(),
            terminal_bounds: RemoteTerminalBounds::from_local(content.terminal_bounds),
            viewport_line_numbers,
            display_offset: content.display_offset,
            selection_text: content.selection_text.clone(),
            selection: content.selection.clone().map(RemoteSelectionRange::from),
            cursor: content.cursor.into(),
            cursor_char: content.cursor_char,
            scrolled_to_top: content.scrolled_to_top,
            scrolled_to_bottom: content.scrolled_to_bottom,
            total_lines,
            viewport_lines,
        }
    }

    pub fn apply_to(&self, content: &mut TerminalContent) {
        let origin = content.terminal_bounds.bounds.origin;
        self.terminal_bounds.apply_to(content, origin);
        content.cells = self.cells.iter().cloned().map(IndexedCell::from).collect();
        content.mode = TerminalMode::from_bits_truncate(self.mode_bits);
        content.display_offset = self.display_offset;
        content.selection_text = self.selection_text.clone();
        content.selection = self.selection.clone().map(SelectionRange::from);
        content.cursor = self.cursor.into();
        content.cursor_char = self.cursor_char;
        content.scrolled_to_top = self.scrolled_to_top;
        content.scrolled_to_bottom = self.scrolled_to_bottom;
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemoteSnapshot {
    pub seq: u64,
    pub content: RemoteTerminalContent,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemoteFrame {
    pub seq: u64,
    pub content: RemoteTerminalContent,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemoteSelectionUpdate {
    pub selection_text: Option<String>,
    pub selection: Option<RemoteSelectionRange>,
}

impl RemoteSelectionUpdate {
    pub fn from_local(content: &TerminalContent) -> Self {
        Self {
            selection_text: content.selection_text.clone(),
            selection: content.selection.clone().map(RemoteSelectionRange::from),
        }
    }

    pub fn apply_to(&self, content: &mut TerminalContent) {
        content.selection_text = self.selection_text.clone();
        content.selection = self.selection.clone().map(SelectionRange::from);
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RemoteInputEvent {
    Keystroke {
        keystroke: String,
    },
    Paste {
        text: String,
    },
    Text {
        text: String,
    },
    /// Positive deltas scroll "up" (towards older scrollback).
    ScrollLines {
        delta: i32,
    },
    ScrollToTop,
    ScrollToBottom,
    SetSelectionRange {
        range: Option<RemoteSelectionRange>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_content_apply_roundtrips_minimal_fields() {
        let mut local = TerminalContent {
            display_offset: 3,
            cursor_char: 'X',
            scrolled_to_top: true,
            scrolled_to_bottom: false,
            selection: Some(SelectionRange {
                start: GridPoint::new(-2, 1),
                end: GridPoint::new(0, 5),
            }),
            ..TerminalContent::default()
        };
        local.terminal_bounds = crate::terminal::TerminalBounds::new(
            gpui::px(11.0),
            gpui::px(7.0),
            gpui::Bounds {
                origin: gpui::point(gpui::px(3.0), gpui::px(5.0)),
                size: gpui::size(gpui::px(70.0), gpui::px(44.0)),
            },
        );
        local.cells.push(IndexedCell {
            point: GridPoint::new(0, 0),
            cell: Cell {
                c: 'a',
                fg: TermColor::Named(NamedColor::Foreground),
                bg: TermColor::Named(NamedColor::Background),
                flags: CellFlags::BOLD,
                hyperlink: None,
                zerowidth: Vec::new(),
            },
        });

        let remote =
            RemoteTerminalContent::from_local(&local, 10, 5, vec![Some(7), None, Some(8), None]);
        let mut applied = TerminalContent::default();
        remote.apply_to(&mut applied);

        assert_eq!(applied.display_offset, 3);
        assert_eq!(applied.cursor_char, 'X');
        assert!(applied.scrolled_to_top);
        assert!(!applied.scrolled_to_bottom);
        assert_eq!(applied.selection, local.selection);
        assert_eq!(applied.cells.len(), 1);
        assert_eq!(applied.cells[0].point, GridPoint::new(0, 0));
        assert_eq!(applied.cells[0].cell.c, 'a');
        assert!(applied.cells[0].cell.flags.contains(CellFlags::BOLD));
        assert_eq!(
            applied.terminal_bounds.cell_width,
            local.terminal_bounds.cell_width
        );
        assert_eq!(
            applied.terminal_bounds.line_height,
            local.terminal_bounds.line_height
        );
        assert_eq!(
            applied.terminal_bounds.bounds.size,
            local.terminal_bounds.bounds.size
        );
        assert_eq!(
            applied.terminal_bounds.num_columns(),
            local.terminal_bounds.num_columns()
        );
        assert_eq!(
            applied.terminal_bounds.num_lines(),
            local.terminal_bounds.num_lines()
        );
    }
}
