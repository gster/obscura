use super::*;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct Client {
    tx: mpsc::UnboundedSender<ServerMessage>,
    replies: mpsc::UnboundedReceiver<String>,
    reply_tx: mpsc::UnboundedSender<String>,
    events: Vec<Value>,
    id: u64,
}

impl Client {
    async fn recv(&mut self) -> Value {
        let text = tokio::time::timeout(std::time::Duration::from_secs(10), self.replies.recv())
            .await.expect("CDP message timeout").expect("processor stopped");
        serde_json::from_str(&text).unwrap()
    }

    async fn command(&mut self, session: Option<&str>, method: &str, params: Value) -> Value {
        self.id += 1;
        self.tx.send(ServerMessage::Cdp(CdpMessage {
            text: json!({"id":self.id,"sessionId":session,"method":method,"params":params}).to_string(),
            reply_tx: self.reply_tx.clone(),
        })).unwrap();
        loop {
            let value = self.recv().await;
            if value["id"] == self.id { return value; }
            self.events.push(value);
        }
    }

    async fn ok(&mut self, session: Option<&str>, method: &str, params: Value) -> Value {
        let response = self.command(session, method, params).await;
        assert!(response.get("error").is_none(), "{response}");
        response["result"].clone()
    }

    async fn pause(&mut self, session: &str) -> Value {
        loop {
            if let Some(index) = self.events.iter().position(|event|
                event["method"] == "Fetch.requestPaused" && event["sessionId"] == session
                && event["params"]["requestId"].as_str().is_some_and(|id| id.starts_with("intercept-"))) {
                return self.events.remove(index);
            }
            let event = self.recv().await;
            self.events.push(event);
        }
    }

    async fn start_fetch(&mut self, session: &str, url: &str, worker: bool) {
        let fetch = format!("fetch({url:?}, {{headers:{{Authorization:'Bearer complete-secret'}}}}).then(r=>r.text()).catch(()=> 'failed')");
        let expression = if worker {
            let source = format!("{fetch}.then(postMessage)");
            format!("globalThis.worker = new Worker(URL.createObjectURL(new Blob([{source:?}], {{type:'application/javascript'}}))); globalThis.result = new Promise(resolve => worker.onmessage=e=>resolve(e.data)); 'started'")
        } else { format!("globalThis.result = {fetch}; 'started'") };
        self.ok(Some(session), "Runtime.evaluate", json!({"expression":expression,"returnByValue":true})).await;
    }

    async fn result(&mut self, session: &str) -> Value {
        self.ok(Some(session), "Runtime.evaluate", json!({"expression":"globalThis.result","awaitPromise":true,"returnByValue":true})).await["result"]["value"].clone()
    }
}

async fn fixture() -> (String, tokio::task::JoinHandle<()>) {
    let (base, task, _) = fixture_with_requests().await;
    (base, task)
}

async fn fixture_with_requests() -> (String, tokio::task::JoinHandle<()>, Arc<std::sync::Mutex<Vec<String>>>) {
    let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured = requests.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let cross_origin = format!("http://localhost:{}", listener.local_addr().unwrap().port());
    let task = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let captured = captured.clone();
            let cross_origin = cross_origin.clone();
            tokio::spawn(async move {
                let mut request = Vec::new();
                loop {
                    let mut buf = [0; 2048];
                    let count = socket.read(&mut buf).await.unwrap();
                    if count == 0 { return; }
                    request.extend_from_slice(&buf[..count]);
                    if request.windows(4).any(|s| s == b"\r\n\r\n") { break; }
                }
                let request = String::from_utf8(request).unwrap();
                let path = request.split_whitespace().nth(1).unwrap();
                captured.lock().unwrap().push(path.to_string());
                let body = if path == "/" { "<html>ready</html>" } else { path };
                let status = if path == "/redirect-preflight" {
                    format!("302 Found\r\nLocation: {cross_origin}/final")
                } else if path.starts_with("/redirect/") { "302 Found\r\nLocation: /final".into() } else { "200 OK".into() };
                let response = format!("HTTP/1.1 {status}\r\nContent-Type: text/html\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Headers: authorization\r\nSet-Cookie: session=complete-secret; Path=/\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body);
                socket.write_all(response.as_bytes()).await.unwrap();
            });
        }
    });
    (base, task, requests)
}

