use std::{
    collections::HashMap,
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

use anyhow::Context as _;
use async_tungstenite::tungstenite::Message;
use futures::StreamExt as _;
use gpui::{App, AppContext, EntityId};
use gpui_term::{
    RemoteBackend, RemoteBackendEvent, Terminal,
    remote::{RemoteFrame, RemoteInputEvent, RemoteSelectionUpdate, RemoteSnapshot},
};
use smol::channel::{Receiver, Sender};
pub(crate) use termua_relay::protocol::{ClientToRelay, RelayToClient};

use crate::window::main_window::TermuaWindow;

gpui::actions!(
    termua_sharing,
    [
        StartSharing,
        StopSharing,
        JoinSharing,
        RequestControl,
        ReleaseControl,
        RevokeControl
    ]
);

pub(crate) const DEFAULT_RELAY_URL: &str = "ws://127.0.0.1:7231/ws";
pub(crate) const ROOM_ID_LEN: usize = 9;
pub(crate) const JOIN_KEY_LEN: usize = 6;
const SHARE_CODE_CHARS: &str = "ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789";

#[derive(Debug, Clone)]
pub(crate) struct HostShare {
    pub room_id: String,
    pub controller_id: Option<String>,
    pub pending_request: bool,
    pub conn: RelayConn,
    pub seq: u64,
    pub dirty: Arc<AtomicBool>,
    pub selection_dirty: Arc<AtomicBool>,
}

#[derive(Debug, Clone)]
pub(crate) struct ViewerShare {
    pub room_id: String,
    pub viewer_id: Arc<Mutex<Option<String>>>,
    pub controlled: Arc<AtomicBool>,
    pub conn: RelayConn,
}

#[derive(Default)]
pub(crate) struct LocalRelayProcessState {
    child: Option<Child>,
}

impl gpui::Global for LocalRelayProcessState {}

#[derive(Default)]
pub(crate) struct RelaySharingState {
    pub hosts: HashMap<EntityId, HostShare>,
    pub viewers: HashMap<EntityId, ViewerShare>,
}

impl gpui::Global for RelaySharingState {}

#[derive(Debug)]
enum RelayConnCommand {
    Send(ClientToRelay),
    Close,
}

#[derive(Debug, Clone)]
pub(crate) struct RelayConn {
    tx: Sender<RelayConnCommand>,
    rx: Receiver<RelayToClient>,
}

impl RelayConn {
    pub fn send(&self, msg: ClientToRelay) {
        let _ = self.tx.try_send(RelayConnCommand::Send(msg));
    }

    pub async fn recv(&self) -> Option<RelayToClient> {
        self.rx.recv().await.ok()
    }

    pub fn close(&self) {
        let _ = self.tx.try_send(RelayConnCommand::Close);
    }
}

pub(crate) fn init_globals(cx: &mut App) {
    if cx.try_global::<RelaySharingState>().is_none() {
        cx.set_global(RelaySharingState::default());
    }
    if cx.try_global::<LocalRelayProcessState>().is_none() {
        cx.set_global(LocalRelayProcessState::default());
    }
}

pub(crate) fn relay_url_from_env() -> String {
    std::env::var("TERMUA_RELAY_URL").unwrap_or_else(|_| DEFAULT_RELAY_URL.to_string())
}

pub(crate) fn sharing_feature_enabled(cx: &App) -> bool {
    cx.try_global::<crate::settings::SharingSettings>()
        .map(|s| s.enabled)
        .unwrap_or(false)
}

pub(crate) fn effective_relay_url(cx: &App) -> String {
    if let Some(cfg) = cx.try_global::<crate::settings::SharingSettings>() {
        if let Some(url) = cfg
            .relay_url
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            return url.to_string();
        }
    }
    relay_url_from_env()
}

