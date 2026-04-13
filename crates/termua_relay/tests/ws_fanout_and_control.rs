use futures::{SinkExt as _, StreamExt as _};
use termua_relay::protocol::{ClientToRelay, RelayToClient};
use tokio_tungstenite::tungstenite::Message;

fn msg_text<T: serde::Serialize>(msg: &T) -> Message {
    Message::Text(serde_json::to_string(msg).unwrap().into())
}

async fn recv_msg(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
) -> RelayToClient {
    let Some(Ok(Message::Text(t))) = ws.next().await else {
        panic!("expected text message");
    };
    serde_json::from_str::<RelayToClient>(&t).unwrap()
}

#[tokio::test]
#[ignore = "requires network sockets; enable in CI or run manually"]
async fn host_frames_fanout_to_all_viewers() {
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
        .send(msg_text(&ClientToRelay::Register {
            room_id: "AbC123xYz".to_string(),
            join_key: "k3Y9a1".to_string(),
            ttl_secs: Some(60),
        }))
        .await
        .unwrap();
    assert!(matches!(recv_msg(&mut host_ws).await, RelayToClient::Ok));

    let (mut v1, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    v1.send(msg_text(&ClientToRelay::Join {
        room_id: "AbC123xYz".to_string(),
        join_key: "k3Y9a1".to_string(),
    }))
    .await
    .unwrap();
    let _ = recv_msg(&mut v1).await;

    let (mut v2, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    v2.send(msg_text(&ClientToRelay::Join {
        room_id: "AbC123xYz".to_string(),
        join_key: "k3Y9a1".to_string(),
    }))
    .await
    .unwrap();
    let _ = recv_msg(&mut v2).await;

    host_ws
        .send(msg_text(&ClientToRelay::Frame {
            room_id: "AbC123xYz".to_string(),
            seq: 1,
            payload: serde_json::json!({"k":"v"}),
        }))
        .await
        .unwrap();

    let m1 = recv_msg(&mut v1).await;
    let m2 = recv_msg(&mut v2).await;
    assert!(matches!(m1, RelayToClient::Frame { .. }));
    assert!(matches!(m2, RelayToClient::Frame { .. }));
}

#[tokio::test]
#[ignore = "requires network sockets; enable in CI or run manually"]
async fn later_control_requests_are_rejected_as_busy() {
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
        .send(msg_text(&ClientToRelay::Register {
            room_id: "AbC123xYz".to_string(),
            join_key: "k3Y9a1".to_string(),
            ttl_secs: Some(60),
        }))
        .await
        .unwrap();
    assert!(matches!(recv_msg(&mut host_ws).await, RelayToClient::Ok));

    let (mut v1, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    v1.send(msg_text(&ClientToRelay::Join {
        room_id: "AbC123xYz".to_string(),
        join_key: "k3Y9a1".to_string(),
    }))
    .await
    .unwrap();
    let v1_joined = recv_msg(&mut v1).await;
    let RelayToClient::Joined {
        viewer_id: v1_id, ..
    } = v1_joined
    else {
        panic!("expected Joined");
    };

    let (mut v2, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    v2.send(msg_text(&ClientToRelay::Join {
        room_id: "AbC123xYz".to_string(),
        join_key: "k3Y9a1".to_string(),
    }))
    .await
    .unwrap();
    let v2_joined = recv_msg(&mut v2).await;
    let RelayToClient::Joined {
        viewer_id: v2_id, ..
    } = v2_joined
    else {
        panic!("expected Joined");
    };

    // v1 requests control -> forwarded to host; relay sets pending_request=true.
    v1.send(msg_text(&ClientToRelay::Request {
        room_id: "AbC123xYz".to_string(),
        viewer_id: v1_id.clone(),
        viewer_label: None,
    }))
    .await
    .unwrap();

    // host receives the request
    let host_msg = recv_msg(&mut host_ws).await;
    assert!(matches!(host_msg, RelayToClient::CtrlRequest { .. }));

    // v2 requests control while pending -> busy denied.
    v2.send(msg_text(&ClientToRelay::Request {
        room_id: "AbC123xYz".to_string(),
        viewer_id: v2_id,
        viewer_label: None,
    }))
    .await
    .unwrap();
    let v2_denied = recv_msg(&mut v2).await;
    assert!(matches!(
        v2_denied,
        RelayToClient::CtrlDenied { reason, .. } if reason == "busy"
    ));
}