async fn client() -> (Client, tokio::task::JoinHandle<()>) {
    let (tx, rx) = mpsc::unbounded_channel();
    let (reply_tx, replies) = mpsc::unbounded_channel();
    let context = Arc::new(obscura_browser::BrowserContext::with_storage_and_network(
        "multi-page-pause".into(), None, true, None, None, true,
    ));
    let processor = tokio::task::spawn_local(cdp_processor(rx, context, Arc::new(Notify::new())));
    tx.send(ServerMessage::NewConnection { reply_tx: reply_tx.clone() }).unwrap();
    let mut client = Client { tx, replies, reply_tx, events: Vec::new(), id: 0 };
    assert_eq!(client.recv().await["__init"], true);
    (client, processor)
}

async fn page(client: &mut Client, base: &str) -> (String, String) {
    let target = client.ok(None, "Target.createTarget", json!({"url":format!("{base}/")})).await["targetId"].as_str().unwrap().to_string();
    let session = client.ok(None, "Target.attachToTarget", json!({"targetId":target,"flatten":true})).await["sessionId"].as_str().unwrap().to_string();
    client.ok(Some(&session), "Fetch.enable", json!({})).await;
    (target, session)
}

#[tokio::test(flavor = "current_thread")]
async fn active_multi_page_fetch_pauses_keep_session_ownership_and_body_aliases() {
    tokio::task::LocalSet::new().run_until(async {
        let (base, fixture_task) = fixture().await;
        let (mut client, processor) = client().await;
        let (left_page, left) = page(&mut client, &base).await;
        let (right_page, right) = page(&mut client, &base).await;
        let sibling = client.ok(None, "Target.attachToTarget", json!({"targetId":left_page,"flatten":true})).await["sessionId"].as_str().unwrap().to_string();
        assert!(client.command(Some(&sibling), "Fetch.enable", json!({})).await.get("error").is_some());

        for (round, worker) in [(1, false), (3, true), (5, false)] {
            client.start_fetch(&left, &format!("{base}/left-{round}"), worker).await;
            client.start_fetch(&right, &format!("{base}/right-{round}"), worker).await;
            let a = client.pause(&left).await;
            let b = client.pause(&right).await;
            let id = a["params"]["requestId"].as_str().unwrap();
            assert_eq!(a["params"]["requestId"], b["params"]["requestId"], "real per-Page counters must collide: round={round} a={a} b={b}");
            assert_eq!(id, format!("intercept-{round}"));
            assert_ne!(a["params"]["frameId"], b["params"]["frameId"]);
            assert_eq!(a["params"]["request"]["headers"]["Authorization"], "Bearer complete-secret");
            assert!(a["params"]["request"]["url"].as_str().unwrap().ends_with(&format!("left-{round}")));
            for method in ["Fetch.continueRequest", "Fetch.fulfillRequest", "Fetch.failRequest", "Fetch.getResponseBody", "Fetch.takeResponseBodyAsStream"] {
                for bad in [None, Some("unknown"), Some(sibling.as_str())] {
                    assert!(client.command(bad, method, json!({"requestId":id})).await.get("error").is_some(), "{method} {bad:?}");
                }
            }
            for session in [&left, &right] {
                for method in ["Fetch.getResponseBody", "Fetch.takeResponseBodyAsStream"] {
                    let response = client.command(Some(session), method, json!({"requestId":id})).await;
                    assert!(response["error"]["message"].as_str().unwrap().contains("response_body_not_ready"), "{response}");
                }
            }
            for _ in 0..2 {
                for (method, field) in [("Fetch.continueRequest", "postData"), ("Fetch.continueRequest", "headers"), ("Fetch.fulfillRequest", "body")] {
                    let mut params = json!({"requestId":id}); params[field] = json!("%");
                    let response = client.command(Some(&left), method, params).await;
                    assert_eq!(response["error"]["code"], -32602);
                }
            }
            if round == 1 {
                client.ok(Some(&left), "Fetch.continueRequest", json!({"requestId":id,"headers":super::tests::continue_header_fields()})).await;
                assert_eq!(client.result(&left).await, "/left-1");
                // Resolving left must leave the right resolver alive.
                client.ok(Some(&right), "Fetch.continueRequest", json!({"requestId":id})).await;
                assert_eq!(client.result(&right).await, "/right-1");
            } else if round == 5 {
                for session in [&left, &right] {
                    client.ok(Some(session), "Fetch.failRequest", json!({"requestId":id,"errorReason":"Aborted"})).await;
                    assert_eq!(client.result(session).await, "failed");
                }
            } else {
                client.ok(Some(&left), "Fetch.fulfillRequest", json!({"requestId":id,"responseCode":200,"body":"bGVmdC1zZWNyZXQ="})).await;
                assert_eq!(client.result(&left).await, "left-secret");
                // The right pause must not shadow the left completed alias.
                assert_eq!(client.ok(Some(&left), "Fetch.getResponseBody", json!({"requestId":id})).await["body"], "left-secret");
                assert!(client.command(None, "Network.getResponseBody", json!({"requestId":id})).await.get("error").is_some());
                client.ok(Some(&right), "Fetch.fulfillRequest", json!({"requestId":id,"responseCode":200,"body":"cmlnaHQtc2VjcmV0"})).await;
                assert_eq!(client.result(&right).await, "right-secret");
                for method in ["Network.getResponseBody", "Fetch.getResponseBody"] {
                    assert_eq!(client.ok(Some(&right), method, json!({"requestId":id})).await["body"], "right-secret");
                    assert!(client.command(None, method, json!({"requestId":id})).await.get("error").is_some());
                }
                let stream = client.ok(Some(&left), "Fetch.takeResponseBodyAsStream", json!({"requestId":id})).await["stream"].clone();
                assert_eq!(client.ok(Some(&left), "IO.read", json!({"handle":stream})).await["data"], "bGVmdC1zZWNyZXQ=");
                client.ok(Some(&left), "IO.close", json!({"handle":stream})).await;
                assert!(client.command(Some(&left), "Fetch.getResponseBody", json!({"requestId":id})).await["error"]["message"].as_str().unwrap().contains("consumed"));
                assert_eq!(client.ok(Some(&right), "Fetch.getResponseBody", json!({"requestId":id})).await["body"], "right-secret");
            }
        }
        // Disable only the owning session, leaving the other Page paused.
        client.start_fetch(&left, &format!("{base}/left-disable"), false).await;
        client.start_fetch(&right, &format!("{base}/right-disable"), false).await;
        let a = client.pause(&left).await;
        let b = client.pause(&right).await;
        assert_eq!(a["params"]["requestId"], b["params"]["requestId"]);
        assert!(client.command(Some(&sibling), "Fetch.disable", json!({})).await.get("error").is_some());
        client.ok(Some(&left), "Fetch.disable", json!({})).await;
        assert_eq!(client.result(&left).await, "/left-disable");
        let response = client.command(Some(&right), "Fetch.getResponseBody", json!({"requestId":b["params"]["requestId"]})).await;
        assert!(response["error"]["message"].as_str().unwrap().contains("not_ready"));
        // Detach aborts the old owner and permits a new flattened owner.
        client.ok(None, "Target.detachFromTarget", json!({"sessionId":right})).await;
        let new_right = client.ok(None, "Target.attachToTarget", json!({"targetId":right_page,"flatten":true})).await["sessionId"].as_str().unwrap().to_string();
        assert_eq!(client.result(&new_right).await, "failed");
        client.ok(Some(&new_right), "Fetch.enable", json!({})).await;
        client.start_fetch(&new_right, &format!("{base}/right-reattached"), false).await;
        client.pause(&new_right).await;
        client.ok(None, "Target.closeTarget", json!({"targetId":right_page})).await;
        // Close with a real active resolver; processor cleanup cannot hang.
        client.ok(Some(&left), "Fetch.enable", json!({})).await;
        client.start_fetch(&left, &format!("{base}/left-close"), false).await;
        let left_pause = client.pause(&left).await;
        let context = client.ok(None, "Target.createBrowserContext", json!({})).await["browserContextId"].clone();
        let target = client.ok(None, "Target.createTarget", json!({"url":format!("{base}/"),"browserContextId":context})).await["targetId"].clone();
        let disposable = client.ok(None, "Target.attachToTarget", json!({"targetId":target,"flatten":true})).await["sessionId"].as_str().unwrap().to_string();
        client.ok(Some(&disposable), "Fetch.enable", json!({})).await;
        client.start_fetch(&disposable, &format!("{base}/disposed"), false).await;
        client.pause(&disposable).await;
        client.ok(None, "Target.disposeBrowserContext", json!({"browserContextId":context})).await;
        assert!(client.command(Some(&disposable), "Fetch.getResponseBody", json!({"requestId":"intercept-1"})).await.get("error").is_some());
        assert!(client.command(Some(&left), "Fetch.getResponseBody", json!({"requestId":left_pause["params"]["requestId"]})).await["error"]["message"].as_str().unwrap().contains("not_ready"));
        drop(client);
        tokio::time::timeout(std::time::Duration::from_secs(3), processor).await.expect("close releases active pauses").unwrap();
        fixture_task.abort();
    }).await;
}

