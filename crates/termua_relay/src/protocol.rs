use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientToRelay {
    Register {
        room_id: String,
        join_key: String,
        ttl_secs: Option<u64>,
    },
    Join {
        room_id: String,
        join_key: String,
    },
    Request {
        room_id: String,
        viewer_id: String,
        viewer_label: Option<String>,
    },
    Release {
        room_id: String,
        viewer_id: String,
    },
    /// Host -> Relay: grant control to a viewer.
    Granted {
        room_id: String,
        viewer_id: String,
    },
    /// Host -> Relay: deny a specific viewer's control request.
    Denied {
        room_id: String,
        viewer_id: String,
        reason: String,
    },
    /// Host -> Relay: controller voluntarily released control.
    Released {
        room_id: String,
        viewer_id: String,
    },
    /// Host -> Relay: revoke any current controller.
    Revoked {
        room_id: String,
    },
    InputEvent {
        room_id: String,
        viewer_id: String,
        payload: serde_json::Value,
    },
    Frame {
        room_id: String,
        seq: u64,
        payload: serde_json::Value,
    },
    /// Host -> Relay: selection update (range/text) without a full frame.
    Selection {
        room_id: String,
        seq: u64,
        payload: serde_json::Value,
    },
    Snapshot {
        room_id: String,
        seq: u64,
        payload: serde_json::Value,
    },
    Stop {
        room_id: String,
    },
    Ping,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RelayToClient {
    Ok,
    Error {
        code: String,
        message: String,
    },
    Joined {
        room_id: String,
        viewer_id: String,
    },
    /// Relay -> Host: viewer is requesting control.
    CtrlRequest {
        room_id: String,
        viewer_id: String,
        viewer_label: Option<String>,
    },
    /// Relay -> Host: viewer is releasing control.
    CtrlRelease {
        room_id: String,
        viewer_id: String,
    },
    CtrlDenied {
        room_id: String,
        reason: String,
    },
    CtrlGranted {
        room_id: String,
        viewer_id: String,
    },
    CtrlReleased {
        room_id: String,
        viewer_id: String,
    },
    CtrlRevoked {
        room_id: String,
    },
    /// Relay -> Host: input event from the current controller (or ungated if disabled).
    InputEvent {
        room_id: String,
        viewer_id: String,
        payload: serde_json::Value,
    },
    Frame {
        room_id: String,
        seq: u64,
        payload: serde_json::Value,
    },
    Selection {
        room_id: String,
        seq: u64,
        payload: serde_json::Value,
    },
    Snapshot {
        room_id: String,
        seq: u64,
        payload: serde_json::Value,
    },
    Pong,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_to_relay_deserializes_selection() {
        let raw = serde_json::json!({
            "type": "selection",
            "room_id": "ABCDEF123",
            "seq": 42,
            "payload": {
                "selection": null,
                "selection_text": null
            }
        });

        let parsed: Result<ClientToRelay, _> = serde_json::from_value(raw);
        assert!(
            parsed.is_ok(),
            "expected selection to deserialize: {parsed:?}"
        );
    }

    #[test]
    fn relay_to_client_deserializes_selection() {
        let raw = serde_json::json!({
            "type": "selection",
            "room_id": "ABCDEF123",
            "seq": 42,
            "payload": {
                "selection": null,
                "selection_text": null
            }
        });

        let parsed: Result<RelayToClient, _> = serde_json::from_value(raw);
        assert!(
            parsed.is_ok(),
            "expected selection to deserialize: {parsed:?}"
        );
    }
}
