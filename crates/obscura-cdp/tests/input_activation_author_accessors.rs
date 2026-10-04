use obscura_cdp::dispatch::{dispatch, CdpContext};
use obscura_cdp::types::CdpRequest;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

async fn serve_fixture(with_frame: bool) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
        let Ok((mut socket, _)) = listener.accept().await else { break; };
        let mut buf = [0u8; 2048];
        let count = socket.read(&mut buf).await.unwrap();
        let child = buf[..count].starts_with(b"GET /child ");
        let body = r#"<!doctype html><html><head><style>
          input { display: block; width: 40px; height: 40px; margin: 10px }
        </style></head><body>
          <input id="box" type="checkbox">
          <input id="first" type="radio" name="choice" checked>
          <input id="second" type="radio" name="choice">
          <script>
            window.writes = []; window.events = []; window.cancel = false;
            window.getters = []; window.strict = false; window.nativeReaders = {};
            window.installControl = function(el) {
              const id = el.id;
              if (id === 'box') el.indeterminate = true;
              for (const key of ['checked', 'indeterminate']) {
                let proto = Object.getPrototypeOf(el), descriptor;
                while (proto && !(descriptor = Object.getOwnPropertyDescriptor(proto, key))) {
                  proto = Object.getPrototypeOf(proto);
                }
                (nativeReaders[id] ||= {})[key] = () => descriptor.get.call(el);
                Object.defineProperty(el, key, {
                  configurable: true,
                  get() {
                    if (strict) { getters.push(id + ':' + key); throw new Error('author getter called'); }
                    return descriptor.get.call(this);
                  },
                  set(value) { writes.push(id + ':' + key); descriptor.set.call(this, value); }
                });
              }
              for (const kind of ['click', 'input', 'change']) {
                el.addEventListener(kind, e => {
                  events.push([id, kind, nativeReaders[id].checked(), nativeReaders[id].indeterminate()]);
                  if (kind === 'click' && cancel) e.preventDefault();
                });
              }
            };
            for (const id of ['box', 'first', 'second']) installControl(document.getElementById(id));
          </script>
        </body></html>"#;
        let body = if with_frame && !child {
            body.replace("          <script>", "          <iframe id=frame src=/child></iframe><script>")
        } else { body.to_string() };
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = socket.write_all(response.as_bytes()).await;
        }
    });
    format!("http://{addr}/")
}

async fn cdp(ctx: &mut CdpContext, id: u64, method: &str, params: Value, sid: &str) -> Value {
    let response = dispatch(
        &CdpRequest {
            id,
            method: method.to_string(),
            params,
            session_id: Some(sid.to_string()),
        },
        ctx,
    )
    .await;
    assert!(response.error.is_none(), "CDP {method} failed: {:?}", response.error);
    response.result.unwrap_or_else(|| json!({}))
}

async fn evaluate(ctx: &mut CdpContext, id: u64, expression: &str, sid: &str) -> Value {
    cdp(
        ctx,
        id,
        "Runtime.evaluate",
        json!({"expression": expression, "returnByValue": true, "awaitPromise": true}),
        sid,
    )
    .await
}

async fn setup() -> (CdpContext, String) { setup_fixture(false).await }

async fn setup_fixture(with_frame: bool) -> (CdpContext, String) {
    std::env::set_var("OBSCURA_ALLOW_PRIVATE_NETWORK", "1");
    let url = serve_fixture(with_frame).await;
    let mut ctx = CdpContext::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));
    let page_id = ctx.create_page();
    let sid = "author-accessors-session";
    ctx.sessions.insert(sid.to_string(), page_id);
    cdp(&mut ctx, 1, "Page.navigate", json!({"url": url, "waitUntil": "load"}), sid).await;
    (ctx, sid.to_string())
}

/// Click the centre of `selector` the way a real pointer would.
async fn click_element(ctx: &mut CdpContext, id: u64, sid: &str, selector: &str) {
    let rect = evaluate(
        ctx,
        id,
        &format!(
            "JSON.stringify(document.querySelector('{selector}').getBoundingClientRect().toJSON())"
        ),
        sid,
    )
    .await;
    let rect: Value = serde_json::from_str(rect["result"]["value"].as_str().unwrap()).unwrap();
    let x = rect["x"].as_f64().unwrap() + rect["width"].as_f64().unwrap() / 2.0;
    let y = rect["y"].as_f64().unwrap() + rect["height"].as_f64().unwrap() / 2.0;
    for kind in ["mousePressed", "mouseReleased"] {
        cdp(
            ctx,
            id + 1,
            "Input.dispatchMouseEvent",
            json!({"type": kind, "x": x, "y": y, "button": "left", "clickCount": 1}),
            sid,
        )
        .await;
    }
}