#[tokio::test(flavor = "current_thread")]
async fn single_page_sessionless_pause_resolution_and_navigation_disconnect() {
    tokio::task::LocalSet::new().run_until(async {
        let (base, fixture_task) = fixture().await;
        let (mut client, processor) = client().await;
        let (_, session) = page(&mut client, &base).await;
        client.start_fetch(&session, &format!("{base}/single"), false).await;
        let pause = client.pause(&session).await;
        let id = pause["params"]["requestId"].clone();
        for method in ["Fetch.getResponseBody", "Fetch.takeResponseBodyAsStream"] {
            assert!(client.command(None, method, json!({"requestId":id})).await["error"]["message"].as_str().unwrap().contains("not_ready"));
        }
        client.ok(None, "Fetch.fulfillRequest", json!({"requestId":id,"body":"c2luZ2xl"})).await;
        assert_eq!(client.result(&session).await, "single");
        assert_eq!(client.ok(None, "Fetch.getResponseBody", json!({"requestId":id})).await["body"], "single");
        client.ok(None, "Fetch.disable", json!({})).await;
        client.ok(None, "Fetch.enable", json!({})).await;
        client.start_fetch(&session, &format!("{base}/sessionless-owner"), false).await;
        let pause = loop {
            if let Some(index) = client.events.iter().position(|event| event["method"] == "Fetch.requestPaused"
                && event["sessionId"].is_null() && event["params"]["requestId"].as_str().is_some_and(|id| id.starts_with("intercept-"))) {
                break client.events.remove(index);
            }
            let event = client.recv().await; client.events.push(event);
        };
        let id = pause["params"]["requestId"].clone();
        // An explicit session cannot claim a pause enabled without a session.
        assert!(client.command(Some(&session), "Fetch.failRequest", json!({"requestId":id})).await.get("error").is_some());
        client.ok(None, "Fetch.continueRequest", json!({"requestId":id})).await;
        assert_eq!(client.result(&session).await, "/sessionless-owner");
        client.ok(None, "Fetch.disable", json!({})).await;
        client.ok(Some(&session), "Fetch.enable", json!({})).await;
        // A script pauses while its Page is temporarily removed from ctx.pages
        // by the navigation task. Route using the enable owner, not ctx.pages.
        let html = format!("<script>globalThis.result=fetch('{base}/during-nav').then(r=>r.text())</script>");
        client.id += 1;
        client.tx.send(ServerMessage::Cdp(CdpMessage {
            text:json!({"id":client.id,"method":"Page.navigate","sessionId":session,"params":{"url":format!("data:text/html,{html}")}}).to_string(),
            reply_tx:client.reply_tx.clone(),
        })).unwrap();
        let pause = client.pause(&session).await;
        assert_eq!(pause["params"]["requestId"], "intercept-5", "navigation retains Page pause and capture counter");
        drop(client);
        tokio::time::timeout(std::time::Duration::from_secs(3), processor).await.expect("close during navigation must release pause").unwrap();
        fixture_task.abort();
    }).await;
}

