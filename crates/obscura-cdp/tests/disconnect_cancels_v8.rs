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

#[derive(Clone, Default)]
struct CompleteLogCapture(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

struct CompleteLogWriter(CompleteLogCapture);

impl std::io::Write for CompleteLogWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CompleteLogCapture {
    type Writer = CompleteLogWriter;

    fn make_writer(&'a self) -> Self::Writer {
        CompleteLogWriter(self.clone())
    }
}

fn install_complete_log_capture() -> CompleteLogCapture {
    static CAPTURE: std::sync::OnceLock<CompleteLogCapture> = std::sync::OnceLock::new();
    CAPTURE
        .get_or_init(|| {
            let capture = CompleteLogCapture::default();
            tracing_subscriber::fmt()
                .with_ansi(false)
                .with_writer(capture.clone())
                .try_init()
                .expect("install disconnect qualification tracing capture");
            capture
        })
        .clone()
}

async fn wait_for_log(capture: &CompleteLogCapture, marker: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let bytes = capture.0.lock().unwrap().clone();
        if bytes.windows(marker.len()).any(|window| window == marker.as_bytes()) {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "missing synchronous iframe marker {marker}; complete logs:\n{}",
            String::from_utf8_lossy(&bytes)
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

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
    reconnect_within(url, Duration::from_secs(5)).await
}

async fn reconnect_within(url: &str, timeout: Duration) -> Ws {
    let deadline = tokio::time::Instant::now() + timeout;
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

async fn raw_fin(mut ws: Ws) -> Ws {
    let MaybeTlsStream::Plain(stream) = ws.get_mut() else {
        panic!("disconnect qualification requires a plain loopback socket");
    };
    stream.shutdown().await.expect("send raw TCP FIN");
    ws
}

fn raw_rst(mut ws: Ws) {
    let MaybeTlsStream::Plain(stream) = ws.get_mut() else {
        panic!("disconnect qualification requires a plain loopback socket");
    };
    socket2::SockRef::from(&*stream)
        .set_linger(Some(Duration::ZERO))
        .expect("configure abortive TCP close");
    drop(ws);
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

#[test]
fn raw_fin_interrupts_infinite_iframe_script() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let local = tokio::task::LocalSet::new();
    local.block_on(&runtime, async {
        let log_capture = install_complete_log_capture();
        let marker = "obscura-iframe-disconnect-loop-entered-7f4a8d23";
        let queued_marker = "obscura-queued-command-must-not-run-a12c6e95";
        let fixture = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let fixture_port = fixture.local_addr().unwrap().port();
        let replay_fixture = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let replay_port = replay_fixture.local_addr().unwrap().port();
        let served = std::sync::Arc::new(Notify::new());
        let fixture_served = served.clone();
        tokio::task::spawn_local(async move {
            let (mut socket, _) = fixture.accept().await.unwrap();
            let mut request = [0_u8; 2048];
            let _ = socket.read(&mut request).await;
            let body = format!(
                "<!doctype html><script>for(let first=true;;){{if(first){{first=false;console.info('{marker}')}}}}</script>"
            );
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            socket.write_all(head.as_bytes()).await.unwrap();
            socket.write_all(body.as_bytes()).await.unwrap();
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
        let session = create_page(&mut first, 1).await;
        first
            .send(Message::Text(
                json!({
                    "id": 2,
                    "method": "Runtime.evaluate",
                    "sessionId": session,
                    "params": {
                        "expression": format!("(function(){{const frame=document.createElement('iframe');frame.src='http://127.0.0.1:{fixture_port}/child';document.body.appendChild(frame);return 'created'}})()"),
                        "returnByValue": true
                    }
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), served.notified())
            .await
            .expect("iframe document was not requested");
        wait_for_log(&log_capture, marker).await;

        // This command is fully written before FIN while the processor is
        // pinned in the frame. Sticky cancellation must reject it rather than
        // execute it during teardown or replay it on the next connection.
        first
            .send(Message::Text(
                json!({
                    "id": 3,
                    "method": "Runtime.evaluate",
                    "sessionId": session,
                    "params": {
                        "expression": format!("console.info('{queued_marker}');fetch('http://127.0.0.1:{replay_port}/must-not-run')"),
                        "returnByValue": true
                    }
                })
                .to_string()
                .into(),
            ))
            .await
            .unwrap();
        let mut fin_socket = raw_fin(first).await;

        // Keep the read half open and drain it until the server closes. Dropping
        // a socket with unread peer data can turn an intended FIN into RST on
        // some TCP stacks, which would not qualify the graceful EOF path.
        let drain = async {
            let mut messages = Vec::new();
            while let Some(message) = fin_socket.next().await {
                let done = message.is_err() || matches!(&message, Ok(Message::Close(_)));
                messages.push(message);
                if done { break; }
            }
            messages
        };
        let (mut second, drained) = tokio::join!(
            reconnect(&url),
            tokio::time::timeout(Duration::from_secs(5), drain),
        );
        let _complete_old_connection_messages = drained
            .expect("server did not close the read half after client FIN");
        let second_session = create_page(&mut second, 10).await;
        let response = command(
            &mut second,
            11,
            "Runtime.evaluate",
            json!({"expression": "6 * 7", "returnByValue": true}),
            Some(&second_session),
        )
        .await;
        assert_eq!(response["result"]["result"]["value"].as_f64(), Some(42.0));
        let complete_logs = log_capture.0.lock().unwrap().clone();
        assert!(
            !complete_logs
                .windows(queued_marker.len())
                .any(|window| window == queued_marker.as_bytes()),
            "queued JavaScript ran after FIN; complete logs:\n{}",
            String::from_utf8_lossy(&complete_logs)
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(300), replay_fixture.accept())
                .await
                .is_err(),
            "a command queued behind cancelled iframe V8 was executed or replayed"
        );
    });
}

#[test]
fn raw_rst_interrupts_infinite_dedicated_worker_script() {
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
        let response = command(
            &mut first,
            2,
            "Runtime.evaluate",
            json!({
                "expression": "new Promise(resolve=>{const source=\"for(let first=true;;){if(first){first=false;postMessage('started')}}\";const worker=new Worker(URL.createObjectURL(new Blob([source],{type:'application/javascript'})));globalThis.__disconnectWorker=worker;worker.onmessage=event=>resolve(event.data)})",
                "awaitPromise": true,
                "returnByValue": true
            }),
            Some(&session),
        )
        .await;
        assert_eq!(
            response["result"]["result"]["value"],
            "started",
            "the worker must enter its own synchronous script before disconnect: {response}"
        );
        raw_rst(first);

        // This wire assertion is paired with the obscura-js regression which
        // keeps the owner alive and proves connection cancellation itself stops
        // the Worker thread. Here the raw RST proves the server observes the
        // transport failure and releases the connection slot.
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
        assert_eq!(response["result"]["result"]["value"].as_f64(), Some(42.0));
    });
}
