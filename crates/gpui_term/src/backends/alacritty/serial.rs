#[cfg(unix)]
use std::os::fd::{AsFd, AsRawFd};
#[cfg(windows)]
use std::os::windows::io::{AsRawSocket, AsSocket};
use std::{
    ffi::OsStr,
    io::{Read, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use alacritty_terminal::{
    event::{OnResize, WindowSize},
    tty::{ChildEvent, EventedPty, EventedReadWrite},
};
use filedescriptor::FileDescriptor;
use parking_lot::Mutex;
use polling::{Event, PollMode, Poller};
use serial2::SerialPort;

use crate::{
    SerialOptions, cast::CastRecorderSender, serial::apply_serial_options_to_serial2_settings,
};

// Keep token aligned with SSH PTY implementation so we don't accidentally
// collide with any reserved internal token usage.
#[cfg(unix)]
const PTY_READ_WRITE_TOKEN: usize = 0;
#[cfg(windows)]
const PTY_READ_WRITE_TOKEN: usize = 2;

#[derive(Debug)]
pub struct PtyReader {
    fd: FileDescriptor,
    cast_slot: Arc<Mutex<Option<CastRecorderSender>>>,
}

impl PtyReader {
    fn set_non_blocking(&mut self, enabled: bool) -> std::io::Result<()> {
        self.fd
            .set_non_blocking(enabled)
            .map_err(|e| std::io::Error::other(format!("{e:#}")))
    }
}

impl Read for PtyReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.fd.read(buf)?;
        if n != 0
            && let Some(sender) = self.cast_slot.lock().as_ref()
        {
            sender.output(&buf[..n]);
        }
        Ok(n)
    }
}

#[cfg(unix)]
impl AsRawFd for PtyReader {
    fn as_raw_fd(&self) -> i32 {
        self.fd.as_raw_fd()
    }
}

#[cfg(unix)]
impl AsFd for PtyReader {
    fn as_fd(&self) -> std::os::fd::BorrowedFd<'_> {
        self.fd.as_fd()
    }
}

#[cfg(windows)]
impl AsRawSocket for PtyReader {
    fn as_raw_socket(&self) -> std::os::windows::io::RawSocket {
        self.fd.as_raw_socket()
    }
}

#[cfg(windows)]
impl AsSocket for PtyReader {
    fn as_socket(&self) -> std::os::windows::io::BorrowedSocket<'_> {
        self.fd.as_socket()
    }
}

#[derive(Debug)]
pub struct PtyWriter {
    fd: FileDescriptor,
    cast_slot: Arc<Mutex<Option<CastRecorderSender>>>,
}

impl PtyWriter {
    fn set_non_blocking(&mut self, enabled: bool) -> std::io::Result<()> {
        self.fd
            .set_non_blocking(enabled)
            .map_err(|e| std::io::Error::other(format!("{e:#}")))
    }
}

impl Write for PtyWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.fd.write(buf)?;
        if n != 0
            && let Some(sender) = self.cast_slot.lock().as_ref()
        {
            sender.input(&buf[..n]);
        }
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.fd.flush()
    }
}

#[cfg(unix)]
impl AsRawFd for PtyWriter {
    fn as_raw_fd(&self) -> i32 {
        self.fd.as_raw_fd()
    }
}

#[cfg(unix)]
impl AsFd for PtyWriter {
    fn as_fd(&self) -> std::os::fd::BorrowedFd<'_> {
        self.fd.as_fd()
    }
}

#[cfg(windows)]
impl AsRawSocket for PtyWriter {
    fn as_raw_socket(&self) -> std::os::windows::io::RawSocket {
        self.fd.as_raw_socket()
    }
}

#[cfg(windows)]
impl AsSocket for PtyWriter {
    fn as_socket(&self) -> std::os::windows::io::BorrowedSocket<'_> {
        self.fd.as_socket()
    }
}

#[derive(Debug)]
pub struct Pty {
    io_reader: PtyReader,
    io_writer: PtyWriter,
    stop: Arc<AtomicBool>,
}

impl Drop for Pty {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl EventedPty for Pty {
    fn next_child_event(&mut self) -> Option<ChildEvent> {
        None
    }
}

impl EventedReadWrite for Pty {
    type Reader = PtyReader;
    type Writer = PtyWriter;

    unsafe fn register(
        &mut self,
        poller: &Arc<Poller>,
        mut interest: Event,
        mode: PollMode,
    ) -> std::io::Result<()> {
        interest.key = PTY_READ_WRITE_TOKEN;
        let _ = self.io_reader.set_non_blocking(true);
        let _ = self.io_writer.set_non_blocking(true);

        #[cfg(unix)]
        unsafe {
            poller.add_with_mode(self.io_reader.as_raw_fd(), interest, mode)?;
            poller.add_with_mode(self.io_writer.as_raw_fd(), interest, mode)?;
        }

        #[cfg(windows)]
        unsafe {
            poller.add_with_mode(self.io_reader.as_raw_socket(), interest, mode)?;
            poller.add_with_mode(self.io_writer.as_raw_socket(), interest, mode)?;
        }

        Ok(())
    }