#[tokio::test(flavor = "current_thread")]
async fn navigation_keeps_page_count_and_sessionless_owner_stable() {
    tokio::task::LocalSet::new().run_until(async {
        let (base, fixture_task) = fixture().await;
        let (mut client, processor) = client().await;
        let (_, left) = page(&mut client, &base).await;
        client.ok(Some(&left), "Fetch.disable", json!({})).await;
        client.ok(None, "Fetch.enable", json!({})).await;
        let (_, right) = page(&mut client, &base).await;
        client.start_fetch(&right, &format!("{base}/right-wait"), false).await;
        let right_pause = client.pause(&right).await;
        client.id += 1;
        let html = format!("<script>globalThis.result=fetch('{base}/left-nav').then(r=>r.text())</script>");
        client.tx.send(ServerMessage::Cdp(CdpMessage {
            text:json!({"id":client.id,"method":"Page.navigate","sessionId":left,"params":{"url":format!("data:text/html,{html}")}}).to_string(),
            reply_tx:client.reply_tx.clone(),
        })).unwrap();
        loop {
            let event = client.recv().await;
            if event["method"] == "Fetch.requestPaused" && event["sessionId"].is_null()
                && event["params"]["request"]["url"].as_str().is_some_and(|url| url.ends_with("/left-nav")) { break; }
            client.events.push(event);
        }
        for method in ["Fetch.disable", "Fetch.continueRequest", "Fetch.failRequest", "Fetch.fulfillRequest", "Fetch.getResponseBody", "Fetch.takeResponseBodyAsStream"] {
            assert!(client.command(None, method, json!({"requestId":right_pause["params"]["requestId"]})).await.get("error").is_some());
        }
        assert!(client.command(Some(&right), "Fetch.getResponseBody", json!({"requestId":right_pause["params"]["requestId"]})).await["error"]["message"].as_str().unwrap().contains("not_ready"));
        drop(client);
        tokio::time::timeout(std::time::Duration::from_secs(3), processor).await.expect("both navigating and live pauses release on close").unwrap();
        fixture_task.abort();
    }).await;
}

