#![cfg(feature = "render")]

use obscura_cdp::dispatch::{dispatch, CdpContext};
use obscura_cdp::types::CdpRequest;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

async fn serve_fixture() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 2048];
        let _ = socket.read(&mut buf).await.unwrap();
        let body = r#"<!doctype html><html><head><style>
            html, body { margin: 0; }
            #page { width: 1800px; height: 2400px; }
            #box { position: absolute; left: 20px; top: 20px; width: 180px;
                   height: 120px; overflow: auto; border: 10px solid black; }
            #inner { width: 700px; height: 800px; }
            #check, #radio-a, #radio-b { position: absolute; margin: 0;
                width: 24px; height: 24px; }
            #check { left: 300px; top: 20px; }
            #radio-a { left: 340px; top: 20px; }
            #radio-b { left: 380px; top: 20px; }
        </style></head><body>
          <div id="page"></div>
          <div id="box"><div id="inner"></div></div>
          <input id="check" type="checkbox">
          <form id="radio-form">
            <input id="radio-a" type="radio" name="choice" checked>
            <input id="radio-b" type="radio" name="choice">
          </form>
        </body></html>"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = socket.write_all(response.as_bytes()).await;
    });
    format!("http://{addr}/")
}

async fn cdp(
    ctx: &mut CdpContext,
    id: u64,
    method: &str,
    params: Value,
    session_id: &str,
) -> Value {
    let response = dispatch(
        &CdpRequest {
            id,
            method: method.to_string(),
            params,
            session_id: Some(session_id.to_string()),
        },
        ctx,
    )
    .await;
    assert!(response.error.is_none(), "CDP {method} failed: {:?}", response.error);
    response.result.unwrap_or_else(|| json!({}))
}

async fn evaluate(ctx: &mut CdpContext, id: u64, expression: &str, session_id: &str) -> Value {
    cdp(
        ctx,
        id,
        "Runtime.evaluate",
        json!({"expression": expression, "returnByValue": true, "awaitPromise": true}),
        session_id,
    )
    .await
}

async fn setup() -> (CdpContext, String) {
    std::env::set_var("OBSCURA_ALLOW_PRIVATE_NETWORK", "1");
    let url = serve_fixture().await;
    let mut ctx = CdpContext::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));
    let page_id = ctx.create_page();
    let session_id = "input-mouse-session";
    ctx.sessions.insert(session_id.to_string(), page_id);
    cdp(
        &mut ctx,
        1,
        "Page.navigate",
        json!({"url": url, "waitUntil": "load"}),
        session_id,
    )
    .await;
    (ctx, session_id.to_string())
}

async fn wheel(ctx: &mut CdpContext, id: u64, sid: &str, x: f64, y: f64, dx: f64, dy: f64) {
    cdp(
        ctx,
        id,
        "Input.dispatchMouseEvent",
        json!({"type": "mouseWheel", "x": x, "y": y, "deltaX": dx, "deltaY": dy}),
        sid,
    )
    .await;
}

async fn scroll_state(ctx: &mut CdpContext, id: u64, sid: &str) -> Value {
    let result = evaluate(
        ctx,
        id,
        r#"JSON.stringify({
            rootX: scrollX, rootY: scrollY,
            boxX: document.getElementById('box').scrollLeft,
            boxY: document.getElementById('box').scrollTop,
            rootScrollWidth: document.scrollingElement.scrollWidth,
            rootClientWidth: document.scrollingElement.clientWidth,
            pageRect: document.getElementById('page').getBoundingClientRect().toJSON(),
            maxBoxX: document.getElementById('box').scrollWidth - document.getElementById('box').clientWidth,
            maxBoxY: document.getElementById('box').scrollHeight - document.getElementById('box').clientHeight
        })"#,
        sid,
    )
    .await;
    serde_json::from_str(result["result"]["value"].as_str().unwrap()).unwrap()
}

#[tokio::test(flavor = "current_thread")]
async fn wheel_over_page_scrolls_the_root_on_both_axes() {
    let (mut ctx, sid) = setup().await;
    wheel(&mut ctx, 2, &sid, 600.0, 300.0, 45.0, 160.0).await;
    let state = scroll_state(&mut ctx, 3, &sid).await;
    assert_eq!(state["rootX"], 45.0, "unexpected root geometry: {state}");
    assert_eq!(state["rootY"], 160.0);
    assert_eq!(state["boxX"], 0.0);
    assert_eq!(state["boxY"], 0.0);
}