async fn snapshot(ctx: &mut CdpContext, sid: &str) -> Value {
    let value = evaluate(ctx, 90, r#"JSON.stringify({
        box: nativeReaders.box.checked(),
        indeterminate: nativeReaders.box.indeterminate(),
        first: nativeReaders.first.checked(),
        second: nativeReaders.second.checked(),
        writes, events, getters
    })"#, sid).await;
    serde_json::from_str(value["result"]["value"].as_str().unwrap()).unwrap()
}

async fn activate(ctx: &mut CdpContext, sid: &str, id: &str, pointer: bool) {
    if pointer {
        click_element(ctx, 20, sid, &format!("#{id}")).await;
    } else {
        evaluate(ctx, 20, &format!("document.getElementById('{id}').click()"), sid).await;
    }
}

async fn prepare_strict(ctx: &mut CdpContext, sid: &str, created: bool) {
    if created {
        evaluate(ctx, 6, r#"(() => {
            for (const id of ['box', 'first', 'second']) {
                const element = document.createElement('input');
                element.id = id;
                element.type = id === 'box' ? 'checkbox' : 'radio';
                if (id !== 'box') element.name = 'choice';
                if (id === 'first') element.checked = true;
                document.getElementById(id).replaceWith(element);
                installControl(element);
            }
        })()"#, sid).await;
    }
    evaluate(ctx, 7, "strict = true; writes = []; events = []; getters = []", sid).await;
}

async fn verify_activation(pointer: bool, strict: bool, created: bool) {
    let (mut ctx, sid) = setup().await;
    if (strict) { prepare_strict(&mut ctx, &sid, created).await; }
    activate(&mut ctx, &sid, "box", pointer).await;
    let s = snapshot(&mut ctx, &sid).await;
    assert_eq!(s["writes"], json!([]), "native activation must bypass author setters: {s}");
    assert_eq!(s["box"], true);
    assert_eq!(s["indeterminate"], false);
    assert_eq!(s["events"], json!([
        ["box", "click", true, false], ["box", "input", true, false], ["box", "change", true, false]
    ]));
    evaluate(&mut ctx, 30, "events = []", &sid).await;
    activate(&mut ctx, &sid, "second", pointer).await;
    let s = snapshot(&mut ctx, &sid).await;
    assert_eq!(s["writes"], json!([]), "radio peers must bypass author setters: {s}");
    assert_eq!(s["first"], false);
    assert_eq!(s["second"], true);
    assert_eq!(s["events"], json!([
        ["second", "click", true, false], ["second", "input", true, false], ["second", "change", true, false]
    ]));
    evaluate(&mut ctx, 40, "document.getElementById('box').checked = false", &sid).await;
    let s = snapshot(&mut ctx, &sid).await;
    assert_eq!(s["writes"], json!(["box:checked"]), "script assignment still invokes author setter");
    assert_eq!(s["box"], false);
    assert_eq!(s["getters"], json!([]), "activation does not call author getters");
}

async fn verify_cancellation(pointer: bool, strict: bool, created: bool) {
    let (mut ctx, sid) = setup().await;
    if (strict) { prepare_strict(&mut ctx, &sid, created).await; }
    evaluate(&mut ctx, 10, "cancel = true", &sid).await;
    activate(&mut ctx, &sid, "box", pointer).await;
    activate(&mut ctx, &sid, "second", pointer).await;
    let s = snapshot(&mut ctx, &sid).await;
    assert_eq!(s["writes"], json!([]), "activation and rollback bypass author setters: {s}");
    assert_eq!(s["box"], false);
    assert_eq!(s["indeterminate"], true);
    assert_eq!(s["first"], true);
    assert_eq!(s["second"], false);
    assert_eq!(s["events"], json!([
        ["box", "click", true, false], ["second", "click", true, false]
    ]), "canceled clicks expose pre-activation state but emit no input/change");
    assert_eq!(s["getters"], json!([]), "rollback does not call author getters");
}

#[cfg(feature = "render")]
#[tokio::test(flavor = "current_thread")]
async fn native_pointer_activation_bypasses_author_setters() { verify_activation(true, false, false).await; }
#[tokio::test(flavor = "current_thread")]
async fn element_click_activation_bypasses_author_setters() { verify_activation(false, false, false).await; }
#[cfg(feature = "render")]
#[tokio::test(flavor = "current_thread")]
async fn native_pointer_cancellation_restores_internal_state() { verify_cancellation(true, false, false).await; }
#[tokio::test(flavor = "current_thread")]
async fn element_click_cancellation_restores_internal_state() { verify_cancellation(false, false, false).await; }

#[tokio::test(flavor = "current_thread")]
async fn script_click_bypasses_throwing_author_getters() { verify_activation(false, true, false).await; }