pub(crate) fn disconnect_terminal_sharing<C>(terminal_view_id: EntityId, cx: &mut C)
where
    C: AppContext + std::borrow::BorrowMut<App>,
{
    let app = cx.borrow_mut();
    let Some(state) = app.try_global::<RelaySharingState>() else {
        return;
    };

    if state.hosts.contains_key(&terminal_view_id) {
        let host = app
            .global_mut::<RelaySharingState>()
            .hosts
            .remove(&terminal_view_id);
        if let Some(host) = host {
            host.conn.send(ClientToRelay::Stop {
                room_id: host.room_id.clone(),
            });
            host.conn.close();
        }
        return;
    }

    let viewer = app
        .global_mut::<RelaySharingState>()
        .viewers
        .remove(&terminal_view_id);
    if let Some(viewer) = viewer {
        viewer.conn.close();
    }
}

pub(crate) fn local_relay_listen_addr_from_ws_url(url: &str) -> anyhow::Result<String> {
    let url = url.trim();
    if url.starts_with("wss://") {
        anyhow::bail!("wss:// is not supported for local relay");
    }
    if !url.starts_with("ws://") {
        anyhow::bail!("relay url must start with ws://");
    }

    let rest = &url["ws://".len()..];
    let hostport = rest.split('/').next().unwrap_or("").trim();
    if hostport.is_empty() {
        anyhow::bail!("missing host:port");
    }

    // Relay listen args expect a SocketAddr; it does not resolve hostnames.
    let hostport = if hostport.starts_with("localhost:") {
        hostport.replacen("localhost:", "127.0.0.1:", 1)
    } else {
        hostport.to_string()
    };

    // Validate formatting early so the UI can show a clear error.
    let _: std::net::SocketAddr = hostport.parse()?;
    Ok(hostport)
}

pub(crate) fn local_relay_running(cx: &mut App) -> bool {
    if cx.try_global::<LocalRelayProcessState>().is_none() {
        cx.set_global(LocalRelayProcessState::default());
    }
    let state = cx.global_mut::<LocalRelayProcessState>();
    if let Some(child) = state.child.as_mut() {
        if let Ok(Some(_status)) = child.try_wait() {
            state.child = None;
        }
    }
    state.child.is_some()
}

pub(crate) fn start_local_relay(listen_addr: &str, cx: &mut App) -> anyhow::Result<()> {
    if cx.try_global::<LocalRelayProcessState>().is_none() {
        cx.set_global(LocalRelayProcessState::default());
    }

    // If already running, no-op.
    if local_relay_running(cx) {
        return Ok(());
    }

    let mut cmd = local_relay_command(listen_addr)?;
    cmd.stdout(Stdio::null()).stderr(Stdio::null());

    match cmd.spawn() {
        Ok(child) => {
            let state = cx.global_mut::<LocalRelayProcessState>();
            state.child = Some(child);
            Ok(())
        }
        Err(err) => {
            let state = cx.global_mut::<LocalRelayProcessState>();
            state.child = None;
            Err(err.into())
        }
    }
}

fn local_relay_command(listen_addr: &str) -> anyhow::Result<Command> {
    // Intentionally keep this small and testable: all env/config parsing should happen before
    // calling this function.
    let exe = std::env::current_exe().context("resolve current executable path")?;
    let mut cmd = Command::new(exe);
    cmd.arg("--run-relay").arg("--listen").arg(listen_addr);
    Ok(cmd)
}

pub(crate) fn stop_local_relay(cx: &mut App) {
    if cx.try_global::<LocalRelayProcessState>().is_none() {
        return;
    }
    let mut child = cx.global_mut::<LocalRelayProcessState>().child.take();
    if let Some(mut child) = child.take() {
        thread::spawn(move || {
            let _ = child.kill();
            let _ = child.wait();
        });
    }
}

pub(crate) fn clear_host_control_state(host: &mut HostShare) {
    host.controller_id = None;
    host.pending_request = false;
}

#[cfg(test)]
pub(crate) struct TestRelayConn {
    pub conn: RelayConn,
    sent_rx: Receiver<ClientToRelay>,
}