#[tokio::test(flavor = "current_thread")]
async fn queued_fetches_abort_before_disconnect_without_transport() {
    tokio::task::LocalSet::new().run_until(async {
        let (base, fixture_task, requests) = fixture_with_requests().await;
        let (mut client, processor) = client().await;
        let (_, session) = page(&mut client, &base).await;
        client.id += 1;
        client.tx.send(ServerMessage::Cdp(CdpMessage {
            text:json!({"id":client.id,"method":"Runtime.evaluate","sessionId":session,
                "params":{"expression":format!("fetch('{base}/queued-a').catch(()=>{{}});fetch('{base}/queued-b').catch(()=>{{}}); 'queued'"),"returnByValue":true}}).to_string(),
            reply_tx:client.reply_tx.clone(),
        })).unwrap();
        // rx contains evaluate followed immediately by closure. The processor's
        // biased command branch runs it before observing any routed pauses.
        let replies = client.replies;
        drop(client.tx);
        drop(client.reply_tx);
        tokio::time::timeout(std::time::Duration::from_secs(3), processor).await.expect("queued close must complete").unwrap();
        let mut replies = replies;
        while let Ok(reply) = replies.try_recv() {
            let value: Value = serde_json::from_str(&reply).unwrap();
            assert_ne!(value["method"], "Fetch.requestPaused", "close preceded pause emission");
        }
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        assert_eq!(*requests.lock().unwrap(), vec!["/"], "unannounced pauses must not reach HTTP");
        fixture_task.abort();
    }).await;
}

#[tokio::test(flavor = "current_thread")]
async fn closed_pause_relay_aborts_real_fetch_without_transport() {
    let (base, fixture_task, requests) = fixture_with_requests().await;
    let context = Arc::new(obscura_browser::BrowserContext::with_storage_and_network(
        "closed-relay".into(), None, true, None, None, true,
    ));
    let mut ctx = CdpContext::new_with_shared_context(context);
    let page_id = ctx.create_page();
    let session = Some("closed-relay-session".to_string());
    ctx.sessions.insert(session.clone().unwrap(), page_id.clone());
    ctx.get_page_mut(&page_id).unwrap().navigate(&format!("{base}/")).await.unwrap();
    let (tx, rx) = mpsc::unbounded_channel();
    ctx.intercept_tx = Some(tx);
    drop(rx);
    crate::domains::fetch::handle("enable", &json!({}), &mut ctx, &session).await.unwrap();
    let result = ctx.get_page_mut(&page_id).unwrap().evaluate_for_cdp(
        &format!("Promise.all([fetch('{base}/closed-a').then(()=> 'sent', ()=> 'aborted'), fetch('{base}/closed-b').then(()=> 'sent', ()=> 'aborted')])"), true, true,
    ).await;
    assert_eq!(result.value, Some(json!(["aborted", "aborted"])));
    assert_eq!(*requests.lock().unwrap(), vec!["/"], "relay send failures must fail closed");
    fixture_task.abort();
}

#[test]
fn closed_reply_channel_aborts_instead_of_registering_a_pause() {
    let (reply_tx, reply_rx) = mpsc::unbounded_channel();
    drop(reply_rx);
    let (resolver, mut resolved) = tokio::sync::oneshot::channel();
    let request = obscura_js::ops::InterceptedRequest {
                document_generation: 0, document_url: "https://example.test/".into(), redirect_response: None,
                network_id: "fixture-network-id".into(),
                network_start: Arc::new(std::sync::atomic::AtomicU8::new(0)),
                request_raw_headers: None, request_body_size: 0,
        request_id: "intercept-1".into(), url: "https://example.test/".into(), method: "GET".into(),
        headers: HashMap::new(), resource_type: "Fetch".into(), resolver,
    };
    let mut paused = InterceptedPauses::new();
    emit_intercepted_request(request, "frame", "loader", "https://example.test/", Some("session".into()), &reply_tx, &mut paused);
    assert!(paused.is_empty());
    assert!(matches!(resolved.try_recv(), Ok(obscura_js::ops::InterceptResolution::Fail { reason }) if reason == "Aborted"));
}