#[tokio::test(flavor = "current_thread")]
async fn script_click_rollback_bypasses_throwing_author_getters() { verify_cancellation(false, true, false).await; }

#[cfg(feature = "render")]
#[tokio::test(flavor = "current_thread")]
async fn pointer_activation_bypasses_throwing_author_getters() { verify_activation(true, true, false).await; }

#[cfg(feature = "render")]
#[tokio::test(flavor = "current_thread")]
async fn pointer_rollback_bypasses_throwing_author_getters() { verify_cancellation(true, true, false).await; }

#[tokio::test(flavor = "current_thread")]
async fn created_control_click_bypasses_author_accessors() { verify_activation(false, true, true).await; }

#[tokio::test(flavor = "current_thread")]
async fn created_control_rollback_bypasses_author_accessors() { verify_cancellation(false, true, true).await; }


async fn verify_borrowed_activation(root_method: bool, cancel: bool) {
    let (mut ctx, sid) = setup_fixture(true).await;
    let expression = format!(r#"(() => {{
        const child = document.getElementById('frame').contentWindow;
        const rootBox = document.getElementById('box'), childBox = child.document.getElementById('box');
        strict = true; child.strict = true;
        cancel = {cancel}; child.cancel = {cancel};
        const source = {root_method} ? window : child;
        const receiver = {root_method} ? childBox : rootBox;
        source.HTMLElement.prototype.click.call(receiver);
        return JSON.stringify({{
            root: [nativeReaders.box.checked(), nativeReaders.box.indeterminate()],
            child: [child.nativeReaders.box.checked(), child.nativeReaders.box.indeterminate()],
            sameNativeId: rootBox._nid === childBox._nid,
            writes: [writes, child.writes], getters: [getters, child.getters],
            rootHidden: !Object.hasOwn(window, '__obscura_checked_receivers_handoff')
                && !Object.hasOwn(window, '__obscura_bind_checked_receivers_handoff'),
            childHidden: !Object.hasOwn(child, '__obscura_checked_receivers_handoff')
                && !Object.hasOwn(child, '__obscura_bind_checked_receivers_handoff')
        }});
    }})()"#);
    let result = evaluate(&mut ctx, 50, &expression, &sid).await;
    assert!(result.get("exceptionDetails").is_none(), "{result}");
    let actual: Value = serde_json::from_str(result["result"]["value"].as_str().unwrap()).unwrap();
    assert_eq!(actual["sameNativeId"], true, "fixture must collide actual NodeIds: {actual}");
    let resting = json!([false, true]);
    let activated = if cancel { resting.clone() } else { json!([true, false]) };
    assert_eq!(actual["root"], if root_method { resting.clone() } else { activated.clone() });
    assert_eq!(actual["child"], if root_method { activated } else { resting });
    assert_eq!(actual["writes"], json!([[], []]), "native activation bypasses authored setters: {actual}");
    assert_eq!(actual["getters"], json!([[], []]), "native activation bypasses authored getters: {actual}");
    assert_eq!(actual["rootHidden"], true);
    assert_eq!(actual["childHidden"], true);
}

#[tokio::test(flavor = "current_thread")]
async fn borrowed_root_click_preserves_child_document_with_colliding_node_ids() { verify_borrowed_activation(true, false).await; }
#[tokio::test(flavor = "current_thread")]
async fn borrowed_child_click_preserves_root_document_with_colliding_node_ids() { verify_borrowed_activation(false, false).await; }
#[tokio::test(flavor = "current_thread")]
async fn borrowed_root_click_rolls_back_child_without_touching_root() { verify_borrowed_activation(true, true).await; }
#[tokio::test(flavor = "current_thread")]
async fn borrowed_child_click_rolls_back_root_without_touching_child() { verify_borrowed_activation(false, true).await; }

#[tokio::test(flavor = "current_thread")]
async fn radio_rollback_does_not_use_author_array_iterator() {
    let (mut ctx, sid) = setup().await;
    prepare_strict(&mut ctx, &sid, false).await;
    let result = evaluate(&mut ctx, 50, r#"(() => {
        const iterator = Array.prototype[Symbol.iterator];
        document.getElementById('second').addEventListener('click', event => {
            event.preventDefault();
            event.stopPropagation();
            Array.prototype[Symbol.iterator] = function() { throw new Error('author iterator called'); };
        });
        try { document.getElementById('second').click(); }
        finally { Array.prototype[Symbol.iterator] = iterator; }
        return true;
    })()"#, &sid).await;
    assert!(result.get("exceptionDetails").is_none(), "{result}");
    let actual = snapshot(&mut ctx, &sid).await;
    assert_eq!(actual["first"], true);
    assert_eq!(actual["second"], false);
    assert_eq!(actual["getters"], json!([]));
    assert_eq!(actual["writes"], json!([]));
}
