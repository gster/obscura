//! CDP keyboard and text input must preserve arbitrary protocol strings while
//! routing them through the browser-owned native input path. These regressions
//! retain the escaping cases that previously broke generated page scripts and
//! also cover protected event delivery, focus/document reentry and validation.

use obscura_cdp::dispatch::{dispatch, CdpContext};
use obscura_cdp::types::CdpRequest;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

// Serves a page that records the `key` of the last keydown event on the body.
async fn serve_page() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        tokio::spawn(async move {
            let mut buf = [0u8; 2048];
            let _ = socket.read(&mut buf).await.unwrap();
            let body = r#"<html><body>
<input id="i">
<textarea id="a"></textarea>
<script>
window.__keys = [];
document.body.addEventListener('keydown', function (e) { window.__keys.push(e.key + '|' + e.code); });
</script>
</body></html>"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = socket.write_all(resp.as_bytes()).await;
        });
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

#[tokio::test(flavor = "current_thread")]
async fn dispatch_key_event_escapes_backslash_in_key_and_code() {
    std::env::set_var("OBSCURA_ALLOW_PRIVATE_NETWORK", "1");
    let url = serve_page().await;
    let mut ctx = CdpContext::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));
    let page_id = ctx.create_page();
    let session_id = "session-1";
    ctx.sessions.insert(session_id.to_string(), page_id.clone());

    cdp(&mut ctx, 1, "Page.navigate", json!({"url": url, "waitUntil": "load"}), session_id).await;

    // The backslash key: Chrome sends key="\" (a single backslash) code="Backslash".
    cdp(
        &mut ctx,
        2,
        "Input.dispatchKeyEvent",
        json!({"type": "keyDown", "key": "\\", "code": "Backslash"}),
        session_id,
    )
    .await;

    // A key whose name itself contains a quote AND a backslash, to exercise the
    // ordering of the two replacements.
    cdp(
        &mut ctx,
        3,
        "Input.dispatchKeyEvent",
        json!({"type": "keyDown", "key": "a", "code": "KeyA"}),
        session_id,
    )
    .await;

    let v = cdp(
        &mut ctx,
        4,
        "Runtime.evaluate",
        json!({"expression": "JSON.stringify(window.__keys)", "returnByValue": true}),
        session_id,
    )
    .await;

    let keys: Vec<String> =
        serde_json::from_str(v["result"]["value"].as_str().unwrap()).unwrap();
    assert_eq!(
        keys,
        vec!["\\|Backslash".to_string(), "a|KeyA".to_string()],
        "the backslash key must be dispatched, not dropped by a malformed snippet"
    );
}

// Keep the control-character case that used to be dropped by generated JS.
#[tokio::test(flavor = "current_thread")]
async fn dispatch_key_event_char_carries_a_newline_into_a_textarea() {
    std::env::set_var("OBSCURA_ALLOW_PRIVATE_NETWORK", "1");
    let url = serve_page().await;
    let mut ctx = CdpContext::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));
    let page_id = ctx.create_page();
    let session_id = "session-1";
    ctx.sessions.insert(session_id.to_string(), page_id.clone());

    cdp(&mut ctx, 1, "Page.navigate", json!({"url": url, "waitUntil": "load"}), session_id).await;
    cdp(
        &mut ctx,
        2,
        "Runtime.evaluate",
        json!({
            "expression": "(function () { document.getElementById('a').focus(); return 'ok'; })()",
            "returnByValue": true,
        }),
        session_id,
    )
    .await;

    for (id, ch) in [(3u64, "a"), (4, "\n"), (5, "b")] {
        cdp(
            &mut ctx,
            id,
            "Input.dispatchKeyEvent",
            json!({"type": "char", "text": ch}),
            session_id,
        )
        .await;
    }

    let v = cdp(
        &mut ctx,
        6,
        "Runtime.evaluate",
        json!({
            "expression": "JSON.stringify(document.getElementById('a').value)",
            "returnByValue": true,
        }),
        session_id,
    )
    .await;
    assert_eq!(
        v["result"]["value"].as_str().unwrap_or_default(),
        r#""a\nb""#,
        "a newline sent as a char must reach the field, not be dropped by a malformed snippet"
    );
}

