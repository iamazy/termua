use std::{
    collections::BTreeMap,
    io::{self, Write},
    path::PathBuf,
    sync::{Arc, mpsc},
    thread,
    time::{Instant, SystemTime},
};

use gpui::Global;
use serde::Serialize;

use crate::{Cell, CellFlags, NamedColor, TermColor};

/// Asciinema cast v2 header.
#[derive(Debug, Clone)]
pub struct CastHeader {
    pub width: usize,
    pub height: usize,
    pub timestamp: u64,
    pub env: BTreeMap<String, String>,
}

/// Writes an asciinema `.cast` (v2) stream.
pub struct CastWriter<W: Write> {
    w: W,
}

impl<W: Write> CastWriter<W> {
    pub fn new(mut w: W, header: CastHeader) -> io::Result<Self> {
        #[derive(Serialize)]
        struct Header<'a> {
            version: u8,
            width: usize,
            height: usize,
            timestamp: u64,
            #[serde(skip_serializing_if = "BTreeMap::is_empty")]
            env: &'a BTreeMap<String, String>,
        }

        let header = Header {
            version: 2,
            width: header.width,
            height: header.height,
            timestamp: header.timestamp,
            env: &header.env,
        };

        serde_json::to_writer(&mut w, &header).map_err(io::Error::other)?;
        w.write_all(b"\n")?;
        Ok(Self { w })
    }

    fn write_event<T: Serialize>(
        &mut self,
        t: f64,
        kind: &'static str,
        payload: T,
    ) -> io::Result<()> {
        serde_json::to_writer(&mut self.w, &(t, kind, payload)).map_err(io::Error::other)?;
        self.w.write_all(b"\n")?;
        Ok(())
    }

    fn write_text_event(&mut self, t: f64, kind: &'static str, bytes: &[u8]) -> io::Result<()> {
        let text = String::from_utf8_lossy(bytes);
        self.write_event(t, kind, text.as_ref())
    }

    pub fn write_output(&mut self, t: f64, bytes: &[u8]) -> io::Result<()> {
        self.write_text_event(t, "o", bytes)
    }

    pub fn write_input(&mut self, t: f64, bytes: &[u8]) -> io::Result<()> {
        self.write_text_event(t, "i", bytes)
    }

    pub fn write_resize(&mut self, t: f64, cols: usize, rows: usize) -> io::Result<()> {
        self.write_event(t, "r", format!("{cols}x{rows}"))
    }
}

#[derive(Clone, Copy, PartialEq)]
struct CellStyle {
    fg: TermColor,
    bg: TermColor,
    flags: CellFlags,
}

impl From<&Cell> for CellStyle {
    fn from(cell: &Cell) -> Self {
        Self {
            fg: cell.fg,
            bg: cell.bg,
            flags: cell.flags,
        }
    }
}

pub(crate) fn serialize_pre_cursor_cells(cells: &[Cell], cursor_style: &Cell) -> Vec<u8> {
    let mut output = Vec::new();
    let mut active_style = None;
    for cell in cells {
        if cell.flags.contains(CellFlags::WIDE_CHAR_SPACER) {
            continue;
        }

        let style = CellStyle::from(cell);
        if active_style != Some(style) {
            write_style(&mut output, style);
            active_style = Some(style);
        }
        push_char(&mut output, cell.c);
        if let Some(zerowidth) = cell.zerowidth() {
            for character in zerowidth {
                push_char(&mut output, *character);
            }
        }
    }

    let cursor_style = CellStyle::from(cursor_style);
    if active_style != Some(cursor_style) {
        write_style(&mut output, cursor_style);
    }
    output
}

#[derive(Clone)]
pub struct TerminalScreen {
    columns: usize,
    rows: usize,
    cells: Vec<Cell>,
    line_numbers: Vec<Option<usize>>,
    cursor: crate::Cursor,
    cursor_row: i32,
}

