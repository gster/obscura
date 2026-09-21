use serde_json::{json, Value};

use crate::dispatch::CdpContext;
use crate::types::CdpEvent;
use crate::util::url_is_file_scheme;

fn validate_create_browser_context(
    params: &Value,
) -> Result<Option<obscura_net::EffectivePersona>, String> {
    if params.is_null() {
        return Ok(None);
    }
    let object = params
        .as_object()
        .ok_or("Target.createBrowserContext params must be an object")?;
    if object
        .keys()
        .any(|name| !matches!(name.as_str(), "disposeOnDetach" | "obscuraPersona"))
    {
        return Err(
            "Target.createBrowserContext supports only disposeOnDetach=true and obscuraPersona"
                .to_string(),
        );
    }
    if params
        .get("disposeOnDetach")
        .is_some_and(|value| value.as_bool() != Some(true))
    {
        return Err(
            "Target.createBrowserContext supports only optional disposeOnDetach=true".to_string(),
        );
    }
    params
        .get("obscuraPersona")
        .map(|value| {
            let spec: obscura_net::PersonaSpec = serde_json::from_value(value.clone())
                .map_err(|error| format!("invalid obscuraPersona: {error}"))?;
            spec.compile().map_err(|error| error.to_string())
        })
        .transpose()
}

fn browser_context_id<'a>(method: &str, params: &'a Value) -> Result<&'a str, String> {
    let object = params
        .as_object()
        .ok_or_else(|| format!("Target.{method} params must be an object"))?;
    if object.len() != 1 || !object.contains_key("browserContextId") {
        return Err(format!(
            "Target.{method} supports only string browserContextId"
        ));
    }
    params
        .get("browserContextId")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("Target.{method} requires string browserContextId"))
}

