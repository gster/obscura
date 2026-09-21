//! End-to-end admission checks for discovery and WebSocket CDP entry points.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message;

const TOKEN: &str = "0123456789abcdef0123456789abcdef";

async fn pick_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

async fn raw_request(port: u16, request: Vec<u8>) -> Vec<u8> {
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("connect to CDP listener");
    stream.write_all(&request).await.expect("write request");
    let mut response = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut response))
        .await
        .expect("response timeout")
        .expect("read response");
    response
}

fn discovery_request(host: &str, extra: &str) -> Vec<u8> {
    format!(
        "GET /json/version HTTP/1.1\r\nHost: {host}\r\nAuthorization: Bearer {TOKEN}\r\n{extra}Connection: close\r\n\r\n"
    )
    .into_bytes()
}

#[test]
fn access_policy_gates_discovery_and_websocket_before_cdp() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let local = tokio::task::LocalSet::new();

    local.block_on(&runtime, async move {
        let port = pick_port().await;
        let access = obscura_cdp::CdpAccessOptions::new()
            .with_bearer_token(Some(TOKEN.to_string()));
        tokio::task::spawn_local(async move {
            let _ = obscura_cdp::start_with_serve_options_access_and_limit(
                port,
                "127.0.0.1",
                None,
                false,
                None,
                false,
                1,
                access,
                obscura_net::EffectivePersona::builtin(
                    obscura_net::StealthProfile::WindowsChrome145,
                ),
            )
            .await;
        });
        tokio::time::sleep(Duration::from_millis(300)).await;

        let missing_auth = raw_request(
            port,
            format!(
                "GET /json/version HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
            )
            .into_bytes(),
        )
        .await;
        assert!(missing_auth.starts_with(b"HTTP/1.1 401"));

        let bad_host = raw_request(
            port,
            discovery_request("attacker.test:9222", ""),
        )
        .await;
        assert!(bad_host.starts_with(b"HTTP/1.1 421"));

        let bad_origin = raw_request(
            port,
            discovery_request(
                &format!("127.0.0.1:{port}"),
                "Origin: https://attacker.test\r\n",
            ),
        )
        .await;
        assert!(bad_origin.starts_with(b"HTTP/1.1 403"));

        let duplicate_auth = raw_request(
            port,
            discovery_request(
                &format!("127.0.0.1:{port}"),
                &format!("Authorization: Bearer {TOKEN}\r\n"),
            ),
        )
        .await;
        assert!(duplicate_auth.starts_with(b"HTTP/1.1 400"));

        let valid = raw_request(
            port,
            discovery_request(&format!("127.0.0.1:{port}"), ""),
        )
        .await;
        assert!(valid.starts_with(b"HTTP/1.1 200"));
        let valid_text = String::from_utf8(valid).unwrap();
        assert!(valid_text.contains(&format!(
            "\"webSocketDebuggerUrl\": \"ws://127.0.0.1:{port}/devtools/browser\""
        )));
        assert!(!valid_text.contains(TOKEN), "token must never be reflected");

        let oversized = raw_request(port, vec![b'A'; 4096]).await;
        assert!(oversized.starts_with(b"HTTP/1.1 431"));

        let url = format!("ws://127.0.0.1:{port}/devtools/browser");
        let mut request = url.into_client_request().unwrap();
        request.headers_mut().insert(
            "Authorization",
            HeaderValue::from_str(&format!("Bearer {TOKEN}")).unwrap(),
        );
        request.headers_mut().insert(
            "Origin",
            HeaderValue::from_str(&format!("http://127.0.0.1:{port}")).unwrap(),
        );
        let (mut websocket, _) = tokio_tungstenite::connect_async(request)
            .await
            .expect("authorized WebSocket handshake");
        websocket
            .send(Message::Text(
                json!({"id": 1, "method": "Browser.getVersion"})
                    .to_string()
                    .into(),
            ))
            .await
            .expect("send CDP command");
        let response = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let message = websocket
                    .next()
                    .await
                    .expect("WebSocket closed")
                    .expect("WebSocket message");
                if let Message::Text(text) = message {
                    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
                    if value["id"] == 1 {
                        break value;
                    }
                }
            }
        })
        .await
        .expect("CDP response timeout");
        assert_eq!(response["result"]["product"], "Chrome/145.0.0.0");
    });
}
