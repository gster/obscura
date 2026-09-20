use std::collections::HashMap;

use serde_json::{json, Value};

use crate::dispatch::CdpContext;

/// CDP ownership is attached at Fetch.enable, before Page/Worker requests enter
/// the shared connection channel. The embedded Page API keeps its local IDs.
pub struct RoutedInterceptedRequest {
    pub page_id: String,
    pub frame_id: String,
    pub session_id: Option<String>,
    pub request: obscura_js::ops::InterceptedRequest,
}

// A relay can be cancelled when the owning connection runtime exits. Never
// drop queued resolvers: the JS interception API treats a dropped sender as
// continuation, so cancellation must explicitly resolve Fail instead.
struct PendingIntercepts(tokio::sync::mpsc::UnboundedReceiver<obscura_js::ops::InterceptedRequest>);

impl Drop for PendingIntercepts {
    fn drop(&mut self) {
        self.0.close();
        while let Ok(request) = self.0.try_recv() {
            let _ = request.resolver.send(obscura_js::ops::InterceptResolution::Fail { reason: "Aborted".into() });
        }
    }
}

pub struct PausedRequest {
    pub request_id: String,
    pub url: String,
    pub method: String,
    pub headers: HashMap<String, String>,
    pub resource_type: String,
    pub resolver: tokio::sync::oneshot::Sender<FetchResolution>,
}

pub enum FetchResolution {
    Continue {
        url: Option<String>,
        method: Option<String>,
        headers: Option<Vec<(String, String)>>,
        post_data: Option<Vec<u8>>,
    },
    Fulfill {
        status: u16,
        headers: Vec<(String, String)>,
        body: String,
    },
    FulfillWithHeaders {
        status: u16,
        raw_headers: obscura_net::HeaderCapture,
        body: String,
        body_base64: String,
    },
    Fail {
        reason: String,
    },
}

pub struct FetchInterceptState {
    pub enabled: bool,
    pub patterns: Vec<String>,
    pub paused: HashMap<String, PausedRequest>,
    pub owners: HashMap<String, Option<String>>,
    request_counter: u64,
}

impl FetchInterceptState {
    pub fn new() -> Self {
        FetchInterceptState {
            enabled: false,
            patterns: Vec::new(),
            paused: HashMap::new(),
            owners: HashMap::new(),
            request_counter: 0,
        }
    }

    pub fn next_request_id(&mut self) -> String {
        self.request_counter += 1;
        format!("interception-{}", self.request_counter)
    }
}