pub async fn handle(
    method: &str,
    params: &Value,
    ctx: &mut CdpContext,
    parent_session_id: &Option<String>,
) -> Result<Value, String> {
    match method {
        "setDiscoverTargets" => {
            ctx.pending_events.push(CdpEvent::new(
                "Target.targetCreated",
                json!({
                    "targetInfo": {
                        "targetId": "browser",
                        "type": "browser",
                        "title": "",
                        "url": "",
                        "attached": true,
                        "canAccessOpener": false,
                        "browserContextId": "",
                    }
                }),
            ));
            for page in &ctx.pages {
                ctx.pending_events.push(CdpEvent::new(
                    "Target.targetCreated",
                    json!({
                        "targetInfo": {
                            "targetId": page.id,
                            "type": "page",
                            "title": page.title,
                            "url": page.url_string(),
                            "attached": false,
                            "canAccessOpener": false,
                            "browserContextId": page.context.id,
                        }
                    }),
                ));
            }
            Ok(json!({}))
        }
        "getTargets" => {
            let targets: Vec<Value> = ctx
                .pages
                .iter()
                .map(|page| {
                    json!({
                        "targetId": page.id,
                        "type": "page",
                        "title": page.title,
                        "url": page.url_string(),
                        "attached": true,
                        "canAccessOpener": false,
                        "browserContextId": page.context.id,
                    })
                })
                .collect();
            Ok(json!({ "targetInfos": targets }))
        }
        "createTarget" => {
            let url = params
                .get("url")
                .and_then(|v| v.as_str())
                .unwrap_or("about:blank");
            let context_id = params.get("browserContextId").and_then(|v| v.as_str());
            let context = match context_id {
                Some(id) => ctx
                    .browser_context(id)
                    .ok_or_else(|| format!("Browser context not found: {}", id))?,
                None => &ctx.default_context,
            };

            // Same gate as Page.navigate (GHSA-q55h-vfv9-qcr5). Without this,
            // a CDP client can call Target.createTarget {url:"file:///etc/passwd"}
            // and then Runtime.evaluate the body off the created target,
            // bypassing the page-domain check entirely.
            if url_is_file_scheme(url) && !context.allow_file_access {
                return Err(
                    "Target.createTarget to file:// is disabled. Restart with `obscura serve --allow-file-access` to enable.".to_string()
                );
            }

            let page_id = ctx.create_page_in_context(context_id)?;
            let session_id = format!("{}-session", page_id);

            let committed_document = if let Some(page) = ctx.get_page_mut(&page_id) {
                if url == "about:blank" || url.is_empty() {
                    page.navigate_blank();
                    None
                } else {
                    page.navigate(url).await.ok().map(|_| {
                        (page.frame_id.clone(), page.url_string())
                    })
                }
            } else {
                None
            };
            if let Some((frame_id, origin)) = committed_document {
                ctx.commit_default_context(&page_id, &frame_id, &origin);
            }

            ctx.sessions.insert(session_id.clone(), page_id.clone());

            if let Some(page) = ctx.get_page(&page_id) {
                ctx.pending_events.push(CdpEvent::new(
                    "Target.targetCreated",
                    json!({
                        "targetInfo": {
                            "targetId": page_id,
                            "type": "page",
                            "title": page.title,
                            "url": page.url_string(),
                            "attached": false,
                            "canAccessOpener": false,
                            "browserContextId": page.context.id,
                        }
                    }),
                ));
            }

            if let Some(page) = ctx.get_page(&page_id) {
                ctx.pending_events.push(CdpEvent::new(
                    "Target.attachedToTarget",
                    json!({
                        "sessionId": session_id,
                        "targetInfo": {
                            "targetId": page_id,
                            "type": "page",
                            "title": page.title,
                            "url": page.url_string(),
                            "attached": true,
                            "canAccessOpener": false,
                            "browserContextId": page.context.id,
                        },
                        "waitingForDebugger": false,
                    }),
                ));
            }

            Ok(json!({ "targetId": page_id }))
        }
        "attachToBrowserTarget" => {
            // Playwright calls this on connect to obtain a session for the
            // implicit "browser" target. Returning Unknown method aborts
            // the connect handshake before any user code runs.
            let session_id = "browser-session".to_string();
            ctx.sessions
                .insert(session_id.clone(), "browser".to_string());

            ctx.pending_events.push(CdpEvent::new(
                "Target.attachedToTarget",
                json!({
                    "sessionId": session_id,
                    "targetInfo": {
                        "targetId": "browser",
                        "type": "browser",
                        "title": "",
                        "url": "",
                        "attached": true,
                        "canAccessOpener": false,
                        "browserContextId": "",
                    },
                    "waitingForDebugger": false,
                }),
            ));

            Ok(json!({ "sessionId": session_id }))
        }
        "attachToTarget" => {
            let target_id = params
                .get("targetId")
                .and_then(|v| v.as_str())
                .ok_or("targetId required")?;
            if ctx.get_page(target_id).is_none() {
                return Err("Target not found".to_string());
            }
            let session_id = ctx.next_target_session(target_id);
            ctx.sessions
                .insert(session_id.clone(), target_id.to_string());

            if let Some(page) = ctx.get_page(target_id) {
                let params = json!({
                    "sessionId": session_id,
                    "targetInfo": {
                        "targetId": target_id,
                        "type": "page",
                        "title": page.title,
                        "url": page.url_string(),
                        "attached": true,
                        "canAccessOpener": false,
                        "browserContextId": page.context.id,
                    },
                    "waitingForDebugger": false,
                });
                let event = match parent_session_id {
                    Some(parent_session_id) => CdpEvent::with_session(
                        "Target.attachedToTarget",
                        params,
                        parent_session_id.clone(),
                    ),
                    None => CdpEvent::new("Target.attachedToTarget", params),
                };
                ctx.pending_events.push(event);
            }

            Ok(json!({ "sessionId": session_id }))
        }
        "closeTarget" => {
            let object = params
                .as_object()
                .ok_or("Target.closeTarget params must be an object")?;
            if object.len() != 1 || !object.contains_key("targetId") {
                return Err("Target.closeTarget supports only string targetId".to_string());
            }
            let target_id = object
                .get("targetId")
                .and_then(|v| v.as_str())
                .ok_or("Target.closeTarget requires string targetId")?;
            if ctx.get_page(target_id).is_none() {
                return Err("Target not found".to_string());
            }
            let mut sessions = ctx.sessions.iter()
                .filter(|(_, page_id)| page_id.as_str() == target_id)
                .map(|(session_id, _)| session_id.clone())
                .collect::<Vec<_>>();
            sessions.sort_unstable();
            for session_id in sessions {
                ctx.pending_events.push(CdpEvent::new(
                    "Target.detachedFromTarget",
                    json!({
                        "sessionId": session_id,
                        "targetId": target_id,
                    }),
                ));
            }
            ctx.pending_events.push(CdpEvent::new(
                "Target.targetDestroyed",
                json!({ "targetId": target_id }),
            ));

            ctx.remove_page(target_id);
            Ok(json!({ "success": true }))
        }
        "setAutoAttach" => {
            let object = params
                .as_object()
                .ok_or("Target.setAutoAttach params must be an object")?;
            if object.len() != 3
                || params.get("autoAttach").and_then(Value::as_bool) != Some(true)
                || params.get("waitForDebuggerOnStart").and_then(Value::as_bool) != Some(true)
                || params.get("flatten").and_then(Value::as_bool) != Some(true)
            {
                return Err(
                    "Target.setAutoAttach supports only autoAttach=true, waitForDebuggerOnStart=true, flatten=true"
                        .to_string(),
                );
            }
            Ok(json!({}))
        }
        // No multi-target lifecycle to manage: obscura runs one page per session.
        // Ack these so Chrome-shaped clients that call them do not warn (issue #340).
        "detachFromTarget" => {
            if let Some(session_id) = params.get("sessionId").and_then(Value::as_str) {
                let page_id = ctx.sessions.get(session_id).cloned();
                ctx.sessions.remove(session_id);
                ctx.runtime_enabled_sessions.remove(session_id);
                ctx.disable_network_session(session_id);
                if let Some(page_id) = page_id {
                    ctx.refresh_runtime_event_collection(&page_id);
                }
                #[cfg(feature = "render")]
                ctx.screencasts.remove(session_id);
            }
            Ok(json!({}))
        }
        "activateTarget" => Ok(json!({})),
        "getBrowserContexts" => {
            let mut ids: Vec<&String> = ctx.browser_contexts.keys().collect();
            ids.sort();
            Ok(json!({ "browserContextIds": ids }))
        }
        "createBrowserContext" => {
            let persona = validate_create_browser_context(params)?;
            let id = ctx.create_browser_context(persona)?;
            Ok(json!({ "browserContextId": id }))
        }
        "disposeBrowserContext" => {
            let context_id = browser_context_id("disposeBrowserContext", params)?;
            let sessions: Vec<(String, String)> = ctx
                .sessions
                .iter()
                .filter_map(|(session_id, page_id)| {
                    ctx.get_page(page_id)
                        .filter(|page| page.context.id == context_id)
                        .map(|_| (session_id.clone(), page_id.clone()))
                })
                .collect();
            let page_ids = ctx.dispose_browser_context(context_id)?;
            for (session_id, page_id) in sessions {
                ctx.pending_events.push(CdpEvent::new(
                    "Target.detachedFromTarget",
                    json!({ "sessionId": session_id, "targetId": page_id }),
                ));
            }
            for page_id in page_ids {
                ctx.pending_events.push(CdpEvent::new(
                    "Target.targetDestroyed",
                    json!({ "targetId": page_id }),
                ));
            }
            Ok(json!({}))
        }
        "getTargetInfo" => {
            let target_id = params.get("targetId").and_then(|v| v.as_str());
            match target_id {
                Some(id) => {
                    let page = ctx.get_page(id).ok_or("Target not found")?;
                    Ok(json!({
                        "targetInfo": {
                            "targetId": id,
                            "type": "page",
                            "title": page.title,
                            "url": page.url_string(),
                            "attached": true,
                            "canAccessOpener": false,
                            "browserContextId": page.context.id,
                        }
                    }))
                }
                None => {
                    // canAccessOpener is required on every TargetInfo per the
                    // CDP spec. Strict clients (chromiumoxide) panic if it's
                    // missing. The browser target itself has no opener.
                    Ok(json!({
                        "targetInfo": {
                            "targetId": "browser",
                            "type": "browser",
                            "title": "",
                            "url": "",
                            "attached": true,
                            "canAccessOpener": false,
                        }
                    }))
                }
            }
        }
        _ => Err(format!("Unknown Target method: {}", method)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn browser_contexts_are_real_and_do_not_clear_default_cookies() {
        let mut ctx = CdpContext::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));
        ctx.default_context.cookie_jar.set_cookie(
            "sid=default",
            &url::Url::parse("https://example.com").unwrap(),
        );

        let created = handle("createBrowserContext", &json!({}), &mut ctx, &None)
            .await
            .expect("context creation should succeed");
        let context_id = created["browserContextId"].as_str().unwrap();
        assert_ne!(context_id, "default");
        assert!(ctx
            .browser_context(context_id)
            .unwrap()
            .cookie_jar
            .get_all_cookies()
            .is_empty());
        assert_eq!(ctx.default_context.cookie_jar.get_all_cookies().len(), 1);

        let listed = handle("getBrowserContexts", &json!({}), &mut ctx, &None)
            .await
            .expect("context listing should succeed");
        assert_eq!(listed["browserContextIds"], json!([context_id]));
    }

    #[tokio::test]
    async fn playwright_browser_context_shapes_are_explicit() {
        let mut ctx = CdpContext::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));
        for params in [Value::Null, json!({})] {
            let created = handle("createBrowserContext", &params, &mut ctx, &None)
                .await
                .expect("omitted and empty optional params should be equivalent");
            handle(
                "disposeBrowserContext",
                &json!({"browserContextId": created["browserContextId"]}),
                &mut ctx,
                &None,
            )
            .await
            .unwrap();
        }

        let created = handle(
            "createBrowserContext",
            &json!({"disposeOnDetach": true}),
            &mut ctx,
            &None,
        )
        .await
        .expect("the observed Playwright context shape should be accepted");
        let context_id = created["browserContextId"].as_str().unwrap();

        for params in [
            json!({"disposeOnDetach": false}),
            json!({"proxyServer": "http://example.test"}),
            json!({"disposeOnDetach": true, "invented": true}),
        ] {
            handle("createBrowserContext", &params, &mut ctx, &None)
                .await
                .expect_err("unqualified context options must fail");
        }

        handle(
            "disposeBrowserContext",
            &json!({"browserContextId": context_id}),
            &mut ctx,
            &None,
        )
        .await
        .expect("the observed Playwright disposal shape should be accepted");
        handle(
            "disposeBrowserContext",
            &json!({"browserContextId": context_id, "invented": true}),
            &mut ctx,
            &None,
        )
        .await
        .expect_err("extra disposal options must fail");
    }

    #[tokio::test]
    async fn injected_persona_is_compiled_before_context_registration() {
        let mut ctx = CdpContext::new(obscura_net::EffectivePersona::builtin(
            obscura_net::StealthProfile::WindowsChrome145,
        ));
        let custom = json!({
            "schema_version": "1",
            "persona_id": "customer_macos",
            "revision": "7",
            "profile": "macos_chrome153",
            "timezone": "Europe/Berlin",
            "language": "en-US",
            "languages": ["en-US", "en"],
            "accept_language": "en-US,en;q=0.9",
            "do_not_track": "0"
        });
        let created = handle(
            "createBrowserContext",
            &json!({"obscuraPersona": custom}),
            &mut ctx,
            &None,
        )
        .await
        .unwrap();
        let id = created["browserContextId"].as_str().unwrap();
        assert_eq!(ctx.browser_context(id).unwrap().persona().persona_id(), "customer_macos");
        assert_eq!(
            ctx.browser_context(id).unwrap().persona().do_not_track(),
            Some("0")
        );

        let count = ctx.browser_contexts.len();
        let incompatible = json!({
            "schema_version": "1",
            "persona_id": "wrong_timezone",
            "revision": "1",
            "profile": "macos_chrome153",
            "timezone": "Asia/Shanghai"
        });
        let error = handle(
            "createBrowserContext",
            &json!({"obscuraPersona": incompatible}),
            &mut ctx,
            &None,
        )
        .await
        .unwrap_err();
        assert!(error.contains("unsupported in this process"), "{error}");
        assert_eq!(ctx.browser_contexts.len(), count);
    }

    #[tokio::test]
    async fn disposing_context_removes_only_its_pages() {
        let mut ctx = CdpContext::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));
        let context_id = ctx.create_browser_context(None).unwrap();
        let isolated_page = ctx.create_page_in_context(Some(&context_id)).unwrap();
        let default_page = ctx.create_page();

        handle(
            "disposeBrowserContext",
            &json!({"browserContextId": context_id}),
            &mut ctx,
            &None,
        )
        .await
        .expect("context disposal should succeed");

        assert!(ctx.get_page(&isolated_page).is_none());
        assert!(ctx.get_page(&default_page).is_some());
        assert!(ctx.browser_contexts.is_empty());
    }

    #[tokio::test]
    async fn attach_to_browser_target_returns_session_id() {
        let mut ctx = CdpContext::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));
        let result = handle("attachToBrowserTarget", &json!({}), &mut ctx, &None)
            .await
            .expect("attachToBrowserTarget should succeed");

        assert_eq!(result["sessionId"], "browser-session");
        assert_eq!(
            ctx.sessions.get("browser-session").map(String::as_str),
            Some("browser")
        );

        // Playwright/Puppeteer expect a Target.attachedToTarget event before
        // they finish wiring up the session — without it the connect promise
        // hangs.
        let attached_evt = ctx
            .pending_events
            .iter()
            .find(|e| e.method == "Target.attachedToTarget")
            .expect("attachedToTarget event must be emitted");
        assert_eq!(attached_evt.params["sessionId"], "browser-session");
        assert_eq!(attached_evt.params["targetInfo"]["type"], "browser");
    }

    #[tokio::test]
    async fn explicit_page_attachment_is_unique_and_scoped_to_its_parent_session() {
        let mut ctx = CdpContext::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));
        let page_id = ctx.create_page();
        let managed_session = format!("{page_id}-session");
        ctx.sessions
            .insert(managed_session.clone(), page_id.clone());
        let parent_session = Some("browser-session".to_string());

        let first = handle(
            "attachToTarget",
            &json!({"targetId": page_id, "flatten": true}),
            &mut ctx,
            &parent_session,
        )
        .await
        .expect("first explicit attachment should succeed");
        let first_session = first["sessionId"].as_str().unwrap().to_string();

        assert_ne!(first_session, managed_session);
        assert_eq!(
            ctx.sessions.get(&first_session).map(String::as_str),
            Some(page_id.as_str())
        );
        let first_event = ctx.pending_events.last().unwrap();
        assert_eq!(first_event.method, "Target.attachedToTarget");
        assert_eq!(first_event.session_id.as_deref(), Some("browser-session"));
        assert_eq!(first_event.params["sessionId"], first_session);

        let second = handle(
            "attachToTarget",
            &json!({"targetId": page_id, "flatten": true}),
            &mut ctx,
            &parent_session,
        )
        .await
        .expect("second explicit attachment should succeed");
        assert_ne!(second["sessionId"], first["sessionId"]);
    }

    #[tokio::test]
    async fn detaching_explicit_session_removes_its_page_and_network_routes_only() {
        let mut ctx = CdpContext::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));
        let page_id = ctx.create_page();
        let parent_session = Some("browser-session".to_string());
        let attached = handle(
            "attachToTarget",
            &json!({"targetId": page_id}),
            &mut ctx,
            &parent_session,
        )
        .await
        .unwrap();
        let session_id = attached["sessionId"].as_str().unwrap().to_string();
        let sibling = handle(
            "attachToTarget",
            &json!({"targetId": page_id, "flatten": true}),
            &mut ctx,
            &parent_session,
        ).await.unwrap()["sessionId"].as_str().unwrap().to_string();
        for session in [&session_id, &sibling] {
            crate::domains::network::handle(
                "enable",
                &json!({}),
                &mut ctx,
                &Some(session.clone()),
            ).await.unwrap();
            ctx.network_body_sessions.get_mut(session).unwrap().insert("body-id".into());
        }
        ctx.network_request_sessions.insert(
            (page_id.clone(), "request-id".into()),
            vec![session_id.clone(), sibling.clone()],
        );

        handle(
            "detachFromTarget",
            &json!({"sessionId": session_id}),
            &mut ctx,
            &parent_session,
        )
        .await
        .expect("detach should succeed");
        assert!(!ctx.sessions.contains_key(&session_id));
        assert!(!ctx.network_enabled_sessions.contains(&session_id));
        assert!(!ctx.network_body_sessions.contains_key(&session_id));
        assert_eq!(
            ctx.network_request_sessions[&(page_id.clone(), "request-id".into())],
            vec![sibling.clone()]
        );
        assert!(ctx.network_enabled_sessions.contains(&sibling));
        assert!(ctx.network_body_sessions[&sibling].contains("body-id"));

        let replacement = handle(
            "attachToTarget",
            &json!({"targetId": page_id, "flatten": true}),
            &mut ctx,
            &parent_session,
        ).await.unwrap()["sessionId"].as_str().unwrap().to_string();
        assert!(!ctx.network_enabled_sessions.contains(&replacement));
        assert!(!ctx.network_body_sessions.contains_key(&replacement));
    }

    #[tokio::test]
    async fn closing_target_detaches_every_actual_page_session() {
        let mut ctx = CdpContext::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));
        let page_id = ctx.create_page();
        let parent_session = Some("browser-session".to_string());
        let first = handle(
            "attachToTarget",
            &json!({"targetId": page_id, "flatten": true}),
            &mut ctx,
            &parent_session,
        ).await.unwrap()["sessionId"].as_str().unwrap().to_string();
        let second = handle(
            "attachToTarget",
            &json!({"targetId": page_id, "flatten": true}),
            &mut ctx,
            &parent_session,
        ).await.unwrap()["sessionId"].as_str().unwrap().to_string();
        ctx.pending_events.clear();

        handle(
            "closeTarget",
            &json!({"targetId": page_id}),
            &mut ctx,
            &None,
        ).await.unwrap();

        let detached = ctx.pending_events.iter()
            .filter(|event| event.method == "Target.detachedFromTarget")
            .map(|event| event.params["sessionId"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(detached, vec![first.as_str(), second.as_str()]);
        let fabricated = format!("{page_id}-session");
        assert!(!detached.contains(&fabricated.as_str()));
    }

    #[tokio::test]
    async fn close_target_rejects_ignored_and_unknown_parameters() {
        let mut ctx = CdpContext::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));
        let page_id = ctx.create_page();

        for params in [
            Value::Null,
            json!({}),
            json!({"targetId": page_id.clone(), "invented": true}),
            json!({"targetId": 7}),
        ] {
            handle("closeTarget", &params, &mut ctx, &None)
                .await
                .expect_err("malformed closeTarget parameters must fail");
        }
        handle(
            "closeTarget",
            &json!({"targetId": "missing-target"}),
            &mut ctx,
            &None,
        )
        .await
        .expect_err("an unknown target must fail");
        assert!(ctx.get_page(&page_id).is_some());
    }

    #[tokio::test]
    async fn unknown_target_method_still_errors() {
        let mut ctx = CdpContext::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));
        let err = handle("notARealMethod", &json!({}), &mut ctx, &None)
            .await
            .expect_err("unknown methods must surface as errors");
        assert!(err.contains("Unknown Target method"));
    }

    #[tokio::test]
    async fn auto_attach_accepts_only_the_observed_playwright_initializer() {
        let valid = json!({
            "autoAttach": true,
            "waitForDebuggerOnStart": true,
            "flatten": true,
        });
        handle("setAutoAttach", &valid, &mut CdpContext::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145)), &None)
            .await
            .expect("observed initializer must remain compatible");

        for params in [
            json!({}),
            json!({"autoAttach": true, "waitForDebuggerOnStart": true, "flatten": false}),
            json!({"autoAttach": true, "waitForDebuggerOnStart": true, "flatten": true, "invented": 1}),
        ] {
            assert!(
                handle("setAutoAttach", &params, &mut CdpContext::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145)), &None)
                    .await
                    .is_err(),
                "must reject {params}"
            );
        }
    }

    /// Regression for #122 item 5: every TargetInfo payload must carry the
    /// `canAccessOpener` field. The browser-target branch of getTargetInfo
    /// (no targetId passed → no page) used to omit it; strict CDP clients
    /// like chromiumoxide panic when the field is missing.
    #[tokio::test]
    async fn get_target_info_browser_target_includes_can_access_opener() {
        let mut ctx = CdpContext::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));
        // No targetId → falls through to the browser-target branch.
        let result = handle("getTargetInfo", &json!({}), &mut ctx, &None)
            .await
            .expect("getTargetInfo with no targetId must return browser info");

        let info = &result["targetInfo"];
        assert_eq!(info["type"], "browser", "must be the browser target");
        assert!(
            info.get("canAccessOpener").is_some(),
            "canAccessOpener must be present on every TargetInfo, got: {result}"
        );
        assert_eq!(info["canAccessOpener"], false);
    }
}