#[cfg(test)]
impl TestRelayConn {
    pub(crate) fn new() -> Self {
        let (cmd_tx, cmd_rx) = smol::channel::unbounded::<RelayConnCommand>();
        let (sent_tx, sent_rx) = smol::channel::unbounded::<ClientToRelay>();
        let (_incoming_tx, incoming_rx) = smol::channel::unbounded::<RelayToClient>();

        smol::spawn(async move {
            while let Ok(cmd) = cmd_rx.recv().await {
                match cmd {
                    RelayConnCommand::Send(msg) => {
                        let _ = sent_tx.send(msg).await;
                    }
                    RelayConnCommand::Close => break,
                }
            }
        })
        .detach();

        Self {
            conn: RelayConn {
                tx: cmd_tx,
                rx: incoming_rx,
            },
            sent_rx,
        }
    }

    pub(crate) async fn next_sent(&self) -> Option<ClientToRelay> {
        self.sent_rx.recv().await.ok()
    }
}

#[cfg(test)]
mod local_relay_url_tests {
    use super::*;

    #[test]
    fn local_relay_listen_addr_parses_ip_port() {
        assert_eq!(
            local_relay_listen_addr_from_ws_url("ws://127.0.0.1:7231/ws").unwrap(),
            "127.0.0.1:7231"
        );
    }

    #[test]
    fn local_relay_listen_addr_maps_localhost() {
        assert_eq!(
            local_relay_listen_addr_from_ws_url("ws://localhost:7231/ws").unwrap(),
            "127.0.0.1:7231"
        );
    }

    #[test]
    fn local_relay_listen_addr_rejects_wss() {
        assert!(local_relay_listen_addr_from_ws_url("wss://127.0.0.1:7231/ws").is_err());
    }
}

#[cfg(test)]
mod local_relay_command_tests {
    use super::*;

    #[test]
    fn local_relay_command_uses_current_exe_and_run_relay_flag() {
        let cmd = local_relay_command("127.0.0.1:7231").unwrap();
        assert_eq!(cmd.get_program(), std::env::current_exe().unwrap());

        let args: Vec<_> = cmd
            .get_args()
            .map(|v| v.to_string_lossy().to_string())
            .collect();
        assert_eq!(
            args,
            vec![
                "--run-relay".to_string(),
                "--listen".to_string(),
                "127.0.0.1:7231".to_string(),
            ]
        );
    }
}

#[cfg(test)]
mod host_control_state_tests {
    use super::*;

    fn dummy_relay_conn() -> RelayConn {
        let (conn, _evt_tx) = dummy_relay_conn_with_event_sender();
        conn
    }

    fn dummy_relay_conn_with_event_sender() -> (RelayConn, smol::channel::Sender<RelayToClient>) {
        let (cmd_tx, _cmd_rx) = smol::channel::unbounded::<RelayConnCommand>();
        let (evt_tx, evt_rx) = smol::channel::unbounded::<RelayToClient>();
        (
            RelayConn {
                tx: cmd_tx,
                rx: evt_rx,
            },
            evt_tx,
        )
    }

    #[test]
    fn clear_host_control_state_clears_controller_and_pending_request() {
        let mut host = HostShare {
            room_id: "AbC123xYz".to_string(),
            controller_id: Some("viewer-1".to_string()),
            pending_request: true,
            conn: dummy_relay_conn(),
            seq: 0,
            dirty: Arc::new(AtomicBool::new(false)),
            selection_dirty: Arc::new(AtomicBool::new(false)),
        };

        clear_host_control_state(&mut host);
        assert!(host.controller_id.is_none());
        assert!(!host.pending_request);
    }

    #[test]
    fn relay_conn_recv_waits_for_next_message() {
        smol::block_on(async {
            let (conn, evt_tx) = dummy_relay_conn_with_event_sender();

            evt_tx
                .send(RelayToClient::Pong)
                .await
                .expect("send test relay event");

            assert!(matches!(conn.recv().await, Some(RelayToClient::Pong)));
        });
    }

    #[test]
    fn connect_relay_exposes_async_api() {
        let _future = async {
            let _ = connect_relay("ws://127.0.0.1:1/ws", ClientToRelay::Ping).await;
        };
    }
}

pub(crate) fn gen_room_id() -> String {
    gen_code(ROOM_ID_LEN)
}

pub(crate) fn gen_join_key() -> String {
    gen_code(JOIN_KEY_LEN)
}