#[tokio::test(flavor = "current_thread")]
async fn wheel_over_nested_overflow_scrolls_the_nested_container() {
    let (mut ctx, sid) = setup().await;
    wheel(&mut ctx, 2, &sid, 50.0, 50.0, 70.0, 110.0).await;
    let state = scroll_state(&mut ctx, 3, &sid).await;
    assert_eq!(state["boxX"], 70.0);
    assert_eq!(state["boxY"], 110.0);
    assert_eq!(state["rootX"], 0.0, "nested wheel must not leak to the viewport");
    assert_eq!(state["rootY"], 0.0, "nested wheel must not leak to the viewport");
}

#[tokio::test(flavor = "current_thread")]
async fn wheel_offsets_clamp_to_nested_scroll_extents() {
    let (mut ctx, sid) = setup().await;
    wheel(&mut ctx, 2, &sid, 50.0, 50.0, 100_000.0, 100_000.0).await;
    let state = scroll_state(&mut ctx, 3, &sid).await;
    assert_eq!(state["boxX"], state["maxBoxX"]);
    assert_eq!(state["boxY"], state["maxBoxY"]);

    wheel(&mut ctx, 4, &sid, 50.0, 50.0, -100_000.0, -100_000.0).await;
    let state = scroll_state(&mut ctx, 5, &sid).await;
    assert_eq!(state["boxX"], 0.0);
    assert_eq!(state["boxY"], 0.0);
}

#[tokio::test(flavor = "current_thread")]
async fn wheel_chains_to_root_when_nested_scroller_is_saturated() {
    let (mut ctx, sid) = setup().await;
    evaluate(
        &mut ctx,
        2,
        "(() => { const box = document.getElementById('box'); box.scrollTop = box.scrollHeight; })()",
        &sid,
    )
    .await;
    let saturated = scroll_state(&mut ctx, 3, &sid).await;
    assert_eq!(saturated["boxY"], saturated["maxBoxY"]);

    wheel(&mut ctx, 4, &sid, 50.0, 50.0, 0.0, 90.0).await;
    let state = scroll_state(&mut ctx, 5, &sid).await;
    assert_eq!(state["boxY"], state["maxBoxY"], "inner remains clamped");
    assert_eq!(state["rootY"], 90.0, "remaining wheel gesture chains to the viewport");
}

#[tokio::test(flavor = "current_thread")]
async fn canceling_wheel_prevents_its_scroll_default() {
    let (mut ctx, sid) = setup().await;
    evaluate(
        &mut ctx,
        2,
        r#"(() => {
            globalThis.wheelProbe = null;
            const page = document.getElementById('page');
            page.addEventListener('wheel', event => {
                wheelProbe = {
                    x: event.clientX, y: event.clientY,
                    dx: event.deltaX, dy: event.deltaY,
                    ctrl: event.ctrlKey, trusted: event.isTrusted
                };
                event.preventDefault();
            });
        })()"#,
        &sid,
    )
    .await;
    cdp(
        &mut ctx,
        3,
        "Input.dispatchMouseEvent",
        json!({
            "type": "mouseWheel", "x": 600.0, "y": 300.0,
            "deltaX": 25.0, "deltaY": 75.0, "modifiers": 2
        }),
        &sid,
    )
    .await;
    let state = scroll_state(&mut ctx, 4, &sid).await;
    assert_eq!(state["rootX"], 0.0);
    assert_eq!(state["rootY"], 0.0);
    let probe = evaluate(&mut ctx, 5, "JSON.stringify(wheelProbe)", &sid).await;
    let probe: Value = serde_json::from_str(probe["result"]["value"].as_str().unwrap()).unwrap();
    assert_eq!(probe["x"], 600.0);
    assert_eq!(probe["y"], 300.0);
    assert_eq!(probe["dx"], 25.0);
    assert_eq!(probe["dy"], 75.0);
    assert_eq!(probe["ctrl"], true);
    assert_eq!(probe["trusted"], true);
}

