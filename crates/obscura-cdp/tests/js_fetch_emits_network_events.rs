// Regression for issue #406: requests initiated by page JS (fetch/XHR/dynamic
// resource) must emit Network.requestWillBeSent / responseReceived so
// Puppeteer/Playwright `page.on('request'|'response')` observe them. On main
// only the static navigation subresources surfaced; a `fetch()` fired from the
// page produced no CDP Network event, so clients captured zero XHR/JSON
// responses (this is also the root cause of the Aviasales half of #394).

use obscura_cdp::dispatch::{dispatch, CdpContext};
use obscura_cdp::types::CdpRequest;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

// Serves an HTML page that fetches /api/start.json, which redirects to the
// JSON response at /api/data.json.
async fn serve() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        for _ in 0..6 {
            let (mut socket, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let mut buf = [0u8; 2048];
                let _ = socket.read(&mut buf).await.unwrap();
                let req = String::from_utf8_lossy(&buf[..]);
                if req.starts_with("GET /api/start.json") {
                    let resp = "HTTP/1.1 302 Found\r\nLocation: /api/data.json\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                    let _ = socket.write_all(resp.as_bytes()).await;
                    return;
                }
                if req.starts_with("GET /api/blob.bin") {
                    let body = [0u8, 128, 255, 16];
                    let headers = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len(),
                    );
                    let _ = socket.write_all(headers.as_bytes()).await;
                    let _ = socket.write_all(&body).await;
                    return;
                }
                let (ct, body) = if req.starts_with("GET /api/data.json") {
                    ("application/json", "{\"value\":42}")
                } else {
                    (
                        "text/html",
                        r#"<html><head></head><body>
<div id="r">stage1</div>
<script>
window.__done = new Promise(function (resolve) {
  Promise.all([
    fetch("/api/start.json").then(function (r) { return r.json(); }),
    fetch("/api/blob.bin").then(function (r) { return r.arrayBuffer(); })
  ])
    .then(function (values) { document.getElementById("r").textContent = "got:" + values[0].value; resolve("ok"); })
    .catch(function (e) { resolve("err:" + e); });
});
</script>
</body></html>"#,
                    )
                };
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {ct}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(resp.as_bytes()).await;
            });
        }
    });
    format!("http://{addr}/")
}

async fn cdp(ctx: &mut CdpContext, id: u64, method: &str, params: Value, session_id: &str) -> Value {
    let resp = dispatch(
        &CdpRequest {
            id,
            method: method.to_string(),
            params,
            session_id: Some(session_id.to_string()),
        },
        ctx,
    )
    .await;
    assert!(resp.error.is_none(), "CDP {method} failed: {:?}", resp.error);
    resp.result.unwrap_or_else(|| json!({}))
}

// Collect the request URLs from every Network.requestWillBeSent currently
// queued in ctx.pending_events, then clear the queue.
fn drain_request_urls(ctx: &mut CdpContext) -> Vec<String> {
    let urls = ctx
        .pending_events
        .iter()
        .filter(|e| e.method == "Network.requestWillBeSent")
        .filter_map(|e| e.params.get("request").and_then(|r| r.get("url")).and_then(|u| u.as_str()).map(str::to_string))
        .collect();
    ctx.pending_events.clear();
    urls
}

// The requestId that Network.responseReceived reported for the given URL.
fn response_request_id(ctx: &CdpContext, url_needle: &str) -> Option<String> {
    ctx.pending_events
        .iter()
        .find(|e| {
            e.method == "Network.responseReceived"
                && e.params
                    .get("response")
                    .and_then(|r| r.get("url"))
                    .and_then(|u| u.as_str())
                    .map(|u| u.contains(url_needle))
                    .unwrap_or(false)
        })
        .and_then(|e| e.params.get("requestId").and_then(|v| v.as_str()).map(str::to_string))
}

