#[cfg(unix)]
use std::os::{
    fd::{AsFd, AsRawFd},
    unix::net::UnixStream,
};
use std::{collections::HashMap, process::ExitStatus, sync::Arc};
#[cfg(windows)]
use std::{
    net::{TcpListener, TcpStream},
    os::windows::io::{AsRawSocket, AsSocket},
};

use alacritty_terminal::{
    event::{OnResize, WindowSize},
    tty::{ChildEvent, EventedPty, EventedReadWrite},
};
use log::error;
use parking_lot::Mutex;
use polling::{Event, PollMode, Poller};
#[cfg(unix)]
use signal_hook::{
    SigId, consts,
    low_level::{pipe, unregister},
};
use wezterm_ssh::{
    Child, ChildKiller, FileDescriptor, MasterPty, PtySize, SshChildProcess, SshPty,
};

use crate::{SshOptions, cast::CastRecorderSender};

// Interest in PTY read/writes.
#[cfg(unix)]
const PTY_READ_WRITE_TOKEN: usize = 0;
#[cfg(windows)]
const PTY_READ_WRITE_TOKEN: usize = 2;
const PTY_CHILD_EVENT_TOKEN: usize = 1;

#[derive(Debug)]
pub struct PtyWriter {
    inner: FileDescriptor,
    cast_slot: Arc<Mutex<Option<CastRecorderSender>>>,
}

impl PtyWriter {
    fn new(inner: FileDescriptor, cast_slot: Arc<Mutex<Option<CastRecorderSender>>>) -> Self {
        Self { inner, cast_slot }
    }

    fn set_non_blocking(&mut self, enabled: bool) -> std::io::Result<()> {
        self.inner
            .set_non_blocking(enabled)
            .map_err(std::io::Error::other)
    }
}

impl std::io::Write for PtyWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        if n != 0
            && let Some(sender) = self.cast_slot.lock().as_ref()
        {
            sender.input(&buf[..n]);
        }
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

#[cfg(unix)]
impl AsRawFd for PtyWriter {
    fn as_raw_fd(&self) -> i32 {
        self.inner.as_raw_fd()
    }
}

#[cfg(unix)]
impl AsFd for PtyWriter {
    fn as_fd(&self) -> std::os::fd::BorrowedFd<'_> {
        self.inner.as_fd()
    }
}

#[cfg(windows)]
impl AsRawSocket for PtyWriter {
    fn as_raw_socket(&self) -> std::os::windows::io::RawSocket {
        self.inner.as_raw_socket()
    }
}

#[cfg(windows)]
impl AsSocket for PtyWriter {
    fn as_socket(&self) -> std::os::windows::io::BorrowedSocket<'_> {
        self.inner.as_socket()
    }
}

#[derive(Debug)]
pub struct Pty {
    pub pty: SshPty,
    pub child: SshChildProcess,
    pub io_reader: PtyReader,
    pub io_writer: PtyWriter,
    #[cfg(unix)]
    pub signals: UnixStream,
    #[cfg(unix)]
    pub sig_id: SigId,
    #[cfg(windows)]
    pub signals: TcpStream,
}

#[derive(Debug)]
pub struct PtyReader {
    fd: FileDescriptor,
    cast_slot: Arc<Mutex<Option<CastRecorderSender>>>,
}

