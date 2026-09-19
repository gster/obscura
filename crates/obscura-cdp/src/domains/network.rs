use std::sync::Arc;

use serde_json::{json, Value};

use obscura_net::CookieJar;

use crate::cookie_params::{parse_cdp_cookie, parse_delete_cookies_params};
use crate::dispatch::CdpContext;

const SESSION_COOKIE_EXPIRES: i64 = -1;
const DEFAULT_SECURE_PORT: u16 = 443;
const DEFAULT_INSECURE_PORT: u16 = 80;
const SOURCE_SCHEME_SECURE: &str = "Secure";
const SOURCE_SCHEME_NONSECURE: &str = "NonSecure";
const DEFAULT_SAME_SITE: &str = "Lax";

// Resolve the cookie jar for a Network request: prefer the session's page jar,
// fall back to the default browser context. Puppeteer and Playwright both call
// Network.setCookie/getCookies/deleteCookies BEFORE attaching to a target —
// requiring a session would break those flows (Storage.* already mirrors this).
fn cookie_jar_for<'a>(ctx: &'a CdpContext, session_id: &Option<String>) -> &'a Arc<CookieJar> {
    ctx.get_session_page(session_id)
        .map(|p| &p.context.cookie_jar)
        .unwrap_or(&ctx.default_context.cookie_jar)
}

pub async fn handle(
    method: &str,
    params: &Value,
    ctx: &mut CdpContext,
    session_id: &Option<String>,
) -> Result<Value, String> {
    match method {
        "enable" => {
            if !(params.is_null()
                || params.as_object().is_some_and(serde_json::Map::is_empty))
            {
                return Err("Network.enable supports only empty params".to_string());
            }
            Ok(json!({}))
        }
        "disable" => {
            if let Some(page) = ctx.get_session_page_mut(session_id) {
                page.clear_response_bodies();
            } else {
                for page in &mut ctx.pages {
                    page.clear_response_bodies();
                }
            }
            Ok(json!({}))
        }
        "setExtraHTTPHeaders" => {
            let headers = params.get("headers").and_then(|v| v.as_object());
            if let Some(page) = ctx.get_session_page(session_id) {
                if let Some(headers) = headers {
                    if let Some(name) = headers.keys().find(|name| {
                        let name = name.to_ascii_lowercase();
                        matches!(
                            name.as_str(),
                            "user-agent" | "accept-language" | "accept-encoding" | "dnt"
                        ) || name.starts_with("sec-ch-ua")
                    }) {
                        return Err(format!(
                            "Network.setExtraHTTPHeaders cannot override persona-owned header {name}"
                        ));
                    }
                    let header_map: std::collections::HashMap<String, String> = headers
                        .iter()
                        .map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string()))
                        .collect();
                    page.stealth_client.set_extra_headers(header_map).await;
                }
            }
            Ok(json!({}))
        }
        "setUserAgentOverride" => {
            let _user_agent = params
                .get("userAgent")
                .and_then(Value::as_str)
                .ok_or("Network.setUserAgentOverride requires a string userAgent")?;
            Err(
                "Network.setUserAgentOverride is unsupported after BrowserContext initialization; configure the immutable browser persona when creating the context"
                    .to_string(),
            )
        }
        "getCookies" | "getAllCookies" => {
            let cookies = cookie_jar_for(ctx, session_id).get_all_cookies();
            let cdp_cookies: Vec<Value> = cookies.iter().map(cookie_info_to_cdp_json).collect();
            Ok(json!({ "cookies": cdp_cookies }))
        }
        "setCookie" => {
            let cookie = parse_cdp_cookie(params)
                .ok_or("setCookie: missing required name/domain (or url)")?;
            cookie_jar_for(ctx, session_id).set_cookies_from_cdp(vec![cookie]);
            Ok(json!({ "success": true }))
        }
        "setCookies" => {
            if let Some(cookies) = params.get("cookies").and_then(|v| v.as_array()) {
                let parsed: Vec<_> = cookies.iter().filter_map(parse_cdp_cookie).collect();
                cookie_jar_for(ctx, session_id).set_cookies_from_cdp(parsed);
            }
            Ok(json!({}))
        }
        "deleteCookies" => {
            if let Some(filter) = parse_delete_cookies_params(params) {
                cookie_jar_for(ctx, session_id).delete_cookies_filtered(
                    &filter.name,
                    &filter.domain,
                    filter.path.as_deref(),
                );
            }
            Ok(json!({}))
        }
        "clearBrowserCookies" => {
            cookie_jar_for(ctx, session_id).clear();
            Ok(json!({}))
        }
        "setCacheDisabled" => Ok(json!({})),
        "setRequestInterception" => Ok(json!({})),
        "setBlockedURLs" => {
            let patterns = params
                .get("urls")
                .and_then(|value| value.as_array())
                .map(|values| {
                    values
                        .iter()
                        .filter_map(|value| value.as_str().map(ToString::to_string))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();

            if let Some(page) = ctx.get_session_page_mut(session_id) {
                page.set_blocked_urls(patterns);
            } else {
                for page in &mut ctx.pages {
                    page.set_blocked_urls(patterns.clone());
                }
            }
            Ok(json!({}))
        }
        "getResponseBody" => {
            let request_id = params
                .get("requestId")
                .and_then(|v| v.as_str())
                .ok_or("Network.getResponseBody requires requestId")?;

            get_response_body(ctx, session_id, request_id)
        }
        _ => Err(format!("Unknown Network method: {}", method)),
    }
}

