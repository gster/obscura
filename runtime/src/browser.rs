use obscura_browser::{lifecycle::WaitUntil, BrowserContext, Page};
use obscura_net::{
    interceptor::{InterceptAction, RequestInterceptor},
    RequestInfo,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::PathBuf,
    sync::Arc,
};
use url::Url;

use crate::protocol::{self, Request};

#[path = "manual.rs"]
mod manual;
#[path = "automation.rs"]
mod automation;
#[path = "network.rs"]
pub mod network;

type Error = (&'static str, &'static str);
fn invalid(code: &'static str) -> Error {
    (code, "NOT_SENT")
}
fn params<T: serde::de::DeserializeOwned>(request: &Request) -> Result<T, Error> {
    serde_json::from_value(request.params.clone()).map_err(|_| invalid("INVALID_PARAMS"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use obscura_net::{CookieJar, ObscuraHttpClient, Response, StealthHttpClient};

    #[tokio::test(flavor = "current_thread")]
    async fn inline_wrapper_around_block_content_has_client_geometry() {
        let mut page = input_fixture(r#"<!doctype html>
          <style>body{margin:0;font:16px/20px Arial}#container{width:240px;padding:10px}</style>
          <div id="container"><span id="wrapper"><div style="height:5px"></div><div style="height:10px"></div><div id="last" style="height:15px"></div></span></div>"#).await;
        let geometry = "(()=>{const e=document.querySelector('#wrapper');const r=e.getBoundingClientRect();return {height:r.height,width:r.width,x:r.x,y:r.y,rects:Array.from(e.getClientRects()).map(r=>({height:r.height,width:r.width}))}})()";
        assert_eq!(page.evaluate(geometry), json!({"height":30,"width":240,"x":10,"y":10,
            "rects":[{"height":0,"width":0},{"height":30,"width":240},{"height":0,"width":0}]}));
        page.evaluate("document.querySelector('#last').style.height='35px'");
        assert_eq!(page.evaluate(geometry)["height"], 50);
        page.evaluate("document.querySelector('#container').style.display='none'");
        assert_eq!(page.evaluate(geometry)["height"], 0);
        page.evaluate("document.querySelector('#container').style.display='block'");
        assert_eq!(page.evaluate(geometry)["height"], 50);
        page.evaluate("document.querySelector('#wrapper').style.display='contents'");
        assert_eq!(page.evaluate(geometry)["height"], 0);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn window_open_self_drives_navigation_from_native_submit() {
        let mut page = input_fixture(r#"<!doctype html><base href="http://127.0.0.1/booking/">
          <form id="search" novalidate><button id="submit">Search flights</button></form>"#).await;
        page.js.as_mut().unwrap().execute_script("<search-submit>", r#"
          document.querySelector('#search').addEventListener('submit', event => {
            event.preventDefault();
            window.openedSelf = window.open('flights?route=LGA-LAS', '_SeLf') === window;
          });
        "#).unwrap();
        page.js.as_mut().unwrap().native_click("#submit").unwrap();
        let navigation = page.js.as_ref().unwrap().take_pending_navigation_request()
            .expect("the submit handler must navigate the current window");
        assert_eq!(navigation.url, "http://127.0.0.1/booking/flights?route=LGA-LAS");
        assert_eq!(navigation.method, "GET");
        assert_eq!(page.evaluate("openedSelf"), json!(true));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn window_open_self_empty_url_returns_current_window() {
        let mut page = input_fixture("<!doctype html><body>current window</body>").await;
        assert_eq!(page.evaluate("window.open('', '_self') === window"), json!(true));
        assert!(!page.js.as_ref().unwrap().has_pending_navigation());
        assert_eq!(page.evaluate("(()=>{try { window.open('http://[', '_self'); return false; } catch(e) { return e.name === 'SyntaxError'; }})()"), json!(true));
        assert!(!page.js.as_ref().unwrap().has_pending_navigation());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn window_open_keeps_unsupported_targets_and_features_inactive() {
        let mut page = input_fixture("<!doctype html><body>one window</body>").await;
        assert_eq!(page.evaluate("[window.open('/new'), window.open('/new', '_blank'), window.open('/new', 'other'), window.open('/new', '_self', 'noreferrer')]"), json!([null,null,null,null]));
        assert!(!page.js.as_ref().unwrap().has_pending_navigation());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn navigator_interface_is_exposed_to_page_scripts() {
        let mut page = input_fixture("<!doctype html><body>navigator</body>").await;
        assert_eq!(page.evaluate("typeof window.Navigator"), json!("function"));
        assert_eq!(page.evaluate("navigator instanceof Navigator"), json!(true));
        assert_eq!(page.evaluate("Navigator.prototype.isPrototypeOf(navigator)"), json!(true));
        assert_eq!(page.evaluate("(()=>{try{new Navigator();return false}catch(e){return e instanceof TypeError}})()"), json!(true));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn window_name_exists_as_a_string_in_page_scope() {
        let mut page = input_fixture("<!doctype html><body>name</body>").await;
        assert_eq!(page.evaluate("typeof name"), json!("string"));
        assert_eq!(page.evaluate("[name,window.name]"), json!(["",""]));
        assert_eq!(page.evaluate("(()=>{window.name=42;return [name,window.name]})()"), json!(["42","42"]));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn script_src_reflection_does_not_reenter_page_instrumentation() {
        let mut page = input_fixture("<!doctype html><base href='https://example.com/assets/'><script id='script' type='application/json' src='app.js'></script>").await;
        page.js.as_mut().unwrap().execute_script("<instrumentation>", r#"
          const script=document.querySelector('#script');
          const original=String.prototype.charCodeAt;
          window.sourceReads=0;
          String.prototype.charCodeAt=function(index){sourceReads++;const source=script.src;return original.call(this,index)};
          window.source=script.src;
          String.prototype.charCodeAt=original;
        "#).unwrap();
        assert_eq!(page.evaluate("source"), json!("https://example.com/assets/app.js"));
        assert_eq!(page.evaluate("sourceReads"), json!(0.0));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn html_interfaces_keep_prototype_hooks_on_their_own_elements() {
        let mut page = input_fixture("<!doctype html><input id='input'><button id='button'>Search</button><script id='script' type='application/json'>{}</script>").await;
        assert_eq!(page.evaluate("HTMLScriptElement === HTMLInputElement"), json!(false));
        assert_eq!(page.evaluate("[document.querySelector('#input') instanceof HTMLInputElement, document.querySelector('#input') instanceof HTMLButtonElement, document.querySelector('#script') instanceof HTMLScriptElement, document.createElement('button') instanceof HTMLButtonElement]"), json!([true,false,true,true]));
        page.js.as_mut().unwrap().execute_script("<script-hook>", "Object.defineProperty(HTMLScriptElement.prototype,'src',{get(){return 'script hook'}})").unwrap();
        assert_eq!(page.evaluate("[document.querySelector('#script').src,document.querySelector('#input').src]"), json!(["script hook",""]));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn file_list_is_a_browser_interface_not_an_array() {
        let mut page = input_fixture("<!doctype html><input id='file' type='file'>").await;
        assert_eq!(page.evaluate("typeof FileList"), json!("function"));
        assert_eq!(page.evaluate("(()=>{const f=document.querySelector('#file').files;return [f instanceof FileList,Array.isArray(f),f.length,f.item(0),Array.from(f).length,Object.prototype.toString.call(f)]})()"), json!([true,false,0,null,0,"[object FileList]"]));
        assert_eq!(page.evaluate("(()=>{try{new FileList();return false}catch(e){return e instanceof TypeError}})()"), json!(true));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn svg_script_interface_matches_only_svg_script_nodes() {
        let mut page = input_fixture("<!doctype html><svg><script id='script' type='application/json'>{}</script><circle id='circle'/></svg>").await;
        assert_eq!(page.evaluate("typeof SVGScriptElement"), json!("function"));
        assert_eq!(page.evaluate("[document.querySelector('#script') instanceof SVGScriptElement,document.querySelector('#circle') instanceof SVGScriptElement,document.createElementNS('http://www.w3.org/2000/svg','script') instanceof SVGScriptElement]"), json!([true,false,true]));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn data_transfer_items_preserve_data_and_disable_removed_items() {
        let mut page = input_fixture("<!doctype html><body>transfer</body>").await;
        assert_eq!(page.evaluate("typeof DataTransferItem"), json!("function"));
        page.js.as_mut().unwrap().execute_script("<transfer>", r#"
          const transfer = new DataTransfer();
          const item = transfer.items.add('query', 'TEXT/PLAIN');
          window.transferBefore = [item instanceof DataTransferItem, item.kind, item.type, transfer.getData('text'), transfer.items.length];
          window.duplicate = ''; try {transfer.items.add('duplicate', 'text/plain')} catch(e) {duplicate=e.name}
          window.callbackData = []; item.getAsString(value => callbackData.push(value));
          const file = new File(['abc'], 'query.txt', {type:'text/plain'});
          transfer.items.add(file);
          window.fileResult = [transfer.files instanceof FileList, transfer.files[0].name, transfer.files.item(0).size, transfer.items[1].getAsFile() === file];
          transfer.items.remove(0);
          window.transferAfter = [item.kind, item.type, item.getAsFile(), transfer.getData('text'), transfer.items.length];
          item.getAsString(value => callbackData.push('WRONG'));
        "#).unwrap();
        assert_eq!(page.evaluate("transferBefore"), json!([true,"string","text/plain","query",1]));
        assert_eq!(page.evaluate("duplicate"), json!("NotSupportedError"));
        assert_eq!(page.evaluate("fileResult"), json!([true,"query.txt",3,true]));
        assert_eq!(page.evaluate("transferAfter"), json!(["","",null,"",1]));
        assert_eq!(page.evaluate("callbackData"), json!([]));
        page.settle(20).await;
        assert_eq!(page.evaluate("callbackData"), json!(["query"]));
    }

    #[test]
    fn macos_identity_is_inherited_by_frame() {
        let mut rt = obscura_js::runtime::ObscuraJsRuntime::new();
        rt.set_dom(obscura_dom::parse_html("<!doctype html><body>identity</body>"));
        rt.set_url("http://127.0.0.1/identity");
        rt.set_user_agent(obscura_net::StealthProfile::MacChrome152.user_agent());
        rt.set_platform("MacIntel", "macOS", "26.6.2");
        rt.set_user_agent_details(obscura_net::StealthProfile::MacChrome152.full_version(), "arm");
        rt.run_page_init();
        let script = "[navigator.userAgent,navigator.platform,JSON.stringify(navigator.userAgentData.brands)]";
        let expected = rt.evaluate(script).unwrap();
        let child = obscura_js::frame::FrameRealm::new(&mut rt, 1, 0, "http://127.0.0.1/child", "<!doctype html><body>child</body>").unwrap();
        assert_eq!(child.evaluate(&mut rt, script).unwrap(), expected);
        let high = "navigator.userAgentData.getHighEntropyValues(['architecture','uaFullVersion']).then(v=>globalThis.identityResult=[v.architecture,v.uaFullVersion])";
        child.evaluate(&mut rt, high).unwrap();
        // Frame microtasks settle when returning from the V8 call.
        assert_eq!(child.evaluate(&mut rt, "identityResult").unwrap(), json!(["arm","152.0.7977.83"]));
    }

    #[test]
    fn native_persona_seed_and_frame_identity_are_stable() {
        let persona: Persona = serde_json::from_value(json!({
            "schema_version":"1", "persona_id":"fixture_windows145", "revision":"1",
            "profile":"windows_chrome145", "viewport":{"width":640,"height":480}
        }))
        .unwrap();
        let identity = persona.device_identity();
        let mut changed = persona.clone();
        changed.revision = "2".into();
        assert_ne!(identity.seed, changed.device_identity().seed);
        let mut changed_viewport = persona.clone();
        changed_viewport.viewport.width = 800;
        assert_eq!(identity.seed, changed_viewport.device_identity().seed);
        let mut macos = persona.clone();
        macos.profile = "macos_chrome152".into();
        macos.apply_defaults();
        let macos_identity = macos.device_identity();
        assert_eq!(macos_identity.hardware_concurrency, 15);
        assert_eq!(macos_identity.device_memory, 32.0);
        assert_eq!(macos.language.as_deref(), Some("en"));
        assert_eq!(macos.languages.as_deref(), Some(&["en".into(), "zh-CN".into()][..]));
        assert_eq!(macos.accept_language.as_deref(), Some("en,zh-CN;q=0.9,zh;q=0.8"));
        assert_eq!(macos.timezone.as_deref(), Some("Asia/Shanghai"));
        // Chrome 153 on macOS sends no DNT header and reports
        // navigator.doNotTrack === null, so the macOS defaults must leave the
        // preference unset rather than fabricate one.
        assert_eq!(macos.do_not_track, None);
        assert_eq!((macos.screen_width, macos.screen_height), (Some(2560), Some(1440)));
        assert_eq!((macos.screen_avail_width, macos.screen_avail_height), (Some(2560), Some(1320)));
        assert_eq!((macos.outer_width, macos.outer_height), (Some(640), Some(480)));
        assert_eq!(macos.device_scale_factor, Some(2.0));
        assert_eq!((macos.battery_charging, macos.battery_level), (Some(true), Some(0.8)));
        assert_eq!(macos.network_rtt, Some(100));
        assert_eq!(macos.storage_quota, Some(10_738_064_711));
        assert_eq!(macos.webgl_vendor.as_deref(), Some("Google Inc. (Apple)"));
        assert!(macos.webgl_renderer.as_deref().unwrap().contains("Apple M5 Pro"));
        assert!(macos.locale_is_consistent());
        let mut custom = persona.clone();
        custom.language = Some("fr-CA".into());
        custom.languages = None;
        custom.accept_language = None;
        custom.apply_defaults();
        // A single `language` expands to the primary tag plus its base language,
        // which is what a browser reports and what `accept_language` already
        // derived. Reporting ["fr-CA"] alongside "fr-CA,fr;q=0.9" was internally
        // inconsistent.
        assert_eq!(custom.languages.as_deref(), Some(&["fr-CA".into(), "fr".into()][..]));
        assert_eq!(custom.accept_language.as_deref(), Some("fr-CA,fr;q=0.9"));
        assert!(custom.locale_is_consistent());
        custom.accept_language = Some("en-US,en;q=0.9".into());
        assert!(!custom.locale_is_consistent());
        let snapshot = "[navigator.hardwareConcurrency,navigator.deviceMemory,screen.width,screen.height,screen.availWidth,screen.availHeight]";
        for _ in 0..2 {
            let mut rt = obscura_js::runtime::ObscuraJsRuntime::new();
            rt.set_dom(obscura_dom::parse_html(
                "<!doctype html><body>PERSONA</body>",
            ));
            rt.set_url("http://127.0.0.1/persona");
            rt.set_device_identity(Some(identity.clone()));
            rt.run_page_init();
            assert_eq!(
                rt.evaluate(snapshot).unwrap(),
                json!([8, 8, 1920, 1080, 1920, 1040])
            );
            rt.evaluate("(()=>{globalThis.__obscura_hw=999;globalThis.__obscura_mem=999;globalThis.__obscura_device_identity={seed:999};return true})()").unwrap();
            assert_eq!(
                rt.evaluate(snapshot).unwrap(),
                json!([8, 8, 1920, 1080, 1920, 1040])
            );
            let child = obscura_js::frame::FrameRealm::new(
                &mut rt,
                1,
                0,
                "http://127.0.0.1/child",
                "<!doctype html><body>CHILD</body>",
            )
            .unwrap();
            assert_eq!(
                child.evaluate(&mut rt, snapshot).unwrap(),
                json!([8, 8, 1920, 1080, 1920, 1040])
            );
        }
    }

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

    struct FixtureResponse;
    #[tokio::test(flavor = "current_thread")]
    async fn chinese_fallback_paints_distinct_glyphs_in_text_and_native_controls() {
        for (html, set) in [
            ("<!doctype html><div id='v' style='font:32px sans-serif'>中</div>", "document.getElementById('v').textContent="),
            ("<!doctype html><input id='v' style='font:32px sans-serif' value='中'>", "document.getElementById('v').value="),
            ("<!doctype html><textarea id='v' style='font:32px sans-serif'>中</textarea>", "document.getElementById('v').value="),
        ] {
            let mut page = input_fixture(html).await;
            let chinese = page.screenshot(page.viewport).unwrap();
            page.js.as_mut().unwrap().execute_script("<fixture>", &format!("{set}'文'")).unwrap();
            let other = page.screenshot(page.viewport).unwrap();
            assert_ne!(chinese, other, "Chinese glyphs must not collapse to the same missing box");
            page.js.as_mut().unwrap().execute_script("<fixture>", &format!("{set}'\\u0378'")).unwrap();
            let missing = page.screenshot(page.viewport).unwrap();
            assert_ne!(chinese, missing);
            assert_ne!(other, missing);
        }
    }

    #[async_trait::async_trait]
    impl RequestInterceptor for FixtureResponse {
        async fn intercept(&self, request: &RequestInfo) -> InterceptAction {
            InterceptAction::Fulfill(Response {
                status: 200,
                url: request.url.clone(),
                headers: HashMap::new(),
                body: b"fixture".to_vec(),
                redirected_from: vec![],
                request_referrer: None,
            })
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn tracker_policy_is_independent_of_stealth_transport() {
        let url = Url::parse("https://www.google-analytics.com/g/collect").unwrap();
        assert!(obscura_net::is_tracker_blocked(url.host_str().unwrap()));
        for (blocked, status) in [(false, 200), (true, 0)] {
            let cookies = Arc::new(CookieJar::new());
            let mut policy = ObscuraHttpClient::with_full_options(cookies.clone(), None, false);
            policy.block_trackers = blocked;
            *policy.interceptor.write().await = Some(std::sync::Arc::new(FixtureResponse));
            let stealth = StealthHttpClient::with_policy(cookies, None, Arc::new(policy));
            assert_eq!(stealth.fetch(&url).await.unwrap().status, status);
            assert_eq!(
                stealth
                    .send_single("GET", &url, &HashMap::new(), b"", false, false)
                    .await
                    .unwrap()
                    .status,
                status
            );
        }
    }

    struct RequestHeaders;
    #[async_trait::async_trait]
    impl RequestInterceptor for RequestHeaders {
        async fn intercept(&self, _: &RequestInfo) -> InterceptAction {
            InterceptAction::ModifyHeaders(HashMap::from([(
                "x-fixture".into(),
                "request-only".into(),
            )]))
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn interceptor_headers_do_not_mutate_the_shared_client() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let server = std::thread::spawn(move || {
            use std::io::BufRead;
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(3)))
                .unwrap();
            let mut reader = std::io::BufReader::new(&stream);
            let mut headers = String::new();
            loop {
                let mut line = String::new();
                assert!(reader.read_line(&mut line).unwrap() > 0);
                if line == "\r\n" {
                    break;
                }
                headers.push_str(&line);
            }
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .unwrap();
            headers
        });
        let cookies = Arc::new(CookieJar::new());
        let policy = ObscuraHttpClient::with_full_options(cookies.clone(), None, true);
        *policy.interceptor.write().await = Some(std::sync::Arc::new(RequestHeaders));
        let stealth = StealthHttpClient::with_policy(cookies, None, Arc::new(policy));
        assert_eq!(stealth.fetch(&url).await.unwrap().status, 200);
        assert!(server
            .join()
            .unwrap()
            .to_lowercase()
            .contains("x-fixture: request-only"));
        assert!(stealth.extra_headers.read().await.is_empty());
    }

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
                request_referrer: None,
            })
        }
    }

    async fn input_fixture(html: &'static str) -> Page {
        let mut context = BrowserContext::with_storage_and_network(
            "native-input-test".into(),
            None,
            true,
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
        let mut decoder = png::Decoder::new(std::io::Cursor::new(png))
            .read_info()
            .unwrap();
        let mut data = vec![0; decoder.output_buffer_size().unwrap()];
        let info = decoder.next_frame(&mut data).unwrap();
        assert_eq!(info.color_type, png::ColorType::Rgba);
        let offset = (y * info.width as usize + x) * 4;
        data[offset..offset + 4].try_into().unwrap()
    }

    fn save_input_evidence(page: &Page, name: &str) {
        if let Ok(root) = std::env::var("AUTOPILOT_BROWSER_EVIDENCE_DIR") {
            let root = PathBuf::from(root);
            fs::create_dir_all(&root).unwrap();
            fs::write(root.join(name), page.screenshot((640.0, 480.0)).unwrap()).unwrap();
        }
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
        save_input_evidence(&page, "native-hover-active.png");
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

    fn fragment_landing(page: &mut Page, fragment: &str) {
        page.js
            .as_mut()
            .unwrap()
            .scroll_to_fragment(&format!("http://127.0.0.1/native-input-fixture{fragment}"))
            .unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn native_location_hash_uses_shared_history_and_skips_redundant_values() {
        let mut page = input_fixture(r#"<!doctype html><style>body{margin:0;height:2400px}#one{position:absolute;top:900px}#two{position:absolute;top:1200px}</style><input id="field" value="KEEP"><div id="one" tabindex="-1">ONE</div><div id="two" tabindex="-1">TWO</div>"#).await;
        page.js.as_mut().unwrap().execute_script("<location>",r#"
            globalThis.events=[];const P=PopStateEvent,H=HashChangeEvent;
            addEventListener('popstate',e=>events.push([e.type,e.isTrusted,e instanceof P,e.state]));
            addEventListener('hashchange',e=>events.push([e.type,e.isTrusted,e instanceof H,e.oldURL,e.newURL]));
            globalThis.URL=globalThis.DOMException=function(){throw Error('public constructor')};
            globalThis.setTimeout=globalThis.dispatchEvent=history.pushState=history.replaceState=()=>{throw Error('public helper')};
        "#).unwrap();
        assert_eq!(
            history_eval(
                &mut page,
                "(location.hash='', [location.href,history.length,events.length])"
            ),
            json!(["http://127.0.0.1/native-input-fixture", 1, 0])
        );
        assert!(page
            .js
            .as_ref()
            .unwrap()
            .take_same_document_navigation()
            .is_none());
        assert_eq!(history_eval(&mut page,"(location.hash='one', [location.hash,history.length,history.state,window.scrollY,document.activeElement.id,document.querySelector(':target').id,events])"),json!(["#one",2,null,900,"one","one",[["popstate",true,true,null]]]));
        assert!(page
            .js
            .as_ref()
            .unwrap()
            .take_pending_navigation_request()
            .is_none());
        page.js
            .as_mut()
            .unwrap()
            .run_event_loop_bounded(100)
            .await
            .unwrap();
        page.js.as_ref().unwrap().take_same_document_navigation();
        assert_eq!(
            history_eval(
                &mut page,
                "(location.hash='#one', [history.length,events.length,window.scrollY])"
            ),
            json!([2, 2, 900])
        );
        assert!(page
            .js
            .as_ref()
            .unwrap()
            .take_same_document_navigation()
            .is_none());
        assert_eq!(history_eval(&mut page,"(location.hash='', [location.href,location.hash,history.length,window.scrollY,document.querySelector(':target')===null,document.getElementById('field').value])"),json!(["http://127.0.0.1/native-input-fixture#","",3,0,true,"KEEP"]));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn native_location_replace_keeps_forward_entries_and_repeated_assign_lands() {
        let mut page = input_fixture(r#"<!doctype html><style>body{height:2400px}#one{position:absolute;top:900px}#two{position:absolute;top:1200px}</style><div id="one">ONE</div><div id="two">TWO</div>"#).await;
        history_eval(
            &mut page,
            "(()=>{location.assign('#one');location.assign('#two');history.back()})()",
        );
        page.js
            .as_mut()
            .unwrap()
            .run_event_loop_bounded(100)
            .await
            .unwrap();
        assert_eq!(
            history_eval(
                &mut page,
                "(location.replace('#replaced'),[history.length,location.hash,history.state])"
            ),
            json!([3, "#replaced", null])
        );
        history_eval(&mut page, "history.forward()");
        page.js
            .as_mut()
            .unwrap()
            .run_event_loop_bounded(100)
            .await
            .unwrap();
        assert_eq!(
            history_eval(&mut page, "[location.hash,history.length,window.scrollY]"),
            json!(["#two", 3, 1200])
        );
        history_eval(&mut page, "history.replaceState({old:true},'')");
        fragment_landing(&mut page, "#top");
        assert_eq!(
            history_eval(
                &mut page,
                "(location.assign(location.href),[history.length,history.state,window.scrollY])"
            ),
            json!([3, null, 1200])
        );
        assert!(page
            .js
            .as_ref()
            .unwrap()
            .take_pending_navigation_request()
            .is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn native_location_aliases_and_conversion_reentry_keep_native_authority() {
        for script in [
            "location.href='#one'",
            "location.assign('#one')",
            "location.replace('#one')",
            "window.location='#one'",
            "document.location='#one'",
        ] {
            let mut page = input_fixture("<!doctype html><div id=one>ONE</div>").await;
            page.js.as_mut().unwrap().execute_script("<spoof-location>","globalThis.URL=function(){throw Error('public URL')};globalThis.__virtualUrl='https://spoof.invalid/';globalThis._resolveUrl=()=>{throw Error('public resolver')};").unwrap();
            history_eval(&mut page, script);
            assert_eq!(history_eval(&mut page,"[location.href,document.URL,document.location===window.location,String(location)]"),json!(["http://127.0.0.1/native-input-fixture#one","http://127.0.0.1/native-input-fixture#one",true,"http://127.0.0.1/native-input-fixture#one"]));
            assert!(page
                .js
                .as_ref()
                .unwrap()
                .take_pending_navigation_request()
                .is_none());
        }
        let mut page = input_fixture("<!doctype html><div id=one>ONE</div>").await;
        assert_eq!(history_eval(&mut page,"(()=>{let calls=0;location.hash={toString(){calls++;history.pushState({inner:true},'','/new');return 'one'}};return [calls,location.href,history.length,history.state]})()"),json!([1,"http://127.0.0.1/new#one",3,null]));
        history_eval(&mut page, "location.assign('/pending')");
        history_eval(&mut page, "location.hash='newer'");
        assert!(page
            .js
            .as_ref()
            .unwrap()
            .take_pending_navigation_request()
            .is_none());
        assert_eq!(
            history_eval(&mut page, "location.href"),
            json!("http://127.0.0.1/new#newer")
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn native_location_components_resolve_base_and_queue_exact_full_urls() {
        let mut page =
            input_fixture("<!doctype html><base href='http://127.0.0.1/base/'><p>KEEP</p>").await;
        page.navigate("http://user:pass@[::1]:8080/dir/file?old=1#x")
            .await
            .unwrap();
        history_eval(
            &mut page,
            "(globalThis.URL=function(){throw Error('public URL')})",
        );
        assert_eq!(history_eval(&mut page,"[location.origin,location.protocol,location.host,location.hostname,location.port,location.pathname,location.search,location.hash]"),json!(["http://[::1]:8080","http:","[::1]:8080","[::1]","8080","/dir/file","?old=1","#x"]));
        for (script, url) in [
            (
                "location.pathname='/新 路'",
                "http://user:pass@[::1]:8080/%E6%96%B0%20%E8%B7%AF?old=1#x",
            ),
            (
                "location.search='?'",
                "http://user:pass@[::1]:8080/dir/file?#x",
            ),
            (
                "location.search=''",
                "http://user:pass@[::1]:8080/dir/file#x",
            ),
            (
                "location.host='example.test:9090'",
                "http://user:pass@example.test:9090/dir/file?old=1#x",
            ),
            (
                "location.hostname='example.test'",
                "http://user:pass@example.test:8080/dir/file?old=1#x",
            ),
            (
                "location.port='80tail'",
                "http://user:pass@[::1]/dir/file?old=1#x",
            ),
            (
                "location.protocol='https:'",
                "https://user:pass@[::1]:8080/dir/file?old=1#x",
            ),
            ("location.assign('next')", "http://127.0.0.1/base/next"),
            ("location.replace('')", "http://127.0.0.1/base/"),
            (
                "location.reload()",
                "http://user:pass@[::1]:8080/dir/file?old=1#x",
            ),
        ] {
            history_eval(&mut page, script);
            let pending = page
                .js
                .as_ref()
                .unwrap()
                .take_pending_navigation_request()
                .unwrap();
            assert_eq!(pending.url, url, "{script}");
            assert_eq!(pending.method, "GET");
            assert_eq!(
                history_eval(&mut page, "location.href"),
                json!("http://user:pass@[::1]:8080/dir/file?old=1#x")
            );
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn native_location_errors_preserve_url_history_and_pending_navigation() {
        let mut page = input_fixture("<!doctype html><p>KEEP</p>").await;
        assert_eq!(history_eval(&mut page,"(()=>{const E=DOMException;globalThis.DOMException=function(){throw Error('public error')};let names=[];for(const fn of [()=>location.assign('http://['),()=>{location.protocol='1bad'},()=>location.assign(),()=>location.replace.call({},'#x'),()=>{location.hash=Symbol('x')}]){try{fn()}catch(e){names.push([e.name,e instanceof E])}}return names})()"),json!([["SyntaxError",true],["SyntaxError",true],["TypeError",false],["TypeError",false],["TypeError",false]]));
        assert_eq!(history_eval(&mut page,"(()=>{const error={unique:true};try{location.href={toString(){throw error}}}catch(e){return e===error}})()"),json!(true));
        assert_eq!(
            history_eval(&mut page, "[history.length,location.href]"),
            json!([1, "http://127.0.0.1/native-input-fixture"])
        );
        assert!(page
            .js
            .as_ref()
            .unwrap()
            .take_pending_navigation_request()
            .is_none());
        assert!(page
            .js
            .as_ref()
            .unwrap()
            .take_same_document_navigation()
            .is_none());
        history_eval(&mut page, "location.assign('/queued')");
        assert_eq!(
            history_eval(
                &mut page,
                "(()=>{try{location.hash=Symbol('x')}catch(e){return e.name}})()"
            ),
            json!("TypeError")
        );
        history_eval(&mut page, "location.hash=''");
        assert_eq!(
            page.js
                .as_ref()
                .unwrap()
                .take_pending_navigation_request()
                .unwrap()
                .url,
            "http://127.0.0.1/queued"
        );
        history_eval(&mut page, "location.protocol='ftp:'");
        assert!(page
            .js
            .as_ref()
            .unwrap()
            .take_pending_navigation_request()
            .is_none());
        history_eval(&mut page, "location.protocol='javascript:'");
        assert_eq!(
            page.js
                .as_ref()
                .unwrap()
                .take_pending_navigation_request()
                .unwrap()
                .url,
            "http://127.0.0.1/native-input-fixture"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn native_fragment_link_commits_history_and_lands_without_network_navigation() {
        let mut page = input_fixture(r##"<!doctype html><style>body{margin:0;height:2400px}a{position:fixed;top:20px;left:10px;display:block;width:150px;height:30px}#target{position:absolute;top:1000px;width:200px;height:40px;background:blue}</style><input id="field" value="KEPT"><a id="link" href="#target">GO</a><div id="target" tabindex="-1"></div>"##).await;
        page.js.as_mut().unwrap().execute_script("<fragment-link>",r#"
            globalThis.log=[];const H=HashChangeEvent,P=PopStateEvent;globalThis.originalReplace=history.replaceState.bind(history);
            addEventListener('popstate',e=>log.push([e.type,e.isTrusted,e instanceof P,e.state]));
            addEventListener('hashchange',e=>log.push([e.type,e.isTrusted,e instanceof H,e.oldURL,e.newURL]));
            globalThis.HashChangeEvent=globalThis.PopStateEvent=globalThis.URL=function(){throw Error('public constructor')};
            globalThis.dispatchEvent=globalThis.setTimeout=history.pushState=history.replaceState=()=>{throw Error('public helper')};
            globalThis.__obscura_native_fragment_handoff=()=>{throw Error('public handoff')};
        "#).unwrap();
        page.js.as_mut().unwrap().native_click("#link").unwrap();
        assert_eq!(
            page.evaluate("log"),
            json!([["popstate", true, true, null]])
        );
        assert_eq!(page.evaluate("[history.length,history.state,location.hash,window.scrollY,document.activeElement.id,document.querySelector(':target').id,document.getElementById('field').value]"),json!([2,null,"#target",1000,"target","target","KEPT"]));
        assert!(page
            .js
            .as_ref()
            .unwrap()
            .take_pending_navigation_request()
            .is_none());
        page.js
            .as_mut()
            .unwrap()
            .run_event_loop_bounded(100)
            .await
            .unwrap();
        assert_eq!(
            page.evaluate("log[1]"),
            json!([
                "hashchange",
                true,
                true,
                "http://127.0.0.1/native-input-fixture",
                "http://127.0.0.1/native-input-fixture#target"
            ])
        );
        page.js.as_ref().unwrap().take_same_document_navigation();
        page.js
            .as_mut()
            .unwrap()
            .execute_script("<repeat-fragment>", "originalReplace({old:true},'')")
            .unwrap();
        page.js.as_ref().unwrap().take_same_document_navigation();
        fragment_landing(&mut page, "#top");
        page.js.as_mut().unwrap().native_click("#link").unwrap();
        page.js
            .as_mut()
            .unwrap()
            .run_event_loop_bounded(100)
            .await
            .unwrap();
        assert_eq!(
            page.evaluate("[history.length,log.length,window.scrollY]"),
            json!([2, 3, 1000])
        );
        assert_eq!(
            page.evaluate("history.state===null && log[2][0]==='popstate'"),
            json!(true)
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn native_fragment_link_cancellation_and_href_rewrite_use_native_state() {
        let mut page = input_fixture(r##"<!doctype html><base href="http://127.0.0.1/native-input-fixture"><style>body{height:2400px}a{display:block;width:100px;height:30px}#one{position:absolute;top:900px}#two{position:absolute;top:1200px}</style><a id="link" href="#one">GO</a><div id="one">ONE</div><div id="two">TWO</div>"##).await;
        page.js.as_mut().unwrap().execute_script("<fragment-cancel>","const link=document.getElementById('link');link.addEventListener('click',e=>e.preventDefault(),{once:true})").unwrap();
        assert!(
            page.js
                .as_mut()
                .unwrap()
                .native_click("#link")
                .unwrap()
                .default_prevented
        );
        assert_eq!(page.evaluate("history.length===1 && location.hash==='' && document.querySelector(':target')===null"),json!(true));
        page.js.as_mut().unwrap().execute_script("<fragment-rewrite>","link.addEventListener('click',()=>link.setAttribute('href','#two'));link.getAttribute=()=>{throw Error('public attribute')};").unwrap();
        page.js.as_mut().unwrap().native_click("#link").unwrap();
        assert_eq!(
            page.evaluate("[location.hash,document.querySelector(':target').id,window.scrollY]"),
            json!(["#two", "two", 1200])
        );
        assert!(page
            .js
            .as_ref()
            .unwrap()
            .take_pending_navigation_request()
            .is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn native_fragment_history_traversal_and_popstate_reentry_keep_shared_state() {
        let mut page = input_fixture(r##"<!doctype html><style>body{height:2400px}a{display:block;width:100px;height:30px}#one{position:absolute;top:900px}#two{position:absolute;top:1200px}</style><a id="link" href="#one">GO</a><div id="one">ONE</div><div id="two">TWO</div>"##).await;
        page.js.as_mut().unwrap().execute_script("<fragment-popstate>",r#"
            globalThis.events=[];
            addEventListener('popstate',()=>{if(history.length===2)history.pushState({newer:true},'','#two')},{once:true});
            addEventListener('hashchange',e=>events.push([e.oldURL,e.newURL]));
        "#).unwrap();
        page.js.as_mut().unwrap().native_click("#link").unwrap();
        assert_eq!(page.evaluate("[history.length,history.state,location.hash,document.querySelector(':target').id,window.scrollY]"),json!([3,{"newer":true},"#two","two",1200]));
        page.js
            .as_mut()
            .unwrap()
            .run_event_loop_bounded(100)
            .await
            .unwrap();
        assert_eq!(
            page.evaluate("events[0]"),
            json!([
                "http://127.0.0.1/native-input-fixture",
                "http://127.0.0.1/native-input-fixture#one"
            ])
        );
        page.evaluate("history.back()");
        page.js
            .as_mut()
            .unwrap()
            .run_event_loop_bounded(100)
            .await
            .unwrap();
        assert_eq!(page.evaluate("[history.length,history.state,location.hash,document.querySelector(':target').id,window.scrollY]"),json!([3,null,"#one","one",0]));
        page.evaluate("history.forward()");
        page.js
            .as_mut()
            .unwrap()
            .run_event_loop_bounded(100)
            .await
            .unwrap();
        assert_eq!(
            page.evaluate(
                "[history.state,location.hash,document.querySelector(':target').id,window.scrollY]"
            ),
            json!([{"newer":true},"#two","two",1200])
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn native_fragment_manual_history_preserves_view_but_link_still_scrolls() {
        let mut page = input_fixture(r##"<!doctype html><style>body{height:2400px}a{display:block;width:100px;height:30px}#one{position:absolute;top:900px}#two{position:absolute;top:1200px}</style><a id="link" href="#one">GO</a><div id="one">ONE</div><div id="two">TWO</div>"##).await;
        page.js.as_mut().unwrap().execute_script("<manual-history>","history.scrollRestoration='manual';history.pushState(null,'','#one');history.pushState(null,'','#two');window.scrollTo(0,500);").unwrap();
        page.evaluate("history.back()");
        page.js
            .as_mut()
            .unwrap()
            .run_event_loop_bounded(100)
            .await
            .unwrap();
        assert_eq!(page.evaluate("[location.hash,document.querySelector(':target').id,window.scrollY,history.scrollRestoration]"),json!(["#one","one",500,"manual"]));
        page.js.as_ref().unwrap().take_same_document_navigation();
        page.js.as_mut().unwrap().native_click("#link").unwrap();
        assert_eq!(
            page.evaluate("[window.scrollY,history.length,history.state]"),
            json!([900, 3, null])
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn native_fragment_selection_obeys_raw_decoded_and_tree_order() {
        let mut page = input_fixture(r#"<!doctype html><body>
            <a name="same" data-mark="name"></a><div id="same" data-mark="first"></div><div id="same" data-mark="second"></div>
            <a name="%E4%B8%AD" data-mark="raw"></a><div id="中" data-mark="decoded"></div>
            <div id="a+b" data-mark="plus"></div><div id="bad%zz" data-mark="bad"></div>
            <div id="�" data-mark="replacement"></div><div id="﻿x" data-mark="bom"></div>
            <div id="TOP" data-mark="top-id"></div><div id="host"></div>
            </body>"#).await;
        page.js.as_mut().unwrap().execute_script("<fragment-selection>", r#"
            document.getElementById('host').attachShadow({mode:'open'}).innerHTML='<div id="shadow"></div>';
            const detached=document.createElement('div');detached.id='detached';
            globalThis.beforeURL=document.URL;
            document.getElementById=()=>{throw Error('public lookup')};
            globalThis.decodeURIComponent=globalThis.URL=()=>{throw Error('public decode')};
        "#).unwrap();
        for (fragment, expected) in [
            ("#same", json!("first")),
            ("#%E4%B8%AD", json!("raw")),
            ("#a+b", json!("plus")),
            ("#bad%zz", json!("bad")),
            ("#%FF", json!("replacement")),
            ("#%EF%BB%BFx", json!("bom")),
            ("#TOP", json!("top-id")),
            ("#shadow", Value::Null),
            ("#detached", Value::Null),
            ("#missing", Value::Null),
            ("", Value::Null),
        ] {
            fragment_landing(&mut page, fragment);
            assert_eq!(
                page.evaluate(
                    "document.querySelector(':target')?.getAttribute('data-mark') ?? null"
                ),
                expected,
                "{fragment}"
            );
        }
        page.evaluate("document.querySelector('[data-mark=raw]').remove()");
        fragment_landing(&mut page, "#%E4%B8%AD");
        assert_eq!(
            page.evaluate("document.querySelector(':target').getAttribute('data-mark')"),
            json!("decoded")
        );
        assert_eq!(
            page.evaluate("document.URL===beforeURL && history.length===1"),
            json!(true)
        );
        assert!(!page.js.as_ref().unwrap().has_pending_navigation());
        page.js
            .as_mut()
            .unwrap()
            .scroll_to_fragment("http://127.0.0.1/other#same")
            .unwrap();
        assert_eq!(
            page.evaluate("document.querySelector(':target')===null"),
            json!(true)
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn native_fragment_target_survives_id_change_and_reorders_without_aliasing() {
        let mut page = input_fixture(r#"<!doctype html><body><div id="same" data-mark="first"></div><div id="same" data-mark="second"></div></body>"#).await;
        fragment_landing(&mut page, "#same");
        page.evaluate("document.querySelector(':target').id='changed'");
        assert_eq!(
            page.evaluate("document.querySelector(':target').id"),
            json!("changed")
        );
        page.evaluate("history.replaceState(null,'','#same')");
        assert_eq!(
            page.evaluate("document.querySelector(':target').id"),
            json!("changed")
        );
        page.evaluate("(()=>{const first=document.querySelector('[data-mark=first]');first.id='same';document.body.appendChild(first)})()");
        fragment_landing(&mut page, "#same");
        assert_eq!(
            page.evaluate("document.querySelector(':target').getAttribute('data-mark')"),
            json!("second")
        );
        page.js.as_ref().unwrap().with_dom(|dom| {
            let target = dom.target_element().unwrap();
            dom.remove(target);
            assert_eq!(dom.target_element(), None);
        });
        page.evaluate("(()=>{const node=document.createElement('div');node.id='same';document.body.appendChild(node)})()");
        assert_eq!(
            page.evaluate("document.querySelector(':target')===null"),
            json!(true)
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn native_fragment_target_changes_real_layout_pixels_and_focus() {
        let mut page = input_fixture(r#"<!doctype html><style>
            body{margin:0;height:2600px}#target{position:absolute;left:40px;top:900px;width:200px;height:60px;background:red}
            #target:target{top:1200px;background:blue}
            </style><input id="source"><div id="target" tabindex="-1"></div>"#).await;
        page.js.as_mut().unwrap().native_focus("#source").unwrap();
        page.js.as_mut().unwrap().execute_script("<fragment-events>", r#"
            globalThis.events=[];
            for(const id of ['source','target']) for(const kind of ['blur','focus'])
                document.getElementById(id).addEventListener(kind,e=>events.push([id,kind,e.isTrusted]));
            document.addEventListener('scroll',e=>events.push(['document','scroll',e.isTrusted]));
            Element.prototype.scrollIntoView=Element.prototype.scrollTo=globalThis.scrollTo=()=>{throw Error('public scroll')};
            Element.prototype.getBoundingClientRect=HTMLElement.prototype.focus=()=>{throw Error('public geometry/focus')};
        "#).unwrap();
        fragment_landing(&mut page, "#target");
        assert_eq!(
            page.evaluate("[window.scrollX,window.scrollY,document.activeElement.id]"),
            json!([0, 1200, "target"])
        );
        assert_eq!(
            page.evaluate("events"),
            json!([
                ["source", "blur", true],
                ["target", "focus", true],
                ["document", "scroll", true]
            ])
        );
        assert_eq!(pixel(&page, 60, 20), [0, 0, 255, 255]);
        save_input_evidence(&page, "native-fragment-target.png");
        page.evaluate("events.length=0");
        fragment_landing(&mut page, "#target");
        assert_eq!(page.evaluate("events.length===0"), json!(true));
        fragment_landing(&mut page, "#missing");
        assert_eq!(
            page.evaluate(
                "[window.scrollY,document.activeElement.id,document.querySelector(':target')]"
            ),
            json!([1200, "target", null])
        );
        fragment_landing(&mut page, "#%74Op");
        assert_eq!(
            page.evaluate("[window.scrollY,document.activeElement.id]"),
            json!([0, "target"])
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn native_fragment_nested_scroll_aligns_start_and_nearest() {
        let mut page = input_fixture(r#"<!doctype html><style>
            body{margin:0;height:2600px}
            #outer{position:absolute;left:40px;top:900px;width:300px;height:150px;overflow:auto}
            #inner{position:relative;left:400px;top:300px;width:200px;height:100px;overflow:auto}
            #target{position:relative;left:300px;top:250px;width:100px;height:30px;background:blue}
            .space{width:900px;height:900px}
            </style><div id="outer"><div id="inner"><div id="target"></div><div class="space"></div></div><div class="space"></div></div>"#).await;
        fragment_landing(&mut page, "#target");
        assert_eq!(page.evaluate("[document.getElementById('inner').scrollLeft,document.getElementById('inner').scrollTop,document.getElementById('outer').scrollLeft,document.getElementById('outer').scrollTop,window.scrollX,window.scrollY]"),json!([200,250,300,300,0,900]));
        assert_eq!(pixel(&page, 245, 10), [0, 0, 255, 255]);
        fragment_landing(&mut page, "#");
        assert_eq!(page.evaluate("[window.scrollY,document.getElementById('inner').scrollTop,document.getElementById('outer').scrollTop]"),json!([0,250,300]));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn native_fragment_nearest_axis_handles_oversized_and_transparent_boxes() {
        let mut page = input_fixture(r#"<!doctype html><style>body{margin:0;width:3000px;height:3000px}#target{position:absolute;top:1000px;height:30px;opacity:0}</style><div id="target"></div>"#).await;
        for (left, width, initial, expected) in [
            (900, 100, 0, 360),
            (100, 100, 300, 100),
            (100, 900, 300, 300),
            (900, 900, 0, 900),
            (100, 900, 1100, 360),
            (100, 100, 0, 0),
        ] {
            page.js.as_mut().unwrap().execute_script("<fragment-axis>",&format!(
                "document.getElementById('target').style.left='{left}px';document.getElementById('target').style.width='{width}px';window.scrollTo({initial},0);"
            )).unwrap();
            fragment_landing(&mut page, "#target");
            assert_eq!(
                page.evaluate("[window.scrollX,window.scrollY]"),
                json!([expected, 1000]),
                "left={left} width={width} initial={initial}"
            );
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn native_fragment_nonfocusable_target_uses_viewport_and_preserves_reentry() {
        let mut page = input_fixture(r#"<!doctype html><style>body{height:2600px}#plain{position:absolute;top:1000px}#hidden{display:none}#focusable{width:100px;height:30px}</style><input id="source"><input id="other"><div id="plain">PLAIN</div><div id="hidden" tabindex="0"></div><div id="focusable" tabindex="0"></div>"#).await;
        page.js.as_mut().unwrap().native_focus("#source").unwrap();
        fragment_landing(&mut page, "#plain");
        assert_eq!(page.evaluate("document.activeElement===document.body && document.querySelector(':target').id==='plain'"),json!(true));
        fragment_landing(&mut page, "#");
        page.js.as_mut().unwrap().native_focus("#source").unwrap();
        fragment_landing(&mut page, "#hidden");
        assert_eq!(page.evaluate("document.activeElement===document.body && document.querySelector(':target').id==='hidden'"),json!(true));
        page.js.as_mut().unwrap().execute_script("<fragment-reentry>","document.getElementById('focusable').addEventListener('focus',()=>document.getElementById('other').focus())").unwrap();
        fragment_landing(&mut page, "#focusable");
        assert_eq!(page.evaluate("document.activeElement.id"), json!("other"));
    }

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
        save_input_evidence(&page, "native-focus.png");
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
        save_input_evidence(&page, "native-fill-scrolled.png");
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
        save_input_evidence(&page, "native-text-values.png");
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

    fn history_eval(page: &mut Page, script: &str) -> serde_json::Value {
        let expression = format!("(() => {{try {{return {{ok:true,value:({script})}}}} catch(error) {{return {{ok:false,name:error.name,message:error.message,stack:error.stack}}}}}})()");
        let result = page.js.as_mut().unwrap().evaluate(&expression).unwrap();
        assert_eq!(result["ok"], json!(true), "{result}");
        result
            .get("value")
            .cloned()
            .unwrap_or(serde_json::Value::Null)
    }

    struct HistoryRedirectFixture {
        enabled: Arc<std::sync::atomic::AtomicBool>,
        destination: &'static str,
    }
    #[async_trait::async_trait]
    impl RequestInterceptor for HistoryRedirectFixture {
        async fn intercept(&self, request: &RequestInfo) -> InterceptAction {
            let redirected = self.enabled.load(std::sync::atomic::Ordering::SeqCst)
                && request.url.path() == "/a"
                && request.url.query() == Some("one");
            InterceptAction::Fulfill(Response {
                status: 200,
                url: if redirected {
                    Url::parse(self.destination).unwrap()
                } else {
                    request.url.clone()
                },
                headers: HashMap::from([("content-type".into(), "text/html".into())]),
                body: b"<!doctype html><script>window.initial=history.state</script>".to_vec(),
                redirected_from: if redirected {
                    vec![request.url.clone()]
                } else {
                    vec![]
                },
                request_referrer: None,
            })
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn history_redirect_clears_state_and_separates_sibling_documents() {
        for (destination, reload) in [
            "http://127.0.0.1/changed",
            "http://changed.test/landing",
            "http://127.0.0.1/a?one",
        ]
        .into_iter()
        .flat_map(|url| [(url, false), (url, true)])
        {
            let enabled = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let mut context = BrowserContext::with_storage_and_network(
                "history-redirect".into(),
                None,
                true,
                None,
                None,
                true,
            );
            let client = Arc::get_mut(&mut context.http_client).unwrap();
            client.block_trackers = false;
            *client.interceptor.write().await = Some(std::sync::Arc::new(HistoryRedirectFixture {
                enabled: enabled.clone(),
                destination,
            }));
            let mut page = Page::new("history-redirect".into(), Arc::new(context));
            page.navigate("http://127.0.0.1/a").await.unwrap();
            history_eval(&mut page,"(history.replaceState({sibling:'PRIVATE'},''),history.pushState({redirected:'PRIVATE'},'','?one'))");
            if !reload {
                page.navigate("http://127.0.0.1/b").await.unwrap();
            }
            enabled.store(true, std::sync::atomic::Ordering::SeqCst);
            session_traverse(
                &mut page,
                if reload {
                    "location.reload()"
                } else {
                    "history.back()"
                },
            )
            .await;
            assert_eq!(
                history_eval(
                    &mut page,
                    "[location.href,history.length,initial,history.state]"
                ),
                json!([destination, if reload { 2 } else { 3 }, null, null])
            );
            history_eval(&mut page, "(window.redirectedRealm=true)");
            session_traverse(&mut page, "history.back()").await;
            assert_eq!(
                history_eval(&mut page, "[location.href,initial,typeof redirectedRealm]"),
                json!(["http://127.0.0.1/a",{"sibling":"PRIVATE"},"undefined"])
            );
            history_eval(&mut page, "(window.siblingRealm=true)");
            session_traverse(&mut page, "history.forward()").await;
            assert_eq!(
                history_eval(&mut page, "[location.href,initial,typeof siblingRealm]"),
                json!([destination, null, "undefined"])
            );
        }
    }

    struct HistoryResponseGate(std::sync::Arc<std::sync::atomic::AtomicU16>);
    #[async_trait::async_trait]
    impl RequestInterceptor for HistoryResponseGate {
        async fn intercept(&self, request: &RequestInfo) -> InterceptAction {
            let status = if request.url.path() == "/a" {
                self.0.load(std::sync::atomic::Ordering::SeqCst)
            } else {
                200
            };
            if status == 0 {
                return InterceptAction::Block;
            }
            InterceptAction::Fulfill(Response {
                status,
                url: request.url.clone(),
                headers: HashMap::from([("content-type".into(), "text/html".into())]),
                body: b"<!doctype html><script>window.initial=history.state</script>".to_vec(),
                redirected_from: vec![],
                request_referrer: None,
            })
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn session_history_failed_and_no_content_traversals_do_not_move_cursor() {
        for failure in [0, 204, 205] {
            let gate = std::sync::Arc::new(std::sync::atomic::AtomicU16::new(200));
            let mut context = BrowserContext::with_storage_and_network(
                "history-failure".into(),
                None,
                true,
                None,
                None,
                true,
            );
            let client = Arc::get_mut(&mut context.http_client).unwrap();
            client.block_trackers = false;
            *client.interceptor.write().await = Some(std::sync::Arc::new(HistoryResponseGate(gate.clone())));
            let mut page = Page::new("history-failure".into(), Arc::new(context));
            page.navigate("http://127.0.0.1/a").await.unwrap();
            history_eval(&mut page, "history.replaceState({name:'A'},'')");
            page.navigate("http://127.0.0.1/b").await.unwrap();
            history_eval(
                &mut page,
                "(history.replaceState({name:'B'},''),window.marker=true)",
            );
            gate.store(failure, std::sync::atomic::Ordering::SeqCst);
            history_eval(&mut page, "history.back()");
            page.js
                .as_mut()
                .unwrap()
                .run_event_loop_bounded(100)
                .await
                .unwrap();
            let result = page.process_pending_navigation().await;
            assert_eq!(result.is_err(), failure == 0, "{result:?}");
            assert_eq!(
                history_eval(
                    &mut page,
                    "[location.pathname,history.state,history.length,marker]"
                ),
                json!(["/b",{"name":"B"},2,true])
            );
            assert_eq!(page.url_string(), "http://127.0.0.1/b");
            assert_eq!(page.js.as_ref().unwrap().document_url(), "http://127.0.0.1/b");
            gate.store(200, std::sync::atomic::Ordering::SeqCst);
            session_traverse(&mut page, "history.back()").await;
            assert_eq!(
                history_eval(&mut page, "[location.pathname,initial,typeof marker]"),
                json!(["/a",{"name":"A"},"undefined"])
            );
        }
    }

    async fn session_traverse(page: &mut Page, script: &str) {
        history_eval(page, script);
        page.js
            .as_mut()
            .unwrap()
            .run_event_loop_bounded(100)
            .await
            .unwrap();
        page.process_pending_navigation().await.unwrap();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn session_history_recreates_documents_with_state_before_authors() {
        let mut page=input_fixture(r#"<!doctype html><script>
          window.initial=[location.pathname,history.length,history.state];window.events=[];
          addEventListener('popstate',()=>events.push('pop'));addEventListener('hashchange',()=>events.push('hash'));
          addEventListener('pageshow',e=>events.push(['show',e.persisted,e.isTrusted]));
        </script>"#).await;
        history_eval(
            &mut page,
            "(history.replaceState({name:'A'},''),window.oldRealm=true)",
        );
        page.navigate("http://127.0.0.1/b").await.unwrap();
        history_eval(&mut page, "history.replaceState({name:'B'},'')");
        session_traverse(&mut page, "history.back()").await;
        assert_eq!(
            history_eval(
                &mut page,
                "[initial,history.state===history.state,typeof oldRealm,events]"
            ),
            json!([["/native-input-fixture",2,{"name":"A"}],true,"undefined",[["show",false,true]]])
        );
        session_traverse(&mut page, "history.forward()").await;
        assert_eq!(
            history_eval(&mut page, "[initial,events]"),
            json!([["/b",2,{"name":"B"}],[["show",false,true]]])
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn session_history_reload_preserves_stored_clone_and_entry_count() {
        let mut page =
            input_fixture("<!doctype html><script>window.initial=history.state</script>").await;
        history_eval(
            &mut page,
            "(()=>{const x={n:1};x.self=x;history.replaceState(x,'');history.state.n=99})()",
        );
        for reload in ["location.reload()", "history.go(0)"] {
            session_traverse(&mut page, reload).await;
            assert_eq!(
                history_eval(
                    &mut page,
                    "[history.length,initial.n,initial.self===initial,initial===history.state]"
                ),
                json!([1, 1, true, true])
            );
            history_eval(&mut page, "(history.state.n=99)");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn session_history_replace_and_new_navigation_truncate_forward_entries() {
        let mut page = input_fixture("<!doctype html><body>NEW</body>").await;
        page.navigate("http://127.0.0.1/b").await.unwrap();
        session_traverse(&mut page, "location.replace('/c')").await;
        assert_eq!(
            history_eval(
                &mut page,
                "[location.pathname,history.length,history.state]"
            ),
            json!(["/c", 2, null])
        );
        session_traverse(&mut page, "history.back()").await;
        page.navigate("http://127.0.0.1/d").await.unwrap();
        session_traverse(&mut page, "history.forward()").await;
        assert_eq!(
            history_eval(&mut page, "[location.pathname,history.length]"),
            json!(["/d", 2])
        );
        assert_eq!(
            page.history,
            vec![
                "http://127.0.0.1/native-input-fixture",
                "http://127.0.0.1/d"
            ]
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn session_history_same_url_navigation_still_creates_a_distinct_document() {
        let mut page =
            input_fixture("<!doctype html><script>window.initial=history.state</script>").await;
        history_eval(&mut page, "history.replaceState({first:true},'')");
        page.navigate("http://127.0.0.1/native-input-fixture")
            .await
            .unwrap();
        assert_eq!(
            history_eval(&mut page, "[initial,history.length]"),
            json!([null, 2])
        );
        history_eval(&mut page, "(window.marker=true)");
        session_traverse(&mut page, "history.back()").await;
        assert_eq!(
            history_eval(&mut page, "[initial,typeof marker,history.length]"),
            json!([{"first":true},"undefined",2])
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn session_history_cross_origin_revisit_restores_only_destination_state() {
        let mut page =
            input_fixture("<!doctype html><script>window.initial=history.state</script>").await;
        history_eval(&mut page, "history.replaceState({origin:'a'},'')");
        page.navigate("http://example.test/b").await.unwrap();
        assert_eq!(
            history_eval(&mut page, "[history.state,history.length]"),
            json!([null, 2])
        );
        history_eval(&mut page, "history.replaceState({origin:'b'},'')");
        session_traverse(&mut page, "history.back()").await;
        assert_eq!(
            history_eval(&mut page, "[location.origin,initial]"),
            json!(["http://127.0.0.1",{"origin":"a"}])
        );
        session_traverse(&mut page, "history.forward()").await;
        assert_eq!(
            history_eval(&mut page, "[location.origin,initial]"),
            json!(["http://example.test",{"origin":"b"}])
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn session_history_recreated_document_keeps_same_document_entry_group() {
        let mut page=input_fixture("<!doctype html><script>window.pops=[];addEventListener('popstate',e=>pops.push(e.state))</script>").await;
        history_eval(
            &mut page,
            "(history.replaceState({n:0},''),history.pushState({n:1},'','?one'))",
        );
        page.navigate("http://127.0.0.1/b").await.unwrap();
        session_traverse(&mut page, "history.back()").await;
        history_eval(&mut page, "(window.marker=true)");
        session_traverse(&mut page, "history.back()").await;
        assert_eq!(
            history_eval(
                &mut page,
                "[marker,history.state,pops,location.search,history.length]"
            ),
            json!([true,{"n":0},[{"n":0}],"",3])
        );
        session_traverse(&mut page, "history.forward()").await;
        assert_eq!(
            history_eval(&mut page, "[marker,history.state,pops]"),
            json!([true,{"n":1},[{"n":0},{"n":1}]])
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn session_history_new_realm_restores_viewport_without_reusing_node_positions() {
        for manual in [false, true] {
            let mut page=input_fixture(r#"<!doctype html><style>body{margin:0;height:2500px}#box{height:100px;width:100px;overflow:auto}#content{height:600px}</style><div id="box"><div id="content"></div></div><script>window.initial=[scrollY,document.getElementById('box').scrollTop]</script>"#).await;
            history_eval(
                &mut page,
                "(scrollTo(0,700),document.getElementById('box').scrollTop=200)",
            );
            if manual {
                history_eval(&mut page, "(history.scrollRestoration='manual')");
            }
            page.navigate("http://127.0.0.1/b").await.unwrap();
            session_traverse(&mut page, "history.back()").await;
            assert_eq!(
                history_eval(
                    &mut page,
                    "[initial,scrollY,document.getElementById('box').scrollTop]"
                ),
                json!([
                    [if manual { 0 } else { 700 }, 0],
                    if manual { 0 } else { 700 },
                    0
                ])
            );
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn session_history_post_revisits_fail_without_scheduling_a_request() {
        let mut page = input_fixture("<!doctype html><body>POST</body>").await;
        page.navigate_with_wait_post("http://127.0.0.1/post", WaitUntil::Load, "POST", "value=1")
            .await
            .unwrap();
        assert_eq!(history_eval(&mut page,"(()=>{try{location.reload();return 'scheduled'}catch(e){return [e.name,e.message]}})()"),json!(["NotSupportedError","HISTORY_POST_REQUIRES_AUTHORIZATION"]));
        assert!(page
            .js
            .as_ref()
            .unwrap()
            .take_pending_navigation_request()
            .is_none());
        page.navigate("http://127.0.0.1/b").await.unwrap();
        history_eval(&mut page, "history.back()");
        let _ = page.js.as_mut().unwrap().run_event_loop_bounded(100).await;
        assert!(!page.process_pending_navigation().await.unwrap());
        assert_eq!(
            history_eval(&mut page, "[location.pathname,history.length]"),
            json!(["/b", 3])
        );
        // CDP cannot bypass the same POST gate through an entry index.
        page.set_history_index(1);
        assert!(page
            .navigate("http://127.0.0.1/post")
            .await
            .unwrap_err()
            .to_string()
            .contains("HISTORY_POST_REQUIRES_AUTHORIZATION"));
        assert_eq!(page.url_string(), "http://127.0.0.1/b");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn native_pageshow_follows_load_microtasks_and_dispatches_once() {
        let mut page=input_fixture(r#"<!doctype html><script>
        window.events=[];window.shows=0;const OriginalPageTransitionEvent=PageTransitionEvent;
        const removed=()=>events.push('removed');window.addEventListener('pageshow',removed);
        window.addEventListener('load',()=>{events.push('load');window.removeEventListener('pageshow',removed);queueMicrotask(()=>events.push('microtask'))});
        document.addEventListener('pageshow',()=>events.push('document'));
        window.addEventListener('pageshow',e=>{events.push(['capture',e.eventPhase,e.composedPath().length]);e.preventDefault()},true);
        window.addEventListener('pageshow',e=>{events.push(['once',e.defaultPrevented]);throw Error('listener')},{once:true});
        window.onpageshow=e=>{shows++;events.push(['show',e instanceof OriginalPageTransitionEvent,e.target===document,e.currentTarget===window,e.eventPhase,e.bubbles,e.cancelable,e.composed,e.persisted,e.isTrusted,document.readyState]);queueMicrotask(()=>events.push('show-microtask'))};
        window.PageTransitionEvent=window.Event=window.dispatchEvent=document.dispatchEvent=()=>{throw Error('public override')};
        </script>"#).await;
        let expected = json!([
            "load",
            "microtask",
            ["capture", 2, 1],
            ["once", true],
            ["show", true, true, true, 2, true, true, false, false, true, "complete"],
            "show-microtask"
        ]);
        assert_eq!(history_eval(&mut page, "events"), expected);
        for phase in [1, 2, 3, 4, 4] {
            page.js.as_mut().unwrap().document_lifecycle(phase).unwrap();
        }
        assert_eq!(history_eval(&mut page, "events"), expected);
        assert_eq!(
            history_eval(
                &mut page,
                "[shows,typeof __obscura_native_lifecycle_handoff]"
            ),
            json!([1, "undefined"])
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn native_pageshow_constructor_is_readonly_and_script_events_untrusted() {
        let mut page = input_fixture("<!doctype html><body>EVENT</body>").await;
        assert_eq!(
            history_eval(
                &mut page,
                r#"(() => {
            const event=new PageTransitionEvent('pageshow',{persisted:1});
            const empty=new PageTransitionEvent('pagehide');
            const nullInit=new PageTransitionEvent('pageshow',null);
            let received;window.addEventListener('pageshow',e=>received=[e.persisted,e.isTrusted]);
            event.persisted=false;window.dispatchEvent(event);
            let invalid=false;try{Object.getOwnPropertyDescriptor(PageTransitionEvent.prototype,'persisted').get.call({})}catch(e){invalid=e instanceof TypeError}
            return [event.persisted,empty.persisted,nullInit.persisted,event.bubbles,event.cancelable,
              event instanceof Event,Object.prototype.toString.call(event),received,invalid];
        })()"#
            ),
            json!([
                true,
                false,
                false,
                false,
                false,
                true,
                "[object PageTransitionEvent]",
                [true, false],
                true
            ])
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn document_lifecycle_uses_one_native_path_and_readiness() {
        let mut page=input_fixture(r#"<!doctype html><script>
        window.events=[['script',document.readyState]];window.loads=0;
        document.addEventListener('readystatechange',e=>events.push(['ready',document.readyState,e.isTrusted,e.target===document,e.bubbles]));
        for(const [owner,label,capture] of [[window,'capture',true],[document,'document',false],[window,'bubble',false]]) {
          owner.addEventListener('DOMContentLoaded',e=>events.push([label,e.isTrusted,e.target===document,e.currentTarget===owner,e.eventPhase,e.bubbles,document.readyState]),capture);
        }
        window.onload=e=>{loads++;events.push(['load',e.isTrusted,e.target===document,e.currentTarget===window,e.eventPhase,e.bubbles,document.readyState,e.composedPath().length])};
        window.Event=window.dispatchEvent=document.dispatchEvent=()=>{throw Error('public override')};
        window.__documentReadyState__='forged';
        </script>"#).await;
        assert_eq!(
            history_eval(&mut page, "events"),
            json!([
                ["script", "loading"],
                ["ready", "interactive", true, true, false],
                ["capture", true, true, true, 1, true, "interactive"],
                ["document", true, true, true, 2, true, "interactive"],
                ["bubble", true, true, true, 3, true, "interactive"],
                ["ready", "complete", true, true, false],
                ["load", true, true, true, 2, false, "complete", 1]
            ])
        );
        for phase in [1, 2, 3] {
            page.js.as_mut().unwrap().document_lifecycle(phase).unwrap();
        }
        assert_eq!(
            history_eval(
                &mut page,
                "[loads,document.readyState,typeof __obscura_native_lifecycle_handoff]"
            ),
            json!([1, "complete", "undefined"])
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn document_lifecycle_obeys_listener_changes_and_microtasks() {
        let mut page=input_fixture(r#"<!doctype html><script>
        window.events=[];const removed=()=>events.push('REMOVED');
        window.addEventListener('DOMContentLoaded',e=>{events.push('capture');document.removeEventListener('DOMContentLoaded',removed)},true);
        document.addEventListener('DOMContentLoaded',removed);
        document.addEventListener('DOMContentLoaded',e=>{events.push('document');queueMicrotask(()=>events.push('microtask'));e.stopPropagation();e.preventDefault();events.push(e.defaultPrevented)},{once:true});
        window.addEventListener('DOMContentLoaded',()=>events.push('BUBBLE'));
        window.addEventListener('load',()=>{events.push('load');throw Error('listener failure')});
        window.addEventListener('load',()=>events.push('after-error'));
        </script>"#).await;
        assert_eq!(
            history_eval(&mut page, "events"),
            json!([
                "capture",
                "document",
                false,
                "microtask",
                "load",
                "after-error"
            ])
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cssom_scroll_coalesces_native_targets_before_animation_frame() {
        let mut page=input_fixture(r#"<!doctype html><style>body{margin:0;height:3000px}#box{width:100px;height:100px;overflow:auto}#content{height:1000px}</style><div id="box"><div id="content"></div></div>"#).await;
        history_eval(
            &mut page,
            r#"(() => {
          window.events=[];const box=document.getElementById('box');
          document.addEventListener('scroll',e=>events.push(['document',e.target===document,e.isTrusted,e.bubbles,scrollY]));
          window.addEventListener('scroll',e=>events.push(['window',e.target===document,e.isTrusted,e.bubbles,scrollY]));
          box.addEventListener('scroll',e=>events.push(['box',e.target===box,e.isTrusted,e.bubbles,box.scrollTop]));
          window.Event=window.dispatchEvent=document.dispatchEvent=Element.prototype.dispatchEvent=Element.prototype._fireScroll=window.setTimeout=()=>{throw Error('public override')};
          scrollTo(0,100);scrollTo(0,200);box.scrollTop=40;box.scrollTop=60;
          requestAnimationFrame(()=>events.push(['raf']));return events.length;
        })()"#,
        );
        assert_eq!(history_eval(&mut page, "events.length"), json!(0));
        page.settle(40).await;
        assert_eq!(
            history_eval(&mut page, "events"),
            json!([
                ["document", true, true, true, 200],
                ["window", true, true, true, 200],
                ["box", true, true, false, 60],
                ["raf"]
            ])
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cssom_scroll_reentry_defers_to_next_rendering_opportunity() {
        let mut page = input_fixture("<!doctype html><style>body{height:3000px}</style>").await;
        history_eval(
            &mut page,
            r#"(() => {
          window.events=[];document.addEventListener('scroll',()=>{events.push(scrollY);if(scrollY===200)scrollTo(0,300)});
          scrollTo(0,200);requestAnimationFrame(()=>events.push('raf'));
        })()"#,
        );
        page.settle(50).await;
        assert_eq!(history_eval(&mut page, "events"), json!([200, "raf", 300]));
        history_eval(&mut page, "scrollTo(0,300)");
        page.settle(20).await;
        assert_eq!(history_eval(&mut page, "events"), json!([200, "raf", 300]));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn document_lifecycle_frames_keep_independent_native_readiness() {
        let mut page = input_fixture("<!doctype html><body>PARENT</body>").await;
        let js = page.js.as_mut().unwrap();
        let frame = obscura_js::frame::FrameRealm::new(
            js,
            1,
            0,
            "http://127.0.0.1/child",
            "<!doctype html><body>CHILD</body>",
        )
        .unwrap();
        frame.execute_script(js,r#"
            globalThis.events=[document.readyState];
            document.addEventListener('readystatechange',e=>events.push([document.readyState,e.isTrusted]));
            document.addEventListener('DOMContentLoaded',e=>events.push(['dcl',e.target===document,e.isTrusted]));
            window.onload=e=>events.push(['load',e.target===document,e.currentTarget===window,e.isTrusted]);
            window.onpageshow=e=>events.push(['show',e.target===document,e.currentTarget===window,e.persisted,e.isTrusted]);
            window.Event=window.dispatchEvent=document.dispatchEvent=()=>{throw Error('public override')};
            window.__documentReadyState__='forged';
        "#).unwrap();
        frame.dispatch_load_events(js).unwrap();
        frame.dispatch_load_events(js).unwrap();
        assert_eq!(
            frame.evaluate(js, "events").unwrap(),
            json!([
                "loading",
                ["interactive", true],
                ["dcl", true, true],
                ["complete", true],
                ["load", true, true, true],
                ["show", true, true, false, true]
            ])
        );
        assert_eq!(
            frame
                .evaluate(js, "typeof __obscura_native_lifecycle_handoff")
                .unwrap(),
            json!("undefined")
        );
        assert_eq!(
            js.evaluate("document.readyState").unwrap(),
            json!("complete")
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cssom_scroll_has_no_public_trusted_dispatch_entry() {
        let mut page = input_fixture("<!doctype html><style>body{height:3000px}</style>").await;
        assert_eq!(
            history_eval(
                &mut page,
                r#"(() => {
            window.events=[];document.addEventListener('scroll',e=>events.push(e.isTrusted));
            document.dispatchEvent(new Event('scroll'));
            window.scrollTo(0,0);
            return [typeof document.body._fireScroll,typeof _queueScrollEvent,scrollY];
        })()"#
            ),
            json!(["undefined", "undefined", 0])
        );
        page.settle(20).await;
        assert_eq!(history_eval(&mut page, "events"), json!([false]));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn document_fragment_lifecycle_uses_native_target_focus_scroll_and_pixels() {
        let mut page = input_fixture(r#"<!doctype html><style>body{margin:0;height:2600px}#one{position:absolute;left:40px;top:900px;width:200px;height:40px}:target{background:blue}</style><div id="one" tabindex="-1"></div><script>
        globalThis.events=[];globalThis.initialTarget=document.querySelector(':target')?.id??null;
        const one=document.getElementById('one');one.addEventListener('focus',e=>events.push(['focus',e.isTrusted,scrollY]));
        addEventListener('scroll',e=>events.push(['scroll',e.isTrusted,scrollY]),true);
        addEventListener('DOMContentLoaded',()=>events.push(['dcl',document.activeElement.id,scrollY]));
        addEventListener('load',()=>events.push(['load',document.activeElement.id,scrollY]));
        addEventListener('popstate',()=>events.push('unexpected-pop'));addEventListener('hashchange',()=>events.push('unexpected-hash'));
        globalThis.scrollTo=Element.prototype.scrollIntoView=HTMLElement.prototype.focus=Element.prototype.getBoundingClientRect=()=>{throw Error('public helper')};
        </script>"#).await;
        page.navigate("http://127.0.0.1/new-document#one")
            .await
            .unwrap();
        assert_eq!(history_eval(&mut page,"[initialTarget,document.querySelector(':target').id,scrollY,history.length,events]"),json!(["one","one",900,2,[["focus",true,900],["scroll",true,900],["dcl","one",900],["load","one",900]]]));
        let png = page.screenshot(page.viewport).unwrap();
        assert_eq!(pixel(&page, 50, 10), [0, 0, 255, 255]);
        if let Ok(dir) = std::env::var("AUTOPILOT_BROWSER_EVIDENCE_DIR") {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                std::path::Path::new(&dir).join("document-fragment-landed.png"),
                png,
            )
            .unwrap();
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn document_fragment_finds_targets_inserted_before_load() {
        let mut page=input_fixture(r#"<!doctype html><style>body{margin:0;height:2600px}#late{position:absolute;top:1100px}</style><script>
        globalThis.events=[];addEventListener('DOMContentLoaded',()=>{events.push(['dcl',scrollY]);const target=document.createElement('div');target.id='late';target.tabIndex=-1;target.textContent='LATE';document.body.appendChild(target)});
        addEventListener('load',()=>events.push(['load',scrollY,document.querySelector(':target')?.id,document.activeElement.id]));
        </script>"#).await;
        page.navigate("http://127.0.0.1/late-document#late")
            .await
            .unwrap();
        assert_eq!(
            history_eval(&mut page, "events"),
            json!([["dcl", 0], ["load", 1100, "late", "late"]])
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn document_fragment_cancels_old_landing_after_script_scroll_or_navigation() {
        for (callback, expected_url, expected_y) in [
            ("window.scrollTo(0,333)", "http://127.0.0.1/source#one", 333),
            (
                "history.replaceState({keep:true},'')",
                "http://127.0.0.1/source#one",
                900,
            ),
            (
                "history.replaceState({},'', '?new')",
                "http://127.0.0.1/source?new",
                0,
            ),
            ("location.hash='two'", "http://127.0.0.1/source#two", 1200),
            (
                "location.assign('/pending')",
                "http://127.0.0.1/source#one",
                0,
            ),
        ] {
            let mut page=input_fixture(r#"<!doctype html><style>body{margin:0;height:2600px}#one{position:absolute;top:900px}#two{position:absolute;top:1200px}</style><div id="one" tabindex="-1">ONE</div><div id="two" tabindex="-1">TWO</div>"#).await;
            let js = page.js.as_mut().unwrap();
            js.set_url("http://127.0.0.1/source#one");
            let mut fragment = js.begin_document_fragment();
            js.execute_script("<fragment-priority>", callback).unwrap();
            js.try_document_fragment(&mut fragment).unwrap();
            assert!(fragment.is_none());
            assert_eq!(
                history_eval(&mut page, "[location.href,scrollY]"),
                json!([expected_url, expected_y])
            );
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn document_fragment_focus_reentry_preserves_the_newer_target() {
        let mut page=input_fixture(r#"<!doctype html><style>body{margin:0;height:2600px}#one{position:absolute;top:900px}#two{position:absolute;top:1200px}</style><div id="one" tabindex="-1">ONE</div><div id="two" tabindex="-1">TWO</div><script>
        document.getElementById('one').addEventListener('focus',()=>location.hash='two');
        addEventListener('load',()=>{window.loaded=[location.hash,scrollY,document.querySelector(':target').id,document.activeElement.id]});
        </script>"#).await;
        page.navigate("http://127.0.0.1/reentry#one").await.unwrap();
        assert_eq!(
            history_eval(&mut page, "loaded"),
            json!(["#two", 1200, "two", "two"])
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn document_fragment_cross_document_link_plans_one_native_request() {
        let mut page=input_fixture(r#"<!doctype html><a id="link" href="/different#one" style="display:block;width:120px;height:40px">NEXT</a>"#).await;
        page.js.as_mut().unwrap().native_click("#link").unwrap();
        let request = page
            .js
            .as_ref()
            .unwrap()
            .take_pending_navigation_request()
            .unwrap();
        assert_eq!(request.url, "http://127.0.0.1/different#one");
        assert_eq!(request.method, "GET");
        assert_eq!(
            request.request.referrer.unwrap().as_str(),
            "http://127.0.0.1/native-input-fixture"
        );
        assert_eq!(
            history_eval(&mut page, "[location.href,history.length]"),
            json!(["http://127.0.0.1/native-input-fixture", 1])
        );
        assert!(page
            .js
            .as_ref()
            .unwrap()
            .take_pending_navigation_request()
            .is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn document_fragment_redirect_inheritance_matches_plain_and_stealth_http() {
        for stealth in [false, true] {
            for (location, expected) in [
                ("/final", Some("one")),
                ("/final#two", Some("two")),
                ("/final#", Some("")),
            ] {
                let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
                let base = format!("http://{}", listener.local_addr().unwrap());
                let server = std::thread::spawn(move || {
                    use std::io::BufRead;
                    let mut paths = Vec::new();
                    for redirect in [true, false] {
                        let (mut stream, _) = listener.accept().unwrap();
                        stream
                            .set_read_timeout(Some(std::time::Duration::from_secs(3)))
                            .unwrap();
                        let mut reader = std::io::BufReader::new(&stream);
                        let mut first = String::new();
                        reader.read_line(&mut first).unwrap();
                        paths.push(first);
                        loop {
                            let mut line = String::new();
                            assert!(reader.read_line(&mut line).unwrap() > 0);
                            if line == "\r\n" {
                                break;
                            }
                        }
                        let response = if redirect {
                            format!("HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                        } else {
                            "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                                .into()
                        };
                        stream.write_all(response.as_bytes()).unwrap();
                    }
                    paths
                });
                let cookies = Arc::new(CookieJar::new());
                let mut client = ObscuraHttpClient::with_full_options(cookies.clone(), None, true);
                client.block_trackers = false;
                let url = Url::parse(&(base.clone() + "/start#one")).unwrap();
                let client = Arc::new(client);
                let response = if stealth {
                    StealthHttpClient::with_policy(cookies, None, client)
                        .fetch(&url)
                        .await
                        .unwrap()
                } else {
                    client.fetch(&url).await.unwrap()
                };
                assert_eq!(response.url.fragment(), expected);
                assert_eq!(response.url.path(), "/final");
                assert_eq!(
                    server.join().unwrap(),
                    vec!["GET /start HTTP/1.1\r\n", "GET /final HTTP/1.1\r\n"]
                );
            }
        }
    }
    #[tokio::test(flavor = "current_thread")]
    async fn history_scroll_restores_native_viewport_nested_regions_and_pixels() {
        let mut page = input_fixture(r#"<!doctype html><style>body{margin:0;width:2000px;height:3000px}#box{position:absolute;left:100px;top:900px;width:200px;height:100px;overflow:auto}#content{width:600px;height:800px}#inner{position:relative;left:50px;top:200px;width:100px;height:80px;overflow:auto}#wide{width:400px;height:400px}#blue{position:absolute;left:220px;top:740px;width:80px;height:60px;background:blue}</style><input id="field"><div id="blue"></div><div id="box"><div id="content"><div id="inner"><div id="wide"></div></div></div></div>"#).await;
        page.js.as_mut().unwrap().execute_script("<scroll-setup>",r#"
            const box=document.getElementById('box'),inner=document.getElementById('inner');
            const topGet=Object.getOwnPropertyDescriptor(Element.prototype,'scrollTop').get,leftGet=Object.getOwnPropertyDescriptor(Element.prototype,'scrollLeft').get;
            globalThis.readScroll=()=>[leftGet.call(document.scrollingElement),topGet.call(document.scrollingElement),leftGet.call(box),topGet.call(box),leftGet.call(inner),topGet.call(inner)];
            document.getElementById('field').focus();window.scrollTo(100,300);box.scrollTo(60,120);inner.scrollTo(20,30);history.pushState(null,'','?a');
            window.scrollTo(200,700);box.scrollTo(140,220);inner.scrollTo(40,70);history.pushState(null,'','?b');
            window.scrollTo(300,1000);box.scrollTo(180,320);inner.scrollTo(60,90);
            globalThis.events=[];addEventListener('popstate',()=>events.push(['popstate',readScroll()]));
            addEventListener('scroll',e=>events.push(['scroll',e.isTrusted]),true);
            globalThis.setTimeout=globalThis.dispatchEvent=()=>{throw Error('public helper')};
            globalThis.scrollTo=Element.prototype.scrollTo=Element.prototype.getBoundingClientRect=()=>{throw Error('public geometry')};
            Object.defineProperty(Element.prototype,'scrollTop',{get(){throw Error('public scrollTop')}});
            Object.defineProperty(Element.prototype,'scrollLeft',{get(){throw Error('public scrollLeft')}});
        "#).unwrap();
        page.js
            .as_mut()
            .unwrap()
            .run_event_loop_bounded(100)
            .await
            .unwrap();
        history_eval(&mut page, "(events=[])");
        history_eval(&mut page, "history.back()");
        page.js
            .as_mut()
            .unwrap()
            .run_event_loop_bounded(100)
            .await
            .unwrap();
        assert_eq!(
            history_eval(&mut page, "readScroll()"),
            json!([200, 700, 140, 220, 40, 70])
        );
        assert_eq!(
            history_eval(&mut page, "events[0]"),
            json!(["popstate", [300, 1000, 180, 320, 60, 90]])
        );
        assert_eq!(
            history_eval(&mut page, "events.slice(1)"),
            json!([["scroll", true], ["scroll", true], ["scroll", true]])
        );
        assert_eq!(
            history_eval(&mut page, "document.activeElement.id"),
            json!("field")
        );
        assert_eq!(pixel(&page, 30, 50), [0, 0, 255, 255]);
        save_input_evidence(&page, "history-scroll-restored.png");
        history_eval(&mut page, "history.forward()");
        page.js
            .as_mut()
            .unwrap()
            .run_event_loop_bounded(100)
            .await
            .unwrap();
        assert_eq!(
            history_eval(&mut page, "readScroll()"),
            json!([300, 1000, 180, 320, 60, 90])
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn history_scroll_manual_and_popstate_updates_follow_traversal_restore_order() {
        for (callback, query, expected_mode) in [
            (
                "history.scrollRestoration='manual';window.scrollTo(0,555)",
                "?a",
                "manual",
            ),
            (
                "history.pushState({newer:true},'','?new');window.scrollTo(0,222)",
                "?new",
                "auto",
            ),
            (
                "location.hash='target';window.scrollTo(0,222)",
                "?a",
                "auto",
            ),
        ] {
            let mut page=input_fixture(r#"<!doctype html><style>body{margin:0;height:2600px}#target{position:absolute;top:1600px}</style><div id="target">TARGET</div>"#).await;
            page.js.as_mut().unwrap().execute_script("<scroll-manual>",&format!("window.scrollTo(0,300);history.pushState(null,'','?a');window.scrollTo(0,700);history.pushState(null,'','?b');window.scrollTo(0,1000);addEventListener('popstate',()=>{{{callback}}},{{once:true}});history.back()")).unwrap();
            page.js
                .as_mut()
                .unwrap()
                .run_event_loop_bounded(100)
                .await
                .unwrap();
            assert_eq!(
                history_eval(
                    &mut page,
                    "[window.scrollY,location.search,history.scrollRestoration]"
                ),
                json!([
                    if expected_mode == "manual" { 555 } else { 700 },
                    query,
                    expected_mode
                ])
            );
            if query == "?new" {
                assert_eq!(
                    history_eval(&mut page, "history.state"),
                    json!({"newer":true})
                );
            }
            if callback.starts_with("location.hash") {
                assert_eq!(
                    history_eval(
                        &mut page,
                        "[location.hash,document.querySelector(':target').id]"
                    ),
                    json!(["#target", "target"])
                );
            }
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn history_scroll_clamps_after_popstate_layout_changes_and_restores_zero() {
        let mut page=input_fixture(r#"<!doctype html><style>body{margin:0;height:2000px}#box{position:fixed;top:10px;width:200px;height:100px;overflow:auto}#content{height:800px}</style><div id="box"><div id="content"></div></div>"#).await;
        page.js.as_mut().unwrap().execute_script("<scroll-clamp>","const box=document.getElementById('box');history.pushState(null,'','?a');window.scrollTo(0,900);box.scrollTop=250;history.pushState(null,'','?b');window.scrollTo(0,1000);box.scrollTop=350;addEventListener('popstate',()=>{document.body.style.height='600px';document.getElementById('content').style.height='150px'}, {once:true});history.back()").unwrap();
        page.js
            .as_mut()
            .unwrap()
            .run_event_loop_bounded(100)
            .await
            .unwrap();
        assert_eq!(
            history_eval(&mut page, "[window.scrollY,box.scrollTop]"),
            json!([120, 50])
        );
        history_eval(&mut page, "history.back()");
        page.js
            .as_mut()
            .unwrap()
            .run_event_loop_bounded(100)
            .await
            .unwrap();
        assert_eq!(
            history_eval(&mut page, "[window.scrollY,box.scrollTop]"),
            json!([0, 0])
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn history_scroll_distinguishes_reinserted_nodes_from_reused_arena_slots() {
        for reuse in [false, true] {
            let mut page=input_fixture(r#"<!doctype html><style>#box{width:200px;height:100px;overflow:auto}#content{height:800px}</style><div id="box"><div id="content"></div></div>"#).await;
            history_eval(&mut page,"(()=>{document.getElementById('box').scrollTop=300;history.pushState(null,'','?a')})()");
            page.js.as_ref().unwrap().with_dom(|dom| {
                let node = dom.query_selector_all("#box").unwrap()[0];
                let generation = dom.node_generation(node).unwrap();
                let parent = dom.get_node(node).unwrap().parent.unwrap();
                if reuse {
                    let data = dom.get_node(node).unwrap().data;
                    let children = dom.children(node);
                    for child in &children {
                        dom.detach(*child);
                    }
                    dom.remove(node);
                    let replacement = dom.new_node(data);
                    assert_eq!(replacement, node);
                    assert_ne!(dom.node_generation(node), Some(generation));
                    dom.append_child(parent, replacement);
                    for child in children {
                        dom.append_child(replacement, child);
                    }
                } else {
                    dom.detach(node);
                    dom.append_child(parent, node);
                    assert_eq!(dom.node_generation(node), Some(generation));
                }
            });
            history_eval(&mut page,"(()=>{document.getElementById('box').style.border='0px';document.getElementById('box').scrollTop=50;history.back()})()");
            page.js
                .as_mut()
                .unwrap()
                .run_event_loop_bounded(100)
                .await
                .unwrap();
            assert_eq!(
                history_eval(&mut page, "document.getElementById('box').scrollTop"),
                json!(if reuse { 50 } else { 300 })
            );
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn history_scroll_fragment_traversal_restores_view_without_refocusing_target() {
        let mut page=input_fixture(r#"<!doctype html><style>body{margin:0;height:2600px}#one{position:absolute;top:900px}#two{position:absolute;top:1200px}</style><div id="one" tabindex="-1">ONE</div><div id="two" tabindex="-1">TWO</div>"#).await;
        page.js.as_mut().unwrap().execute_script("<scroll-fragments>","location.hash='one';window.scrollTo(0,650);location.hash='two';window.scrollTo(0,1050);globalThis.popView=[];addEventListener('popstate',()=>popView.push([window.scrollY,document.querySelector(':target').id,document.activeElement.id]));history.back()").unwrap();
        page.js
            .as_mut()
            .unwrap()
            .run_event_loop_bounded(100)
            .await
            .unwrap();
        assert_eq!(history_eval(&mut page,"[window.scrollY,document.querySelector(':target').id,document.activeElement.id,popView]"),json!([650,"one","two",[[1050,"one","two"]]]));
        history_eval(&mut page, "history.forward()");
        page.js
            .as_mut()
            .unwrap()
            .run_event_loop_bounded(100)
            .await
            .unwrap();
        assert_eq!(history_eval(&mut page, "window.scrollY"), json!(1050));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn history_scroll_preserves_shadow_region_positions() {
        let mut page = input_fixture("<!doctype html><div id=host></div>").await;
        page.js.as_mut().unwrap().execute_script("<shadow-scroll>",r#"const shadow=document.getElementById('host').attachShadow({mode:'open'});shadow.innerHTML='<div id="box" style="width:200px;height:100px;overflow:auto"><div style="height:800px"></div></div>';globalThis.box=shadow.querySelector('#box');box.scrollTop=250;history.pushState(null,'','?a');box.scrollTop=50;history.back()"#).unwrap();
        page.js
            .as_mut()
            .unwrap()
            .run_event_loop_bounded(100)
            .await
            .unwrap();
        assert_eq!(history_eval(&mut page, "box.scrollTop"), json!(250));
    }
    #[tokio::test(flavor = "current_thread")]
    async fn history_storage_snapshots_and_silent_push_replace() {
        let mut page = input_fixture("<!doctype html><input id=field value=KEEP>").await;
        assert_eq!(
            history_eval(
                &mut page,
                r#"(() => {
            window.events=[];onpopstate=e=>events.push(e.type);onhashchange=e=>events.push(e.type);
            const original={value:1,map:new Map([['k',2]]),bytes:new Uint8Array([3,4])}; original.self=original;
            history.pushState(original,'','#a'); original.value=9; original.map.set('k',8);original.bytes[0]=7;
            window.snapshot=[history.state.value,history.state.map.get('k'),history.state.bytes[0],history.state.self===history.state,history.state!==original];
            history.state.value=6;
            history.pushState({value:2},'','#b');history.replaceState({value:3},'','#c');
            snapshot.push(history.length,history.state.value,events.length);
            return snapshot;
        })()"#
            ),
            json!([1, 2, 3, true, true, 3, 3, 0])
        );
        history_eval(&mut page, "history.back()");
        assert_eq!(history_eval(&mut page, "location.hash"), json!("#c"));
        page.settle(30).await;
        assert_eq!(
            history_eval(&mut page, "[history.state.value,location.hash,events]"),
            json!([1, "#a", ["popstate", "hashchange"]])
        );
        assert_eq!(
            history_eval(&mut page, "document.getElementById('field').value"),
            json!("KEEP")
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn history_rejects_bad_state_and_url_without_partial_changes() {
        let mut page = input_fixture(r#"<!doctype html><base href="/assets/"><p>READY</p>"#).await;
        assert_eq!(
            history_eval(
                &mut page,
                r#"(() => {
            history.replaceState({ok:1},'', 'next?q=1#base');
            window.errors=[];const fail=fn=>{try{fn()}catch(e){errors.push(e.name)}};
            for(const state of [()=>{},Symbol('x'),new Proxy({},{}),new SharedArrayBuffer(8),new WebAssembly.Module(new Uint8Array([0,97,115,109,1,0,0,0]))]) fail(()=>history.pushState(state,'','/bad'));
            const thrown={marker:1};try{history.pushState({get value(){throw thrown}},'','/bad')}catch(e){errors.push(e===thrown)}
            fail(()=>history.pushState({},'','https://forbidden.invalid/'));
            fail(()=>history.pushState({},'','http://user@127.0.0.1/'));
            fail(()=>History.prototype.pushState.call({},1,''));
            fail(()=>history.replaceState(1));
            history.replaceState({ok:2},'', '');history.replaceState({ok:3},'',null);history.replaceState({ok:4},'');
            return [errors,history.length,history.state.ok,location.href];
        })()"#
            ),
            json!([
                [
                    "DataCloneError",
                    "DataCloneError",
                    "DataCloneError",
                    "DataCloneError",
                    "DataCloneError",
                    true,
                    "SecurityError",
                    "SecurityError",
                    "TypeError",
                    "TypeError"
                ],
                1,
                4,
                "http://127.0.0.1/assets/next?q=1#base"
            ])
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn history_private_traversal_events_survive_public_overrides_and_reentry() {
        for replacement in [
            "globalThis.PopStateEvent=globalThis.HashChangeEvent=function(){throw Error('constructor')}",
            "globalThis.URL=function(){throw Error('URL')}",
            "globalThis.dispatchEvent=()=>{throw Error('dispatch')}",
            "globalThis.setTimeout=()=>{throw Error('timeout')}",
            "Object.defineProperty(globalThis,'__virtualUrl',{get(){throw Error('fake URL')},set(){throw Error('fake URL')},configurable:true})",
        ] {
        let mut page = input_fixture("<!doctype html><p>READY</p>").await;
        history_eval(&mut page, &r#"(() => {
            const H=HashChangeEvent,P=PopStateEvent;window.events=[];window.timerErrors=[];console.error=(...v)=>timerErrors.push(v.map(String).join(" "));
            history.pushState({n:1},'', '/first?q=1#a');history.pushState({n:2},'', '/second?q=2#b');
            const removed=()=>events.push('REMOVED');addEventListener('popstate',removed);removeEventListener('popstate',removed);
            addEventListener('popstate',e=>{events.push([e.type,e.isTrusted,e instanceof P,e.state.n,e.bubbles,e.cancelable,e.composed]);history.pushState({n:9},'', '/callback#c')});
            addEventListener('hashchange',e=>{const old=e.oldURL;try{e.oldURL='forged'}catch{};events.push([e.type,e.isTrusted,e instanceof H,e.oldURL===old,e.oldURL,e.newURL])});
            REPLACEMENT;
            history.back();
        })()"#.replace("REPLACEMENT", replacement));
        page.js.as_mut().unwrap().run_event_loop_bounded(100).await.unwrap();
        assert_eq!(history_eval(&mut page, "events"), json!([
            ["popstate",true,true,1,false,false,false],
            ["hashchange",true,true,true,"http://127.0.0.1/second?q=2#b","http://127.0.0.1/first?q=1#a"]
        ]), "{replacement}: {}", history_eval(&mut page, "[timerErrors,history.state,location.href]"));
        assert_eq!(history_eval(&mut page, "[location.href,history.length,history.state.n]"), json!(["http://127.0.0.1/callback#c",3,9]));
    }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn history_out_of_range_traversal_and_same_url_state_navigation() {
        let mut page = input_fixture("<!doctype html><p>READY</p>").await;
        history_eval(&mut page, "(() => {window.events=[];onpopstate=e=>events.push(e.state);onhashchange=e=>events.push('hash');history.pushState(1,'');history.pushState(2,'')})()");
        page.process_pending_navigation().await.unwrap();
        history_eval(&mut page, "(() => {history.go(-9);history.go(9)})()");
        page.settle(30).await;
        assert_eq!(
            history_eval(&mut page, "[history.state,events]"),
            json!([2, []])
        );
        assert!(!page.process_pending_navigation().await.unwrap());
        history_eval(&mut page, "(() => {history.back();history.back()})()");
        page.settle(30).await;
        assert_eq!(
            history_eval(&mut page, "[history.state,events]"),
            json!([null, [1, null]])
        );
        assert!(page.process_pending_navigation().await.unwrap());
        history_eval(&mut page, "history.forward()");
        page.settle(30).await;
        assert_eq!(
            history_eval(&mut page, "[history.state,events]"),
            json!([1, [1, null, 1]])
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn history_resolves_against_active_document_during_pending_navigation() {
        let mut page = input_fixture(r#"<!doctype html><base href="/assets/"><p>READY</p>"#).await;
        history_eval(&mut page, "(() => {location.assign('https://pending.invalid/');history.pushState({ok:1},'', 'next')})()");
        assert_eq!(
            history_eval(&mut page, "location.href"),
            json!("http://127.0.0.1/assets/next")
        );
        assert_eq!(
            page.js
                .as_ref()
                .unwrap()
                .pending_navigation_url()
                .as_deref(),
            Some("https://pending.invalid/")
        );
        assert_eq!(
            history_eval(&mut page, "document.baseURI"),
            json!("http://127.0.0.1/assets/")
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn native_navigation_observation_uses_history_state_not_public_url_globals() {
        let mut page = input_fixture(r#"<!doctype html><input id="field" value="OLD">"#).await;
        page.js.as_mut().unwrap().execute_script("<fake-url>",r#"
            window.reads=0;Object.defineProperty(globalThis,'__virtualUrl',{get(){reads++;throw Error('public URL read')},configurable:true});
        "#).unwrap();
        assert!(!page.sync_virtual_url());
        assert_eq!(page.evaluate("reads"), json!(0.0));
        assert_eq!(page.js.as_ref().unwrap().pending_navigation_url(), None);
        page.js.as_mut().unwrap().execute_script("<history-url>",r#"
            Object.defineProperty(globalThis,'__virtualUrl',{value:'https://forged.invalid/base',writable:true,configurable:true});
            window.failure='';try{history.pushState({},'', 'https://forbidden.invalid/')}catch(error){failure=error.name}
            window.rejected=[history.length,location.href];history.pushState({},'', '/next');
        "#).unwrap();
        assert_eq!(page.evaluate("failure"), json!("SecurityError"));
        assert_eq!(page.evaluate("history.length"), json!(2.0));
        assert_eq!(
            page.js.as_ref().unwrap().pending_navigation_url(),
            Some("http://127.0.0.1/next".into())
        );
        assert_eq!(
            page.js
                .as_mut()
                .unwrap()
                .native_fill("#field", "NEW")
                .err()
                .unwrap(),
            ("UNEXPECTED_NAVIGATION", "NOT_SENT")
        );
        assert!(page.process_pending_navigation().await.unwrap());
        assert_eq!(page.url_string(), "http://127.0.0.1/next");
        page.js
            .as_mut()
            .unwrap()
            .native_fill("#field", "NEW")
            .unwrap();
        page.evaluate("history.back()");
        page.settle(20).await;
        assert!(page.process_pending_navigation().await.unwrap());
        assert_eq!(page.url_string(), "http://127.0.0.1/native-input-fixture");
        assert_eq!(
            page.evaluate("document.getElementById('field').value"),
            json!("NEW")
        );
        assert!(!page.process_pending_navigation().await.unwrap());
    }

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
        save_input_evidence(&page, "native-checked-controls.png");
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
        save_input_evidence(&page, "native-click-scrolled.png");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn native_link_uses_current_dom_and_ignores_public_navigation_helpers() {
        let mut page = input_fixture(r#"<!doctype html><base href="https://example.test/first/" target="_blank"><base href="https://wrong.test/" target="wrong">
            <style>a,span{display:block;width:160px;height:40px}</style><a id="link" href="old" target=""><span id="child">NEXT</span></a>"#).await;
        page.js.as_mut().unwrap().execute_script("<link-fixture>", r#"
            const link=document.getElementById('link'); globalThis.events=[];
            link.addEventListener('click',event=>{
                events.push([event.type,event.isTrusted]);
                link.setAttribute('href','new?q=hello world');
                document.querySelector('base').setAttribute('href','https://example.test/current/');
            });
            link.click=link.closest=link.getAttribute=()=>{throw Error('public helper called')};
            Object.defineProperty(link,'href',{get(){throw Error('public href called')}});
            location.assign=()=>{throw Error('public location called')};
            globalThis.__virtualUrl='https://forged.test/';
        "#).unwrap();
        assert!(!page.js.as_mut().unwrap().native_click("#child").unwrap().default_prevented);
        assert_eq!(page.evaluate("events"), json!([["click", true]]));
        assert_eq!(page.js.as_ref().unwrap().take_pending_navigation(), Some((
            "https://example.test/current/new?q=hello%20world".into(), "GET".into(), String::new())));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn native_link_cancel_removal_and_interactive_child_do_not_navigate() {
        for callback in ["event.preventDefault()", "link.removeAttribute('href')", "link.remove()", "event.target.remove()"] {
            let mut page = input_fixture(r#"<!doctype html><style>a,span{display:block;width:160px;height:40px}</style><a id="link" href="/next"><span id="child">NEXT</span></a>"#).await;
            page.js.as_mut().unwrap().execute_script("<link-cancel>", &format!(
                "const link=document.getElementById('link');link.addEventListener('click',event=>{{{callback}}});"
            )).unwrap();
            let clicked = page.js.as_mut().unwrap().native_click("#child").unwrap();
            assert_eq!(clicked.default_prevented, callback == "event.preventDefault()");
            assert!(page.js.as_ref().unwrap().pending_navigation_url().is_none(), "{callback}");
        }
        let mut page = input_fixture(r#"<!doctype html><a href="/next"><button type="button" id="button" style="width:100px;height:40px">BUTTON</button></a>"#).await;
        page.js.as_mut().unwrap().native_click("#button").unwrap();
        assert!(page.js.as_ref().unwrap().pending_navigation_url().is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn native_inline_link_followed_by_block_remains_clickable() {
        let mut page = input_fixture("<!doctype html><a id='link' href='/next'>NEXT</a><pre>result</pre>").await;
        assert_eq!(page.evaluate("(()=>{const r=document.getElementById('link').getBoundingClientRect();return r.width>0&&r.height>0})()"), json!(true));
        page.js.as_mut().unwrap().native_click("#link").unwrap();
        assert_eq!(page.js.as_ref().unwrap().pending_navigation_url().as_deref(), Some("http://127.0.0.1/next"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn native_link_checks_unsupported_defaults_before_and_after_dispatch() {
        for mutation in [
            "link.setAttribute('download','')",
            "link.setAttribute('ping','https://example.test/ping')",
            "link.setAttribute('target','_blank')",
            "link.setAttribute('target','named')",
            "link.setAttribute('href','javascript:void(0)')",
            "link.setAttribute('href','mailto:test@example.test')",
            "document.head.innerHTML='<base target=\"_blank\">'",
        ] {
            for before in [true, false] {
                let mut page = input_fixture(r#"<!doctype html><style>a{display:block;width:160px;height:40px}</style><a id="link" href="/next">NEXT</a>"#).await;
                let action = if before { mutation.to_string() } else { format!("link.addEventListener('click',()=>{{{mutation}}})") };
                page.js.as_mut().unwrap().execute_script("<unsupported-link>", &format!(
                    "const link=document.getElementById('link');globalThis.events=[];document.addEventListener('click',()=>events.push('click'));{action};"
                )).unwrap();
                assert_eq!(page.js.as_mut().unwrap().native_click("#link").unwrap_err(),
                    ("INPUT_ELEMENT_UNSUPPORTED", if before { "NOT_SENT" } else { "SENT" }), "{mutation}");
                assert_eq!(page.evaluate("events"), if before { json!([]) } else { json!(["click"]) });
                assert!(page.js.as_ref().unwrap().pending_navigation_url().is_none());
            }
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn native_link_preserves_listener_navigation_and_rejects_pointer_retarget() {
        for (event, callback, error) in [
            ("click", "location.href='/listener'", "UNEXPECTED_NAVIGATION"),
            ("click", "history.pushState({},'', '/listener')", "UNEXPECTED_NAVIGATION"),
            ("pointerdown", "link.replaceWith(link.cloneNode(true))", "INPUT_TARGET_CHANGED"),
        ] {
            let mut page = input_fixture(r#"<!doctype html><style>a{display:block;width:160px;height:40px}</style><a id="link" href="/next">NEXT</a>"#).await;
            page.js.as_mut().unwrap().execute_script("<link-route>", &format!(
                "const link=document.getElementById('link');link.addEventListener('{event}',()=>{{{callback}}});"
            )).unwrap();
            assert_eq!(page.js.as_mut().unwrap().native_click("#link").map(|_| ()), Err((error, "SENT")), "{event}: {callback}");
            if event == "click" {
                assert_eq!(page.js.as_ref().unwrap().pending_navigation_url().as_deref(), Some("http://127.0.0.1/listener"));
            }
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn native_frozen_base_survives_history_before_first_read_and_public_url_spoofing() {
        let mut page = input_fixture(r#"<!doctype html><base href="assets/"><a id="link" href="next" style="display:block;width:100px;height:40px">NEXT</a><script>
            history.pushState({},'', '/moved/page');
            globalThis.__virtualUrl='https://forged.invalid/';
        </script>"#).await;
        assert_eq!(page.evaluate("[document.baseURI,document.getElementById('link').href]"),
            json!(["http://127.0.0.1/assets/", "http://127.0.0.1/assets/next"]));
        page.js.as_mut().unwrap().native_click("#link").unwrap();
        assert_eq!(page.js.as_ref().unwrap().pending_navigation_url().as_deref(), Some("http://127.0.0.1/assets/next"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn native_frozen_base_updates_on_mutation_and_first_element_changes() {
        let mut page = input_fixture(r#"<!doctype html><base id="first" href="one/"><base id="second" href="second/">"#).await;
        page.js.as_mut().unwrap().execute_script("<base-mutations>", r#"
            const first=document.getElementById('first'),second=document.getElementById('second');
            globalThis.results=[document.baseURI];
            history.replaceState({},'', '/moved/page');results.push(document.baseURI);
            first.setAttribute('href','two/');first.setAttribute('href','one/');results.push(document.baseURI);
            first.remove();results.push(document.baseURI);
            history.pushState({},'', '/third/page');results.push(document.baseURI);
            document.head.insertBefore(first,second);results.push(document.baseURI);
            first.remove();document.head.insertBefore(first,second);results.push(document.baseURI);
            first.removeAttribute('href');results.push(document.baseURI);
            document.head.innerHTML='<base href="replacement/">';results.push(document.baseURI);
            document.head.innerHTML='';results.push(document.baseURI);
            globalThis.__virtualUrl='https://forged.invalid/';results.push(document.baseURI);
        "#).unwrap();
        assert_eq!(page.evaluate("results"), json!([
            "http://127.0.0.1/one/", "http://127.0.0.1/one/", "http://127.0.0.1/moved/one/",
            "http://127.0.0.1/moved/second/", "http://127.0.0.1/moved/second/", "http://127.0.0.1/third/one/",
            "http://127.0.0.1/third/one/", "http://127.0.0.1/third/second/", "http://127.0.0.1/third/replacement/",
            "http://127.0.0.1/third/page", "http://127.0.0.1/third/page"
        ]));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn native_frozen_base_reinsert_same_node_and_native_writes_invalidate_cache() {
        let mut page = input_fixture(r#"<!doctype html><base id="base" href="initial/">"#).await;
        assert_eq!(page.evaluate("document.baseURI"), json!("http://127.0.0.1/initial/"));
        page.js.as_mut().unwrap().execute_script("<base-reinsert>", r#"
            const base=document.getElementById('base');history.pushState({},'', '/new/page');
            base.remove();document.head.appendChild(base);
        "#).unwrap();
        assert_eq!(page.evaluate("document.baseURI"), json!("http://127.0.0.1/new/initial/"));
        page.js.as_ref().unwrap().with_dom(|dom| {
            let id=dom.get_element_by_id("base").unwrap();
            dom.with_node_mut(id, |node| node.set_attribute("href", "native/".into()));
        });
        assert_eq!(page.evaluate("document.baseURI"), json!("http://127.0.0.1/new/native/"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn native_frozen_base_uses_frozen_fallback_for_empty_invalid_and_disallowed_schemes() {
        for href in ["", "http://[", "data:text/html,base", "javascript:void(0)"] {
            let mut page = input_fixture("<!doctype html><base id=base>").await;
            page.js.as_mut().unwrap().execute_script("<base-fallback>", &format!(
                "document.getElementById('base').setAttribute('href',{});history.pushState({{}},'', '/changed/page');",
                serde_json::to_string(href).unwrap()
            )).unwrap();
            assert_eq!(page.evaluate("document.baseURI"), json!("http://127.0.0.1/native-input-fixture"), "{href}");
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn native_frozen_base_ignores_foreign_namespaces_shadow_and_detached_nodes() {
        let mut page = input_fixture("<!doctype html><div id=host></div>").await;
        page.js.as_mut().unwrap().execute_script("<base-scopes>", r#"
            const foreign=document.createElementNS('http://www.w3.org/2000/svg','base');
            foreign.setAttribute('href','https://wrong.invalid/');document.head.appendChild(foreign);
            const detached=document.createElement('base');detached.setAttribute('href','detached/');
            const shadow=document.getElementById('host').attachShadow({mode:'open'});
            shadow.innerHTML='<base href="https://shadow.invalid/">';
            const base=document.createElement('base');base.setAttributeNS('urn:test','href','https://namespace.invalid/');
            document.head.appendChild(base);globalThis.results=[document.baseURI];
            base.setAttributeNS(null,'href','actual/');results.push(document.baseURI);
            history.pushState({},'', '/new/page');base.removeAttributeNS(null,'href');results.push(document.baseURI);
        "#).unwrap();
        assert_eq!(page.evaluate("results"), json!([
            "http://127.0.0.1/native-input-fixture", "http://127.0.0.1/actual/", "http://127.0.0.1/new/page"
        ]));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn native_navigation_captures_document_source_before_location_changes() {
        let mut page = input_fixture("<!doctype html><base href='https://other.invalid/base/'>").await;
        page.js.as_mut().unwrap().execute_script("<test>", r#"
            history.pushState({}, '', 'http://127.0.0.1/source?q=1#fragment');
            location.href='/discarded'; location.href='/destination';
        "#).unwrap();
        let navigation = page.js.as_ref().unwrap().take_pending_navigation_request().unwrap();
        assert_eq!(navigation.url, "https://other.invalid/destination");
        assert_eq!(navigation.method, "GET");
        assert!(navigation.body.is_empty());
        let source = Url::parse("http://127.0.0.1/source?q=1#fragment").unwrap();
        assert_eq!(navigation.request.referrer, Some(source.clone()));
        assert_eq!(navigation.request.initiator, Some(source));
        assert_eq!(navigation.request.referrer_policy, obscura_net::ReferrerPolicy::default());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn native_frozen_base_does_not_leak_across_document_replacement() {
        let mut page = input_fixture("<!doctype html><base href=old/>").await;
        assert_eq!(page.evaluate("document.baseURI"), json!("http://127.0.0.1/old/"));
        let js=page.js.as_ref().unwrap();
        js.set_url("http://127.0.0.1/new/page");
        js.set_dom(obscura_dom::parse_html("<!doctype html><base href=fresh/>"));
        assert_eq!(page.evaluate("document.baseURI"), json!("http://127.0.0.1/new/fresh/"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn native_frozen_base_same_value_attribute_writes_use_the_current_fallback() {
        let mut page = input_fixture("<!doctype html><base id=first href=assets/><base id=second href=ignored/>").await;
        page.js.as_mut().unwrap().execute_script("<base-same-value>", r#"
            const first=document.getElementById('first'),second=document.getElementById('second');
            history.pushState({},'', '/other/page');
            second.setAttribute('href','ignored/');globalThis.results=[document.baseURI];
            first.setAttribute('class','changed');results.push(document.baseURI);
            first.setAttribute('href','assets/');results.push(document.baseURI);
            history.pushState({},'', '/third/page');
            first.setAttributeNS(null,'href','assets/');results.push(document.baseURI);
        "#).unwrap();
        assert_eq!(page.evaluate("results"), json!([
            "http://127.0.0.1/assets/", "http://127.0.0.1/assets/",
            "http://127.0.0.1/other/assets/", "http://127.0.0.1/third/assets/"
        ]));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn native_link_accepts_top_level_targets_and_refuses_image_map_coordinates() {
        for target in ["_SELF", "_TOP", "_PARENT", ""] {
            let mut page = input_fixture(r#"<!doctype html><style>a{display:block;width:160px;height:40px}</style><a id="link" href="/next">NEXT</a>"#).await;
            page.js.as_mut().unwrap().execute_script("<link-target>", &format!(
                "document.getElementById('link').setAttribute('target','{target}');"
            )).unwrap();
            page.js.as_mut().unwrap().native_click("#link").unwrap();
            assert_eq!(page.js.as_ref().unwrap().pending_navigation_url().as_deref(), Some("http://127.0.0.1/next"));
        }
        let mut page = input_fixture(r#"<!doctype html><a id="link" href="/next"><img id="map" ismap width="160" height="40"></a>"#).await;
        assert_eq!(page.js.as_mut().unwrap().native_click("#map").unwrap_err(), ("INPUT_ELEMENT_UNSUPPORTED", "NOT_SENT"));
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
    async fn sdk_inline_link_after_button_has_geometry() {
        let page=input_fixture("<!doctype html><body><main><button>我已确认购票、搭乘相关的注意事项。</button><a href='#'>下一步</a></main>").await;
        let js=page.js.as_ref().unwrap();
        let node=js.input_node("a").unwrap();
        assert!(js.automation_box(node).is_some(), "native target: {:?}", js.input_target("a"));
    }

}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Viewport {
    width: u32,
    height: u32,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Persona {
    schema_version: String,
    persona_id: String,
    revision: String,
    profile: String,
    viewport: Viewport,
    #[serde(default)]
    tracker_blocking: bool,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    languages: Option<Vec<String>>,
    #[serde(default)]
    accept_language: Option<String>,
    #[serde(default)]
    timezone: Option<String>,
    #[serde(default)]
    do_not_track: Option<String>,
    #[serde(default)]
    hardware_concurrency: Option<u32>,
    #[serde(default)]
    device_memory: Option<f64>,
    #[serde(default)]
    screen_width: Option<u32>,
    #[serde(default)]
    screen_height: Option<u32>,
    #[serde(default)]
    screen_avail_width: Option<u32>,
    #[serde(default)]
    screen_avail_height: Option<u32>,
    #[serde(default)]
    outer_width: Option<u32>,
    #[serde(default)]
    outer_height: Option<u32>,
    #[serde(default)]
    device_scale_factor: Option<f64>,
    #[serde(default)]
    battery_charging: Option<bool>,
    #[serde(default)]
    battery_level: Option<f64>,
    #[serde(default)]
    network_rtt: Option<u32>,
    #[serde(default)]
    storage_quota: Option<u64>,
    #[serde(default)]
    webgl_vendor: Option<String>,
    #[serde(default)]
    webgl_renderer: Option<String>,
}

impl Persona {
    fn apply_defaults(&mut self) {
        let macos = matches!(self.profile.as_str(), "macos_chrome152" | "macos_chrome153");
        if self.language.is_none() && self.languages.is_none() {
            let defaults = if macos {
                vec!["en".into(), "zh-CN".into()]
            } else {
                vec!["en-US".into(), "en".into()]
            };
            self.language = defaults.first().cloned();
            self.languages = Some(defaults);
        } else if self.language.is_none() {
            self.language = self.languages.as_ref().and_then(|v| v.first()).cloned();
        } else if self.languages.is_none() {
            // Derive the language list the way a browser reports it: the primary
            // tag followed by its base language. `accept_language` below already
            // expands the same way, so a single `language` must not leave
            // navigator.languages reporting ["en-US"] while Accept-Language says
            // "en-US,en" -- no real browser is internally inconsistent like that.
            let primary = self.language.clone().unwrap();
            let mut derived = vec![primary.clone()];
            if let Some((base, _)) = primary.split_once('-') {
                if !derived.iter().any(|tag| tag == base) {
                    derived.push(base.to_string());
                }
            }
            self.languages = Some(derived);
        }
        self.accept_language.get_or_insert_with(|| {
            let mut expanded = Vec::<String>::new();
            for language in self.languages.as_ref().unwrap() {
                if !expanded.contains(language) { expanded.push(language.clone()); }
                if let Some((base, _)) = language.split_once('-') {
                    let base = base.to_string();
                    if !expanded.contains(&base) { expanded.push(base); }
                }
            }
            expanded.into_iter().enumerate().map(|(index, language)| {
                if index == 0 { language } else {
                    format!("{};q=0.{}", language, 9usize.saturating_sub(index - 1).max(1))
                }
            }).collect::<Vec<_>>().join(",")
        });
        self.timezone.get_or_insert_with(|| if macos { "Asia/Shanghai".into() } else { "America/New_York".into() });
        // Do NOT invent a DNT preference. Real Chrome ships no default DNT
        // value: it sends no `DNT` request header and reports
        // `navigator.doNotTrack === null` (measured against Chrome 153 on
        // macOS). Defaulting the macOS profile to "1" asserted the opposite on
        // both layers at once - a deliberate-looking privacy stance that no
        // stock Chrome has, and a well-known automation tell. A persona may
        // still set `do_not_track` explicitly; only the fabricated default is
        // gone.
        self.hardware_concurrency.get_or_insert(if macos { 15 } else { 8 });
        self.device_memory.get_or_insert(if macos { 32.0 } else { 8.0 });
        self.screen_width.get_or_insert(if macos { 2560 } else { 1920 });
        self.screen_height.get_or_insert(if macos { 1440 } else { 1080 });
        self.screen_avail_width.get_or_insert(self.screen_width.unwrap());
        self.screen_avail_height.get_or_insert(
            self.screen_height.unwrap().saturating_sub(if macos { 120 } else { 40 }),
        );
        self.outer_width.get_or_insert(if macos { self.viewport.width } else { self.screen_width.unwrap() });
        self.outer_height.get_or_insert(if macos { self.viewport.height } else { self.screen_avail_height.unwrap() });
        self.device_scale_factor.get_or_insert(if macos { 2.0 } else { 1.0 });
        self.battery_charging.get_or_insert(true);
        self.battery_level.get_or_insert(0.8);
        self.network_rtt.get_or_insert(100);
        self.storage_quota.get_or_insert(10_738_064_711);
        self.webgl_vendor.get_or_insert_with(|| if macos {
            "Google Inc. (Apple)".into()
        } else {
            "Google Inc. (NVIDIA)".into()
        });
        self.webgl_renderer.get_or_insert_with(|| if macos {
            "ANGLE (Apple, ANGLE Metal Renderer: Apple M5 Pro, Unspecified Version)".into()
        } else {
            "ANGLE (NVIDIA, NVIDIA GeForce RTX 3060 Direct3D11 vs_5_0 ps_5_0, D3D11)".into()
        });
    }

    fn locale_is_consistent(&self) -> bool {
        let Some(language) = self.language.as_deref() else { return false; };
        let Some(languages) = self.languages.as_ref() else { return false; };
        if languages.first().map(String::as_str) != Some(language) { return false; }
        let Some(header) = self.accept_language.as_deref() else { return false; };
        let tags = header.split(',').map(|part| part.split(';').next().unwrap_or("").trim()).collect::<Vec<_>>();
        if tags.first().copied() != Some(language) { return false; }
        let mut offset = 0;
        for language in languages {
            let Some(index) = tags[offset..].iter().position(|tag| *tag == language) else { return false; };
            offset += index + 1;
        }
        true
    }

    fn preload_script(&self) -> String {
        format!(
            "globalThis.__obscura_battery_charging={};\
             globalThis.__obscura_battery_level={};\
             globalThis.__obscura_network_rtt={};\
             globalThis.__obscura_storage_quota={};\
             if(globalThis.screen){{globalThis.screen._availW={};globalThis.screen._availH={};}}\
             globalThis.outerWidth={};globalThis.outerHeight={};",
            self.battery_charging.unwrap(), self.battery_level.unwrap(),
            self.network_rtt.unwrap(), self.storage_quota.unwrap(),
            self.screen_avail_width.unwrap(), self.screen_avail_height.unwrap(),
            self.outer_width.unwrap(), self.outer_height.unwrap(),
        )
    }

    fn device_identity(&self) -> obscura_browser::DeviceIdentity {
        let source = serde_json::to_vec(&[
            "autopilot-persona-v1",
            &self.persona_id,
            &self.revision,
            &self.profile,
        ])
        .unwrap();
        let hash = Sha256::digest(source);
        let macos = matches!(self.profile.as_str(), "macos_chrome152" | "macos_chrome153");
        obscura_browser::DeviceIdentity {
            seed: u32::from_be_bytes(hash[..4].try_into().unwrap()),
            hardware_concurrency: self.hardware_concurrency.unwrap_or(if macos { 15 } else { 8 }),
            device_memory: self.device_memory.unwrap_or(if macos { 32.0 } else { 8.0 }),
            screen_width: self.screen_width.unwrap_or(1920),
            screen_height: self.screen_height.unwrap_or(1080),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Init {
    protocol_version: String,
    runtime_sha256: String,
    initial_mode: String,
    persona: Persona,
    allowed_origins: Vec<String>,
    #[serde(default)]
    proxy_url: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Navigate {
    url: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadText {
    selector: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Fill {
    selector: String,
    value: String,
}
#[derive(Default, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum ClickNavigation {
    #[default]
    None,
    Required,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Click {
    selector: String,
    #[serde(default)]
    navigation: ClickNavigation,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WaitText {
    selector: String,
    text: String,
    #[serde(default)]
    contains: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Mode {
    generation: u64,
    mode: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Recheck {
    generation: u64,
}

struct OriginGuard(Vec<String>);
#[async_trait::async_trait]
impl RequestInterceptor for OriginGuard {
    async fn intercept(&self, request: &RequestInfo) -> InterceptAction {
        if allowed(&request.url, &self.0) {
            InterceptAction::Continue
        } else {
            InterceptAction::Block
        }
    }
}

fn allowed(url: &Url, origins: &[String]) -> bool {
    if url.scheme() == "blob" {
        if let Ok(inner) = Url::parse(url.path()) {
            return matches!(inner.scheme(), "http" | "https")
                && inner.username().is_empty()
                && inner.password().is_none()
                && origins.contains(&inner.origin().ascii_serialization());
        }
        return false;
    }
    matches!(url.scheme(), "http" | "https")
        && url.username().is_empty()
        && url.password().is_none()
        && origins.contains(&url.origin().ascii_serialization())
}

struct OwnedPage {
    page: Page,
    generation: u64,
    url: String,
}

impl OwnedPage {
    fn navigation_url(&self) -> Option<String> {
        let js = self.page.js.as_ref()?;
        js.pending_navigation_url().or_else(|| {
            let url = js.document_url();
            (url != self.url).then_some(url)
        })
    }
}

pub struct BrowserRuntime {
    networks: HashMap<String, Arc<std::sync::Mutex<network::Network>>>,
    protocol_version: String,
    workspace: PathBuf,
    sha256: String,
    context: Option<Arc<BrowserContext>>,
    persona: Option<Persona>,
    origins: Vec<String>,
    pages: HashMap<String, OwnedPage>,
    page_counter: u64,
    capture_count: u32,
    capture_bytes: usize,
    mode: String,
    generation: u64,
    pub poisoned: bool,
    recheck_generation: Option<u64>,
    pub takeover: Option<crate::takeover::Channel>,
    takeover_attached: bool,
    manual: Option<crate::takeover::Session>,
    manual_generation: u64,
    manual_sessions: std::collections::HashSet<String>,
}

pub fn file_hash(path: &std::path::Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

impl BrowserRuntime {
    pub fn new(workspace: PathBuf) -> io::Result<Self> {
        let canonical = workspace.canonicalize()?;
        if workspace != canonical || !canonical.is_dir() {
            return Err(io::Error::other("INVALID_WORKSPACE"));
        }
        Ok(Self {
            networks: HashMap::new(),
            protocol_version: "1".into(),
            workspace,
            sha256: file_hash(&std::env::current_exe()?)?,
            context: None,
            persona: None,
            origins: vec![],
            pages: HashMap::new(),
            page_counter: 0,
            capture_count: 0,
            capture_bytes: 0,
            mode: "PAUSED".into(),
            generation: 0,
            recheck_generation: None,
            poisoned: false,
            takeover: None,
            takeover_attached: false,
            manual: None,
            manual_generation: 0,
            manual_sessions: std::collections::HashSet::new(),
        })
    }

    pub fn uses_automation(&self) -> bool { self.protocol_version == "2" }

    pub async fn handle(&mut self, request: &Request) -> Value {
        let mut response = match self.execute(request).await {
            Ok(result) => json!({"id": request.id, "ok": true, "result": result}),
            Err((code, dispatch)) => protocol::error(request.id, code, dispatch),
        };
        if self.protocol_version == "2" {
            if let Some(page)=request.page_id.as_ref().and_then(|id|self.pages.get(id)) {
                let target=if response["ok"]==true {&mut response["result"]} else {&mut response};
                target["page_generation"]=json!(page.generation);
                target["url"]=json!(page.url);
            }
        }
        response
    }

    async fn initialize(&mut self, request: &Request) -> Result<Value, Error> {
        if self.context.is_some() {
            return Err(invalid("ALREADY_INITIALIZED"));
        }
        let init: Init = params(request)?;
        if !matches!(init.protocol_version.as_str(), "1" | "2") || init.runtime_sha256 != self.sha256 {
            return Err(invalid("RUNTIME_VERSION_MISMATCH"));
        }
        if !matches!(init.initial_mode.as_str(), "RUNNING" | "PAUSED") {
            return Err(invalid("INVALID_MODE"));
        }
        let mut persona = init.persona;
        persona.apply_defaults();
        let identifier = |v: &str| {
            !v.is_empty()
                && v.len() <= 64
                && v.bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
        };
        if persona.schema_version != "1"
            || !(persona.profile == "windows_chrome145"
                || (init.protocol_version == "2"
                    && matches!(persona.profile.as_str(), "macos_chrome152" | "macos_chrome153")))
            || !identifier(&persona.persona_id)
            || !identifier(&persona.revision)
            || !(320..=3840).contains(&persona.viewport.width)
            || !(240..=2160).contains(&persona.viewport.height)
            || persona.language.as_deref().is_none_or(|v| v.is_empty() || v.len() > 32 || !v.is_ascii())
            || persona.languages.as_ref().is_none_or(|v| v.is_empty() || v.len() > 8 || v.iter().any(|s| s.is_empty() || s.len() > 32 || !s.is_ascii()))
            || persona.accept_language.as_deref().is_none_or(|v| v.is_empty() || v.len() > 256 || v.contains('\r') || v.contains('\n'))
            || !persona.locale_is_consistent()
            || persona.timezone.as_deref().is_none_or(|v| v.is_empty() || v.len() > 64 || !v.bytes().all(|c| c.is_ascii_alphanumeric() || b"_+-/".contains(&c)))
            || persona.do_not_track.as_deref().is_some_and(|v| !matches!(v, "0" | "1"))
            || !(1..=256).contains(&persona.hardware_concurrency.unwrap_or(0))
            || !persona.device_memory.is_some_and(|v| v.is_finite() && (0.25..=128.0).contains(&v))
            || !(320..=16384).contains(&persona.screen_width.unwrap_or(0))
            || !(240..=16384).contains(&persona.screen_height.unwrap_or(0))
            || !(320..=persona.screen_width.unwrap_or(0)).contains(&persona.screen_avail_width.unwrap_or(0))
            || !(240..=persona.screen_height.unwrap_or(0)).contains(&persona.screen_avail_height.unwrap_or(0))
            || !(320..=persona.screen_width.unwrap_or(0)).contains(&persona.outer_width.unwrap_or(0))
            || !(240..=persona.screen_height.unwrap_or(0)).contains(&persona.outer_height.unwrap_or(0))
            || !persona.device_scale_factor.is_some_and(|v| v.is_finite() && (0.5..=4.0).contains(&v))
            || !persona.battery_level.is_some_and(|v| v.is_finite() && (0.0..=1.0).contains(&v))
            || !(1..=10_000).contains(&persona.network_rtt.unwrap_or(0))
            || !(1_000_000..=100_000_000_000).contains(&persona.storage_quota.unwrap_or(0))
            || persona.webgl_vendor.as_deref().is_none_or(|v| v.is_empty() || v.len() > 256 || v.chars().any(char::is_control))
            || persona.webgl_renderer.as_deref().is_none_or(|v| v.is_empty() || v.len() > 512 || v.chars().any(char::is_control))
        {
            return Err(invalid("UNSUPPORTED_PERSONA"));
        }
        if init.allowed_origins.is_empty() || init.allowed_origins.len() > 16 {
            return Err(invalid("INVALID_ORIGINS"));
        }
        let mut loopbacks = 0;
        for origin in &init.allowed_origins {
            let url = Url::parse(origin).map_err(|_| invalid("INVALID_ORIGINS"))?;
            if !allowed(&url, &init.allowed_origins)
                || url.origin().ascii_serialization() != *origin
            {
                return Err(invalid("INVALID_ORIGINS"));
            }
            if matches!(url.host_str(), Some("127.0.0.1" | "[::1]")) {
                loopbacks += 1;
            }
        }
        // Private access is enabled only for an all-literal-loopback fixture list.
        if loopbacks > 0 && loopbacks != init.allowed_origins.len() {
            return Err(invalid("MIXED_PRIVATE_ORIGINS"));
        }
        if let Some(proxy) = &init.proxy_url {
            let url = Url::parse(proxy).map_err(|_| invalid("INVALID_PROXY"))?;
            if url.scheme() != "http" || url.host_str().is_none() || url.port_or_known_default().is_none()
                || !url.username().is_empty() || url.password().is_some() || url.path() != "/"
                || url.query().is_some() || url.fragment().is_some() {
                return Err(invalid("INVALID_PROXY"));
            }
        }
        let profile = match persona.profile.as_str() {
            "macos_chrome153" => obscura_net::StealthProfile::MacChrome153,
            "macos_chrome152" => obscura_net::StealthProfile::MacChrome152,
            _ => obscura_net::StealthProfile::WindowsChrome145,
        };
        let mut context = BrowserContext::with_storage_and_network(
            "autopilot".into(),
            init.proxy_url,
            true,
            Some(profile.user_agent().into()),
            None,
            loopbacks > 0,
        );
        context.device_identity = Some(persona.device_identity());
        context.stealth_profile = profile;
        let (platform, ua_platform, version) = profile.platform();
        context.platform = platform.into();
        context.ua_platform = ua_platform.into();
        context.ua_platform_version = version.into();
        context.language = persona.language.clone().unwrap();
        context.languages = persona.languages.clone().unwrap();
        context.accept_language = persona.accept_language.clone().unwrap();
        context.do_not_track = persona.do_not_track.clone();
        context.webgl_vendor = persona.webgl_vendor.clone().unwrap();
        context.webgl_renderer = persona.webgl_renderer.clone().unwrap();
        let client =
            Arc::get_mut(&mut context.http_client).ok_or(invalid("CONTEXT_ALREADY_SHARED"))?;
        client.block_trackers = persona.tracker_blocking;
        *client.interceptor.write().await =
            Some(std::sync::Arc::new(OriginGuard(init.allowed_origins.clone())));
        self.protocol_version = init.protocol_version;
        self.origins = init.allowed_origins;
        self.mode = init.initial_mode;
        self.persona = Some(persona.clone());
        self.context = Some(Arc::new(context));
        let mut result = json!({"ready": true, "protocol_version": self.protocol_version, "runtime_version": format!("br_{}", &self.sha256[..24]),
            "runtime_sha256": self.sha256, "persona": persona, "device_identity": persona.device_identity(),
            "font_bundle_sha256": env!("AUTOPILOT_FONT_BUNDLE_SHA256"), "mode": self.mode, "generation": self.generation,
            "supported_methods": ["init", "new_page", "navigate", "read_text", "read_value", "read_checked", "fill", "click", "wait", "capture", "set_mode", "begin_recheck", "finish_recheck", "attach_takeover", "close"]});
        if self.uses_automation() {
            result["browser_identity"] = json!({
                "profile": persona.profile,
                "user_agent": profile.user_agent(),
                "transport_profile": match profile {
                    obscura_net::StealthProfile::MacChrome153 => "primp_chrome153_macos",
                    obscura_net::StealthProfile::MacChrome152 => "primp_chrome152_macos",
                    obscura_net::StealthProfile::WindowsChrome145 => "primp_chrome145_windows",
                },
                "chrome152_transport_verified": false,
            });
            result["supported_methods"].as_array_mut().unwrap().extend([json!("automation"), json!("network_body")]);
        }
        Ok(result)
    }

    fn page(&mut self, request: &Request) -> Result<&mut OwnedPage, Error> {
        let entry = self
            .pages
            .get_mut(request.page_id.as_deref().ok_or(invalid("MISSING_PAGE"))?)
            .ok_or(invalid("UNKNOWN_PAGE"))?;
        if Some(entry.generation) != request.page_generation {
            return Err(invalid("STALE_PAGE"));
        }
        if self.protocol_version == "1" && (entry.page.url_string() != entry.url
            || entry
                .page
                .js
                .as_ref()
                .is_some_and(|js| js.has_pending_navigation()))
        {
            return Err(invalid("UNEXPECTED_NAVIGATION"));
        }
        Ok(entry)
    }

    async fn execute(&mut self, request: &Request) -> Result<Value, Error> {
        if request.method == "close" {
            let _: Empty = params(request)?;
            self.revoke_takeover("BROWSER_CLOSED");
            self.takeover = None;
            self.pages.clear();
            self.context = None;
            return Ok(json!({"closed": true}));
        }
        if self.poisoned {
            return Err(invalid("SESSION_POISONED"));
        }
        if request.method == "init" {
            return self.initialize(request).await;
        }
        if self.context.is_none() {
            return Err(invalid("NOT_INITIALIZED"));
        }
        if request.method == "attach_takeover" {
            let _: Empty = params(request)?;
            if self.takeover_attached {
                return Err(invalid("TAKEOVER_ALREADY_ATTACHED"));
            }
            self.takeover_attached = true;
            self.takeover = Some(
                crate::takeover::Channel::connect(&self.workspace)
                    .await
                    .map_err(|_| invalid("TAKEOVER_ATTACH_FAILED"))?,
            );
            return Ok(json!({"attached":true,"capabilities":["control"]}));
        }
        if request.method == "begin_recheck" {
            let check: Recheck = params(request)?;
            if check.generation < self.generation {
                return Err(invalid("STALE_CONTROL"));
            }
            if check.generation == self.generation {
                if self.recheck_generation != Some(check.generation) || self.mode != "PAUSED" {
                    return Err(invalid("CONTROL_CONFLICT"));
                }
            } else {
                self.revoke_takeover("MANUAL_RECHECK");
                self.mode = "PAUSED".into();
                self.generation = check.generation;
                self.recheck_generation = Some(check.generation);
            }
            return Ok(json!({"mode": self.mode, "generation": self.generation}));
        }
        if request.method == "finish_recheck" {
            let check: Recheck = params(request)?;
            if self.recheck_generation != Some(check.generation)
                || self.generation != check.generation
            {
                return Err(invalid("RECHECK_REQUIRED"));
            }
            self.mode = "RUNNING".into();
            return Ok(json!({"mode": self.mode, "generation": self.generation}));
        }
        if request.method == "set_mode" {
            let desired: Mode = params(request)?;
            if !matches!(desired.mode.as_str(), "PAUSED" | "RUNNING") {
                return Err(invalid("INVALID_MODE"));
            }
            if desired.generation < self.generation {
                return Err(invalid("STALE_CONTROL"));
            }
            if desired.generation == self.generation && desired.mode != self.mode {
                return Err(invalid("CONTROL_CONFLICT"));
            }
            if desired.generation > self.generation {
                self.recheck_generation = None;
            }
            if self.manual.is_some() && desired.generation > self.generation {
                self.revoke_takeover("CONTROL_CHANGED");
                if desired.mode == "RUNNING" {
                    return Err(invalid("TAKEOVER_RECHECK_REQUIRED"));
                }
            }
            self.generation = desired.generation;
            self.mode = desired.mode;
            return Ok(json!({"mode": self.mode, "generation": self.generation}));
        }
        if self.mode == "PAUSED"
            && !(request.method == "automation" && request.params.get("operation").and_then(Value::as_str) == Some("close"))
            && matches!(
                request.method.as_str(),
                "new_page" | "navigate" | "fill" | "click" | "wait" | "automation"
            )
        {
            return Err(invalid("BROWSER_PAUSED"));
        }
        if self.protocol_version == "1" && (request.timeout_ms > 30000 || matches!(request.method.as_str(), "automation" | "network_body")) {
            return Err(invalid("PROTOCOL_VERSION_REQUIRED"));
        }
        match request.method.as_str() {
            "automation" => self.automation(request).await,
            "network_body" => self.network_body(request),
            "new_page" => {
                let _: Empty = params(request)?;
                if self.pages.len() >= 4 {
                    return Err(invalid("PAGE_LIMIT"));
                }
                self.page_counter += 1;
                let id = format!("p{}", self.page_counter);
                let mut page = Page::new(id.clone(), self.context.as_ref().unwrap().clone());
                let viewport = &self.persona.as_ref().unwrap().viewport;
                page.set_viewport((viewport.width as f32, viewport.height as f32));
                page.set_device_scale_factor(self.persona.as_ref().unwrap().device_scale_factor.unwrap() as f32);
                page.add_preload_script(&self.persona.as_ref().unwrap().preload_script());
                self.pages.insert(
                    id.clone(),
                    OwnedPage {
                        page,
                        generation: 0,
                        url: "about:blank".into(),
                    },
                );
                if self.protocol_version == "2" { self.observe(&id); }
                Ok(json!({"page_id": id, "page_generation": 0}))
            }
            "navigate" => {
                let navigation: Navigate = params(request)?;
                let url = Url::parse(&navigation.url).map_err(|_| invalid("INVALID_URL"))?;
                if !allowed(&url, &self.origins) {
                    return Err(invalid("ORIGIN_NOT_ALLOWED"));
                }
                let wait = if self.protocol_version == "2" { WaitUntil::DomContentLoaded } else { WaitUntil::Load };
                let entry = self.page(request)?;
                entry.generation += 1;
                entry
                    .page
                    .navigate_with_wait(url.as_str(), wait)
                    .await
                    .map_err(|_| ("NAVIGATION_FAILED", "SENT"))?;
                entry.page.prepare_screenshot_resources(500).await;
                entry.url = entry.page.url_string();
                Ok(
                    json!({"page_id": request.page_id, "page_generation": entry.generation, "url": entry.url}),
                )
            }
            "read_text" => {
                let read: ReadText = params(request)?;
                if read.selector.is_empty() || read.selector.len() > 1024 {
                    return Err(invalid("INVALID_SELECTOR"));
                }
                let page = self.page(request)?;
                let text = page
                    .page
                    .with_dom(|dom| {
                        let node = dom
                            .query_selector(&read.selector)
                            .map_err(|_| invalid("INVALID_SELECTOR"))?
                            .ok_or(invalid("ELEMENT_NOT_FOUND"))?;
                        Ok::<_, Error>(dom.text_content(node))
                    })
                    .ok_or(invalid("NO_DOCUMENT"))??;
                if text.len() > 16384 {
                    return Err(invalid("TEXT_LIMIT"));
                }
                Ok(json!({"text": text}))
            }
            "read_value" => {
                let read: ReadText = params(request)?;
                let entry = self.page(request)?;
                let js = entry.page.js.as_ref().ok_or(invalid("NO_DOCUMENT"))?;
                let node = js.input_node(&read.selector).map_err(invalid)?;
                let value = js
                    .with_dom(|dom| dom.text_control(node))
                    .flatten()
                    .ok_or(invalid("INPUT_ELEMENT_UNSUPPORTED"))?
                    .value;
                if value.len() > 16384 {
                    return Err(invalid("INPUT_VALUE_LIMIT"));
                }
                Ok(json!({"value": value}))
            }
            "read_checked" => {
                let read: ReadText = params(request)?;
                let entry = self.page(request)?;
                let js = entry.page.js.as_ref().ok_or(invalid("NO_DOCUMENT"))?;
                let node = js.input_node(&read.selector).map_err(invalid)?;
                let checked = js
                    .with_dom(|dom| {
                        matches!(dom.input_type(node).as_deref(), Some("checkbox" | "radio"))
                            .then(|| dom.checked_state(node).map(|state| state.checked))
                            .flatten()
                    })
                    .flatten()
                    .ok_or(invalid("INPUT_ELEMENT_UNSUPPORTED"))?;
                Ok(json!({"checked": checked}))
            }
            "click" => {
                let click: Click = params(request)?;
                let origins = self.origins.clone();
                let entry = self.page(request)?;
                let result = entry
                    .page
                    .js
                    .as_mut()
                    .ok_or(invalid("NO_DOCUMENT"))?
                    .native_click(&click.selector)?;
                loop {
                    if let Some(navigation_url) = entry.navigation_url() {
                        if click.navigation == ClickNavigation::None {
                            return Err(("UNEXPECTED_NAVIGATION", "SENT"));
                        }
                        let url =
                            Url::parse(&navigation_url).map_err(|_| ("INVALID_URL", "SENT"))?;
                        if !allowed(&url, &origins) {
                            return Err(("ORIGIN_NOT_ALLOWED", "SENT"));
                        }
                        entry.generation += 1;
                        entry
                            .page
                            .process_pending_navigation()
                            .await
                            .map_err(|_| ("NAVIGATION_FAILED", "SENT"))?;
                        entry.url = entry.page.url_string();
                        let final_url =
                            Url::parse(&entry.url).map_err(|_| ("INVALID_URL", "SENT"))?;
                        if !allowed(&final_url, &origins) {
                            return Err(("ORIGIN_NOT_ALLOWED", "SENT"));
                        }
                        // Same-document commits can queue hashchange and a
                        // callback can navigate again. Observe those tasks
                        // under this RPC deadline without replaying the click.
                        entry.page.settle(20).await;
                        if entry.navigation_url().is_some() {
                            continue;
                        }
                        break;
                    }
                    entry.page.settle(20).await;
                    if click.navigation == ClickNavigation::None {
                        if entry.navigation_url().is_some() {
                            return Err(("UNEXPECTED_NAVIGATION", "SENT"));
                        }
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
                Ok(
                    json!({"page_id": request.page_id, "page_generation": entry.generation,
                    "default_prevented": result.default_prevented}),
                )
            }
            "fill" => {
                let fill: Fill = params(request)?;
                let entry = self.page(request)?;
                let result = entry
                    .page
                    .js
                    .as_mut()
                    .ok_or(invalid("NO_DOCUMENT"))?
                    .native_fill(&fill.selector, &fill.value)?;
                entry.page.settle(20).await;
                let js = entry.page.js.as_ref().ok_or(("NO_DOCUMENT", "SENT"))?;
                if entry.navigation_url().is_some() {
                    return Err(("UNEXPECTED_NAVIGATION", "SENT"));
                }
                if js
                    .input_node(&fill.selector)
                    .map_err(|code| (code, "SENT"))?
                    != result.node
                {
                    return Err(("INPUT_TARGET_CHANGED", "SENT"));
                }
                let current = js
                    .with_dom(|dom| dom.text_control(result.node))
                    .flatten()
                    .ok_or(("INPUT_TARGET_CHANGED", "SENT"))?;
                if current.value != result.value {
                    return Err(("INPUT_VALUE_CHANGED", "SENT"));
                }
                Ok(
                    json!({"changed": result.changed, "length": result.value.encode_utf16().count()}),
                )
            }
            "wait" => {
                let wait: WaitText = params(request)?;
                if wait.selector.is_empty() || wait.selector.len() > 1024 || wait.text.len() > 16384
                {
                    return Err(invalid("INVALID_PARAMS"));
                }
                let entry = self.page(request)?;
                loop {
                    let text = entry
                        .page
                        .with_dom(|dom| {
                            dom.query_selector(&wait.selector)
                                .map(|node| node.map(|node| dom.text_content(node)))
                                .map_err(|_| invalid("INVALID_SELECTOR"))
                        })
                        .ok_or(invalid("NO_DOCUMENT"))??;
                    if text.as_ref().is_some_and(|text| if wait.contains {
                        text.contains(&wait.text)
                    } else {
                        text == &wait.text
                    }) {
                        return Ok(json!({"text": text}));
                    }
                    entry.page.settle(20).await;
                    if entry.navigation_url().is_some() {
                        return Err(("UNEXPECTED_NAVIGATION", "SENT"));
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            }
            "capture" => {
                let _: Empty = params(request)?;
                if self.capture_count >= 16 {
                    return Err(invalid("CAPTURE_QUOTA"));
                }
                let entry = self.page(request)?;
                let png = entry
                    .page
                    .screenshot(entry.page.viewport)
                    .ok_or(invalid("CAPTURE_UNAVAILABLE"))?;
                if png.len() > 2 * 1024 * 1024 {
                    return Err(invalid("CAPTURE_LIMIT"));
                }
                if self.capture_bytes + png.len() > 16 * 1024 * 1024 {
                    return Err(invalid("CAPTURE_QUOTA"));
                }
                let directory = self.workspace.join("captures");
                if !directory.exists() {
                    fs::DirBuilder::new()
                        .mode(0o700)
                        .create(&directory)
                        .map_err(|_| invalid("CAPTURE_IO_FAILED"))?;
                }
                if directory.canonicalize().ok().as_ref() != Some(&directory) {
                    return Err(invalid("CAPTURE_PATH_INVALID"));
                }
                let relative = format!("captures/capture-{}.png", request.id);
                self.capture_count += 1;
                self.capture_bytes += png.len();
                let mut file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(self.workspace.join(&relative))
                    .map_err(|_| invalid("CAPTURE_IO_FAILED"))?;
                file.write_all(&png)
                    .and_then(|_| file.sync_all())
                    .map_err(|_| invalid("CAPTURE_IO_FAILED"))?;
                Ok(
                    json!({"path": relative, "sha256": format!("{:x}", Sha256::digest(&png)), "bytes": png.len(), "mime_type": "image/png"}),
                )
            }
            _ => Err(invalid("METHOD_UNAVAILABLE")),
        }
    }
}
