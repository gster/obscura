use super::support::*;

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
            raw_headers: None,
            request_raw_headers: None,
            request_referrer: None,
        })
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
            raw_headers: None,
            request_raw_headers: None,
            request_referrer: None,
        })
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
            "history-redirect".into(), obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145),
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

#[tokio::test(flavor = "current_thread")]
async fn session_history_failed_and_no_content_traversals_do_not_move_cursor() {
    for failure in [0, 204, 205] {
        let gate = std::sync::Arc::new(std::sync::atomic::AtomicU16::new(200));
        let mut context = BrowserContext::with_storage_and_network(
            "history-failure".into(), obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145),
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
