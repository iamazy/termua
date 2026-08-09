use std::{
    collections::{HashMap, HashSet},
    io,
    net::SocketAddr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use async_tungstenite::tungstenite::Message;
use futures::{FutureExt, StreamExt};
use serde::{Deserialize, Serialize};
use smol::io::{AsyncReadExt, AsyncWriteExt};

const TERMINAL_PAGE: &str = include_str!("index.html");
const TERMINAL_STYLE: &str = include_str!("terminal.css");
const TERMINAL_SCRIPT: &str = include_str!("terminal.js");

pub struct WebShareServer {
    addr: SocketAddr,
    state: Arc<Mutex<ServerState>>,
    control_rx: smol::channel::Receiver<ControlRequest>,
    input_rx: smol::channel::Receiver<WebInput>,
    snapshot_tx: smol::channel::Sender<()>,
    shutdown_tx: smol::channel::Sender<()>,
}

struct ServerState {
    access: ShareAccess,
    snapshot: gpui_term::TerminalScreen,
    broadcast_screen: gpui_term::TerminalScreen,
    clients: HashMap<ClientId, ClientConnection>,
    control_tx: smol::channel::Sender<ControlRequest>,
    input_tx: smol::channel::Sender<WebInput>,
}

#[derive(Clone)]
struct ClientConnection {
    messages: smol::channel::Sender<Message>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct XtermTheme {
    foreground: String,
    background: String,
    cursor: String,
    selection_background: String,
    black: String,
    red: String,
    green: String,
    yellow: String,
    blue: String,
    magenta: String,
    cyan: String,
    white: String,
    bright_black: String,
    bright_red: String,
    bright_green: String,
    bright_yellow: String,
    bright_blue: String,
    bright_magenta: String,
    bright_cyan: String,
    bright_white: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControlRequest {
    pub request_id: ControlRequestId,
    pub client_id: ClientId,
    pub peer: SocketAddr,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebInput {
    pub client_id: ClientId,
    pub data: Vec<u8>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientMessage {
    Authenticate { token: String },
    RequestControl,
    Input { data: String },
}

static NEXT_CLIENT_ID: AtomicU64 = AtomicU64::new(1);

pub fn generate_token() -> String {
    let bytes: [u8; 32] = rand::random();
    let mut token = String::with_capacity(64);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(token, "{byte:02x}");
    }
    token
}

impl XtermTheme {
    pub fn from_app_theme(colors: &gpui_component::ThemeColor) -> Self {
        Self {
            foreground: css_color(colors.foreground),
            background: css_color(colors.background),
            cursor: css_color(colors.caret),
            selection_background: css_color(colors.selection),
            black: css_color(colors.foreground),
            red: css_color(colors.red),
            green: css_color(colors.green),
            yellow: css_color(colors.yellow),
            blue: css_color(colors.blue),
            magenta: css_color(colors.magenta),
            cyan: css_color(colors.cyan),
            white: css_color(colors.background),
            bright_black: css_color(colors.muted_foreground),
            bright_red: css_color(colors.red_light),
            bright_green: css_color(colors.green_light),
            bright_yellow: css_color(colors.yellow_light),
            bright_blue: css_color(colors.blue_light),
            bright_magenta: css_color(colors.magenta_light),
            bright_cyan: css_color(colors.cyan_light),
            bright_white: css_color(colors.background),
        }
    }
}

impl Default for XtermTheme {
    fn default() -> Self {
        Self {
            foreground: "#d8dee9".into(),
            background: "#111111".into(),
            cursor: "#d8dee9".into(),
            selection_background: "#434c5e".into(),
            black: "#d8dee9".into(),
            red: "#bf616a".into(),
            green: "#a3be8c".into(),
            yellow: "#ebcb8b".into(),
            blue: "#81a1c1".into(),
            magenta: "#b48ead".into(),
            cyan: "#88c0d0".into(),
            white: "#111111".into(),
            bright_black: "#7b8496".into(),
            bright_red: "#d57780".into(),
            bright_green: "#b1d196".into(),
            bright_yellow: "#f0d399".into(),
            bright_blue: "#8faed0".into(),
            bright_magenta: "#c19abb".into(),
            bright_cyan: "#95ccdc".into(),
            bright_white: "#111111".into(),
        }
    }
}

fn css_color(color: gpui::Hsla) -> String {
    let color = color.to_rgb();
    let channel = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!(
        "#{:02x}{:02x}{:02x}",
        channel(color.r),
        channel(color.g),
        channel(color.b)
    )
}

fn escape_html_text(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(character),
        }
    }
    escaped
}

pub fn local_network_ip() -> std::net::IpAddr {
    std::net::UdpSocket::bind(("0.0.0.0", 0))
        .and_then(|socket| {
            socket.connect(("8.8.8.8", 80))?;
            socket.local_addr().map(|addr| addr.ip())
        })
        .unwrap_or(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST))
}

impl WebShareServer {
    #[cfg(test)]
    pub async fn bind(token: String, snapshot: gpui_term::TerminalScreen) -> io::Result<Self> {
        Self::bind_with_theme(
            token,
            snapshot,
            XtermTheme::default(),
            "Termua Web Terminal".into(),
        )
        .await
    }

    pub async fn bind_with_theme(
        token: String,
        snapshot: gpui_term::TerminalScreen,
        theme: XtermTheme,
        tab_name: String,
    ) -> io::Result<Self> {
        let listener = smol::net::TcpListener::bind(("0.0.0.0", 0)).await?;
        let addr = listener.local_addr()?;
        let (control_tx, control_rx) = smol::channel::bounded(16);
        let (input_tx, input_rx) = smol::channel::bounded(256);
        let (snapshot_tx, snapshot_rx) = smol::channel::bounded(1);
        let (shutdown_tx, shutdown_rx) = smol::channel::bounded(1);
        let state = Arc::new(Mutex::new(ServerState {
            access: ShareAccess::new(token),
            broadcast_screen: snapshot.clone(),
            snapshot,
            clients: HashMap::new(),
            control_tx,
            input_tx,
        }));
        let listener_state = Arc::clone(&state);
        let broadcaster_state = Arc::clone(&state);
        smol::spawn(async move {
            while snapshot_rx.recv().await.is_ok() {
                smol::Timer::after(std::time::Duration::from_millis(16)).await;
                while snapshot_rx.try_recv().is_ok() {}

                let mut state = broadcaster_state.lock().expect("web share state poisoned");
                let ansi = gpui_term::serialize_terminal_screen_update_ansi(
                    &state.broadcast_screen,
                    &state.snapshot,
                );
                state.broadcast_screen = state.snapshot.clone();
                let message = screen_message(2, &state.snapshot, ansi);
                let failed: Vec<_> = state
                    .clients
                    .iter()
                    .filter_map(|(client_id, client)| {
                        client
                            .messages
                            .try_send(message.clone())
                            .is_err()
                            .then_some(*client_id)
                    })
                    .collect();
                for client_id in failed {
                    state.clients.remove(&client_id);
                    state.access.disconnect(client_id);
                }
            }
        })
        .detach();
        let terminal_page: Arc<str> = TERMINAL_PAGE
            .replace("__TERMUA_STYLE__", TERMINAL_STYLE)
            .replace("__TERMUA_SCRIPT__", TERMINAL_SCRIPT)
            .replace(
                "__TERMUA_THEME__",
                &serde_json::to_string(&theme).expect("web terminal theme must serialize"),
            )
            .replace("__TERMUA_TAB_NAME__", &escape_html_text(&tab_name))
            .into();
        smol::spawn(async move {
            loop {
                let accept = listener.accept().fuse();
                let shutdown = shutdown_rx.recv().fuse();
                futures::pin_mut!(accept, shutdown);
                let accepted = futures::select! {
                    accepted = accept => accepted.ok(),
                    _ = shutdown => None,
                };
                let Some((stream, peer)) = accepted else {
                    break;
                };
                let state = Arc::clone(&listener_state);
                let terminal_page = Arc::clone(&terminal_page);
                smol::spawn(async move {
                    let mut peek = [0; 2048];
                    let is_websocket = stream
                        .peek(&mut peek)
                        .await
                        .map(|n| {
                            String::from_utf8_lossy(&peek[..n])
                                .to_ascii_lowercase()
                                .contains("upgrade: websocket")
                        })
                        .unwrap_or(false);
                    if is_websocket {
                        let _ = serve_websocket(stream, peer, state).await;
                    } else {
                        let _ = serve_http(stream, &terminal_page).await;
                    }
                })
                .detach();
            }
        })
        .detach();
        Ok(Self {
            addr,
            state,
            control_rx,
            input_rx,
            snapshot_tx,
            shutdown_tx,
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.try_send(());
        let mut state = self.state.lock().expect("web share state poisoned");
        for client in state.clients.values() {
            let _ = client.messages.try_send(Message::Close(None));
        }
        state.clients.clear();
    }

    pub fn control_requests(&self) -> smol::channel::Receiver<ControlRequest> {
        self.control_rx.clone()
    }

    pub fn inputs(&self) -> smol::channel::Receiver<WebInput> {
        self.input_rx.clone()
    }

    pub fn approve_control(&self, request: ControlRequestId) -> Result<(), AccessError> {
        let mut state = self.state.lock().expect("web share state poisoned");
        state.access.approve(request)?;
        broadcast_access(&state);
        Ok(())
    }

    pub fn deny_control(&self, request: ControlRequestId) {
        self.state
            .lock()
            .expect("web share state poisoned")
            .access
            .deny(request);
    }

    pub fn update_snapshot(&self, snapshot: gpui_term::TerminalScreen) {
        let mut state = self.state.lock().expect("web share state poisoned");
        state.snapshot = snapshot;
        drop(state);
        let _ = self.snapshot_tx.try_send(());
    }
}

fn access_message(control: bool) -> Message {
    Message::Text(format!(r#"{{"type":"access","control":{control}}}"#).into())
}

fn screen_message(kind: u8, screen: &gpui_term::TerminalScreen, ansi: Vec<u8>) -> Message {
    let mut message = Vec::with_capacity(ansi.len() + 9);
    message.push(kind);
    message.extend_from_slice(&(screen.columns() as u32).to_be_bytes());
    message.extend_from_slice(&(screen.rows() as u32).to_be_bytes());
    message.extend(ansi);
    Message::Binary(message.into())
}

fn broadcast_access(state: &ServerState) {
    for (client_id, client) in &state.clients {
        let _ = client
            .messages
            .try_send(access_message(state.access.can_input(*client_id)));
    }
}

async fn serve_websocket(
    stream: smol::net::TcpStream,
    peer: SocketAddr,
    state: Arc<Mutex<ServerState>>,
) -> Result<(), async_tungstenite::tungstenite::Error> {
    let socket = async_tungstenite::accept_async(stream).await?;
    let client_id = NEXT_CLIENT_ID.fetch_add(1, Ordering::Relaxed);
    let (out_tx, out_rx) = smol::channel::bounded::<Message>(64);
    let connection = ClientConnection {
        messages: out_tx.clone(),
    };
    let (mut sink, mut source) = socket.split();
    let mut authenticated = false;

    loop {
        let incoming = source.next().fuse();
        let outgoing = out_rx.recv().fuse();
        futures::pin_mut!(incoming, outgoing);
        futures::select! {
            incoming = incoming => {
                let Some(incoming) = incoming else { break };
                let message = incoming?;
                if message.is_close() { break; }
                let Ok(text) = message.into_text() else { continue };
                let Ok(message) = serde_json::from_str::<ClientMessage>(&text) else { continue };
                match message {
                    ClientMessage::Authenticate { token } if !authenticated => {
                        let result = {
                            let mut state = state.lock().expect("web share state poisoned");
                            let result = state.access.connect(&token, client_id);
                            if result.is_ok() {
                                state.clients.insert(client_id, connection.clone());
                                let _ = out_tx.try_send(access_message(false));
                                let snapshot = screen_message(
                                    0,
                                    &state.snapshot,
                                    gpui_term::serialize_terminal_screen_ansi(&state.snapshot),
                                );
                                let _ = out_tx.try_send(snapshot);
                            }
                            result
                        };
                        if result.is_err() { break; }
                        authenticated = true;
                    }
                    ClientMessage::RequestControl if authenticated => {
                        let request = {
                            let mut state = state.lock().expect("web share state poisoned");
                            state.access.request_control(client_id).ok().map(|request_id| {
                                (state.control_tx.clone(), ControlRequest { request_id, client_id, peer })
                            })
                        };
                        if let Some((tx, request)) = request { let _ = tx.send(request).await; }
                    }
                    ClientMessage::Input { data } if authenticated => {
                        let tx = {
                            let state = state.lock().expect("web share state poisoned");
                            state.access.can_input(client_id).then(|| state.input_tx.clone())
                        };
                        if let Some(tx) = tx {
                            let _ = tx.send(WebInput { client_id, data: data.into_bytes() }).await;
                        }
                    }
                    _ => {}
                }
            },
            outgoing = outgoing => {
                let Ok(outgoing) = outgoing else { break };
                sink.send(outgoing).await?;
            },
        }
    }

    let mut state = state.lock().expect("web share state poisoned");
    state.clients.remove(&client_id);
    state.access.disconnect(client_id);
    broadcast_access(&state);
    Ok(())
}

async fn serve_http(mut stream: smol::net::TcpStream, terminal_page: &str) -> io::Result<()> {
    let mut request = vec![0; 8192];
    let read = stream.read(&mut request).await?;
    let request = String::from_utf8_lossy(&request[..read]);
    let (status, content_type, body) = if request.starts_with("GET / ") {
        ("200 OK", "text/html; charset=utf-8", terminal_page)
    } else {
        ("404 Not Found", "text/plain; charset=utf-8", "Not Found")
    };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await?;
    stream.flush().await
}

pub type ClientId = u64;
pub type ControlRequestId = u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientAccess {
    ReadOnly,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccessError {
    InvalidToken,
    UnknownClient,
    UnknownRequest,
    ControllerAlreadyActive,
    RequestAlreadyPending,
}

pub struct ShareAccess {
    token: String,
    clients: HashSet<ClientId>,
    controller: Option<ClientId>,
    pending: HashMap<ControlRequestId, ClientId>,
    next_request_id: ControlRequestId,
}

impl ShareAccess {
    pub fn new(token: String) -> Self {
        Self {
            token,
            clients: HashSet::new(),
            controller: None,
            pending: HashMap::new(),
            next_request_id: 1,
        }
    }

    pub fn connect(&mut self, token: &str, client: ClientId) -> Result<ClientAccess, AccessError> {
        if token != self.token {
            return Err(AccessError::InvalidToken);
        }
        self.clients.insert(client);
        Ok(ClientAccess::ReadOnly)
    }

    pub fn can_input(&self, client: ClientId) -> bool {
        self.controller == Some(client)
    }

    pub fn request_control(&mut self, client: ClientId) -> Result<ControlRequestId, AccessError> {
        if !self.clients.contains(&client) {
            return Err(AccessError::UnknownClient);
        }
        if self.controller.is_some() {
            return Err(AccessError::ControllerAlreadyActive);
        }
        if self.pending.values().any(|pending| *pending == client) {
            return Err(AccessError::RequestAlreadyPending);
        }
        let request = self.next_request_id;
        self.next_request_id = self.next_request_id.wrapping_add(1).max(1);
        self.pending.insert(request, client);
        Ok(request)
    }

    pub fn approve(&mut self, request: ControlRequestId) -> Result<(), AccessError> {
        if self.controller.is_some() {
            return Err(AccessError::ControllerAlreadyActive);
        }
        let client = self
            .pending
            .remove(&request)
            .ok_or(AccessError::UnknownRequest)?;
        if !self.clients.contains(&client) {
            return Err(AccessError::UnknownClient);
        }
        self.controller = Some(client);
        self.pending.clear();
        Ok(())
    }

    pub fn deny(&mut self, request: ControlRequestId) {
        self.pending.remove(&request);
    }

    pub fn disconnect(&mut self, client: ClientId) {
        self.clients.remove(&client);
        self.pending.retain(|_, pending| *pending != client);
        if self.controller == Some(client) {
            self.controller = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_term::{Cell, GridPoint, IndexedCell, TerminalContent};
    use smol::io::{AsyncReadExt, AsyncWriteExt};

    fn screen_with_text(text: &str) -> gpui_term::TerminalScreen {
        let mut content = TerminalContent::default();
        content.terminal_bounds = gpui_term::TerminalBounds::new(
            gpui::px(10.),
            gpui::px(10.),
            gpui::Bounds {
                origin: gpui::Point::default(),
                size: gpui::size(gpui::px(200.), gpui::px(20.)),
            },
        );
        content.cells = text
            .chars()
            .take(20)
            .enumerate()
            .map(|(column, c)| IndexedCell {
                point: GridPoint::new(0, column),
                cell: Cell {
                    c,
                    ..Default::default()
                },
            })
            .collect();
        gpui_term::capture_terminal_screen(&content)
    }

    fn loopback_addr(server: &WebShareServer) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], server.local_addr().port()))
    }

    #[test]
    fn authenticated_clients_start_read_only() {
        let mut access = ShareAccess::new("secret".into());
        assert_eq!(access.connect("wrong", 1), Err(AccessError::InvalidToken));
        assert_eq!(access.connect("secret", 1), Ok(ClientAccess::ReadOnly));
        assert!(!access.can_input(1));
    }

    #[test]
    fn control_requires_approval_and_is_exclusive() {
        let mut access = ShareAccess::new("secret".into());
        access.connect("secret", 1).unwrap();
        access.connect("secret", 2).unwrap();

        let request = access.request_control(1).unwrap();
        assert!(!access.can_input(1));
        assert_eq!(access.approve(request), Ok(()));
        assert!(access.can_input(1));
        assert_eq!(
            access.request_control(2),
            Err(AccessError::ControllerAlreadyActive)
        );
    }

    #[test]
    fn disconnecting_controller_releases_control() {
        let mut access = ShareAccess::new("secret".into());
        access.connect("secret", 1).unwrap();
        let request = access.request_control(1).unwrap();
        access.approve(request).unwrap();
        access.disconnect(1);

        access.connect("secret", 2).unwrap();
        assert!(access.request_control(2).is_ok());
    }

    #[test]
    fn denied_control_request_can_be_requested_again() {
        let mut access = ShareAccess::new("secret".into());
        access.connect("secret", 1).unwrap();
        let request = access.request_control(1).unwrap();
        assert_eq!(
            access.request_control(1),
            Err(AccessError::RequestAlreadyPending)
        );
        access.deny(request);
        assert!(access.request_control(1).is_ok());
    }

    #[test]
    fn screen_snapshot_contains_clear_rows_and_cursor_position() {
        let mut content = TerminalContent::default();
        content.terminal_bounds = gpui_term::TerminalBounds::new(
            gpui::px(10.),
            gpui::px(10.),
            gpui::Bounds {
                origin: gpui::Point::default(),
                size: gpui::size(gpui::px(20.), gpui::px(20.)),
            },
        );
        content.cursor.shape = gpui_term::CursorRenderShape::Block;
        content.cursor.point = GridPoint::new(1, 1);
        content.cells = vec![
            IndexedCell {
                point: GridPoint::new(0, 0),
                cell: Cell {
                    c: 'a',
                    ..Default::default()
                },
            },
            IndexedCell {
                point: GridPoint::new(1, 0),
                cell: Cell {
                    c: 'b',
                    ..Default::default()
                },
            },
        ];

        let ansi = gpui_term::serialize_terminal_content_ansi(&content);
        let ansi = String::from_utf8(ansi).unwrap();
        assert!(ansi.starts_with("\u{1b}[2J\u{1b}[H"));
        assert!(!ansi.contains("\r\n"));
        assert!(ansi.contains("\u{1b}[1;1H"));
        assert!(ansi.contains("a "));
        assert!(ansi.contains("\u{1b}[2;1H"));
        assert!(ansi.contains("b "));
        assert!(ansi.ends_with("\u{1b}[2;2H"));
    }

    #[test]
    fn screen_update_serializes_only_changed_rows() {
        let previous = screen_with_text("before");
        let current = screen_with_text("after");
        let ansi = gpui_term::serialize_terminal_screen_update_ansi(&previous, &current);
        let ansi = String::from_utf8(ansi).unwrap();

        assert!(!ansi.contains("\u{1b}[2J"));
        assert!(ansi.contains("\u{1b}[1;1H"));
        assert!(!ansi.contains("\u{1b}[2;1H"));
    }

    #[test]
    fn tiny_terminal_screen_keeps_valid_web_dimensions() {
        let mut content = TerminalContent::default();
        content.terminal_bounds = gpui_term::TerminalBounds::new(
            gpui::px(10.),
            gpui::px(10.),
            gpui::Bounds {
                origin: gpui::Point::default(),
                size: gpui::size(gpui::px(0.), gpui::px(0.)),
            },
        );
        let screen = gpui_term::capture_terminal_screen(&content);
        assert_eq!(screen.columns(), 1);
        assert_eq!(screen.rows(), 1);
    }

    #[test]
    fn local_server_serves_terminal_page() {
        smol::block_on(async {
            let server = WebShareServer::bind_with_theme(
                "secret".into(),
                screen_with_text("screen"),
                XtermTheme::default(),
                "bash & <tools>".into(),
            )
            .await
            .unwrap();
            let mut stream = smol::net::TcpStream::connect(loopback_addr(&server))
                .await
                .unwrap();
            stream
                .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                .await
                .unwrap();
            let mut response = String::new();
            stream.read_to_string(&mut response).await.unwrap();
            let compact_response: String = response
                .chars()
                .filter(|character| !character.is_ascii_whitespace())
                .collect();
            assert!(response.starts_with("HTTP/1.1 200 OK"));
            assert!(response.contains("xterm"));
            assert!(response.contains("request-control"));
            assert!(response.contains("<title>bash &amp; &lt;tools&gt;</title>"));
            assert!(compact_response.contains("theme:{"));
            assert!(compact_response.contains("term.resize(columns,rows)"));
            assert!(compact_response.contains("bytes[0]===2"));
            assert!(compact_response.contains("justify-content:center"));
            assert!(compact_response.contains("background:#000"));
            assert!(compact_response.contains("transform=`scale(${scale})`"));
            assert!(compact_response.contains("Math.min(1.15,"));
            assert!(!compact_response.contains("Math.min(wantedWidth,bounds.width)"));
            assert!(compact_response.contains("scrollback:0"));
            assert!(compact_response.contains(".xterm-viewport{overflow-y:hidden!important"));
            assert!(compact_response.contains("event.preventDefault()"));
            assert!(compact_response.contains("event.stopImmediatePropagation()"));
            assert!(!compact_response.contains("term.onScroll("));
            assert!(!compact_response.contains(r#"type:"history""#));
            assert!(response.contains(r#"<style data-termua-asset="terminal.css">"#));
            assert!(response.contains(r#"<script data-termua-asset="terminal.js">"#));
            assert!(!response.contains("__TERMUA_"));
        });
    }

    #[test]
    fn xterm_theme_uses_termua_named_color_mapping() {
        let colors = gpui_component::ThemeColor {
            foreground: gpui::rgb(0x123456).into(),
            background: gpui::rgb(0xabcdef).into(),
            muted_foreground: gpui::rgb(0x654321).into(),
            ..Default::default()
        };
        let theme = XtermTheme::from_app_theme(&colors);
        let json = serde_json::to_string(&theme).unwrap();

        assert!(json.contains(r##""foreground":"#123456""##));
        assert!(json.contains(r##""black":"#123456""##));
        assert!(json.contains(r##""white":"#abcdef""##));
        assert!(json.contains(r##""brightBlack":"#654321""##));
    }

    #[test]
    fn shutting_down_share_closes_listener() {
        smol::block_on(async {
            let server = WebShareServer::bind("secret".into(), screen_with_text(""))
                .await
                .unwrap();
            let addr = loopback_addr(&server);
            server.shutdown();
            smol::Timer::after(std::time::Duration::from_millis(20)).await;
            assert!(smol::net::TcpStream::connect(addr).await.is_err());
        });
    }

    #[test]
    fn websocket_input_is_blocked_until_desktop_approval() {
        smol::block_on(async {
            let server = WebShareServer::bind("secret".into(), screen_with_text("screen"))
                .await
                .unwrap();
            let url = format!("ws://127.0.0.1:{}/ws", server.local_addr().port());
            let (mut socket, _) = async_tungstenite::smol::connect_async(url).await.unwrap();
            socket
                .send(async_tungstenite::tungstenite::Message::Text(
                    r#"{"type":"authenticate","token":"secret"}"#.into(),
                ))
                .await
                .unwrap();
            let access = socket.next().await.unwrap().unwrap().into_text().unwrap();
            assert!(access.contains(r#""control":false"#));
            assert!(!access.contains("history_before"));
            let screen = socket.next().await.unwrap().unwrap().into_data();
            assert_eq!(screen[0], 0);
            assert_eq!(u32::from_be_bytes(screen[1..5].try_into().unwrap()), 20);
            assert_eq!(u32::from_be_bytes(screen[5..9].try_into().unwrap()), 2);

            socket
                .send(async_tungstenite::tungstenite::Message::Text(
                    r#"{"type":"input","data":"blocked"}"#.into(),
                ))
                .await
                .unwrap();
            assert!(server.inputs().try_recv().is_err());

            socket
                .send(async_tungstenite::tungstenite::Message::Text(
                    r#"{"type":"request_control"}"#.into(),
                ))
                .await
                .unwrap();
            let request = server.control_requests().recv().await.unwrap();
            server.approve_control(request.request_id).unwrap();
            let access = socket.next().await.unwrap().unwrap().into_text().unwrap();
            assert!(access.contains(r#""control":true"#));
            socket
                .send(async_tungstenite::tungstenite::Message::Text(
                    r#"{"type":"input","data":"allowed"}"#.into(),
                ))
                .await
                .unwrap();
            assert_eq!(server.inputs().recv().await.unwrap().data, b"allowed");
        });
    }

    #[test]
    fn rapid_snapshots_do_not_revoke_client_access() {
        smol::block_on(async {
            let server = WebShareServer::bind("secret".into(), screen_with_text("screen"))
                .await
                .unwrap();
            let url = format!("ws://127.0.0.1:{}/ws", server.local_addr().port());
            let (mut socket, _) = async_tungstenite::smol::connect_async(url).await.unwrap();
            socket
                .send(async_tungstenite::tungstenite::Message::Text(
                    r#"{"type":"authenticate","token":"secret"}"#.into(),
                ))
                .await
                .unwrap();
            assert!(socket.next().await.unwrap().unwrap().is_text());
            assert!(socket.next().await.unwrap().unwrap().is_binary());

            for frame in 0..100 {
                server.update_snapshot(screen_with_text(&format!("frame-{frame}")));
                smol::Timer::after(std::time::Duration::from_millis(1)).await;
            }
            let mut latest = Vec::new();
            loop {
                let message = socket.next().fuse();
                let quiet = futures::FutureExt::fuse(smol::Timer::after(
                    std::time::Duration::from_millis(30),
                ));
                futures::pin_mut!(message, quiet);
                let received = futures::select! {
                    message = message => message.map(|message| message.unwrap().into_data()),
                    _ = quiet => None,
                };
                let Some(received) = received else { break };
                latest = received.to_vec();
            }
            assert_eq!(latest[0], 2);
            assert!(String::from_utf8_lossy(&latest[9..]).contains("frame-99"));
            socket
                .send(async_tungstenite::tungstenite::Message::Text(
                    r#"{"type":"request_control"}"#.into(),
                ))
                .await
                .unwrap();

            let requests = server.control_requests();
            let request = requests.recv().fuse();
            let timeout =
                futures::FutureExt::fuse(smol::Timer::after(std::time::Duration::from_millis(100)));
            futures::pin_mut!(request, timeout);
            let received = futures::select! {
                request = request => request.is_ok(),
                _ = timeout => false,
            };
            assert!(received, "rapid snapshots must retain authenticated access");
        });
    }

    #[test]
    fn generated_share_token_has_256_bits_of_hex_entropy() {
        let first = generate_token();
        let second = generate_token();
        assert_eq!(first.len(), 64);
        assert!(first.chars().all(|ch| ch.is_ascii_hexdigit()));
        assert_ne!(first, second);
    }
}