// #577: Playwright's fill() focuses the field and sends one Input.insertText.
#[tokio::test(flavor = "current_thread")]
async fn insert_text_types_into_the_focused_field() {
    std::env::set_var("OBSCURA_ALLOW_PRIVATE_NETWORK", "1");
    let url = serve_page().await;
    let mut ctx = CdpContext::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));
    let page_id = ctx.create_page();
    let session_id = "session-1";
    ctx.sessions.insert(session_id.to_string(), page_id.clone());

    cdp(&mut ctx, 1, "Page.navigate", json!({"url": url, "waitUntil": "load"}), session_id).await;
    cdp(
        &mut ctx,
        2,
        "Runtime.evaluate",
        json!({"expression": "document.getElementById('i').focus()", "returnByValue": true}),
        session_id,
    )
    .await;
    cdp(
        &mut ctx,
        3,
        "Input.insertText",
        json!({"text": "he'll\\o\nbye"}),
        session_id,
    )
    .await;

    let v = cdp(
        &mut ctx,
        4,
        "Runtime.evaluate",
        json!({
            "expression": "JSON.stringify(document.getElementById('i').value)",
            "returnByValue": true,
        }),
        session_id,
    )
    .await;
    assert_eq!(
        v["result"]["value"].as_str().unwrap_or_default(),
        r#""he'll\\o bye""#,
        "Input.insertText preserves quotes and backslashes and maps a line break to a space"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn insert_text_uses_the_native_text_path_and_real_input_events() {
    std::env::set_var("OBSCURA_ALLOW_PRIVATE_NETWORK", "1");
    let url = serve_page().await;
    let mut ctx = CdpContext::new(obscura_net::EffectivePersona::builtin(
        obscura_net::StealthProfile::WindowsChrome145,
    ));
    let page_id = ctx.create_page();
    let session_id = "session-1";
    ctx.sessions.insert(session_id.to_string(), page_id);

    cdp(
        &mut ctx,
        1,
        "Page.navigate",
        json!({"url": url, "waitUntil": "load"}),
        session_id,
    )
    .await;
    cdp(
        &mut ctx,
        2,
        "Runtime.evaluate",
        json!({
            "expression": r#"(() => {
                const target = document.getElementById('i');
                target.value = 'A😀B';
                target.focus();
                target.setSelectionRange(1, 3);
                globalThis.__textEvents = [];
                for (const type of ['beforeinput', 'input']) {
                    target.addEventListener(type, event => __textEvents.push([
                        event.type, event.inputType, event.data, event.isTrusted,
                        event.bubbles, event.cancelable, target.value,
                    ]));
                }
                globalThis.InputEvent = function () { throw Error('public InputEvent called'); };
                Element.prototype.dispatchEvent = function () { throw Error('public dispatchEvent called'); };
                globalThis.__obscura_native_text_handoff = function () { throw Error('public handoff called'); };
            })()"#,
            "returnByValue": true,
        }),
        session_id,
    )
    .await;
    cdp(
        &mut ctx,
        3,
        "Input.insertText",
        json!({"text": "中🚀"}),
        session_id,
    )
    .await;

    let result = cdp(
        &mut ctx,
        4,
        "Runtime.evaluate",
        json!({
            "expression": "JSON.stringify([i.value,i.selectionStart,i.selectionEnd,__textEvents])",
            "returnByValue": true,
        }),
        session_id,
    )
    .await;
    let actual: Value = serde_json::from_str(result["result"]["value"].as_str().unwrap()).unwrap();
    assert_eq!(
        actual,
        json!([
            "A中🚀B",
            4,
            4,
            [
                ["beforeinput", "insertText", "中🚀", true, true, true, "A😀B"],
                ["input", "insertText", "中🚀", true, true, false, "A中🚀B"]
            ]
        ])
    );
}