    fn reregister(
        &mut self,
        poller: &Arc<Poller>,
        mut interest: Event,
        mode: PollMode,
    ) -> std::io::Result<()> {
        interest.key = PTY_READ_WRITE_TOKEN;

        #[cfg(unix)]
        {
            poller.modify_with_mode(self.io_reader.as_fd(), interest, mode)?;
            poller.modify_with_mode(self.io_writer.as_fd(), interest, mode)?;
        }

        #[cfg(windows)]
        {
            poller.modify_with_mode(self.io_reader.as_socket(), interest, mode)?;
            poller.modify_with_mode(self.io_writer.as_socket(), interest, mode)?;
        }

        Ok(())
    }

    fn deregister(&mut self, poller: &Arc<Poller>) -> std::io::Result<()> {
        #[cfg(unix)]
        {
            poller.delete(self.io_reader.as_fd())?;
            poller.delete(self.io_writer.as_fd())?;
        }

        #[cfg(windows)]
        {
            poller.delete(self.io_reader.as_socket())?;
            poller.delete(self.io_writer.as_socket())?;
        }

        Ok(())
    }

    fn reader(&mut self) -> &mut Self::Reader {
        &mut self.io_reader
    }

    fn writer(&mut self) -> &mut Self::Writer {
        &mut self.io_writer
    }
}

impl OnResize for Pty {
    fn on_resize(&mut self, _window_size: WindowSize) {
        // Serial ports have no concept of size.
    }
}

impl Pty {
    pub fn new(
        opts: SerialOptions,
        cast_slot: Arc<Mutex<Option<CastRecorderSender>>>,
    ) -> anyhow::Result<Self> {
        log::debug!(
            "gpui_term: opening serial (alacritty backend): port={} baud={} data_bits={} \
             parity={:?} stop_bits={:?} flow_control={:?}",
            opts.port,
            opts.baud,
            opts.data_bits,
            opts.parity,
            opts.stop_bits,
            opts.flow_control
        );

        let mut port = SerialPort::open(OsStr::new(&opts.port), opts.baud)?;
        let mut settings = port.get_configuration()?;
        apply_serial_options_to_serial2_settings(&mut settings, &opts)?;
        port.set_configuration(&settings)?;

        // Keep timeouts short: on Windows, long reads can block concurrent writes.
        port.set_read_timeout(Duration::from_millis(50))?;
        port.set_write_timeout(Duration::from_millis(50))?;

        let (mut a, mut b) = filedescriptor::socketpair()?;
        a.set_non_blocking(true)?;
        b.set_non_blocking(true)?;

        let io_reader = PtyReader {
            fd: a.try_clone()?,
            cast_slot: Arc::clone(&cast_slot),
        };
        let io_writer = PtyWriter { fd: a, cast_slot };

        let stop = Arc::new(AtomicBool::new(false));
        let stop_for_thread = Arc::clone(&stop);

        thread::spawn(move || {
            let mut socket = b;
            let serial = port;
            let mut socket_buf = vec![0u8; 8192];
            let mut serial_buf = vec![0u8; 8192];

            loop {
                if stop_for_thread.load(Ordering::Relaxed) {
                    break;
                }

                // Drain socket -> serial.
                loop {
                    match socket.read(&mut socket_buf) {
                        Ok(0) => return, // peer closed
                        Ok(n) => {
                            let bytes = &socket_buf[..n];
                            if let Err(err) = serial.write_all(bytes) {
                                log::error!("serial write error: {err}");
                                return;
                            }
                        }
                        Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => break,
                        Err(err) => {
                            log::error!("serial socket read error: {err}");
                            return;
                        }
                    }
                }

                // One serial read per loop; `read_timeout` bounds the sleep here.
                match serial.read(&mut serial_buf) {
                    Ok(0) => {}
                    Ok(n) => {
                        let mut offset = 0usize;
                        while offset < n {
                            match socket.write(&serial_buf[offset..n]) {
                                Ok(0) => return,
                                Ok(wrote) => offset += wrote,
                                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                                    thread::sleep(Duration::from_millis(1));
                                }
                                Err(err) => {
                                    log::error!("serial socket write error: {err}");
                                    return;
                                }
                            }
                        }
                    }
                    Err(err)
                        if matches!(
                            err.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                        ) => {}
                    Err(err) => {
                        log::error!("serial read error: {err}");
                        return;
                    }
                }
            }
        });

        Ok(Self {
            io_reader,
            io_writer,
            stop,
        })
    }
}