impl PtyReader {
    fn set_non_blocking(&mut self, enabled: bool) -> std::io::Result<()> {
        self.fd
            .set_non_blocking(enabled)
            .map_err(std::io::Error::other)
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

impl std::io::Read for PtyReader {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        let n = self.fd.read(out)?;
        if n == 0 {
            return Ok(0);
        }
        if let Some(sender) = self.cast_slot.lock().as_ref() {
            sender.output(&out[..n]);
        }
        Ok(n)
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        let _ = self.child.kill();

        // Clear signal-hook handler.
        #[cfg(unix)]
        unregister(self.sig_id);

        let _ = self.child.wait();
    }
}

impl EventedPty for Pty {
    fn next_child_event(&mut self) -> Option<ChildEvent> {
        match self.child.try_wait() {
            Ok(Some(status)) => {
                #[cfg(unix)]
                {
                    use std::os::unix::process::ExitStatusExt;
                    let code = ExitStatus::from_raw(status.exit_code() as i32);
                    Some(ChildEvent::Exited(Some(code)))
                }

                #[cfg(windows)]
                {
                    use std::os::windows::process::ExitStatusExt;
                    let code = ExitStatus::from_raw(status.exit_code());
                    Some(ChildEvent::Exited(Some(code)))
                }
            }
            Ok(None) => None,
            Err(err) => {
                error!("Error checking child process termination: {}", err);
                None
            }
        }
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
        let _ = self.signals.set_nonblocking(true);

        #[cfg(unix)]
        {
            // Safety: `self` owns these fds and guarantees they outlive their registration in the
            // poller; `deregister` removes them before drop.
            unsafe {
                poller.add_with_mode(self.io_reader.as_raw_fd(), interest, mode)?;
                poller.add_with_mode(self.io_writer.as_raw_fd(), interest, mode)?;

                poller.add_with_mode(
                    &self.signals,
                    Event::readable(PTY_CHILD_EVENT_TOKEN),
                    PollMode::Level,
                )?;
            }
        }

        #[cfg(windows)]
        {
            // Safety: `self` owns these sockets and guarantees they outlive their registration in
            // the poller; `deregister` removes them before drop.
            unsafe {
                poller.add_with_mode(self.io_reader.as_raw_socket(), interest, mode)?;
                poller.add_with_mode(self.io_writer.as_raw_socket(), interest, mode)?;

                poller.add_with_mode(
                    self.signals.as_raw_socket(),
                    Event::readable(PTY_CHILD_EVENT_TOKEN),
                    PollMode::Level,
                )?;
            }
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

            poller.modify_with_mode(
                &self.signals,
                Event::readable(PTY_CHILD_EVENT_TOKEN),
                PollMode::Level,
            )?;
        }

        #[cfg(windows)]
        {
            poller.modify_with_mode(self.io_reader.as_socket(), interest, mode)?;
            poller.modify_with_mode(self.io_writer.as_socket(), interest, mode)?;

            poller.modify_with_mode(
                self.signals.as_socket(),
                Event::readable(PTY_CHILD_EVENT_TOKEN),
                PollMode::Level,
            )?;
        }

        Ok(())
    }

    fn deregister(&mut self, poller: &Arc<Poller>) -> std::io::Result<()> {
        #[cfg(unix)]
        {
            poller.delete(self.io_reader.as_fd())?;
            poller.delete(self.io_writer.as_fd())?;

            poller.delete(&self.signals)?;
        }

        #[cfg(windows)]
        {
            poller.delete(self.io_reader.as_socket())?;
            poller.delete(self.io_writer.as_socket())?;

            poller.delete(self.signals.as_socket())?;
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
    fn on_resize(&mut self, window_size: WindowSize) {
        let size = PtySize {
            rows: window_size.num_lines,
            cols: window_size.num_cols,
            pixel_width: window_size.cell_width,
            pixel_height: window_size.cell_height,
        };

        let _ = self.pty.resize(size);
    }
}

impl Pty {
    pub fn new(
        env: HashMap<String, String>,
        opts: SshOptions,
        cast_slot: Arc<Mutex<Option<CastRecorderSender>>>,
    ) -> anyhow::Result<(Self, wezterm_ssh::Sftp)> {
        let (pty, child, sftp) = crate::backends::ssh::connect(env, opts)?;
        let io_reader_fd = pty.reader.try_clone()?;
        let io_writer_fd = pty.writer.try_clone()?;

        #[cfg(unix)]
        {
            // Prepare signal handling before spawning child.
            let (signals, sig_id) = {
                let (sender, recv) = UnixStream::pair()?;

                // Register the recv end of the pipe for SIGCHLD.
                let sig_id = pipe::register(consts::SIGCHLD, sender)?;
                recv.set_nonblocking(true)?;
                (recv, sig_id)
            };

            Ok((
                Pty {
                    pty,
                    child,
                    io_reader: PtyReader {
                        fd: io_reader_fd,
                        cast_slot: Arc::clone(&cast_slot),
                    },
                    io_writer: PtyWriter::new(io_writer_fd, cast_slot),
                    signals,
                    sig_id,
                },
                sftp,
            ))
        }

        #[cfg(windows)]
        {
            let listener = TcpListener::bind("127.0.0.1:0")?;
            let signals = TcpStream::connect(listener.local_addr()?)?;
            Ok((
                Pty {
                    pty,
                    child,
                    io_reader: PtyReader {
                        fd: io_reader_fd,
                        cast_slot: Arc::clone(&cast_slot),
                    },
                    io_writer: PtyWriter::new(io_writer_fd, cast_slot),
                    signals,
                },
                sftp,
            ))
        }
    }
}