pub(crate) fn compose_share_key(room_id: &str, join_key: &str) -> String {
    format!("{room_id}-{join_key}")
}

pub(crate) fn parse_share_key(value: &str) -> anyhow::Result<(String, String)> {
    let normalized: String = value
        .trim()
        .chars()
        .filter(|c| !matches!(c, '-' | ':' | '_' | ' '))
        .collect();

    let expected_len = ROOM_ID_LEN + JOIN_KEY_LEN;
    if normalized.chars().count() != expected_len {
        anyhow::bail!("Share Key must be {expected_len} chars total (excluding separators).");
    }

    let room_id: String = normalized.chars().take(ROOM_ID_LEN).collect();
    let join_key: String = normalized.chars().skip(ROOM_ID_LEN).collect();

    if !valid_share_code(&room_id, ROOM_ID_LEN) {
        anyhow::bail!("Share Key is invalid.");
    }
    if !valid_share_code(&join_key, JOIN_KEY_LEN) {
        anyhow::bail!("Share Key is invalid.");
    }

    Ok((room_id, join_key))
}

pub(crate) fn valid_share_code(value: &str, len: usize) -> bool {
    value.chars().count() == len && value.chars().all(|c| SHARE_CODE_CHARS.contains(c))
}

fn gen_code(len: usize) -> String {
    let mut out = String::with_capacity(len);
    for _ in 0..len {
        let idx = (rand::random::<u32>() as usize) % SHARE_CODE_CHARS.len();
        out.push(SHARE_CODE_CHARS.as_bytes()[idx] as char);
    }
    out
}

pub(crate) async fn connect_relay(
    relay_url: &str,
    initial: ClientToRelay,
) -> anyhow::Result<RelayConn> {
    let (ws, _resp) = async_tungstenite::smol::connect_async(relay_url).await?;
    let (mut ws_tx, mut ws_rx) = ws.split();

    futures::SinkExt::send(
        &mut ws_tx,
        Message::Text(serde_json::to_string(&initial)?.into()),
    )
    .await?;

    let (cmd_tx, cmd_rx) = smol::channel::unbounded::<RelayConnCommand>();
    let (evt_tx, evt_rx) = smol::channel::unbounded::<RelayToClient>();

    smol::spawn(async move {
        loop {
            match cmd_rx.recv().await {
                Ok(RelayConnCommand::Send(cmd)) => {
                    let Ok(text) = serde_json::to_string(&cmd) else {
                        continue;
                    };
                    if futures::SinkExt::send(&mut ws_tx, Message::Text(text.into()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Ok(RelayConnCommand::Close) | Err(_) => {
                    let _ = futures::SinkExt::close(&mut ws_tx).await;
                    break;
                }
            }
        }
    })
    .detach();

    smol::spawn(async move {
        while let Some(msg) = ws_rx.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    let Ok(msg) = serde_json::from_str::<RelayToClient>(text.as_str()) else {
                        continue;
                    };
                    if evt_tx.send(msg).await.is_err() {
                        break;
                    }
                }
                Ok(Message::Close(_)) => break,
                Ok(_) => {}
                Err(_) => break,
            }
        }
    })
    .detach();

    Ok(RelayConn {
        tx: cmd_tx,
        rx: evt_rx,
    })
}

pub(crate) fn is_remote_terminal(terminal: &gpui::Entity<Terminal>, cx: &App) -> bool {
    terminal.read(cx).backend_name() == "remote"
}

pub(crate) fn viewer_controlled(terminal_view_id: EntityId, cx: &App) -> bool {
    cx.try_global::<RelaySharingState>()
        .and_then(|s| s.viewers.get(&terminal_view_id))
        .map(|v| v.controlled.load(Ordering::Relaxed))
        .unwrap_or(false)
}

pub(crate) fn host_sharing(terminal_view_id: EntityId, cx: &App) -> bool {
    cx.try_global::<RelaySharingState>()
        .is_some_and(|s| s.hosts.contains_key(&terminal_view_id))
}