#[tokio::test(flavor = "current_thread")]
async fn insert_text_applies_maxlength_after_reentry_and_emits_actual_text() {
    std::env::set_var("OBSCURA_ALLOW_PRIVATE_NETWORK", "1");
    let url = serve_page().await;
    let mut ctx = CdpContext::new(obscura_net::EffectivePersona::builtin(
        obscura_net::StealthProfile::WindowsChrome145,
    ));
    let page_id = ctx.create_page();
    let session_id = "session-1";
    ctx.sessions.insert(session_id.to_string(), page_id);
    cdp(&mut ctx, 1, "Page.navigate", json!({"url": url, "waitUntil": "load"}), session_id).await;
    cdp(&mut ctx, 2, "Runtime.evaluate", json!({
        "expression": r#"(() => {
            i.maxLength=4;i.value='ABCD';i.focus();i.setSelectionRange(1,3);
            globalThis.__maxlengthEvents=[];
            for (const type of ['beforeinput','input']) i.addEventListener(type,e=>{
                __maxlengthEvents.push([e.type,e.data,i.value,i.selectionStart,i.selectionEnd,i.getAttribute('maxlength')]);
            });
        })()"#,
        "returnByValue": true
    }), session_id).await;
    cdp(&mut ctx, 3, "Input.insertText", json!({"text":"😀X"}), session_id).await;
    let partial = cdp(&mut ctx, 4, "Runtime.evaluate", json!({
        "expression":"JSON.stringify([i.value,i.selectionStart,i.selectionEnd,__maxlengthEvents])",
        "returnByValue":true
    }), session_id).await;
    assert_eq!(
        serde_json::from_str::<Value>(partial["result"]["value"].as_str().unwrap()).unwrap(),
        json!(["A😀D",4,4,[
            ["beforeinput","😀X","ABCD",1,3,"4"],
            ["input","😀","A😀D",4,4,"4"]
        ]])
    );

    cdp(&mut ctx, 5, "Runtime.evaluate", json!({
        "expression":"__maxlengthEvents=[];i.maxLength=1;i.value='';i.setSelectionRange(0,0)",
        "returnByValue":true
    }), session_id).await;
    cdp(&mut ctx, 6, "Input.insertText", json!({"text":"😀"}), session_id).await;
    let no_capacity = cdp(&mut ctx, 7, "Runtime.evaluate", json!({
        "expression":"JSON.stringify([i.value,i.selectionStart,__maxlengthEvents])",
        "returnByValue":true
    }), session_id).await;
    assert_eq!(
        serde_json::from_str::<Value>(no_capacity["result"]["value"].as_str().unwrap()).unwrap(),
        json!(["",0,[["beforeinput","😀","",0,0,"1"]]])
    );

    cdp(&mut ctx, 8, "Runtime.evaluate", json!({
        "expression":r#"(() => {
            __maxlengthEvents=[];i.maxLength=3;i.value='ABCD';i.setSelectionRange(1,4);
        })()"#,
        "returnByValue":true
    }), session_id).await;
    cdp(&mut ctx, 9, "Input.insertText", json!({"text":""}), session_id).await;
    let deletion = cdp(&mut ctx, 10, "Runtime.evaluate", json!({
        "expression":"JSON.stringify([i.value,i.selectionStart,__maxlengthEvents])",
        "returnByValue":true
    }), session_id).await;
    assert_eq!(
        serde_json::from_str::<Value>(deletion["result"]["value"].as_str().unwrap()).unwrap(),
        json!(["A",1,[
            ["beforeinput","","ABCD",1,4,"3"],
            ["input","","A",1,1,"3"]
        ]])
    );

    cdp(&mut ctx, 11, "Runtime.evaluate", json!({
        "expression":r#"(() => {
            __maxlengthEvents=[];i.maxLength=4;i.value='X';i.setSelectionRange(1,1);
            i.addEventListener('beforeinput',()=>{i.maxLength=2},{once:true});
        })()"#,
        "returnByValue":true
    }), session_id).await;
    cdp(&mut ctx, 12, "Input.insertText", json!({"text":"ABC"}), session_id).await;
    let reentry = cdp(&mut ctx, 13, "Runtime.evaluate", json!({
        "expression":"JSON.stringify([i.value,i.selectionStart,__maxlengthEvents])",
        "returnByValue":true
    }), session_id).await;
    assert_eq!(
        serde_json::from_str::<Value>(reentry["result"]["value"].as_str().unwrap()).unwrap(),
        json!(["XA",2,[
            ["beforeinput","ABC","X",1,1,"4"],
            ["input","A","XA",2,2,"2"]
        ]])
    );

    cdp(&mut ctx, 14, "Runtime.evaluate", json!({
        "expression":"__maxlengthEvents=[];i.maxLength=2;i.value='ABC';i.setSelectionRange(1,2)",
        "returnByValue":true
    }), session_id).await;
    cdp(&mut ctx, 15, "Input.insertText", json!({"text":"X"}), session_id).await;
    let retained = cdp(&mut ctx, 16, "Runtime.evaluate", json!({
        "expression":"JSON.stringify([i.value,i.selectionStart,__maxlengthEvents])",
        "returnByValue":true
    }), session_id).await;
    assert_eq!(
        serde_json::from_str::<Value>(retained["result"]["value"].as_str().unwrap()).unwrap(),
        json!(["AC",2,[
            ["beforeinput","X","ABC",1,2,"2"],
            ["input","","AC",2,2,"2"]
        ]])
    );
}