#[tokio::test(flavor = "current_thread")]
async fn hit_testing_clips_scrolled_children_at_overflow_padding_edge() {
    let (mut ctx, sid) = setup().await;
    let result = evaluate(
        &mut ctx,
        2,
        r#"(() => {
            const box = document.getElementById('box');
            box.scrollLeft = 50;
            const inner = document.getElementById('inner').getBoundingClientRect();
            return JSON.stringify({
                hit: document.elementFromPoint(25, 50).id,
                innerLeft: inner.left, innerRight: inner.right,
                boxLeft: box.getBoundingClientRect().left
            });
        })()"#,
        &sid,
    )
    .await;
    let result: Value = serde_json::from_str(result["result"]["value"].as_str().unwrap()).unwrap();
    assert!(result["innerLeft"].as_f64().unwrap() <= 25.0);
    assert!(result["innerRight"].as_f64().unwrap() >= 25.0);
    assert_eq!(result["boxLeft"], 20.0);
    assert_eq!(result["hit"], "box", "content hidden behind the border cannot win hit testing");
}

#[tokio::test(flavor = "current_thread")]
async fn press_release_orders_events_and_defers_click_activation() {
    let (mut ctx, sid) = setup().await;
    evaluate(
        &mut ctx,
        2,
        r#"(() => {
            const target = document.getElementById('check');
            globalThis.mouseLog = [];
            for (const type of ['pointerdown', 'mousedown', 'focus', 'focusin', 'pointerup', 'mouseup', 'click', 'input', 'change']) {
                target.addEventListener(type, event => mouseLog.push({
                    type, checked: target.checked, x: event.clientX,
                    ctrl: event.ctrlKey, shift: event.shiftKey, trusted: event.isTrusted
                }));
            }
        })()"#,
        &sid,
    )
    .await;

    cdp(
        &mut ctx,
        3,
        "Input.dispatchMouseEvent",
        json!({
            "type": "mousePressed", "x": 312.0, "y": 32.0,
            "button": "left", "clickCount": 1, "modifiers": 10
        }),
        &sid,
    )
    .await;
    let pressed = evaluate(
        &mut ctx,
        4,
        "JSON.stringify({log: mouseLog, checked: document.getElementById('check').checked})",
        &sid,
    )
    .await;
    let pressed: Value = serde_json::from_str(pressed["result"]["value"].as_str().unwrap()).unwrap();
    assert_eq!(pressed["checked"], false, "checkbox activation must wait for release");
    assert_eq!(pressed["log"][0]["type"], "pointerdown");
    assert_eq!(pressed["log"].as_array().unwrap().len(), 4, "press must not synthesize click");

    cdp(
        &mut ctx,
        5,
        "Input.dispatchMouseEvent",
        json!({
            "type": "mouseReleased", "x": 312.0, "y": 32.0,
            "button": "left", "clickCount": 1, "modifiers": 10
        }),
        &sid,
    )
    .await;
    let released = evaluate(
        &mut ctx,
        6,
        "JSON.stringify({log: mouseLog, checked: document.getElementById('check').checked})",
        &sid,
    )
    .await;
    let released: Value = serde_json::from_str(released["result"]["value"].as_str().unwrap()).unwrap();
    let types: Vec<&str> = released["log"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["type"].as_str().unwrap())
        .collect();
    assert_eq!(types, ["pointerdown", "mousedown", "focus", "focusin", "pointerup", "mouseup", "click", "input", "change"]);
    assert_eq!(released["checked"], true);
    assert_eq!(released["log"][6]["checked"], true, "click sees checkbox pre-activation");
    assert_eq!(released["log"][6]["x"], 312.0);
    assert_eq!(released["log"][6]["ctrl"], true);
    assert_eq!(released["log"][6]["shift"], true);
    assert_eq!(released["log"][6]["trusted"], true);
}