/// Read a completed capture without consuming its shared raw storage.
/// Fetch uses the same lookup and protocol encoding as Network.
pub(super) fn get_response_body(
    ctx: &CdpContext,
    session_id: &Option<String>,
    request_id: &str,
) -> Result<Value, String> {
    let body = response_body_page(ctx, session_id, request_id)?
        .get_response_body_result(request_id)
        .ok_or_else(|| format!("No response body found for requestId {request_id}"))??;
    Ok(json!({ "body": body.body, "base64Encoded": body.base64_encoded }))
}

/// A session never falls through to another Page, even if IDs overlap or its
/// capture failed. Sessionless reads search all Pages, retaining diagnostics.
pub(super) fn response_body_page<'a>(
    ctx: &'a CdpContext,
    session_id: &Option<String>,
    request_id: &str,
) -> Result<&'a obscura_browser::Page, String> {
    let missing = || format!("No response body found for requestId {request_id}");
    if let Some(session) = session_id {
        let page = ctx.get_session_page(session_id)
            .ok_or_else(|| format!("No page found for sessionId {session}"))?;
        page.response_body_size(request_id).ok_or_else(missing)??;
        return Ok(page);
    }
    let mut diagnostic = None;
    let found = ctx.pages.iter().find(|page| {
        match page.response_body_size(request_id) {
            Some(Ok(_)) => true,
            Some(Err(error)) => { diagnostic.get_or_insert(error); false },
            None => false,
        }
    });
    found.ok_or_else(|| diagnostic.unwrap_or_else(missing))
}

#[cfg(test)]
mod tests {
    use super::*;
    use obscura_net::CookieInfo;

    fn sample_cookie(name: &str) -> CookieInfo {
        CookieInfo {
            name: name.to_string(),
            value: "v".to_string(),
            domain: "example.com".to_string(),
            path: "/".to_string(),
            secure: false,
            http_only: false,
            same_site: String::new(),
            expires: None,
        }
    }

    #[tokio::test]
    async fn set_cookie_without_session_targets_default_context() {
        let mut ctx = CdpContext::new();
        let params = json!({
            "name": "sid",
            "value": "abc",
            "domain": "example.com",
            "path": "/"
        });
        let resp = handle("setCookie", &params, &mut ctx, &None)
            .await
            .expect("setCookie must succeed without a session");
        assert_eq!(resp["success"], json!(true));
        let cookies = ctx.default_context.cookie_jar.get_all_cookies();
        assert_eq!(cookies.len(), 1, "default cookie jar must receive the cookie");
        assert_eq!(cookies[0].name, "sid");
    }

    #[tokio::test]
    async fn set_cookies_without_session_targets_default_context() {
        let mut ctx = CdpContext::new();
        let params = json!({
            "cookies": [
                { "name": "a", "value": "1", "domain": "example.com", "path": "/" },
                { "name": "b", "value": "2", "domain": "example.com", "path": "/" }
            ]
        });
        handle("setCookies", &params, &mut ctx, &None)
            .await
            .expect("setCookies must succeed without a session");
        assert_eq!(ctx.default_context.cookie_jar.get_all_cookies().len(), 2);
    }