impl TerminalScreen {
    pub fn columns(&self) -> usize {
        self.columns
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

    pub fn line_numbers(&self) -> &[Option<usize>] {
        &self.line_numbers
    }
}

pub fn capture_terminal_screen(content: &crate::TerminalContent) -> TerminalScreen {
    let columns = content.terminal_bounds.num_columns().max(1);
    let rows = content.terminal_bounds.num_lines().max(1);
    let blank = Cell::default();
    let mut cells = vec![blank; columns.saturating_mul(rows)];
    for cell in &content.cells {
        let row = cell.point.line + content.display_offset as i32;
        if row >= 0 && (row as usize) < rows && cell.point.column < columns {
            cells[row as usize * columns + cell.point.column] = cell.cell.clone();
        }
    }

    TerminalScreen {
        columns,
        rows,
        cells,
        line_numbers: vec![None; rows],
        cursor: content.cursor,
        cursor_row: content.cursor.point.line + content.display_offset as i32,
    }
}

pub fn capture_terminal_screen_with_line_numbers(
    terminal: &crate::Terminal,
    show_line_numbers: bool,
) -> TerminalScreen {
    let content = terminal.last_content();
    let mut screen = capture_terminal_screen(content);
    if !crate::view::line_number::should_show_line_numbers(show_line_numbers, content.mode) {
        return screen;
    }

    if let Some(data) = crate::view::line_number::compute_line_number_paint_data(
        terminal,
        content.display_offset,
        screen.rows,
    ) {
        for row in 0..=data.last_row_to_number.min(screen.rows.saturating_sub(1)) {
            screen.line_numbers[row] = data.line_numbers.get(row).copied().flatten();
        }
    }
    screen
}

pub fn serialize_terminal_content_ansi(content: &crate::TerminalContent) -> Vec<u8> {
    serialize_terminal_screen_ansi(&capture_terminal_screen(content))
}

pub fn serialize_terminal_screen_ansi(screen: &TerminalScreen) -> Vec<u8> {
    let mut output = b"\x1b[2J\x1b[H".to_vec();
    for row in 0..screen.rows {
        output.extend_from_slice(format!("\x1b[{};1H", row + 1).as_bytes());
        serialize_screen_row(&mut output, &screen.cells, screen.columns, row);
    }
    serialize_screen_cursor(&mut output, screen);
    output
}

pub fn serialize_terminal_screen_update_ansi(
    previous: &TerminalScreen,
    current: &TerminalScreen,
) -> Vec<u8> {
    if previous.columns != current.columns || previous.rows != current.rows {
        return serialize_terminal_screen_ansi(current);
    }

    let mut output = Vec::new();
    for row in 0..current.rows {
        let start = row * current.columns;
        let end = start + current.columns;
        if previous.cells[start..end]
            .iter()
            .zip(&current.cells[start..end])
            .all(|(previous, current)| cells_equal(previous, current))
        {
            continue;
        }
        output.extend_from_slice(format!("\x1b[{};1H", row + 1).as_bytes());
        serialize_screen_row(&mut output, &current.cells, current.columns, row);
    }
    serialize_screen_cursor(&mut output, current);
    output
}

fn serialize_screen_row(output: &mut Vec<u8>, cells: &[Cell], columns: usize, row: usize) {
    let mut active_style = None;
    for cell in &cells[row * columns..(row + 1) * columns] {
        if cell.flags.contains(CellFlags::WIDE_CHAR_SPACER) {
            continue;
        }
        let style = CellStyle::from(cell);
        if active_style != Some(style) {
            write_style(output, style);
            active_style = Some(style);
        }
        push_char(output, cell.c);
        if let Some(zerowidth) = cell.zerowidth() {
            for character in zerowidth {
                push_char(output, *character);
            }
        }
    }
}

fn serialize_screen_cursor(output: &mut Vec<u8>, screen: &TerminalScreen) {
    if screen.cursor.shape == crate::CursorRenderShape::Hidden {
        output.extend_from_slice(b"\x1b[?25l");
    } else {
        output.extend_from_slice(b"\x1b[?25h");
    }
    output.extend_from_slice(
        format!(
            "\x1b[{};{}H",
            screen.cursor_row + 1,
            screen.cursor.point.column + 1
        )
        .as_bytes(),
    );
}

fn cells_equal(left: &Cell, right: &Cell) -> bool {
    left.c == right.c
        && left.fg == right.fg
        && left.bg == right.bg
        && left.flags == right.flags
        && left.hyperlink == right.hyperlink
        && left.zerowidth == right.zerowidth
}

pub fn serialize_terminal_rows_ansi(
    columns: usize,
    rows: usize,
    cells: &[crate::IndexedCell],
) -> Vec<u8> {
    let mut output = Vec::new();
    let mut active_style = None;
    let blank = Cell::default();
    let mut indexed = vec![None; columns.saturating_mul(rows)];
    for cell in cells {
        if cell.point.line >= 0 && (cell.point.line as usize) < rows && cell.point.column < columns
        {
            indexed[cell.point.line as usize * columns + cell.point.column] = Some(&cell.cell);
        }
    }
    for row in 0..rows {
        for column in 0..columns {
            let cell = indexed[row * columns + column].unwrap_or(&blank);
            if cell.flags.contains(CellFlags::WIDE_CHAR_SPACER) {
                continue;
            }
            let style = CellStyle::from(cell);
            if active_style != Some(style) {
                write_style(&mut output, style);
                active_style = Some(style);
            }
            push_char(&mut output, cell.c);
            if let Some(zerowidth) = cell.zerowidth() {
                for character in zerowidth {
                    push_char(&mut output, *character);
                }
            }
        }
        output.extend_from_slice(b"\r\n");
    }
    output
}

fn push_char(output: &mut Vec<u8>, character: char) {
    let mut buf = [0; 4];
    output.extend_from_slice(character.encode_utf8(&mut buf).as_bytes());
}

fn write_style(output: &mut Vec<u8>, style: CellStyle) {
    let mut params = vec!["0".to_string()];
    if style.flags.contains(CellFlags::BOLD) {
        params.push("1".to_string());
    }
    if style.flags.contains(CellFlags::DIM) {
        params.push("2".to_string());
    }
    if style.flags.contains(CellFlags::ITALIC) {
        params.push("3".to_string());
    }
    if style.flags.contains(CellFlags::DOUBLE_UNDERLINE) {
        params.push("4:2".to_string());
    } else if style.flags.contains(CellFlags::CURLY_UNDERLINE) {
        params.push("4:3".to_string());
    } else if style.flags.contains(CellFlags::DOTTED_UNDERLINE) {
        params.push("4:4".to_string());
    } else if style.flags.contains(CellFlags::DASHED_UNDERLINE) {
        params.push("4:5".to_string());
    } else if style.flags.contains(CellFlags::UNDERLINE) {
        params.push("4".to_string());
    }
    if style.flags.contains(CellFlags::INVERSE) {
        params.push("7".to_string());
    }
    if style.flags.contains(CellFlags::STRIKEOUT) {
        params.push("9".to_string());
    }
    push_color(&mut params, style.fg, false);
    push_color(&mut params, style.bg, true);

    output.extend_from_slice(b"\x1b[");
    output.extend_from_slice(params.join(";").as_bytes());
    output.push(b'm');
}

fn push_color(params: &mut Vec<String>, color: TermColor, background: bool) {
    let base = if background { 48 } else { 38 };
    match color {
        TermColor::Rgb(r, g, b) => params.push(format!("{base};2;{r};{g};{b}")),
        TermColor::Indexed(index) => params.push(format!("{base};5;{index}")),
        TermColor::Named(named) => params.push(named_color_code(named, background).to_string()),
    }
}

fn named_color_code(color: NamedColor, background: bool) -> u8 {
    let offset = u8::from(background) * 10;
    match color {
        NamedColor::Black => 30 + offset,
        NamedColor::Red => 31 + offset,
        NamedColor::Green => 32 + offset,
        NamedColor::Yellow => 33 + offset,
        NamedColor::Blue => 34 + offset,
        NamedColor::Magenta => 35 + offset,
        NamedColor::Cyan => 36 + offset,
        NamedColor::White => 37 + offset,
        NamedColor::BrightBlack => 90 + offset,
        NamedColor::BrightRed => 91 + offset,
        NamedColor::BrightGreen => 92 + offset,
        NamedColor::BrightYellow => 93 + offset,
        NamedColor::BrightBlue => 94 + offset,
        NamedColor::BrightMagenta => 95 + offset,
        NamedColor::BrightCyan => 96 + offset,
        NamedColor::BrightWhite => 97 + offset,
        NamedColor::Foreground | NamedColor::Background | NamedColor::Cursor => 39 + offset,
    }
}

#[derive(Clone, Debug)]
pub struct CastRecordingOptions {
    pub path: PathBuf,
    pub include_input: bool,
}

#[derive(Default)]
pub struct CastRecordingConfig {
    pub include_input_by_default: bool,
    pub default_dir: Option<PathBuf>,
    pub request_path: Option<Arc<dyn Send + Sync + Fn() -> Option<PathBuf>>>,
}

impl Global for CastRecordingConfig {}

pub fn default_cast_path(config: &CastRecordingConfig) -> PathBuf {
    if let Some(provider) = config.request_path.as_ref()
        && let Some(path) = provider()
    {
        return path;
    }

    let base = config
        .default_dir
        .clone()
        .or_else(|| {
            home::home_dir().map(|home| {
                let dl = home.join("Downloads");
                if dl.exists() { dl } else { home }
            })
        })
        .unwrap_or_else(|| PathBuf::from("/tmp"));

    let dir = base.join("termua-casts");
    let ts = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let pid = std::process::id();
    dir.join(format!("termua-{ts}-{pid}.cast"))
}

#[derive(Clone, Debug)]
pub(crate) struct CastRecorderSender {
    tx: mpsc::Sender<CastMsg>,
    start: Instant,
    include_input: bool,
}

impl CastRecorderSender {
    pub(crate) fn output(&self, bytes: &[u8]) {
        let t = self.start.elapsed().as_secs_f64();
        let _ = self.tx.send(CastMsg::Output {
            t,
            bytes: bytes.to_vec(),
        });
    }

