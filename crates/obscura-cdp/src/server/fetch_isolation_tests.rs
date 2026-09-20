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
    let task = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let captured = captured.clone();
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
                let response = format!("HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nAccess-Control-Allow-Origin: *\r\nSet-Cookie: session=complete-secret; Path=/\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body);
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
                for (method, field) in [("Fetch.continueRequest", "postData"), ("Fetch.fulfillRequest", "body")] {
                    let mut params = json!({"requestId":id}); params[field] = json!("%");
                    let response = client.command(Some(&left), method, params).await;
                    assert_eq!(response["error"]["code"], -32602);
                }
            }
            if round == 1 {
                client.ok(Some(&left), "Fetch.continueRequest", json!({"requestId":id})).await;
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
        request_id: "intercept-1".into(), url: "https://example.test/".into(), method: "GET".into(),
        headers: HashMap::new(), resource_type: "Fetch".into(), resolver,
    };
    let mut paused = InterceptedPauses::new();
    emit_intercepted_request(request, "frame", Some("session".into()), &reply_tx, &mut paused);
    assert!(paused.is_empty());
    assert!(matches!(resolved.try_recv(), Ok(obscura_js::ops::InterceptResolution::Fail { reason }) if reason == "Aborted"));
}