pub(crate) fn viewer_sharing(terminal_view_id: EntityId, cx: &App) -> bool {
    cx.try_global::<RelaySharingState>()
        .is_some_and(|s| s.viewers.contains_key(&terminal_view_id))
}

pub(crate) fn host_controller_present(terminal_view_id: EntityId, cx: &App) -> bool {
    cx.try_global::<RelaySharingState>()
        .and_then(|s| s.hosts.get(&terminal_view_id))
        .and_then(|h| h.controller_id.clone())
        .is_some()
}

pub(crate) fn viewer_can_copy_paste(
    terminal: &gpui::Entity<Terminal>,
    terminal_view_id: EntityId,
    cx: &App,
) -> bool {
    if !is_remote_terminal(terminal, cx) {
        return true;
    }
    viewer_controlled(terminal_view_id, cx)
}

pub(crate) fn make_remote_terminal(
    send_input: Arc<dyn Send + Sync + Fn(RemoteInputEvent)>,
    controlled: Arc<AtomicBool>,
    cx: &mut gpui::Context<TermuaWindow>,
) -> gpui::Entity<Terminal> {
    cx.new(|_cx| {
        Terminal::new(
            gpui_term::TerminalType::WezTerm,
            Box::new(RemoteBackend::new(controlled, send_input)),
        )
    })
}

pub(crate) fn apply_remote_snapshot(
    terminal: &gpui::Entity<Terminal>,
    snapshot: RemoteSnapshot,
    cx: &mut gpui::Context<TermuaWindow>,
) {
    terminal.update(cx, |term, cx| {
        term.dispatch_backend_event(Box::new(RemoteBackendEvent::ApplySnapshot(snapshot)), cx);
    });
}

pub(crate) fn apply_remote_frame(
    terminal: &gpui::Entity<Terminal>,
    frame: RemoteFrame,
    cx: &mut gpui::Context<TermuaWindow>,
) {
    terminal.update(cx, |term, cx| {
        term.dispatch_backend_event(Box::new(RemoteBackendEvent::ApplyFrame(frame)), cx);
    });
}

pub(crate) fn apply_remote_selection_update(
    terminal: &gpui::Entity<Terminal>,
    update: RemoteSelectionUpdate,
    cx: &mut gpui::Context<TermuaWindow>,
) {
    terminal.update(cx, |term, cx| {
        term.dispatch_backend_event(
            Box::new(RemoteBackendEvent::ApplySelectionUpdate(update)),
            cx,
        );
    });
}

#[cfg(test)]
mod share_key_tests {
    use super::*;

    #[test]
    fn compose_share_key_roundtrips_via_parse() {
        let share_key = compose_share_key("AbC234xYz", "k3Y9a2");

        assert_eq!(share_key, "AbC234xYz-k3Y9a2");
        assert_eq!(
            parse_share_key(&share_key).expect("expected share key to parse"),
            ("AbC234xYz".to_string(), "k3Y9a2".to_string())
        );
    }

    #[test]
    fn parse_share_key_accepts_compact_and_spaced_forms() {
        assert_eq!(
            parse_share_key(" AbC234xYz k3Y9a2 ").expect("expected compact share key to parse"),
            ("AbC234xYz".to_string(), "k3Y9a2".to_string())
        );
        assert_eq!(
            parse_share_key("AbC234xYz:k3Y9a2").expect("expected colon share key to parse"),
            ("AbC234xYz".to_string(), "k3Y9a2".to_string())
        );
    }

    #[test]
    fn parse_share_key_rejects_invalid_segments() {
        assert!(parse_share_key("bad-key").is_err());
        assert!(parse_share_key("AbC234xYz-00OOIl").is_err());
        assert!(parse_share_key("AbC234xY-k3Y9a2").is_err());
    }

    #[test]
    fn parse_share_key_errors_use_share_key_wording_only() {
        let err = parse_share_key("AbC234xYz-00OOIl")
            .expect_err("expected invalid share key to fail")
            .to_string();

        assert!(err.contains("Share Key"));
        assert!(!err.contains("Room ID"));
        assert!(!err.contains("Join Key"));
    }
}