    pub(crate) fn input(&self, bytes: &[u8]) {
        if !self.include_input {
            return;
        }
        let t = self.start.elapsed().as_secs_f64();
        let _ = self.tx.send(CastMsg::Input {
            t,
            bytes: bytes.to_vec(),
        });
    }

    pub(crate) fn resize(&self, cols: usize, rows: usize) {
        let t = self.start.elapsed().as_secs_f64();
        let _ = self.tx.send(CastMsg::Resize { t, cols, rows });
    }
}

enum CastMsg {
    Output { t: f64, bytes: Vec<u8> },
    Input { t: f64, bytes: Vec<u8> },
    Resize { t: f64, cols: usize, rows: usize },
    Stop,
}

pub(crate) struct CastRecorderState {
    tx: mpsc::Sender<CastMsg>,
    join: thread::JoinHandle<io::Result<()>>,
}

impl CastRecorderState {
    pub(crate) fn stop_and_join(self) -> io::Result<()> {
        let _ = self.tx.send(CastMsg::Stop);
        match self.join.join() {
            Ok(r) => r,
            Err(_) => Err(io::Error::other("cast recorder thread panicked")),
        }
    }
}

pub(crate) fn start_cast_recorder(
    path: PathBuf,
    header: CastHeader,
    include_input: bool,
) -> io::Result<(CastRecorderSender, CastRecorderState)> {
    let file = std::fs::File::create(path)?;
    let writer = std::io::BufWriter::new(file);
    let (tx, rx) = mpsc::channel::<CastMsg>();

    let join = thread::spawn(move || {
        let mut w = CastWriter::new(writer, header)?;
        while let Ok(msg) = rx.recv() {
            match msg {
                CastMsg::Output { t, bytes } => w.write_output(t, &bytes)?,
                CastMsg::Input { t, bytes } => w.write_input(t, &bytes)?,
                CastMsg::Resize { t, cols, rows } => w.write_resize(t, cols, rows)?,
                CastMsg::Stop => break,
            }
        }
        Ok(())
    });

    Ok((
        CastRecorderSender {
            tx: tx.clone(),
            start: Instant::now(),
            include_input,
        },
        CastRecorderState { tx, join },
    ))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    struct CastEventSink<'a, W: Write> {
        include_input: bool,
        writer: &'a mut CastWriter<W>,
    }