#[tokio::test(flavor = "current_thread")]
async fn radio_release_selects_only_the_target_in_its_group() {
    let (mut ctx, sid) = setup().await;
    evaluate(
        &mut ctx,
        2,
        r#"(() => {
            const a = document.getElementById('radio-a');
            const b = document.getElementById('radio-b');
            globalThis.radioEvents = [];
            for (const radio of [a, b]) {
                for (const type of ['mousedown', 'mouseup', 'click', 'input', 'change']) {
                    radio.addEventListener(type, () => radioEvents.push(radio.id + ':' + type));
                }
            }
        })()"#,
        &sid,
    )
    .await;
    cdp(
        &mut ctx,
        3,
        "Input.dispatchMouseEvent",
        json!({"type": "mousePressed", "x": 392.0, "y": 32.0, "button": "left", "clickCount": 1}),
        &sid,
    )
    .await;
    cdp(
        &mut ctx,
        4,
        "Input.dispatchMouseEvent",
        json!({"type": "mouseReleased", "x": 392.0, "y": 32.0, "button": "left", "clickCount": 1}),
        &sid,
    )
    .await;
    let result = evaluate(
        &mut ctx,
        5,
        "JSON.stringify({a: document.getElementById('radio-a').checked, b: document.getElementById('radio-b').checked, events: radioEvents})",
        &sid,
    )
    .await;
    let result: Value = serde_json::from_str(result["result"]["value"].as_str().unwrap()).unwrap();
    assert_eq!(result["a"], false);
    assert_eq!(result["b"], true);
    assert_eq!(
        result["events"],
        json!(["radio-b:mousedown", "radio-b:mouseup", "radio-b:click", "radio-b:input", "radio-b:change"]),
        "the newly selected radio alone receives activation events"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn canceled_click_restores_checkbox_preactivation_without_change_events() {
    let (mut ctx, sid) = setup().await;
    evaluate(&mut ctx, 2, r#"(() => {
        const check = document.getElementById('check');
        check.indeterminate = true;
        globalThis.cancelLog = [];
        check.addEventListener('click', e => {
            cancelLog.push([e.type, check.checked, check.indeterminate]);
            e.preventDefault();
        });
        for (const type of ['input', 'change']) check.addEventListener(type, () => cancelLog.push([type]));
    })()"#, &sid).await;
    for (id, phase) in [(3, "mousePressed"), (4, "mouseReleased")] {
        cdp(&mut ctx, id, "Input.dispatchMouseEvent",
            json!({"type":phase,"x":312,"y":32,"button":"left","clickCount":1}), &sid).await;
    }
    let result = evaluate(&mut ctx, 5, "JSON.stringify({log:cancelLog,checked:document.getElementById('check').checked,indeterminate:document.getElementById('check').indeterminate})", &sid).await;
    let state: Value = serde_json::from_str(result["result"]["value"].as_str().unwrap()).unwrap();
    assert_eq!(state, json!({"log":[["click",true,false]],"checked":false,"indeterminate":true}));
}

#[tokio::test(flavor = "current_thread")]
async fn canceled_mousedown_skips_focus_but_keeps_release_and_click() {
    let (mut ctx, sid) = setup().await;
    evaluate(&mut ctx, 2, r#"(() => {
        const check = document.getElementById('check');
        globalThis.cancelLog = [];
        check.addEventListener('mousedown', e => e.preventDefault());
        for (const type of ['focus', 'mouseup', 'click']) check.addEventListener(type, () => cancelLog.push(type));
    })()"#, &sid).await;
    for (id, phase) in [(3, "mousePressed"), (4, "mouseReleased")] {
        cdp(&mut ctx, id, "Input.dispatchMouseEvent",
            json!({"type":phase,"x":312,"y":32,"button":"left","clickCount":1}), &sid).await;
    }
    let result = evaluate(&mut ctx, 5, "JSON.stringify({log:cancelLog,checked:document.getElementById('check').checked})", &sid).await;
    let state: Value = serde_json::from_str(result["result"]["value"].as_str().unwrap()).unwrap();
    assert_eq!(state, json!({"log":["mouseup","click"],"checked":true}));
}

#[tokio::test(flavor = "current_thread")]
async fn right_button_preserves_metadata_and_does_not_activate_checkbox() {
    let (mut ctx, sid) = setup().await;
    evaluate(&mut ctx, 2, r#"(() => {
        const check = document.getElementById('check');
        globalThis.buttonLog = [];
        for (const type of ['mousedown', 'mouseup', 'click']) check.addEventListener(type, e => {
            buttonLog.push([type,e.button,e.buttons,e.detail,e.altKey,e.ctrlKey,e.metaKey,e.shiftKey,e.isTrusted]);
        });
    })()"#, &sid).await;
    for (id, phase) in [(3, "mousePressed"), (4, "mouseReleased")] {
        cdp(&mut ctx, id, "Input.dispatchMouseEvent",
            json!({"type":phase,"x":312,"y":32,"button":"right","clickCount":2,"modifiers":15}), &sid).await;
    }
    let result = evaluate(&mut ctx, 5, "JSON.stringify({log:buttonLog,checked:document.getElementById('check').checked})", &sid).await;
    let state: Value = serde_json::from_str(result["result"]["value"].as_str().unwrap()).unwrap();
    assert_eq!(state, json!({"log":[
        ["mousedown",2,2,2,true,true,true,true,true],
        ["mouseup",2,0,2,true,true,true,true,true]
    ],"checked":false}));
}