#[tokio::test(flavor = "current_thread")]
async fn insert_text_normalizes_multiline_payloads_before_maxlength() {
    std::env::set_var("OBSCURA_ALLOW_PRIVATE_NETWORK", "1");
    let url = serve_page().await;
    let mut ctx = CdpContext::new(obscura_net::EffectivePersona::builtin(
        obscura_net::StealthProfile::WindowsChrome145,
    ));
    let page_id = ctx.create_page();
    let session_id = "session-1";
    ctx.sessions.insert(session_id.to_string(), page_id);
    cdp(&mut ctx, 1, "Page.navigate", json!({"url":url,"waitUntil":"load"}), session_id).await;
    cdp(&mut ctx, 2, "Runtime.evaluate", json!({
        "expression":r#"(() => {
            a.maxLength=3;a.value='';a.focus();a.setSelectionRange(0,0);
            globalThis.__multilineEvents=[];
            for(const type of ['beforeinput','input']) a.addEventListener(type,e=>{
                __multilineEvents.push([e.type,e.data,a.value,a.selectionStart,a.selectionEnd]);
            });
        })()"#,
        "returnByValue":true
    }), session_id).await;
    cdp(&mut ctx, 3, "Input.insertText", json!({"text":"A\r\nB😀"}), session_id).await;
    let textarea = cdp(&mut ctx, 4, "Runtime.evaluate", json!({
        "expression":"JSON.stringify([a.value,a.selectionStart,a.selectionEnd,__multilineEvents])",
        "returnByValue":true
    }), session_id).await;
    assert_eq!(
        serde_json::from_str::<Value>(textarea["result"]["value"].as_str().unwrap()).unwrap(),
        json!(["A\nB",3,3,[
            ["beforeinput","A\r\nB😀","",0,0],
            ["input","A","A\nB",3,3],
            ["input",null,"A\nB",3,3],
            ["input","B","A\nB",3,3]
        ]])
    );

    cdp(&mut ctx, 5, "Runtime.evaluate", json!({
        "expression":r#"(() => {
            i.removeAttribute('maxlength');i.value='';i.focus();i.setSelectionRange(0,0);
            globalThis.__singleLineEvents=[];
            for(const type of ['beforeinput','input']) i.addEventListener(type,e=>{
                __singleLineEvents.push([e.type,e.data,i.value,i.selectionStart]);
            });
        })()"#,
        "returnByValue":true
    }), session_id).await;
    cdp(&mut ctx, 6, "Input.insertText", json!({"text":"A\r\nB"}), session_id).await;
    let input = cdp(&mut ctx, 7, "Runtime.evaluate", json!({
        "expression":"JSON.stringify([i.value,i.selectionStart,__singleLineEvents])",
        "returnByValue":true
    }), session_id).await;
    assert_eq!(
        serde_json::from_str::<Value>(input["result"]["value"].as_str().unwrap()).unwrap(),
        json!(["A B",3,[
            ["beforeinput","A\r\nB","",0],
            ["input","A B","A B",3]
        ]])
    );
}

