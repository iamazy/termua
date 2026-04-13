use futures::{SinkExt as _, StreamExt as _};
use termua_relay::protocol::{ClientToRelay, RelayToClient};
use tokio_tungstenite::tungstenite::Message;

#[tokio::test]
#[ignore = "requires network sockets; enable in CI or run manually"]
async fn register_then_join_works() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        termua_relay::server::serve_with_listener(listener, Default::default())
            .await
            .unwrap();
    });

    let url = format!("ws://{addr}/ws");

    let (mut host_ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    host_ws
        .send(Message::Text(
            serde_json::to_string(&ClientToRelay::Register {
                room_id: "AbC123xYz".to_string(),
                join_key: "k3Y9a1".to_string(),
                ttl_secs: Some(60),
            })
            .unwrap()
            .into(),
        ))
        .await
        .unwrap();

    let host_reply = host_ws.next().await.unwrap().unwrap();
    let Message::Text(host_text) = host_reply else {
        panic!("expected text");
    };
    let host_msg: RelayToClient = serde_json::from_str(&host_text).unwrap();
    assert!(matches!(host_msg, RelayToClient::Ok));

    let (mut viewer_ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    viewer_ws
        .send(Message::Text(
            serde_json::to_string(&ClientToRelay::Join {
                room_id: "AbC123xYz".to_string(),
                join_key: "k3Y9a1".to_string(),
            })
            .unwrap()
            .into(),
        ))
        .await
        .unwrap();

    let viewer_reply = viewer_ws.next().await.unwrap().unwrap();
    let Message::Text(viewer_text) = viewer_reply else {
        panic!("expected text");
    };
    let viewer_msg: RelayToClient = serde_json::from_str(&viewer_text).unwrap();
    match viewer_msg {
        RelayToClient::Joined { room_id, viewer_id } => {
            assert_eq!(room_id, "AbC123xYz");
            assert!(!viewer_id.trim().is_empty());
        }
        other => panic!("unexpected message: {other:?}"),
    }
}