    impl<'a, W: Write> CastEventSink<'a, W> {
        fn new(writer: &'a mut CastWriter<W>, include_input: bool) -> Self {
            Self {
                include_input,
                writer,
            }
        }

        fn output(&mut self, t: f64, bytes: &[u8]) -> io::Result<()> {
            self.writer.write_output(t, bytes)
        }

        fn input(&mut self, t: f64, bytes: &[u8]) -> io::Result<()> {
            if !self.include_input {
                return Ok(());
            }
            self.writer.write_input(t, bytes)
        }

        fn resize(&mut self, t: f64, cols: usize, rows: usize) -> io::Result<()> {
            self.writer.write_resize(t, cols, rows)
        }
    }

    fn styled_cell(c: char, fg: TermColor, bg: TermColor, flags: CellFlags) -> Cell {
        Cell {
            c,
            fg,
            bg,
            flags,
            ..Cell::default()
        }
    }

    #[test]
    fn pre_cursor_serialization_preserves_text_and_skips_wide_spacers() {
        let mut combined = Cell {
            c: 'e',
            zerowidth: vec!['\u{301}'],
            ..Cell::default()
        };
        combined.fg = TermColor::Indexed(7);
        let spacer = Cell {
            c: 'x',
            flags: CellFlags::WIDE_CHAR_SPACER,
            ..Cell::default()
        };
        let cursor = Cell::default();

        assert_eq!(
            serialize_pre_cursor_cells(&[combined, spacer], &cursor),
            b"\x1b[0;38;5;7;49me\xcc\x81\x1b[0;39;49m"
        );
    }

