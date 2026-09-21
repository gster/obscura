use std::sync::Arc;

use base64::Engine as _;
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

fn network_enable_integer(params: &Value, name: &str) -> Result<Option<i64>, String> {
    let Some(value) = params.get(name) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    value.as_i64().map(Some).ok_or_else(|| {
        format!("Network.enable {name} must be an integer or null")
    })
}

fn network_enable_post_data_size(params: &Value) -> Result<Option<usize>, String> {
    let Some(integer) = network_enable_integer(params, "maxPostDataSize")? else {
        return Ok(None);
    };
    if integer <= 0 { return Ok(None); }
    usize::try_from(integer).map(Some).map_err(|_| {
        "Network.enable maxPostDataSize is too large for this platform".to_string()
    })
}

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
            if !(params.is_null() || params.is_object()) {
                return Err("Network.enable params must be an object or null".to_string());
            }
            // Chrome validates the three integer buffer arguments. Only
            // maxPostDataSize has a qualified projection contract here. Keep
            // the two response-cache hints in agent state for future Chrome
            // eviction parity; neither may truncate Page/history raw data.
            let max_total_buffer_size = network_enable_integer(params, "maxTotalBufferSize")?;
            let max_resource_buffer_size = network_enable_integer(params, "maxResourceBufferSize")?;
            let max_post_data_size = network_enable_post_data_size(params)?;
            if let Some(session) = session_id {
                if !ctx.sessions.get(session).is_some_and(|page_id| {
                    ctx.has_page(page_id)
                        || ctx.navigating_page_id.as_deref() == Some(page_id.as_str())
                }) {
                    return Err(format!("No page found for sessionId {session}"));
                }
                ctx.network_agent_limits.insert(
                    session.clone(),
                    crate::dispatch::NetworkAgentLimits {
                        max_total_buffer_size,
                        max_resource_buffer_size,
                        max_post_data_size,
                    },
                );
                if ctx.network_enabled_sessions.insert(session.clone()) {
                    // A fresh Network agent cannot read bodies from requests
                    // observed before it enabled. Repeated enable is idempotent.
                    ctx.network_body_sessions.insert(session.clone(), Default::default());
                    ctx.network_request_body_sessions.insert(session.clone(), Default::default());
                    ctx.network_body_failure_sessions.remove(session);
                }
            }
            Ok(json!({}))
        }
        "disable" => {
            if let Some(session) = session_id {
                ctx.disable_network_session(session);
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
        "getRequestPostData" => {
            let request_id = params
                .get("requestId")
                .and_then(|v| v.as_str())
                .ok_or("Network.getRequestPostData requires requestId")?;
            get_request_post_data(ctx, session_id, request_id)
        }
        _ => Err(format!("Unknown Network method: {}", method)),
    }
}

