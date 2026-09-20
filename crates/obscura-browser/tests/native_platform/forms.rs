use super::support::*;

#[tokio::test(flavor = "current_thread")]
async fn native_submit_click_uses_private_validation_events_and_navigation() {
    let mut page=input_fixture(r#"<!doctype html><form id="f" method="get" action="/wrong"><input id="field" name="field" value="OLD"></form>
      <button id="b" form="f" name="button" value="GO" formmethod="post" formaction="/native"><span id="child">SEND</span></button>"#).await;
    page.js.as_mut().unwrap().execute_script("<native-submit>",r#"
      const f=document.getElementById('f'),b=document.getElementById('b'),field=document.getElementById('field');window.log=[];
      b.addEventListener('click',e=>{log.push(['click',e.isTrusted]);field.value='CLICK'});
      f.addEventListener('submit',e=>{log.push(['submit',e.isTrusted,e.submitter===b,field.value]);field.value='SUBMIT'});
      f.addEventListener('formdata',e=>{log.push(['formdata',e.isTrusted,e.formData.get('field')]);e.formData.set('field','EVENT')});
      f.submit=f.requestSubmit=f.checkValidity=f.dispatchEvent=()=>{throw Error('PUBLIC FORM')};
      b.getAttribute=()=>{throw Error('PUBLIC ATTRIBUTE')};Object.defineProperty(b,'form',{get(){throw Error('PUBLIC OWNER')}});
      window.__obscura_native_submit_handoff=()=>{throw Error('PUBLIC HANDOFF')};
    "#).unwrap();
    assert!(
        !page
            .js
            .as_mut()
            .unwrap()
            .native_click("#child")
            .unwrap()
            .default_prevented
    );
    let nav = page
        .js
        .as_ref()
        .unwrap()
        .take_pending_navigation_request()
        .unwrap();
    assert_eq!(nav.url, "http://127.0.0.1/native");
    assert_eq!(nav.method, "POST");
    assert_eq!(nav.body, "field=EVENT&button=GO");
    assert_eq!(
        page.evaluate("log"),
        json!([
            ["click", true],
            ["submit", true, true, "CLICK"],
            ["formdata", true, "SUBMIT"]
        ])
    );
}

#[tokio::test(flavor = "current_thread")]
async fn native_submit_click_cancellation_invalid_disable_and_removal_do_not_navigate() {
    for mode in ["click", "submit", "invalid", "disabled", "remove"] {
        let mut page=input_fixture(r#"<!doctype html><form id="f" method="post" action="/send"><input id="field" name="field" value="OK"><button id="b">SEND</button></form>"#).await;
        let script = format!(
            r#"
          const mode={mode:?},f=document.getElementById('f'),b=document.getElementById('b');window.log=[];b.getAttribute=()=>{{throw Error('PUBLIC ATTRIBUTE')}};
          if(mode==='invalid'){{document.getElementById('field').value='';document.getElementById('field').setAttribute('required','')}}
          b.addEventListener('click',e=>{{log.push('click');if(mode==='click')e.preventDefault();if(mode==='disabled')b.disabled=true;if(mode==='remove')b.remove()}});
          f.addEventListener('submit',e=>{{log.push('submit');if(mode==='submit')e.preventDefault()}});
          f.addEventListener('invalid',e=>{{log.push('invalid');e.preventDefault()}},true);
          f.addEventListener('formdata',()=>log.push('WRONG DATA'));
        "#
        );
        page.js
            .as_mut()
            .unwrap()
            .execute_script("<cancel-native-submit>", &script)
            .unwrap();
        let click = page.js.as_mut().unwrap().native_click("#b").unwrap();
        assert_eq!(click.default_prevented, mode == "click");
        assert!(
            !page.js.as_ref().unwrap().has_pending_navigation(),
            "{mode}"
        );
        let expected = if mode == "submit" {
            json!(["click", "submit"])
        } else if mode == "invalid" {
            json!(["click", "invalid"])
        } else {
            json!(["click"])
        };
        assert_eq!(page.evaluate("log"), expected, "{mode}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn native_submit_label_and_coordinate_share_input_submit_default() {
    let mut page=input_fixture(r#"<!doctype html><style>#b{position:absolute;left:20px;top:120px;width:100px;height:30px}#label{display:block;width:100px;height:30px}</style>
      <label id="label" for="input">LABEL</label><form id="f" method="post" action="/send"><input id="input" type="submit" name="button" value="INPUT"></form>
      <button id="b" form="f" name="button" value="COORD">COORD</button>"#).await;
    page.js.as_mut().unwrap().execute_script("<label-submit>","window.count=0;document.getElementById('f').addEventListener('submit',()=>count++);").unwrap();
    page.js.as_mut().unwrap().native_click("#label").unwrap();
    assert_eq!(
        page.js
            .as_ref()
            .unwrap()
            .take_pending_navigation_request()
            .unwrap()
            .body,
        "button=INPUT"
    );
    page.js
        .as_mut()
        .unwrap()
        .native_pointer_click(60.0, 135.0)
        .unwrap();
    assert_eq!(
        page.js
            .as_ref()
            .unwrap()
            .take_pending_navigation_request()
            .unwrap()
            .body,
        "button=COORD"
    );
    assert_eq!(page.evaluate("count"), json!(2.0));
    assert_eq!(
        page.evaluate("typeof __obscura_native_submit_handoff"),
        json!("undefined")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn native_submit_rejects_changed_identity_and_late_unsupported_plans() {
    for (change, error) in [
        ("b.setAttribute('form','other')", "INPUT_TARGET_CHANGED"),
        ("b.type='button'", "INPUT_TARGET_CHANGED"),
        (
            "f.setAttribute('enctype','multipart/form-data')",
            "INPUT_ELEMENT_UNSUPPORTED",
        ),
    ] {
        let mut page=input_fixture(r#"<!doctype html><form id="f" action="/send" method="post"><button id="b">SEND</button></form><form id="other"></form>"#).await;
        page.js.as_mut().unwrap().execute_script("<change-submit>",&format!("const f=document.getElementById('f'),b=document.getElementById('b');b.addEventListener('click',()=>{{{change}}});")).unwrap();
        assert_eq!(
            page.js.as_mut().unwrap().native_click("#b").unwrap_err(),
            (error, "SENT")
        );
        assert!(!page.js.as_ref().unwrap().has_pending_navigation());
    }
    let mut page=input_fixture(r#"<!doctype html><form id="f" action="/send" method="post"><button id="b">SEND</button></form>"#).await;
    page.js.as_mut().unwrap().execute_script("<late-plan>","const f=document.getElementById('f');f.addEventListener('formdata',()=>f.setAttribute('target','_blank'));").unwrap();
    assert_eq!(
        page.js.as_mut().unwrap().native_click("#b").unwrap_err(),
        ("INPUT_ELEMENT_UNSUPPORTED", "SENT")
    );
    assert!(!page.js.as_ref().unwrap().has_pending_navigation());
}

#[tokio::test(flavor = "current_thread")]
async fn native_submit_rejects_extra_navigation_but_allows_cancelled_script_submit() {
    for cancel in [false, true] {
        let mut page=input_fixture(r#"<!doctype html><form id="f" action="/send" method="post"><input name="x" value="Y"><button id="b" name="button" value="GO">SEND</button></form>"#).await;
        page.js.as_mut().unwrap().execute_script("<script-submit>",&format!("const f=document.getElementById('f');document.getElementById('b').addEventListener('click',e=>{{if({cancel})e.preventDefault();f.submit()}});")).unwrap();
        let result = page.js.as_mut().unwrap().native_click("#b");
        if cancel {
            assert!(result.unwrap().default_prevented);
        } else {
            assert_eq!(result.unwrap_err(), ("UNEXPECTED_NAVIGATION", "SENT"));
        }
        assert_eq!(
            page.js
                .as_ref()
                .unwrap()
                .take_pending_navigation_request()
                .unwrap()
                .body,
            "x=Y"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn native_formdata_api_preserves_order_usv_and_live_iteration() {
    let mut page = input_fixture("<!doctype html><body>DATA</body>").await;
    page.js.as_mut().unwrap().execute_script("<formdata-api>",r#"
      const d=new FormData();d.append('x','1');d.append('y','2');d.append('x','3');d.set('x','SET');
      d.append('bad\ud800','\udc00');d.append('nil',null);
      const iterator=d.entries();window.first=iterator.next().value;d.append('late','YES');
      window.rest=Array.from(iterator);d.append('tooLate','NO');window.done=iterator.next().done;
      d.delete('x');window.all=d.getAll('y');window.missing=[d.get('x'),d.has('x'),Object.prototype.toString.call(d)];
      const visited=[];d.forEach((v,k,self)=>{visited.push([k,v,self===d]);if(k==='y')d.append('during','LOOP')});window.visited=visited;
      d._d=[['poison','BAD']];window.keys=Array.from(d.keys());
    "#).unwrap();
    assert_eq!(page.evaluate("first"), json!(["x", "SET"]));
    assert_eq!(
        page.evaluate("rest"),
        json!([["y", "2"], ["bad�", "�"], ["nil", "null"], ["late", "YES"]])
    );
    assert_eq!(
        page.evaluate("[done,all,missing]"),
        json!([true, ["2"], [null, false, "[object FormData]"]])
    );
    assert_eq!(
        page.evaluate("keys"),
        json!(["y", "bad�", "nil", "late", "tooLate", "during"])
    );
    assert_eq!(
        page.evaluate("visited[visited.length-1]"),
        json!(["during", "LOOP", true])
    );
}

#[tokio::test(flavor = "current_thread")]
async fn native_formdata_event_changes_request_and_clones_after_callback() {
    let mut page=input_fixture(r#"<!doctype html><form id="f" method="post" action="/before"><input id="x" name="x" value="OLD"></form>
      <button id="b" form="f" name="button" value="GO">GO</button>"#).await;
    page.js.as_mut().unwrap().execute_script("<formdata-event>",r#"
      const f=document.getElementById('f'),x=document.getElementById('x'),Ctor=FormDataEvent,originalJSON=JSON.stringify;window.log=[];
      f.addEventListener('submit',()=>{log.push('submit');x.value='SUBMIT'});
      f.addEventListener('formdata',e=>{
        log.push([e.isTrusted,e.bubbles,e.cancelable,e.composed,e instanceof Ctor,e.formData.get('x')]);
        window.saved=e.formData;e.preventDefault();window.cancelled=e.defaultPrevented;
        saved.set('x','EVENT');saved.append('line','A\nB');saved.append('file',new Blob(['bytes']),'name.txt');
        x.value='DOM AFTER';f.setAttribute('action','/after');
        saved._d=[['poison','BAD']];saved.entries=()=>{throw Error('PUBLIC ITERATOR')};
        globalThis.FormData=()=>{throw Error('PUBLIC CONSTRUCTOR')};globalThis.FormDataEvent=()=>{throw Error('PUBLIC EVENT')};
        JSON.stringify=()=>{throw Error('PUBLIC JSON')};Array.prototype.toJSON=()=>[['poison','BAD']];
      });
      f.requestSubmit(document.getElementById('b'));saved.set('x','TOO LATE');JSON.stringify=originalJSON;delete Array.prototype.toJSON;
    "#).unwrap();
    let nav = page
        .js
        .as_ref()
        .unwrap()
        .take_pending_navigation_request()
        .unwrap();
    assert_eq!(nav.url, "http://127.0.0.1/after");
    assert_eq!(nav.body, "x=EVENT&button=GO&line=A%0D%0AB&file=name.txt");
    assert_eq!(page.evaluate("cancelled"), json!(false));
    assert_eq!(
        page.evaluate("log"),
        json!(["submit", [true, true, false, false, true, "SUBMIT"]])
    );
}

#[tokio::test(flavor = "current_thread")]
async fn native_formdata_constructor_reentry_identity_and_detached_forms() {
    let mut page=input_fixture(r#"<!doctype html><form id="f"><input name="x" value="VALUE"><button id="b" name="button" value="GO">GO</button>
      <input id="text"></form><form id="other"><button id="foreign">OTHER</button></form>"#).await;
    page.js.as_mut().unwrap().execute_script("<formdata-constructor>",r#"
      const f=document.getElementById('f'),b=document.getElementById('b');window.errors=[];window.count=0;
      for(const value of [document.getElementById('text'),document.getElementById('foreign'),{}]){
        try{new FormData(f,value)}catch(e){errors.push(e.name)}
      }
      f.addEventListener('formdata',e=>{
        count++;window.eventData=e.formData;
        try{new FormData(f)}catch(e){errors.push(e.name)}
        f.submit();f.requestSubmit();e.formData.append('event','YES');
        new FormData(document.getElementById('other'));
      });
      f.remove();window.data=new FormData(f,b);eventData.set('x','LATE');
    "#).unwrap();
    assert_eq!(
        page.evaluate("errors"),
        json!([
            "TypeError",
            "NotFoundError",
            "TypeError",
            "InvalidStateError"
        ])
    );
    assert_eq!(
        page.evaluate("[count,data===eventData,Array.from(data)]"),
        json!([
            1,
            false,
            [["x", "VALUE"], ["button", "GO"], ["event", "YES"]]
        ])
    );
    assert!(page
        .js
        .as_ref()
        .unwrap()
        .take_pending_navigation_request()
        .is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn native_formdata_releases_locks_after_errors_and_form_removal() {
    let mut page=input_fixture(r#"<!doctype html><form id="f" action="/send" method="post"><select name="bad" id="bad"></select></form>"#).await;
    page.js.as_mut().unwrap().execute_script("<formdata-release>",r#"
      const f=document.getElementById('f');window.error='';try{new FormData(f)}catch(e){error=e.message}
      document.getElementById('bad').remove();window.count=0;
      f.addEventListener('formdata',e=>{count++;e.formData.append('kept','YES');throw Error('listener')});
      window.data=new FormData(f);f.addEventListener('formdata',()=>f.remove(),{once:true});f.submit();
      document.body.appendChild(f);f.submit();
    "#).unwrap();
    assert_eq!(page.evaluate("error"), json!("INPUT_ELEMENT_UNSUPPORTED"));
    assert_eq!(page.evaluate("[count,data.get('kept')]"), json!([3, "YES"]));
    assert_eq!(
        page.js
            .as_ref()
            .unwrap()
            .take_pending_navigation_request()
            .unwrap()
            .body,
        "kept=YES"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn native_formdata_blob_overloads_and_required_arguments() {
    let mut page = input_fixture("<!doctype html><body>FILES</body>").await;
    page.js.as_mut().unwrap().execute_script("<formdata-files>",r#"
      const d=new FormData(),f=new File(['bytes'],'original.txt');d.append('same',f);d.append('renamed',f,'renamed.txt');
      d.append('blob',new Blob(['DATA']));window.files=[d.get('same')===f,d.get('renamed').name,d.get('blob').name];
      window.errors=[];for(const call of [()=>d.append('x'),()=>d.set('x','text','filename'),()=>d.get(),()=>d.delete(),
        ()=>FormData.prototype.get.call({},'x'),()=>new FormData(null),()=>new FormDataEvent('formdata')]) {
        try{call()}catch(e){errors.push(e.name)}
      }
    "#).unwrap();
    assert_eq!(page.evaluate("files"), json!([true, "renamed.txt", "blob"]));
    assert_eq!(
        page.evaluate("errors"),
        json!([
            "TypeError",
            "TypeError",
            "TypeError",
            "TypeError",
            "TypeError",
            "TypeError",
            "TypeError"
        ])
    );
}

#[tokio::test(flavor = "current_thread")]
async fn native_validity_is_live_and_keeps_custom_errors_after_reset() {
    let mut page=input_fixture(r#"<!doctype html><form id="f"><input id="a" required><input id="disabled" required disabled>
      <input id="readonly" required readonly><datalist><input id="listed" required></datalist></form>"#).await;
    page.js
        .as_mut()
        .unwrap()
        .execute_script(
            "<validity-live>",
            r#"
      const a=document.getElementById('a');window.v=a.validity;
      window.first=[v===a.validity,v.valueMissing,v.valid,a.willValidate];
      a.value='filled';a.setCustomValidity('CUSTOM');document.getElementById('f').reset();
      window.second=[v.valueMissing,v.customError,v.valid,a.validationMessage];
      a.setCustomValidity('');a.value='good';
    "#,
        )
        .unwrap();
    assert_eq!(page.evaluate("first"), json!([true, true, false, true]));
    assert_eq!(
        page.evaluate("second"),
        json!([true, true, false, "CUSTOM"])
    );
    assert_eq!(
        page.evaluate("[v.valid,a.validationMessage]"),
        json!([true, ""])
    );
    assert_eq!(page.evaluate("['disabled','readonly','listed'].map(id=>document.getElementById(id).willValidate)"),json!([false,false,false]));
    assert_eq!(
        page.evaluate("document.getElementById('disabled').validity.valueMissing"),
        json!(true)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn native_validity_invalid_snapshot_and_submission_lock() {
    let mut page=input_fixture(r#"<!doctype html><input id="a" form="f" required><form id="f" action="/ok" method="post">
      <input id="b" required name="b"></form>"#).await;
    page.js.as_mut().unwrap().execute_script("<invalid-events>",r#"
      const f=document.getElementById('f'),a=document.getElementById('a'),b=document.getElementById('b');window.log=[];
      document.addEventListener('invalid',e=>log.push([e.target.id,e.isTrusted,e.bubbles,e.cancelable,e.composed]),true);
      f.addEventListener('invalid',()=>log.push('WRONG_BUBBLE'));
      f.addEventListener('submit',()=>log.push('submit'));
      a.addEventListener('invalid',e=>{e.preventDefault();a.value='fixed';b.value='fixed';f.requestSubmit()},{once:true});
      b.addEventListener('invalid',e=>e.preventDefault());
      a.validity.valid=true;a.checkValidity=()=>true;f.checkValidity=()=>true;
      f.dispatchEvent=a.dispatchEvent=b.dispatchEvent=()=>{throw Error('public dispatch')};
      window.Event=()=>{throw Error('public event')};f.requestSubmit();
    "#).unwrap();
    assert_eq!(
        page.evaluate("log"),
        json!([
            ["a", true, false, true, false],
            ["b", true, false, true, false]
        ])
    );
    assert!(page
        .js
        .as_ref()
        .unwrap()
        .take_pending_navigation_request()
        .is_none());
    page.js
        .as_mut()
        .unwrap()
        .execute_script("<valid-submit>", "f.requestSubmit();")
        .unwrap();
    assert_eq!(
        page.js
            .as_ref()
            .unwrap()
            .take_pending_navigation_request()
            .unwrap()
            .body,
        "b=fixed"
    );
    assert_eq!(page.evaluate("log[2]"), json!("submit"));
}

#[tokio::test(flavor = "current_thread")]
async fn native_validity_types_patterns_and_public_intrinsics() {
    let mut page=input_fixture(r#"<!doctype html><input id="email" type="email" multiple value="a@example.com,b@example.com">
      <input id="url" type="url" value="/relative"><input id="pattern" pattern="[\p{ASCII}&&\p{Letter}]+" value="ABC">
      <input id="invalid_pattern" pattern="[" value="ANY">"#).await;
    page.js.as_mut().unwrap().execute_script("<type-validation>",r#"
      const email=document.getElementById('email'),url=document.getElementById('url'),p=document.getElementById('pattern');
      window.before=[email.validity.typeMismatch,url.validity.typeMismatch,p.validity.patternMismatch,document.getElementById('invalid_pattern').validity.valid];
      const originalPrototype=RegExp.prototype;window.RegExp=()=>{throw Error('public regexp')};originalPrototype.exec=()=>{throw Error('public exec')};
      email.value='a@example.com,invalid';url.value='https://example.com';p.value='123';
    "#).unwrap();
    assert_eq!(page.evaluate("before"), json!([false, true, false, true]));
    assert_eq!(page.evaluate("[email.validity.typeMismatch,url.validity.typeMismatch,p.validity.patternMismatch]"),json!([true,false,true]));
}

#[tokio::test(flavor = "current_thread")]
async fn native_validity_lengths_distinguish_user_and_script_edits() {
    let mut page = input_fixture(
        r#"<!doctype html><input id="a" minlength=" +3tail" maxlength="4" value="START">"#,
    )
    .await;
    page.js
        .as_mut()
        .unwrap()
        .execute_script(
            "<script-length>",
            "const a=document.getElementById('a');a.value='abcdef';",
        )
        .unwrap();
    assert_eq!(
        page.evaluate("[a.validity.tooLong,a.validity.tooShort]"),
        json!([false, false])
    );
    page.js.as_mut().unwrap().native_fill("#a", "中").unwrap();
    assert_eq!(page.evaluate("a.validity.tooShort"), json!(true));
    page.js.as_mut().unwrap().native_fill("#a", "🚀中").unwrap();
    assert_eq!(page.evaluate("a.validity.tooShort"), json!(false));
    page.js
        .as_mut()
        .unwrap()
        .execute_script("<lower-max>", "a.setAttribute('maxlength','2');")
        .unwrap();
    assert_eq!(page.evaluate("a.validity.tooLong"), json!(true));
    page.js
        .as_mut()
        .unwrap()
        .execute_script("<same-script-value>", "a.value=a.value;")
        .unwrap();
    assert_eq!(page.evaluate("a.validity.tooLong"), json!(false));
}

#[tokio::test(flavor = "current_thread")]
async fn native_validity_check_report_cancel_and_focus() {
    let mut page = input_fixture(
        r#"<!doctype html><form id="f"><input id="a" required><input id="b" required></form>"#,
    )
    .await;
    page.js.as_mut().unwrap().execute_script("<check-report>",r#"
      const f=document.getElementById('f'),a=document.getElementById('a'),b=document.getElementById('b');window.log=[];
      a.addEventListener('invalid',e=>{log.push('a');e.preventDefault()});
      b.addEventListener('invalid',()=>log.push('b'));
      window.checked=f.checkValidity();window.checkFocus=document.activeElement===b;
      window.reported=f.reportValidity();window.reportFocus=document.activeElement===b;
      window.single=a.checkValidity();
    "#).unwrap();
    assert_eq!(
        page.evaluate("[checked,checkFocus,reported,reportFocus,single]"),
        json!([false, false, false, true, false])
    );
    assert_eq!(page.evaluate("log"), json!(["a", "b", "a", "b", "a"]));
}

#[tokio::test(flavor = "current_thread")]
async fn native_validity_radio_owner_and_no_validate_paths() {
    let mut page=input_fixture(r#"<!doctype html><form id="f" method="post" action="/ok"><input type="radio" id="a" name="r" required>
      <input type="radio" id="b" name="r"><button id="go" formnovalidate>GO</button></form>
      <form id="other"><input type="radio" name="r" checked></form>"#).await;
    assert_eq!(page.evaluate("[document.getElementById('a').validity.valueMissing,document.getElementById('b').validity.valueMissing]"),json!([true,true]));
    page.js.as_mut().unwrap().execute_script("<bypass>","const f=document.getElementById('f');f.requestSubmit(document.getElementById('go'));").unwrap();
    assert!(page
        .js
        .as_ref()
        .unwrap()
        .take_pending_navigation_request()
        .is_some());
    page.js
        .as_mut()
        .unwrap()
        .execute_script(
            "<native-radio>",
            "document.getElementById('b').checked=true;",
        )
        .unwrap();
    assert_eq!(
        page.evaluate("document.getElementById('a').validity.valueMissing"),
        json!(false)
    );
    page.js.as_mut().unwrap().execute_script("<novalidate>","document.getElementById('b').checked=false;f.setAttribute('novalidate','');f.requestSubmit();").unwrap();
    assert!(page
        .js
        .as_ref()
        .unwrap()
        .take_pending_navigation_request()
        .is_some());
}

#[tokio::test(flavor = "current_thread")]
async fn native_form_navigation_uses_overrides_and_protected_document_facts() {
    let mut page = input_fixture(r#"<!doctype html><base href="/base/"><form id="f" action="wrong" method="get" target="_blank">
      <input name="field" value="VALUE"></form><button id="b" form="f" name="button" value="GO"
      formaction="result?keep=1" formmethod="PoSt" formenctype="invalid" formtarget="_self">GO</button>"#).await;
    page.js
        .as_mut()
        .unwrap()
        .execute_script(
            "<form-plan>",
            r#"
      const f=document.getElementById('f'),b=document.getElementById('b');
      f.getAttribute=b.getAttribute=()=>{throw Error('public attribute')};
      window.URL=()=>{throw Error('public URL')};
      f.requestSubmit(b);
    "#,
        )
        .unwrap();
    let nav = page
        .js
        .as_ref()
        .unwrap()
        .take_pending_navigation_request()
        .unwrap();
    assert_eq!(nav.url, "http://127.0.0.1/base/result?keep=1");
    assert_eq!(nav.method, "POST");
    assert_eq!(nav.body, "field=VALUE&button=GO");
    assert_eq!(
        nav.request.referrer.unwrap().as_str(),
        "http://127.0.0.1/native-input-fixture"
    );
    assert_eq!(
        nav.request.initiator.unwrap().as_str(),
        "http://127.0.0.1/native-input-fixture"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn native_form_navigation_replaces_get_query_and_ignores_submit_arguments() {
    let mut page=input_fixture(r#"<!doctype html><base href="/base/"><form id="f" action="result?old=1" method="invalid">
      <input id="value" name="field" value="VALUE"><button id="b" name="button" value="GO" formaction="/wrong" formmethod="post">GO</button></form>"#).await;
    page.js
        .as_mut()
        .unwrap()
        .execute_script(
            "<plain-submit>",
            r#"
      const f=document.getElementById('f');window.events=0;
      f.addEventListener('submit',()=>events++);
      f._navigateSubmit=()=>{throw Error('public helper')};
      f.submit(document.getElementById('b'));
    "#,
        )
        .unwrap();
    let nav = page
        .js
        .as_ref()
        .unwrap()
        .take_pending_navigation_request()
        .unwrap();
    assert_eq!(nav.url, "http://127.0.0.1/base/result?field=VALUE");
    assert_eq!(nav.method, "GET");
    assert_eq!(nav.body, "");
    assert_eq!(page.evaluate("events"), json!(0.0));
    // Pending state.url must not become the source for a later empty action.
    page.js
        .as_mut()
        .unwrap()
        .execute_script(
            "<empty-action>",
            "f.setAttribute('action','');document.getElementById('value').remove();f.submit();",
        )
        .unwrap();
    let nav = page
        .js
        .as_ref()
        .unwrap()
        .take_pending_navigation_request()
        .unwrap();
    assert_eq!(nav.url, "http://127.0.0.1/native-input-fixture?");
    assert_eq!(
        nav.request.referrer.unwrap().as_str(),
        "http://127.0.0.1/native-input-fixture"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn native_form_navigation_rejects_unsupported_plans_atomically() {
    let mut page=input_fixture(r#"<!doctype html><form id="f" action="/good" method="post"><input name="x" value="y"></form>"#).await;
    for (attribute, value) in [
        ("target", "_blank"),
        ("target", "named"),
        ("method", "dialog"),
        ("enctype", "multipart/form-data"),
        ("enctype", "text/plain"),
        ("accept-charset", "shift_jis"),
        ("action", "javascript:alert(1)"),
        ("action", "/path#fragment"),
    ] {
        let script=format!("const f=document.getElementById('f');f.submit();f.setAttribute({},{});window.error='';try{{f.submit()}}catch(e){{error=e.message}}",
            serde_json::to_string(attribute).unwrap(),serde_json::to_string(value).unwrap());
        // Use a block to avoid redeclaring the fixture's lexical bindings.
        page.js
            .as_mut()
            .unwrap()
            .execute_script("<reject-plan>", &format!("{{{script}}}"))
            .unwrap();
        assert_eq!(
            page.evaluate("error"),
            json!("INPUT_ELEMENT_UNSUPPORTED"),
            "{attribute}={value}"
        );
        let nav = page
            .js
            .as_ref()
            .unwrap()
            .take_pending_navigation_request()
            .unwrap();
        assert_eq!(nav.url, "http://127.0.0.1/good");
        assert_eq!(nav.body, "x=y");
        page.js.as_mut().unwrap().execute_script("<restore-plan>",&format!("document.getElementById('f').removeAttribute({});document.getElementById('f').setAttribute('action','/good');document.getElementById('f').setAttribute('method','post');",serde_json::to_string(attribute).unwrap())).unwrap();
    }
    page.js
        .as_mut()
        .unwrap()
        .execute_script(
            "<disconnected-submit>",
            "const f=document.getElementById('f');f.remove();f.submit();",
        )
        .unwrap();
    assert!(page
        .js
        .as_ref()
        .unwrap()
        .take_pending_navigation_request()
        .is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn native_form_navigation_uses_base_target_and_noreferrer() {
    let mut page=input_fixture(r#"<!doctype html><base target="_blank"><form id="f" action="/result" rel="external NoReFeRrEr"><input name="x" value="y"></form>"#).await;
    page.js.as_mut().unwrap().execute_script("<base-target>","const f=document.getElementById('f');window.error='';try{f.submit()}catch(e){error=e.message}").unwrap();
    assert_eq!(page.evaluate("error"), json!("INPUT_ELEMENT_UNSUPPORTED"));
    assert!(page
        .js
        .as_ref()
        .unwrap()
        .take_pending_navigation_request()
        .is_none());
    page.js
        .as_mut()
        .unwrap()
        .execute_script(
            "<self-target>",
            "f.setAttribute('target','_TOP');f.submit();",
        )
        .unwrap();
    let nav = page
        .js
        .as_ref()
        .unwrap()
        .take_pending_navigation_request()
        .unwrap();
    assert_eq!(nav.url, "http://127.0.0.1/result?x=y");
    assert_eq!(
        nav.request.referrer_policy,
        obscura_net::ReferrerPolicy::NoReferrer
    );
}

#[tokio::test(flavor = "current_thread")]
async fn native_submit_event_is_trusted_and_ignores_public_dispatch_overrides() {
    let mut page = input_fixture(r#"<!doctype html><form id="f" action="/submit" method="post"><input id="value" name="value" value="old"></form>
      <button form="f" name="button" value="GO" id="submit">SEND</button>"#).await;
    page.js.as_mut().unwrap().execute_script("<submit-event>", r#"
      const f=document.getElementById('f'),b=document.getElementById('submit'),Ctor=SubmitEvent;
      window.log=[];window.saved=null;
      document.addEventListener('submit',e=>log.push(['capture',e.target===f,e.submitter===b]),true);
      f.addEventListener('submit',e=>{
        saved=e;log.push([e.type,e.isTrusted,e.bubbles,e.cancelable,e.composed,e.submitter===b,e instanceof Ctor]);
        document.getElementById('value').value='after';
      });
      window.addEventListener('submit',e=>log.push(['bubble',e.submitter===b]));
      Object.defineProperty(b,'form',{get(){throw Error('public form getter')}});
      f.dispatchEvent=()=>{throw Error('public dispatch')};f._navigateSubmit=()=>{throw Error('public navigation helper')};
      window.SubmitEvent=()=>{throw Error('public constructor')};window.Event=()=>{throw Error('public Event')};
      f.requestSubmit(b);
    "#).unwrap();
    assert_eq!(page.evaluate("log"),json!([
        ["capture",true,true], ["submit",true,true,true,false,true,true], ["bubble",true]
    ]));
    assert_eq!(page.evaluate("[saved.eventPhase,saved.currentTarget,saved.composedPath().length]"),json!([0,null,0]));
    let navigation=page.js.as_ref().unwrap().take_pending_navigation_request().unwrap();
    assert_eq!(navigation.body,"value=after&button=GO");
}

#[tokio::test(flavor = "current_thread")]
async fn native_submit_event_reentry_cancellation_and_other_forms() {
    let mut page=input_fixture(r#"<!doctype html><form id="f" method="post" action="/main"><input name="field" value="VALUE"></form>
      <form id="other" action="/other"></form>"#).await;
    page.js.as_mut().unwrap().execute_script("<submit-reentry>",r#"
      const f=document.getElementById('f'),other=document.getElementById('other');
      window.log=[];window.cancel=true;
      other.addEventListener('submit',e=>{log.push('other');e.preventDefault();other.requestSubmit()});
      f.addEventListener('submit',e=>{
        log.push(e.submitter);f.requestSubmit();other.requestSubmit();if(cancel)e.preventDefault();
      });
      f.requestSubmit();
    "#).unwrap();
    assert_eq!(page.evaluate("log"),json!([null,"other"]));
    assert!(page.js.as_ref().unwrap().take_pending_navigation_request().is_none());
    page.js.as_mut().unwrap().execute_script("<submit-after-cancel>","cancel=false;f.requestSubmit();").unwrap();
    assert_eq!(page.evaluate("log"),json!([null,"other",null,"other"]));
    assert_eq!(page.js.as_ref().unwrap().take_pending_navigation_request().unwrap().body,"field=VALUE");
}

#[tokio::test(flavor = "current_thread")]
async fn native_submit_event_checks_identity_and_does_not_navigate_detached_form() {
    let mut page=input_fixture(r#"<!doctype html><form id="f" action="/main"><input id="text"><button id="button" type="button">NO</button></form>
      <form id="other"><button id="foreign">OTHER</button></form>"#).await;
    page.js.as_mut().unwrap().execute_script("<submit-validation>",r#"
      const f=document.getElementById('f');window.errors=[];window.count=0;
      f.addEventListener('submit',e=>{count++;f.remove()});
      for(const id of ['text','button','foreign']) {try{f.requestSubmit(document.getElementById(id))}catch(e){errors.push(e.name)}}
      for(const value of [{},3]) {try{f.requestSubmit(value)}catch(e){errors.push(e.name)}}
      f.requestSubmit();f.requestSubmit();
    "#).unwrap();
    assert_eq!(page.evaluate("errors"),json!(["TypeError","TypeError","NotFoundError","TypeError","TypeError"]));
    assert_eq!(page.evaluate("count"),json!(1.0));
    assert!(page.js.as_ref().unwrap().take_pending_navigation_request().is_none());
    page.js.as_mut().unwrap().execute_script("<submit-reattach>","document.body.appendChild(f);f.requestSubmit();").unwrap();
    assert_eq!(page.evaluate("count"),json!(2.0));
}

#[tokio::test(flavor = "current_thread")]
async fn native_submit_event_allows_explicit_submit_and_releases_after_data_failure() {
    let mut page=input_fixture(r#"<!doctype html><form id="f" method="post" action="/main"><input name="value" value="VALUE"></form>"#).await;
    page.js.as_mut().unwrap().execute_script("<submit-explicit>",r#"
      const f=document.getElementById('f');window.count=0;
      f.addEventListener('submit',e=>{count++;e.preventDefault();f.submit()},{once:true});
      f.requestSubmit();
    "#).unwrap();
    assert_eq!(page.evaluate("count"),json!(1.0));
    assert_eq!(page.js.as_ref().unwrap().take_pending_navigation_request().unwrap().body,"value=VALUE");
    page.js.as_mut().unwrap().execute_script("<submit-bad-data>",r#"
      f.addEventListener('submit',e=>{count++;throw Error('fixture listener exception')});
      f.setAttribute('novalidate','');const unsupported=document.createElement('select');unsupported.name='bad';f.appendChild(unsupported);
      window.error='';try{f.requestSubmit()}catch(e){error=e.message}
      unsupported.remove();f.requestSubmit();
    "#).unwrap();
    assert_eq!(page.evaluate("error"),json!("INPUT_ELEMENT_UNSUPPORTED"));
    assert_eq!(page.evaluate("count"),json!(3.0));
    assert_eq!(page.js.as_ref().unwrap().take_pending_navigation_request().unwrap().body,"value=VALUE");
}

#[tokio::test(flavor = "current_thread")]
async fn native_form_entries_use_tree_order_owner_and_live_values() {
    let mut page = input_fixture(r#"<!doctype html>
        <input name="duplicate" value="before" form="f">
        <form id="f" action="/submit" method="post">
          <input name="duplicate" id="text" value="default">
          <input name="empty"><input type="password" name="password" value="PASS" dirname="ignored"><input type="hidden" name="_ChArSeT_" value="ignored">
          <input id="check" type="checkbox" name="check"><input type="checkbox" name="off">
          <input id="r1" type="radio" name="radio" value="old" checked>
          <input id="r2" type="radio" name="radio" value="new">
          <input name="read" value="readonly" readonly><input name="hidden" value="visible-data" hidden>
          <fieldset disabled><legend><input name="legend" value="first"></legend>
            <legend><input name="wrong" value="second"></legend><input name="wrong" value="disabled"></fieldset>
          <datalist><input name="wrong" value="datalist"></datalist>
          <input name="wrong" value="other" form="other">
          <textarea id="area" name="area">default</textarea>
          <button name="button" value="skip">unused</button>
          <button id="submit" name="button" value="chosen">submit</button>
          <input name="wrong" type="file" disabled><select disabled name="wrong"></select>
        </form><input name="duplicate" form="f" value="after"><form id="other"></form>"#).await;
    page.js.as_mut().unwrap().execute_script("<form-fields>", r#"
        const form=document.getElementById('f'),text=document.getElementById('text');
        text.value='中文 +%';document.getElementById('area').value='A\rB\nC\r\nD';
        document.getElementById('check').checked=true;document.getElementById('r2').checked=true;
        Object.defineProperty(text,'value',{get(){throw Error('public value')}});
        Object.defineProperty(document.getElementById('check'),'checked',{get(){throw Error('public checked')}});
        form.querySelectorAll=()=>{throw Error('public query')};
        Object.defineProperty(form,'elements',{get(){throw Error('public elements')}});
        form.requestSubmit(document.getElementById('submit'));
    "#).unwrap();
    let navigation = page
        .js
        .as_ref()
        .unwrap()
        .take_pending_navigation_request()
        .unwrap();
    assert_eq!(navigation.method, "POST");
    assert_eq!(navigation.body, "duplicate=before&duplicate=%E4%B8%AD%E6%96%87+%2B%25&empty=&password=PASS&_ChArSeT_=UTF-8&check=on&radio=new&read=readonly&hidden=visible-data&legend=first&area=A%0D%0AB%0D%0AC%0D%0AD&button=chosen&duplicate=after");
    assert!(navigation.url.ends_with("/submit"));
}

#[tokio::test(flavor = "current_thread")]
async fn native_form_entries_follow_reassociation_and_submitter() {
    let mut page = input_fixture(r#"<!doctype html><form novalidate id="f" method="post" action="/submit">
        <input id="moved" name="moved" value="old"><button name="button" value="wrong" type="button">button</button>
        <input id="submit" type="submit" name="submit" value="GO"><input type="reset" name="reset" value="no">
        <input type="image" name="image"><input type="file"><select></select>
        </form><form id="other"><input id="added" name="added" value="new"></form>"#).await;
    page.js
        .as_mut()
        .unwrap()
        .execute_script(
            "<form-reassociate>",
            r#"
        document.getElementById('moved').setAttribute('form','other');
        document.getElementById('added').setAttribute('form','f');
        document.getElementById('f').requestSubmit(document.getElementById('submit'));
    "#,
        )
        .unwrap();
    assert_eq!(
        page.js
            .as_ref()
            .unwrap()
            .take_pending_navigation_request()
            .unwrap()
            .body,
        "submit=GO&added=new"
    );
    page.js
        .as_mut()
        .unwrap()
        .execute_script(
            "<form-no-submitter>",
            "document.getElementById('f').submit();",
        )
        .unwrap();
    assert_eq!(
        page.js
            .as_ref()
            .unwrap()
            .take_pending_navigation_request()
            .unwrap()
            .body,
        "added=new"
    );
    page.js.as_mut().unwrap().execute_script("<form-invalid-submitter>", r#"
        document.getElementById('submit').setAttribute('form','other');
        window.error='';try{document.getElementById('f').requestSubmit(document.getElementById('submit'))}catch(e){error=e.message}
    "#).unwrap();
    assert_eq!(page.evaluate("error"), json!("FORM_SUBMITTER_OWNER"));
    assert!(page
        .js
        .as_ref()
        .unwrap()
        .take_pending_navigation_request()
        .is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn native_form_entries_reject_unsupported_successful_fields_without_request() {
    for control in [
        "<select name='x'><option selected>VALUE</option></select>",
        "<input type='file' name='x'>",
        "<input type='number' name='x' value='10'>",
        "<input type='date' name='x' value='2026-09-08'>",
        "<object name='x'></object>",
        "<input name='x' dirname='direction'>",
        "<textarea name='x' wrap='hard'>a b</textarea>",
    ] {
        let mut page = input_fixture("<!doctype html><form id='f' action='/submit' method='post'><input name='first' value='ok'></form>").await;
        page.js
            .as_mut()
            .unwrap()
            .execute_script(
                "<form-field>",
                &format!(
                    "document.getElementById('f').innerHTML += {};",
                    serde_json::to_string(control).unwrap()
                ),
            )
            .unwrap();
        page.js.as_mut().unwrap().execute_script("<form-unsupported>",
            "window.error='';try{document.getElementById('f').submit()}catch(e){error=e.message}").unwrap();
        assert_eq!(
            page.evaluate("error"),
            json!("INPUT_ELEMENT_UNSUPPORTED"),
            "{control}"
        );
        assert!(
            page.js
                .as_ref()
                .unwrap()
                .take_pending_navigation_request()
                .is_none(),
            "{control}"
        );
    }
    let mut page =
        input_fixture("<!doctype html><form id='f'><input id='image' type='image'></form>")
            .await;
    page.js.as_mut().unwrap().execute_script("<form-image>",
        "window.error='';try{document.getElementById('f').requestSubmit(document.getElementById('image'))}catch(e){error=e.message}").unwrap();
    assert_eq!(page.evaluate("error"), json!("INPUT_ELEMENT_UNSUPPORTED"));
    assert!(page
        .js
        .as_ref()
        .unwrap()
        .take_pending_navigation_request()
        .is_none());
}

#[test]
fn native_form_url_encoding_preserves_duplicates_and_normalizes_newlines() {
    let entries = vec![
        (
            "a\rb\nc\r\nd".into(),
            " !\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~中文🚀".into(),
        ),
        ("duplicate".into(), "one".into()),
        ("duplicate".into(), "".into()),
        ("line".into(), "\r\n\r\r\n\n".into()),
    ];
    assert_eq!(obscura_js::ops::encode_form_text(&entries),
        "a%0D%0Ab%0D%0Ac%0D%0Ad=+%21%22%23%24%25%26%27%28%29*%2B%2C-.%2F%3A%3B%3C%3D%3E%3F%40%5B%5C%5D%5E_%60%7B%7C%7D%7E%E4%B8%AD%E6%96%87%F0%9F%9A%80&duplicate=one&duplicate=&line=%0D%0A%0D%0A%0D%0A%0D%0A");
    assert_eq!(obscura_js::ops::encode_form_text(&[]), "");
}

#[tokio::test(flavor = "current_thread")]
async fn form_submission_uses_native_current_values_including_empty_textarea() {
    let mut page = input_fixture(r#"<!doctype html><form id="form" method="POST" action="/submit"><textarea id="area" name="area">DEFAULT</textarea><input id="text" name="text" value="OLD"></form>"#).await;
    page.js.as_mut().unwrap().native_fill("#area", "").unwrap();
    page.js
        .as_mut()
        .unwrap()
        .native_fill("#text", "CURRENT VALUE")
        .unwrap();
    page.js
        .as_mut()
        .unwrap()
        .execute_script(
            "<submit-values>",
            r#"
        Object.defineProperty(document.getElementById('text'),'value',{get(){return 'FORGED'}});
        document.getElementById('form').submit();
    "#,
        )
        .unwrap();
    assert_eq!(
        page.take_pending_navigation(),
        Some((
            "http://127.0.0.1/submit".into(),
            "POST".into(),
            "area=&text=CURRENT+VALUE".into()
        ))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn native_form_reset_restores_native_state_without_public_helpers() {
    let mut page=input_fixture(r#"<!doctype html><form id="form">
        <input id="text" value="TEXT"><textarea id="area">AREA</textarea>
        <input id="check" type="checkbox" checked value="CHOICE">
        <input id="r1" type="radio" name="group" checked><input id="r2" type="radio" name="group">
        <input id="hidden" type="hidden" value="TOKEN"><input id="disabled" disabled value="DISABLED">
        <input id="read" readonly value="READ"><button id="reset" type="reset" value="BUTTON">RESET</button>
        </form><input id="external" form="form" value="EXTERNAL"><form id="other"><input id="untouched" value="OTHER"></form>"#).await;
    page.js.as_mut().unwrap().execute_script("<reset-native-state>",r#"
        const form=document.getElementById('form'),text=document.getElementById('text'),area=document.getElementById('area'),check=document.getElementById('check');
        window.log=[];window.values=()=>['text','area','hidden','disabled','read','external','untouched'].map(id=>value.get.call(document.getElementById(id)));
        const value=Object.getOwnPropertyDescriptor(Element.prototype,'value');
        for(const id of ['text','area','disabled','read','external','untouched']) document.getElementById(id).value='EDIT';
        document.getElementById('hidden').value='NEW TOKEN';document.getElementById('reset').value='NEW BUTTON';
        check.checked=false;check.indeterminate=true;document.getElementById('r2').checked=true;
        text.setSelectionRange(1,2,'backward');area.setSelectionRange(1,2,'backward');
        form.addEventListener('reset',event=>log.push([event.type,event.isTrusted,event.bubbles,event.cancelable,event.composed,values()]));
        form.addEventListener('input',()=>log.push('input'));form.addEventListener('change',()=>log.push('change'));
        Object.defineProperty(form,'elements',{get(){throw Error('public elements called')}});
        Object.defineProperty(text,'value',{get(){return 'FORGED'},set(){throw Error('public value called')}});
        Object.defineProperty(check,'checked',{get(){return false},set(){throw Error('public checked called')}});
        HTMLFormElement.prototype.reset=()=>{throw Error('public reset called')};
        Element.prototype.dispatchEvent=()=>{throw Error('public dispatch called')};
        window.Event=()=>{throw Error('public constructor called')};
    "#).unwrap();
    assert!(
        !page
            .js
            .as_mut()
            .unwrap()
            .native_click("#reset")
            .unwrap()
            .default_prevented
    );
    assert_eq!(
        page.evaluate("values()"),
        json!([
            "TEXT",
            "AREA",
            "NEW TOKEN",
            "DISABLED",
            "READ",
            "EXTERNAL",
            "EDIT"
        ])
    );
    assert_eq!(
        page.evaluate("log"),
        json!([[
            "reset",
            true,
            true,
            true,
            false,
            ["EDIT", "EDIT", "NEW TOKEN", "EDIT", "EDIT", "EDIT", "EDIT"]
        ]])
    );
    assert_eq!(page.evaluate("[text.selectionStart,text.selectionEnd,area.selectionStart,area.selectionEnd,document.getElementById('reset').value]"),json!([4,4,4,4,"NEW BUTTON"]));
    page.js.as_ref().unwrap().with_dom(|dom| {
        let id = |s| dom.query_selector_all(s).unwrap()[0];
        assert!(!dom.text_control(id("#text")).unwrap().dirty);
        let check = dom.checked_state(id("#check")).unwrap();
        assert!(check.checked && check.indeterminate && !check.dirty);
        assert!(dom.checked_state(id("#r1")).unwrap().checked);
        assert!(!dom.checked_state(id("#r2")).unwrap().checked);
    });
}

#[tokio::test(flavor = "current_thread")]
async fn native_form_reset_cancellation_and_reentry_share_one_form_lock() {
    for event in ["click", "reset"] {
        let mut page=input_fixture(r#"<!doctype html><form id="form"><input id="field" value="DEFAULT"><input id="reset" type="reset"></form>"#).await;
        page.js.as_mut().unwrap().execute_script("<cancel-reset>",&format!(r#"
            const form=document.getElementById('form'),field=document.getElementById('field');window.resets=0;field.value='EDIT';
            form.addEventListener('reset',()=>resets++);form.addEventListener('{event}',e=>e.preventDefault(),{{once:true}});
        "#)).unwrap();
        let click = page.js.as_mut().unwrap().native_click("#reset").unwrap();
        assert_eq!(click.default_prevented, event == "click");
        assert_eq!(page.evaluate("field.value"), json!("EDIT"));
        assert_eq!(
            page.evaluate("resets"),
            json!(if event == "click" { 0.0 } else { 1.0 })
        );
        page.js.as_mut().unwrap().native_click("#reset").unwrap();
        assert_eq!(page.evaluate("field.value"), json!("DEFAULT"));
    }
    let mut page=input_fixture(r#"<!doctype html><form id="form"><input id="field" value="DEFAULT"><button id="reset" type="reset">RESET</button></form><form id="other"><input id="otherfield" value="OTHER"></form>"#).await;
    page.js.as_mut().unwrap().execute_script("<reset-reentry>",r#"
        const form=document.getElementById('form'),other=document.getElementById('other'),button=document.getElementById('reset');
        const original=HTMLFormElement.prototype.reset;window.log=[];
        document.getElementById('field').value='EDIT';document.getElementById('otherfield').value='EDIT';
        form.addEventListener('reset',()=>{log.push('form');original.call(form);button.click();other.reset();throw Error('page handler failed')},{once:true});
        other.addEventListener('reset',()=>{log.push('other');original.call(form)});
        form.reset=()=>{throw Error('overridden reset called')};
    "#).unwrap();
    page.js.as_mut().unwrap().native_click("#reset").unwrap();
    assert_eq!(page.evaluate("[log,document.getElementById('field').value,document.getElementById('otherfield').value]"),json!([["form","other"],"DEFAULT","OTHER"]));
    page.evaluate("(document.getElementById('field').value='AGAIN',button.click())");
    assert_eq!(
        page.evaluate("document.getElementById('field').value"),
        json!("DEFAULT")
    );
}

#[tokio::test(flavor = "current_thread")]
async fn native_form_reset_collects_after_callbacks_and_handles_detached_forms() {
    let mut page=input_fixture(r#"<!doctype html><form id="form"><input id="stay" value="OLD"><input id="move" value="MOVE"><button id="reset" type="reset">RESET</button></form><form id="other"></form><input id="external" form="other" value="EXTERNAL">"#).await;
    page.js.as_mut().unwrap().execute_script("<reset-collection>",r#"
        const form=document.getElementById('form'),stay=document.getElementById('stay'),move=document.getElementById('move'),external=document.getElementById('external');
        stay.value=move.value=external.value='EDIT';
        form.addEventListener('reset',()=>{
            stay.defaultValue='NEW DEFAULT';move.setAttribute('form','other');external.setAttribute('form','form');
            const added=document.createElement('input');added.id='added';added.defaultValue='ADDED';added.value='EDIT';form.appendChild(added);
        },{once:true});
    "#).unwrap();
    page.js.as_mut().unwrap().native_click("#reset").unwrap();
    assert_eq!(
        page.evaluate(
            "[stay.value,move.value,external.value,document.getElementById('added').value]"
        ),
        json!(["NEW DEFAULT", "EDIT", "EXTERNAL", "ADDED"])
    );
    page.js.as_mut().unwrap().execute_script("<detached-reset>",r#"
        window.log=[];form.addEventListener('reset',()=>log.push('form'));
        document.addEventListener('reset',()=>log.push('document'));window.addEventListener('reset',()=>log.push('window'));
        form.remove();stay.value=move.value='EDIT AGAIN';form.reset();
    "#).unwrap();
    assert_eq!(
        page.evaluate("[stay.value,move.value,external.value,log]"),
        json!(["NEW DEFAULT", "MOVE", "EXTERNAL", ["form"]])
    );
    assert_eq!(page.evaluate("move.form===form"), json!(true));
    page.js.as_mut().unwrap().execute_script("<remove-during-reset>",r#"
        document.body.appendChild(form);stay.value='LAST EDIT';form.addEventListener('reset',()=>form.remove(),{once:true});
    "#).unwrap();
    page.js.as_mut().unwrap().native_click("#reset").unwrap();
    assert_eq!(page.evaluate("stay.value"), json!("NEW DEFAULT"));
}

#[tokio::test(flavor = "current_thread")]
async fn native_form_reset_refuses_unsupported_controls_without_partial_reset() {
    let mut page=input_fixture(r#"<!doctype html><form id="form"><input id="field" value="DEFAULT"><select id="unsupported"><option>A</option></select><button id="reset" type="reset">RESET</button></form>"#).await;
    page.js.as_mut().unwrap().execute_script("<unsupported-reset>",r#"
        const form=document.getElementById('form'),field=document.getElementById('field'),unsupported=document.getElementById('unsupported');window.resets=0;
        field.value='EDIT';form.addEventListener('reset',()=>resets++);
    "#).unwrap();
    assert_eq!(
        page.js
            .as_mut()
            .unwrap()
            .native_click("#reset")
            .unwrap_err(),
        ("INPUT_ELEMENT_UNSUPPORTED", "NOT_SENT")
    );
    assert_eq!(page.evaluate("[resets,field.value]"), json!([0, "EDIT"]));
    page.js.as_mut().unwrap().execute_script("<unsupported-reset-callback>",r#"
        unsupported.remove();form.addEventListener('reset',()=>form.appendChild(unsupported),{once:true});
    "#).unwrap();
    assert_eq!(
        page.js
            .as_mut()
            .unwrap()
            .native_click("#reset")
            .unwrap_err(),
        ("INPUT_ELEMENT_UNSUPPORTED", "SENT")
    );
    assert_eq!(page.evaluate("[resets,field.value]"), json!([1, "EDIT"]));
    page.js
        .as_mut()
        .unwrap()
        .execute_script(
            "<unsupported-script-reset>",
            r#"
        window.failure='';try{form.reset()}catch(error){failure=error.name}
        unsupported.remove();form.reset();
    "#,
        )
        .unwrap();
    assert_eq!(
        page.evaluate("[failure,resets,field.value]"),
        json!(["NotSupportedError", 3, "DEFAULT"])
    );
    page.evaluate("field.value='AGAIN'");
    page.js.as_mut().unwrap().native_click("#reset").unwrap();
    assert_eq!(page.evaluate("field.value"), json!("DEFAULT"));
}

#[tokio::test(flavor = "current_thread")]
async fn native_form_reset_preserves_attribute_values_selection_and_user_edit_semantics() {
    let mut page=input_fixture(r#"<!doctype html><form id="form"><input id="hidden" type="hidden"><input id="submit" type="submit"><input id="reset" type="reset"><input id="button" type="button"><input id="image" type="image"><input id="check" type="checkbox"><button id="htmlbutton" value="OLD">BUTTON</button><input id="text" value="SAME"><textarea id="area">SAME</textarea></form>"#).await;
    page.js.as_mut().unwrap().execute_script("<attribute-value-reset>",r#"
        const form=document.getElementById('form');window.attributes=[];
        for(const id of ['hidden','submit','reset','button','image','check']) {
            const field=document.getElementById(id);field.value='SET';field.defaultValue='DEFAULT';_formValues[field._nid]='FORGED';
            attributes.push([field.value,field.defaultValue,field.getAttribute('value')]);
        }
        document.getElementById('htmlbutton').value='BUTTON VALUE';
        for(const id of ['text','area']) document.getElementById(id).setSelectionRange(1,3,'backward');
        form.reset();window.selection=['text','area'].map(id=>{const e=document.getElementById(id);return [e.selectionStart,e.selectionEnd,e.selectionDirection]});
    "#).unwrap();
    assert_eq!(
        page.evaluate("attributes"),
        json!(vec![vec!["DEFAULT"; 3]; 6])
    );
    assert_eq!(
        page.evaluate("selection"),
        json!([[1, 3, "backward"], [1, 3, "backward"]])
    );
    assert_eq!(page.evaluate("[document.getElementById('htmlbutton').value,document.getElementById('htmlbutton').getAttribute('value')]"),json!(["BUTTON VALUE","BUTTON VALUE"]));
    for selector in ["#text", "#area"] {
        for (default, changes) in [("SAME", 0), ("NEW", 1)] {
            let mut page=input_fixture(r#"<!doctype html><form id="form"><input id="text" value="SAME"><textarea id="area">SAME</textarea></form><input id="other">"#).await;
            page.js.as_mut().unwrap().execute_script("<reset-user-edit>",&format!(
                "const field=document.querySelector('{selector}');window.changes=0;field.addEventListener('change',()=>changes++);"
            )).unwrap();
            page.js
                .as_mut()
                .unwrap()
                .native_fill(selector, "EDIT")
                .unwrap();
            page.evaluate(&format!(
                "(field.defaultValue='{default}',document.getElementById('form').reset())"
            ));
            assert_eq!(page.evaluate("changes"), json!(0.0));
            page.js.as_mut().unwrap().native_focus("#other").unwrap();
            assert_eq!(
                page.evaluate(&format!("changes==={changes}")),
                json!(true),
                "{selector} {default}"
            );
        }
    }
}
