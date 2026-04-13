use std::{collections::HashMap, net::SocketAddr, time::Duration};

use anyhow::Context as _;
use axum::{
    Router,
    extract::{
        State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response::IntoResponse,
    routing::get,
};
use futures::{SinkExt as _, StreamExt as _};

use crate::{
    protocol::{ClientToRelay, RelayToClient},
    state::{ClientTx, ConnId, RelayState, Room},
};

const DEFAULT_TTL: Duration = Duration::from_secs(30 * 60);
const SWEEP_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy)]
pub struct ServerConfig {
    pub gate_input: bool,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self { gate_input: true }
    }
}

pub async fn serve(listen: SocketAddr, config: ServerConfig) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .with_context(|| format!("bind {listen}"))?;
    serve_with_listener(listener, config).await
}

pub fn serve_blocking(listen: SocketAddr, config: ServerConfig) -> anyhow::Result<()> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("build tokio runtime")?
        .block_on(async move { serve(listen, config).await })
}

pub async fn serve_with_listener(
    listener: tokio::net::TcpListener,
    config: ServerConfig,
) -> anyhow::Result<()> {
    let state = RelayState::default();

    // Best-effort TTL sweep.
    {
        let state = state.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(SWEEP_INTERVAL);
            loop {
                tick.tick().await;
                let expired = state.remove_expired_rooms().await;
                if expired.is_empty() {
                    continue;
                }
                let msg = RelayToClient::Error {
                    code: "room_expired".to_string(),
                    message: "room expired".to_string(),
                };
                for (_room_id, viewers) in expired {
                    for (_id, tx) in viewers {
                        let _ = send_json(&tx, &msg);
                    }
                }
            }
        });
    }

    let app = Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/ws", get(ws_route))
        .with_state((state, config));

    axum::serve(listener, app).await.context("axum serve")?;
    Ok(())
}

async fn ws_route(
    ws: WebSocketUpgrade,
    State((state, config)): State<(RelayState, ServerConfig)>,
) -> impl IntoResponse {
    let conn_id = state.alloc_conn_id();
    ws.on_upgrade(move |socket| handle_socket(state, config, conn_id, socket))
}

#[derive(Debug, Clone)]
enum Role {
    Host { room_id: String },
    Viewer { room_id: String, viewer_id: String },
}