#[test]
fn closed_routed_pause_does_not_restore_a_retired_network_owner() {
    let mut ctx = CdpContext::new();
    let page_id = ctx.create_page();
    let session = Some("closed-route-session".to_string());
    ctx.sessions.insert(session.clone().unwrap(), page_id.clone());
    ctx.fetch_intercept.owners.insert(page_id.clone(), session.clone());
    let (resolver, resolved) = tokio::sync::oneshot::channel();
    drop(resolved);
    let request = obscura_js::ops::InterceptedRequest {
        document_generation: 0, document_url: "https://example.test/".into(), redirect_response: None,
        network_id: "retired-network-id".into(),
        network_start: Arc::new(std::sync::atomic::AtomicU8::new(0)),
        request_raw_headers: None, request_body_size: 0,
        request_id: "intercept-retired".into(), url: "https://example.test/".into(), method: "GET".into(),
        headers: HashMap::new(), resource_type: "Fetch".into(), resolver,
    };
    let routed = crate::domains::fetch::RoutedInterceptedRequest {
        page_id: page_id.clone(), frame_id: "frame".into(), session_id: session, request,
    };
    let (reply_tx, mut replies) = mpsc::unbounded_channel();
    let mut paused = InterceptedPauses::new();
    emit_routed_intercepted_request(routed, &mut ctx, &reply_tx, &mut paused);
    assert!(paused.is_empty());
    assert!(ctx.network_owners.is_empty());
    assert!(replies.try_recv().is_err());
}

#[tokio::test(flavor = "current_thread")]
async fn failure_observation_native_abort_retires_cdp_pause_and_keeps_session() {
    tokio::task::LocalSet::new().run_until(async {
        let (base, fixture_task, requests) = fixture_with_requests().await;
        let (mut client, processor) = client().await;
        let (_, session) = page(&mut client, &base).await;
        client.ok(Some(&session), "Runtime.evaluate", json!({"expression":format!(
            "globalThis.c=new AbortController(); globalThis.result=fetch({:?},{{signal:c.signal}}).catch(e=>e.name); 'started'", format!("{base}/abort")), "returnByValue":true})).await;
        let pause = client.pause(&session).await;
        let id = pause["params"]["requestId"].as_str().unwrap();
        let network_id = pause["params"]["networkId"].as_str().unwrap();
        assert_ne!(id, network_id);
        assert!(client.events.iter().any(|event| event["method"] == "Network.requestWillBeSent" && event["params"]["requestId"] == network_id && event["sessionId"] == session));
        client.ok(Some(&session), "Runtime.evaluate", json!({"expression":"c.abort(); 'aborted'", "returnByValue":true})).await;
        assert_eq!(client.result(&session).await, json!("AbortError"));
        for method in ["Fetch.continueRequest", "Fetch.failRequest", "Fetch.fulfillRequest"] {
            assert!(client.command(Some(&session), method, json!({"requestId":id,"errorReason":"Failed","responseCode":200})).await.get("error").is_some());
        }
        while !client.events.iter().any(|event| event["method"] == "Network.loadingFailed" && event["params"]["requestId"] == network_id) {
            let event = client.recv().await; client.events.push(event);
        }
        let terminal = client.events.iter().filter(|event| event["params"]["requestId"] == network_id &&
            (event["method"] == "Network.loadingFailed" || event["method"] == "Network.loadingFinished")).collect::<Vec<_>>();
        assert_eq!(terminal.len(), 1);
        assert_eq!(terminal[0]["method"], "Network.loadingFailed");
        assert_eq!(terminal[0]["sessionId"], session);
        assert_eq!(*requests.lock().unwrap(), vec!["/"]);
        drop(client); processor.await.unwrap(); fixture_task.abort();
    }).await;
}