#[tokio::test(flavor = "current_thread")]
async fn public_mouse_overrides_cannot_redirect_native_click() {
    let (mut ctx, sid) = setup().await;
    evaluate(&mut ctx, 2, r#"(() => {
        const check = document.getElementById('check');
        globalThis.nativeClicks = 0;
        check.addEventListener('click', () => nativeClicks++);
        document.elementFromPoint = () => document.getElementById('radio-b');
        globalThis.MouseEvent = function() { throw new Error('public constructor'); };
        globalThis.PointerEvent = function() { throw new Error('public constructor'); };
        check.dispatchEvent = function() { throw new Error('public dispatcher'); };
        globalThis.__obscura_mouse_down = {target:document.getElementById('radio-b'),button:0};
    })()"#, &sid).await;
    for (id, phase) in [(3, "mousePressed"), (4, "mouseReleased")] {
        cdp(&mut ctx, id, "Input.dispatchMouseEvent",
            json!({"type":phase,"x":312,"y":32,"button":"left","clickCount":1}), &sid).await;
    }
    let result = evaluate(&mut ctx, 5, "JSON.stringify({clicks:nativeClicks,checked:document.getElementById('check').checked,radio:document.getElementById('radio-b').checked})", &sid).await;
    let state: Value = serde_json::from_str(result["result"]["value"].as_str().unwrap()).unwrap();
    assert_eq!(state, json!({"clicks":1,"checked":true,"radio":false}));
}

#[tokio::test(flavor = "current_thread")]
async fn mouse_move_uses_protocol_defaults_and_preserves_explicit_buttons() {
    let (mut ctx, sid) = setup().await;
    evaluate(&mut ctx, 2, r#"(() => {
        globalThis.moveLog = [];
        for (const type of ['pointermove','mousemove']) {
            document.getElementById('check').addEventListener(type, e => {
                moveLog.push([type,e.button,e.buttons,e.detail,e.altKey,e.ctrlKey,e.metaKey,e.shiftKey]);
            });
        }
    })()"#, &sid).await;
    cdp(&mut ctx, 3, "Input.dispatchMouseEvent",
        json!({"type":"mouseMoved","x":312,"y":32}), &sid).await;
    cdp(&mut ctx, 4, "Input.dispatchMouseEvent",
        json!({"type":"mouseMoved","x":312,"y":32,"button":"left","buttons":3,"modifiers":5}), &sid).await;
    let result = evaluate(&mut ctx, 5, "JSON.stringify(moveLog)", &sid).await;
    let log: Value = serde_json::from_str(result["result"]["value"].as_str().unwrap()).unwrap();
    assert_eq!(log, json!([
        ["pointermove",-1,0,0,false,false,false,false],
        ["mousemove",0,0,0,false,false,false,false],
        ["pointermove",-1,3,0,true,false,true,false],
        ["mousemove",0,3,0,true,false,true,false]
    ]));
}

#[tokio::test(flavor = "current_thread")]
async fn release_on_sibling_clicks_only_the_common_ancestor_once() {
    let (mut ctx, sid) = setup().await;
    evaluate(&mut ctx, 2, r#"(() => {
        document.body.innerHTML = '<div id="parent" style="position:absolute;left:20px;top:20px;width:200px;height:60px"><div id="left" style="position:absolute;left:0;top:0;width:80px;height:60px"></div><div id="right" style="position:absolute;left:120px;top:0;width:80px;height:60px"></div></div>';
        globalThis.phaseLog = [];
        for (const id of ['left','right','parent']) {
            document.getElementById(id).addEventListener('click', e => phaseLog.push([id,e.target.id]));
        }
    })()"#, &sid).await;
    cdp(&mut ctx, 3, "Input.dispatchMouseEvent",
        json!({"type":"mousePressed","x":40,"y":40,"button":"left","clickCount":1}), &sid).await;
    cdp(&mut ctx, 4, "Input.dispatchMouseEvent",
        json!({"type":"mouseReleased","x":160,"y":40,"button":"left","clickCount":1}), &sid).await;
    let result = evaluate(&mut ctx, 5, "JSON.stringify(phaseLog)", &sid).await;
    let log: Value = serde_json::from_str(result["result"]["value"].as_str().unwrap()).unwrap();
    assert_eq!(log, json!([["parent","parent"]]));
}