async fn handle_socket(
    state: RelayState,
    config: ServerConfig,
    conn_id: ConnId,
    socket: WebSocket,
) {
    let (mut ws_tx, mut ws_rx) = socket.split();
    let (out_tx, mut out_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    let send_task = tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            if ws_tx.send(Message::Text(msg.into())).await.is_err() {
                break;
            }
        }
    });

    let mut role: Option<Role> = None;

    while let Some(Ok(msg)) = ws_rx.next().await {
        let Message::Text(text) = msg else {
            continue;
        };

        let parsed: ClientToRelay = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(e) => {
                let _ = send_json(
                    &out_tx,
                    &RelayToClient::Error {
                        code: "bad_json".to_string(),
                        message: e.to_string(),
                    },
                );
                continue;
            }
        };

        if role.is_none() {
            match parsed {
                ClientToRelay::Register {
                    room_id,
                    join_key,
                    ttl_secs,
                } => {
                    let ttl = ttl_secs.map(Duration::from_secs).unwrap_or(DEFAULT_TTL);

                    let inserted = state
                        .with_rooms_mut(|rooms| {
                            if rooms.contains_key(&room_id) {
                                return false;
                            }
                            rooms.insert(
                                room_id.clone(),
                                Room::new(&room_id, conn_id, out_tx.clone(), &join_key, ttl),
                            );
                            true
                        })
                        .await;

                    if !inserted {
                        let _ = send_json(
                            &out_tx,
                            &RelayToClient::Error {
                                code: "room_exists".to_string(),
                                message: "room already exists".to_string(),
                            },
                        );
                        break;
                    }

                    role = Some(Role::Host { room_id });
                    let _ = send_json(&out_tx, &RelayToClient::Ok);
                    continue;
                }
                ClientToRelay::Join { room_id, join_key } => {
                    let viewer_id = conn_id.to_string();
                    let room_id_for_role = room_id.clone();
                    let mut last_snapshot: Option<(u64, serde_json::Value)> = None;
                    let mut last_selection: Option<(u64, serde_json::Value)> = None;
                    let join_ok = state
                        .with_rooms_mut(|rooms| {
                            let Some(room) = rooms.get_mut(&room_id) else {
                                return JoinResult::NoSuchRoom;
                            };
                            if room.is_expired() {
                                return JoinResult::Expired;
                            }
                            if !room.join_key_hash.matches(&join_key) {
                                return JoinResult::Denied;
                            }
                            room.viewers.insert(conn_id, out_tx.clone());
                            last_snapshot = room.last_snapshot.clone();
                            last_selection = room.last_selection.clone();
                            JoinResult::Joined
                        })
                        .await;

                    match join_ok {
                        JoinResult::Joined => {
                            role = Some(Role::Viewer {
                                room_id: room_id_for_role.clone(),
                                viewer_id: viewer_id.clone(),
                            });
                            let _ =
                                send_json(&out_tx, &RelayToClient::Joined { room_id, viewer_id });
                            if let Some((seq, payload)) = last_snapshot {
                                let _ = send_json(
                                    &out_tx,
                                    &RelayToClient::Snapshot {
                                        room_id: room_id_for_role.clone(),
                                        seq,
                                        payload,
                                    },
                                );
                            }
                            if let Some((seq, payload)) = last_selection {
                                let _ = send_json(
                                    &out_tx,
                                    &RelayToClient::Selection {
                                        room_id: room_id_for_role.clone(),
                                        seq,
                                        payload,
                                    },
                                );
                            }
                            continue;
                        }
                        JoinResult::NoSuchRoom => {
                            let _ = send_json(
                                &out_tx,
                                &RelayToClient::Error {
                                    code: "no_such_room".to_string(),
                                    message: "room not found".to_string(),
                                },
                            );
                            break;
                        }
                        JoinResult::Expired => {
                            let _ = send_json(
                                &out_tx,
                                &RelayToClient::Error {
                                    code: "room_expired".to_string(),
                                    message: "room expired".to_string(),
                                },
                            );
                            break;
                        }
                        JoinResult::Denied => {
                            let _ = send_json(
                                &out_tx,
                                &RelayToClient::Error {
                                    code: "join_denied".to_string(),
                                    message: "join denied".to_string(),
                                },
                            );
                            break;
                        }
                    }
                }
                _ => {
                    let _ = send_json(
                        &out_tx,
                        &RelayToClient::Error {
                            code: "unauthenticated".to_string(),
                            message: "first message must be register or join".to_string(),
                        },
                    );
                    break;
                }
            }
        }

        match parsed {
            ClientToRelay::Ping => {
                if let Some(Role::Host { room_id }) = role.as_ref() {
                    let _ = state
                        .with_rooms_mut(|rooms| {
                            if let Some(room) = rooms.get_mut(room_id) {
                                room.refresh_ttl();
                            }
                        })
                        .await;
                }
                let _ = send_json(&out_tx, &RelayToClient::Pong);
            }
            ClientToRelay::Snapshot {
                room_id,
                seq,
                payload,
            } => {
                let is_host =
                    matches!(role.as_ref(), Some(Role::Host { room_id: rid }) if rid == &room_id);
                if !is_host {
                    let _ = send_json(
                        &out_tx,
                        &RelayToClient::Error {
                            code: "forbidden".to_string(),
                            message: "only host can send snapshot".to_string(),
                        },
                    );
                    continue;
                }
                let msg = RelayToClient::Snapshot {
                    room_id: room_id.clone(),
                    seq,
                    payload: payload.clone(),
                };
                let _ = state
                    .with_rooms_mut(|rooms| {
                        if let Some(room) = rooms.get_mut(&room_id) {
                            room.last_snapshot = Some((seq, payload));
                        }
                    })
                    .await;
                broadcast_to_viewers(&state, &room_id, &msg).await;
            }
            ClientToRelay::Frame {
                room_id,
                seq,
                payload,
            } => {
                let is_host =
                    matches!(role.as_ref(), Some(Role::Host { room_id: rid }) if rid == &room_id);
                if !is_host {
                    let _ = send_json(
                        &out_tx,
                        &RelayToClient::Error {
                            code: "forbidden".to_string(),
                            message: "only host can send frame".to_string(),
                        },
                    );
                    continue;
                }
                let msg = RelayToClient::Frame {
                    room_id: room_id.clone(),
                    seq,
                    payload: payload.clone(),
                };
                let _ = state
                    .with_rooms_mut(|rooms| {
                        if let Some(room) = rooms.get_mut(&room_id) {
                            room.last_snapshot = Some((seq, payload));
                        }
                    })
                    .await;
                broadcast_to_viewers(&state, &room_id, &msg).await;
            }
            ClientToRelay::Selection {
                room_id,
                seq,
                payload,
            } => {
                let is_host =
                    matches!(role.as_ref(), Some(Role::Host { room_id: rid }) if rid == &room_id);
                if !is_host {
                    let _ = send_json(
                        &out_tx,
                        &RelayToClient::Error {
                            code: "forbidden".to_string(),
                            message: "only host can send selection".to_string(),
                        },
                    );
                    continue;
                }
                let msg = RelayToClient::Selection {
                    room_id: room_id.clone(),
                    seq,
                    payload: payload.clone(),
                };
                let _ = state
                    .with_rooms_mut(|rooms| {
                        if let Some(room) = rooms.get_mut(&room_id) {
                            room.last_selection = Some((seq, payload));
                        }
                    })
                    .await;
                broadcast_to_viewers(&state, &room_id, &msg).await;
            }
            ClientToRelay::Request {
                room_id,
                viewer_id,
                viewer_label,
            } => {
                let Some(Role::Viewer {
                    room_id: rid,
                    viewer_id: vid,
                }) = role.as_ref()
                else {
                    let _ = send_json(
                        &out_tx,
                        &RelayToClient::Error {
                            code: "forbidden".to_string(),
                            message: "only viewers can request control".to_string(),
                        },
                    );
                    continue;
                };
                if rid != &room_id || vid != &viewer_id {
                    let _ = send_json(
                        &out_tx,
                        &RelayToClient::Error {
                            code: "bad_viewer_id".to_string(),
                            message: "viewer_id mismatch".to_string(),
                        },
                    );
                    continue;
                }

                let mut host_tx: Option<ClientTx> = None;
                let denied = state
                    .with_rooms_mut(|rooms| {
                        let Some(room) = rooms.get_mut(&room_id) else {
                            return Some("no_such_room".to_string());
                        };
                        if room.is_expired() {
                            return Some("room_expired".to_string());
                        }
                        if room.controller_id.is_some() || room.pending_request {
                            return Some("busy".to_string());
                        }
                        if !room.viewers.contains_key(&conn_id) {
                            return Some("not_joined".to_string());
                        }
                        room.pending_request = true;
                        host_tx = Some(room.host_tx.clone());
                        None
                    })
                    .await;

                if let Some(reason) = denied {
                    let _ = send_json(&out_tx, &RelayToClient::CtrlDenied { room_id, reason });
                    continue;
                }

                if let Some(tx) = host_tx {
                    let _ = send_json(
                        &tx,
                        &RelayToClient::CtrlRequest {
                            room_id,
                            viewer_id,
                            viewer_label,
                        },
                    );
                }
            }
            ClientToRelay::Release { room_id, viewer_id } => {
                let Some(Role::Viewer {
                    room_id: rid,
                    viewer_id: vid,
                }) = role.as_ref()
                else {
                    let _ = send_json(
                        &out_tx,
                        &RelayToClient::Error {
                            code: "forbidden".to_string(),
                            message: "only viewers can release control".to_string(),
                        },
                    );
                    continue;
                };
                if rid != &room_id || vid != &viewer_id {
                    let _ = send_json(
                        &out_tx,
                        &RelayToClient::Error {
                            code: "bad_viewer_id".to_string(),
                            message: "viewer_id mismatch".to_string(),
                        },
                    );
                    continue;
                }

                // Release is voluntary: clear the controller immediately to avoid "busy" races.
                // Also notify both host and viewers (idempotent if already released).
                let (host_tx, released) = state
                    .with_rooms_mut(|rooms| {
                        let Some(room) = rooms.get_mut(&room_id) else {
                            return (None, false);
                        };
                        let released = room.release_control_if_controller(&viewer_id);
                        (Some(room.host_tx.clone()), released)
                    })
                    .await;

                if released {
                    let msg = RelayToClient::CtrlReleased {
                        room_id: room_id.clone(),
                        viewer_id: viewer_id.clone(),
                    };
                    if let Some(tx) = host_tx {
                        let _ = send_json(&tx, &msg);
                    }
                    broadcast_to_viewers(&state, &room_id, &msg).await;
                } else {
                    // Not the current controller; forward to host for best-effort handling.
                    if let Some(tx) = host_tx {
                        let _ = send_json(&tx, &RelayToClient::CtrlRelease { room_id, viewer_id });
                    }
                }
            }
            ClientToRelay::Granted { room_id, viewer_id } => {
                let is_host =
                    matches!(role.as_ref(), Some(Role::Host { room_id: rid }) if rid == &room_id);
                if !is_host {
                    let _ = send_json(
                        &out_tx,
                        &RelayToClient::Error {
                            code: "forbidden".to_string(),
                            message: "only host can grant control".to_string(),
                        },
                    );
                    continue;
                }
                state
                    .with_rooms_mut(|rooms| {
                        let Some(room) = rooms.get_mut(&room_id) else {
                            return;
                        };
                        room.controller_id = Some(viewer_id.clone());
                        room.pending_request = false;
                    })
                    .await;
                let msg = RelayToClient::CtrlGranted {
                    room_id: room_id.clone(),
                    viewer_id,
                };
                broadcast_to_viewers(&state, &room_id, &msg).await;
            }
            ClientToRelay::Denied {
                room_id,
                viewer_id,
                reason,
            } => {
                let is_host =
                    matches!(role.as_ref(), Some(Role::Host { room_id: rid }) if rid == &room_id);
                if !is_host {
                    let _ = send_json(
                        &out_tx,
                        &RelayToClient::Error {
                            code: "forbidden".to_string(),
                            message: "only host can deny control".to_string(),
                        },
                    );
                    continue;
                }
                state
                    .with_rooms_mut(|rooms| {
                        let Some(room) = rooms.get_mut(&room_id) else {
                            return;
                        };
                        room.pending_request = false;
                    })
                    .await;
                let msg = RelayToClient::CtrlDenied {
                    room_id: room_id.clone(),
                    reason,
                };
                send_to_viewer(&state, &room_id, &viewer_id, &msg).await;
            }
            ClientToRelay::Released { room_id, viewer_id } => {
                let is_host =
                    matches!(role.as_ref(), Some(Role::Host { room_id: rid }) if rid == &room_id);
                if !is_host {
                    let _ = send_json(
                        &out_tx,
                        &RelayToClient::Error {
                            code: "forbidden".to_string(),
                            message: "only host can release control".to_string(),
                        },
                    );
                    continue;
                }
                let changed = state
                    .with_rooms_mut(|rooms| {
                        let Some(room) = rooms.get_mut(&room_id) else {
                            return false;
                        };
                        room.release_control_if_controller(&viewer_id)
                    })
                    .await;
                if !changed {
                    continue;
                }
                let msg = RelayToClient::CtrlReleased {
                    room_id: room_id.clone(),
                    viewer_id,
                };
                broadcast_to_viewers(&state, &room_id, &msg).await;
            }
            ClientToRelay::Revoked { room_id } => {
                let is_host =
                    matches!(role.as_ref(), Some(Role::Host { room_id: rid }) if rid == &room_id);
                if !is_host {
                    let _ = send_json(
                        &out_tx,
                        &RelayToClient::Error {
                            code: "forbidden".to_string(),
                            message: "only host can revoke control".to_string(),
                        },
                    );
                    continue;
                }
                state
                    .with_rooms_mut(|rooms| {
                        let Some(room) = rooms.get_mut(&room_id) else {
                            return;
                        };
                        room.controller_id = None;
                        room.pending_request = false;
                    })
                    .await;
                let msg = RelayToClient::CtrlRevoked {
                    room_id: room_id.clone(),
                };
                broadcast_to_viewers(&state, &room_id, &msg).await;
            }
            ClientToRelay::InputEvent {
                room_id,
                viewer_id,
                payload,
            } => {
                let Some(Role::Viewer {
                    room_id: rid,
                    viewer_id: vid,
                }) = role.as_ref()
                else {
                    let _ = send_json(
                        &out_tx,
                        &RelayToClient::Error {
                            code: "forbidden".to_string(),
                            message: "only viewers can send input".to_string(),
                        },
                    );
                    continue;
                };
                if rid != &room_id || vid != &viewer_id {
                    let _ = send_json(
                        &out_tx,
                        &RelayToClient::Error {
                            code: "bad_viewer_id".to_string(),
                            message: "viewer_id mismatch".to_string(),
                        },
                    );
                    continue;
                }

                let (host_tx, controller) = state
                    .with_rooms_mut(|rooms| {
                        let Some(room) = rooms.get_mut(&room_id) else {
                            return (None, None);
                        };
                        (Some(room.host_tx.clone()), room.controller_id.clone())
                    })
                    .await;

                if config.gate_input && controller.as_deref() != Some(&viewer_id) {
                    let _ = send_json(
                        &out_tx,
                        &RelayToClient::Error {
                            code: "not_controller".to_string(),
                            message: "viewer does not hold control".to_string(),
                        },
                    );
                    continue;
                }

                if let Some(tx) = host_tx {
                    let _ = send_json(
                        &tx,
                        &RelayToClient::InputEvent {
                            room_id,
                            viewer_id,
                            payload,
                        },
                    );
                }
            }
            ClientToRelay::Stop { room_id } => {
                let is_host =
                    matches!(role.as_ref(), Some(Role::Host { room_id: rid }) if rid == &room_id);
                if !is_host {
                    let _ = send_json(
                        &out_tx,
                        &RelayToClient::Error {
                            code: "forbidden".to_string(),
                            message: "only host can stop".to_string(),
                        },
                    );
                    continue;
                }
                close_room(&state, &room_id, "room_closed", "host stopped sharing").await;
                break;
            }
            _ => {
                // Other message types are ignored for v0.
            }
        }
    }

    if let Some(role) = role {
        match role {
            Role::Host { room_id } => {
                close_room(&state, &room_id, "room_closed", "host disconnected").await;
            }
            Role::Viewer { room_id, viewer_id } => {
                state
                    .with_rooms_mut(|rooms| {
                        let Some(room) = rooms.get_mut(&room_id) else {
                            return;
                        };
                        room.viewers.remove(&conn_id);
                        if room.controller_id.as_deref() == Some(&viewer_id) {
                            room.controller_id = None;
                            room.pending_request = false;
                        }
                    })
                    .await;
            }
        }
    }

    send_task.abort();
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JoinResult {
    Joined,
    NoSuchRoom,
    Expired,
    Denied,
}

