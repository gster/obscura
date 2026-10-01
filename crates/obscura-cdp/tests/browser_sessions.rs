use obscura_cdp::dispatch::{dispatch, CdpContext};
use obscura_cdp::types::CdpRequest;
use serde_json::{json, Value};

async fn command(
    ctx: &mut CdpContext,
    id: u64,
    method: &str,
    params: Value,
    session: Option<&str>,
) -> Value {
    let response = dispatch(&CdpRequest {
        id,
        method: method.to_string(),
        params,
        session_id: session.map(str::to_string),
    }, ctx).await;
    assert_eq!(response.id, id);
    assert_eq!(response.session_id.as_deref(), session);
    assert!(response.error.is_none(), "{method}: {:?}", response.error);
    response.result.unwrap()
}

fn assert_context_history(inventory: &Value, context: &str, live: bool, finalized: bool) {
    assert!(inventory["histories"].as_array().unwrap().iter().any(|history| {
        history["browserContextId"] == context && history["live"] == live
            && history["finalized"] == finalized
    }));
}

#[tokio::test(flavor = "current_thread")]
async fn browser_attachments_survive_page_attachment_and_sibling_detach() {
    let mut ctx = CdpContext::new(obscura_net::EffectivePersona::builtin(
        obscura_net::StealthProfile::WindowsChrome145,
    ));
    let first = command(&mut ctx, 1, "Target.attachToBrowserTarget", json!({}), None)
        .await["sessionId"].as_str().unwrap().to_string();
    let second = command(&mut ctx, 2, "Target.attachToBrowserTarget", json!({}), None)
        .await["sessionId"].as_str().unwrap().to_string();
    assert_ne!(first, second, "independent browser attachments must not alias");
    for session in [&first, &second] {
        assert_eq!(ctx.sessions.get(session).map(String::as_str), Some("browser"));
        let event = ctx.pending_events.iter().find(|event| {
            event.method == "Target.attachedToTarget" && event.params["sessionId"] == *session
        }).unwrap();
        assert_eq!(event.params["targetInfo"]["type"], "browser");
        assert_eq!(event.params["targetInfo"]["canAccessOpener"], false);
    }
    let context = command(&mut ctx, 3, "Target.createBrowserContext",
        json!({"disposeOnDetach":true}), None).await["browserContextId"]
        .as_str().unwrap().to_string();
    let page = command(&mut ctx, 4, "Target.createTarget",
        json!({"url":"about:blank", "browserContextId":context}), None).await["targetId"]
        .as_str().unwrap().to_string();
    let managed = format!("{page}-session");
    let raw = command(&mut ctx, 5, "Target.attachToTarget",
        json!({"targetId":page, "flatten":true}), Some(&second)).await["sessionId"]
        .as_str().unwrap().to_string();
    assert_ne!(raw, managed);
    assert_ne!(raw, first);
    assert_ne!(raw, second);
    for (id, session) in [(6, &first), (7, &second)] {
        let inventory = command(&mut ctx, id, "Obscura.getNetworkHistories",
            json!({}), Some(session)).await;
        assert_context_history(&inventory, &context, true, false);
    }
    command(&mut ctx, 8, "Target.detachFromTarget", json!({"sessionId":second}), None).await;
    assert!(!ctx.sessions.contains_key(&second));
    assert_eq!(ctx.sessions.get(&first).map(String::as_str), Some("browser"));
    for session in [&managed, &raw] {
        assert_eq!(ctx.sessions.get(session).map(String::as_str), Some(page.as_str()));
    }
    command(&mut ctx, 9, "Target.closeTarget", json!({"targetId":page}), Some(&first)).await;
    let after_page = command(&mut ctx, 10, "Obscura.getNetworkHistories",
        json!({}), Some(&first)).await;
    assert_context_history(&after_page, &context, true, false);
    command(&mut ctx, 11, "Target.disposeBrowserContext",
        json!({"browserContextId":context}), Some(&first)).await;
    let after_context = command(&mut ctx, 12, "Obscura.getNetworkHistories",
        json!({}), Some(&first)).await;
    assert_context_history(&after_context, &context, false, true);
    assert_eq!(ctx.sessions.get(&first).map(String::as_str), Some("browser"));
}
