#![cfg(feature = "render")]

use std::{collections::HashMap, sync::Arc};

use obscura_browser::{BrowserContext, Page};
use obscura_net::{
    interceptor::{InterceptAction, RequestInterceptor},
    RequestInfo, Response,
};
use serde_json::json;

struct HtmlFixture(&'static str);
#[async_trait::async_trait]
impl RequestInterceptor for HtmlFixture {
    async fn intercept(&self, request: &RequestInfo) -> InterceptAction {
        InterceptAction::Fulfill(Response {
            status: 200,
            url: request.url.clone(),
            headers: HashMap::from([("content-type".into(), "text/html".into())]),
            body: self.0.as_bytes().to_vec(),
            redirected_from: vec![],
            raw_headers: None,
            request_raw_headers: None,
            request_referrer: None,
        })
    }
}

async fn input_fixture(html: &'static str) -> Page {
    let mut context = BrowserContext::with_storage_and_network(
        "native-input-test".into(), obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145),
        None,
        None,
        true,
    );
    let client = Arc::get_mut(&mut context.http_client).unwrap();
    client.block_trackers = false;
    *client.interceptor.write().await = Some(std::sync::Arc::new(HtmlFixture(html)));
    let mut page = Page::new("native-test".into(), Arc::new(context));
    page.set_viewport((640.0, 480.0));
    page.navigate("http://127.0.0.1/native-input-fixture")
        .await
        .unwrap();
    page
}

#[tokio::test(flavor = "current_thread")]
async fn native_hit_uses_paint_order_and_transparent_overlays() {
    let mut page = input_fixture(
        r#"<!doctype html><style>
        body{margin:0}button,div{position:absolute;left:20px;top:20px;width:100px;height:60px}
        #top{z-index:20;background:red}#bottom{z-index:1;background:blue}
        </style><button id="top">TOP</button><button id="bottom">BOTTOM</button>"#,
    )
    .await;
    // A page can replace its public geometry methods; native lookup must not call them.
    page.evaluate("document.elementFromPoint=()=>{throw Error('page geometry was called')}");
    let target = page.js.as_ref().unwrap().input_target("#top").unwrap();
    assert_eq!(
        page.js
            .as_ref()
            .unwrap()
            .hit_test(target.x, target.y)
            .unwrap(),
        Some(target.node)
    );
    assert_eq!(
        page.js
            .as_ref()
            .unwrap()
            .input_target("#bottom")
            .unwrap_err(),
        "ELEMENT_OCCLUDED"
    );
    page.evaluate("document.getElementById('top').style.opacity='0'");
    assert_eq!(
        page.js
            .as_ref()
            .unwrap()
            .input_target("#bottom")
            .unwrap_err(),
        "ELEMENT_OCCLUDED"
    );
    assert_eq!(
        page.js.as_ref().unwrap().input_target("#top").unwrap_err(),
        "ELEMENT_NOT_VISIBLE"
    );
    page.evaluate("document.getElementById('top').style.pointerEvents='none'");
    assert!(page.js.as_ref().unwrap().input_target("#bottom").is_ok());
}

#[tokio::test(flavor = "current_thread")]
async fn native_hit_keeps_atomic_layers_and_pointer_inheritance() {
    let mut page = input_fixture(
        r#"<!doctype html><style>
        body{margin:0}.box{position:absolute;left:0;top:0;width:100px;height:60px}
        #group{opacity:.5}#inside{z-index:999}#cover{z-index:1}
        </style><div id="group" class="box"><button id="inside" class="box">INNER</button></div>
        <div id="cover" class="box"><span id="leaf" class="box">COVER</span></div>"#,
    )
    .await;
    assert_eq!(
        page.js
            .as_ref()
            .unwrap()
            .input_target("#inside")
            .unwrap_err(),
        "ELEMENT_OCCLUDED"
    );
    page.evaluate("document.getElementById('cover').style.pointerEvents='none'");
    assert!(page.js.as_ref().unwrap().input_target("#inside").is_ok());
    page.evaluate("document.getElementById('leaf').style.pointerEvents='auto'");
    assert_eq!(
        page.js
            .as_ref()
            .unwrap()
            .input_target("#inside")
            .unwrap_err(),
        "ELEMENT_OCCLUDED"
    );
    page.evaluate("document.getElementById('cover').style.visibility='hidden'");
    assert!(page.js.as_ref().unwrap().input_target("#inside").is_ok());
}