async fn close_room(state: &RelayState, room_id: &str, code: &str, message: &str) {
    let viewers: HashMap<ConnId, ClientTx> = state
        .with_rooms_mut(|rooms| {
            rooms
                .remove(room_id)
                .map(|room| room.viewers)
                .unwrap_or_default()
        })
        .await;

    let msg = RelayToClient::Error {
        code: code.to_string(),
        message: message.to_string(),
    };
    let Ok(text) = serialize_json(&msg) else {
        return;
    };
    for (_id, tx) in viewers {
        let _ = send_text(&tx, &text);
    }
}

fn serialize_json(msg: &RelayToClient) -> anyhow::Result<String> {
    Ok(serde_json::to_string(msg)?)
}

fn send_text(tx: &ClientTx, text: &str) -> anyhow::Result<()> {
    let _ = tx.send(text.to_string());
    Ok(())
}

fn send_json(tx: &ClientTx, msg: &RelayToClient) -> anyhow::Result<()> {
    let text = serialize_json(msg)?;
    let _ = tx.send(text);
    Ok(())
}

async fn broadcast_to_viewers(state: &RelayState, room_id: &str, msg: &RelayToClient) {
    let viewers: HashMap<ConnId, ClientTx> = state
        .with_rooms(|rooms| {
            rooms
                .get(room_id)
                .map(|room| room.viewers.clone())
                .unwrap_or_default()
        })
        .await;
    let Ok(text) = serialize_json(msg) else {
        return;
    };
    for (_id, tx) in viewers {
        let _ = send_text(&tx, &text);
    }
}

async fn send_to_viewer(state: &RelayState, room_id: &str, viewer_id: &str, msg: &RelayToClient) {
    let viewer_conn: Option<ConnId> = viewer_id.parse::<u64>().ok();
    let tx_opt = state
        .with_rooms(|rooms| {
            let room = rooms.get(room_id)?;
            let conn = viewer_conn?;
            room.viewers.get(&conn).cloned()
        })
        .await;
    if let Some(tx) = tx_opt {
        let _ = send_json(&tx, msg);
    }
}