/// Chrome resolves a logical request id to the latest redirect hop. Obscura
/// additionally accepts a per-hop `requestBodyRequestId` or
/// `transportRequestBodyRequestId` emitted to the same Network session, so a
/// client can retrieve every exact body rather than losing earlier or
/// interception-overridden bytes.
pub(super) fn get_request_post_data(
    ctx: &CdpContext,
    session_id: &Option<String>,
    request_id: &str,
) -> Result<Value, String> {
    let (body_id, store) = if let Some(session) = session_id {
        let page_id = ctx.sessions.get(session)
            .ok_or_else(|| format!("No page found for sessionId {session}"))?;
        if !ctx.network_enabled_sessions.contains(session) {
            return Err(format!("No post data available for requestId {request_id}"));
        }
        let body_id = ctx.network_request_body_sessions.get(session)
            .and_then(|visible| visible.get(request_id)).cloned()
            .ok_or_else(|| format!("No post data available for requestId {request_id}"))?;
        let store = ctx.get_page(page_id).map(|page| page.request_body_store())
            .or_else(|| ctx.navigating_request_bodies.as_ref()
                .filter(|(owner, _)| owner == page_id)
                .map(|(_, store)| store.clone()))
            .ok_or_else(|| format!("No page found for sessionId {session}"))?;
        (body_id, store)
    } else {
        let mut owners = ctx.pages.iter().filter(|page| page.has_request_body(request_id))
            .map(|page| page.request_body_store()).collect::<Vec<_>>();
        if let Some((_, store)) = &ctx.navigating_request_bodies {
            if store.lock().unwrap_or_else(|error| error.into_inner()).contains(request_id) {
                owners.push(store.clone());
            }
        }
        if owners.len() > 1 {
            return Err(format!("Ambiguous requestId {request_id}; supply sessionId"));
        }
        let store = owners.pop()
            .ok_or_else(|| format!("No post data available for requestId {request_id}"))?;
        (request_id.to_string(), store)
    };
    let body = store.lock().unwrap_or_else(|error| error.into_inner()).get(&body_id)
        .ok_or_else(|| format!("No post data available for requestId {request_id}"))?
        .map_err(|error| error.to_string())?;
    let (post_data, base64_encoded) = body.with_bytes(|bytes| match std::str::from_utf8(bytes) {
        Ok(text) => (text.to_owned(), false),
        Err(_) => (base64::engine::general_purpose::STANDARD.encode(bytes), true),
    }).map_err(|error| error.to_string())?;
    Ok(json!({ "postData": post_data, "base64Encoded": base64_encoded }))
}

/// Read a completed capture without consuming its shared raw storage.
/// Fetch uses the same lookup and protocol encoding as Network.
pub(super) fn get_response_body(
    ctx: &CdpContext,
    session_id: &Option<String>,
    request_id: &str,
) -> Result<Value, String> {
    if let Some(session) = session_id {
        let page_id = ctx.sessions.get(session)
            .ok_or_else(|| format!("No page found for sessionId {session}"))?;
        let store = ctx.get_page(page_id).map(|page| page.response_body_store())
            .or_else(|| ctx.navigating_response_bodies.as_ref()
                .filter(|(owner, _)| owner == page_id)
                .map(|(_, store)| store.clone()))
            .ok_or_else(|| format!("No page found for sessionId {session}"))?;
        let retained = store.lock().unwrap_or_else(|error| error.into_inner())
            .contains(request_id);
        if !ctx.network_body_is_visible(session, request_id, retained) {
            return Err(format!("No response body found for requestId {request_id}"));
        }
    }
    let (_, store) = response_body_owner(ctx, session_id, request_id)?;
    let body = store.lock().unwrap_or_else(|e| e.into_inner()).get(request_id)
        .ok_or_else(|| format!("No response body found for requestId {request_id}"))?
        .map_err(|error| error.to_string())?;
    let (body, binary) = body;
    let encoded = body.with_bytes(|bytes| {
        if !binary {
            if let Ok(text) = std::str::from_utf8(bytes) {
                return (text.to_owned(), false);
            }
        }
        (base64::engine::general_purpose::STANDARD.encode(bytes), true)
    }).map_err(|error| error.to_string())?;
    Ok(json!({ "body": encoded.0, "base64Encoded": encoded.1 }))
}

