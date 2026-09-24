use super::support::*;

struct FixtureResponse;

#[async_trait::async_trait]
impl RequestInterceptor for FixtureResponse {
    async fn intercept(&self, request: &RequestInfo) -> InterceptAction {
        InterceptAction::Fulfill(Response {
            status: 200,
            url: request.url.clone(),
            headers: HashMap::new(),
            body: b"fixture".to_vec(),
            redirected_from: vec![],
            raw_headers: None,
            request_raw_headers: None,
            request_referrer: None,
        })
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

#[test]
fn macos_identity_is_inherited_by_frame() {
    let mut rt = obscura_js::runtime::ObscuraJsRuntime::new(
        obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::MacChrome152),
    );
    rt.set_dom(obscura_dom::parse_html("<!doctype html><body>identity</body>"));
    rt.set_url("http://127.0.0.1/identity");
    rt.run_page_init();
    let script = "[navigator.userAgent,navigator.platform,JSON.stringify(navigator.userAgentData.brands)]";
    let expected = rt.evaluate(script).unwrap();
    assert_eq!(
        rt.evaluate("devicePixelRatio").unwrap().as_f64(),
        Some(2.0),
    );
    let child = obscura_js::frame::FrameRealm::new(&mut rt, 1, 0, "http://127.0.0.1/child", "<!doctype html><body>child</body>").unwrap();
    assert_eq!(child.evaluate(&mut rt, script).unwrap(), expected);
    assert_eq!(
        child
            .evaluate(&mut rt, "devicePixelRatio")
            .unwrap()
            .as_f64(),
        Some(2.0),
    );
    let high = "navigator.userAgentData.getHighEntropyValues(['architecture','uaFullVersion']).then(v=>globalThis.identityResult=[v.architecture,v.uaFullVersion])";
    child.evaluate(&mut rt, high).unwrap();
    // Frame microtasks settle when returning from the V8 call.
    assert_eq!(child.evaluate(&mut rt, "identityResult").unwrap(), json!(["arm","152.0.7977.83"]));
}

#[test]
fn native_persona_seed_and_frame_identity_are_stable() {
    let spec: obscura_net::PersonaSpec = serde_json::from_value(json!({
        "schema_version":"1", "persona_id":"fixture_windows145", "revision":"1",
        "profile":"windows_chrome145", "viewport":{"width":640,"height":480}
    }))
    .unwrap();
    let persona = spec.clone().compile().unwrap();
    let identity = obscura_browser::DeviceIdentity {
        seed: persona.seed(),
        hardware_concurrency: persona.hardware_concurrency(),
        device_memory: persona.device_memory(),
        screen_width: persona.screen_width(),
        screen_height: persona.screen_height(),
        screen_color_depth: persona.screen_color_depth(),
    };
    let mut changed = spec.clone();
    changed.revision = "2".into();
    assert_ne!(identity.seed, changed.compile().unwrap().seed());
    let mut changed_viewport = spec.clone();
    changed_viewport.viewport.as_mut().unwrap().width = 800;
    assert_eq!(identity.seed, changed_viewport.compile().unwrap().seed());
    let mut macos = spec.clone();
    macos.profile = "macos_chrome152".into();
    let macos = macos.compile().unwrap();
    assert_eq!(macos.hardware_concurrency(), 15);
    assert_eq!(macos.device_memory(), 32.0);
    assert_eq!(macos.language(), "en");
    assert_eq!(macos.languages(), ["en", "zh-CN"]);
    assert_eq!(macos.accept_language(), "en,zh-CN;q=0.9,zh;q=0.8");
    assert_eq!(macos.timezone(), "Asia/Shanghai");
    assert_eq!(macos.do_not_track(), None);
    assert_eq!((macos.screen_width(), macos.screen_height()), (2560, 1440));
    assert_eq!((macos.screen_avail_width(), macos.screen_avail_height()), (2560, 1320));
    assert_eq!((macos.outer_width(), macos.outer_height()), (640, 480));
    assert_eq!(macos.device_scale_factor(), 2.0);
    assert_eq!((macos.battery_charging(), macos.battery_level()), (true, 0.8));
    assert_eq!(macos.network_rtt(), 100);
    assert_eq!(macos.storage_quota(), 10_738_064_711);
    assert_eq!(macos.webgl_vendor(), "Google Inc. (Apple)");
    assert!(macos.webgl_renderer().contains("Apple M5 Pro"));
    let mut custom = spec;
    custom.language = Some("fr-CA".into());
    custom.languages = None;
    custom.accept_language = None;
    let custom = custom.compile().unwrap();
    assert_eq!(custom.languages(), ["fr-CA", "fr"]);
    assert_eq!(custom.accept_language(), "fr-CA,fr;q=0.9");
    let snapshot = "[navigator.hardwareConcurrency,navigator.deviceMemory,screen.width,screen.height,screen.availWidth,screen.availHeight]";
    for _ in 0..2 {
        let mut rt = obscura_js::runtime::ObscuraJsRuntime::new(persona.clone());
        rt.set_dom(obscura_dom::parse_html(
            "<!doctype html><body>PERSONA</body>",
        ));
        rt.set_url("http://127.0.0.1/persona");
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

#[test]
fn startup_persona_drives_the_primp_transport_profile() {
    let spec: obscura_net::PersonaSpec = serde_json::from_value(json!({
        "schema_version":"1", "persona_id":"fixture_macos153", "revision":"1",
        "profile":"macos_chrome153", "viewport":{"width":640,"height":480},
        "language":"zh-CN", "languages":["zh-CN","zh"],
        "accept_language":"zh-CN,zh;q=0.9", "do_not_track":"1"
    }))
    .unwrap();
    let persona = spec.compile().unwrap();
    let context = BrowserContext::new("persona".into(), persona.clone());
    let page = Page::new("persona-page".into(), Arc::new(context));
    let transport = page.stealth_client.transport_params();

    assert_eq!(transport.profile, obscura_net::StealthProfile::MacChrome153);
    assert_eq!(transport.accept_language.as_deref(), Some(persona.accept_language()));
    assert_eq!(transport.do_not_track.as_deref(), persona.do_not_track());
    assert_eq!(page.context.persona().user_agent(), transport.profile.user_agent());
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
        let stealth = StealthHttpClient::with_policy(
            cookies,
            None,
            Arc::new(policy),
            &obscura_net::EffectivePersona::builtin(
                obscura_net::StealthProfile::WindowsChrome145,
            ),
        );
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
    let stealth = StealthHttpClient::with_policy(
        cookies,
        None,
        Arc::new(policy),
        &obscura_net::EffectivePersona::builtin(
            obscura_net::StealthProfile::WindowsChrome145,
        ),
    );
    assert_eq!(stealth.fetch(&url).await.unwrap().status, 200);
    assert!(server
        .join()
        .unwrap()
        .to_lowercase()
        .contains("x-fixture: request-only"));
    assert!(stealth.extra_headers.read().await.is_empty());
}

#[tokio::test(flavor = "current_thread")]
async fn document_fragment_redirect_inheritance_matches_persona_transports() {
    for profile in [obscura_net::StealthProfile::WindowsChrome145, obscura_net::StealthProfile::MacChrome153] {
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
            let persona = obscura_net::EffectivePersona::builtin(profile);
            let response = StealthHttpClient::with_policy(cookies, None, client, &persona)
                .fetch(&url)
                .await
                .unwrap();
            assert_eq!(response.url.fragment(), expected);
            assert_eq!(response.url.path(), "/final");
            assert_eq!(
                server.join().unwrap(),
                vec!["GET /start HTTP/1.1\r\n", "GET /final HTTP/1.1\r\n"]
            );
        }
    }
}