#[tokio::test(flavor = "current_thread")]
async fn js_fetch_emits_network_request_and_response() {
    std::env::set_var("OBSCURA_ALLOW_PRIVATE_NETWORK", "1");
    let base = serve().await;
    let mut ctx = CdpContext::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));
    let page_id = ctx.create_page();
    let session_id = "session-1";
    ctx.sessions.insert(session_id.to_string(), page_id.clone());
    cdp(&mut ctx, 0, "Network.enable", json!({}), session_id).await;

    // An ordinary fetch() is not load-delaying in Chromium: `load` may fire
    // while its response is still pending. Ask for networkidle0 explicitly so
    // this output-level assertion observes the completed request without
    // turning every load navigation into an implicit global settle.
    cdp(
        &mut ctx,
        1,
        "Page.navigate",
        json!({"url": base, "waitUntil": "networkidle0"}),
        session_id,
    )
    .await;

    // The final fetched JSON URL must appear as a requestWillBeSent event.
    let request_urls = ctx
        .pending_events
        .iter()
        .filter(|e| e.method == "Network.requestWillBeSent")
        .filter_map(|e| e.params.get("request").and_then(|r| r.get("url")).and_then(|u| u.as_str()).map(str::to_string))
        .collect::<Vec<_>>();
    assert!(
        request_urls.iter().any(|u| u.contains("/api/data.json")),
        "script-initiated fetch must emit Network.requestWillBeSent; saw {request_urls:?}"
    );
    // And its response body must be resolvable via the same requestId, so a
    // client can read the captured JSON.
    let request_id = response_request_id(&ctx, "/api/data.json")
        .expect("fetch must emit Network.responseReceived with a requestId");
    let body = cdp(
        &mut ctx,
        2,
        "Network.getResponseBody",
        json!({"requestId": request_id}),
        session_id,
    )
    .await;
    assert_eq!(
        body.get("body").and_then(|b| b.as_str()),
        Some("{\"value\":42}"),
        "Network.getResponseBody must return the script-fetched JSON"
    );

    let binary_request_id = response_request_id(&ctx, "/api/blob.bin")
        .expect("binary fetch must emit Network.responseReceived with a requestId");
    let binary = cdp(
        &mut ctx,
        3,
        "Network.getResponseBody",
        json!({"requestId": binary_request_id}),
        session_id,
    )
    .await;
    assert_eq!(binary.get("base64Encoded"), Some(&Value::Bool(true)));
    let encoded = binary.get("body").and_then(Value::as_str).unwrap();
    assert_eq!(BASE64.decode(encoded).unwrap(), [0u8, 128, 255, 16]);
}

