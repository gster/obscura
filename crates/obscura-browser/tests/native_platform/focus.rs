use super::support::*;

#[tokio::test(flavor = "current_thread")]
async fn native_focus_has_shared_state_event_order_and_private_entry() {
    let mut page = input_fixture(r#"<!doctype html><style>
        body{margin:0}#outer{width:240px;height:100px;background:white}
        button{position:absolute;top:20px;width:100px;height:60px;border:0;padding:0;background:red}
        #a{left:10px}#b{left:120px}button:focus{background:blue}#outer:focus-within{background:yellow}
        </style><body id="body"><div id="outer"><button id="a"></button><button id="b"></button></div></body>"#).await;
    assert_eq!(
        page.evaluate("typeof __obscura_native_focus_handoff"),
        json!("undefined")
    );
    page.js.as_mut().unwrap().execute_script("<focus-events>", r#"
        window.log=[];window.bubbles=[];const a=document.getElementById('a'),b=document.getElementById('b');
        for (const type of ['blur','focusout','focus','focusin']) {
            document.addEventListener(type,e=>bubbles.push(e.type));
            for(const node of [a,b]) node.addEventListener(type,e=>{
                e.preventDefault();
                log.push([e.type,e.target.id,document.activeElement.id,e.relatedTarget?.id,e.isTrusted,e.bubbles,e.cancelable,e.defaultPrevented]);
            });
        }
        a.focus();log.length=0;bubbles.length=0;
        window.__obscura_focused=b;
        Element.prototype.focus=()=>{throw Error('page focus called')};
        window.FocusEvent=()=>{throw Error('page constructor called')};
        window.__obscura_native_focus_handoff=()=>{throw Error('page handoff called')};
        b.onfocus=()=>false;
    "#).unwrap();
    assert_eq!(page.evaluate("document.activeElement.id"), json!("a"));
    assert!(page.js.as_mut().unwrap().native_focus("#b").unwrap());
    assert_eq!(
        page.evaluate("JSON.stringify(log)"),
        json!(serde_json::to_string(&json!([
            ["blur", "a", "body", "b", true, false, false, false],
            ["focusout", "a", "body", "b", true, true, false, false],
            ["focus", "b", "b", "a", true, false, false, false],
            ["focusin", "b", "b", "a", true, true, false, false]
        ]))
        .unwrap())
    );
    assert_eq!(
        page.evaluate("bubbles.join(',')"),
        json!("focusout,focusin")
    );
    assert_eq!(
        page.evaluate("document.querySelector(':focus').id"),
        json!("b")
    );
    assert_eq!(
        page.evaluate("document.querySelector('#outer:focus')===null"),
        json!(true)
    );
    assert_eq!(
        page.evaluate("document.querySelector('#outer:focus-within').id"),
        json!("outer")
    );
    assert_eq!(pixel(&page, 50, 40), [255, 0, 0, 255]);
    assert_eq!(pixel(&page, 160, 40), [0, 0, 255, 255]);
    assert_eq!(pixel(&page, 5, 5), [255, 255, 0, 255]);
}

#[tokio::test(flavor = "current_thread")]
async fn native_focus_dom_bridge_ignores_replaced_public_converters() {
    let mut page = input_fixture(
        r#"<!doctype html><style>button{width:100px;height:60px}</style>
        <button id="a">A</button><button id="b">B</button>"#,
    )
    .await;
    assert!(page.js.as_mut().unwrap().native_focus("#a").unwrap());
    page.js.as_mut().unwrap().execute_script("<replace-converters>",r#"
        window.originalString=String;window.originalParse=JSON.parse;window.originalSetHas=Set.prototype.has;
        window.String=()=> '9999';JSON.parse=()=> [true,9999,9999];Set.prototype.has=()=>false;
    "#).unwrap();
    assert!(page.js.as_mut().unwrap().native_focus("#b").unwrap());
    assert_eq!(page.evaluate("document.activeElement.id"), json!("b"));
    page.js.as_mut().unwrap().execute_script("<restore-converters>",
        "window.String=originalString;JSON.parse=originalParse;Set.prototype.has=originalSetHas;").unwrap();
    assert_eq!(
        page.evaluate("document.querySelector(':focus').id"),
        json!("b")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn native_mouse_default_focus_obeys_cancellation_and_modality() {
    let mut page=input_fixture(r#"<!doctype html><style>body{margin:0}
        input,button{position:absolute;top:20px;width:100px;height:60px;box-sizing:border-box}
        input{left:10px}button{left:120px}</style><input id="text"><button id="button">B</button>"#).await;
    assert!(page.js.as_mut().unwrap().native_focus("#text").unwrap());
    page.js.as_mut().unwrap().execute_script("<cancel-focus>",
        "document.getElementById('button').addEventListener('mousedown',e=>e.preventDefault(),{once:true});").unwrap();
    assert!(!page
        .js
        .as_mut()
        .unwrap()
        .native_mouse_down(150.0, 40.0)
        .unwrap());
    assert_eq!(page.evaluate("document.activeElement.id"), json!("text"));
    assert!(page
        .js
        .as_mut()
        .unwrap()
        .native_mouse_up(150.0, 40.0)
        .unwrap());
    assert!(page
        .js
        .as_mut()
        .unwrap()
        .native_mouse_down(150.0, 40.0)
        .unwrap());
    assert_eq!(page.evaluate("document.activeElement.id"), json!("button"));
    assert_eq!(
        page.evaluate("document.querySelector(':focus-visible')===null"),
        json!(true)
    );
    assert!(page
        .js
        .as_mut()
        .unwrap()
        .native_mouse_up(150.0, 40.0)
        .unwrap());
    assert!(page
        .js
        .as_mut()
        .unwrap()
        .native_mouse_down(50.0, 40.0)
        .unwrap());
    assert_eq!(
        page.evaluate("document.querySelector(':focus-visible').id"),
        json!("text")
    );
    assert!(page
        .js
        .as_mut()
        .unwrap()
        .native_mouse_up(50.0, 40.0)
        .unwrap());
}

#[tokio::test(flavor = "current_thread")]
async fn native_focus_reentrant_callbacks_keep_the_newer_focus() {
    let mut page = input_fixture(
        r#"<!doctype html><style>button{width:100px;height:60px}</style>
        <button id="a">A</button><button id="b">B</button><button id="c">C</button>"#,
    )
    .await;
    page.js.as_mut().unwrap().execute_script("<reentrant-blur>",r#"
        const a=document.getElementById('a'),b=document.getElementById('b'),c=document.getElementById('c');
        window.log=[];for(const node of [a,b,c]) node.addEventListener('focus',()=>log.push(node.id));
        a.focus();log.length=0;a.addEventListener('blur',()=>c.focus(),{once:true});
    "#).unwrap();
    assert!(!page.js.as_mut().unwrap().native_focus("#b").unwrap());
    assert_eq!(page.evaluate("document.activeElement.id"), json!("c"));
    assert_eq!(page.evaluate("log.join(',')"), json!("c"));
    page.js
        .as_mut()
        .unwrap()
        .execute_script(
            "<reentrant-focus>",
            "log.length=0;b.addEventListener('focus',()=>a.focus(),{once:true});",
        )
        .unwrap();
    assert!(!page.js.as_mut().unwrap().native_focus("#b").unwrap());
    assert_eq!(page.evaluate("document.activeElement.id"), json!("a"));
    assert_eq!(page.evaluate("log.join(',')"), json!("b,a"));
}

#[tokio::test(flavor = "current_thread")]
async fn native_focus_rejects_unavailable_nodes_and_clears_detached_state() {
    let mut page=input_fixture(r#"<!doctype html><style>
        body{margin:0}button{width:60px;height:30px}#hidden{display:none}
        #invisible{visibility:hidden}#inherited{visibility:inherit}#transparent{opacity:0}
        </style><body id="body"><button id="a"></button><button id="disabled" disabled>D</button>
        <button id="hidden">H</button><div id="invisible"><button id="inherited">V</button></div>
        <div inert><button id="inert">I</button></div><button id="transparent">T</button>
        <fieldset disabled><legend><button id="legend">L</button></legend><button id="fieldset">F</button></fieldset></body>"#).await;
    assert!(page.js.as_mut().unwrap().native_focus("#a").unwrap());
    for id in ["disabled", "hidden", "inherited", "inert", "fieldset"] {
        page.evaluate(&format!("document.getElementById('{id}').focus()"));
        assert_eq!(
            page.evaluate("document.activeElement.id"),
            json!("a"),
            "{id}"
        );
    }
    assert_eq!(
        page.js
            .as_mut()
            .unwrap()
            .native_focus("#disabled")
            .unwrap_err(),
        "ELEMENT_DISABLED"
    );
    assert_eq!(
        page.js
            .as_mut()
            .unwrap()
            .native_focus("#transparent")
            .unwrap_err(),
        "ELEMENT_NOT_VISIBLE"
    );
    assert_eq!(
        page.evaluate("document.querySelector('#fieldset:disabled').id"),
        json!("fieldset")
    );
    assert_eq!(
        page.evaluate("document.querySelector('#legend:enabled').id"),
        json!("legend")
    );
    page.evaluate("document.getElementById('transparent').focus()");
    assert_eq!(
        page.evaluate("document.activeElement.id"),
        json!("transparent")
    );
    page.evaluate("document.getElementById('legend').focus()");
    assert_eq!(page.evaluate("document.activeElement.id"), json!("legend"));
    page.js
        .as_mut()
        .unwrap()
        .execute_script(
            "<detached-focus>",
            r#"
        const old=document.getElementById('legend');old.remove();document.body.appendChild(old);
    "#,
        )
        .unwrap();
    assert_eq!(page.evaluate("document.activeElement.id"), json!("body"));
    assert_eq!(
        page.evaluate("document.querySelectorAll(':focus').length===0"),
        json!(true)
    );
    // Native arena removal must not let a newly allocated node inherit focus.
    assert!(page.js.as_mut().unwrap().native_focus("#a").unwrap());
    page.js.as_ref().unwrap().with_dom(|dom| {
        let id = dom.query_selector_all("#a").unwrap()[0];
        let data = dom.get_node(id).unwrap().data;
        dom.remove(id);
        let replacement = dom.new_node(data);
        assert_eq!(replacement, id);
        assert_eq!(dom.input_state().focused, None);
    });
    assert_eq!(
        page.evaluate("document.querySelectorAll(':focus').length===0"),
        json!(true)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn native_hit_skips_inert_overlays() {
    let page = input_fixture(
        r#"<!doctype html><style>body{margin:0}
        button,div{position:absolute;left:0;top:0;width:100px;height:60px}
        #overlay{z-index:10;background:red}</style>
        <button id="target">T</button><div id="overlay" inert><span>INERT</span></div>"#,
    )
    .await;
    assert!(page.js.as_ref().unwrap().input_target("#target").is_ok());
    assert_eq!(
        page.js
            .as_ref()
            .unwrap()
            .input_target("#overlay")
            .unwrap_err(),
        "ELEMENT_DISABLED"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn native_focus_fixup_waits_for_execution_opportunity_and_keeps_reads_passive() {
    for (script, changes) in [
        ("field.style.display='none'", 1),
        ("field.style.visibility='hidden'", 1),
        ("field.hidden=true", 1),
        ("field.disabled=true", 1),
        ("field.parentNode.disabled=true", 1),
        ("field.parentNode.style.display='none'", 1),
        ("field.parentNode.setAttribute('inert','')", 1),
        ("field.type='hidden'", 0),
    ] {
        let mut page = input_fixture(r#"<!doctype html><body id="body"><fieldset><input id="field" value="OLD"></fieldset></body>"#).await;
        page.js.as_mut().unwrap().execute_script("<fixup-events>",r#"
            const field=document.getElementById('field');window.log=[];
            for(const type of ['change','blur','focusout']) field.addEventListener(type,()=>log.push(type));
        "#).unwrap();
        page.js
            .as_mut()
            .unwrap()
            .native_fill("#field", "USER")
            .unwrap();
        page.js
            .as_mut()
            .unwrap()
            .execute_script("<focusability-mutation>", script)
            .unwrap();
        assert_eq!(
            page.evaluate("[document.activeElement.id,log]"),
            json!(["field", []]),
            "{script}"
        );
        page.js.as_ref().unwrap().with_dom(|dom| {
            let id = dom.query_selector_all("#field").unwrap()[0];
            dom.text_control(id);
            dom.text_content(dom.document());
        });
        page.screenshot((640.0, 480.0)).unwrap();
        assert_eq!(
            page.evaluate("log.length"),
            json!(0.0),
            "read/capture {script}"
        );
        page.js
            .as_mut()
            .unwrap()
            .run_event_loop_for_duration(10)
            .await
            .unwrap();
        let expected = if changes == 1 {
            json!(["change", "blur", "focusout"])
        } else {
            json!(["blur", "focusout"])
        };
        assert_eq!(page.evaluate("log"), expected, "{script}");
        assert_eq!(
            page.evaluate("document.activeElement.id"),
            json!("body"),
            "{script}"
        );
        page.js
            .as_mut()
            .unwrap()
            .run_event_loop_for_duration(10)
            .await
            .unwrap();
        assert_eq!(page.evaluate("log"), expected, "duplicate {script}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn native_focus_fixup_preserves_focusable_nodes_and_defers_reentry() {
    for script in [
        "field.style.display='none';field.style.display='block'",
        "field.disabled=true;field.disabled=false",
        "field.style.opacity='0'",
        "field.style.position='absolute';field.style.top='10000px'",
        "field.setAttribute('tabindex','-1')",
        "field.parentNode.style.visibility='hidden';field.style.visibility='visible'",
    ] {
        let mut page = input_fixture(r#"<!doctype html><div><input id="field"></div>"#).await;
        page.js
            .as_mut()
            .unwrap()
            .execute_script(
                "<retained-focus>",
                r#"
            const field=document.getElementById('field');window.blurs=0;
            field.addEventListener('blur',()=>blurs++);field.focus();
        "#,
            )
            .unwrap();
        page.js
            .as_mut()
            .unwrap()
            .execute_script("<restored-focusability>", script)
            .unwrap();
        page.js
            .as_mut()
            .unwrap()
            .run_event_loop_for_duration(10)
            .await
            .unwrap();
        assert_eq!(
            page.evaluate("[document.activeElement.id,blurs]"),
            json!(["field", 0]),
            "{script}"
        );
    }
    let mut page=input_fixture(r#"<!doctype html><body id="body"><input id="field"><div id="next" tabindex="0"></div></body>"#).await;
    page.js.as_mut().unwrap().execute_script("<fixup-reentry>",r#"
        const field=document.getElementById('field'),next=document.getElementById('next');window.log=[];
        field.focus();field.addEventListener('blur',()=>{log.push('field');next.focus();next.removeAttribute('tabindex')});
        next.addEventListener('blur',()=>log.push('next'));field.disabled=true;
    "#).unwrap();
    assert!(!page
        .js
        .as_mut()
        .unwrap()
        .run_autonomous_event_loop_turn()
        .await
        .unwrap());
    assert_eq!(
        page.evaluate("[document.activeElement.id,log]"),
        json!(["next", ["field"]])
    );
    assert!(!page
        .js
        .as_mut()
        .unwrap()
        .run_autonomous_event_loop_turn()
        .await
        .unwrap());
    assert_eq!(
        page.evaluate("[document.activeElement.id,log]"),
        json!(["body", ["field", "next"]])
    );
    assert!(page
        .js
        .as_mut()
        .unwrap()
        .run_autonomous_event_loop_turn()
        .await
        .unwrap());
}

#[tokio::test(flavor = "current_thread")]
async fn native_focus_fixup_runs_after_page_tasks_and_input_handlers() {
    let mut page = input_fixture(
        r#"<!doctype html><body id="body"><input id="field" value="OLD"></body>"#,
    )
    .await;
    page.js.as_mut().unwrap().execute_script("<input-fixup>",r#"
        const field=document.getElementById('field');window.log=[];
        field.addEventListener('input',()=>{log.push('input');Promise.resolve().then(()=>field.disabled=true)});
        for(const type of ['change','blur','focusout']) field.addEventListener(type,()=>log.push(type));
    "#).unwrap();
    page.js
        .as_mut()
        .unwrap()
        .native_fill("#field", "EDIT")
        .unwrap();
    assert_eq!(
        page.evaluate("[document.activeElement.id,log]"),
        json!(["body", ["input", "change", "blur", "focusout"]])
    );
    page.js
        .as_mut()
        .unwrap()
        .execute_script(
            "<timer-fixup>",
            r#"
        field.disabled=false;field.focus();log.length=0;setTimeout(()=>field.hidden=true,0);
    "#,
        )
        .unwrap();
    page.js
        .as_mut()
        .unwrap()
        .run_event_loop_for_duration(20)
        .await
        .unwrap();
    assert_eq!(
        page.evaluate("[document.activeElement.id,log]"),
        json!(["body", ["blur", "focusout"]])
    );
}