pub async fn handle(
    method: &str,
    params: &Value,
    ctx: &mut CdpContext,
    session_id: &Option<String>,
) -> Result<Value, String> {
    match method {
        "enable" => {
            let patterns = params
                .get("patterns")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|p| {
                            p.get("urlPattern")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string())
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_else(|| vec!["*".to_string()]);

            let page_id = match session_id {
                Some(session) => ctx.sessions.get(session).cloned()
                    .ok_or_else(|| format!("No page found for sessionId {session}"))?,
                None if ctx.page_count() == 1 => ctx.single_page_id().unwrap().to_string(),
                None => return Err("Fetch.enable requires a sessionId unless exactly one Page exists".into()),
            };
            if ctx.get_page(&page_id).is_none() {
                return Err("Fetch.enable requires a Page session".into());
            }
            if ctx.fetch_intercept.owners.get(&page_id).is_some_and(|owner| owner != session_id) {
                return Err("Fetch is already enabled by another session on this Page".into());
            }
            ctx.fetch_intercept.owners.insert(page_id.clone(), session_id.clone());
            ctx.fetch_intercept.enabled = true;
            ctx.fetch_intercept.patterns = patterns.clone();
            let tx_clone = ctx.intercept_tx.clone();
            if let Some(page) = ctx.get_page_mut(&page_id) {
                page.intercept_block_patterns = patterns.clone();
                if let Some(tx) = tx_clone {
                    let (page_tx, page_rx) = tokio::sync::mpsc::unbounded_channel();
                    let mut pending = PendingIntercepts(page_rx);
                    let frame_id = page.frame_id.clone();
                    let session_id = session_id.clone();
                    tokio::spawn(async move {
                        while let Some(request) = pending.0.recv().await {
                            if let Err(error) = tx.send(RoutedInterceptedRequest {
                                page_id: page_id.clone(), frame_id: frame_id.clone(),
                                session_id: session_id.clone(), request,
                            }) {
                                let _ = error.0.request.resolver.send(obscura_js::ops::InterceptResolution::Fail { reason: "Aborted".into() });
                            }
                        }
                    });
                    page.set_intercept_tx(page_tx);
                }
                page.enable_intercept(true);
            }

            tracing::info!("Fetch interception enabled");
            Ok(json!({}))
        }
        "disable" => {
            let page_id = match session_id {
                Some(session) => ctx.sessions.get(session).cloned()
                    .ok_or_else(|| format!("No page found for sessionId {session}"))?,
                None if ctx.page_count() == 1 => ctx.single_page_id().unwrap().to_string(),
                None => return Err("Fetch.disable requires a sessionId unless exactly one Page exists".into()),
            };
            if ctx.fetch_intercept.owners.get(&page_id).is_some_and(|owner| owner != session_id)
                && session_id.is_some() {
                return Err("Fetch is enabled by another session on this Page".into());
            }
            ctx.fetch_intercept.owners.remove(&page_id);
            ctx.fetch_intercept.enabled = !ctx.fetch_intercept.owners.is_empty();
            if !ctx.fetch_intercept.enabled { ctx.fetch_intercept.patterns.clear(); }
            if let Some(page) = ctx.get_page_mut(&page_id) {
                page.intercept_block_patterns.clear();
                page.enable_intercept(false);
            }
            let paused: Vec<_> = ctx.fetch_intercept.paused.drain().collect();
            for (_, req) in paused {
                let _ = req.resolver.send(FetchResolution::Continue {
                    url: None,
                    method: None,
                    headers: None,
                    post_data: None,
                });
            }
            Ok(json!({}))
        }
        "continueRequest" => {
            let request_id = params
                .get("requestId")
                .and_then(|v| v.as_str())
                .ok_or("requestId required")?;

            let post_data = crate::server::parse_continue_post_data(params)?;
            let headers = crate::server::parse_cdp_headers(params)?;
            if let Some(paused) = ctx.fetch_intercept.paused.remove(request_id) {
                let _ = paused.resolver.send(FetchResolution::Continue {
                    url: params
                        .get("url")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    method: params
                        .get("method")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                    // Honor client header overrides (route.continue({ headers }))
                    // — parity with server.rs handle_fetch_resolution (#919).
                    headers,
                    post_data,
                });
            }
            Ok(json!({}))
        }
        "fulfillRequest" => {
            let request_id = params
                .get("requestId")
                .and_then(|v| v.as_str())
                .ok_or("requestId required")?;

            let obscura_js::ops::InterceptResolution::FulfillWithHeaders { status, raw_headers, body, body_base64, .. } =
                crate::server::parse_fulfill_resolution(params)? else { unreachable!("fulfill parser returns captured response") };

            if let Some(paused) = ctx.fetch_intercept.paused.remove(request_id) {
                let _ = paused.resolver.send(FetchResolution::FulfillWithHeaders {
                    status, raw_headers, body, body_base64,
                });
            }
            Ok(json!({}))
        }
        "failRequest" => {
            let request_id = params
                .get("requestId")
                .and_then(|v| v.as_str())
                .ok_or("requestId required")?;

            let reason = params
                .get("errorReason")
                .and_then(|v| v.as_str())
                .unwrap_or("Failed")
                .to_string();

            if let Some(paused) = ctx.fetch_intercept.paused.remove(request_id) {
                let _ = paused.resolver.send(FetchResolution::Fail { reason });
            }
            Ok(json!({}))
        }
        "getResponseBody" => {
            let request_id = params
                .get("requestId")
                .and_then(|v| v.as_str())
                .ok_or("Fetch.getResponseBody requires requestId")?;
            if ctx.fetch_intercept.paused.contains_key(request_id) {
                return Err(response_body_not_ready(request_id));
            }
            // Obscura accepts completed capture IDs, as takeResponseBodyAsStream
            // does. This is not response-stage interception: an active request
            // pause has no complete response body and must remain paused.
            super::network::get_response_body(ctx, session_id, request_id)
        },
        "takeResponseBodyAsStream" => {
            // Move raw storage into IO; file-backed bodies are read in chunks.
            let request_id = params
                .get("requestId")
                .and_then(|v| v.as_str())
                .ok_or("Fetch.takeResponseBodyAsStream requires requestId")?;

            if ctx.fetch_intercept.paused.contains_key(request_id) {
                return Err(response_body_not_ready(request_id));
            }
            let page = super::network::response_body_page(ctx, session_id, request_id)?;
            let page_id = page.id.clone();
            let size = page.response_body_size(request_id).unwrap()?;
            let page = ctx.pages.iter_mut().find(|page| page.id == page_id).unwrap();
            let reservation = ctx.io_streams.reserve(size)?;
            let bytes = page.take_response_body_result(request_id)
                .ok_or_else(|| format!("Fetch.takeResponseBodyAsStream: no cached body for {request_id}"))??;
            let handle = reservation.commit(bytes);
            Ok(json!({ "stream": handle }))
        }
        _ => Err(format!("Unknown Fetch method: {}", method)),
    }
}