    #[test]
    fn pre_cursor_serialization_emits_all_text_styles_and_rgb_colors() {
        let flags = CellFlags::BOLD
            | CellFlags::DIM
            | CellFlags::ITALIC
            | CellFlags::DOUBLE_UNDERLINE
            | CellFlags::INVERSE
            | CellFlags::STRIKEOUT;
        let cell = styled_cell('x', TermColor::Rgb(1, 2, 3), TermColor::Rgb(4, 5, 6), flags);

        assert_eq!(
            serialize_pre_cursor_cells(std::slice::from_ref(&cell), &cell),
            b"\x1b[0;1;2;3;4:2;7;9;38;2;1;2;3;48;2;4;5;6mx"
        );
    }

    #[test]
    fn pre_cursor_serialization_emits_each_underline_variant() {
        let variants = [
            (CellFlags::CURLY_UNDERLINE, "4:3"),
            (CellFlags::DOTTED_UNDERLINE, "4:4"),
            (CellFlags::DASHED_UNDERLINE, "4:5"),
            (CellFlags::UNDERLINE, "4"),
        ];

        for (flags, code) in variants {
            let cell = styled_cell(
                'x',
                TermColor::Named(NamedColor::Foreground),
                TermColor::Named(NamedColor::Background),
                flags,
            );
            assert_eq!(
                String::from_utf8(serialize_pre_cursor_cells(
                    std::slice::from_ref(&cell),
                    &cell
                ))
                .unwrap(),
                format!("\x1b[0;{code};39;49mx")
            );
        }
    }