#[tokio::test(flavor = "current_thread")]
async fn key_text_maxlength_uses_actual_prefix_and_actual_caret() {
    std::env::set_var("OBSCURA_ALLOW_PRIVATE_NETWORK", "1");
    let url = serve_page().await;
    let mut ctx = CdpContext::new(obscura_net::EffectivePersona::builtin(
        obscura_net::StealthProfile::WindowsChrome145,
    ));
    let page_id = ctx.create_page();
    let session_id = "session-1";
    ctx.sessions.insert(session_id.to_string(), page_id);
    cdp(&mut ctx, 1, "Page.navigate", json!({"url":url,"waitUntil":"load"}), session_id).await;
    cdp(&mut ctx, 2, "Runtime.evaluate", json!({
        "expression":r#"(() => {
            i.maxLength=4;i.value='ABCD';i.focus();i.setSelectionRange(1,3);
            globalThis.__keyMaxEvents=[];
            for(const type of ['keydown','keypress','beforeinput','input']) i.addEventListener(type,e=>{
                __keyMaxEvents.push([e.type,e.data,i.value,i.selectionStart]);
            });
        })()"#,
        "returnByValue":true
    }), session_id).await;
    cdp(&mut ctx, 3, "Input.dispatchKeyEvent", json!({
        "type":"keyDown","text":"😀X"
    }), session_id).await;
    let result = cdp(&mut ctx, 4, "Runtime.evaluate", json!({
        "expression":"JSON.stringify([i.value,i.selectionStart,i.selectionEnd,__keyMaxEvents])",
        "returnByValue":true
    }), session_id).await;
    assert_eq!(
        serde_json::from_str::<Value>(result["result"]["value"].as_str().unwrap()).unwrap(),
        json!(["A😀D",3,3,[
            ["keydown",null,"ABCD",1],
            ["keypress",null,"ABCD",1],
            ["beforeinput","😀X","ABCD",1],
            ["input","😀","A😀D",3]
        ]])
    );

    cdp(&mut ctx, 5, "Runtime.evaluate", json!({
        "expression":"__keyMaxEvents=[];i.maxLength=2;i.value='ABC';i.setSelectionRange(1,2)",
        "returnByValue":true
    }), session_id).await;
    cdp(&mut ctx, 6, "Input.dispatchKeyEvent", json!({
        "type":"keyDown","text":"X"
    }), session_id).await;
    let retained = cdp(&mut ctx, 7, "Runtime.evaluate", json!({
        "expression":"JSON.stringify([i.value,i.selectionStart,i.selectionEnd,__keyMaxEvents])",
        "returnByValue":true
    }), session_id).await;
    assert_eq!(
        serde_json::from_str::<Value>(retained["result"]["value"].as_str().unwrap()).unwrap(),
        json!(["AC",1,1,[
            ["keydown",null,"ABC",1],
            ["keypress",null,"ABC",1],
            ["beforeinput","X","ABC",1],
            ["input","","AC",1]
        ]])
    );
}

#[tokio::test(flavor = "current_thread")]
async fn enter_obeys_textarea_maxlength_without_a_protocol_error() {
    std::env::set_var("OBSCURA_ALLOW_PRIVATE_NETWORK", "1");
    let url = serve_page().await;
    let mut ctx = CdpContext::new(obscura_net::EffectivePersona::builtin(
        obscura_net::StealthProfile::WindowsChrome145,
    ));
    let page_id = ctx.create_page();
    let session_id = "session-1";
    ctx.sessions.insert(session_id.to_string(), page_id);
    cdp(&mut ctx, 1, "Page.navigate", json!({"url":url,"waitUntil":"load"}), session_id).await;
    cdp(&mut ctx, 2, "Runtime.evaluate", json!({
        "expression":r#"(() => {
            a.maxLength=3;a.value='ABC';a.focus();a.setSelectionRange(3,3);
            globalThis.__lineEvents=[];
            for(const type of ['keydown','keypress','beforeinput','input']) a.addEventListener(type,e=>{
                __lineEvents.push([e.type,e.inputType,e.data,a.value,a.selectionStart]);
            });
        })()"#,
        "returnByValue":true
    }), session_id).await;
    let enter = json!({
        "type":"keyDown","key":"Enter","code":"Enter","text":"\r",
        "windowsVirtualKeyCode":13
    });
    cdp(&mut ctx, 3, "Input.dispatchKeyEvent", enter.clone(), session_id).await;
    let full = cdp(&mut ctx, 4, "Runtime.evaluate", json!({
        "expression":"JSON.stringify([a.value,a.selectionStart,__lineEvents])",
        "returnByValue":true
    }), session_id).await;
    assert_eq!(
        serde_json::from_str::<Value>(full["result"]["value"].as_str().unwrap()).unwrap(),
        json!(["ABC",3,[
            ["keydown",null,null,"ABC",3],
            ["keypress",null,null,"ABC",3],
            ["beforeinput","insertLineBreak",null,"ABC",3]
        ]])
    );

    cdp(&mut ctx, 5, "Runtime.evaluate", json!({
        "expression":"__lineEvents=[];a.value='AB';a.setSelectionRange(2,2)",
        "returnByValue":true
    }), session_id).await;
    cdp(&mut ctx, 6, "Input.dispatchKeyEvent", enter, session_id).await;
    let available = cdp(&mut ctx, 7, "Runtime.evaluate", json!({
        "expression":"JSON.stringify([a.value,a.selectionStart,__lineEvents])",
        "returnByValue":true
    }), session_id).await;
    assert_eq!(
        serde_json::from_str::<Value>(available["result"]["value"].as_str().unwrap()).unwrap(),
        json!(["AB\n",3,[
            ["keydown",null,null,"AB",2],
            ["keypress",null,null,"AB",2],
            ["beforeinput","insertLineBreak",null,"AB",2],
            ["input","insertLineBreak",null,"AB\n",3]
        ]])
    );
}