#[tokio::test(flavor = "current_thread")]
async fn mousedown_layout_change_allows_release_to_hit_the_current_target() {
    let (mut ctx, sid) = setup().await;
    evaluate(&mut ctx, 2, r#"(() => {
        document.body.innerHTML = '<div id="parent" style="position:absolute;left:20px;top:20px;width:300px;height:60px"><div id="under" style="position:absolute;left:0;top:0;width:80px;height:60px"></div><div id="moving" style="position:absolute;left:0;top:0;width:80px;height:60px;z-index:1"></div></div>';
        globalThis.phaseLog = [];
        document.getElementById('moving').addEventListener('mousedown', e => {
            phaseLog.push(['down',e.target.id]);
            e.target.style.left = '180px';
        });
        document.getElementById('parent').addEventListener('mouseup', e => phaseLog.push(['up',e.target.id]));
        document.getElementById('parent').addEventListener('click', e => phaseLog.push(['click',e.target.id]));
    })()"#, &sid).await;
    for (id, phase) in [(3, "mousePressed"), (4, "mouseReleased")] {
        cdp(&mut ctx, id, "Input.dispatchMouseEvent",
            json!({"type":phase,"x":40,"y":40,"button":"left","clickCount":1}), &sid).await;
    }
    let result = evaluate(&mut ctx, 5, "JSON.stringify(phaseLog)", &sid).await;
    let log: Value = serde_json::from_str(result["result"]["value"].as_str().unwrap()).unwrap();
    assert_eq!(log, json!([["down","moving"],["up","under"],["click","parent"]]));
}

#[tokio::test(flavor = "current_thread")]
async fn document_replacement_discards_the_old_mouse_press() {
    let (mut ctx, sid) = setup().await;
    evaluate(&mut ctx, 2, r#"(() => {
        document.body.innerHTML = '<button id="old" style="position:absolute;left:20px;top:20px;width:100px;height:40px">old</button>';
    })()"#, &sid).await;
    cdp(&mut ctx, 3, "Input.dispatchMouseEvent",
        json!({"type":"mousePressed","x":40,"y":35,"button":"left","clickCount":1}), &sid).await;
    evaluate(&mut ctx, 4, r#"(() => {
        document.open();
        document.write('<!doctype html><html><body><button id="new" style="position:absolute;left:20px;top:20px;width:100px;height:40px">new</button></body></html>');
        document.close();
        globalThis.replacementLog = [];
        for (const type of ['mouseup','click']) {
            document.getElementById('new').addEventListener(type, e => replacementLog.push([type,e.target.id]));
        }
    })()"#, &sid).await;
    cdp(&mut ctx, 5, "Input.dispatchMouseEvent",
        json!({"type":"mouseReleased","x":40,"y":35,"button":"left","clickCount":1}), &sid).await;
    let result = evaluate(&mut ctx, 6, "JSON.stringify(replacementLog)", &sid).await;
    let log: Value = serde_json::from_str(result["result"]["value"].as_str().unwrap()).unwrap();
    assert_eq!(log, json!([["mouseup","new"]]), "new document must not inherit a press, even when node ids are reused");
}

#[tokio::test(flavor = "current_thread")]
async fn canceled_pointerdown_suppresses_compatibility_mouse_but_not_click_default() {
    let (mut ctx, sid) = setup().await;
    evaluate(&mut ctx, 2, r#"(() => {
        const check = document.getElementById('check');
        globalThis.pointerCancelLog = [];
        check.addEventListener('pointerdown', e => e.preventDefault());
        for (const type of ['pointerdown','mousedown','focus','pointerup','mouseup','click','input','change']) {
            check.addEventListener(type, e => pointerCancelLog.push([type,check.checked]));
        }
    })()"#, &sid).await;
    for (id, phase) in [(3, "mousePressed"), (4, "mouseReleased")] {
        cdp(&mut ctx, id, "Input.dispatchMouseEvent",
            json!({"type":phase,"x":312,"y":32,"button":"left","clickCount":1}), &sid).await;
    }
    let result = evaluate(&mut ctx, 5, "JSON.stringify({log:pointerCancelLog,checked:document.getElementById('check').checked})", &sid).await;
    let state: Value = serde_json::from_str(result["result"]["value"].as_str().unwrap()).unwrap();
    assert_eq!(state, json!({"log":[["pointerdown",false],["pointerup",false],["click",true],["input",true],["change",true]],"checked":true}));
}

