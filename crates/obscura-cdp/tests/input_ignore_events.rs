use obscura_cdp::dispatch::{dispatch, CdpContext};
use obscura_cdp::types::{CdpRequest, CdpResponse};
use serde_json::{json, Value};
#[cfg(feature = "render")]
use tokio::io::{AsyncReadExt, AsyncWriteExt};
#[cfg(feature = "render")]
use tokio::net::TcpListener;

#[cfg(feature = "render")]
async fn serve_fixture() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        for _ in 0..2 {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 2048];
            let _ = socket.read(&mut buf).await.unwrap();
        let body = r##"<!doctype html><html><head><style>
            html, body { margin: 0; }
            #hit { position: absolute; left: 20px; top: 20px; width: 120px; height: 40px; }
            #field { position: absolute; left: 20px; top: 80px; width: 180px; height: 30px; }
            #page { width: 800px; height: 2400px; }
        </style></head><body>
          <div id="page"></div>
          <a id="hit" href="#clicked">click</a>
          <input id="field">
          <script>
            globalThis.counts = {};
            for (const type of ['pointerdown','mousedown','pointerup','mouseup','click','wheel',
                                'keydown','keypress','beforeinput','input']) {
              counts[type] = 0;
              document.addEventListener(type, () => counts[type]++);
            }
          </script>
        </body></html>"##;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
        }
    });
    format!("http://{addr}/")
}

async fn raw(
    ctx: &mut CdpContext,
    id: u64,
    method: &str,
    params: Value,
    session_id: Option<&str>,
) -> CdpResponse {
    dispatch(
        &CdpRequest {
            id,
            method: method.to_string(),
            params,
            session_id: session_id.map(str::to_string),
        },
        ctx,
    )
    .await
}

#[cfg(feature = "render")]
async fn cdp(
    ctx: &mut CdpContext,
    id: u64,
    method: &str,
    params: Value,
    session_id: &str,
) -> Value {
    let response = raw(ctx, id, method, params, Some(session_id)).await;
    assert!(response.error.is_none(), "CDP {method} failed: {:?}", response.error);
    response.result.unwrap_or_else(|| json!({}))
}

#[cfg(feature = "render")]
async fn snapshot(ctx: &mut CdpContext, id: u64, session_id: &str) -> Value {
    let result = cdp(ctx, id, "Runtime.evaluate", json!({
        "expression": r#"JSON.stringify({
            value: field.value, hash: location.hash, scrollY, counts
        })"#,
        "returnByValue": true
    }), session_id).await;
    serde_json::from_str(result["result"]["value"].as_str().unwrap()).unwrap()
}