    #[test]
    fn named_color_codes_cover_normal_bright_and_default_colors() {
        let colors = [
            NamedColor::Black,
            NamedColor::Red,
            NamedColor::Green,
            NamedColor::Yellow,
            NamedColor::Blue,
            NamedColor::Magenta,
            NamedColor::Cyan,
            NamedColor::White,
            NamedColor::BrightBlack,
            NamedColor::BrightRed,
            NamedColor::BrightGreen,
            NamedColor::BrightYellow,
            NamedColor::BrightBlue,
            NamedColor::BrightMagenta,
            NamedColor::BrightCyan,
            NamedColor::BrightWhite,
            NamedColor::Foreground,
            NamedColor::Background,
            NamedColor::Cursor,
        ];
        let foreground = [
            30, 31, 32, 33, 34, 35, 36, 37, 90, 91, 92, 93, 94, 95, 96, 97, 39, 39, 39,
        ];

        for (index, color) in colors.into_iter().enumerate() {
            assert_eq!(named_color_code(color, false), foreground[index]);
            assert_eq!(named_color_code(color, true), foreground[index] + 10);
        }
    }

    #[test]
    fn cast_writer_emits_header_then_events() {
        let mut buf = Vec::<u8>::new();
        let mut w = CastWriter::new(
            &mut buf,
            CastHeader {
                width: 80,
                height: 24,
                timestamp: 1,
                env: BTreeMap::new(),
            },
        )
        .unwrap();

        w.write_output(0.0, b"hi\r\n").unwrap();

        let s = String::from_utf8(buf).unwrap();
        let mut lines = s.lines();
        let header = lines.next().unwrap();
        assert!(header.contains("\"version\":2"));
        assert!(header.contains("\"width\":80"));
        assert!(header.contains("\"height\":24"));

        let ev = lines.next().unwrap();
        assert!(ev.starts_with("["));
        assert!(ev.contains("\"o\""));
        assert!(ev.contains("hi"));
    }

    #[test]
    fn cast_writer_resize_formats_cols_x_rows() {
        let mut buf = Vec::<u8>::new();
        let mut w = CastWriter::new(
            &mut buf,
            CastHeader {
                width: 1,
                height: 1,
                timestamp: 1,
                env: BTreeMap::new(),
            },
        )
        .unwrap();

        w.write_resize(0.1, 120, 40).unwrap();

        let s = String::from_utf8(buf).unwrap();
        assert!(s.lines().nth(1).unwrap().contains("\"120x40\""));
    }

    #[test]
    fn event_sink_can_disable_input_events() {
        let mut buf = Vec::<u8>::new();
        let mut w = CastWriter::new(
            &mut buf,
            CastHeader {
                width: 80,
                height: 24,
                timestamp: 1,
                env: BTreeMap::new(),
            },
        )
        .unwrap();

        let mut sink = CastEventSink::new(&mut w, false);
        sink.input(0.0, b"secret\r").unwrap();
        sink.resize(0.0, 120, 40).unwrap();
        sink.output(0.0, b"ok\r\n").unwrap();

        let s = String::from_utf8(buf).unwrap();
        assert!(!s.contains("\"i\""));
        assert!(s.contains("\"o\""));
    }

    #[test]
    fn cast_recorder_writes_output_and_filters_input() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!(
            "termua-test-{}-{}.cast",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));

        let (sender, state) = start_cast_recorder(
            path.clone(),
            CastHeader {
                width: 80,
                height: 24,
                timestamp: 1,
                env: BTreeMap::new(),
            },
            false,
        )
        .unwrap();

        sender.input(b"secret\r");
        sender.resize(120, 40);
        sender.output(b"ok\r\n");

        state.stop_and_join().unwrap();

        let bytes = fs::read(&path).unwrap();
        let s = String::from_utf8(bytes).unwrap();
        let _ = fs::remove_file(&path);

        assert!(!s.contains("\"i\""));
        assert!(s.contains("\"o\""));
    }

    #[test]
    fn default_cast_path_prefers_provider() {
        let p = PathBuf::from("/tmp/provider.cast");
        let cfg = CastRecordingConfig {
            include_input_by_default: false,
            default_dir: Some(PathBuf::from("/tmp/ignored")),
            request_path: Some(Arc::new({
                let p = p.clone();
                move || Some(p.clone())
            })),
        };

        assert_eq!(default_cast_path(&cfg), p);
    }
}