pub(crate) fn response_body_not_ready(request_id: &str) -> String {
    format!("response_body_not_ready: request {request_id} is paused before the response")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::CdpContext;
    use serde_json::json;
    use std::collections::HashMap;

    fn pause(ctx: &mut CdpContext, id: &str) -> tokio::sync::oneshot::Receiver<FetchResolution> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        ctx.fetch_intercept.paused.insert(
            id.to_string(),
            PausedRequest {
                request_id: id.to_string(),
                url: "https://example.test/".to_string(),
                method: "GET".to_string(),
                headers: HashMap::new(),
                resource_type: "Fetch".to_string(),
                resolver: tx,
            },
        );
        rx
    }

    #[tokio::test(flavor = "current_thread")]
    async fn fulfilled_js_bodies_reach_network_and_fetch_without_transport_capture() {
        use base64::Engine as _;
        use obscura_js::ops::InterceptResolution;
        let large: Vec<u8> = (0..2 * 1024 * 1024 + 17).map(|i| (i % 256) as u8).collect();
        let invalid = vec![0xff, 0xe9, 0];
        let bodies = vec![large.clone(), invalid.clone(), Vec::new(), b"internal-text".to_vec(), b"redirect-body".to_vec()];
        let mut ctx = CdpContext::new();
        let page_id = ctx.create_page();
        let session = Some(format!("{page_id}-session"));
        ctx.sessions.insert(session.clone().unwrap(), page_id.clone());
        let page = ctx.get_page_mut(&page_id).unwrap();
        page.navigate("data:text/html,<html></html>").await.unwrap();
        page.network_events.clear();
        let mut requests = page.enable_interception();
        let mut pause_ids = Vec::new();
        let respond = async {
            for (index, bytes) in bodies.iter().enumerate() {
                let request = requests.recv().await.unwrap();
                pause_ids.push(request.request_id.clone());
                assert_eq!(request.method, if index == 0 { "POST" } else { "GET" });
                let legacy = InterceptResolution::Fulfill {
                    status: if index == 4 { 302 } else { 201 },
                    headers: HashMap::from([
                        ("Content-Type".into(), "application/octet-stream".into()),
                        ("Set-Cookie".into(), "session=complete-secret; HttpOnly".into()),
                        ("X-Complete".into(), "unaltered".into()),
                        ("Location".into(), "https://unused.test/redirect".into()),
                    ]),
                    body: String::from_utf8_lossy(bytes).into_owned(),
                    body_base64: if index == 3 { String::new() } else { base64::engine::general_purpose::STANDARD.encode(bytes) },
                };
                let resolution = if index == 0 {
                    let InterceptResolution::Fulfill { status, body_base64, .. } = legacy else { unreachable!() };
                    let fields = b"Set-Cookie: first=secret\0Set-Cookie: second=secret\0X-Bytes: \xff\xfe\0Content-Type: application/octet-stream\0X-Complete: unaltered\0";
                    crate::server::parse_fulfill_resolution(&json!({"responseCode": status, "body": body_base64,
                        "binaryResponseHeaders": base64::engine::general_purpose::STANDARD.encode(fields)})).unwrap()
                } else { legacy };
                request.resolver.send(resolution).unwrap();
            }
            requests.recv().await.unwrap().resolver.send(InterceptResolution::Fail { reason: "Failed".into() }).unwrap();
        };
        let evaluate = page.evaluate_for_cdp(r#"(async () => {
            const first = await fetch('https://synthetic.test/large', {method:'POST'});
            const binary = new Uint8Array(await first.arrayBuffer());
            const xhr = await new Promise((resolve, reject) => {
                const x = new XMLHttpRequest(); x.open('GET', 'https://synthetic.test/invalid');
                x.responseType = 'arraybuffer'; x.onload = () => resolve(Array.from(new Uint8Array(x.response)));
                x.onerror = reject; x.send();
            });
            const empty = await (await fetch('https://synthetic.test/empty')).text();
            const text = await (await fetch('https://synthetic.test/text')).text();
            const redirect = await fetch('https://synthetic.test/redirect');
            let failed; try { await fetch('https://synthetic.test/fail'); } catch(e) { failed = e.name; }
            return [binary.length, binary[0], binary[binary.length-1], xhr, empty, text, redirect.status, failed];
        })()"#, true, true);
        let (result, ()) = tokio::join!(evaluate, respond);
        assert!(!result.thrown, "{result:?}");
        assert_eq!(result.value, Some(json!([large.len(), 0, 16, invalid, "", "internal-text", 302, "AbortError"])));
        page.sync_js_network_events();
        let events: Vec<_> = page.network_events.drain(..).collect();
        assert_eq!(events.len(), bodies.len(), "Fail must not emit successful response observations");
        for (index, (event, bytes)) in events.iter().zip(&bodies).enumerate() {
            assert_eq!(event.status, if index == 4 { 302 } else { 201 });
            assert_eq!(event.method, if index == 0 { "POST" } else { "GET" });
            assert_eq!(event.resource_type, "Fetch");
            assert_eq!(event.body_size, bytes.len());
            assert!(event.url.starts_with("https://synthetic.test/"));
            if index == 0 {
                let capture = event.raw_headers.as_ref().unwrap();
                assert_eq!(capture.capture_stage, "cdpFulfillResponse");
                assert_eq!(capture.fields.len(), 5);
                assert_eq!(capture.fields[0].value, b"first=secret");
                assert_eq!(capture.fields[1].value, b"second=secret");
                assert_eq!(capture.fields[2].value, [0xff, 0xfe]);
            } else { assert!(event.raw_headers.is_none()); }
            assert!(event.request_raw_headers.is_none());
            if index == 0 {
                assert_eq!(event.response_headers["set-cookie"], "second=secret");
                assert_eq!(event.response_headers["x-complete"], "unaltered");
                assert!(!event.response_headers.contains_key("x-bytes"));
            } else {
                assert_eq!(event.response_headers["Set-Cookie"], "session=complete-secret; HttpOnly");
                assert_eq!(event.response_headers["X-Complete"], "unaltered");
            }
            let body = super::super::network::handle("getResponseBody", &json!({"requestId": event.request_id}), &mut ctx, &session).await.unwrap();
            let fetched = handle("getResponseBody", &json!({"requestId": event.request_id}), &mut ctx, &session).await.unwrap();
            assert_eq!(fetched, body);
            for _ in 0..2 {
                let alias = handle("getResponseBody", &json!({"requestId": pause_ids[index]}), &mut ctx, &session).await.unwrap();
                assert_eq!(alias, body);
            }
            let actual = if body["base64Encoded"] == true {
                base64::engine::general_purpose::STANDARD.decode(body["body"].as_str().unwrap()).unwrap()
            } else { body["body"].as_str().unwrap().as_bytes().to_vec() };
            assert_eq!(&actual, bytes);
        }
        let stream = handle("takeResponseBodyAsStream", &json!({"requestId": pause_ids[0]}), &mut ctx, &session).await.unwrap();
        for id in [&pause_ids[0], &events[0].request_id] {
            let error = handle("getResponseBody", &json!({"requestId": id}), &mut ctx, &session).await.unwrap_err();
            assert!(error.contains("response_body_already_consumed"), "{error}");
            let error = super::super::network::handle("getResponseBody", &json!({"requestId": id}), &mut ctx, &session).await.unwrap_err();
            assert!(error.contains("response_body_already_consumed"), "{error}");
        }
        super::super::io::handle("close", &json!({"handle": stream["stream"]}), &mut ctx).await.unwrap();
        let other_id = ctx.create_page();
        let other_session = Some(format!("{other_id}-session"));
        ctx.sessions.insert(other_session.clone().unwrap(), other_id.clone());
        let other = ctx.get_page_mut(&other_id).unwrap();
        other.navigate("data:text/html,<html></html>").await.unwrap();
        let mut other_requests = other.enable_interception();
        let respond = async {
            let request = other_requests.recv().await.unwrap();
            let id = request.request_id.clone();
            request.resolver.send(InterceptResolution::Fulfill { status: 200, headers: HashMap::new(), body: "other-page".into(), body_base64: String::new() }).unwrap();
            id
        };
        let (other_result, other_pause_id) = tokio::join!(other.evaluate_for_cdp("fetch('https://synthetic.test/other').then(r=>r.text())", true, true), respond);
        assert_eq!(other_result.value, Some(json!("other-page")));
        assert_eq!(other_pause_id, pause_ids[0], "fixture exercises identical IDs in separate Page stores");
        let body = handle("getResponseBody", &json!({"requestId": other_pause_id}), &mut ctx, &other_session).await.unwrap();
        assert_eq!(body["body"], "other-page");
        assert!(handle("getResponseBody", &json!({"requestId": other_pause_id}), &mut ctx, &session).await.unwrap_err().contains("response_body_already_consumed"));
        let page = ctx.get_page_mut(&page_id).unwrap();
        page.navigate("data:text/html,<html>rebuilt</html>").await.unwrap();
        page.network_events.clear();
        let respond = async {
            let request = requests.recv().await.unwrap();
            let id = request.request_id.clone();
            request.resolver.send(InterceptResolution::Fulfill { status: 200, headers: HashMap::new(), body: "after-navigation".into(), body_base64: String::new() }).unwrap();
            id
        };
        let (result, rebuilt_pause_id) = tokio::join!(page.evaluate_for_cdp("fetch('https://synthetic.test/rebuilt').then(r=>r.text())", true, true), respond);
        assert_eq!(result.value, Some(json!("after-navigation")));
        assert_ne!(rebuilt_pause_id, pause_ids[0], "pause IDs must not collide with retained aliases after navigation");
        let body = handle("getResponseBody", &json!({"requestId": rebuilt_pause_id}), &mut ctx, &session).await.unwrap();
        assert_eq!(body["body"], "after-navigation");
        assert!(handle("getResponseBody", &json!({"requestId": pause_ids[0]}), &mut ctx, &session).await.unwrap_err().contains("response_body_already_consumed"));
        let page = ctx.get_page_mut(&page_id).unwrap();
        page.set_response_body_limits(obscura_net::response_body::ResponseBodyLimits {
            memory_threshold: 0, total_bytes: 4, entries: 1,
        });
        let respond = async {
            let request = requests.recv().await.unwrap();
            let id = request.request_id.clone();
            request.resolver.send(InterceptResolution::Fulfill {
                status: 200, headers: HashMap::new(), body: "uncaptured".into(), body_base64: String::new(),
            }).unwrap();
            id
        };
        let (result, budget_pause_id) = tokio::join!(page.evaluate_for_cdp("fetch('https://synthetic.test/budget').then(r=>r.text())", true, true), respond);
        assert_eq!(result.value, Some(json!("uncaptured")));
        page.sync_js_network_events();
        let id = page.network_events.pop().unwrap().request_id;
        let error = super::super::network::handle("getResponseBody", &json!({"requestId": id}), &mut ctx, &session).await.unwrap_err();
        assert!(error.contains("response_body_budget_exhausted"), "{error}");
        let error = handle("getResponseBody", &json!({"requestId": id}), &mut ctx, &session).await.unwrap_err();
        assert!(error.contains("response_body_budget_exhausted"), "{error}");
        for method in ["getResponseBody", "takeResponseBodyAsStream"] {
            let error = handle(method, &json!({"requestId": budget_pause_id}), &mut ctx, &session).await.unwrap_err();
            assert!(error.contains("response_body_budget_exhausted"), "{error}");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn raw_body_store_js_module_cdp_reads_stream_drop_and_budget_errors() {
        use base64::Engine as _;
        use std::io::{Read, Write};
        use std::sync::Arc;
        let text = vec![b'x'; 2 * 1024 * 1024 + 31];
        let binary: Vec<u8> = (0..2 * 1024 * 1024 + 17).map(|i| (i % 256) as u8).collect();
        let mut module = b"export default 7; /*".to_vec();
        module.extend(vec![b'x'; 2 * 1024 * 1024]);
        module.extend_from_slice(b"\xff*/");
        let responses = vec![
            ("/", "text/html", b"<html></html>".to_vec()),
            ("/text", "text/plain", text.clone()),
            ("/binary", "application/octet-stream", binary.clone()),
            ("/module.js", "text/javascript", module.clone()),
            ("/budget", "text/plain", b"denied".to_vec()),
        ];
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let proxy = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            for (path, content_type, body) in responses {
                let (mut socket, _) = listener.accept().unwrap();
                socket.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
                let mut request = Vec::new();
                while !request.windows(4).any(|part| part == b"\r\n\r\n") {
                    let mut buffer = [0; 4096];
                    let count = socket.read(&mut buffer).unwrap();
                    assert!(count > 0); request.extend_from_slice(&buffer[..count]);
                }
                assert!(String::from_utf8_lossy(&request).starts_with(&format!("GET http://body.test{path} HTTP/1.1\r\n")));
                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).as_bytes()).unwrap();
                socket.write_all(&body).unwrap();
            }
        });
        let mut ctx = CdpContext::new();
        ctx.default_context = Arc::new(obscura_browser::BrowserContext::with_proxy("raw-body".into(), Some(proxy)));
        let page_id = ctx.create_page();
        let session = Some(format!("{page_id}-session"));
        ctx.sessions.insert(session.clone().unwrap(), page_id.clone());
        let page = ctx.get_page_mut(&page_id).unwrap();
        page.navigate("http://body.test/").await.unwrap();
        page.network_events.clear();
        let result = page.evaluate_for_cdp(r#"(async () => {
            const text = await (await fetch('/text')).text();
            const binary = await new Promise((resolve, reject) => {
                const xhr = new XMLHttpRequest(); xhr.open('GET', '/binary'); xhr.responseType = 'arraybuffer';
                xhr.onload = () => resolve(xhr.response.byteLength); xhr.onerror = reject; xhr.send();
            });
            const module = await import('/module.js');
            return [text.length, binary, module.default];
        })()"#, true, true).await;
        assert!(!result.thrown, "{result:?}");
        assert_eq!(result.value, Some(json!([text.len(), binary.len(), 7])));
        page.sync_js_network_events();
        let events: Vec<_> = page.network_events.drain(..).collect();
        assert_eq!(events.len(), 3);
        for (event, bytes) in events.iter().zip([&text, &binary, &module]) {
            assert_eq!(event.body_size, bytes.len());
            let value = super::super::network::handle("getResponseBody", &json!({"requestId": event.request_id}), &mut ctx, &session).await.unwrap();
            for _ in 0..2 {
                let fetched = handle("getResponseBody", &json!({"requestId": event.request_id}), &mut ctx, &session).await.unwrap();
                assert_eq!(fetched, value, "Fetch reads must be repeatable and match Network");
            }
            let actual = if value["base64Encoded"] == true {
                base64::engine::general_purpose::STANDARD.decode(value["body"].as_str().unwrap()).unwrap()
            } else { value["body"].as_str().unwrap().as_bytes().to_vec() };
            assert_eq!(&actual, bytes);
        }
        assert_eq!(events[2].resource_type, "Script");
        let request_id = &events[1].request_id;
        ctx.get_page_mut(&page_id).unwrap().alias_response_body(request_id, "js-alias");
        let result = handle("takeResponseBodyAsStream", &json!({"requestId": "js-alias"}), &mut ctx, &session).await.unwrap();
        let stream = result["stream"].as_str().unwrap();
        let error = super::super::network::handle("getResponseBody", &json!({"requestId": request_id}), &mut ctx, &session).await.unwrap_err();
        assert!(error.contains("response_body_already_consumed"), "{error}");
        let error = handle("getResponseBody", &json!({"requestId": "js-alias"}), &mut ctx, &session).await.unwrap_err();
        assert!(error.contains("response_body_already_consumed"), "{error}");
        let page = ctx.get_page_mut(&page_id).unwrap();
        page.set_response_body_limits(obscura_net::response_body::ResponseBodyLimits { memory_threshold: 0, total_bytes: 4, entries: 1 });
        let result = page.evaluate_for_cdp("fetch('/budget').then(r => r.text())", true, true).await;
        assert_eq!(result.value, Some(json!("denied")));
        page.sync_js_network_events();
        let rejected = page.network_events.last().unwrap().request_id.clone();
        let error = super::super::network::handle("getResponseBody", &json!({"requestId": rejected}), &mut ctx, &session).await.unwrap_err();
        assert!(error.contains("response_body_budget_exhausted"), "{error}");
        let error = handle("getResponseBody", &json!({"requestId": rejected}), &mut ctx, &session).await.unwrap_err();
        assert!(error.contains("response_body_budget_exhausted"), "{error}");
        let error = handle("takeResponseBodyAsStream", &json!({"requestId": rejected}), &mut ctx, &session).await.unwrap_err();
        assert!(error.contains("response_body_budget_exhausted"), "{error}");
        ctx.get_page_mut(&page_id).unwrap().clear_response_bodies();
        ctx.remove_page(&page_id);
        let mut received = Vec::new();
        loop {
            let result = super::super::io::handle("read", &json!({"handle": stream, "size": 131071}), &mut ctx).await.unwrap();
            received.extend(base64::engine::general_purpose::STANDARD.decode(result["data"].as_str().unwrap()).unwrap());
            if result["eof"] == true { break; }
        }
        assert_eq!(received, binary);
        super::super::io::handle("close", &json!({"handle": stream}), &mut ctx).await.unwrap();
        server.join().unwrap();
    }

    #[tokio::test]
    async fn response_body_stream_spool_is_once_and_survives_page_drop() {
        use base64::Engine as _;
        let mut ctx = CdpContext::new();
        let page_id = ctx.create_page();
        let session = Some(format!("{page_id}-session"));
        ctx.sessions.insert(session.clone().unwrap(), page_id.clone());
        let bytes: Vec<u8> = (0..2 * 1024 * 1024 + 19).map(|i| (i % 256) as u8).collect();
        let url = format!("data:application/octet-stream;base64,{}", base64::engine::general_purpose::STANDARD.encode(&bytes));
        let page = ctx.get_page_mut(&page_id).unwrap();
        page.navigate(&url).await.unwrap();
        let request_id = page.network_events.last().unwrap().request_id.clone();
        page.alias_response_body(&request_id, "loader");
        for id in [&request_id, "loader"] {
            for _ in 0..2 {
                let body = handle("getResponseBody", &json!({"requestId": id}), &mut ctx, &session).await.unwrap();
                assert_eq!(body["base64Encoded"], true);
                assert_eq!(base64::engine::general_purpose::STANDARD.decode(body["body"].as_str().unwrap()).unwrap(), bytes);
            }
        }
        let result = handle("takeResponseBodyAsStream", &json!({"requestId": "loader"}), &mut ctx, &session).await.unwrap();
        let stream = result["stream"].as_str().unwrap();
        for id in [&request_id, "loader"] {
            let error = handle("takeResponseBodyAsStream", &json!({"requestId": id}), &mut ctx, &session).await.unwrap_err();
            assert!(error.contains("response_body_already_consumed"), "{error}");
            let error = handle("getResponseBody", &json!({"requestId": id}), &mut ctx, &session).await.unwrap_err();
            assert!(error.contains("response_body_already_consumed"), "{error}");
            let error = super::super::network::handle("getResponseBody", &json!({"requestId": id}), &mut ctx, &session).await.unwrap_err();
            assert!(error.contains("response_body_already_consumed"), "{error}");
        }
        ctx.get_page_mut(&page_id).unwrap().clear_response_bodies();
        ctx.remove_page(&page_id);
        let mut received = Vec::new();
        loop {
            let result = super::super::io::handle("read", &json!({"handle": stream, "size": 131071}), &mut ctx).await.unwrap();
            received.extend(base64::engine::general_purpose::STANDARD.decode(result["data"].as_str().unwrap()).unwrap());
            if result["eof"] == true { break; }
        }
        assert_eq!(received, bytes);
        super::super::io::handle("close", &json!({"handle": stream}), &mut ctx).await.unwrap();
        assert!(super::super::io::handle("read", &json!({"handle": stream}), &mut ctx).await.is_err());
    }

    #[tokio::test]
    async fn response_body_stream_handle_exhaustion_does_not_consume_body() {
        let mut ctx = CdpContext::new();
        let page_id = ctx.create_page();
        let session = Some(format!("{page_id}-session"));
        ctx.sessions.insert(session.clone().unwrap(), page_id.clone());
        let page = ctx.get_page_mut(&page_id).unwrap();
        page.navigate("data:text/plain,retained").await.unwrap();
        let request_id = page.network_events.last().unwrap().request_id.clone();
        ctx.io_streams.set_handle_counter(u64::MAX);
        let error = handle("takeResponseBodyAsStream", &json!({"requestId": request_id}), &mut ctx, &session).await.unwrap_err();
        assert!(error.contains("handle space exhausted"), "{error}");
        let body = super::super::network::handle("getResponseBody", &json!({"requestId": request_id}), &mut ctx, &session).await.unwrap();
        assert_eq!(body["body"], "retained");
        ctx.io_streams.set_handle_counter(0);
        let result = handle("takeResponseBodyAsStream", &json!({"requestId": request_id}), &mut ctx, &session).await.unwrap();
        assert_eq!(result["stream"], "stream-0");
    }

    #[tokio::test]
    async fn response_body_stream_budget_failure_does_not_consume_page_body() {
        let mut ctx = CdpContext::new();
        let page_id = ctx.create_page();
        let session = Some(format!("{page_id}-session"));
        ctx.sessions.insert(session.clone().unwrap(), page_id.clone());
        ctx.io_streams = super::super::io::IoStreamStore::with_limits(0, 0);
        let page = ctx.get_page_mut(&page_id).unwrap();
        page.navigate("data:text/plain,hello").await.unwrap();
        let request_id = page.network_events.last().unwrap().request_id.clone();
        let error = handle("takeResponseBodyAsStream", &json!({"requestId": request_id}), &mut ctx, &session).await.unwrap_err();
        assert!(error.contains("io_stream_budget_exhausted"));
        let body = super::super::network::handle("getResponseBody", &json!({"requestId": request_id}), &mut ctx, &session).await.unwrap();
        assert_eq!(body["body"], "hello");
    }

    #[tokio::test]
    async fn get_response_body_requires_completed_capture_and_preserves_request_pause() {
        let mut ctx = CdpContext::new();
        let page_id = ctx.create_page();
        let session = Some(format!("{page_id}-session"));
        ctx.sessions.insert(session.clone().unwrap(), page_id.clone());
        let page = ctx.get_page_mut(&page_id).unwrap();
        page.navigate("data:text/plain,complete").await.unwrap();
        let request_id = page.network_events.last().unwrap().request_id.clone();
        let mut rx = pause(&mut ctx, &request_id);
        for method in ["getResponseBody", "takeResponseBodyAsStream"] {
            let error = handle(method, &json!({"requestId": request_id}), &mut ctx, &session).await.unwrap_err();
            assert!(error.contains("response_body_not_ready"), "{error}");
            assert!(ctx.fetch_intercept.paused.contains_key(&request_id));
            assert!(matches!(rx.try_recv(), Err(tokio::sync::oneshot::error::TryRecvError::Empty)));
        }
        handle("continueRequest", &json!({"requestId": request_id}), &mut ctx, &session).await.unwrap();
        assert!(matches!(rx.await.unwrap(), FetchResolution::Continue { .. }));
        let body = handle("getResponseBody", &json!({"requestId": request_id}), &mut ctx, &session).await.unwrap();
        assert_eq!(body, json!({"body": "complete", "base64Encoded": false}));
        let error = handle("getResponseBody", &json!({}), &mut ctx, &session).await.unwrap_err();
        assert_eq!(error, "Fetch.getResponseBody requires requestId");
        let error = handle("getResponseBody", &json!({"requestId": "unfinished-or-unknown"}), &mut ctx, &session).await.unwrap_err();
        assert!(error.contains("No response body found"), "{error}");
        ctx.get_page_mut(&page_id).unwrap().clear_response_bodies();
        assert!(handle("getResponseBody", &json!({"requestId": request_id}), &mut ctx, &session).await.is_err());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn response_body_reads_and_streams_are_session_scoped_with_overlapping_ids() {
        let mut ctx = CdpContext::new();
        let mut sessions = Vec::new();
        let mut ids = Vec::new();
        for text in ["first", "second"] {
            let page_id = ctx.create_page();
            let session = Some(format!("{page_id}-session"));
            ctx.sessions.insert(session.clone().unwrap(), page_id.clone());
            let page = ctx.get_page_mut(&page_id).unwrap();
            page.navigate(&format!("data:text/plain,{text}")).await.unwrap();
            let request_id = page.network_events.last().unwrap().request_id.clone();
            page.alias_response_body(&request_id, "shared-id");
            sessions.push(session);
            ids.push(request_id);
        }
        for (session, expected) in sessions.iter().zip(["first", "second"]) {
            let body = handle("getResponseBody", &json!({"requestId": "shared-id"}), &mut ctx, session).await.unwrap();
            assert_eq!(body["body"], expected);
        }
        let stream = handle("takeResponseBodyAsStream", &json!({"requestId": "shared-id"}), &mut ctx, &sessions[0]).await.unwrap();
        super::super::io::handle("close", &json!({"handle": stream["stream"]}), &mut ctx).await.unwrap();
        for method in ["getResponseBody", "takeResponseBodyAsStream"] {
            let error = handle(method, &json!({"requestId": "shared-id"}), &mut ctx, &sessions[0]).await.unwrap_err();
            assert!(error.contains("response_body_already_consumed"), "{error}");
            let error = handle(method, &json!({"requestId": ids[1]}), &mut ctx, &sessions[0]).await.unwrap_err();
            assert!(error.contains("No response body found"), "{error}");
            let error = handle(method, &json!({"requestId": "shared-id"}), &mut ctx, &Some("unknown-session".into())).await.unwrap_err();
            assert!(error.contains("No page found for sessionId"), "{error}");
        }
        let error = handle("getResponseBody", &json!({"requestId": "shared-id"}), &mut ctx, &None).await.unwrap_err();
        assert!(error.contains("Ambiguous requestId"), "{error}");
        let body = handle("getResponseBody", &json!({"requestId": "shared-id"}), &mut ctx, &sessions[1]).await.unwrap();
        assert_eq!(body["body"], "second");
    }

    // Parity with server.rs handle_fetch_resolution: continueRequest must
    // forward the client's header overrides (route.continue({ headers })), not
    // drop them. See #919.
    #[tokio::test]
    async fn continue_request_forwards_header_overrides() {
        let mut ctx = CdpContext::new();
        let rx = pause(&mut ctx, "req-1");
        handle(
            "continueRequest",
            &json!({ "requestId": "req-1", "headers": [{ "name": "X-Test", "value": "42" }] }),
            &mut ctx,
            &None,
        )
        .await
        .expect("continueRequest should succeed");

        match rx.await.expect("resolver should fire") {
            FetchResolution::Continue { headers, .. } => {
                let expected = vec![("X-Test".to_string(), "42".to_string())];
                assert_eq!(headers, Some(expected), "continue must forward header overrides");
            }
            _ => panic!("expected FetchResolution::Continue"),
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn continue_headers_domain_preserves_order_case_values_and_retry() {
        let mut ctx = CdpContext::new();
        for supplied in [crate::server::tests::continue_header_fields(), json!([])] {
            let mut rx = pause(&mut ctx, "continued");
            for invalid in crate::server::tests::malformed_continue_headers() {
                let request = serde_json::from_value(json!({"id":1,"method":"Fetch.continueRequest",
                    "params":{"requestId":"continued","headers":invalid}})).unwrap();
                let response = crate::dispatch::dispatch(&request, &mut ctx).await;
                assert_eq!(response.error.unwrap().code, -32602);
                assert!(ctx.fetch_intercept.paused.contains_key("continued"));
                assert!(matches!(rx.try_recv(), Err(tokio::sync::oneshot::error::TryRecvError::Empty)));
            }
            let request = serde_json::from_value(json!({"id":2,"method":"Fetch.continueRequest",
                "params":{"requestId":"continued","headers":supplied}})).unwrap();
            assert!(crate::dispatch::dispatch(&request, &mut ctx).await.error.is_none());
            let FetchResolution::Continue { headers, .. } = rx.try_recv().unwrap() else { panic!("expected continue") };
            let roundtrip: Vec<_> = headers.unwrap().into_iter().map(|(name, value)| json!({"name":name,"value":value})).collect();
            assert_eq!(json!(roundtrip), supplied);
            assert!(ctx.fetch_intercept.paused.is_empty());
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn continue_post_data_domain_preserves_bytes_and_retryable_errors() {
        let mut ctx = CdpContext::new();
        for (value, expected) in [(Some(json!("AP8A/w==")), Some(vec![0, 255, 0, 255])),
            (Some(json!("")), Some(vec![])), (None, None)] {
            let mut rx = pause(&mut ctx, "continued");
            for invalid in [json!("%"), json!("AP8"), json!("AP8=\n"), json!("AP9="), json!(7), json!(null), json!("%") ] {
                let request = serde_json::from_value(json!({"id":1,"method":"Fetch.continueRequest",
                    "params":{"requestId":"continued","postData":invalid}})).unwrap();
                let response = crate::dispatch::dispatch(&request, &mut ctx).await;
                assert_eq!(response.error.unwrap().code, -32602);
                assert!(ctx.fetch_intercept.paused.contains_key("continued"));
                assert!(matches!(rx.try_recv(), Err(tokio::sync::oneshot::error::TryRecvError::Empty)));
            }
            let mut params = json!({"requestId":"continued","url":"https://example.com/new","method":"PUT",
                "headers":[{"name":"Authorization","value":"Bearer complete-secret"}]});
            if let Some(value) = value { params["postData"] = value; }
            let request = serde_json::from_value(json!({"id":2,"method":"Fetch.continueRequest","params":params})).unwrap();
            assert!(crate::dispatch::dispatch(&request, &mut ctx).await.error.is_none());
            let FetchResolution::Continue { post_data, url, method, headers } = rx.try_recv().unwrap() else { panic!("expected continue") };
            assert_eq!(post_data, expected);
            assert_eq!(url.as_deref(), Some("https://example.com/new"));
            assert_eq!(method.as_deref(), Some("PUT"));
            assert_eq!(headers.unwrap(), vec![("Authorization".into(), "Bearer complete-secret".into())]);
            assert!(ctx.fetch_intercept.paused.is_empty());
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn fulfilled_domain_retains_binary_headers_and_retryable_parse_errors() {
        use base64::Engine as _;
        let mut ctx = CdpContext::new();
        let rx = pause(&mut ctx, "binary-headers");
        for params in [json!({"requestId":"binary-headers","binaryResponseHeaders":"%"}), json!({"requestId":"binary-headers","body":"%"})] {
            let result = handle("fulfillRequest", &params, &mut ctx, &None).await;
            assert!(result.is_err());
            assert!(ctx.fetch_intercept.paused.contains_key("binary-headers"));
        }
        let headers = b"Set-Cookie: a=full\0Set-Cookie: b=full\0X-Bytes: \xff\0";
        handle("fulfillRequest", &json!({"requestId":"binary-headers","body":"AP8=",
            "binaryResponseHeaders":base64::engine::general_purpose::STANDARD.encode(headers)}), &mut ctx, &None).await.unwrap();
        let FetchResolution::FulfillWithHeaders { raw_headers, body_base64, .. } = rx.await.unwrap() else { panic!("missing raw fulfill") };
        assert_eq!(body_base64, "AP8=");
        assert_eq!(raw_headers.capture_stage, "cdpFulfillResponse");
        assert_eq!(raw_headers.fields.len(), 3);
        assert_eq!(raw_headers.fields[0].value, b"a=full");
        assert_eq!(raw_headers.fields[1].value, b"b=full");
        assert_eq!(raw_headers.fields[2].value, [0xff]);
    }

    // Parity with server.rs: the fulfillRequest body is base64-encoded per CDP
    // and must be decoded, not passed through as raw base64 text. See #919/#912.
    #[tokio::test]
    async fn fulfill_request_base64_decodes_body() {
        let mut ctx = CdpContext::new();
        let rx = pause(&mut ctx, "req-2");
        handle(
            "fulfillRequest",
            &json!({ "requestId": "req-2", "responseCode": 200, "body": "SGVsbG8=" }),
            &mut ctx,
            &None,
        )
        .await
        .expect("fulfillRequest should succeed");

        match rx.await.expect("resolver should fire") {
            FetchResolution::FulfillWithHeaders { body, body_base64, raw_headers, .. } => {
                assert_eq!(body, "Hello", "fulfill body must be base64-decoded");
                assert_eq!(body_base64, "SGVsbG8=");
                assert_eq!(raw_headers.capture_stage, "cdpFulfillResponse");
            }
            _ => panic!("expected FetchResolution::Fulfill"),
        }
    }
}

#[cfg(test)]
mod relay_cleanup_tests {
    use super::*;

    #[test]
    fn cancelled_pause_relay_aborts_every_queued_resolver() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let pending = PendingIntercepts(rx);
        let mut receivers = Vec::new();
        for id in ["intercept-1", "intercept-2"] {
            let (resolver, receiver) = tokio::sync::oneshot::channel();
            tx.send(obscura_js::ops::InterceptedRequest {
                request_id: id.into(), url: "https://example.test/".into(), method: "GET".into(),
                headers: HashMap::new(), resource_type: "Fetch".into(), resolver,
            }).unwrap();
            receivers.push(receiver);
        }
        drop(pending);
        for mut receiver in receivers {
            assert!(matches!(receiver.try_recv(), Ok(obscura_js::ops::InterceptResolution::Fail { reason }) if reason == "Aborted"));
        }
    }
}
