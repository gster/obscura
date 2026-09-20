use super::support::*;

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
    assert_eq!(pixel(&page, 50, 10), [0, 0, 255, 255]);
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