#[tokio::test(flavor = "current_thread")]
async fn failure_observation_redirect_pauses_each_hop_and_keeps_owner_headers_and_body() {
    tokio::task::LocalSet::new().run_until(async {
        let (base, fixture_task, requests) = fixture_with_requests().await;
        let (mut client, processor) = client().await;
        let (target, owner) = page(&mut client, &base).await;
        let other = client.ok(None, "Target.attachToTarget", json!({"targetId":target,"flatten":true})).await["sessionId"].as_str().unwrap().to_string();
        for fail in [false, true] {
            let first_url=format!("{base}/redirect/{}",if fail {"failure"}else{"success"});
            client.start_fetch(&owner,&first_url,false).await;
            let first=client.pause(&owner).await;
            let network=first["params"]["networkId"].clone();
            client.ok(Some(&owner),"Fetch.continueRequest",json!({"requestId":first["params"]["requestId"]})).await;
            let second=client.pause(&owner).await;
            assert_eq!(second["params"]["networkId"],network);
            assert_ne!(second["params"]["requestId"],first["params"]["requestId"]);
            let starts=client.events.iter().filter(|e|e["method"]=="Network.requestWillBeSent" && e["params"]["requestId"]==network).collect::<Vec<_>>();
            assert_eq!(starts.len(),2);
            assert_eq!(starts[1]["params"]["redirectResponse"]["status"],302);
            assert_eq!(starts[1]["params"]["redirectResponse"]["url"],first_url);
            assert!(starts.iter().all(|e|e["params"]["loaderId"].as_str().is_some_and(|id|!id.is_empty()) && e["params"]["documentURL"]==format!("{base}/")));
            let hop_body=starts[1]["params"]["redirectResponse"]["bodyRequestId"].clone();
            assert!(client.command(Some(&other),"Fetch.failRequest",json!({"requestId":second["params"]["requestId"]})).await.get("error").is_some());
            if fail {
                client.ok(Some(&owner),"Fetch.failRequest",json!({"requestId":second["params"]["requestId"],"errorReason":"BlockedByClient"})).await;
            } else {
                client.ok(Some(&owner),"Fetch.continueRequest",json!({"requestId":second["params"]["requestId"],"headers":[{"name":"Authorization","value":"Bearer override-complete-secret"},{"name":"X-Route","value":"second-hop"}]})).await;
            }
            assert_eq!(client.result(&owner).await,if fail {"failed"}else{"/final"});
            let terminal=if fail {"Network.loadingFailed"}else{"Network.loadingFinished"};
            while !client.events.iter().any(|e|e["method"]==terminal && e["params"]["requestId"]==network) {
                let event=client.recv().await; client.events.push(event);
            }
            let events=client.events.iter().filter(|e|e["params"]["requestId"]==network).collect::<Vec<_>>();
            assert!(events.iter().all(|e|e["sessionId"]==owner));
            assert_eq!(events.iter().filter(|e|e["method"]=="Network.requestWillBeSent").count(),2);
            assert_eq!(events.iter().filter(|e|e["method"]=="Network.responseReceived").count(),usize::from(!fail));
            assert_eq!(events.iter().filter(|e|e["method"]==terminal).count(),1);
            if !fail {
                let extra=events.iter().filter(|e|e["method"]=="Network.requestWillBeSentExtraInfo").last().unwrap();
                let headers=extra["params"]["headers"].as_object().unwrap();
                let get=|name:&str|headers.iter().find(|(key,_)|key.eq_ignore_ascii_case(name)).map(|(_,value)|value.as_str().unwrap()).unwrap();
                assert_eq!(get("Authorization"),"Bearer override-complete-secret");
                assert!(get("Cookie").contains("session=complete-secret"));
                assert_eq!(get("X-Route"),"second-hop");
                assert!(!get("User-Agent").is_empty());
                assert_eq!(client.ok(Some(&owner),"Fetch.getResponseBody",json!({"requestId":first["params"]["requestId"]})).await["body"],format!("/redirect/success"));
                assert_eq!(client.ok(Some(&owner),"Fetch.getResponseBody",json!({"requestId":second["params"]["requestId"]})).await["body"],"/final");
            }
            assert_eq!(client.ok(Some(&owner),"Network.getResponseBody",json!({"requestId":hop_body})).await["body"],if fail {"/redirect/failure"}else{"/redirect/success"});
        }
        assert_eq!(*requests.lock().unwrap(),vec!["/","/redirect/success","/final","/redirect/failure"]);
        drop(client);processor.await.unwrap();fixture_task.abort();
    }).await;
}

#[tokio::test(flavor = "current_thread")]
async fn failure_observation_immediate_abort_before_router_drain_has_one_start_and_terminal() {
    tokio::task::LocalSet::new().run_until(async {
        let (base,fixture_task,requests)=fixture_with_requests().await;
        let (mut client,processor)=client().await;
        let (_,session)=page(&mut client,&base).await;
        client.ok(Some(&session),"Runtime.evaluate",json!({"expression":format!("globalThis.c=new AbortController();globalThis.result=fetch('{base}/immediate',{{signal:c.signal}}).catch(e=>'aborted');c.abort('complete abort reason');'started'"),"returnByValue":true})).await;
        assert_eq!(client.result(&session).await,"aborted");
        while !client.events.iter().any(|e|e["method"]=="Network.loadingFailed") {
            let event=client.recv().await;client.events.push(event);
        }
        let failed=client.events.iter().find(|e|e["method"]=="Network.loadingFailed").unwrap();
        let id=failed["params"]["requestId"].clone();
        assert!(failed["params"]["errorText"].as_str().unwrap().contains("complete abort reason"));
        let events=client.events.iter().filter(|e|e["params"]["requestId"]==id).collect::<Vec<_>>();
        assert_eq!(events.iter().filter(|e|e["method"]=="Network.requestWillBeSent").count(),1);
        assert_eq!(events.iter().filter(|e|e["method"]=="Network.loadingFailed").count(),1);
        assert_eq!(events[0]["method"],"Network.requestWillBeSent");
        assert!(!events.iter().any(|e|e["method"]=="Network.loadingFinished"));
        assert_eq!(*requests.lock().unwrap(),vec!["/"]);
        drop(client);processor.await.unwrap();fixture_task.abort();
    }).await;
}


