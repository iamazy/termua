use std::{
    collections::{HashMap, HashSet},
    io,
    net::SocketAddr,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use async_tungstenite::tungstenite::Message;
use futures::{FutureExt, StreamExt};
use serde::{Deserialize, Serialize};
use smol::io::{AsyncReadExt, AsyncWriteExt};

const TERMINAL_PAGE: &str = include_str!("index.html");
const XTERM_STYLE: &str = include_str!("../../../assets/xterm/xterm.css");
const XTERM_SCRIPT: &str = include_str!("../../../assets/xterm/xterm.js");
const TERMINAL_STYLE: &str = include_str!("terminal.css");
const TERMINAL_SCRIPT: &str = include_str!("terminal.js");

pub struct WebShareServer {
    addr: SocketAddr,
    session_id: String,
    hub: Arc<WebShareHub>,
    state: Arc<Mutex<ServerState>>,
    control_rx: smol::channel::Receiver<ControlRequest>,
    input_rx: smol::channel::Receiver<WebInput>,
    snapshot_tx: smol::channel::Sender<()>,
    closed: AtomicBool,
}

#[derive(Default)]
pub struct WebShareManager {
    hubs: smol::lock::Mutex<HashMap<u16, Weak<WebShareHub>>>,
}

struct WebShareHub {
    addr: SocketAddr,
    sessions: Arc<Mutex<HashMap<String, HubSession>>>,
    shutdown_tx: smol::channel::Sender<()>,
    closed: AtomicBool,
}

#[derive(Clone)]
struct HubSession {
    state: Arc<Mutex<ServerState>>,
    terminal_page: Arc<str>,
}

struct ServerState {
    access: ShareAccess,
    snapshot: gpui_term::TerminalScreen,
    broadcast_screen: gpui_term::TerminalScreen,
    clients: HashMap<ClientId, ClientConnection>,
    control_tx: smol::channel::Sender<ControlRequest>,
    input_tx: smol::channel::Sender<WebInput>,
    last_activity: Instant,
}

#[derive(Clone)]
struct ClientConnection {
    messages: smol::channel::Sender<Message>,
    socket: smol::net::TcpStream,
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
    Activity,
    RequestControl,
    ReleaseControl,
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

pub fn xterm_font_family(settings: &gpui_term::TerminalSettings) -> String {
    let mut families = Vec::new();
    let mut push = |family: &str| {
        let family = family.trim();
        if !family.is_empty() && !families.iter().any(|existing| existing == family) {
            families.push(family.to_string());
        }
    };
    push(settings.font_family.as_ref());
    if let Some(fallbacks) = &settings.font_fallbacks {
        for fallback in fallbacks.fallback_list() {
            push(fallback);
        }
    }
    for fallback in [
        "Symbols Nerd Font Mono",
        "Symbols Nerd Font",
        "Noto Sans Symbols 2",
        "Apple Symbols",
        "Segoe UI Symbol",
    ] {
        push(fallback);
    }
    families
        .into_iter()
        .map(|family| format!("\"{}\"", family.replace('\\', "\\\\").replace('"', "\\\"")))
        .chain(std::iter::once("monospace".to_string()))
        .collect::<Vec<_>>()
        .join(", ")
}

fn generate_session_id() -> String {
    let bytes: [u8; 16] = rand::random();
    let mut id = String::with_capacity(32);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(id, "{byte:02x}");
    }
    id
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

impl WebShareManager {
    async fn hub(&self, port: u16) -> io::Result<Arc<WebShareHub>> {
        let mut hubs = self.hubs.lock().await;
        if let Some(hub) = hubs.get(&port).and_then(Weak::upgrade)
            && !hub.closed.load(Ordering::Acquire)
        {
            return Ok(hub);
        }

        let hub = WebShareHub::bind(port).await?;
        hubs.insert(port, Arc::downgrade(&hub));
        Ok(hub)
    }
}

impl WebShareHub {
    async fn bind(port: u16) -> io::Result<Arc<Self>> {
        let listener = smol::net::TcpListener::bind(("0.0.0.0", port)).await?;
        let addr = listener.local_addr()?;
        let sessions = Arc::new(Mutex::new(HashMap::new()));
        let (shutdown_tx, shutdown_rx) = smol::channel::bounded(1);
        let hub = Arc::new(Self {
            addr,
            sessions: Arc::clone(&sessions),
            shutdown_tx,
            closed: AtomicBool::new(false),
        });

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
                let sessions = Arc::clone(&sessions);
                smol::spawn(async move {
                    let mut peek = [0; 2048];
                    let Ok(read) = stream.peek(&mut peek).await else {
                        return;
                    };
                    let request = String::from_utf8_lossy(&peek[..read]);
                    let is_websocket = request.to_ascii_lowercase().contains("upgrade: websocket");
                    let session = {
                        let sessions = sessions.lock().expect("web share sessions poisoned");
                        session_for_request(&request, &sessions)
                    };
                    let Some(session) = session else {
                        let _ = serve_http(stream, None).await;
                        return;
                    };
                    if is_websocket {
                        let _ = serve_websocket(stream, peer, session.state).await;
                    } else {
                        let _ = serve_http(stream, Some(&session.terminal_page)).await;
                    }
                })
                .detach();
            }
        })
        .detach();
        Ok(hub)
    }

    fn remove_session(&self, session_id: &str) {
        let empty = {
            let mut sessions = self.sessions.lock().expect("web share sessions poisoned");
            sessions.remove(session_id);
            sessions.is_empty()
        };
        if empty && !self.closed.swap(true, Ordering::AcqRel) {
            let _ = self.shutdown_tx.try_send(());
        }
    }
}