#[tokio::test(flavor = "current_thread")]
async fn navigation_without_script_fetch_is_unaffected() {
    // A page that issues no script fetch must still emit exactly its document
    // request, proving the #406 change adds nothing spurious.
    std::env::set_var("OBSCURA_ALLOW_PRIVATE_NETWORK", "1");
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 1024];
        let _ = socket.read(&mut buf).await.unwrap();
        let body = "<html><body>plain</body></html>";
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = socket.write_all(resp.as_bytes()).await;
    });
    let base = format!("http://{addr}/");

    let mut ctx = CdpContext::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));
    let page_id = ctx.create_page();
    let session_id = "session-1";
    ctx.sessions.insert(session_id.to_string(), page_id.clone());
    cdp(&mut ctx, 0, "Network.enable", json!({}), session_id).await;

    cdp(&mut ctx, 1, "Page.navigate", json!({"url": base, "waitUntil": "load"}), session_id).await;

    let urls = drain_request_urls(&mut ctx);
    assert!(
        urls.iter().any(|u| u == &base || u.starts_with(&base)),
        "the document request must still be emitted; saw {urls:?}"
    );
    assert!(
        !urls.iter().any(|u| u.contains("/api/")),
        "no spurious script-fetch events for a page that makes none; saw {urls:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn network_post_data_projection_is_per_session_and_body_access_is_session_owned() {
    std::env::set_var("OBSCURA_ALLOW_PRIVATE_NETWORK", "1");
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        for _ in 0..4 {
            let (mut socket, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                let n = socket.read(&mut buf).await.unwrap();
                let request = String::from_utf8_lossy(&buf[..n]);
                let (content_type, body) = if request.starts_with("POST /submit") {
                    ("text/plain", "ok")
                } else if request.starts_with("GET /reset") {
                    ("text/html", r#"<script>
fetch('/submit-reset', {method:'POST', body:'ééé'})
  .then(() => document.body.dataset.reset = 'yes');
</script><body></body>"#)
                } else {
                    ("text/html", r#"<script>
fetch('/submit', {method:'POST', body:'ééé'})
  .then(() => document.body.dataset.done = 'yes');
</script><body></body>"#)
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.as_bytes().len(),
                );
                let _ = socket.write_all(response.as_bytes()).await;
            });
        }
    });

    let mut ctx = CdpContext::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));
    let page_id = ctx.create_page();
    let tight = "network-tight";
    let exact = "network-exact";
    ctx.sessions.insert(tight.to_string(), page_id.clone());
    ctx.sessions.insert(exact.to_string(), page_id.clone());
    cdp(&mut ctx, 0, "Network.enable", json!({"maxPostDataSize": 5}), tight).await;
    cdp(&mut ctx, 1, "Network.enable", json!({"maxPostDataSize": 6}), exact).await;
    cdp(&mut ctx, 2, "Page.navigate", json!({"url": format!("http://{addr}/"), "waitUntil": "networkidle0"}), tight).await;

    let post_url = format!("http://{addr}/submit");
    let starts = ctx.pending_events.iter().filter(|event| {
        event.method == "Network.requestWillBeSent"
            && event.params["request"]["url"] == post_url
    }).collect::<Vec<_>>();
    assert_eq!(starts.len(), 2);
    let tight_start = starts.iter().find(|event| event.session_id.as_deref() == Some(tight)).unwrap();
    let exact_start = starts.iter().find(|event| event.session_id.as_deref() == Some(exact)).unwrap();
    let request_id = tight_start.params["requestId"].as_str().unwrap().to_string();
    assert_eq!(exact_start.params["requestId"], request_id);
    assert!(tight_start.params["request"].get("postData").is_none());
    assert!(tight_start.params["request"].get("postDataEntries").is_none());
    assert_eq!(exact_start.params["request"]["postData"], "ééé");
    assert_eq!(exact_start.params["request"]["postDataEntries"][0]["bytes"], "w6nDqcOp");

    for session in [tight, exact] {
        let body = cdp(&mut ctx, 3, "Network.getRequestPostData", json!({"requestId": request_id}), session).await;
        assert_eq!(body, json!({"postData": "ééé", "base64Encoded": false}));
    }

    // Re-enable tight with defaults: only future projections change, and the
    // retained canonical body remains readable. Disable/re-enable exact must
    // not restore its old body capability.
    cdp(&mut ctx, 4, "Network.enable", json!({}), tight).await;
    cdp(&mut ctx, 5, "Network.disable", json!({}), exact).await;
    cdp(&mut ctx, 6, "Network.enable", json!({"maxPostDataSize": 6}), exact).await;
    let old_exact = dispatch(&CdpRequest { id: 7, method: "Network.getRequestPostData".into(), params: json!({"requestId": request_id}), session_id: Some(exact.into()) }, &mut ctx).await;
    assert!(old_exact.error.is_some());

    ctx.pending_events.clear();
    cdp(&mut ctx, 8, "Page.navigate", json!({
        "url": format!("http://{addr}/reset"), "waitUntil": "networkidle0"
    }), tight).await;
    let reset_url = format!("http://{addr}/submit-reset");
    let reset_starts = ctx.pending_events.iter().filter(|event| {
        event.method == "Network.requestWillBeSent"
            && event.params["request"]["url"] == reset_url
    }).collect::<Vec<_>>();
    assert_eq!(reset_starts.len(), 2);
    let reset_id = reset_starts[0].params["requestId"].as_str().unwrap().to_string();
    for event in &reset_starts {
        assert_eq!(event.params["requestId"], reset_id);
        assert_eq!(event.params["request"]["postData"], "ééé");
        assert_eq!(event.params["request"]["postDataEntries"][0]["bytes"], "w6nDqcOp");
    }
    for session in [tight, exact] {
        let body = cdp(&mut ctx, 9, "Network.getRequestPostData", json!({"requestId": reset_id}), session).await;
        assert_eq!(body, json!({"postData": "ééé", "base64Encoded": false}));
    }
}
