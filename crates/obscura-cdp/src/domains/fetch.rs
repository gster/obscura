use std::collections::HashMap;

use serde_json::{json, Value};

use crate::dispatch::CdpContext;

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
        headers: Option<HashMap<String, String>>,
        post_data: Option<String>,
    },
    Fulfill {
        status: u16,
        headers: Vec<(String, String)>,
        body: String,
    },
    Fail {
        reason: String,
    },
}

pub struct FetchInterceptState {
    pub enabled: bool,
    pub patterns: Vec<String>,
    pub paused: HashMap<String, PausedRequest>,
    request_counter: u64,
}

impl FetchInterceptState {
    pub fn new() -> Self {
        FetchInterceptState {
            enabled: false,
            patterns: Vec::new(),
            paused: HashMap::new(),
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

            ctx.fetch_intercept.enabled = true;
            ctx.fetch_intercept.patterns = patterns.clone();
            let tx_clone = ctx.intercept_tx.clone();
            if let Some(page) = ctx.get_session_page_mut(session_id) {
                page.intercept_block_patterns = patterns.clone();
                if let Some(tx) = tx_clone {
                    page.set_intercept_tx(tx);
                }
                page.enable_intercept(true);
            }

            tracing::info!("Fetch interception enabled");
            Ok(json!({}))
        }
        "disable" => {
            ctx.fetch_intercept.enabled = false;
            ctx.fetch_intercept.patterns.clear();
            if let Some(page) = ctx.get_session_page_mut(session_id) {
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
                    headers: crate::server::parse_cdp_headers(params),
                    post_data: params
                        .get("postData")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                });
            }
            Ok(json!({}))
        }
        "fulfillRequest" => {
            let request_id = params
                .get("requestId")
                .and_then(|v| v.as_str())
                .ok_or("requestId required")?;

            let status = params
                .get("responseCode")
                .and_then(|v| v.as_u64())
                .unwrap_or(200) as u16;
            let headers: HashMap<String, String> = params
                .get("responseHeaders")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|h| {
                            let name = h.get("name")?.as_str()?.to_string();
                            let value = h.get("value")?.as_str()?.to_string();
                            Some((name, value))
                        })
                        .collect()
                })
                .unwrap_or_default();
            // The CDP fulfillRequest body is base64-encoded; decode it — parity
            // with server.rs handle_fetch_resolution (#919). (Binary-safe body
            // transport across the JS boundary remains tracked in #912.)
            let body =
                crate::server::decode_base64(params.get("body").and_then(|v| v.as_str()).unwrap_or(""));

            if let Some(paused) = ctx.fetch_intercept.paused.remove(request_id) {
                let _ = paused.resolver.send(FetchResolution::Fulfill {
                    status,
                    headers: headers.into_iter().collect(),
                    body,
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
        "getResponseBody" => Ok(json!({ "body": "", "base64Encoded": false })),
        "takeResponseBodyAsStream" => {
            // Move raw storage into IO; file-backed bodies are read in chunks.
            let request_id = params
                .get("requestId")
                .and_then(|v| v.as_str())
                .ok_or("Fetch.takeResponseBodyAsStream requires requestId")?;

            let page = ctx.get_session_page(session_id);
            let mut diagnostic = None;
            let (page_id, size) = page.into_iter().chain(ctx.pages.iter()).find_map(|page| {
                match page.response_body_size(request_id)? {
                    Ok(size) => Some((page.id.clone(), size)),
                    Err(error) => { diagnostic.get_or_insert(error); None },
                }
            }).ok_or_else(|| diagnostic.unwrap_or_else(|| {
                format!("Fetch.takeResponseBodyAsStream: no cached body for {request_id}")
            }))?;
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
        let result = handle("takeResponseBodyAsStream", &json!({"requestId": "loader"}), &mut ctx, &session).await.unwrap();
        let stream = result["stream"].as_str().unwrap();
        for id in [&request_id, "loader"] {
            let error = handle("takeResponseBodyAsStream", &json!({"requestId": id}), &mut ctx, &session).await.unwrap_err();
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
                let mut expected = HashMap::new();
                expected.insert("X-Test".to_string(), "42".to_string());
                assert_eq!(headers, Some(expected), "continue must forward header overrides");
            }
            _ => panic!("expected FetchResolution::Continue"),
        }
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
            FetchResolution::Fulfill { body, .. } => {
                assert_eq!(body, "Hello", "fulfill body must be base64-decoded");
            }
            _ => panic!("expected FetchResolution::Fulfill"),
        }
    }
}