/// A session never falls through to another Page, even while that Page is
/// moved into the navigation task. Sessionless reads require a unique owner.
pub(crate) fn response_body_owner(
    ctx: &CdpContext,
    session_id: &Option<String>,
    request_id: &str,
) -> Result<(String, Arc<std::sync::Mutex<obscura_net::response_body::ResponseBodyStore>>), String> {
    let missing = || format!("No response body found for requestId {request_id}");
    if let Some(session) = session_id {
        let page_id = ctx.sessions.get(session)
            .ok_or_else(|| format!("No page found for sessionId {session}"))?.clone();
        let store = ctx.get_page(&page_id).map(|page| page.response_body_store())
            .or_else(|| ctx.navigating_response_bodies.as_ref()
                .filter(|(navigating_id, _)| navigating_id == &page_id).map(|(_, store)| store.clone()))
            .ok_or_else(|| format!("No page found for sessionId {session}"))?;
        store.lock().unwrap_or_else(|e| e.into_inner()).get(request_id)
            .ok_or_else(missing)?
            .map_err(|error| error.to_string())?;
        return Ok((page_id, store));
    }
    let mut owners: Vec<_> = ctx.pages.iter().filter(|page| page.has_response_body(request_id))
        .map(|page| (page.id.clone(), page.response_body_store())).collect();
    if let Some((page_id, store)) = &ctx.navigating_response_bodies {
        if store.lock().unwrap_or_else(|e| e.into_inner()).contains(request_id) {
            owners.push((page_id.clone(), store.clone()));
        }
    }
    if owners.len() > 1 {
        return Err(format!("Ambiguous requestId {request_id}; supply sessionId"));
    }
    if let Some((page_id, store)) = owners.pop() {
        store.lock().unwrap_or_else(|e| e.into_inner()).get(request_id)
            .ok_or_else(missing)?
            .map_err(|error| error.to_string())?;
        return Ok((page_id, store));
    }
    let error = ctx.pages.iter().find_map(|page| page.response_body_size(request_id).and_then(Result::err))
        .or_else(|| ctx.navigating_response_bodies.as_ref().and_then(|(_, store)| {
            store.lock().unwrap_or_else(|e| e.into_inner()).get(request_id).and_then(Result::err).map(|e| e.to_string())
        }));
    Err(error.unwrap_or_else(missing))
}

/// Fetch stream state is independent from ordinary Network body visibility.
/// Keep this typed so lifecycle handling never mistakes a body-store budget or
/// I/O error for a stream transfer.
pub(crate) fn fetch_response_body_access(
    ctx: &CdpContext,
    session_id: &Option<String>,
    request_id: &str,
) -> Result<obscura_net::response_body::FetchAccess, String> {
    let (_, store) = response_body_owner(ctx, session_id, request_id)?;
    let access = {
        let store = store.lock().unwrap_or_else(|error| error.into_inner());
        store
            .fetch_access(request_id)
            .ok_or_else(|| format!("No response body found for requestId {request_id}"))?
            .map_err(|error| error.to_string())?
    };
    Ok(access)
}

#[cfg(test)]
mod tests {
    use super::*;
    use obscura_net::CookieInfo;

    fn grant_network_body_access(ctx: &mut CdpContext, session: &Option<String>, ids: &[&str]) {
        let session = session.as_ref().expect("test Network access needs a session");
        ctx.network_enabled_sessions.insert(session.clone());
        ctx.network_body_sessions.entry(session.clone()).or_default()
            .extend(ids.iter().map(|id| (*id).to_string()));
    }

