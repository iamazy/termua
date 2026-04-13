use std::{fs::File, sync::Arc};

use alacritty_terminal::{
    event::{OnResize, WindowSize},
    tty::{ChildEvent, EventedPty, EventedReadWrite, Pty},
};
use parking_lot::Mutex;

use crate::cast::CastRecorderSender;

pub(crate) struct RecordingRead {
    file: File,
    cast_slot: Arc<Mutex<Option<CastRecorderSender>>>,
}

impl std::io::Read for RecordingRead {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        let n = std::io::Read::read(&mut self.file, out)?;
        if n != 0
            && let Some(sender) = self.cast_slot.lock().as_ref()
        {
            sender.output(&out[..n]);
        }
        Ok(n)
    }
}

pub(crate) struct RecordingWrite {
    file: File,
    cast_slot: Arc<Mutex<Option<CastRecorderSender>>>,
}

impl std::io::Write for RecordingWrite {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = std::io::Write::write(&mut self.file, buf)?;
        if n != 0
            && let Some(sender) = self.cast_slot.lock().as_ref()
        {
            sender.input(&buf[..n]);
        }
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        std::io::Write::flush(&mut self.file)
    }
}

/// Wrap alacritty's local PTY on Unix so we can record PTY bytes without modifying the upstream
/// PTY type.
pub(crate) struct RecordingLocalPty {
    inner: Pty,
    reader: RecordingRead,
    writer: RecordingWrite,
}

impl RecordingLocalPty {
    pub(crate) fn new(
        mut inner: Pty,
        cast_slot: Arc<Mutex<Option<CastRecorderSender>>>,
    ) -> std::io::Result<Self> {
        let reader_file = inner.reader().try_clone()?;
        let writer_file = inner.writer().try_clone()?;

        Ok(Self {
            inner,
            reader: RecordingRead {
                file: reader_file,
                cast_slot: Arc::clone(&cast_slot),
            },
            writer: RecordingWrite {
                file: writer_file,
                cast_slot,
            },
        })
    }
}

impl EventedReadWrite for RecordingLocalPty {
    type Reader = RecordingRead;
    type Writer = RecordingWrite;

    unsafe fn register(
        &mut self,
        poller: &Arc<polling::Poller>,
        interest: polling::Event,
        mode: polling::PollMode,
    ) -> std::io::Result<()> {
        // Safety: delegated to upstream PTY.
        unsafe { self.inner.register(poller, interest, mode) }
    }

    fn reregister(
        &mut self,
        poller: &Arc<polling::Poller>,
        interest: polling::Event,
        mode: polling::PollMode,
    ) -> std::io::Result<()> {
        self.inner.reregister(poller, interest, mode)
    }

    fn deregister(&mut self, poller: &Arc<polling::Poller>) -> std::io::Result<()> {
        self.inner.deregister(poller)
    }

    fn reader(&mut self) -> &mut Self::Reader {
        &mut self.reader
    }

    fn writer(&mut self) -> &mut Self::Writer {
        &mut self.writer
    }
}

impl EventedPty for RecordingLocalPty {
    fn next_child_event(&mut self) -> Option<ChildEvent> {
        self.inner.next_child_event()
    }
}

impl OnResize for RecordingLocalPty {
    fn on_resize(&mut self, window_size: WindowSize) {
        self.inner.on_resize(window_size);
    }
}