#[tokio::test(flavor = "current_thread")]
async fn document_open_discards_a_press_on_the_retained_body_node() {
    let (mut ctx, sid) = setup().await;
    evaluate(&mut ctx, 2, r#"(() => {
        document.body.innerHTML = '';
        document.body.style.cssText = 'margin:0;width:600px;height:400px';
        globalThis.retainedBodyLog = [];
        document.body.addEventListener('mousedown', e => retainedBodyLog.push(['down',e.target.tagName]));
    })()"#, &sid).await;
    cdp(&mut ctx, 3, "Input.dispatchMouseEvent",
        json!({"type":"mousePressed","x":40,"y":35,"button":"left","clickCount":1}), &sid).await;
    let pressed = evaluate(&mut ctx, 4, "JSON.stringify(retainedBodyLog)", &sid).await;
    let pressed: Value = serde_json::from_str(pressed["result"]["value"].as_str().unwrap()).unwrap();
    assert_eq!(pressed, json!([["down","BODY"]]), "the press must hit the node document.open retains");
    evaluate(&mut ctx, 5, r#"(() => {
        document.open();
        document.write('<button id="replacement" style="position:absolute;left:20px;top:20px;width:100px;height:40px">new</button>');
        document.close();
        globalThis.retainedBodyLog = [];
        for (const type of ['mouseup','click']) {
            document.body.addEventListener(type, e => retainedBodyLog.push([type,e.target.id || e.target.tagName]));
        }
    })()"#, &sid).await;
    cdp(&mut ctx, 6, "Input.dispatchMouseEvent",
        json!({"type":"mouseReleased","x":40,"y":35,"button":"left","clickCount":1}), &sid).await;
    let result = evaluate(&mut ctx, 7, "JSON.stringify(retainedBodyLog)", &sid).await;
    let log: Value = serde_json::from_str(result["result"]["value"].as_str().unwrap()).unwrap();
    assert_eq!(log, json!([["mouseup","replacement"]]), "retained body identity must not preserve the old press epoch");
}

#[tokio::test(flavor = "current_thread")]
async fn triple_click_document_replacement_does_not_select_the_new_textarea() {
    let (mut ctx, sid) = setup().await;
    evaluate(&mut ctx, 2, r#"(() => {
        document.body.innerHTML = '<textarea id="original" style="position:absolute;left:20px;top:20px;width:200px;height:60px">old content</textarea>';
        globalThis.originalTextArea = document.getElementById('original');
        globalThis.replacementClicks = 0;
        originalTextArea.addEventListener('click', () => {
            replacementClicks++;
            document.open();
            document.write('<textarea id="replacement" style="position:absolute;left:20px;top:20px;width:200px;height:60px">replacement text</textarea>');
            document.close();
            document.getElementById('replacement').setSelectionRange(2,2);
        });
    })()"#, &sid).await;
    for (id, phase) in [(3, "mousePressed"), (4, "mouseReleased")] {
        cdp(&mut ctx, id, "Input.dispatchMouseEvent",
            json!({"type":phase,"x":40,"y":35,"button":"left","clickCount":3}), &sid).await;
    }
    let result = evaluate(&mut ctx, 5, r#"JSON.stringify({
        clicks:replacementClicks,
        value:document.getElementById('replacement').value,
        start:document.getElementById('replacement').selectionStart,
        end:document.getElementById('replacement').selectionEnd
    })"#, &sid).await;
    let state: Value = serde_json::from_str(result["result"]["value"].as_str().unwrap()).unwrap();
    assert_eq!(state["clicks"], 1, "the click callback must actually replace the document");
    assert_eq!(state["value"], "replacement text");
    assert_eq!(state["start"], 2);
    assert_eq!(state["end"], 2);
}

