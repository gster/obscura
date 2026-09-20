use super::support::*;

#[tokio::test(flavor = "current_thread")]
async fn native_checked_state_defaults_selection_and_value_are_not_public_tables() {
    let mut page=input_fixture(r#"<!doctype html><form id="form"><input id="check" type="checkbox" checked><input id="radio" type="radio" name="a"></form><div id="fake" checked></div>"#).await;
    page.js.as_mut().unwrap().execute_script("<checked-state>",r#"
        const check=document.getElementById('check');
        globalThis.initial=[check.checked,check.defaultChecked,check.value,check.matches(':checked')];
        check.checked=false;check.indeterminate=true;
        check.defaultChecked=false;check.defaultChecked=true;
        _formChecked[check._nid]=true;_formIndeterminate[check._nid]=false;_formValues[check._nid]='FORGED';
        check.value='CHOICE';
        globalThis.current=[check.checked,check.defaultChecked,check.indeterminate,check.value,
            check.getAttribute('value'),check.matches(':checked'),check.matches(':indeterminate'),
            document.getElementById('fake').matches(':checked'),document.getElementById('radio').matches(':indeterminate')];
    "#).unwrap();
    assert_eq!(page.evaluate("initial"), json!([true, true, "on", true]));
    assert_eq!(
        page.evaluate("current"),
        json!([false, true, true, "CHOICE", "CHOICE", false, true, false, true])
    );
    page.evaluate("document.getElementById('form').reset()");
    assert_eq!(page.evaluate("[document.getElementById('check').checked,document.getElementById('check').indeterminate,document.getElementById('check').value]"),json!([true,true,"CHOICE"]));
}

#[tokio::test(flavor = "current_thread")]
async fn native_radio_groups_follow_form_owner_type_name_and_connection() {
    let mut page=input_fixture(r#"<!doctype html><form id="one"><input id="a" type="radio" name="g" checked><input id="b" type="radio" name="g" checked></form><form id="two"><input id="c" type="radio" name="g" checked></form><input id="external" type="radio" name="g" form="one"><div id="duplicate"></div><form id="duplicate"></form><input id="unowned" type="radio" name="g" form="duplicate">"#).await;
    page.js.as_mut().unwrap().execute_script("<radio-group>",r#"
        const a=document.getElementById('a'),b=document.getElementById('b'),c=document.getElementById('c'),external=document.getElementById('external');
        globalThis.initialGroup=[a.checked,b.checked,c.checked,external.form.id,document.getElementById('unowned').form===null];
        a.checked=true;globalThis.selectedGroup=[a.checked,b.checked,c.checked];
        external.checked=true;globalThis.externalGroup=[a.checked,b.checked,external.checked];
        external.setAttribute('form','two');globalThis.movedGroup=[external.checked,c.checked];
        a.checked=true;external.setAttribute('form','one');globalThis.regrouped=[a.checked,external.checked];
        b.checked=true;external.setAttribute('name','other');external.checked=true;external.setAttribute('name','g');
        globalThis.renamed=[b.checked,external.checked];
        external.remove();b.checked=true;document.body.append(external);globalThis.reinserted=[b.checked,external.checked];
        external.type='text';b.checked=true;external.type='radio';globalThis.retyped=[b.checked,external.checked];
    "#).unwrap();
    assert_eq!(
        page.evaluate("initialGroup"),
        json!([false, true, true, "one", true])
    );
    assert_eq!(page.evaluate("selectedGroup"), json!([true, false, true]));
    assert_eq!(page.evaluate("externalGroup"), json!([false, false, true]));
    for variable in ["regrouped", "renamed", "reinserted", "retyped"] {
        assert_eq!(page.evaluate(variable), json!([false, true]), "{variable}");
    }
    assert_eq!(page.evaluate("movedGroup"), json!([true, false]));
}

#[tokio::test(flavor = "current_thread")]
async fn native_checked_clones_and_external_form_reset_preserve_default_contract() {
    let mut page=input_fixture(r#"<!doctype html><form id="form"><input id="source" type="checkbox" checked></form><input id="outside" type="checkbox" form="form" checked>"#).await;
    page.js.as_mut().unwrap().execute_script("<checked-lifecycle>",r#"
        const source=document.getElementById('source'),outside=document.getElementById('outside'),form=document.getElementById('form');
        source.checked=false;source.indeterminate=true;outside.checked=false;
        const clone=source.cloneNode(true);clone.id='clone';document.body.append(clone);
        globalThis.cloned=[clone.checked,clone.defaultChecked,clone.indeterminate];
        clone.checked=true;source.remove();form.append(source);
        globalThis.reinserted=[source.checked,source.indeterminate,clone.checked];
        form.addEventListener('reset',event=>event.preventDefault(),{once:true});form.reset();
        globalThis.cancelledReset=[source.checked,outside.checked];form.reset();
        globalThis.resetValues=[source.checked,outside.checked,source.indeterminate,form.elements.length===2];
    "#).unwrap();
    assert_eq!(page.evaluate("cloned"), json!([false, true, true]));
    assert_eq!(page.evaluate("reinserted"), json!([false, true, true]));
    assert_eq!(page.evaluate("cancelledReset"), json!([false, false]));
    assert_eq!(
        page.evaluate("resetValues"),
        json!([true, true, true, true])
    );
}

#[tokio::test(flavor = "current_thread")]
async fn native_checked_import_and_arena_reuse_do_not_share_live_state() {
    let page =
        input_fixture(r#"<!doctype html><input id="source" type="checkbox" checked>"#).await;
    page.js.as_ref().unwrap().with_dom(|source| {
        let id = source.query_selector_all("#source").unwrap()[0];
        source.set_checked(id, false);
        source.set_indeterminate(id, true);
        let destination = obscura_dom::DomTree::new();
        let imported = destination
            .import_node_from(destination.document(), source, id)
            .unwrap();
        let state = destination.checked_state(imported).unwrap();
        assert!(!state.checked && state.default_checked && state.dirty && state.indeterminate);
        destination.reset_checked(imported);
        let state = destination.checked_state(imported).unwrap();
        assert!(state.checked && state.default_checked && !state.dirty && state.indeterminate);
        assert!(!source.checked_state(id).unwrap().checked);
        let data = destination.get_node(imported).unwrap().data;
        destination.remove(imported);
        let replacement = destination.new_node(data);
        assert_eq!(replacement, imported);
        let state = destination.checked_state(replacement).unwrap();
        assert!(state.checked && state.default_checked && !state.dirty && !state.indeterminate);
    });
}

#[tokio::test(flavor = "current_thread")]
async fn native_checked_pixels_follow_state_appearance_and_overflow_clips() {
    let mut page=input_fixture(r#"<!doctype html><style>
        body{margin:0;background:white}input{margin:0;width:24px;height:24px}
        #check,#radio,#none,#parent,#clip{position:absolute;top:20px}
        #check{left:20px}#radio{left:80px}#none{left:140px;appearance:none;background:rgb(7,9,11)}
        #parent{left:200px;appearance:none}#inherited{appearance:inherit;background:rgb(7,9,11)}
        #clip{left:260px;width:12px;height:24px;overflow:hidden}
        #clipped{position:absolute;left:0;top:0}
        </style><input id="check" type="checkbox"><input id="radio" type="radio">
        <input id="none" type="checkbox" checked><div id="parent"><input id="inherited" type="checkbox" checked></div>
        <div id="clip"><input id="clipped" type="checkbox" checked></div>"#).await;
    assert_eq!(pixel(&page, 25, 25), [255, 255, 255, 255]);
    for x in [145, 205] {
        assert_eq!(pixel(&page, x, 25), [7, 9, 11, 255]);
    }
    page.evaluate("(document.getElementById('check').checked=true,document.getElementById('radio').checked=true)");
    assert_eq!(pixel(&page, 25, 25), [25, 103, 210, 255]);
    assert_eq!(pixel(&page, 92, 32), [25, 103, 210, 255]);
    assert_eq!(pixel(&page, 264, 24), [25, 103, 210, 255]);
    assert_eq!(pixel(&page, 276, 24), [255, 255, 255, 255]);
    page.evaluate("document.getElementById('check').indeterminate=true");
    assert_eq!(pixel(&page, 32, 32), [255, 255, 255, 255]);
}

#[tokio::test(flavor = "current_thread")]
async fn native_click_activates_checked_state_through_private_events() {
    let mut page=input_fixture(r#"<!doctype html><style>body{margin:0}#check{position:absolute;left:20px;top:20px;width:30px;height:30px;margin:0}</style><input id="check" type="checkbox">"#).await;
    page.js.as_mut().unwrap().execute_script("<native-click>",r#"
        const check=document.getElementById('check'),RealPointerEvent=PointerEvent;globalThis.events=[];globalThis.clickFacts=[];
        for(const type of ['pointermove','mousemove','pointerdown','mousedown','focus','focusin','pointerup','mouseup','click','input','change']) {
            check.addEventListener(type,event=>{
                events.push(type);
                if(type==='click') clickFacts=[event instanceof RealPointerEvent,event.isTrusted,event.detail===1,event.clientX===35,event.clientY===35,event.buttons===0,check.matches(':checked'),check.indeterminate];
                if(type==='input'||type==='change') events.push([event.isTrusted,event.bubbles,!event.cancelable,event.composed]);
            });
        }
        check.indeterminate=true;
        Element.prototype.click=()=>{throw Error('public click used')};
        Element.prototype.dispatchEvent=()=>{throw Error('public dispatch used')};
        Object.defineProperty(check,'checked',{get(){return false},set(){throw Error('public checked used')}});
        globalThis.__obscura_markTrusted=()=>{throw Error('public trust used')};
        globalThis.PointerEvent=()=>{throw Error('public constructor used')};
    "#).unwrap();
    let result = page.js.as_mut().unwrap().native_click("#check").unwrap();
    assert!(!result.default_prevented);
    assert!(page
        .js
        .as_ref()
        .unwrap()
        .with_dom(|dom| dom.checked_state(result.node).unwrap().checked)
        .unwrap());
    assert_eq!(
        page.evaluate("clickFacts"),
        json!([true, true, true, true, true, true, true, false])
    );
    assert_eq!(
        page.evaluate("events"),
        json!([
            "pointermove",
            "mousemove",
            "pointerdown",
            "mousedown",
            "focus",
            "focusin",
            "pointerup",
            "mouseup",
            "click",
            "input",
            [true, true, true, true],
            "change",
            [true, true, true, false]
        ])
    );
    assert_eq!(page.evaluate("document.activeElement.id"), json!("check"));
}

#[tokio::test(flavor = "current_thread")]
async fn native_click_cancellation_restores_controls_and_radio_membership() {
    let mut page=input_fixture(r#"<!doctype html><style>input{width:30px;height:30px}</style><input id="check" type="checkbox"><input id="a" type="radio" name="group" checked><input id="b" type="radio" name="group">"#).await;
    page.js.as_mut().unwrap().execute_script("<cancel-activation>",r#"
        const check=document.getElementById('check'),a=document.getElementById('a'),b=document.getElementById('b');
        globalThis.cancelFacts=[];globalThis.events=[];check.indeterminate=true;
        check.addEventListener('click',event=>{cancelFacts.push([check.checked,check.indeterminate]);event.preventDefault()},{once:true});
        b.addEventListener('click',event=>{cancelFacts.push([a.checked,b.checked]);event.preventDefault()},{once:true});
        for(const node of [check,a,b]) for(const type of ['input','change']) node.addEventListener(type,event=>events.push(event.type));
    "#).unwrap();
    assert!(
        page.js
            .as_mut()
            .unwrap()
            .native_click("#check")
            .unwrap()
            .default_prevented
    );
    assert!(
        page.js
            .as_mut()
            .unwrap()
            .native_click("#b")
            .unwrap()
            .default_prevented
    );
    assert_eq!(
        page.evaluate("cancelFacts"),
        json!([[true, false], [false, true]])
    );
    assert_eq!(
        page.evaluate(
            "[check.checked,check.indeterminate,a.checked,b.checked,events.length===0]"
        ),
        json!([false, true, true, false, true])
    );
    page.js.as_mut().unwrap().execute_script("<change-radio-group>","b.addEventListener('click',event=>{a.name='other';event.preventDefault()},{once:true});").unwrap();
    assert!(
        page.js
            .as_mut()
            .unwrap()
            .native_click("#b")
            .unwrap()
            .default_prevented
    );
    assert_eq!(
        page.evaluate("[a.checked,b.checked,events.length===0]"),
        json!([false, false, true])
    );
    assert!(
        !page
            .js
            .as_mut()
            .unwrap()
            .native_click("#b")
            .unwrap()
            .default_prevented
    );
    assert_eq!(page.evaluate("events"), json!(["input", "change"]));
    assert!(
        !page
            .js
            .as_mut()
            .unwrap()
            .native_click("#b")
            .unwrap()
            .default_prevented
    );
    assert_eq!(page.evaluate("events"), json!(["input", "change"]));
}

#[tokio::test(flavor = "current_thread")]
async fn native_click_pointer_cancel_suppresses_mouse_but_preserves_click() {
    let mut page=input_fixture(r#"<!doctype html><style>input{width:30px;height:30px}</style><input id="check" type="checkbox">"#).await;
    page.js.as_mut().unwrap().execute_script("<cancel-pointer>",r#"
        globalThis.events=[];const check=document.getElementById('check');
        for(const type of ['pointerdown','mousedown','pointerup','mouseup','click','input','change']) check.addEventListener(type,event=>events.push(event.type));
        check.addEventListener('pointerdown',event=>event.preventDefault(),{once:true});
    "#).unwrap();
    page.js.as_mut().unwrap().native_click("#check").unwrap();
    assert_eq!(
        page.evaluate("events"),
        json!(["pointerdown", "pointerup", "click", "input", "change"])
    );
    page.evaluate("events.length=0");
    page.js.as_mut().unwrap().native_click("#check").unwrap();
    assert_eq!(
        page.evaluate("events"),
        json!([
            "pointerdown",
            "mousedown",
            "pointerup",
            "mouseup",
            "click",
            "input",
            "change"
        ])
    );
}

#[tokio::test(flavor = "current_thread")]
async fn native_click_stops_after_target_replacement_at_every_pointer_stage() {
    for stage in [
        "pointermove",
        "mousemove",
        "pointerdown",
        "mousedown",
        "focus",
        "pointerup",
        "mouseup",
    ] {
        let mut page=input_fixture(r#"<!doctype html><style>#target{position:absolute;left:20px;top:20px;width:30px;height:30px;margin:0}</style><input id="target" type="checkbox">"#).await;
        page.js.as_mut().unwrap().execute_script("<replace-click-target>",&format!(r#"
            globalThis.events=[];const target=document.getElementById('target');
            for(const type of ['pointermove','mousemove','pointerdown','mousedown','focus','pointerup','mouseup','click']) target.addEventListener(type,event=>events.push(event.type));
            target.addEventListener('{stage}',()=>Promise.resolve().then(()=>{{target.remove();const replacement=document.createElement('input');replacement.id='target';replacement.type='checkbox';document.body.append(replacement)}}),{{once:true}});
        "#)).unwrap();
        let error = page
            .js
            .as_mut()
            .unwrap()
            .native_click("#target")
            .unwrap_err();
        assert_eq!(error, ("INPUT_TARGET_CHANGED", "SENT"), "{stage}");
        assert_eq!(
            page.evaluate("events[events.length-1]"),
            json!(stage),
            "{stage}"
        );
        assert_eq!(page.evaluate("[events.includes('click'),document.getElementById('target').checked,document.querySelectorAll(':active').length===0]"),json!([false,false,true]),"{stage}");
        page.js.as_mut().unwrap().native_click("#target").unwrap();
        assert_eq!(
            page.evaluate("document.getElementById('target').checked"),
            json!(true),
            "{stage}"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn native_click_label_forwards_once_and_obeys_cancel_disabled_and_interactive_targets() {
    let mut page=input_fixture(r#"<!doctype html><style>label{display:block;width:200px;height:40px}input,button{width:30px;height:30px}</style>
        <label id="label" for="check">EXPLICIT</label><input id="check" type="checkbox">
        <label id="nested">NESTED<input id="inside" type="checkbox"></label>
        <label id="interactive" for="check"><button id="button" type="button">B</button></label>
        <label id="disabled" for="off">DISABLED</label><input id="off" type="checkbox" disabled>"#).await;
    page.js.as_mut().unwrap().execute_script("<label-click>",r#"
        globalThis.events=[];document.addEventListener('click',event=>events.push(event.target.id));
        globalThis.__obscura_activateLabel=()=>{throw Error('public label used')};
        Element.prototype.click=()=>{throw Error('public click used')};
    "#).unwrap();
    page.js.as_mut().unwrap().native_click("#label").unwrap();
    assert_eq!(
        page.evaluate("[events,document.getElementById('check').checked]"),
        json!([["label", "check"], true])
    );
    page.evaluate("events.length=0");
    page.js.as_mut().unwrap().native_click("#inside").unwrap();
    assert_eq!(
        page.evaluate("[events,document.getElementById('inside').checked]"),
        json!([["inside"], true])
    );
    page.js.as_mut().unwrap().native_click("#button").unwrap();
    page.js.as_mut().unwrap().native_click("#disabled").unwrap();
    assert_eq!(page.evaluate("[events,document.getElementById('check').checked,document.getElementById('off').checked]"),json!([["inside","button","disabled"],true,false]));
    page.js.as_mut().unwrap().execute_script("<cancel-label>","document.getElementById('label').addEventListener('click',event=>event.preventDefault(),{once:true});events.length=0;").unwrap();
    assert!(
        page.js
            .as_mut()
            .unwrap()
            .native_click("#label")
            .unwrap()
            .default_prevented
    );
    assert_eq!(page.evaluate("events"), json!(["label"]));
}

#[tokio::test(flavor = "current_thread")]
async fn native_click_rejects_unsupported_defaults_and_scrolls_supported_controls() {
    let mut page=input_fixture(r#"<!doctype html><style>input,button{width:30px;height:30px}#scrolled{position:absolute;left:20px;top:900px}</style>
        <input id="file" type="file"><select id="picker"><option>A</option></select><a id="link" href="/next" target="_blank">LINK</a>
        <form target="_blank"><button id="submit">SUBMIT</button></form><button id="command" type="button" popovertarget="popover">POP</button>
        <fieldset disabled><legend><input id="legend" type="checkbox"></legend><input id="disabled" type="checkbox"></fieldset>
        <input id="scrolled" type="checkbox">"#).await;
    page.js.as_mut().unwrap().execute_script("<preflight-click>","globalThis.events=[];for(const type of ['pointermove','pointerdown','click'])document.addEventListener(type,event=>events.push(event.type));").unwrap();
    for selector in ["#file", "#picker", "#link", "#submit", "#command"] {
        assert_eq!(
            page.js
                .as_mut()
                .unwrap()
                .native_click(selector)
                .unwrap_err(),
            ("INPUT_ELEMENT_UNSUPPORTED", "NOT_SENT"),
            "{selector}"
        );
    }
    assert_eq!(
        page.js
            .as_mut()
            .unwrap()
            .native_click("#disabled")
            .unwrap_err(),
        ("ELEMENT_DISABLED", "NOT_SENT")
    );
    assert_eq!(page.evaluate("events.length===0"), json!(true));
    page.js.as_mut().unwrap().native_click("#legend").unwrap();
    page.js.as_mut().unwrap().native_click("#scrolled").unwrap();
    assert_eq!(page.evaluate("[document.getElementById('legend').checked,document.getElementById('scrolled').checked,scrollY>0]"),json!([true,true,true]));
}

#[tokio::test(flavor = "current_thread")]
async fn native_click_observes_checkbox_handler_writes_but_only_changed_radio_events() {
    let mut page=input_fixture(r#"<!doctype html><style>input{width:30px;height:30px}</style><input id="check" type="checkbox"><input id="radio" type="radio" name="group" checked><input id="detach" type="checkbox">"#).await;
    page.js.as_mut().unwrap().execute_script("<activation-handler-writes>",r#"
        globalThis.events=[];const check=document.getElementById('check'),radio=document.getElementById('radio'),detach=document.getElementById('detach');
        for(const node of [check,radio,detach])for(const type of ['input','change'])node.addEventListener(type,event=>events.push([event.target.id,type]));
        check.addEventListener('click',()=>{check.checked=false;check.indeterminate=true});
        radio.addEventListener('click',()=>{radio.checked=false});
        detach.addEventListener('click',()=>detach.remove());
    "#).unwrap();
    for selector in ["#check", "#radio", "#detach"] {
        page.js.as_mut().unwrap().native_click(selector).unwrap();
    }
    assert_eq!(
        page.evaluate("events"),
        json!([["check", "input"], ["check", "change"]])
    );
    assert_eq!(page.evaluate("[check.checked,check.indeterminate,radio.checked,detach.checked,detach.isConnected]"),json!([false,true,false,true,false]));
}

#[tokio::test(flavor = "current_thread")]
async fn native_click_label_and_script_click_reentrancy_are_bounded() {
    let mut page=input_fixture(r#"<!doctype html><style>label{display:block;width:120px;height:30px}input{width:30px;height:30px}</style><label id="label" for="check">LABEL</label><input id="check" type="checkbox"><input id="script" type="checkbox">"#).await;
    page.js.as_mut().unwrap().execute_script("<click-reentrancy>",r#"
        const label=document.getElementById('label'),check=document.getElementById('check'),script=document.getElementById('script');
        globalThis.events=[];document.addEventListener('click',event=>events.push(event.target.id));
        check.addEventListener('click',()=>label.click());
        script.addEventListener('click',()=>script.click());
    "#).unwrap();
    page.js.as_mut().unwrap().native_click("#label").unwrap();
    assert_eq!(
        page.evaluate("[events,check.checked]"),
        json!([["label", "label", "check"], true])
    );
    page.evaluate("events.length=0");
    page.js.as_mut().unwrap().native_click("#script").unwrap();
    assert_eq!(
        page.evaluate("[events,script.checked]"),
        json!([["script", "script"], false])
    );
}

#[tokio::test(flavor = "current_thread")]
async fn native_click_refuses_label_retarget_and_new_default_action_after_callbacks() {
    let mut page=input_fixture(r#"<!doctype html><style>label{display:block;width:120px;height:30px}input,button{width:30px;height:30px}</style><label id="label" for="a">LABEL</label><input id="a" type="checkbox"><input id="b" type="checkbox"><form><button id="button" type="button">B</button></form>"#).await;
    page.js.as_mut().unwrap().execute_script("<retarget-label>",r#"
        const label=document.getElementById('label'),a=document.getElementById('a'),b=document.getElementById('b'),button=document.getElementById('button');
        a.addEventListener('focus',()=>label.htmlFor='b',{once:true});
        button.addEventListener('click',()=>button.type='submit',{once:true});
    "#).unwrap();
    assert_eq!(
        page.js
            .as_mut()
            .unwrap()
            .native_click("#label")
            .unwrap_err(),
        ("INPUT_TARGET_CHANGED", "SENT")
    );
    assert_eq!(
        page.evaluate("[a.checked,b.checked]"),
        json!([false, false])
    );
    assert_eq!(
        page.js
            .as_mut()
            .unwrap()
            .native_click("#button")
            .unwrap_err(),
        ("INPUT_TARGET_CHANGED", "SENT")
    );
    assert!(!page.js.as_ref().unwrap().has_pending_navigation());
    page.evaluate("label.htmlFor='a'");
    page.js.as_mut().unwrap().native_click("#label").unwrap();
    assert_eq!(page.evaluate("[a.checked,b.checked]"), json!([true, false]));
}