fn session_for_request(
    request: &str,
    sessions: &HashMap<String, HubSession>,
) -> Option<HubSession> {
    let path = request.lines().next()?.split_whitespace().nth(1)?;
    if let Some(session_id) = path
        .strip_prefix("/s/")
        .and_then(|path| path.split('/').next())
        && let Some(session) = sessions.get(session_id)
    {
        return Some(session.clone());
    }
    (sessions.len() == 1 && matches!(path, "/" | "/ws"))
        .then(|| sessions.values().next().cloned())
        .flatten()
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

    #[cfg(test)]
    pub async fn bind_with_theme(
        token: String,
        snapshot: gpui_term::TerminalScreen,
        theme: XtermTheme,
        tab_name: String,
    ) -> io::Result<Self> {
        Self::bind_with_manager(
            &WebShareManager::default(),
            0,
            token,
            snapshot,
            theme,
            tab_name,
        )
        .await
    }

    #[cfg(test)]
    pub async fn bind_with_manager(
        manager: &WebShareManager,
        port: u16,
        token: String,
        snapshot: gpui_term::TerminalScreen,
        theme: XtermTheme,
        tab_name: String,
    ) -> io::Result<Self> {
        Self::bind_with_manager_and_font(
            manager,
            port,
            token,
            snapshot,
            theme,
            xterm_font_family(&gpui_term::TerminalSettings::default()),
            tab_name,
        )
        .await
    }

    pub async fn bind_with_manager_and_font(
        manager: &WebShareManager,
        port: u16,
        token: String,
        snapshot: gpui_term::TerminalScreen,
        theme: XtermTheme,
        font_family: String,
        tab_name: String,
    ) -> io::Result<Self> {
        let hub = manager.hub(port).await?;
        let (control_tx, control_rx) = smol::channel::bounded(16);
        let (input_tx, input_rx) = smol::channel::bounded(256);
        let (snapshot_tx, snapshot_rx) = smol::channel::bounded(1);
        let state = Arc::new(Mutex::new(ServerState {
            access: ShareAccess::new(token),
            broadcast_screen: snapshot.clone(),
            snapshot,
            clients: HashMap::new(),
            control_tx,
            input_tx,
            last_activity: Instant::now(),
        }));
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
                    if let Some(client) = state.clients.remove(&client_id) {
                        let _ = client.socket.shutdown(std::net::Shutdown::Both);
                    }
                    state.access.disconnect(client_id);
                }
            }
        })
        .detach();
        let terminal_page: Arc<str> = TERMINAL_PAGE
            .replace("__XTERM_STYLE__", XTERM_STYLE)
            .replace("__XTERM_SCRIPT__", XTERM_SCRIPT)
            .replace("__TERMUA_STYLE__", TERMINAL_STYLE)
            .replace("__TERMUA_SCRIPT__", TERMINAL_SCRIPT)
            .replace(
                "__TERMUA_THEME__",
                &serde_json::to_string(&theme).expect("web terminal theme must serialize"),
            )
            .replace(
                "__TERMUA_FONT_FAMILY__",
                &serde_json::to_string(&font_family)
                    .expect("web terminal font family must serialize"),
            )
            .replace("__TERMUA_TAB_NAME__", &escape_html_text(&tab_name))
            .into();
        let session_id = generate_session_id();
        hub.sessions
            .lock()
            .expect("web share sessions poisoned")
            .insert(
                session_id.clone(),
                HubSession {
                    state: Arc::clone(&state),
                    terminal_page,
                },
            );
        Ok(Self {
            addr: hub.addr,
            session_id,
            hub,
            state,
            control_rx,
            input_rx,
            snapshot_tx,
            closed: AtomicBool::new(false),
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn session_path(&self) -> String {
        format!("/s/{}/", self.session_id)
    }

    pub fn shutdown(&self) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        let mut state = self.state.lock().expect("web share state poisoned");
        for client in state.clients.values() {
            let _ = client.socket.shutdown(std::net::Shutdown::Both);
        }
        state.clients.clear();
        drop(state);
        self.hub.remove_session(&self.session_id);
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

    pub fn revoke_control(&self) -> bool {
        let mut state = self.state.lock().expect("web share state poisoned");
        let revoked = state.access.revoke_control();
        if revoked {
            broadcast_access(&state);
        }
        revoked
    }

    pub fn update_snapshot(&self, snapshot: gpui_term::TerminalScreen) {
        let mut state = self.state.lock().expect("web share state poisoned");
        if state.snapshot == snapshot {
            return;
        }
        state.snapshot = snapshot;
        state.last_activity = Instant::now();
        drop(state);
        let _ = self.snapshot_tx.try_send(());
    }

    pub fn is_inactive_for(&self, timeout: Duration) -> bool {
        self.state
            .lock()
            .expect("web share state poisoned")
            .last_activity
            .elapsed()
            >= timeout
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }
}

impl Drop for WebShareServer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn access_message(control: bool) -> Message {
    Message::Text(format!(r#"{{"type":"access","control":{control}}}"#).into())
}

fn control_request_message(status: &str) -> Message {
    Message::Text(format!(r#"{{"type":"control_request","status":"{status}"}}"#).into())
}

fn screen_message(kind: u8, screen: &gpui_term::TerminalScreen, ansi: Vec<u8>) -> Message {
    let mut message = Vec::with_capacity(ansi.len() + 21 + screen.rows() * size_of::<u32>());
    message.push(kind);
    message.extend_from_slice(&(screen.columns() as u32).to_be_bytes());
    message.extend_from_slice(&(screen.rows() as u32).to_be_bytes());
    for line_number in screen.line_numbers() {
        let line_number = line_number
            .and_then(|number| u32::try_from(number).ok())
            .unwrap_or(0);
        message.extend_from_slice(&line_number.to_be_bytes());
    }
    let (selection_column, selection_row, selection_length) =
        screen.selection().unwrap_or_default();
    for value in [selection_column, selection_row, selection_length] {
        message.extend_from_slice(&u32::try_from(value).unwrap_or(u32::MAX).to_be_bytes());
    }
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
    let shutdown_socket = stream.clone();
    let socket = async_tungstenite::accept_async(stream).await?;
    let client_id = NEXT_CLIENT_ID.fetch_add(1, Ordering::Relaxed);
    let (out_tx, out_rx) = smol::channel::bounded::<Message>(64);
    let connection = ClientConnection {
        messages: out_tx.clone(),
        socket: shutdown_socket,
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
                if authenticated {
                    state.lock().expect("web share state poisoned").last_activity = Instant::now();
                }
                match message {
                    ClientMessage::Authenticate { token } if !authenticated => {
                        let result = {
                            let mut state = state.lock().expect("web share state poisoned");
                            let result = state.access.connect(&token, client_id);
                            if result.is_ok() {
                                state.last_activity = Instant::now();
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
                    ClientMessage::Activity if authenticated => {}
                    ClientMessage::RequestControl if authenticated => {
                        let result = {
                            let mut state = state.lock().expect("web share state poisoned");
                            state.access.request_control(client_id).map(|request_id| {
                                (state.control_tx.clone(), ControlRequest { request_id, client_id, peer })
                            })
                        };
                        match result {
                            Ok((tx, request)) => { let _ = tx.send(request).await; }
                            Err(AccessError::ControllerAlreadyActive) => {
                                let _ = out_tx.try_send(control_request_message("unavailable"));
                            }
                            Err(AccessError::RequestAlreadyPending) => {
                                let _ = out_tx.try_send(control_request_message("pending"));
                            }
                            Err(_) => {}
                        }
                    }
                    ClientMessage::ReleaseControl if authenticated => {
                        let mut state = state.lock().expect("web share state poisoned");
                        if state.access.release_control(client_id) {
                            broadcast_access(&state);
                        }
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

async fn serve_http(
    mut stream: smol::net::TcpStream,
    terminal_page: Option<&str>,
) -> io::Result<()> {
    let mut request = vec![0; 8192];
    let read = stream.read(&mut request).await?;
    let request = String::from_utf8_lossy(&request[..read]);
    let (status, content_type, body) = if request.starts_with("GET ") {
        terminal_page.map_or(
            ("404 Not Found", "text/plain; charset=utf-8", "Not Found"),
            |page| ("200 OK", "text/html; charset=utf-8", page),
        )
    } else {
        ("404 Not Found", "text/plain; charset=utf-8", "Not Found")
    };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: \
         close\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\n\r\n{body}",
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
        if self.controller == Some(client) {
            return Err(AccessError::ControllerAlreadyActive);
        }
        if !self.pending.is_empty() {
            return Err(AccessError::RequestAlreadyPending);
        }
        let request = self.next_request_id;
        self.next_request_id = self.next_request_id.wrapping_add(1).max(1);
        self.pending.insert(request, client);
        Ok(request)
    }

    pub fn approve(&mut self, request: ControlRequestId) -> Result<(), AccessError> {
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

    pub fn revoke_control(&mut self) -> bool {
        self.controller.take().is_some()
    }

    pub fn release_control(&mut self, client: ClientId) -> bool {
        if self.controller == Some(client) {
            self.controller = None;
            true
        } else {
            false
        }
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
    use gpui_term::{Cell, GridPoint, IndexedCell, SelectionRange, TerminalContent};
    use smol::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

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

    async fn connect_authenticated(
        server: &WebShareServer,
    ) -> async_tungstenite::WebSocketStream<smol::net::TcpStream> {
        let url = format!(
            "ws://127.0.0.1:{}{}ws",
            server.local_addr().port(),
            server.session_path()
        );
        let (mut socket, _) = async_tungstenite::smol::connect_async(url).await.unwrap();
        socket
            .send(Message::Text(
                r#"{"type":"authenticate","token":"secret"}"#.into(),
            ))
            .await
            .unwrap();
        let _ = socket.next().await;
        let _ = socket.next().await;
        socket
    }

    async fn websocket_disconnects(
        socket: &mut async_tungstenite::WebSocketStream<smol::net::TcpStream>,
        timeout: std::time::Duration,
    ) -> bool {
        let disconnected = async {
            while let Some(message) = socket.next().await {
                match message {
                    Ok(message) if message.is_close() => return true,
                    Err(_) => return true,
                    _ => {}
                }
            }
            true
        }
        .fuse();
        let timeout = futures::FutureExt::fuse(smol::Timer::after(timeout));
        futures::pin_mut!(disconnected, timeout);
        futures::select! {
            disconnected = disconnected => disconnected,
            _ = timeout => false,
        }
    }

    #[test]
    fn screen_message_reserves_a_line_number_for_each_app_row() {
        let screen = screen_with_text("screen");
        let Message::Binary(message) = screen_message(
            0,
            &screen,
            gpui_term::serialize_terminal_screen_ansi(&screen),
        ) else {
            panic!("screen update must be binary");
        };

        let line_number_bytes = screen.rows() * size_of::<u32>();
        assert_eq!(
            &message[9..9 + line_number_bytes],
            vec![0; line_number_bytes]
        );
        assert_eq!(message[21 + line_number_bytes], b'\x1b');
    }

    #[test]
    fn screen_message_contains_the_visible_app_selection() {
        let mut content = TerminalContent::default();
        content.terminal_bounds = gpui_term::TerminalBounds::new(
            gpui::px(10.),
            gpui::px(10.),
            gpui::Bounds {
                origin: gpui::Point::default(),
                size: gpui::size(gpui::px(50.), gpui::px(20.)),
            },
        );
        content.selection = Some(SelectionRange {
            start: GridPoint::new(0, 1),
            end: GridPoint::new(1, 2),
        });
        let screen = gpui_term::capture_terminal_screen(&content);
        let Message::Binary(message) = screen_message(0, &screen, Vec::new()) else {
            panic!("screen update must be binary");
        };

        let selection_offset = 9 + screen.rows() * size_of::<u32>();
        assert_eq!(
            &message[selection_offset..selection_offset + 12],
            &[0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 7]
        );
    }

    #[test]
    fn authenticated_clients_start_read_only() {
        let mut access = ShareAccess::new("secret".into());
        assert_eq!(access.connect("wrong", 1), Err(AccessError::InvalidToken));
        assert_eq!(access.connect("secret", 1), Ok(ClientAccess::ReadOnly));
        assert!(!access.can_input(1));
    }

    #[test]
    fn approving_later_request_transfers_exclusive_control() {
        let mut access = ShareAccess::new("secret".into());
        access.connect("secret", 1).unwrap();
        access.connect("secret", 2).unwrap();

        let request = access.request_control(1).unwrap();
        assert!(!access.can_input(1));
        assert_eq!(access.approve(request), Ok(()));
        assert!(access.can_input(1));

        let later_request = access.request_control(2).unwrap();
        assert!(access.can_input(1));
        assert!(!access.can_input(2));
        assert_eq!(access.approve(later_request), Ok(()));
        assert!(!access.can_input(1));
        assert!(access.can_input(2));
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
    fn revoking_control_returns_controller_to_read_only() {
        let mut access = ShareAccess::new("secret".into());
        access.connect("secret", 1).unwrap();
        let request = access.request_control(1).unwrap();
        access.approve(request).unwrap();
        assert!(access.can_input(1));

        assert!(access.revoke_control());
        assert!(!access.can_input(1));
        assert!(!access.revoke_control());
    }

    #[test]
    fn controller_can_release_only_its_own_control() {
        let mut access = ShareAccess::new("secret".into());
        access.connect("secret", 1).unwrap();
        access.connect("secret", 2).unwrap();
        let request = access.request_control(1).unwrap();
        access.approve(request).unwrap();

        assert!(!access.release_control(2));
        assert!(access.can_input(1));
        assert!(access.release_control(1));
        assert!(!access.can_input(1));
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
    fn only_one_control_request_can_be_pending() {
        let mut access = ShareAccess::new("secret".into());
        access.connect("secret", 1).unwrap();
        access.connect("secret", 2).unwrap();

        access.request_control(1).unwrap();

        assert_eq!(
            access.request_control(2),
            Err(AccessError::RequestAlreadyPending)
        );
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
            let request = format!(
                "GET {} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
                server.session_path()
            );
            stream.write_all(request.as_bytes()).await.unwrap();
            let mut response = String::new();
            stream.read_to_string(&mut response).await.unwrap();
            let compact_response: String = response
                .chars()
                .filter(|character| !character.is_ascii_whitespace())
                .collect();
            assert!(response.starts_with("HTTP/1.1 200 OK"));
            assert!(response.contains("xterm"));
            assert!(!response.contains("cdn.jsdelivr.net"));
            assert!(response.contains(r#"data-termua-asset="xterm.css""#));
            assert!(response.contains(r#"data-termua-asset="xterm.js""#));
            assert!(response.contains("Copyright (c) 2014 The xterm.js authors"));
            assert!(response.contains("request-control"));
            assert!(response.contains(r#"id="request-control" disabled"#));
            assert!(response.contains("Release control"));
            assert!(response.contains(r#"type: "release_control""#));
            assert!(response.contains(r#"<div id="line-numbers"></div>"#));
            assert!(response.contains("<title>bash &amp; &lt;tools&gt;</title>"));
            assert!(compact_response.contains("theme:{"));
            assert!(compact_response.contains("fontFamily:"));
            assert!(response.contains("Symbols Nerd Font Mono"));
            assert!(compact_response.contains("term.resize(columns,rows)"));
            assert!(compact_response.contains("renderLineNumbers(lineNumbers)"));
            assert!(compact_response.contains("lineNumbers.replaceChildren"));
            assert!(compact_response.contains("view.getUint32(9+row*4)"));
            assert!(compact_response.contains("bytes.slice(selectionOffset+12)"));
            assert!(
                compact_response
                    .contains("term.select(selectionColumn,selectionRow,selectionLength)")
            );
            assert!(compact_response.contains("ws.onclose="));
            assert!(compact_response.contains("term.clear()"));
            assert!(compact_response.contains("requestControl.disabled=true"));
            assert!(compact_response.contains("requestControl.disabled=false"));
            assert!(compact_response.contains(r#"m.type==="control_request""#));
            assert!(compact_response.contains("Controlisalreadyinuse"));
            assert!(compact_response.contains("bytes[0]===2"));
            assert!(compact_response.contains("justify-content:center"));
            assert!(compact_response.contains("background:#000"));
            assert!(compact_response.contains("term.options.fontSize=baseFontSize*scale"));
            assert!(!compact_response.contains("terminalFrame.style.zoom"));
            assert!(!compact_response.contains("transform=`scale(${scale})`"));
            assert!(compact_response.contains("Math.min(1.15,"));
            assert!(!compact_response.contains("Math.min(wantedWidth,bounds.width)"));
            assert!(compact_response.contains("scrollback:0"));
            assert!(compact_response.contains(".xterm-viewport{overflow-y:hidden!important"));
            assert!(compact_response.contains("event.preventDefault()"));
            assert!(compact_response.contains("event.stopImmediatePropagation()"));
            assert!(!compact_response.contains("term.onScroll("));
            assert!(!compact_response.contains(r#"type:"history""#));
            assert!(compact_response.contains(r#"type:"activity""#));
            assert!(compact_response.contains(r#"["pointerdown","wheel","touchstart"]"#));
            assert!(response.contains(r#"<style data-termua-asset="terminal.css">"#));
            assert!(response.contains(r#"<script data-termua-asset="terminal.js">"#));
            assert!(!response.contains("__TERMUA_"));
        });
    }

    #[test]
    fn manager_reuses_one_listener_for_multiple_terminal_sessions() {
        smol::block_on(async {
            let manager = WebShareManager::default();
            let first = WebShareServer::bind_with_manager(
                &manager,
                0,
                "first-token".into(),
                screen_with_text("first"),
                XtermTheme::default(),
                "first tab".into(),
            )
            .await
            .unwrap();
            let second = WebShareServer::bind_with_manager(
                &manager,
                0,
                "second-token".into(),
                screen_with_text("second"),
                XtermTheme::default(),
                "second tab".into(),
            )
            .await
            .unwrap();

            assert_eq!(first.local_addr(), second.local_addr());
            assert_ne!(first.session_path(), second.session_path());

            let mut stream = smol::net::TcpStream::connect(loopback_addr(&first))
                .await
                .unwrap();
            let request = format!(
                "GET {} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
                second.session_path()
            );
            stream.write_all(request.as_bytes()).await.unwrap();
            let mut response = String::new();
            stream.read_to_string(&mut response).await.unwrap();
            assert!(response.contains("<title>second tab</title>"));
            assert!(!response.contains("<title>first tab</title>"));

            first.shutdown();
            let mut stream = smol::net::TcpStream::connect(loopback_addr(&second))
                .await
                .expect("stopping one session must keep the shared listener alive");
            let request = format!(
                "GET {} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
                second.session_path()
            );
            stream.write_all(request.as_bytes()).await.unwrap();
            let mut response = String::new();
            stream.read_to_string(&mut response).await.unwrap();
            assert!(response.contains("<title>second tab</title>"));
        });
    }

    #[test]
    fn shared_listener_keeps_websocket_sessions_and_tokens_isolated() {
        smol::block_on(async {
            let manager = WebShareManager::default();
            let first = WebShareServer::bind_with_manager(
                &manager,
                0,
                "first-token".into(),
                screen_with_text("first"),
                XtermTheme::default(),
                "first".into(),
            )
            .await
            .unwrap();
            let second = WebShareServer::bind_with_manager(
                &manager,
                0,
                "second-token".into(),
                screen_with_text("second"),
                XtermTheme::default(),
                "second".into(),
            )
            .await
            .unwrap();

            let first_url = format!(
                "ws://127.0.0.1:{}{}ws",
                first.local_addr().port(),
                first.session_path()
            );
            let (mut wrong_session, _) = async_tungstenite::smol::connect_async(&first_url)
                .await
                .unwrap();
            wrong_session
                .send(Message::Text(
                    r#"{"type":"authenticate","token":"second-token"}"#.into(),
                ))
                .await
                .unwrap();
            assert!(
                websocket_disconnects(&mut wrong_session, std::time::Duration::from_millis(300))
                    .await,
                "a token from another session must not authenticate"
            );

            let (mut first_socket, _) = async_tungstenite::smol::connect_async(first_url)
                .await
                .unwrap();
            first_socket
                .send(Message::Text(
                    r#"{"type":"authenticate","token":"first-token"}"#.into(),
                ))
                .await
                .unwrap();
            let _access = first_socket.next().await.unwrap().unwrap();
            let first_screen = first_socket.next().await.unwrap().unwrap().into_data();
            assert!(
                first_screen
                    .windows(b"first".len())
                    .any(|bytes| bytes == b"first")
            );

            let second_url = format!(
                "ws://127.0.0.1:{}{}ws",
                second.local_addr().port(),
                second.session_path()
            );
            let (mut second_socket, _) = async_tungstenite::smol::connect_async(second_url)
                .await
                .unwrap();
            second_socket
                .send(Message::Text(
                    r#"{"type":"authenticate","token":"second-token"}"#.into(),
                ))
                .await
                .unwrap();
            let _access = second_socket.next().await.unwrap().unwrap();
            let second_screen = second_socket.next().await.unwrap().unwrap().into_data();
            assert!(
                second_screen
                    .windows(b"second".len())
                    .any(|bytes| bytes == b"second")
            );
        });
    }

    #[test]
    fn manager_uses_requested_port_for_new_hubs() {
        smol::block_on(async {
            let reserve_port = || {
                let listener = std::net::TcpListener::bind(("0.0.0.0", 0)).unwrap();
                let port = listener.local_addr().unwrap().port();
                drop(listener);
                port
            };
            let first_port = reserve_port();
            let mut second_port = reserve_port();
            while second_port == first_port {
                second_port = reserve_port();
            }

            let manager = WebShareManager::default();
            let first = WebShareServer::bind_with_manager(
                &manager,
                first_port,
                "first-token".into(),
                screen_with_text("first"),
                XtermTheme::default(),
                "first".into(),
            )
            .await
            .unwrap();
            let second = WebShareServer::bind_with_manager(
                &manager,
                second_port,
                "second-token".into(),
                screen_with_text("second"),
                XtermTheme::default(),
                "second".into(),
            )
            .await
            .unwrap();

            assert_eq!(first.local_addr().port(), first_port);
            assert_eq!(second.local_addr().port(), second_port);
        });
    }

    #[test]
    fn manager_reports_requested_port_conflicts_without_random_fallback() {
        smol::block_on(async {
            let occupied = std::net::TcpListener::bind(("0.0.0.0", 0)).unwrap();
            let port = occupied.local_addr().unwrap().port();
            let manager = WebShareManager::default();

            let result = WebShareServer::bind_with_manager(
                &manager,
                port,
                "secret".into(),
                screen_with_text("screen"),
                XtermTheme::default(),
                "tab".into(),
            )
            .await;
            let Err(error) = result else {
                panic!("an occupied configured port must not fall back to a random port");
            };

            assert_eq!(error.kind(), io::ErrorKind::AddrInUse);
        });
    }

    #[test]
    fn terminal_updates_refresh_web_share_activity() {
        smol::block_on(async {
            let server = WebShareServer::bind("secret".into(), screen_with_text("before"))
                .await
                .unwrap();
            let idle_timeout = std::time::Duration::from_millis(10);

            smol::Timer::after(std::time::Duration::from_millis(20)).await;
            assert!(server.is_inactive_for(idle_timeout));

            server.update_snapshot(screen_with_text("after"));
            assert!(!server.is_inactive_for(idle_timeout));
        });
    }

    #[test]
    fn identical_terminal_updates_do_not_refresh_web_share_activity() {
        smol::block_on(async {
            let server = WebShareServer::bind("secret".into(), screen_with_text("same"))
                .await
                .unwrap();
            let idle_timeout = Duration::from_millis(10);

            smol::Timer::after(Duration::from_millis(20)).await;
            server.update_snapshot(screen_with_text("same"));

            assert!(server.is_inactive_for(idle_timeout));
        });
    }

    #[test]
    fn authenticated_web_activity_refreshes_idle_timeout() {
        smol::block_on(async {
            let server = WebShareServer::bind("secret".into(), screen_with_text("screen"))
                .await
                .unwrap();
            let mut socket = connect_authenticated(&server).await;
            server.state.lock().unwrap().last_activity = Instant::now() - Duration::from_secs(1);
            assert!(server.is_inactive_for(Duration::from_millis(500)));

            socket
                .send(Message::Text(r#"{"type":"activity"}"#.into()))
                .await
                .unwrap();
            smol::Timer::after(Duration::from_millis(10)).await;

            assert!(!server.is_inactive_for(Duration::from_millis(500)));
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
    fn shutting_down_share_closes_connected_websocket() {
        smol::block_on(async {
            let server = WebShareServer::bind("secret".into(), screen_with_text("screen"))
                .await
                .unwrap();
            let mut socket = connect_authenticated(&server).await;

            server.shutdown();
            assert!(
                websocket_disconnects(&mut socket, std::time::Duration::from_millis(300)).await
            );
        });
    }

    #[test]
    fn shutting_down_share_closes_websocket_when_output_queue_is_full() {
        smol::block_on(async {
            let server = WebShareServer::bind("secret".into(), screen_with_text("screen"))
                .await
                .unwrap();
            let mut socket = connect_authenticated(&server).await;

            let messages = server
                .state
                .lock()
                .unwrap()
                .clients
                .values()
                .next()
                .unwrap()
                .messages
                .clone();
            let payload = "x".repeat(256 * 1024);
            while messages
                .try_send(Message::Text(payload.clone().into()))
                .is_ok()
            {}

            server.shutdown();
            assert!(
                websocket_disconnects(&mut socket, std::time::Duration::from_millis(300)).await,
                "shutdown must bypass a full output queue"
            );
        });
    }

    #[test]
    fn full_output_queue_disconnects_websocket_before_removing_client() {
        smol::block_on(async {
            let server = WebShareServer::bind("secret".into(), screen_with_text("screen"))
                .await
                .unwrap();
            let mut socket = connect_authenticated(&server).await;

            let messages = server
                .state
                .lock()
                .unwrap()
                .clients
                .values()
                .next()
                .unwrap()
                .messages
                .clone();
            let payload = "x".repeat(256 * 1024);
            for attempt in 0..20 {
                while messages
                    .try_send(Message::Text(payload.clone().into()))
                    .is_ok()
                {}
                server.update_snapshot(screen_with_text(&format!("screen-{attempt}")));
                smol::Timer::after(std::time::Duration::from_millis(20)).await;
                if server.state.lock().unwrap().clients.is_empty() {
                    break;
                }
            }
            assert!(
                server.state.lock().unwrap().clients.is_empty(),
                "a client with a full output queue must be removed"
            );

            assert!(
                websocket_disconnects(&mut socket, std::time::Duration::from_millis(500)).await,
                "removing a slow client must disconnect its websocket"
            );
        });
    }

    #[test]
    fn websocket_input_is_blocked_until_desktop_approval() {
        smol::block_on(async {
            let server = WebShareServer::bind("secret".into(), screen_with_text("screen"))
                .await
                .unwrap();
            let url = format!(
                "ws://127.0.0.1:{}{}ws",
                server.local_addr().port(),
                server.session_path()
            );
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
    fn approving_later_web_request_transfers_control() {
        smol::block_on(async {
            let server = WebShareServer::bind("secret".into(), screen_with_text("screen"))
                .await
                .unwrap();
            let mut controller = connect_authenticated(&server).await;
            let mut viewer = connect_authenticated(&server).await;

            controller
                .send(Message::Text(r#"{"type":"request_control"}"#.into()))
                .await
                .unwrap();
            let request = server.control_requests().recv().await.unwrap();
            server.approve_control(request.request_id).unwrap();
            assert!(
                controller
                    .next()
                    .await
                    .unwrap()
                    .unwrap()
                    .into_text()
                    .unwrap()
                    .contains(r#""control":true"#)
            );
            assert!(
                viewer
                    .next()
                    .await
                    .unwrap()
                    .unwrap()
                    .into_text()
                    .unwrap()
                    .contains(r#""control":false"#)
            );

            viewer
                .send(Message::Text(r#"{"type":"request_control"}"#.into()))
                .await
                .unwrap();
            let control_requests = server.control_requests();
            let later_request = control_requests.recv().fuse();
            let timeout =
                futures::FutureExt::fuse(smol::Timer::after(std::time::Duration::from_millis(100)));
            futures::pin_mut!(later_request, timeout);
            let later_request = futures::select! {
                request = later_request => request.ok(),
                _ = timeout => None,
            }
            .expect("later control request must reach the desktop for approval");
            server.approve_control(later_request.request_id).unwrap();

            let previous_access = controller
                .next()
                .await
                .unwrap()
                .unwrap()
                .into_text()
                .unwrap();
            let later_access = viewer.next().await.unwrap().unwrap().into_text().unwrap();
            assert!(previous_access.contains(r#""control":false"#));
            assert!(later_access.contains(r#""control":true"#));
        });
    }

    #[test]
    fn revoking_control_notifies_the_controller_and_blocks_input() {
        smol::block_on(async {
            let server = WebShareServer::bind("secret".into(), screen_with_text("screen"))
                .await
                .unwrap();
            let mut controller = connect_authenticated(&server).await;
            controller
                .send(Message::Text(r#"{"type":"request_control"}"#.into()))
                .await
                .unwrap();
            let request = server.control_requests().recv().await.unwrap();
            server.approve_control(request.request_id).unwrap();
            let granted = controller
                .next()
                .await
                .unwrap()
                .unwrap()
                .into_text()
                .unwrap();
            assert!(granted.contains(r#""control":true"#));

            assert!(server.revoke_control());
            let revoked = controller
                .next()
                .await
                .unwrap()
                .unwrap()
                .into_text()
                .unwrap();
            assert!(revoked.contains(r#""control":false"#));
            controller
                .send(Message::Text(
                    r#"{"type":"input","data":"blocked-again"}"#.into(),
                ))
                .await
                .unwrap();
            smol::Timer::after(std::time::Duration::from_millis(20)).await;
            assert!(server.inputs().try_recv().is_err());
        });
    }

    #[test]
    fn controller_can_release_control_from_the_websocket() {
        smol::block_on(async {
            let server = WebShareServer::bind("secret".into(), screen_with_text("screen"))
                .await
                .unwrap();
            let mut controller = connect_authenticated(&server).await;
            controller
                .send(Message::Text(r#"{"type":"request_control"}"#.into()))
                .await
                .unwrap();
            let request = server.control_requests().recv().await.unwrap();
            server.approve_control(request.request_id).unwrap();
            let granted = controller
                .next()
                .await
                .unwrap()
                .unwrap()
                .into_text()
                .unwrap();
            assert!(granted.contains(r#""control":true"#));

            controller
                .send(Message::Text(r#"{"type":"release_control"}"#.into()))
                .await
                .unwrap();
            let response = controller.next().fuse();
            let timeout =
                futures::FutureExt::fuse(smol::Timer::after(std::time::Duration::from_millis(100)));
            futures::pin_mut!(response, timeout);
            let released = futures::select! {
                response = response => response
                    .and_then(Result::ok)
                    .and_then(|message| message.into_text().ok()),
                _ = timeout => None,
            };
            assert!(
                released
                    .as_deref()
                    .is_some_and(|message| message.contains(r#""control":false"#))
            );
        });
    }

    #[test]
    fn rapid_snapshots_do_not_revoke_client_access() {
        smol::block_on(async {
            let server = WebShareServer::bind("secret".into(), screen_with_text("screen"))
                .await
                .unwrap();
            let url = format!(
                "ws://127.0.0.1:{}{}ws",
                server.local_addr().port(),
                server.session_path()
            );
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