    fn grant_network_body_failure(ctx: &mut CdpContext, session: &Option<String>) {
        let session = session.as_ref().expect("test Network access needs a session");
        ctx.network_enabled_sessions.insert(session.clone());
        ctx.network_body_sessions.entry(session.clone()).or_default();
        ctx.network_body_failure_sessions.insert(session.clone());
    }

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
        let mut ctx = CdpContext::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));
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
        let mut ctx = CdpContext::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));
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
        let mut ctx = CdpContext::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));
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
        let mut ctx = CdpContext::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));
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
        let mut ctx = CdpContext::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));
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
        let mut ctx = CdpContext::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));
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
        let mut ctx = CdpContext::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));
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
        let mut ctx = CdpContext::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));
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
        let mut ctx = CdpContext::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));
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
        let mut ctx = CdpContext::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));
        let page_id = ctx.create_page();
        let session_id = Some("session-1".to_string());
        ctx.sessions.insert(session_id.clone().unwrap(), page_id.clone());

        let page = ctx.get_page_mut(&page_id).unwrap();
        page.navigate("data:text/html,<html><body>hello body</body></html>")
            .await
            .unwrap();
        let request_id = page.network_events[0].request_id.clone();
        grant_network_body_access(&mut ctx, &session_id, &[&request_id]);

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
    async fn request_post_data_is_byte_exact_hop_scoped_and_session_owned() {
        let mut ctx = CdpContext::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));
        let page_id = ctx.create_page();
        let first = Some("request-body-first".to_string());
        let late = Some("request-body-late".to_string());
        ctx.sessions.insert(first.clone().unwrap(), page_id.clone());
        ctx.sessions.insert(late.clone().unwrap(), page_id.clone());
        handle("enable", &json!({}), &mut ctx, &first).await.unwrap();

        let standard_id = "fetch-1-request-hop-0-standard";
        let transport_id = "fetch-1-request-hop-0-transport";
        let standard = vec![0, 0xff, b'=', 0x80];
        let transport = vec![0xfe, 1, 0, 0xfd];
        {
            let store = ctx.get_page(&page_id).unwrap().request_body_store();
            let mut store = store.lock().unwrap_or_else(|error| error.into_inner());
            store.insert(standard_id.into(), &standard).unwrap();
            store.insert(transport_id.into(), &transport).unwrap();
            store.alias(standard_id, "fetch-1").unwrap();
        }
        ctx.remember_network_request_bodies(
            &page_id, std::slice::from_ref(first.as_ref().unwrap()), "fetch-1",
            Some(standard_id), Some(transport_id),
        );

        let logical = handle("getRequestPostData", &json!({"requestId":"fetch-1"}), &mut ctx, &first).await.unwrap();
        assert_eq!(logical["base64Encoded"], true);
        assert_eq!(base64::engine::general_purpose::STANDARD.decode(logical["postData"].as_str().unwrap()).unwrap(), standard);
        let wire = handle("getRequestPostData", &json!({"requestId":transport_id}), &mut ctx, &first).await.unwrap();
        assert_eq!(base64::engine::general_purpose::STANDARD.decode(wire["postData"].as_str().unwrap()).unwrap(), transport);

        handle("enable", &json!({}), &mut ctx, &late).await.unwrap();
        assert!(handle("getRequestPostData", &json!({"requestId":"fetch-1"}), &mut ctx, &late).await.is_err());

        // A 302/303 next hop is body-absent for the logical Network id, while
        // the earlier canonical id remains available to the original observer.
        ctx.remember_network_request_bodies(
            &page_id, std::slice::from_ref(first.as_ref().unwrap()), "fetch-1", None, None,
        );
        assert!(handle("getRequestPostData", &json!({"requestId":"fetch-1"}), &mut ctx, &first).await.is_err());
        let earlier = handle("getRequestPostData", &json!({"requestId":standard_id}), &mut ctx, &first).await.unwrap();
        assert_eq!(base64::engine::general_purpose::STANDARD.decode(earlier["postData"].as_str().unwrap()).unwrap(), standard);

        let empty_id = "fetch-empty-request-hop-0-standard";
        ctx.get_page(&page_id).unwrap().request_body_store().lock()
            .unwrap_or_else(|error| error.into_inner()).insert(empty_id.into(), &[]).unwrap();
        ctx.remember_network_request_bodies(
            &page_id, std::slice::from_ref(first.as_ref().unwrap()), "fetch-empty",
            Some(empty_id), Some(empty_id),
        );
        let empty = handle("getRequestPostData", &json!({"requestId":"fetch-empty"}), &mut ctx, &first).await.unwrap();
        assert_eq!(empty, json!({"postData":"", "base64Encoded":false}));

        handle("disable", &json!({}), &mut ctx, &first).await.unwrap();
        assert!(handle("getRequestPostData", &json!({"requestId":standard_id}), &mut ctx, &first).await.is_err());
    }

    #[tokio::test]
    async fn response_body_large_text_binary_and_legacy_text_round_trip() {
        use base64::Engine as _;
        let mut ctx = CdpContext::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));
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
            grant_network_body_access(&mut ctx, &session, &[&request_id]);
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
        let mut ctx = CdpContext::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));
        ctx.default_context = Arc::new(obscura_browser::BrowserContext::with_storage_and_network(
            "body-resources".into(), obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145), None, None, true,
        ));
        let page_id = ctx.create_page();
        let session = Some(format!("{page_id}-session"));
        ctx.sessions.insert(session.clone().unwrap(), page_id.clone());
        let page = ctx.get_page_mut(&page_id).unwrap();
        page.navigate(&format!("{origin}/")).await.unwrap();
        page.prepare_screenshot_resources(3000).await;
        let events = page.network_events.clone();
        let request_ids = events.iter().map(|event| event.request_id.as_str()).collect::<Vec<_>>();
        grant_network_body_access(&mut ctx, &session, &request_ids);
        for (path, (_, expected, resource_type)) in resources.iter() {
            let event = events.iter().find(|event| {
                !event.pending && event.url == format!("{origin}{path}")
            })
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
        let mut ctx = CdpContext::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));
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
            let result = super::super::io::handle("read", &json!({"handle": result["stream"]}), &mut ctx, &None).await.unwrap();
            assert_eq!(base64::engine::general_purpose::STANDARD.decode(result["data"].as_str().unwrap()).unwrap(), b"retained");
        }
        let error = handle("getResponseBody", &json!({"requestId": "missing"}), &mut ctx, &None).await.unwrap_err();
        assert!(error.contains("response_body_budget_exhausted"));
        let error = super::super::fetch::handle("takeResponseBodyAsStream", &json!({"requestId": "missing"}), &mut ctx, &None).await.unwrap_err();
        assert!(error.contains("response_body_budget_exhausted"));
    }

    #[tokio::test]
    async fn response_body_budget_exhaustion_is_not_unknown_request() {
        let mut ctx = CdpContext::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));
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
        grant_network_body_failure(&mut ctx, &session);
        for id in [&request_id, "loader"] {
            let error = handle("getResponseBody", &json!({"requestId": id}), &mut ctx, &session).await.unwrap_err();
            assert!(error.contains("response_body_budget_exhausted"), "{error}");
            let error = super::super::fetch::handle("takeResponseBodyAsStream", &json!({"requestId": id}), &mut ctx, &session).await.unwrap_err();
            assert!(error.contains("response_body_budget_exhausted"), "{error}");
        }
    }

    #[tokio::test]
    async fn get_response_body_errors_for_unknown_request_id() {
        let mut ctx = CdpContext::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));
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
    async fn network_disable_invalidates_only_that_agents_body_view() {
        let mut ctx = CdpContext::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));
        let page_id = ctx.create_page();
        let session_id = Some("session-1".to_string());
        let sibling = Some("session-2".to_string());
        ctx.sessions.insert(session_id.clone().unwrap(), page_id.clone());
        ctx.sessions.insert(sibling.clone().unwrap(), page_id.clone());

        let page = ctx.get_page_mut(&page_id).unwrap();
        page.navigate("data:text/html,<html><body>temporary body</body></html>")
            .await
            .unwrap();
        let request_id = page.network_events[0].request_id.clone();
        grant_network_body_access(&mut ctx, &session_id, &[&request_id]);
        grant_network_body_access(&mut ctx, &sibling, &[&request_id]);

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
        assert_eq!(
            handle("getResponseBody", &json!({"requestId": request_id}), &mut ctx, &sibling)
                .await.unwrap()["body"],
            "<html><body>temporary body</body></html>"
        );
        assert!(ctx.get_page(&page_id).unwrap().has_response_body(&request_id));
    }

    #[tokio::test]
    async fn enable_accepts_chrome_buffer_params_and_resets_session_projection() {
        let mut ctx = CdpContext::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));
        let page_id = ctx.create_page();
        let session = Some("network-limits".to_string());
        ctx.sessions.insert(session.clone().unwrap(), page_id);

        handle(
            "enable",
            &json!({
                "maxTotalBufferSize": 1024,
                "maxResourceBufferSize": null,
                "maxPostDataSize": 6,
                "unknownFutureField": "accepted-by-Chrome"
            }),
            &mut ctx,
            &session,
        ).await.unwrap();
        assert_eq!(
            ctx.network_agent_limits[session.as_ref().unwrap()].max_post_data_size,
            Some(6),
        );
        assert_eq!(
            ctx.network_agent_limits[session.as_ref().unwrap()].max_total_buffer_size,
            Some(1024),
        );
        assert_eq!(
            ctx.network_agent_limits[session.as_ref().unwrap()].max_resource_buffer_size,
            None,
        );
        ctx.network_request_body_sessions.get_mut(session.as_ref().unwrap()).unwrap()
            .insert("already-observed".into(), "already-observed-body".into());

        // Repeated enable updates only event projection policy. Omitted, null,
        // zero, and negative values all restore Chrome's default/unbounded
        // request event projection.
        for params in [Value::Null, json!({}), json!({"maxPostDataSize": null}),
            json!({"maxPostDataSize": 0}), json!({"maxPostDataSize": -1})]
        {
            handle("enable", &params, &mut ctx, &session).await.unwrap();
            assert_eq!(
                ctx.network_agent_limits[session.as_ref().unwrap()].max_post_data_size,
                None,
            );
            assert_eq!(
                ctx.network_request_body_sessions[session.as_ref().unwrap()]["already-observed"],
                "already-observed-body",
            );
        }

        for params in [json!({"maxPostDataSize": 1.5}), json!({"maxPostDataSize": "6"}),
            json!({"maxTotalBufferSize": false}), json!([])]
        {
            assert!(handle("enable", &params, &mut ctx, &session).await.is_err(),
                "invalid Network.enable params unexpectedly accepted: {params}");
        }

        handle("enable", &json!({"maxPostDataSize": 4}), &mut ctx, &session).await.unwrap();
        handle("disable", &json!({}), &mut ctx, &session).await.unwrap();
        assert!(!ctx.network_agent_limits.contains_key(session.as_ref().unwrap()));
    }

    #[tokio::test]
    async fn enable_rejects_a_non_page_session() {
        let mut ctx = CdpContext::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));
        ctx.sessions.insert("browser-session".into(), "browser".into());
        let error = handle(
            "enable",
            &json!({}),
            &mut ctx,
            &Some("browser-session".into()),
        ).await.unwrap_err();
        assert!(error.contains("No page found for sessionId browser-session"), "{error}");
        assert!(ctx.network_enabled_sessions.is_empty());
    }

    #[tokio::test]
    async fn enable_accepts_a_page_temporarily_owned_by_navigation() {
        let mut ctx = CdpContext::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));
        let page_id = ctx.create_page();
        let session = Some("navigating-network-session".to_string());
        ctx.sessions.insert(session.clone().unwrap(), page_id.clone());
        let _navigating_page = ctx.pages.remove(
            ctx.pages.iter().position(|page| page.id == page_id).unwrap(),
        );
        ctx.navigating_page_id = Some(page_id);

        handle(
            "enable",
            &json!({"maxPostDataSize": 7}),
            &mut ctx,
            &session,
        ).await.unwrap();

        assert!(ctx.network_enabled_sessions.contains(session.as_ref().unwrap()));
        assert_eq!(
            ctx.network_agent_limits[session.as_ref().unwrap()].max_post_data_size,
            Some(7),
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn extra_headers_are_applied_to_the_page_primp_transport() {
        let mut ctx = CdpContext::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));
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
        let mut ctx = CdpContext::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));
        ctx.default_context = Arc::new(obscura_browser::BrowserContext::with_proxy("header-context".into(), obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145), Some(proxy)));
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
        let mut ctx = CdpContext::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));
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
