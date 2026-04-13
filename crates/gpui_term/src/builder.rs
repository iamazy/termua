use std::collections::HashMap;

use gpui::Context;

use crate::{CursorShape, PtySource, Terminal, TerminalType, backends};

/// Backend-agnostic terminal builder.
///
/// Both supported backends are always available; callers must pick a `TerminalBackendType`
/// explicitly so the created `Terminal` can remember its backend kind for future switching.
pub struct TerminalBuilder {
    term_type: TerminalType,
    inner: InnerBuilder,
}

enum InnerBuilder {
    Alacritty(backends::alacritty::TerminalBuilder),
    WezTerm(backends::wezterm::TerminalBuilder),
}

impl TerminalBuilder {
    pub fn new(
        backend_type: TerminalType,
        env: HashMap<String, String>,
        cursor_shape: CursorShape,
        max_scroll_history_lines: Option<usize>,
        window_id: u64,
        exit_fn: Option<fn(&mut Context<Terminal>)>,
    ) -> anyhow::Result<Self> {
        Self::new_with_pty(
            backend_type,
            PtySource::Local { env, window_id },
            cursor_shape,
            max_scroll_history_lines,
            exit_fn,
        )
    }

    pub fn new_with_pty(
        backend_type: TerminalType,
        pty_source: PtySource,
        cursor_shape: CursorShape,
        max_scroll_history_lines: Option<usize>,
        exit_fn: Option<fn(&mut Context<Terminal>)>,
    ) -> anyhow::Result<Self> {
        let inner = match backend_type {
            TerminalType::Alacritty => {
                InnerBuilder::Alacritty(backends::alacritty::TerminalBuilder::new(
                    pty_source,
                    cursor_shape,
                    max_scroll_history_lines,
                    exit_fn,
                )?)
            }
            TerminalType::WezTerm => {
                InnerBuilder::WezTerm(backends::wezterm::TerminalBuilder::new(
                    pty_source,
                    cursor_shape,
                    max_scroll_history_lines,
                    exit_fn,
                )?)
            }
        };

        Ok(Self {
            term_type: backend_type,
            inner,
        })
    }

    pub fn backend_type(&self) -> TerminalType {
        self.term_type
    }

    pub fn subscribe(self, cx: &Context<Terminal>) -> Terminal {
        match self.inner {
            InnerBuilder::Alacritty(b) => b.subscribe(cx),
            InnerBuilder::WezTerm(b) => b.subscribe(cx),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SerialFlowControl, SerialOptions, SerialParity, SerialStopBits, SshOptions};

    #[test]
    fn builder_exposes_new_with_pty_source_api() {
        // Compile-time only: keep the old `new(...)` and expose a new API that can accept SSH.
        let _ = || {
            let _ = TerminalBuilder::new(
                TerminalType::WezTerm,
                HashMap::new(),
                CursorShape::default(),
                None,
                0,
                None,
            );

            let _ = TerminalBuilder::new_with_pty(
                TerminalType::WezTerm,
                PtySource::Ssh {
                    env: HashMap::new(),
                    opts: SshOptions {
                        group: "test".to_string(),
                        name: "test".to_string(),
                        host: "127.0.0.1".to_string(),
                        port: None,
                        auth: crate::Authentication::Config,
                        proxy: crate::backends::ssh::SshProxyMode::Inherit,
                        backend: crate::SshBackend::default(),
                        tcp_nodelay: false,
                        tcp_keepalive: false,
                    },
                },
                CursorShape::default(),
                None,
                None,
            );

            let _ = TerminalBuilder::new_with_pty(
                TerminalType::WezTerm,
                PtySource::Serial {
                    opts: SerialOptions {
                        port: "COM1".to_string(),
                        baud: 9600,
                        data_bits: 8,
                        parity: SerialParity::None,
                        stop_bits: SerialStopBits::One,
                        flow_control: SerialFlowControl::None,
                    },
                },
                CursorShape::default(),
                None,
                None,
            );
        };
    }
}