#[tokio::test(flavor = "current_thread")]
async fn native_hit_css_inheritance_disabled_fieldsets_and_nested_scroll() {
    let mut page = input_fixture(r#"<!doctype html><style>
        body{margin:0}fieldset{margin:0;padding:0;border:0;position:absolute;left:0;top:0}
        legend{height:40px}button{width:100px;height:30px}
        #outer{position:absolute;left:200px;top:0;width:100px;height:100px;overflow:hidden}
        #scroller{width:100px;height:160px;overflow:auto}
        #scrolltarget{position:relative;top:150px;width:100px;height:30px}
        #cover{position:absolute;left:400px;top:0;width:100px;height:100px;pointer-events:none}
        #leaf{width:100px;height:100px;pointer-events:inherit}
        #under{position:absolute;left:400px;top:0;width:100px;height:100px}
        </style><fieldset disabled><legend><button id="legend">OK</button></legend>
        <button id="disabled">DISABLED</button><div id="plain" style="width:100px;height:30px">PLAIN</div>
        <legend><button id="second">DISABLED</button></legend></fieldset>
        <div id="outer"><div id="scroller"><button id="scrolltarget">SCROLL</button><div style="height:400px"></div></div></div>
        <button id="under">UNDER</button><div id="cover"><div id="leaf"></div></div>"#).await;
    for selector in ["#legend", "#plain", "#under"] {
        assert!(
            page.js.as_ref().unwrap().input_target(selector).is_ok(),
            "{selector}"
        );
    }
    for selector in ["#disabled", "#second"] {
        assert_eq!(
            page.js
                .as_ref()
                .unwrap()
                .input_target(selector)
                .unwrap_err(),
            "ELEMENT_DISABLED"
        );
    }
    assert_eq!(
        page.js
            .as_ref()
            .unwrap()
            .input_target("#scrolltarget")
            .unwrap_err(),
        "ELEMENT_NOT_VISIBLE"
    );
    page.evaluate("document.getElementById('scroller').scrollTop=150");
    let target = page
        .js
        .as_ref()
        .unwrap()
        .input_target("#scrolltarget")
        .unwrap();
    assert!(target.y < 100.0);
    for value in ["initial", "unset", "auto", "none", "inherit"] {
        page.evaluate(&format!(
            "document.getElementById('leaf').style.pointerEvents='{value}'"
        ));
        let allowed = page.js.as_ref().unwrap().input_target("#under").is_ok();
        assert_eq!(
            allowed,
            matches!(value, "unset" | "none" | "inherit"),
            "{value}"
        );
    }
    page.evaluate("document.getElementById('under').setAttribute('inert','')");
    assert_eq!(
        page.js
            .as_ref()
            .unwrap()
            .input_target("#under")
            .unwrap_err(),
        "ELEMENT_DISABLED"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn native_hit_ignores_empty_auto_sized_positioned_pseudo() {
    let page = input_fixture(r#"<!doctype html><style>
        body { margin:0 }
        .decoration::before { content:''; position:absolute; left:0; top:0 }
        button { position:fixed; left:20px; top:20px; width:100px; height:40px }
        </style><div class="decoration"></div><button id="target">Agree</button>"#).await;
    let target = page.js.as_ref().unwrap().input_target("#target").unwrap();
    assert_eq!(target.node, target.hit_node);
}

#[tokio::test(flavor = "current_thread")]
async fn native_hit_ignores_pseudos_below_display_none_ancestor() {
    let page = input_fixture(r#"<!doctype html><style>
        body { margin:0 }
        #hidden { display:none }
        .decoration::before { content:''; position:absolute; inset:0; background:red }
        button { position:fixed; left:20px; top:20px; width:100px; height:40px }
        </style><div id="hidden"><div class="decoration"></div></div>
        <button id="target">Agree</button>"#).await;
    let target = page.js.as_ref().unwrap().input_target("#target").unwrap();
    assert_eq!(target.node, target.hit_node);
}

#[tokio::test(flavor = "current_thread")]
async fn native_hit_translation_and_unsupported_paint_are_explicit() {
    let page=input_fixture(r#"<!doctype html><style>
        body{margin:0}.box{position:absolute;left:0;top:0;width:80px;height:80px}
        #translated{transform:translate(120px,20px)}
        #clipped{left:220px;clip-path:polygon(0 0,100% 0,50% 100%)}
        #pseudo{left:320px}#pseudo::before{content:'';position:absolute;left:0;top:0;width:80px;height:80px;background:red}
        svg{position:absolute;left:420px;top:0}
        </style><button id="translated" class="box">OK</button><button id="clipped" class="box">CLIP</button>
        <button id="pseudo" class="box">PSEUDO</button><svg id="svg" width="80" height="80"><rect width="80" height="80"/></svg>"#).await;
    let target = page
        .js
        .as_ref()
        .unwrap()
        .input_target("#translated")
        .unwrap();
    assert_eq!((target.x, target.y), (160.0, 60.0));
    assert_eq!(target.node, target.hit_node);
    for selector in ["#clipped", "#pseudo", "#svg"] {
        assert_eq!(
            page.js
                .as_ref()
                .unwrap()
                .input_target(selector)
                .unwrap_err(),
            "INPUT_GEOMETRY_UNSUPPORTED",
            "{selector}"
        );
    }
    for (x, y) in [(f32::NAN, 1.0), (-1.0, 1.0), (640.0, 1.0), (1.0, 480.0)] {
        assert_eq!(
            page.js.as_ref().unwrap().hit_test(x, y).unwrap_err(),
            "INPUT_POINT_OUTSIDE_VIEWPORT"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn native_mouse_uses_private_entry_and_native_event_path() {
    let mut page = input_fixture(
        r#"<!doctype html><style>body{margin:0}button{width:100px;height:80px}</style>
        <div id="outer"><button id="target">INPUT</button></div>"#,
    )
    .await;
    assert_eq!(
        page.evaluate("typeof __obscura_native_mouse_handoff"),
        json!("undefined")
    );
    page.js.as_mut().unwrap().execute_script("<native-input-fixture>", r#"window.log=[];window.events=[];const originalMouse=MouseEvent;
        const outer=document.getElementById('outer'), target=document.getElementById('target');
        for(const [node,label] of [[window,'window'],[document,'document'],[outer,'outer'],[target,'target']]) {
            node.addEventListener('pointerdown',e=>log.push(label+':capture:'+e.eventPhase),true);
            node.addEventListener('pointerdown',e=>log.push(label+':bubble:'+e.eventPhase));
        }
        for(const name of ['pointermove','mousemove','pointerdown','mousedown','pointerup','mouseup']) {
            target.addEventListener(name,e=>events.push([e.type,e.isTrusted,e.clientX,e.clientY,e.buttons,e.button,e.pointerType||'',e instanceof originalMouse]));
        }
        Element.prototype.dispatchEvent=()=>{throw Error('page dispatch called')};
        Element.prototype.getBoundingClientRect=()=>{throw Error('page geometry called')};
        document.elementFromPoint=()=>{throw Error('page hit called')};
        window.__obscura_markTrusted=()=>{throw Error('public trust called')};
        window.__obscura_native_mouse_handoff=()=>{throw Error('public handoff called')};
        window.MouseEvent=()=>{throw Error('page constructor called')};
        window.PointerEvent=()=>{throw Error('page constructor called')};"#).unwrap();
    let js = page.js.as_mut().unwrap();
    assert!(js.native_mouse_move(50.0, 40.0).unwrap());
    assert!(js.native_mouse_down(50.0, 40.0).unwrap());
    assert_eq!(
        js.native_mouse_down(50.0, 40.0).unwrap_err(),
        "INPUT_BUTTON_SEQUENCE"
    );
    assert!(js.native_mouse_up(50.0, 40.0).unwrap());
    assert_eq!(
        js.native_mouse_up(50.0, 40.0).unwrap_err(),
        "INPUT_BUTTON_SEQUENCE"
    );
    assert_eq!(
        page.evaluate("JSON.stringify(log)"),
        json!(serde_json::to_string(&json!([
            "window:capture:1",
            "document:capture:1",
            "outer:capture:1",
            "target:capture:2",
            "target:bubble:2",
            "outer:bubble:3",
            "document:bubble:3",
            "window:bubble:3"
        ]))
        .unwrap())
    );
    assert_eq!(
        page.evaluate("JSON.stringify(events)"),
        json!(serde_json::to_string(&json!([
            ["pointermove", true, 50, 40, 0, -1, "mouse", true],
            ["mousemove", true, 50, 40, 0, 0, "", true],
            ["pointerdown", true, 50, 40, 1, 0, "mouse", true],
            ["mousedown", true, 50, 40, 1, 0, "", true],
            ["pointerup", true, 50, 40, 0, 0, "mouse", true],
            ["mouseup", true, 50, 40, 0, 0, "", true]
        ]))
        .unwrap())
    );
}

#[tokio::test(flavor = "current_thread")]
async fn native_mouse_cancellation_once_passive_and_listener_removal() {
    let mut page = input_fixture(
        r#"<!doctype html><style>body{margin:0}button{width:100px;height:80px}</style>
        <button id="target">INPUT</button>"#,
    )
    .await;
    page.js.as_mut().unwrap().execute_script("<native-input-fixture>", r#"window.log=[];const target=document.getElementById('target');
        target.addEventListener('pointerdown',e=>{log.push('cancel');e.preventDefault();e.stopImmediatePropagation()},{once:true});
        target.addEventListener('pointerdown',e=>{log.push('passive');e.preventDefault()},{passive:true});
        const removed=()=>log.push('removed');target.addEventListener('pointerdown',removed);target.removeEventListener('pointerdown',removed);
        target.addEventListener('pointerdown',{handleEvent(e){log.push('object')}});
        for(const name of ['mousedown','mouseup','pointerup']) target.addEventListener(name,()=>log.push(name));
        document.addEventListener('pointerdown',()=>log.push('document'));"#).unwrap();
    let js = page.js.as_mut().unwrap();
    assert!(!js.native_mouse_down(50.0, 40.0).unwrap());
    assert!(!js.native_mouse_up(50.0, 40.0).unwrap());
    assert!(js.native_mouse_down(50.0, 40.0).unwrap());
    assert!(js.native_mouse_up(50.0, 40.0).unwrap());
    assert_eq!(
        page.evaluate("JSON.stringify(log)"),
        json!(serde_json::to_string(&json!([
            "cancel",
            "pointerup",
            "passive",
            "object",
            "document",
            "mousedown",
            "pointerup",
            "mouseup"
        ]))
        .unwrap())
    );
}

#[tokio::test(flavor = "current_thread")]
async fn native_mouse_handoff_is_absent_in_frames_and_legacy_events_still_work() {
    let mut page = input_fixture(r#"<!doctype html><button id="target">INPUT</button>"#).await;
    {
        let js = page.js.as_mut().unwrap();
        let frame = obscura_js::frame::FrameRealm::new(
            js,
            1,
            0,
            "http://127.0.0.1/child",
            "<p>CHILD</p>",
        )
        .unwrap();
        assert_eq!(
            frame.evaluate(js, "document.body.textContent").unwrap(),
            json!("CHILD")
        );
        assert_eq!(
            frame
                .evaluate(js, "typeof __obscura_native_mouse_handoff")
                .unwrap(),
            json!("undefined")
        );
        assert_eq!(
            frame.evaluate(js, "typeof __obscura_native_fragment_handoff").unwrap(),
            json!("undefined")
        );
        assert_eq!(
            frame
                .evaluate(js, "typeof __obscura_native_focus_handoff")
                .unwrap(),
            json!("undefined")
        );
        assert_eq!(
            frame
                .evaluate(js, "typeof __obscura_native_text_handoff")
                .unwrap(),
            json!("undefined")
        );
    }
    page.js
        .as_mut()
        .unwrap()
        .execute_script(
            "<legacy-event-fixture>",
            r#"
        window.log=[];const target=document.getElementById('target');
        target.addEventListener('click',()=>log.push('target'),{once:true});
        document.addEventListener('click',()=>log.push('document'));
        target.click(); target.click();
        window.addEventListener('resize',()=>log.push('window'),{once:true});
        window.dispatchEvent(new Event('resize'));window.dispatchEvent(new Event('resize'));
    "#,
        )
        .unwrap();
    assert_eq!(
        page.evaluate("JSON.stringify(log)"),
        json!("[\"target\",\"document\",\"document\",\"window\"]")
    );
}

fn pixel(page: &Page, x: usize, y: usize) -> [u8; 4] {
    let png = page.screenshot((640.0, 480.0)).unwrap();
    let image =
        image::load_from_memory_with_format(&png, image::ImageFormat::Png).unwrap();
    assert_eq!(image.color(), image::ColorType::Rgba8);
    image.to_rgba8().get_pixel(x as u32, y as u32).0
}

#[tokio::test(flavor = "current_thread")]
async fn native_pointer_state_changes_selectors_and_pixels() {
    let mut page = input_fixture(r#"<!doctype html><style>
        body{margin:0}#outer{position:absolute;width:200px;height:100px;background:white}
        #target{position:absolute;left:20px;top:20px;width:100px;height:60px;border:0;padding:0;background:red}
        #outer:hover{background:yellow}#target:hover{background:green}#target:active{background:blue}
        </style><div id="outer"><button id="target"></button></div>"#).await;
    assert_eq!(pixel(&page, 50, 40), [255, 0, 0, 255]);
    assert!(page
        .js
        .as_mut()
        .unwrap()
        .native_mouse_move(50.0, 40.0)
        .unwrap());
    assert_eq!(
        page.evaluate("document.querySelector('#outer:hover')?.id"),
        json!("outer")
    );
    assert_eq!(pixel(&page, 50, 40), [0, 128, 0, 255]);
    assert_eq!(pixel(&page, 5, 5), [255, 255, 0, 255]);
    assert!(page
        .js
        .as_mut()
        .unwrap()
        .native_mouse_down(50.0, 40.0)
        .unwrap());
    assert_eq!(
        page.evaluate("document.querySelector('#outer:active')?.id"),
        json!("outer")
    );
    assert_eq!(pixel(&page, 50, 40), [0, 0, 255, 255]);
    assert!(page
        .js
        .as_mut()
        .unwrap()
        .native_mouse_up(50.0, 40.0)
        .unwrap());
    assert_eq!(
        page.evaluate("document.querySelectorAll(':active').length===0"),
        json!(true)
    );
    assert_eq!(pixel(&page, 50, 40), [0, 128, 0, 255]);
    page.js.as_mut().unwrap().execute_script("<remove-hover>",
        "const node=document.getElementById('target');node.remove();document.getElementById('outer').appendChild(node);").unwrap();
    assert_eq!(
        page.evaluate("document.querySelectorAll(':hover').length===0"),
        json!(true)
    );
    assert_eq!(pixel(&page, 50, 40), [255, 0, 0, 255]);
}

#[tokio::test(flavor = "current_thread")]
async fn native_hit_positioned_auto_and_negative_layers_match_pixels() {
    let mut page = input_fixture(
        r#"<!doctype html><style>
        body{margin:0}.box{width:100px;height:100px}
        #auto{position:absolute;left:0;top:0;background:red}
        #normal{background:blue}
        #context{position:absolute;left:120px;top:0;z-index:0;background:blue}
        #negative{position:absolute;left:0;top:0;z-index:-1;background:red}
        #escape{position:absolute;left:240px;top:0;z-index:10;background:red}
        #zero{position:absolute;left:240px;top:0;z-index:0;background:blue}
        </style><div id="auto" class="box"><div id="escape" class="box"></div></div>
        <div id="normal" class="box"></div><div id="context" class="box">
        <div id="negative" class="box"></div></div><div id="zero" class="box"></div>"#,
    )
    .await;
    for (selector, x) in [("#auto", 50), ("#negative", 170), ("#escape", 290)] {
        assert!(
            page.js.as_ref().unwrap().input_target(selector).is_ok(),
            "{selector}"
        );
        assert_eq!(pixel(&page, x, 50), [255, 0, 0, 255], "{selector}");
    }
    // A normal child moves with its positioned auto ancestor, while the
    // child's explicit z-index still belongs to the outer stacking context.
    page.evaluate("document.getElementById('auto').innerHTML='<div id=child style=\"width:100px;height:100px;background:green\"></div>'");
    assert!(page.js.as_ref().unwrap().input_target("#child").is_ok());
    assert_eq!(pixel(&page, 50, 50), [0, 128, 0, 255]);
}

#[tokio::test(flavor = "current_thread")]
async fn native_hit_clips_scrolled_boxes_and_refuses_unsupported_geometry() {
    let mut page = input_fixture(r#"<!doctype html><style>
        body{margin:0}.box{position:absolute;left:0;top:0;width:100px;height:100px}
        #clip{overflow:hidden;border-radius:50px}#inner{border:0;padding:0}
        #disabled{left:120px}#hidden{left:240px;visibility:hidden}
        #rotate{left:360px;transform:rotate(10deg)}
        </style><div id="clip" class="box"><button id="inner" class="box">INNER</button></div>
        <button id="disabled" class="box" disabled>DISABLED</button>
        <button id="hidden" class="box">HIDDEN</button><button id="rotate" class="box">ROTATED</button>
        <div style="height:1000px"></div>"#).await;
    let target = page.js.as_ref().unwrap().input_target("#inner").unwrap();
    assert_eq!(
        page.js.as_ref().unwrap().hit_test(50.0, 50.0).unwrap(),
        Some(target.node)
    );
    assert_ne!(
        page.js.as_ref().unwrap().hit_test(1.0, 1.0).unwrap(),
        Some(target.node)
    );
    assert_eq!(
        page.js
            .as_ref()
            .unwrap()
            .input_target("#disabled")
            .unwrap_err(),
        "ELEMENT_DISABLED"
    );
    assert_eq!(
        page.js
            .as_ref()
            .unwrap()
            .input_target("#hidden")
            .unwrap_err(),
        "ELEMENT_NOT_VISIBLE"
    );
    assert_eq!(
        page.js
            .as_ref()
            .unwrap()
            .input_target("#rotate")
            .unwrap_err(),
        "INPUT_GEOMETRY_UNSUPPORTED"
    );
    assert_eq!(
        page.js
            .as_ref()
            .unwrap()
            .input_target("button")
            .unwrap_err(),
        "ELEMENT_AMBIGUOUS"
    );
    page.evaluate("window.scrollTo(0,200)");
    assert_eq!(
        page.js
            .as_ref()
            .unwrap()
            .input_target("#inner")
            .unwrap_err(),
        "ELEMENT_NOT_VISIBLE"
    );
}
#[tokio::test]
async fn inline_link_after_button_has_native_input_geometry() {
    let page=input_fixture("<!doctype html><body><main><button>我已确认购票、搭乘相关的注意事项。</button><a href='#'>下一步</a></main>").await;
    let js=page.js.as_ref().unwrap();
    let node=js.input_node("a").unwrap();
    assert!(js.automation_box(node).is_some(), "native target: {:?}", js.input_target("a"));
}