#[tokio::test(flavor = "current_thread")]
async fn failure_observation_navigation_pause_uses_pending_document_loader() {
    tokio::task::LocalSet::new().run_until(async {
        let (base,fixture_task)=fixture().await;
        let (mut client,processor)=client().await;
        let (_,session)=page(&mut client,&base).await;
        let html=format!("<script>globalThis.result=fetch('{base}/during-nav').then(r=>r.text())</script>");
        let url=format!("data:text/html,{html}");
        client.id+=1;
        let command_id=client.id;
        client.tx.send(ServerMessage::Cdp(CdpMessage {
            text:json!({"id":command_id,"method":"Page.navigate","sessionId":session,"params":{"url":url}}).to_string(),reply_tx:client.reply_tx.clone(),
        })).unwrap();
        let pause=client.pause(&session).await;
        let network=pause["params"]["networkId"].clone();
        let start=client.events.iter().find(|event|event["method"]=="Network.requestWillBeSent" && event["params"]["requestId"]==network).unwrap();
        let loader=start["params"]["loaderId"].clone();
        assert_eq!(start["params"]["documentURL"],url);
        client.ok(Some(&session),"Fetch.fulfillRequest",json!({"requestId":pause["params"]["requestId"],"body":"b2s="})).await;
        loop {
            if let Some(response)=client.events.iter().find(|event|event["id"]==command_id) {
                assert_eq!(response["result"]["loaderId"],loader);break;
            }
            let event=client.recv().await;client.events.push(event);
        }
        drop(client);processor.await.unwrap();fixture_task.abort();
    }).await;
}

#[tokio::test(flavor = "current_thread")]
async fn failure_observation_unintercepted_redirect_preflight_has_ordered_starts_and_redirect_response() {
    tokio::task::LocalSet::new().run_until(async {
        let (base, fixture_task, requests) = fixture_with_requests().await;
        let (mut client, processor) = client().await;
        let target = client.ok(None,"Target.createTarget",json!({"url":format!("{base}/")})).await["targetId"].as_str().unwrap().to_string();
        let session = client.ok(None,"Target.attachToTarget",json!({"targetId":target,"flatten":true})).await["sessionId"].as_str().unwrap().to_string();
        client.ok(Some(&session),"Network.enable",json!({})).await;
        let url=format!("{base}/redirect-preflight");
        client.start_fetch(&session,&url,false).await;
        assert_eq!(client.result(&session).await,"/final");
        while !client.events.iter().any(|e|e["method"]=="Network.loadingFinished" && e["params"]["requestId"].as_str().is_some_and(|id|id.starts_with("fetch-"))) {
            let event=client.recv().await;client.events.push(event);
        }
        // A final Runtime call drains the terminal event batch as well.
        client.ok(Some(&session),"Runtime.evaluate",json!({"expression":"1"})).await;
        let starts=client.events.iter().filter(|e|e["method"]=="Network.requestWillBeSent" && e["params"]["requestId"].as_str().is_some_and(|id|id.starts_with("fetch-"))).collect::<Vec<_>>();
        assert_eq!(starts.len(),3,"{starts:?}");
        assert_eq!(starts[0]["params"]["request"]["url"],url);
        assert!(starts[1]["params"]["request"]["url"].as_str().unwrap().contains("localhost:"));
        assert_eq!(starts[1]["params"]["requestId"],starts[0]["params"]["requestId"]);
        assert_eq!(starts[1]["params"]["redirectResponse"]["status"],302);
        assert_eq!(starts[1]["params"]["redirectResponse"]["url"],url);
        assert_eq!(starts[2]["params"]["request"]["method"],"OPTIONS");
        assert_ne!(starts[2]["params"]["requestId"],starts[0]["params"]["requestId"]);
        assert_eq!(starts[2]["params"]["initiator"]["requestId"],starts[0]["params"]["requestId"]);
        let body_id=starts[1]["params"]["redirectResponse"]["bodyRequestId"].clone();
        assert_eq!(client.ok(Some(&session),"Network.getResponseBody",json!({"requestId":body_id})).await["body"],"/redirect-preflight");
        assert!(!client.events.iter().any(|e|e["method"]=="Network.loadingFailed"));
        assert_eq!(*requests.lock().unwrap(),vec!["/","/redirect-preflight","/final","/final"]);
        drop(client);processor.await.unwrap();fixture_task.abort();
    }).await;
}