#[cfg(feature = "render")]
#[tokio::test(flavor = "current_thread")]
async fn ignore_contributions_are_session_owned_and_page_aggregated() {
    std::env::set_var("OBSCURA_ALLOW_PRIVATE_NETWORK", "1");
    let url = serve_fixture().await;
    let mut ctx = CdpContext::new(obscura_net::EffectivePersona::builtin(
        obscura_net::StealthProfile::WindowsChrome145,
    ));
    let page_id = ctx.create_page();
    let owner = "input-owner";
    let sibling = "input-sibling";
    ctx.sessions.insert(owner.to_string(), page_id.clone());
    ctx.sessions.insert(sibling.to_string(), page_id.clone());
    cdp(&mut ctx, 1, "Page.navigate", json!({"url":url.clone(),"waitUntil":"load"}), owner).await;
    cdp(&mut ctx, 2, "Runtime.evaluate", json!({
        "expression":"field.focus();field.setSelectionRange(0,0)",
        "returnByValue":true
    }), owner).await;

    cdp(&mut ctx, 3, "Input.setIgnoreInputEvents", json!({"ignore":true}), owner).await;
    cdp(&mut ctx, 4, "Input.setIgnoreInputEvents", json!({"ignore":false}), sibling).await;
    for (id, phase) in [(5, "mousePressed"), (6, "mouseReleased")] {
        cdp(&mut ctx, id, "Input.dispatchMouseEvent", json!({
            "type":phase,"x":40,"y":40,"button":"left","clickCount":1
        }), sibling).await;
    }
    cdp(&mut ctx, 7, "Input.dispatchMouseEvent", json!({
        "type":"mouseWheel","x":400,"y":300,"deltaX":0,"deltaY":120
    }), sibling).await;
    cdp(&mut ctx, 8, "Input.dispatchKeyEvent", json!({
        "type":"keyDown","key":"S","code":"KeyS","text":"S","windowsVirtualKeyCode":83
    }), sibling).await;
    cdp(&mut ctx, 10, "Input.insertText", json!({"text":"I"}), owner).await;

    assert_eq!(snapshot(&mut ctx, 11, owner).await, json!({
        "value":"I", "hash":"", "scrollY":0,
        "counts":{
            "pointerdown":0,"mousedown":0,"pointerup":0,"mouseup":0,"click":0,"wheel":0,
            "keydown":0,"keypress":0,"beforeinput":1,"input":1
        }
    }));

    cdp(&mut ctx, 12, "Input.setIgnoreInputEvents", json!({"ignore":false}), owner).await;
    cdp(&mut ctx, 13, "Runtime.evaluate", json!({
        "expression":"for(const key of Object.keys(counts))counts[key]=0",
        "returnByValue":true
    }), owner).await;
    for (id, phase) in [(14, "mousePressed"), (15, "mouseReleased")] {
        cdp(&mut ctx, id, "Input.dispatchMouseEvent", json!({
            "type":phase,"x":40,"y":40,"button":"left","clickCount":1
        }), owner).await;
    }
    cdp(&mut ctx, 16, "Input.dispatchMouseEvent", json!({
        "type":"mouseWheel","x":400,"y":300,"deltaX":0,"deltaY":120
    }), owner).await;
    cdp(&mut ctx, 17, "Runtime.evaluate", json!({
        "expression":"field.focus();field.setSelectionRange(field.value.length,field.value.length)",
        "returnByValue":true
    }), owner).await;
    cdp(&mut ctx, 18, "Input.dispatchKeyEvent", json!({
        "type":"keyDown","key":"K","code":"KeyK","text":"K","windowsVirtualKeyCode":75
    }), owner).await;
    cdp(&mut ctx, 19, "Input.insertText", json!({"text":"T"}), owner).await;

    assert_eq!(snapshot(&mut ctx, 20, sibling).await, json!({
        "value":"IKT", "hash":"#clicked", "scrollY":120,
        "counts":{
            "pointerdown":1,"mousedown":1,"pointerup":1,"mouseup":1,"click":1,"wheel":1,
            "keydown":1,"keypress":1,"beforeinput":2,"input":2
        }
    }));

    cdp(&mut ctx, 21, "Input.setIgnoreInputEvents", json!({"ignore":true}), sibling).await;
    cdp(&mut ctx, 22, "Input.setIgnoreInputEvents", json!({"ignore":false}), owner).await;
    cdp(&mut ctx, 23, "Page.navigate", json!({"url":url,"waitUntil":"load"}), owner).await;
    cdp(&mut ctx, 24, "Runtime.evaluate", json!({
        "expression":"field.focus();field.setSelectionRange(0,0)",
        "returnByValue":true
    }), owner).await;
    cdp(&mut ctx, 25, "Input.dispatchKeyEvent", json!({
        "type":"keyDown","key":"N","code":"KeyN","text":"N","windowsVirtualKeyCode":78
    }), owner).await;
    cdp(&mut ctx, 26, "Input.insertText", json!({"text":"N"}), owner).await;
    assert_eq!(snapshot(&mut ctx, 27, owner).await, json!({
        "value":"N", "hash":"", "scrollY":0,
        "counts":{
            "pointerdown":0,"mousedown":0,"pointerup":0,"mouseup":0,"click":0,"wheel":0,
            "keydown":0,"keypress":0,"beforeinput":1,"input":1
        }
    }));

    let detached = raw(
        &mut ctx, 28, "Target.detachFromTarget", json!({"sessionId":sibling}), None,
    ).await;
    assert!(detached.error.is_none(), "{:?}", detached.error);
    let attached = raw(
        &mut ctx, 29, "Target.attachToTarget", json!({"targetId":page_id,"flatten":true}), None,
    ).await;
    assert!(attached.error.is_none(), "{:?}", attached.error);
    let replacement = attached.result.unwrap()["sessionId"].as_str().unwrap().to_string();
    cdp(&mut ctx, 30, "Runtime.evaluate", json!({
        "expression":"field.value='';field.focus();field.setSelectionRange(0,0);for(const key of Object.keys(counts))counts[key]=0",
        "returnByValue":true
    }), owner).await;
    cdp(&mut ctx, 31, "Input.dispatchKeyEvent", json!({
        "type":"keyDown","key":"R","code":"KeyR","text":"R","windowsVirtualKeyCode":82
    }), &replacement).await;
    assert_eq!(snapshot(&mut ctx, 32, &replacement).await, json!({
        "value":"R", "hash":"", "scrollY":0,
        "counts":{
            "pointerdown":0,"mousedown":0,"pointerup":0,"mouseup":0,"click":0,"wheel":0,
            "keydown":1,"keypress":1,"beforeinput":1,"input":1
        }
    }));
}

#[tokio::test(flavor = "current_thread")]
async fn set_ignore_requires_a_boolean_and_an_attached_page_session() {
    let mut ctx = CdpContext::new(obscura_net::EffectivePersona::builtin(
        obscura_net::StealthProfile::WindowsChrome145,
    ));
    let page_id = ctx.create_page();
    let session = "input-session";
    ctx.sessions.insert(session.to_string(), page_id);

    for (id, params) in [
        (1, json!({})),
        (2, json!({"ignore":"true"})),
        (3, json!({"ignore":1})),
        (4, json!({"ignore":null})),
    ] {
        let response = raw(&mut ctx, id, "Input.setIgnoreInputEvents", params, Some(session)).await;
        assert_eq!(response.error.as_ref().map(|error| error.code), Some(-32602));
    }
    for (id, ignored) in [(5, true), (6, false)] {
        let response = raw(
            &mut ctx, id, "Input.setIgnoreInputEvents", json!({"ignore":ignored}), Some(session),
        ).await;
        assert!(response.error.is_none(), "{:?}", response.error);
        assert_eq!(response.result, Some(json!({})));
    }
    for (id, session_id) in [(7, None), (8, Some("missing-session"))] {
        let response = raw(
            &mut ctx, id, "Input.setIgnoreInputEvents", json!({"ignore":true}), session_id,
        ).await;
        assert_eq!(response.error.as_ref().map(|error| error.code), Some(-32000));
        assert!(response.error.unwrap().message.contains("attached page session"));
    }
}
