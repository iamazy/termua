use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use sha2::{Digest as _, Sha256};
use tokio::sync::{RwLock, mpsc};

pub type ConnId = u64;
pub type ClientTx = mpsc::UnboundedSender<String>;

#[derive(Clone, Copy)]
pub struct JoinKeyHash([u8; 32]);

impl JoinKeyHash {
    pub fn new(join_key: &str) -> Self {
        let mut h = Sha256::new();
        h.update(join_key.as_bytes());
        let out = h.finalize();
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&out);
        Self(bytes)
    }

    pub fn matches(&self, join_key: &str) -> bool {
        let other = Self::new(join_key);
        constant_time_eq(&self.0, &other.0)
    }
}

pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

pub struct Room {
    pub join_key_hash: JoinKeyHash,
    pub host_conn: ConnId,
    pub host_tx: ClientTx,
    pub viewers: HashMap<ConnId, ClientTx>,
    pub controller_id: Option<String>,
    pub pending_request: bool,
    pub last_snapshot: Option<(u64, serde_json::Value)>,
    pub last_selection: Option<(u64, serde_json::Value)>,
    pub ttl: Duration,
    pub expires_at: Instant,
}

impl Room {
    pub fn new(
        room_id: &str,
        host_conn: ConnId,
        host_tx: ClientTx,
        join_key: &str,
        ttl: Duration,
    ) -> Self {
        let _ = room_id;
        Self {
            join_key_hash: JoinKeyHash::new(join_key),
            host_conn,
            host_tx,
            viewers: HashMap::new(),
            controller_id: None,
            pending_request: false,
            last_snapshot: None,
            last_selection: None,
            ttl,
            expires_at: Instant::now() + ttl,
        }
    }

    pub fn is_expired(&self) -> bool {
        Instant::now() >= self.expires_at
    }

    pub fn refresh_ttl(&mut self) {
        self.expires_at = Instant::now() + self.ttl;
    }

    /// Releases control if `viewer_id` is the current controller.
    ///
    /// Returns `true` when the controller was cleared.
    pub fn release_control_if_controller(&mut self, viewer_id: &str) -> bool {
        if self.controller_id.as_deref() != Some(viewer_id) {
            return false;
        }
        self.controller_id = None;
        self.pending_request = false;
        true
    }
}

#[derive(Clone, Default)]
pub struct RelayState {
    next_conn_id: Arc<AtomicU64>,
    rooms: Arc<RwLock<HashMap<String, Room>>>,
}

impl RelayState {
    pub fn alloc_conn_id(&self) -> ConnId {
        self.next_conn_id
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1)
    }

    pub async fn with_rooms<T>(&self, f: impl FnOnce(&HashMap<String, Room>) -> T) -> T {
        let rooms = self.rooms.read().await;
        f(&rooms)
    }

    pub async fn get_room(&self, room_id: &str) -> Option<RoomSnapshot> {
        self.with_rooms(|rooms| {
            rooms.get(room_id).map(|room| RoomSnapshot {
                room_id: room_id.to_string(),
                controller_id: room.controller_id.clone(),
                pending_request: room.pending_request,
            })
        })
        .await
    }

    pub async fn with_rooms_mut<T>(&self, f: impl FnOnce(&mut HashMap<String, Room>) -> T) -> T {
        let mut rooms = self.rooms.write().await;
        f(&mut rooms)
    }

    pub async fn remove_expired_rooms(&self) -> Vec<(String, HashMap<ConnId, ClientTx>)> {
        self.with_rooms_mut(|rooms| {
            let mut expired_room_ids = Vec::new();
            for (room_id, room) in rooms.iter() {
                if room.is_expired() {
                    expired_room_ids.push(room_id.clone());
                }
            }

            let mut removed = Vec::new();
            for room_id in expired_room_ids {
                if let Some(room) = rooms.remove(&room_id) {
                    removed.push((room_id, room.viewers));
                }
            }
            removed
        })
        .await
    }
}

#[derive(Debug, Clone)]
pub struct RoomSnapshot {
    pub room_id: String,
    pub controller_id: Option<String>,
    pub pending_request: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_key_hash_matches_same_key() {
        let h = JoinKeyHash::new("Ab12xY");
        assert!(h.matches("Ab12xY"));
    }

    #[test]
    fn join_key_hash_rejects_different_key() {
        let h = JoinKeyHash::new("Ab12xY");
        assert!(!h.matches("Ab12xZ"));
    }

    #[test]
    fn constant_time_eq_works() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
    }

    #[tokio::test]
    async fn remove_expired_rooms_removes_only_expired() {
        let state = RelayState::default();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel::<String>();

        state
            .with_rooms_mut(|rooms| {
                rooms.insert(
                    "expired".to_string(),
                    Room {
                        join_key_hash: JoinKeyHash::new("a"),
                        host_conn: 1,
                        host_tx: tx.clone(),
                        viewers: HashMap::new(),
                        controller_id: None,
                        pending_request: false,
                        last_snapshot: None,
                        last_selection: None,
                        ttl: Duration::from_secs(1),
                        expires_at: Instant::now() - Duration::from_secs(1),
                    },
                );
                rooms.insert(
                    "alive".to_string(),
                    Room {
                        join_key_hash: JoinKeyHash::new("b"),
                        host_conn: 2,
                        host_tx: tx.clone(),
                        viewers: HashMap::new(),
                        controller_id: None,
                        pending_request: false,
                        last_snapshot: None,
                        last_selection: None,
                        ttl: Duration::from_secs(60),
                        expires_at: Instant::now() + Duration::from_secs(60),
                    },
                );
            })
            .await;

        let removed = state.remove_expired_rooms().await;
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].0, "expired");
        assert!(state.get_room("expired").await.is_none());
        assert!(state.get_room("alive").await.is_some());
    }

    #[test]
    fn release_control_clears_controller_and_pending() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let mut room = Room::new("room", 1, tx, "k3Y9a1", Duration::from_secs(60));
        room.controller_id = Some("viewer-1".to_string());
        room.pending_request = true;

        assert!(room.release_control_if_controller("viewer-1"));
        assert!(room.controller_id.is_none());
        assert!(!room.pending_request);
    }

    #[test]
    fn release_control_does_not_clear_other_controller() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let mut room = Room::new("room", 1, tx, "k3Y9a1", Duration::from_secs(60));
        room.controller_id = Some("viewer-1".to_string());
        room.pending_request = true;

        assert!(!room.release_control_if_controller("viewer-2"));
        assert_eq!(room.controller_id.as_deref(), Some("viewer-1"));
        assert!(room.pending_request);
    }
}