#[tokio::test(flavor = "current_thread")]
async fn insert_text_re_resolves_focus_after_beforeinput() {
    std::env::set_var("OBSCURA_ALLOW_PRIVATE_NETWORK", "1");
    let url = serve_page().await;
    let mut ctx = CdpContext::new(obscura_net::EffectivePersona::builtin(
        obscura_net::StealthProfile::WindowsChrome145,
    ));
    let page_id = ctx.create_page();
    let session_id = "session-1";
    ctx.sessions.insert(session_id.to_string(), page_id);
    cdp(
        &mut ctx,
        1,
        "Page.navigate",
        json!({"url": url, "waitUntil": "load"}),
        session_id,
    )
    .await;
    cdp(
        &mut ctx,
        2,
        "Runtime.evaluate",
        json!({
            "expression": r#"(() => {
                const first = document.getElementById('i');
                const second = document.getElementById('a');
                first.value = 'abcd';
                second.value = '';
                first.focus();
                first.setSelectionRange(1, 3);
                globalThis.__targets = [];
                document.addEventListener('beforeinput', event => {
                    __targets.push(event.target.id);
                    second.focus();
                    second.setSelectionRange(0, 0);
                });
                document.addEventListener('input', event => __targets.push(event.target.id));
            })()"#,
            "returnByValue": true,
        }),
        session_id,
    )
    .await;
    cdp(
        &mut ctx,
        3,
        "Input.insertText",
        json!({"text": "XY"}),
        session_id,
    )
    .await;
    let result = cdp(
        &mut ctx,
        4,
        "Runtime.evaluate",
        json!({
            "expression": "JSON.stringify([i.value,a.value,document.activeElement.id,__targets])",
            "returnByValue": true,
        }),
        session_id,
    )
    .await;
    assert_eq!(
        serde_json::from_str::<Value>(result["result"]["value"].as_str().unwrap()).unwrap(),
        json!(["abcd", "XY", "a", ["i", "a"]])
    );
}

