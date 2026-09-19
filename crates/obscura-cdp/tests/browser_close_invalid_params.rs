use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio_tungstenite::{connect_async, tungstenite::Message};

async fn pick_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

async fn response_for<S>(ws: &mut tokio_tungstenite::WebSocketStream<S>, id: u64) -> Value
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    loop {
        let message = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .expect("response timeout")
            .expect("connection closed before response")
            .expect("WebSocket error");
        if let Message::Text(text) = message {
            let value: Value = serde_json::from_str(&text).expect("CDP JSON response");
            if value.get("id").and_then(Value::as_u64) == Some(id) {
                return value;
            }
        }
    }
}

#[test]
fn invalid_browser_close_does_not_close_the_websocket() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let local = tokio::task::LocalSet::new();

    local.block_on(&runtime, async move {
        let port = pick_port().await;
        tokio::task::spawn_local(async move {
            let _ = obscura_cdp::start_with_serve_options_and_limit(
                port,
                "127.0.0.1",
                None,
                false,
                None,
                true,
                1,
            )
            .await;
        });
        tokio::time::sleep(Duration::from_millis(300)).await;

        let url = format!("ws://127.0.0.1:{port}/devtools/browser");
        let (mut ws, _) = connect_async(url).await.expect("WebSocket handshake");
        ws.send(Message::Text(
            json!({
                "id": 1,
                "method": "Browser.close",
                "params": {"invented": true},
            })
            .to_string()
            .into(),
        ))
        .await
        .expect("send invalid close");
        let invalid = response_for(&mut ws, 1).await;
        assert_eq!(invalid["error"]["code"], -32601);
        assert_eq!(
            invalid["error"]["message"],
            "Browser.close supports only empty params"
        );

        ws.send(Message::Text(
            json!({"id": 2, "method": "Browser.getVersion"})
                .to_string()
                .into(),
        ))
        .await
        .expect("send follow-up on the same connection");
        let follow_up = response_for(&mut ws, 2).await;
        assert_eq!(follow_up["result"]["product"], "Chrome/145.0.0.0");
    });
}