    #[tokio::test]
    async fn delete_cookies_without_session_targets_default_context() {
        let mut ctx = CdpContext::new();
        ctx.default_context
            .cookie_jar
            .set_cookies_from_cdp(vec![sample_cookie("sid")]);
        let params = json!({ "name": "sid", "domain": "example.com" });
        handle("deleteCookies", &params, &mut ctx, &None)
            .await
            .expect("deleteCookies must succeed without a session");
        assert!(ctx.default_context.cookie_jar.get_all_cookies().is_empty());
    }

    #[tokio::test]
    async fn get_all_cookies_returns_every_cookie_in_jar() {
        let mut ctx = CdpContext::new();
        ctx.default_context.cookie_jar.set_cookies_from_cdp(vec![
            sample_cookie("a"),
            sample_cookie("b"),
        ]);
        let resp = handle("getAllCookies", &json!({}), &mut ctx, &None)
            .await
            .expect("getAllCookies must succeed");
        let arr = resp["cookies"].as_array().expect("cookies array");
        assert_eq!(arr.len(), 2);
    }

    #[tokio::test]
    async fn get_cookies_falls_back_to_default_context_when_no_session() {
        let mut ctx = CdpContext::new();
        ctx.default_context
            .cookie_jar
            .set_cookies_from_cdp(vec![sample_cookie("sid")]);
        let resp = handle("getCookies", &json!({}), &mut ctx, &None)
            .await
            .expect("getCookies must succeed without a session");
        let arr = resp["cookies"].as_array().expect("cookies array");
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["name"], "sid");
    }

    #[tokio::test]
    async fn clear_browser_cookies_without_session_clears_default_context() {
        let mut ctx = CdpContext::new();
        ctx.default_context
            .cookie_jar
            .set_cookies_from_cdp(vec![sample_cookie("sid")]);
        handle("clearBrowserCookies", &json!({}), &mut ctx, &None)
            .await
            .expect("clearBrowserCookies must succeed");
        assert!(ctx.default_context.cookie_jar.get_all_cookies().is_empty());
    }

    #[tokio::test]
    async fn set_blocked_urls_targets_session_page_without_enabling_interception() {
        let mut ctx = CdpContext::new();
        let page_id = ctx.create_page();
        let session_id = Some("session-1".to_string());
        ctx.sessions.insert(session_id.clone().unwrap(), page_id.clone());

        handle(
            "setBlockedURLs",
            &json!({
                "urls": [
                    "*://*.example.com/*.png",
                    "*://cdn.example.com/*"
                ]
            }),
            &mut ctx,
            &session_id,
        )
        .await
        .expect("setBlockedURLs must succeed for a session page");

        let page = ctx.get_page(&page_id).unwrap();
        assert_eq!(
            page.blocked_url_patterns,
            vec![
                "*://*.example.com/*.png".to_string(),
                "*://cdn.example.com/*".to_string(),
            ]
        );
        assert!(!page.intercept_enabled);
        assert!(page.intercept_block_patterns.is_empty());
    }

    #[tokio::test]
    async fn set_blocked_urls_without_session_updates_existing_pages() {
        let mut ctx = CdpContext::new();
        let left = ctx.create_page();
        let right = ctx.create_page();

        handle(
            "setBlockedURLs",
            &json!({ "urls": ["*://tiles.example.test/*"] }),
            &mut ctx,
            &None,
        )
        .await
        .expect("setBlockedURLs must succeed without a session");

        assert_eq!(
            ctx.get_page(&left).unwrap().blocked_url_patterns,
            vec!["*://tiles.example.test/*".to_string()]
        );
        assert_eq!(
            ctx.get_page(&right).unwrap().blocked_url_patterns,
            vec!["*://tiles.example.test/*".to_string()]
        );
    }

    #[tokio::test]
    async fn set_blocked_urls_replaces_existing_patterns() {
        let mut ctx = CdpContext::new();
        let page_id = ctx.create_page();
        let session_id = Some("session-1".to_string());
        ctx.sessions.insert(session_id.clone().unwrap(), page_id.clone());

        handle(
            "setBlockedURLs",
            &json!({ "urls": ["*://old.example.test/*"] }),
            &mut ctx,
            &session_id,
        )
        .await
        .unwrap();
        handle(
            "setBlockedURLs",
            &json!({ "urls": ["*://new.example.test/*"] }),
            &mut ctx,
            &session_id,
        )
        .await
        .unwrap();

        assert_eq!(
            ctx.get_page(&page_id).unwrap().blocked_url_patterns,
            vec!["*://new.example.test/*".to_string()]
        );
    }

    #[tokio::test]
    async fn get_response_body_returns_stored_document_body() {
        let mut ctx = CdpContext::new();
        let page_id = ctx.create_page();
        let session_id = Some("session-1".to_string());
        ctx.sessions.insert(session_id.clone().unwrap(), page_id.clone());

        let page = ctx.get_page_mut(&page_id).unwrap();
        page.navigate("data:text/html,<html><body>hello body</body></html>")
            .await
            .unwrap();
        let request_id = page.network_events[0].request_id.clone();

        let result = handle(
            "getResponseBody",
            &json!({ "requestId": request_id }),
            &mut ctx,
            &session_id,
        )
        .await
        .unwrap();

        assert_eq!(result["body"], "<html><body>hello body</body></html>");
        assert_eq!(result["base64Encoded"], false);
    }

    #[tokio::test]
    async fn response_body_large_text_binary_and_legacy_text_round_trip() {
        use base64::Engine as _;
        let mut ctx = CdpContext::new();
        let page_id = ctx.create_page();
        let session = Some(format!("{page_id}-session"));
        ctx.sessions.insert(session.clone().unwrap(), page_id.clone());
        let cases = [
            ("text/plain", vec![b'x'; 2 * 1024 * 1024 + 31], false),
            ("application/octet-stream", (0..2 * 1024 * 1024 + 17).map(|i| (i % 256) as u8).collect(), true),
            ("text/plain;charset=windows-1252", vec![0xff, 0xe9, 0, b'a'], true),
        ];
        for (mime, bytes, binary) in cases {
            let url = format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(&bytes));
            let page = ctx.get_page_mut(&page_id).unwrap();
            page.navigate(&url).await.unwrap();
            let request_id = page.network_events.last().unwrap().request_id.clone();
            let result = handle("getResponseBody", &json!({"requestId": request_id}), &mut ctx, &session).await.unwrap();
            assert_eq!(result["base64Encoded"], binary);
            let returned = result["body"].as_str().unwrap();
            if binary {
                assert_eq!(base64::engine::general_purpose::STANDARD.decode(returned).unwrap(), bytes);
            } else {
                assert_eq!(returned.as_bytes(), bytes);
            }
        }
    }

    #[cfg(feature = "render")]
    #[tokio::test(flavor = "current_thread")]
    async fn response_body_page_owned_subresources_reach_cdp_byte_exact() {
        use base64::Engine as _;
        use std::io::{Read, Write};
        use std::sync::Arc;
        let padding = "x".repeat(2 * 1024 * 1024 + 1);
        let resources: std::collections::HashMap<_, _> = [
            ("/", ("text/html", br#"<link rel="stylesheet" href="/style.css"><link rel="preload" as="font" href="/font.woff"><script src="/script.js"></script><body>probe<img src="/image.png"></body>"#.to_vec(), "Document")),
            ("/style.css", ("text/css", format!("@font-face{{font-family:Probe;src:url('/font.woff')}}body{{font-family:Probe}}/*{padding}*/").into_bytes(), "Stylesheet")),
            ("/script.js", ("application/javascript", format!("/*{padding}*/globalThis.captureScriptRan=true;").into_bytes(), "Script")),
            ("/image.png", ("image/png", vec![0x89; padding.len()], "Image")),
            ("/font.woff", ("font/woff", vec![0xff; padding.len()], "Font")),
        ].into_iter().collect();
        let resources = Arc::new(resources);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let served = resources.clone();
        let server = std::thread::spawn(move || {
            for _ in 0..served.len() {
                let (mut stream, _) = listener.accept().unwrap();
                stream.set_read_timeout(Some(std::time::Duration::from_secs(10))).unwrap();
                let mut request = Vec::new();
                while !request.windows(4).any(|part| part == b"\r\n\r\n") {
                    let mut chunk = [0; 4096];
                    let len = stream.read(&mut chunk).unwrap();
                    assert!(len > 0);
                    request.extend_from_slice(&chunk[..len]);
                }
                let request = String::from_utf8(request).unwrap();
                let path = request.split_whitespace().nth(1).unwrap();
                let (mime, body, _) = served.get(path).unwrap();
                stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nSet-Cookie: retained=Raw+/=123; Path=/\r\nConnection: close\r\n\r\n", body.len()).as_bytes()).unwrap();
                stream.write_all(body).unwrap();
            }
        });
        let mut ctx = CdpContext::new();
        ctx.default_context = Arc::new(obscura_browser::BrowserContext::with_storage_and_network(
            "body-resources".into(), None, false, None, None, true,
        ));
        let page_id = ctx.create_page();
        let session = Some(format!("{page_id}-session"));
        ctx.sessions.insert(session.clone().unwrap(), page_id.clone());
        let page = ctx.get_page_mut(&page_id).unwrap();
        page.navigate(&format!("{origin}/")).await.unwrap();
        page.prepare_screenshot_resources(3000).await;
        let events = page.network_events.clone();
        for (path, (_, expected, resource_type)) in resources.iter() {
            let event = events.iter().find(|event| event.url == format!("{origin}{path}"))
                .unwrap_or_else(|| panic!("missing {resource_type} event"));
            assert_eq!(&event.resource_type, resource_type);
            assert_eq!(event.body_size, expected.len());
            assert_eq!(event.response_headers["set-cookie"], "retained=Raw+/=123; Path=/");
            let result = handle("getResponseBody", &json!({"requestId": event.request_id}), &mut ctx, &session).await.unwrap();
            let body = result["body"].as_str().unwrap();
            let actual = if result["base64Encoded"] == true {
                base64::engine::general_purpose::STANDARD.decode(body).unwrap()
            } else { body.as_bytes().to_vec() };
            assert_eq!(&actual, expected, "{resource_type} body");
        }
        server.join().unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn response_body_exhausted_page_does_not_hide_another_pages_body() {
        use base64::Engine as _;
        let mut ctx = CdpContext::new();
        let exhausted = ctx.create_page();
        let exhausted_session = Some(format!("{exhausted}-session"));
        ctx.sessions.insert(exhausted_session.clone().unwrap(), exhausted.clone());
        let page = ctx.get_page_mut(&exhausted).unwrap();
        page.set_response_body_limits(obscura_net::response_body::ResponseBodyLimits {
            memory_threshold: 0, total_bytes: 0, entries: 16,
        });
        page.navigate("data:text/plain,rejected").await.unwrap();
        let owner = ctx.create_page();
        for _ in 0..2 {
            let page = ctx.get_page_mut(&owner).unwrap();
            page.navigate("data:text/plain,retained").await.unwrap();
            let request_id = page.network_events.last().unwrap().request_id.clone();
            let body = handle("getResponseBody", &json!({"requestId": request_id}), &mut ctx, &None).await.unwrap();
            assert_eq!(body["body"], "retained");
            let error = super::super::fetch::handle("takeResponseBodyAsStream", &json!({"requestId": request_id}), &mut ctx, &exhausted_session).await.unwrap_err();
            assert!(error.contains("response_body_budget_exhausted"));
            let result = super::super::fetch::handle("takeResponseBodyAsStream", &json!({"requestId": request_id}), &mut ctx, &None).await.unwrap();
            let result = super::super::io::handle("read", &json!({"handle": result["stream"]}), &mut ctx).await.unwrap();
            assert_eq!(base64::engine::general_purpose::STANDARD.decode(result["data"].as_str().unwrap()).unwrap(), b"retained");
        }
        let error = handle("getResponseBody", &json!({"requestId": "missing"}), &mut ctx, &None).await.unwrap_err();
        assert!(error.contains("response_body_budget_exhausted"));
        let error = super::super::fetch::handle("takeResponseBodyAsStream", &json!({"requestId": "missing"}), &mut ctx, &None).await.unwrap_err();
        assert!(error.contains("response_body_budget_exhausted"));
    }

    #[tokio::test]
    async fn response_body_budget_exhaustion_is_not_unknown_request() {
        let mut ctx = CdpContext::new();
        let page_id = ctx.create_page();
        let session = Some(format!("{page_id}-session"));
        ctx.sessions.insert(session.clone().unwrap(), page_id.clone());
        let page = ctx.get_page_mut(&page_id).unwrap();
        page.set_response_body_limits(obscura_net::response_body::ResponseBodyLimits {
            memory_threshold: 2, total_bytes: 4, entries: 16,
        });
        page.navigate("data:text/plain,hello").await.unwrap();
        let request_id = page.network_events.last().unwrap().request_id.clone();
        page.alias_response_body(&request_id, "loader");
        for id in [&request_id, "loader"] {
            let error = handle("getResponseBody", &json!({"requestId": id}), &mut ctx, &session).await.unwrap_err();
            assert!(error.contains("response_body_budget_exhausted"), "{error}");
            let error = super::super::fetch::handle("takeResponseBodyAsStream", &json!({"requestId": id}), &mut ctx, &session).await.unwrap_err();
            assert!(error.contains("response_body_budget_exhausted"), "{error}");
        }
    }

    #[tokio::test]
    async fn get_response_body_errors_for_unknown_request_id() {
        let mut ctx = CdpContext::new();
        let err = handle(
            "getResponseBody",
            &json!({ "requestId": "missing" }),
            &mut ctx,
            &None,
        )
        .await
        .unwrap_err();

        assert!(err.contains("missing"));
    }

    #[tokio::test]
    async fn network_disable_clears_stored_response_bodies() {
        let mut ctx = CdpContext::new();
        let page_id = ctx.create_page();
        let session_id = Some("session-1".to_string());
        ctx.sessions.insert(session_id.clone().unwrap(), page_id.clone());

        let page = ctx.get_page_mut(&page_id).unwrap();
        page.navigate("data:text/html,<html><body>temporary body</body></html>")
            .await
            .unwrap();
        let request_id = page.network_events[0].request_id.clone();

        handle("disable", &json!({}), &mut ctx, &session_id)
            .await
            .unwrap();

        let err = handle(
            "getResponseBody",
            &json!({ "requestId": request_id }),
            &mut ctx,
            &session_id,
        )
        .await
        .unwrap_err();
        assert!(err.contains("No response body found"));
    }

    #[tokio::test]
    async fn enable_accepts_only_empty_params() {
        for params in [Value::Null, json!({})] {
            handle("enable", &params, &mut CdpContext::new(), &None)
                .await
                .expect("omitted and empty params are equivalent");
        }
        assert!(
            handle("enable", &json!({"maxTotalBufferSize": 1}), &mut CdpContext::new(), &None)
                .await
                .is_err()
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn extra_headers_are_applied_to_the_page_primp_transport() {
        let mut ctx = CdpContext::new();
        let page_id = ctx.create_page();
        let session_id = Some("headers-session".to_string());
        ctx.sessions
            .insert(session_id.clone().unwrap(), page_id.clone());

        handle(
            "setExtraHTTPHeaders",
            &json!({"headers": {"X-Probe": "complete-raw-value"}}),
            &mut ctx,
            &session_id,
        )
        .await
        .unwrap();

        let page = ctx.get_page(&page_id).unwrap();
        assert_eq!(
            page.stealth_client
                .extra_headers
                .read()
                .await
                .get("X-Probe")
                .map(String::as_str),
            Some("complete-raw-value")
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn extra_headers_stay_page_local_and_reach_its_worker() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let proxy = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let mut captured = Vec::new();
            for body in ["<html></html>", "<html></html>", "ok", "ok", "ok"] {
                let (mut stream, _) = listener.accept().unwrap();
                stream.set_read_timeout(Some(std::time::Duration::from_secs(10))).unwrap();
                let mut request = Vec::new();
                while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                    let mut buffer = [0; 4096];
                    let count = stream.read(&mut buffer).unwrap();
                    assert!(count > 0);
                    request.extend_from_slice(&buffer[..count]);
                }
                stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).as_bytes()).unwrap();
                captured.push(String::from_utf8(request).unwrap());
            }
            captured
        });
        let mut ctx = CdpContext::new();
        ctx.default_context = Arc::new(obscura_browser::BrowserContext::with_proxy("header-context".into(), Some(proxy)));
        ctx.default_context.http_client.set_extra_headers([
            ("X-Baseline".into(), "Both Pages".into()),
            ("X-Shared".into(), "Context Value".into()),
        ].into_iter().collect()).await;
        let first = ctx.create_page();
        let second = ctx.create_page();
        ctx.get_page_mut(&first).unwrap().navigate("http://headers.test/a").await.unwrap();
        ctx.get_page_mut(&second).unwrap().navigate("http://headers.test/b").await.unwrap();
        let session = Some("isolated-headers-session".to_string());
        ctx.sessions.insert(session.clone().unwrap(), first.clone());
        handle("setExtraHTTPHeaders", &json!({"headers": {
            "Authorization": "Bearer Page.Raw+/=123", "X-Page": "Keep, Case; full=value",
            "x-shared": "Page Override"
        }}), &mut ctx, &session).await.unwrap();
        assert!(ctx.get_page(&second).unwrap().stealth_client.extra_headers.read().await.is_empty());
        assert_eq!(ctx.default_context.http_client.extra_headers.read().await.get("X-Shared").map(String::as_str), Some("Context Value"));
        for (page_id, expression) in [
            (&first, "fetch('/a-data').then(r => r.text())"),
            (&first, r#"new Promise((resolve, reject) => {
                const source = "fetch('http://headers.test/worker-data').then(r => r.text()).then(postMessage)";
                const worker = new Worker(URL.createObjectURL(new Blob([source], {type:'text/javascript'})));
                worker.onmessage = event => { worker.terminate(); resolve(event.data); };
                worker.onerror = reject;
            })"#),
            (&second, "fetch('/b-data').then(r => r.text())"),
        ] {
            let result = ctx.get_page_mut(page_id).unwrap().evaluate_for_cdp(expression, true, true).await;
            assert!(!result.thrown, "{result:?}");
            assert_eq!(result.value, Some(json!("ok")));
        }
        for (index, request) in server.join().unwrap().iter().enumerate() {
            let headers: Vec<_> = request.lines().filter_map(|line| line.split_once(':')).collect();
            let values = |name: &str| headers.iter().filter(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| value.trim()).collect::<Vec<_>>();
            assert_eq!(values("X-Baseline"), ["Both Pages"], "{request}");
            if index == 2 || index == 3 {
                assert_eq!(values("Authorization"), ["Bearer Page.Raw+/=123"], "{request}");
                assert_eq!(values("X-Page"), ["Keep, Case; full=value"], "{request}");
                assert_eq!(values("X-Shared"), ["Page Override"], "{request}");
            } else {
                assert!(values("Authorization").is_empty(), "{request}");
                assert!(values("X-Page").is_empty(), "{request}");
                assert_eq!(values("X-Shared"), ["Context Value"], "{request}");
            }
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn extra_headers_cannot_bypass_the_immutable_persona() {
        let mut ctx = CdpContext::new();
        let page_id = ctx.create_page();
        let session_id = Some("persona-headers-session".to_string());
        ctx.sessions.insert(session_id.clone().unwrap(), page_id);

        for name in ["User-Agent", "Accept-Language", "Accept-Encoding", "DNT", "Sec-CH-UA-Platform"] {
            let error = handle(
                "setExtraHTTPHeaders",
                &json!({"headers": {name: "override"}}),
                &mut ctx,
                &session_id,
            )
            .await
            .expect_err("persona-owned header must be rejected");
            assert!(error.contains("persona-owned header"), "{error}");
        }
    }
}

pub(crate) fn cookie_info_to_cdp_json(c: &obscura_net::CookieInfo) -> Value {
    let expires = c.expires.unwrap_or(SESSION_COOKIE_EXPIRES);
    let session = c.expires.is_none();
    let same_site = if c.same_site.is_empty() {
        DEFAULT_SAME_SITE
    } else {
        c.same_site.as_str()
    };
    json!({
        "name": c.name,
        "value": c.value,
        "domain": c.domain,
        "path": c.path,
        "expires": expires,
        "size": c.name.len() + c.value.len(),
        "httpOnly": c.http_only,
        "secure": c.secure,
        "session": session,
        "sameSite": same_site,
        "sameParty": false,
        "sourceScheme": if c.secure { SOURCE_SCHEME_SECURE } else { SOURCE_SCHEME_NONSECURE },
        "sourcePort": if c.secure { DEFAULT_SECURE_PORT } else { DEFAULT_INSECURE_PORT },
        "priority": "Medium",
    })
}