#[tokio::test(flavor = "current_thread")]
async fn insert_text_processes_same_document_navigation_and_emits_frame_event() {
    std::env::set_var("OBSCURA_ALLOW_PRIVATE_NETWORK", "1");
    let url = serve_page().await;
    let mut ctx = CdpContext::new(obscura_net::EffectivePersona::builtin(
        obscura_net::StealthProfile::WindowsChrome145,
    ));
    let page_id = ctx.create_page();
    let session_id = "session-1";
    ctx.sessions.insert(session_id.to_string(), page_id.clone());
    cdp(
        &mut ctx,
        1,
        "Page.navigate",
        json!({"url": url, "waitUntil": "load"}),
        session_id,
    )
    .await;
    let loader_id = ctx.current_loader_ids[&page_id].clone();
    ctx.pending_events.clear();
    cdp(
        &mut ctx,
        2,
        "Runtime.evaluate",
        json!({
            "expression": r#"(() => {
                const target = document.getElementById('i');
                target.value = 'unchanged';
                target.focus();
                target.addEventListener('beforeinput', () => { location.hash = 'typed'; }, { once: true });
            })()"#,
            "returnByValue": true,
        }),
        session_id,
    )
    .await;
    cdp(
        &mut ctx,
        3,
        "Input.insertText",
        json!({"text": "X"}),
        session_id,
    )
    .await;

    let frame = ctx
        .pending_events
        .iter()
        .find(|event| event.method == "Page.frameNavigated")
        .expect("input-triggered same-document navigation must emit Page.frameNavigated");
    assert_eq!(frame.session_id.as_deref(), Some(session_id));
    assert_eq!(frame.params["frame"]["id"], page_id);
    assert_eq!(frame.params["frame"]["loaderId"], loader_id);
    assert!(
        frame.params["frame"]["url"]
            .as_str()
            .is_some_and(|url| url.ends_with("#typed")),
        "unexpected frame event: {frame:?}"
    );
    assert_eq!(ctx.current_loader_ids[&page_id], loader_id);
    assert!(ctx.pending_events.iter().all(|event| {
        event.method != "Runtime.executionContextsCleared"
            && event.method != "Page.lifecycleEvent"
    }));
}

#[tokio::test(flavor = "current_thread")]
async fn keyboard_and_text_reject_invalid_protocol_parameters_without_delivery() {
    let mut ctx = CdpContext::new(obscura_net::EffectivePersona::builtin(
        obscura_net::StealthProfile::WindowsChrome145,
    ));
    let page_id = ctx.create_page();
    let session_id = "session-1";
    ctx.sessions.insert(session_id.to_string(), page_id);

    for (id, method, params) in [
        (1, "Input.insertText", json!({})),
        (2, "Input.insertText", json!({"text": null})),
        (3, "Input.insertText", json!({"text": 3})),
        (4, "Input.dispatchKeyEvent", json!({})),
        (5, "Input.dispatchKeyEvent", json!({"type": "invalid"})),
        (6, "Input.dispatchKeyEvent", json!({"type": "keyDown", "modifiers": "bad"})),
        (7, "Input.dispatchKeyEvent", json!({"type": "keyDown", "commands": [1]})),
    ] {
        let response = dispatch(
            &CdpRequest {
                id,
                method: method.to_string(),
                params,
                session_id: Some(session_id.to_string()),
            },
            &mut ctx,
        )
        .await;
        assert_eq!(
            response.error.as_ref().map(|error| error.code),
            Some(-32602),
            "{method}: {:?}",
            response.error
        );
    }
}

#[cfg(not(feature = "render"))]
#[tokio::test(flavor = "current_thread")]
async fn keyboard_and_text_explicitly_require_the_render_input_runtime() {
    std::env::set_var("OBSCURA_ALLOW_PRIVATE_NETWORK", "1");
    let url = serve_page().await;
    let mut ctx = CdpContext::new(obscura_net::EffectivePersona::builtin(
        obscura_net::StealthProfile::WindowsChrome145,
    ));
    let page_id = ctx.create_page();
    let session_id = "session-1";
    ctx.sessions.insert(session_id.to_string(), page_id);
    cdp(
        &mut ctx,
        1,
        "Page.navigate",
        json!({"url": url, "waitUntil": "load"}),
        session_id,
    )
    .await;

    for (id, method, params) in [
        (2, "Input.insertText", json!({"text": "x"})),
        (
            3,
            "Input.dispatchKeyEvent",
            json!({"type": "keyDown", "key": "x", "code": "KeyX", "text": "x"}),
        ),
    ] {
        let response = dispatch(
            &CdpRequest {
                id,
                method: method.to_string(),
                params,
                session_id: Some(session_id.to_string()),
            },
            &mut ctx,
        )
        .await;
        let error = response
            .error
            .expect("no-render keyboard/text input must fail explicitly");
        assert_eq!(error.code, -32000, "{method}: {error:?}");
        assert!(error.message.contains("UNSUPPORTED"), "{method}: {error:?}");
    }
}