#[tokio::test(flavor = "current_thread")]
async fn mouse_down_and_up_without_a_button_succeed_without_dispatching_events() {
    let (mut ctx, sid) = setup().await;
    evaluate(&mut ctx, 2, r#"(() => {
        globalThis.noButtonLog = [];
        for (const type of ['pointerdown','mousedown','focus','pointerup','mouseup','click']) {
            document.getElementById('check').addEventListener(type, () => noButtonLog.push(type));
        }
    })()"#, &sid).await;
    for (id, phase) in [(3, "mousePressed"), (4, "mouseReleased")] {
        let result = cdp(&mut ctx, id, "Input.dispatchMouseEvent",
            json!({"type":phase,"x":312,"y":32}), &sid).await;
        assert_eq!(result, json!({}));
    }
    let result = evaluate(&mut ctx, 5, "JSON.stringify({log:noButtonLog,checked:document.getElementById('check').checked})", &sid).await;
    let state: Value = serde_json::from_str(result["result"]["value"].as_str().unwrap()).unwrap();
    assert_eq!(state, json!({"log":[],"checked":false}));
}

#[tokio::test(flavor = "current_thread")]
async fn changed_left_button_is_applied_even_when_explicit_buttons_is_zero() {
    let (mut ctx, sid) = setup().await;
    evaluate(&mut ctx, 2, r#"(() => {
        globalThis.explicitButtonsLog = [];
        for (const type of ['mousedown','mouseup']) {
            document.getElementById('check').addEventListener(type, e => explicitButtonsLog.push([type,e.button,e.buttons,e.detail]));
        }
    })()"#, &sid).await;
    for (id, phase) in [(3, "mousePressed"), (4, "mouseReleased")] {
        cdp(&mut ctx, id, "Input.dispatchMouseEvent",
            json!({"type":phase,"x":312,"y":32,"button":"left","buttons":0}), &sid).await;
    }
    let result = evaluate(&mut ctx, 5, "JSON.stringify(explicitButtonsLog)", &sid).await;
    let log: Value = serde_json::from_str(result["result"]["value"].as_str().unwrap()).unwrap();
    assert_eq!(log, json!([["mousedown",0,1,0],["mouseup",0,0,0]]));
}

#[tokio::test(flavor = "current_thread")]
async fn mouse_force_and_native_event_interfaces_match_the_wire_metadata() {
    let (mut ctx, sid) = setup().await;
    evaluate(&mut ctx, 2, r#"(() => {
        globalThis.forceLog = [];
        for (const type of ['pointerdown','mousedown','focus','focusin','pointerup','mouseup','click']) {
            document.getElementById('check').addEventListener(type, e => forceLog.push({
                type, constructor:e.constructor.name, detail:e.detail,
                pressure:typeof e.pressure === 'undefined' ? null : e.pressure,
                hasPressure:'pressure' in e, view:e.view === window
            }));
        }
    })()"#, &sid).await;
    cdp(&mut ctx, 3, "Input.dispatchMouseEvent",
        json!({"type":"mousePressed","x":312,"y":32,"button":"left","clickCount":1,"force":0.5}), &sid).await;
    cdp(&mut ctx, 4, "Input.dispatchMouseEvent",
        json!({"type":"mouseReleased","x":312,"y":32,"button":"left","clickCount":1}), &sid).await;
    let result = evaluate(&mut ctx, 5, "JSON.stringify(forceLog)", &sid).await;
    let log: Value = serde_json::from_str(result["result"]["value"].as_str().unwrap()).unwrap();
    assert_eq!(log, json!([
        {"type":"pointerdown","constructor":"PointerEvent","detail":0,"pressure":0.5,"hasPressure":true,"view":true},
        {"type":"mousedown","constructor":"MouseEvent","detail":1,"pressure":null,"hasPressure":false,"view":true},
        {"type":"focus","constructor":"FocusEvent","detail":0,"pressure":null,"hasPressure":false,"view":true},
        {"type":"focusin","constructor":"FocusEvent","detail":0,"pressure":null,"hasPressure":false,"view":true},
        {"type":"pointerup","constructor":"PointerEvent","detail":0,"pressure":0,"hasPressure":true,"view":true},
        {"type":"mouseup","constructor":"MouseEvent","detail":1,"pressure":null,"hasPressure":false,"view":true},
        {"type":"click","constructor":"PointerEvent","detail":1,"pressure":0,"hasPressure":true,"view":true}
    ]));
}
