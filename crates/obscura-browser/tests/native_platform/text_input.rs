use super::support::*;

#[tokio::test(flavor = "current_thread")]
async fn manual_text_replaces_utf16_selection_and_commits_change_on_blur() {
    let mut page = input_fixture(
        r#"<!doctype html><input id="v" value="A😀B"><button id="next">Next</button>"#,
    )
    .await;
    page.js.as_mut().unwrap().execute_script("<fixture>", r#"
        const v=document.getElementById('v');window.events=[];
        v.focus();v.setSelectionRange(1,3);
        for(const t of ['beforeinput','input','change']) v.addEventListener(t,e=>events.push([e.type,e.inputType||'',e.data??null,v.value,e.isTrusted]));
        globalThis.InputEvent=function(){throw Error('forged')};
        globalThis.__obscura_native_text_handoff=()=>{throw Error('forged')};
    "#).unwrap();
    assert!(!page
        .js
        .as_mut()
        .unwrap()
        .native_insert_text("中🚀")
        .unwrap());
    assert_eq!(
        page.evaluate("[v.value,v.defaultValue,v.selectionStart,v.selectionEnd]"),
        json!(["A中🚀B", "A😀B", 4, 4])
    );
    page.js.as_mut().unwrap().native_focus("#next").unwrap();
    assert_eq!(
        page.evaluate("events"),
        json!([
            ["beforeinput", "insertText", "中🚀", "A😀B", true],
            ["input", "insertText", "中🚀", "A中🚀B", true],
            ["change", "", null, "A中🚀B", true]
        ])
    );
}

    #[tokio::test(flavor = "current_thread")]
    async fn manual_edit_keys_preserve_unicode_and_have_native_event_order() {
        let mut page = input_fixture(
            r#"<!doctype html><textarea id="v">A😀B
中🚀D</textarea>"#,
        )
        .await;
        page.js.as_mut().unwrap().execute_script("<fixture>", r#"
            const v=document.getElementById('v');v.focus();v.setSelectionRange(3,3);window.events=[];
            for(const t of ['keydown','keyup','beforeinput','input']) v.addEventListener(t,e=>events.push([e.type,e.key||e.inputType,e.data??null,v.value,e.isTrusted]));
            globalThis.KeyboardEvent=function(){throw Error('forged')};
        "#).unwrap();
        assert!(!page
            .js
            .as_mut()
            .unwrap()
            .native_edit_key("Backspace")
            .unwrap());
        assert_eq!(
            page.evaluate("[v.value,v.selectionStart]"),
            json!(["AB\n中🚀D", 1])
        );
        assert_eq!(
            page.evaluate("events"),
            json!([
                ["keydown", "Backspace", null, "A😀B\n中🚀D", true],
                [
                    "beforeinput",
                    "deleteContentBackward",
                    null,
                    "A😀B\n中🚀D",
                    true
                ],
                ["input", "deleteContentBackward", null, "AB\n中🚀D", true],
                ["keyup", "Backspace", null, "AB\n中🚀D", true]
            ])
        );
        page.js
            .as_mut()
            .unwrap()
            .execute_script("<selection>", "v.setSelectionRange(4,4)")
            .unwrap();
        for (key, position) in [("ArrowRight", 6), ("ArrowLeft", 4), ("End", 7), ("Home", 3)] {
            assert!(!page.js.as_mut().unwrap().native_edit_key(key).unwrap());
            assert_eq!(
                page.evaluate("v.selectionStart").as_f64(),
                Some(position as f64)
            );
        }
        assert!(!page.js.as_mut().unwrap().native_edit_key("Delete").unwrap());
        assert_eq!(page.evaluate("v.value"), json!("AB\n🚀D"));
    }

#[tokio::test(flavor = "current_thread")]
async fn manual_text_cancelled_defaults_do_not_edit_and_keyup_still_runs() {
    for cancel in ["keydown", "beforeinput"] {
        let mut page = input_fixture(r#"<!doctype html><input id="v" value="AB">"#).await;
        page.js.as_mut().unwrap().execute_script("<fixture>", &format!(r#"
            const v=document.getElementById('v');v.focus();v.setSelectionRange(2,2);window.events=[];
            for(const t of ['keydown','beforeinput','input','keyup']) v.addEventListener(t,e=>{{events.push(t);if(t==='{}')e.preventDefault()}});
        "#, cancel)).unwrap();
        assert!(page
            .js
            .as_mut()
            .unwrap()
            .native_edit_key("Backspace")
            .unwrap());
        assert_eq!(page.evaluate("v.value"), json!("AB"));
        assert_eq!(
            page.evaluate("events"),
            if cancel == "keydown" {
                json!(["keydown", "keyup"])
            } else {
                json!(["keydown", "beforeinput", "keyup"])
            }
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn manual_text_stops_after_reentrant_focus_value_selection_or_navigation() {
    for change in [
        "v.value=v.value",
        "v.setSelectionRange(1,1)",
        "other.focus();v.focus()",
        "v.remove()",
        "v.style.display='none'",
        "location.href='/next'",
    ] {
        let mut page =
            input_fixture(r#"<!doctype html><input id="v" value="AB"><input id="other">"#)
                .await;
        page.js.as_mut().unwrap().execute_script("<fixture>", &format!(r#"
            const v=document.getElementById('v'),other=document.getElementById('other');v.focus();v.setSelectionRange(2,2);
            v.addEventListener('beforeinput',()=>queueMicrotask(()=>{{{}}}));
        "#, change)).unwrap();
        let error = page
            .js
            .as_mut()
            .unwrap()
            .native_insert_text("X")
            .unwrap_err();
        assert_eq!(error.1, "SENT", "{change}: {error:?}");
        assert_eq!(page.evaluate("v.value"), json!("AB"), "{change}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn manual_text_preflight_rejects_invalid_targets_limits_and_split_surrogates() {
    for (setup, error) in [
        ("v.blur()", "INPUT_NO_FOCUS"),
        ("v.readOnly=true", "ELEMENT_READONLY"),
        ("v.style.display='none'", "INPUT_TARGET_CHANGED"),
        ("v.maxLength=4", "INPUT_TOO_LONG"),
        ("v.setAttribute('maxlength','4suffix')", "INPUT_TOO_LONG"),
        ("v.setSelectionRange(2,2)", "INPUT_SELECTION_UNSUPPORTED"),
    ] {
        let mut page = input_fixture(r#"<!doctype html><input id="v" value="A😀B">"#).await;
        page.js.as_mut().unwrap().execute_script("<fixture>", &format!("const v=document.getElementById('v');v.focus();v.setSelectionRange(4,4);{setup};window.count=0;v.addEventListener('beforeinput',()=>count++)")).unwrap();
        assert_eq!(
            page.js
                .as_mut()
                .unwrap()
                .native_insert_text("X")
                .unwrap_err(),
            (error, "NOT_SENT")
        );
        assert_eq!(page.evaluate("count").as_f64(), Some(0.0));
    }
    let mut page = input_fixture("<!doctype html><input>").await;
    assert_eq!(
        page.js
            .as_mut()
            .unwrap()
            .native_insert_text(&"x".repeat(4097)),
        Err(("INPUT_VALUE_LIMIT", "NOT_SENT"))
    );
    assert_eq!(
        page.js.as_mut().unwrap().native_edit_key("Enter"),
        Err(("INPUT_KEY_UNSUPPORTED", "NOT_SENT"))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn manual_text_frame_identity_detects_invisible_selection_and_same_value_writes() {
    let mut page =
        input_fixture(r#"<!doctype html><input id="v" type="password" value="secret">"#).await;
    page.js
        .as_mut()
        .unwrap()
        .execute_script(
            "<fixture>",
            "const v=document.getElementById('v');v.focus()",
        )
        .unwrap();
    for change in [
        "v.setSelectionRange(1,1)",
        "v.value=v.value",
        "v.blur();v.focus()",
    ] {
        let identity = page.js.as_ref().unwrap().native_text_identity();
        page.js
            .as_mut()
            .unwrap()
            .execute_script("<change>", change)
            .unwrap();
        assert_ne!(identity, page.js.as_ref().unwrap().native_text_identity());
    }
}

#[tokio::test(flavor = "current_thread")]
async fn manual_text_normalization_keeps_caret_at_a_scalar_boundary() {
    for (html, text, value, caret) in [
        ("<input id='v' type='url' value='B'>", " 😀", "😀B", 2),
        ("<input id='v' value='B'>", "A\r\n😀", "A😀B", 3),
        ("<textarea id='v'>B</textarea>", "A\r\n😀", "A\n😀B", 4),
    ] {
        let mut page = input_fixture(html).await;
        page.js
            .as_mut()
            .unwrap()
            .execute_script(
                "<fixture>",
                "const v=document.getElementById('v');v.focus();v.setSelectionRange(0,0)",
            )
            .unwrap();
        assert!(!page.js.as_mut().unwrap().native_insert_text(text).unwrap());
        assert_eq!(
            page.evaluate("[v.value,v.selectionStart]"),
            json!([value, caret])
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn manual_key_stops_after_handlers_mutate_native_state() {
    for phase in ["keydown", "input", "keyup"] {
        let mut page = input_fixture("<!doctype html><input id='v' value='AB'>").await;
        page.js.as_mut().unwrap().execute_script("<fixture>", &format!("const v=document.getElementById('v');v.focus();v.setSelectionRange(2,2);v.addEventListener('{phase}',()=>{{v.value=v.value}})")).unwrap();
        assert_eq!(page.js.as_mut().unwrap().native_edit_key("Backspace"), Err(("INPUT_VALUE_CHANGED", "SENT")));
        assert_eq!(page.evaluate("v.value"), json!(if phase == "keydown" { "AB" } else { "A" }));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn manual_text_constraints_reflect_dom_attributes() {
    let mut page =
        input_fixture("<!doctype html><input id='v'><textarea id='area'></textarea>").await;
    assert_eq!(page.evaluate(r#"(()=>{
      const output=[];for(const v of [document.getElementById('v'),document.getElementById('area')]) {
        output.push([v.readOnly,v.maxLength]);v.readOnly=true;v.maxLength=4;
        output.push([v.hasAttribute('readonly'),v.getAttribute('maxlength')]);v.readOnly=false;
        try{v.maxLength=-1}catch(e){output.push([e.name,v.hasAttribute('readonly'),v.maxLength])}
      }return output;
    })()"#), json!([[false,-1],[true,"4"],["IndexSizeError",false,4],[false,-1],[true,"4"],["IndexSizeError",false,4]]));
}

#[tokio::test(flavor = "current_thread")]
async fn native_text_value_and_default_value_are_independent() {
    let mut page=input_fixture(r#"<!doctype html><input id="input" value="DEFAULT"><textarea id="area">ORIGINAL</textarea>"#).await;
    page.js.as_mut().unwrap().execute_script("<native-values>",r#"
        const input=document.getElementById('input'),area=document.getElementById('area');window.log=[];
        log.push([input.value,input.defaultValue,area.value,area.defaultValue]);
        input.value='EDITED';area.value='CURRENT';
        log.push([input.value,input.defaultValue,area.value,area.defaultValue,area.textContent]);
        input.defaultValue='NEXT';area.defaultValue='NEXT AREA';
        log.push([input.value,input.getAttribute('value'),area.value,area.textContent]);
        window._formValues[input._nid]='FORGED';window._formValues[area._nid]='FORGED';
    "#).unwrap();
    assert_eq!(
        page.evaluate("JSON.stringify(log)"),
        json!(serde_json::to_string(&json!([
            ["DEFAULT", "DEFAULT", "ORIGINAL", "ORIGINAL"],
            ["EDITED", "DEFAULT", "CURRENT", "ORIGINAL", "ORIGINAL"],
            ["EDITED", "NEXT", "CURRENT", "NEXT AREA"]
        ]))
        .unwrap())
    );
    page.js.as_ref().unwrap().with_dom(|dom| {
        for (selector, value, default) in [
            ("#input", "EDITED", "NEXT"),
            ("#area", "CURRENT", "NEXT AREA"),
        ] {
            let id = dom.query_selector_all(selector).unwrap()[0];
            let state = dom.text_control(id).unwrap();
            assert_eq!(state.value, value);
            assert_eq!(state.default_value, default);
            assert!(state.dirty);
        }
    });
    assert_eq!(
        page.evaluate("input.value+':'+area.value"),
        json!("EDITED:CURRENT")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn native_text_selection_uses_utf16_and_sanitizes_supported_controls() {
    let mut page = input_fixture(
        r#"<!doctype html><input id="text"><textarea id="area"></textarea>
        <input id="url" type="url"><input id="email" type="email" multiple>"#,
    )
    .await;
    page.js.as_mut().unwrap().execute_script("<native-selection>",r#"
        const text=document.getElementById('text'),area=document.getElementById('area'),url=document.getElementById('url'),email=document.getElementById('email');
        window.log=[];text.value='A🙂B';text.setSelectionRange(1,3,'backward');
        log.push([text.value,text.selectionStart,text.selectionEnd,text.selectionDirection]);
        text.setRangeText('X',1,3,'select');log.push([text.value,text.selectionStart,text.selectionEnd]);
        text.selectionStart=99;log.push([text.selectionStart,text.selectionEnd]);
        text.value='';text.setRangeText('Q');log.push(text.value);
        text.value='A\r\nB';area.value='A\r\nB\rC';url.value=' \t https://example.com/\r\n ';
        email.value=' a@example.com , b@example.com ';log.push([text.value,area.value,url.value,email.value,email.selectionStart]);
        try {email.setSelectionRange(0,1)} catch(error) {log.push(error.name)}
    "#).unwrap();
    assert_eq!(
        page.evaluate("JSON.stringify(log)"),
        json!(serde_json::to_string(&json!([
            ["A🙂B", 1, 3, "backward"],
            ["AXB", 1, 2],
            [3, 3],
            "Q",
            [
                "AB",
                "A\nB\nC",
                "https://example.com/",
                "a@example.com,b@example.com",
                null
            ],
            "InvalidStateError"
        ]))
        .unwrap())
    );
}

#[tokio::test(flavor = "current_thread")]
async fn native_text_lifecycle_clones_values_and_reset_obeys_cancellation() {
    let mut page = input_fixture(
        r#"<!doctype html><form id="form"><input id="input" value="DEFAULT">
        <textarea id="area">ORIGINAL</textarea><textarea id="clean">CLEAN</textarea></form>"#,
    )
    .await;
    page.js.as_mut().unwrap().execute_script("<native-text-lifecycle>",r#"
        const form=document.getElementById('form'),input=document.getElementById('input'),area=document.getElementById('area'),clean=document.getElementById('clean');window.log=[];
        input.value='EDIT';area.value='AREA EDIT';
        const copy=input.cloneNode(false);copy.id='copy';document.body.appendChild(copy);copy.value='COPY EDIT';
        input.remove();form.appendChild(input);log.push([input.value,copy.value]);
        const shallow=clean.cloneNode(false);log.push([shallow.value,shallow.textContent]);
        shallow.defaultValue='AWAY';shallow.defaultValue='';log.push(shallow.value);
        const deep=area.cloneNode(true);log.push([deep.value,deep.defaultValue]);
        form.addEventListener('reset',e=>e.preventDefault(),{once:true});form.reset();log.push([input.value,area.value]);
        form.reset();log.push([input.value,area.value]);
        input.defaultValue='NEW';area.textContent='NEW AREA';log.push([input.value,area.value]);
    "#).unwrap();
    assert_eq!(
        page.evaluate("JSON.stringify(log)"),
        json!(serde_json::to_string(&json!([
            ["EDIT", "COPY EDIT"],
            ["CLEAN", ""],
            "",
            ["AREA EDIT", "ORIGINAL"],
            ["EDIT", "AREA EDIT"],
            ["DEFAULT", "ORIGINAL"],
            ["NEW", "NEW AREA"]
        ]))
        .unwrap())
    );
    page.js.as_ref().unwrap().with_dom(|dom| {
        let id = dom.query_selector_all("#input").unwrap()[0];
        let data = dom.get_node(id).unwrap().data;
        dom.remove(id);
        let replacement = dom.new_node(data);
        assert_eq!(replacement, id);
        let state = dom.text_control(replacement).unwrap();
        assert_eq!(state.value, "NEW");
        assert!(!state.dirty);
    });
}

#[tokio::test(flavor = "current_thread")]
async fn native_fill_uses_native_value_and_real_input_events() {
    let mut page =
        input_fixture(r#"<!doctype html><input id="text" value="OLD"><div id="status"></div>"#)
            .await;
    assert_eq!(
        page.evaluate("typeof __obscura_native_text_handoff"),
        json!("undefined")
    );
    page.js.as_mut().unwrap().execute_script("<native-fill-events>",r#"
        const target=document.getElementById('text'), value=Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value');
        window.tracked='OLD';window.setterCalls=0;window.log=[];
        Object.defineProperty(target,'value',{get(){return value.get.call(this)},set(next){setterCalls++;tracked=next;value.set.call(this,next)}});
        for(const type of ['focus','select','beforeinput','input']) target.addEventListener(type,e=>{
            log.push([type,e.isTrusted,target.value,target.selectionStart,target.selectionEnd,e.inputType||'',e.data??null,e.cancelable]);
            if(type==='input' && tracked!==target.value) document.getElementById('status').textContent='FRAMEWORK UPDATED';
        });
        Element.prototype.focus=()=>{throw Error('page focus called')};
        Element.prototype.dispatchEvent=()=>{throw Error('page dispatch called')};
        window.InputEvent=()=>{throw Error('page input constructor called')};
        window.__obscura_native_text_handoff=()=>{throw Error('page text handoff called')};
    "#).unwrap();
    let result = page
        .js
        .as_mut()
        .unwrap()
        .native_fill("#text", "NEW VALUE")
        .unwrap();
    assert!(result.changed);
    assert_eq!(result.value, "NEW VALUE");
    assert_eq!(
        page.evaluate("JSON.stringify(log)"),
        json!(serde_json::to_string(&json!([
            ["focus", true, "OLD", 0, 0, "", null, false],
            ["select", true, "OLD", 0, 3, "", null, false],
            [
                "beforeinput",
                true,
                "OLD",
                0,
                3,
                "insertText",
                "NEW VALUE",
                true
            ],
            [
                "input",
                true,
                "NEW VALUE",
                9,
                9,
                "insertText",
                "NEW VALUE",
                false
            ]
        ]))
        .unwrap())
    );
    assert_eq!(page.evaluate("setterCalls===0"), json!(true));
    assert_eq!(
        page.evaluate("document.getElementById('status').textContent"),
        json!("FRAMEWORK UPDATED")
    );
    assert_eq!(page.evaluate("target.defaultValue"), json!("OLD"));
    assert!(
        !page
            .js
            .as_mut()
            .unwrap()
            .native_fill("#text", "NEW VALUE")
            .unwrap()
            .changed
    );
    assert_eq!(
        page.evaluate("target.selectionStart===9 && target.selectionEnd===9"),
        json!(true)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn native_fill_cancellation_and_reentrant_changes_are_not_overwritten() {
    for (script, code, value) in [
        ("e.preventDefault()", "INPUT_CANCELLED", "OLD"),
        ("target.value='PAGE'", "INPUT_VALUE_CHANGED", "PAGE"),
        (
            "Promise.resolve().then(()=>target.value='PAGE')",
            "INPUT_VALUE_CHANGED",
            "PAGE",
        ),
        (
            "document.getElementById('other').focus()",
            "INPUT_FOCUS_CHANGED",
            "OLD",
        ),
        ("target.disabled=true", "ELEMENT_DISABLED", "OLD"),
        ("target.style.display='none'", "ELEMENT_NOT_VISIBLE", "OLD"),
        (
            "target.setAttribute('maxlength','1')",
            "INPUT_TOO_LONG",
            "OLD",
        ),
        ("location.href='/next'", "UNEXPECTED_NAVIGATION", "OLD"),
        (
            "history.pushState({},'', '/next')",
            "UNEXPECTED_NAVIGATION",
            "OLD",
        ),
    ] {
        let mut page =
            input_fixture(r#"<!doctype html><input id="text" value="OLD"><input id="other">"#)
                .await;
        page.js.as_mut().unwrap().execute_script("<fill-reentry>",&format!(
            "const target=document.getElementById('text');window.inputs=0;target.addEventListener('input',()=>inputs++);target.addEventListener('beforeinput',e=>{{{script}}});")).unwrap();
        assert_eq!(
            page.js
                .as_mut()
                .unwrap()
                .native_fill("#text", "NEW VALUE")
                .err()
                .unwrap(),
            (code, "SENT"),
            "{script}"
        );
        assert_eq!(page.evaluate("target.value"), json!(value), "{script}");
        assert_eq!(page.evaluate("inputs===0"), json!(true), "{script}");
    }
    let mut page = input_fixture(r#"<!doctype html><input id="text" value="OLD">"#).await;
    page.js.as_mut().unwrap().execute_script("<input-reentry>","const target=document.getElementById('text');target.addEventListener('input',()=>target.value='PAGE');").unwrap();
    assert_eq!(
        page.js
            .as_mut()
            .unwrap()
            .native_fill("#text", "NEW")
            .err()
            .unwrap(),
        ("INPUT_VALUE_CHANGED", "SENT")
    );
    assert_eq!(page.evaluate("target.value"), json!("PAGE"));
}

#[tokio::test(flavor = "current_thread")]
async fn native_fill_validates_before_dispatch() {
    let mut page = input_fixture(
        r#"<!doctype html><input id="read" readonly><input id="limit" maxlength="3">
        <input id="number" type="number"><input id="disabled" disabled>"#,
    )
    .await;
    page.js
        .as_mut()
        .unwrap()
        .execute_script(
            "<preflight-events>",
            "window.focusEvents=0;document.addEventListener('focus',()=>focusEvents++,true);",
        )
        .unwrap();
    for (selector, value, code) in [
        ("#read", "X", "ELEMENT_READONLY"),
        ("#limit", "A🙂B", "INPUT_TOO_LONG"),
        ("#number", "1", "INPUT_ELEMENT_UNSUPPORTED"),
        ("#disabled", "X", "ELEMENT_DISABLED"),
    ] {
        assert_eq!(
            page.js
                .as_mut()
                .unwrap()
                .native_fill(selector, value)
                .err()
                .unwrap(),
            (code, "NOT_SENT")
        );
    }
    assert_eq!(page.evaluate("focusEvents===0"), json!(true));
}

#[tokio::test(flavor = "current_thread")]
async fn native_fill_scrolls_nested_containers_and_document_using_native_geometry() {
    let mut page = input_fixture(r#"<!doctype html><style>
        body{margin:0;height:2400px}
        #outer{position:absolute;left:40px;top:900px;width:300px;height:150px;overflow:auto}
        #inner{position:relative;left:400px;top:300px;width:200px;height:100px;overflow:auto}
        #field{position:relative;left:300px;top:250px;width:100px;height:30px}
        .space{width:900px;height:900px}
        </style><div id="outer"><div id="inner"><input id="field" value="OLD"><div class="space"></div></div><div class="space"></div></div>"#).await;
    page.js.as_mut().unwrap().execute_script("<scroll-fixture>", r#"
        globalThis.scrollEvents=[];
        for (const id of ['inner','outer']) document.getElementById(id).addEventListener('scroll',e=>scrollEvents.push([id,e.isTrusted,e.bubbles]));
        document.addEventListener('scroll',e=>scrollEvents.push(['document',e.isTrusted,e.bubbles]));
        Element.prototype.scrollIntoView=Element.prototype.scrollTo=globalThis.scrollTo=()=>{throw Error('public scrolling called')};
        Element.prototype.getBoundingClientRect=()=>{throw Error('public geometry called')};
    "#).unwrap();
    let result = page
        .js
        .as_mut()
        .unwrap()
        .native_fill("#field", "SCROLLED")
        .unwrap();
    assert!(result.changed);
    assert_eq!(
        page.evaluate("scrollEvents"),
        json!([
            ["inner", true, false],
            ["outer", true, false],
            ["document", true, true]
        ])
    );
    assert_eq!(page.evaluate("document.getElementById('inner').scrollTop>0 && document.getElementById('outer').scrollTop>0 && window.scrollY>0"), json!(true));
    let target = page.js.as_ref().unwrap().input_target("#field").unwrap();
    assert!(target.x >= 0.0 && target.y >= 0.0 && target.x < 640.0 && target.y < 480.0);
    page.evaluate("scrollEvents.length=0");
    page.js
        .as_mut()
        .unwrap()
        .native_fill("#field", "AGAIN")
        .unwrap();
    assert_eq!(page.evaluate("scrollEvents.length===0"), json!(true));
}

#[tokio::test(flavor = "current_thread")]
async fn native_fill_stops_after_scroll_callback_changes_the_target() {
    let mut page = input_fixture(r#"<!doctype html><style>body{margin:0;height:2400px}input{position:absolute;top:1000px;width:100px;height:30px}</style><input id="field" value="OLD">"#).await;
    page.js.as_mut().unwrap().execute_script("<scroll-reentry>", r#"
        globalThis.focusCalls=0;
        document.getElementById('field').addEventListener('focus',()=>focusCalls++);
        document.addEventListener('scroll',()=>Promise.resolve().then(()=>document.getElementById('field').value='PAGE'));
    "#).unwrap();
    assert_eq!(
        page.js
            .as_mut()
            .unwrap()
            .native_fill("#field", "NEW")
            .err()
            .unwrap(),
        ("INPUT_VALUE_CHANGED", "SENT")
    );
    assert_eq!(
        page.evaluate("[document.getElementById('field').value,focusCalls===0]"),
        json!(["PAGE", true])
    );
}

#[tokio::test(flavor = "current_thread")]
async fn native_fill_rejects_target_replacement_and_focus_microtask_reentry() {
    for (callback, code) in [
        (
            "field.replaceWith(field.cloneNode(true))",
            "INPUT_TARGET_CHANGED",
        ),
        ("field.remove()", "ELEMENT_NOT_FOUND"),
        ("field.setAttribute('inert','')", "ELEMENT_DISABLED"),
        (
            "Promise.resolve().then(()=>{other.focus();field.focus()})",
            "INPUT_FOCUS_CHANGED",
        ),
    ] {
        let mut page = input_fixture(
            r#"<!doctype html><input id="field" value="OLD"><button id="other">OTHER</button>"#,
        )
        .await;
        page.js
            .as_mut()
            .unwrap()
            .execute_script(
                "<fill-reentry>",
                &format!(
                    r#"
            const field=document.getElementById('field'),other=document.getElementById('other');
            globalThis.inputCalls=0;field.addEventListener('input',()=>inputCalls++);
            field.addEventListener('beforeinput',()=>{{{callback}}},{{once:true}});
        "#
                ),
            )
            .unwrap();
        assert_eq!(
            page.js
                .as_mut()
                .unwrap()
                .native_fill("#field", "NEW")
                .err()
                .unwrap(),
            (code, "SENT"),
            "{callback}"
        );
        assert_eq!(page.evaluate("inputCalls===0"), json!(true));
    }
}

    #[tokio::test(flavor = "current_thread")]
    async fn native_text_paint_reads_current_values_and_masks_passwords() {
        let mut page=input_fixture(r#"<!doctype html><style>body{margin:0}input,textarea{font:20px monospace;width:160px;height:60px}</style>
            <input id="text" value="OLD"><textarea id="area">OLD AREA</textarea><input id="password" type="password" value="SECRET">"#).await;
        let reference=input_fixture(r#"<!doctype html><style>body{margin:0}input,textarea{font:20px monospace;width:160px;height:60px}</style>
            <input id="text" value="NEW"><textarea id="area">LINE 1
LINE 2</textarea><input id="password" type="password" value="HIDDEN">"#).await;
        let before = page.screenshot((640.0, 480.0)).unwrap();
        page.js.as_mut().unwrap().execute_script("<paint-values>","document.getElementById('text').value='NEW';document.getElementById('area').value='LINE 1\\nLINE 2';").unwrap();
        let after = page.screenshot((640.0, 480.0)).unwrap();
        assert_ne!(before, after);
        assert_eq!(after, reference.screenshot((640.0, 480.0)).unwrap());
        for _ in 0..3 {
            assert_eq!(after, page.screenshot((640.0, 480.0)).unwrap());
        }
    }

#[tokio::test(flavor = "current_thread")]
async fn native_text_change_commits_once_before_blur_using_private_events() {
    let mut page = input_fixture(r#"<!doctype html><body id="body"><input id="field" value="OLD"><input id="next"></body>"#).await;
    page.js.as_mut().unwrap().execute_script("<text-change>", r#"
        const field=document.getElementById('field');window.log=[];
        for(const type of ['change','blur','focusout']) field.addEventListener(type,e=>{
            log.push([type,field.value,document.activeElement.id,e.isTrusted,e.bubbles,e.cancelable,e.composed]);
        });
        field.value='SCRIPT';field.focus();field.blur();log.length=0;
        Element.prototype.blur=()=>{throw Error('public blur called')};
        Element.prototype.dispatchEvent=()=>{throw Error('public dispatch called')};
        window.Event=()=>{throw Error('public event called')};
    "#).unwrap();
    page.js
        .as_mut()
        .unwrap()
        .native_fill("#field", "ONE")
        .unwrap();
    page.js
        .as_mut()
        .unwrap()
        .native_fill("#field", "TWO")
        .unwrap();
    assert_eq!(page.evaluate("log.length"), json!(0.0));
    assert!(page.js.as_mut().unwrap().native_focus("#next").unwrap());
    assert_eq!(
        page.evaluate("log"),
        json!([
            ["change", "TWO", "body", true, true, false, false],
            ["blur", "TWO", "body", true, false, false, true],
            ["focusout", "TWO", "body", true, true, false, true]
        ])
    );
    page.js.as_mut().unwrap().native_focus("#field").unwrap();
    page.js.as_mut().unwrap().native_focus("#next").unwrap();
    assert_eq!(
        page.evaluate("log.filter(e=>e[0]==='change').length"),
        json!(1.0)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn native_text_change_distinguishes_script_writes_reverts_and_cancelled_edits() {
    for selector in ["#field", "#area"] {
        for (first, second, script, expected) in [
            (None, None, "field.value='SCRIPT'", 0),
            (Some("USER"), None, "field.value='SCRIPT'", 1),
            (Some("USER"), None, "field.value='OLD'", 0),
            (
                Some("USER"),
                None,
                "field.value='OLD';field.value='LATER'",
                1,
            ),
            (Some("USER"), Some("OLD"), "field.value='LATER'", 0),
            (Some("USER"), Some("OLD"), "", 0),
            (
                Some("USER"),
                None,
                "field.addEventListener('beforeinput',e=>e.preventDefault())",
                1,
            ),
            (
                None,
                None,
                "field.addEventListener('beforeinput',e=>e.preventDefault())",
                0,
            ),
        ] {
            let mut page = input_fixture(r#"<!doctype html><input id="field" value="OLD"><textarea id="area">OLD</textarea><button id="next">NEXT</button>"#).await;
            page.js.as_mut().unwrap().execute_script("<text-change-case>", &format!(
                "const field=document.querySelector('{selector}');window.changes=0;field.addEventListener('change',()=>changes++);field.focus();"
            )).unwrap();
            for value in [first, second].into_iter().flatten() {
                page.js
                    .as_mut()
                    .unwrap()
                    .native_fill(selector, value)
                    .unwrap();
            }
            page.js
                .as_mut()
                .unwrap()
                .execute_script("<script-edit>", script)
                .unwrap();
            if script.contains("preventDefault") {
                assert_eq!(
                    page.js
                        .as_mut()
                        .unwrap()
                        .native_fill(selector, "CANCELLED")
                        .err()
                        .unwrap(),
                    ("INPUT_CANCELLED", "SENT")
                );
            }
            page.js.as_mut().unwrap().native_focus("#next").unwrap();
            assert_eq!(
                page.evaluate(&format!("changes==={expected}")),
                json!(true),
                "{selector} {first:?} {second:?} {script}"
            );
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn native_text_change_reentry_preserves_newer_focus_and_consumes_pending_edit() {
    let mut page = input_fixture(
        r#"<!doctype html><input id="field"><input id="next"><input id="newer">"#,
    )
    .await;
    page.js.as_mut().unwrap().execute_script("<change-reentry>", r#"
        const field=document.getElementById('field');window.log=[];
        field.addEventListener('change',()=>{log.push('change');document.getElementById('newer').focus()});
        field.addEventListener('blur',()=>log.push('blur'));
    "#).unwrap();
    page.js
        .as_mut()
        .unwrap()
        .native_fill("#field", "EDIT")
        .unwrap();
    assert!(!page.js.as_mut().unwrap().native_focus("#next").unwrap());
    assert_eq!(
        page.evaluate("[document.activeElement.id,log]"),
        json!(["newer", ["change"]])
    );
    page.js.as_mut().unwrap().native_focus("#field").unwrap();
    assert!(page.js.as_mut().unwrap().native_focus("#next").unwrap());
    assert_eq!(page.evaluate("log"), json!(["change", "blur"]));
}

#[tokio::test(flavor = "current_thread")]
async fn native_text_change_does_not_survive_copy_type_change_or_disconnect() {
    for script in [
        "field.type='search'",
        "field.type='hidden';field.type='text'",
        "field.remove();document.body.appendChild(field)",
        "const parent=field.parentNode;parent.remove();document.body.appendChild(parent)",
        "const copy=field.cloneNode(true);field.replaceWith(copy);field=copy",
        "const copy=document.importNode(field,true);field.replaceWith(copy);field=copy",
    ] {
        let mut page = input_fixture(
            r#"<!doctype html><div><input id="field" value="OLD"></div><input id="next">"#,
        )
        .await;
        page.js
            .as_mut()
            .unwrap()
            .execute_script(
                "<edit-lifecycle>",
                r#"
            let field=document.getElementById('field');window.changes=0;window.blurs=0;
            document.addEventListener('change',()=>changes++);
            field.addEventListener('blur',()=>blurs++);
        "#,
            )
            .unwrap();
        page.js
            .as_mut()
            .unwrap()
            .native_fill("#field", "USER")
            .unwrap();
        page.js
            .as_mut()
            .unwrap()
            .execute_script("<edit-mutation>", script)
            .unwrap();
        assert_eq!(page.evaluate("[changes,blurs]"), json!([0, 0]), "{script}");
        page.js.as_mut().unwrap().native_focus("#field").unwrap();
        page.js.as_mut().unwrap().native_focus("#next").unwrap();
        assert_eq!(page.evaluate("changes"), json!(0.0), "{script}");
    }
    // Cross-document native import must also omit the pending user edit.
    let source = obscura_dom::parse_html("<input id='field' value='OLD'>");
    let field = source.query_selector_all("#field").unwrap()[0];
    source.set_user_text_value(field, "USER").unwrap();
    let target = obscura_dom::DomTree::new();
    let copy = target
        .import_node_from(target.document(), &source, field)
        .unwrap();
    assert!(!target.take_text_change(copy));
    assert!(source.take_text_change(field));
}
