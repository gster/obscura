//! A CDP socket disconnect must interrupt synchronous V8 immediately.
//!
//! The command watchdog's default is far beyond this test's five-second hard
//! bound. The only way the first connection can release the server's sole slot
//! in time is for WebSocket I/O to observe the close on a thread which is not
//! pinned by V8 and cancel the active isolate. A fresh connection and evaluation
//! then prove that no stale cancellation was replayed into later work.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Notify;
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};

type Ws = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

async fn pick_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    listener.local_addr().unwrap().port()
}

async fn command(
    ws: &mut Ws,
    id: u64,
    method: &str,
    params: Value,
    session: Option<&str>,
) -> Value {
    let mut request = json!({"id": id, "method": method, "params": params});
    if let Some(session) = session {
        request["sessionId"] = Value::String(session.to_string());
    }
    ws.send(Message::Text(request.to_string().into()))
        .await
        .unwrap();
    loop {
        let message = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .expect("CDP response timeout")
            .expect("CDP socket closed")
            .expect("CDP WebSocket error");
        let Message::Text(text) = message else { continue; };
        let value: Value = serde_json::from_str(&text).unwrap();
        if value.get("id").and_then(Value::as_u64) == Some(id) {
            return value;
        }
    }
}

async fn create_page(ws: &mut Ws, id: u64) -> String {
    let response = command(
        ws,
        id,
        "Target.createTarget",
        json!({"url": "about:blank"}),
        None,
    )
    .await;
    assert!(response.get("error").is_none(), "createTarget failed: {response}");
    let target = response["result"]["targetId"].as_str().unwrap();
    format!("{target}-session")
}

async fn reconnect(url: &str) -> Ws {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        match connect_async(url).await {
            Ok((ws, _)) => return ws,
            Err(error) if tokio::time::Instant::now() < deadline => {
                let _ = error;
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            Err(error) => panic!("connection slot did not release after V8 cancellation: {error}"),
        }
    }
}

async fn start_single_connection_server(port: u16) {
    let _ = obscura_cdp::start_with_serve_options_and_limit(
        port,
        "127.0.0.1",
        None,
        false,
        None,
        true,
        1,
        obscura_net::EffectivePersona::builtin(
            obscura_net::StealthProfile::WindowsChrome145,
        ),
    )
    .await;
}

#[test]
fn websocket_close_interrupts_infinite_runtime_evaluate() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let local = tokio::task::LocalSet::new();
    local.block_on(&runtime, async {
        let port = pick_port().await;
        tokio::task::spawn_local(async move {
            start_single_connection_server(port).await;
        });
        tokio::time::sleep(Duration::from_millis(250)).await;

        let url = format!("ws://127.0.0.1:{port}/devtools/browser");
        let (mut first, _) = connect_async(&url).await.expect("first connection");
        let session = create_page(&mut first, 1).await;
        first
            .send(Message::Text(
                json!({
                    "id": 2,
                    "method": "Runtime.evaluate",
                    "sessionId": session,
                    "params": {"expression": "while (true) {}", "returnByValue": true}
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;
        let _ = first.send(Message::Close(None)).await;
        drop(first);

        let mut second = reconnect(&url).await;
        let second_session = create_page(&mut second, 10).await;
        let response = command(
            &mut second,
            11,
            "Runtime.evaluate",
            json!({"expression": "6 * 7", "returnByValue": true}),
            Some(&second_session),
        )
        .await;
        assert_eq!(
            response["result"]["result"]["value"].as_f64(),
            Some(42.0),
            "fresh connection response: {response}"
        );
    });
}

#[test]
fn websocket_close_interrupts_infinite_navigation_script() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let local = tokio::task::LocalSet::new();
    local.block_on(&runtime, async {
        let fixture = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let fixture_port = fixture.local_addr().unwrap().port();
        let served = std::sync::Arc::new(Notify::new());
        let fixture_served = served.clone();
        tokio::task::spawn_local(async move {
            let (mut socket, _) = fixture.accept().await.unwrap();
            let mut request = [0_u8; 2048];
            let _ = socket.read(&mut request).await;
            let body = b"<!doctype html><script>while (true) {}</script>";
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            socket.write_all(head.as_bytes()).await.unwrap();
            socket.write_all(body).await.unwrap();
            socket.flush().await.unwrap();
            fixture_served.notify_one();
        });

        let port = pick_port().await;
        tokio::task::spawn_local(async move {
            start_single_connection_server(port).await;
        });
        tokio::time::sleep(Duration::from_millis(250)).await;

        let url = format!("ws://127.0.0.1:{port}/devtools/browser");
        let (mut first, _) = connect_async(&url).await.expect("first connection");
        first
            .send(Message::Text(
                json!({
                    "id": 1,
                    "method": "Target.createTarget",
                    "params": {"url": format!("http://127.0.0.1:{fixture_port}/")}
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), served.notified())
            .await
            .expect("navigation fixture was not requested");
        tokio::time::sleep(Duration::from_millis(150)).await;
        let _ = first.send(Message::Close(None)).await;
        drop(first);

        let mut second = reconnect(&url).await;
        let response = command(&mut second, 10, "Target.getTargets", json!({}), None).await;
        assert!(
            response["result"]["targetInfos"].is_array(),
            "fresh connection response: {response}"
        );
    });
}

#[test]
fn websocket_close_aborts_navigation_waiting_on_transport() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let local = tokio::task::LocalSet::new();
    local.block_on(&runtime, async {
        let fixture = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let fixture_port = fixture.local_addr().unwrap().port();
        let requested = std::sync::Arc::new(Notify::new());
        let fixture_requested = requested.clone();
        tokio::task::spawn_local(async move {
            let (mut socket, _) = fixture.accept().await.unwrap();
            let mut request = [0_u8; 2048];
            let _ = socket.read(&mut request).await;
            fixture_requested.notify_one();
            std::future::pending::<()>().await;
        });

        let port = pick_port().await;
        tokio::task::spawn_local(async move {
            start_single_connection_server(port).await;
        });
        tokio::time::sleep(Duration::from_millis(250)).await;

        let url = format!("ws://127.0.0.1:{port}/devtools/browser");
        let (mut first, _) = connect_async(&url).await.expect("first connection");
        first
            .send(Message::Text(
                json!({
                    "id": 1,
                    "method": "Target.createTarget",
                    "params": {"url": format!("http://127.0.0.1:{fixture_port}/")}
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), requested.notified())
            .await
            .expect("slow navigation fixture was not requested");
        let _ = first.send(Message::Close(None)).await;
        drop(first);

        let mut second = reconnect(&url).await;
        let response = command(&mut second, 10, "Target.getTargets", json!({}), None).await;
        assert!(
            response["result"]["targetInfos"].is_array(),
            "fresh connection response: {response}"
        );
    });
}
