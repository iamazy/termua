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
