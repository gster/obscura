use super::*;
use crate::outbound::{OutboundReceiver, OutboundSender};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct Client {
    tx: ServerMessageSender,
    replies: OutboundReceiver,
    reply_tx: OutboundSender,
    events: Vec<Value>,
    id: u64,
}

impl Client {
    async fn recv(&mut self) -> Value {
        let text = tokio::time::timeout(std::time::Duration::from_secs(10), self.replies.recv())
            .await.expect("CDP message timeout").expect("processor stopped");
        serde_json::from_str(text.as_str()).unwrap()
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
                } else if path == "/redirect-chain/one" {
                    "302 Found\r\nLocation: /redirect-chain/two".into()
                } else if path == "/redirect-chain/two" {
                    "302 Found\r\nLocation: /final".into()
                } else if path.starts_with("/redirect/") { "302 Found\r\nLocation: /final".into() } else { "200 OK".into() };
                let response = format!("HTTP/1.1 {status}\r\nContent-Type: text/html\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Headers: authorization\r\nSet-Cookie: session=complete-secret; Path=/\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body);
                socket.write_all(response.as_bytes()).await.unwrap();
            });
        }
    });
    (base, task, requests)
}

async fn client() -> (Client, tokio::task::JoinHandle<()>) {
    let (tx, rx) = crate::inbound::channel();
    let (reply_tx, replies, _) = crate::outbound::channel();
    let context = Arc::new(obscura_browser::BrowserContext::with_storage_and_network(
        "multi-page-pause".into(), obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145), None, None, true,
    ));
    let processor = tokio::task::spawn_local(cdp_processor(
        rx,
        context,
        ServerShutdown::new(),
        obscura_js::execution_cancellation::ExecutionCancellation::default(),
    ));
    tx.send(ServerMessage::NewConnection { reply_tx: reply_tx.clone() }).unwrap();
    let mut client = Client { tx, replies, reply_tx, events: Vec::new(), id: 0 };
    assert_eq!(client.recv().await["__init"], true);
    (client, processor)
}

async fn page(client: &mut Client, base: &str) -> (String, String) {
    let target = client.ok(None, "Target.createTarget", json!({"url":format!("{base}/")})).await["targetId"].as_str().unwrap().to_string();
    let session = client.ok(None, "Target.attachToTarget", json!({"targetId":target,"flatten":true})).await["sessionId"].as_str().unwrap().to_string();
    client.ok(Some(&session), "Network.enable", json!({})).await;
    client.ok(Some(&session), "Fetch.enable", json!({})).await;
    (target, session)
}

#[tokio::test(flavor = "current_thread")]
async fn network_subscriptions_fan_out_independently_from_fetch_ownership() {
    tokio::task::LocalSet::new().run_until(async {
        let (base, fixture_task, _) = fixture_with_requests().await;
        let (mut client, processor) = client().await;
        let target = client.ok(None, "Target.createTarget", json!({"url":format!("{base}/")})).await["targetId"].as_str().unwrap().to_string();
        let fetch_owner = client.ok(None, "Target.attachToTarget", json!({"targetId":target,"flatten":true})).await["sessionId"].as_str().unwrap().to_string();
        let observer = client.ok(None, "Target.attachToTarget", json!({"targetId":target,"flatten":true})).await["sessionId"].as_str().unwrap().to_string();
        client.ok(Some(&fetch_owner), "Fetch.enable", json!({})).await;
        client.ok(Some(&observer), "Network.enable", json!({})).await;

        client.start_fetch(&fetch_owner, &format!("{base}/observer-only"), false).await;
        let first_pause = client.pause(&fetch_owner).await;
        let first_network = first_pause["params"]["networkId"].as_str().unwrap().to_string();
        let starts = client.events.iter().filter(|event|
            event["method"] == "Network.requestWillBeSent"
                && event["params"]["requestId"] == first_network
        ).collect::<Vec<_>>();
        assert_eq!(starts.len(), 1, "Fetch ownership must not imply Network subscription: {starts:?}");
        assert_eq!(starts[0]["sessionId"], observer);
        assert_eq!(starts[0]["params"]["request"]["headers"]["Authorization"], "Bearer complete-secret");
        client.ok(Some(&fetch_owner), "Fetch.continueRequest", json!({"requestId":first_pause["params"]["requestId"]})).await;
        assert_eq!(client.result(&fetch_owner).await, "/observer-only");
        while !client.events.iter().any(|event|
            event["method"] == "Network.loadingFinished"
                && event["params"]["requestId"] == first_network
        ) {
            let event = client.recv().await;
            client.events.push(event);
        }
        let response = client.events.iter().find(|event|
            event["method"] == "Network.responseReceived"
                && event["params"]["requestId"] == first_network
        ).unwrap();
        assert_eq!(response["sessionId"], observer);
        assert_eq!(response["params"]["response"]["rawHeaders"]["encoding"], "base64");
        assert_eq!(client.ok(Some(&observer), "Network.getResponseBody", json!({"requestId":first_network})).await["body"], "/observer-only");
        assert!(client.command(Some(&fetch_owner), "Network.getResponseBody", json!({"requestId":first_network})).await.get("error").is_some());

        client.ok(Some(&fetch_owner), "Network.enable", json!({})).await;
        client.start_fetch(&fetch_owner, &format!("{base}/both"), false).await;
        let both_pause = client.pause(&fetch_owner).await;
        let both_network = both_pause["params"]["networkId"].as_str().unwrap().to_string();
        let mut both_sessions = client.events.iter().filter(|event|
            event["method"] == "Network.requestWillBeSent"
                && event["params"]["requestId"] == both_network
        ).map(|event| event["sessionId"].as_str().unwrap()).collect::<Vec<_>>();
        both_sessions.sort_unstable();
        let mut expected = vec![fetch_owner.as_str(), observer.as_str()];
        expected.sort_unstable();
        assert_eq!(both_sessions, expected);
        client.ok(Some(&fetch_owner), "Fetch.continueRequest", json!({"requestId":both_pause["params"]["requestId"]})).await;
        assert_eq!(client.result(&fetch_owner).await, "/both");
        while client.events.iter().filter(|event|
            event["method"] == "Network.loadingFinished"
                && event["params"]["requestId"] == both_network
        ).count() < 2 {
            let event = client.recv().await;
            client.events.push(event);
        }
        assert_eq!(client.ok(Some(&fetch_owner), "Network.getResponseBody", json!({"requestId":both_network})).await["body"], "/both");
        assert_eq!(client.ok(Some(&observer), "Network.getResponseBody", json!({"requestId":both_network})).await["body"], "/both");

        client.ok(Some(&fetch_owner), "Network.disable", json!({})).await;
        assert!(client.command(Some(&fetch_owner), "Network.getResponseBody", json!({"requestId":both_network})).await.get("error").is_some());
        assert_eq!(client.ok(Some(&observer), "Network.getResponseBody", json!({"requestId":both_network})).await["body"], "/both");
        client.start_fetch(&fetch_owner, &format!("{base}/fetch-survives-network-disable"), false).await;
        let final_pause = client.pause(&fetch_owner).await;
        let final_network = final_pause["params"]["networkId"].as_str().unwrap().to_string();
        let final_starts = client.events.iter().filter(|event|
            event["method"] == "Network.requestWillBeSent"
                && event["params"]["requestId"] == final_network
        ).collect::<Vec<_>>();
        assert_eq!(final_starts.len(), 1);
        assert_eq!(final_starts[0]["sessionId"], observer);
        client.ok(Some(&fetch_owner), "Fetch.failRequest", json!({"requestId":final_pause["params"]["requestId"],"errorReason":"BlockedByClient"})).await;

        drop(client);
        processor.await.unwrap();
        fixture_task.abort();
    }).await;
}

#[tokio::test(flavor = "current_thread")]
async fn response_stage_pause_exposes_complete_body_and_enforces_stage_lifecycle() {
    tokio::task::LocalSet::new().run_until(async {
        let (base, fixture_task) = fixture().await;
        let (mut client, processor) = client().await;
        let target = client.ok(None, "Target.createTarget", json!({"url":format!("{base}/")})).await["targetId"].as_str().unwrap().to_string();
        let session = client.ok(None, "Target.attachToTarget", json!({"targetId":target,"flatten":true})).await["sessionId"].as_str().unwrap().to_string();
        client.ok(Some(&session), "Network.enable", json!({})).await;
        assert!(client.command(Some(&session), "Fetch.enable", json!({"handleAuthRequests":true})).await["error"]["message"]
            .as_str().unwrap().contains("not supported"));
        assert!(client.command(Some(&session), "Fetch.enable", json!({"handleAuthRequests":"false"})).await["error"]["message"]
            .as_str().unwrap().contains("boolean"));
        client.ok(Some(&session), "Fetch.enable", json!({"handleAuthRequests":false,
            "patterns":[{"urlPattern":format!("{base}/response-stage"),"requestStage":"Response"}]})).await;
        for invalid in [json!({"patterns":[null]}), json!({"patterns":["*"]}),
            json!({"patterns":[{"requestStage":42}]}), json!({"patterns":[{"urlPattern":42}]})]
        {
            assert!(client.command(Some(&session), "Fetch.enable", invalid).await.get("error").is_some());
        }
        client.start_fetch(&session, &format!("{base}/malformed-pattern-must-not-expand-policy"), false).await;
        assert_eq!(client.result(&session).await, "/malformed-pattern-must-not-expand-policy");
        client.ok(Some(&session), "Fetch.enable", json!({"patterns":[{
            "urlPattern":"*","requestStage":"Response"
        }]})).await;

        client.ok(Some(&session), "Runtime.evaluate", json!({"expression":format!(
            "globalThis.result=fetch('{base}/response-stage').then(async r=>[r.status,r.headers.get('x-replaced'),await r.text()]);'started'"),
            "returnByValue":true})).await;
        let pause = client.pause(&session).await;
        let id = pause["params"]["requestId"].as_str().unwrap();
        let network_id = pause["params"]["networkId"].clone();
        assert_eq!(pause["params"]["responseStatusCode"], 200);
        assert_eq!(pause["params"]["responseStatusText"], "");
        assert_eq!(pause["params"]["responseHeaders"].as_array().unwrap().iter()
            .filter(|field| field["name"].as_str().is_some_and(|name| name.eq_ignore_ascii_case("set-cookie"))).count(), 1);
        assert!(pause["params"]["responseRawHeaders"]["fields"].as_array().is_some());
        assert_eq!(client.ok(Some(&session), "Fetch.getResponseBody", json!({"requestId":id})).await["body"], "/response-stage");
        for invalid in [json!({"requestId":id,"url":format!("{base}/other")}), json!({"requestId":id,"url":42}),
            json!({"requestId":id,"interceptResponse":false}), json!({"requestId":id,"unknown":true})]
        {
            assert!(client.command(Some(&session), "Fetch.continueRequest", invalid).await.get("error").is_some());
        }
        assert!(client.command(Some(&session), "Fetch.continueResponse", json!({"requestId":id,"unknown":true})).await.get("error").is_some());
        client.ok(Some(&session), "Fetch.continueResponse", json!({"requestId":id,"responseCode":201,"responsePhrase":"Created by fixture",
            "responseHeaders":[{"name":"Content-Type","value":"text/plain"},{"name":"X-Replaced","value":"yes"}]})).await;
        assert_eq!(client.result(&session).await, json!([201,"yes","/response-stage"]));
        while !client.events.iter().any(|event| event["method"] == "Network.loadingFinished" && event["params"]["requestId"] == network_id) {
            let event = client.recv().await; client.events.push(event);
        }
        let response = client.events.iter().find(|event| event["method"] == "Network.responseReceived"
            && event["params"]["requestId"] == network_id).unwrap();
        assert_eq!(response["params"]["response"]["status"], 201);
        assert_eq!(response["params"]["response"]["statusText"], "Created by fixture");
        assert_eq!(response["params"]["response"]["headers"]["x-replaced"], "yes");

        client.start_fetch(&session, &format!("{base}/history-fulfill"), false).await;
        let fulfilled_pause = client.pause(&session).await;
        let fulfilled_network_id = fulfilled_pause["params"]["networkId"].clone();
        let replacement = b"history-response-replacement";
        client.ok(Some(&session), "Fetch.fulfillRequest", json!({
            "requestId": fulfilled_pause["params"]["requestId"],
            "responseCode": 200,
            "body": base64::engine::general_purpose::STANDARD.encode(replacement),
        })).await;
        assert_eq!(client.result(&session).await, "history-response-replacement");
        while !client.events.iter().any(|event| event["method"] == "Network.loadingFinished"
            && event["params"]["requestId"] == fulfilled_network_id)
        {
            let event = client.recv().await;
            client.events.push(event);
        }
        let histories = client.ok(None, "Obscura.getNetworkHistories", json!({})).await;
        let history_id = histories["histories"].as_array().unwrap().iter()
            .find(|history| history["live"] == true).unwrap()["historyId"].clone();
        let history = client.ok(None, "Obscura.getNetworkHistory", json!({
            "historyId": history_id,
            "limit": 1000,
        })).await;
        let fulfilled_record = history["records"].as_array().unwrap().iter()
            .find(|record| record["event"]["requestId"] == fulfilled_network_id
                && record["responseBody"].is_object())
            .expect("fulfilled response remains in persistent history");
        let fulfilled_body = client.ok(None, "Obscura.getNetworkBody", json!({
            "historyId": history_id,
            "bodyKey": fulfilled_record["responseBody"]["key"],
        })).await;
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(fulfilled_body["data"].as_str().unwrap()).unwrap(),
            replacement,
        );

        client.start_fetch(&session, &format!("{base}/redirect/response-stage"), false).await;
        let redirect = client.pause(&session).await;
        assert_eq!(redirect["params"]["responseStatusCode"], 302);
        for method in ["Fetch.getResponseBody", "Fetch.takeResponseBodyAsStream"] {
            let error = client.command(Some(&session), method, json!({"requestId":redirect["params"]["requestId"]})).await;
            assert!(error["error"]["message"].as_str().unwrap().contains("redirect response"), "{error}");
        }
        client.ok(Some(&session), "Fetch.continueResponse", json!({"requestId":redirect["params"]["requestId"]})).await;
        let final_pause = client.pause(&session).await;
        assert_eq!(final_pause["params"]["networkId"], redirect["params"]["networkId"]);
        assert_eq!(final_pause["params"]["redirectedRequestId"], redirect["params"]["requestId"]);
        assert_eq!(final_pause["params"]["responseStatusCode"], 200);
        let stream = client.ok(Some(&session), "Fetch.takeResponseBodyAsStream", json!({"requestId":final_pause["params"]["requestId"]})).await["stream"].clone();
        for method in ["Fetch.getResponseBody", "Fetch.takeResponseBodyAsStream"] {
            assert!(client.command(Some(&session), method, json!({"requestId":final_pause["params"]["requestId"]})).await
                ["error"]["message"].as_str().unwrap().contains("response_body_access_conflict"));
        }
        let same_target_session = client.ok(None, "Target.attachToTarget", json!({"targetId":target,"flatten":true})).await["sessionId"].as_str().unwrap().to_string();
        assert_eq!(client.ok(Some(&same_target_session), "IO.read", json!({"handle":stream,"size":0})).await["data"], "",
            "Fetch IO handles belong to the target, not only the issuing session");
        let other_target = client.ok(None, "Target.createTarget", json!({"url":"about:blank"})).await["targetId"].as_str().unwrap().to_string();
        let other_session = client.ok(None, "Target.attachToTarget", json!({"targetId":other_target,"flatten":true})).await["sessionId"].as_str().unwrap().to_string();
        assert!(client.command(Some(&other_session), "IO.read", json!({"handle":stream})).await.get("error").is_some());
        assert!(client.command(Some(&other_session), "IO.close", json!({"handle":stream})).await.get("error").is_some());
        assert!(client.command(None, "IO.read", json!({"handle":stream})).await.get("error").is_some());
        assert!(client.command(None, "IO.close", json!({"handle":stream})).await.get("error").is_some());
        assert!(client.command(Some(&session), "IO.read", json!({"handle":stream,"offset":0})).await.get("error").is_some());
        for method in ["Fetch.continueRequest", "Fetch.continueResponse"] {
            assert!(client.command(Some(&session), method, json!({"requestId":final_pause["params"]["requestId"]})).await["error"]["message"]
                .as_str().unwrap().contains("only failRequest or fulfillRequest"));
        }
        client.ok(Some(&session), "Fetch.fulfillRequest", json!({"requestId":final_pause["params"]["requestId"],
            "responseCode":200,"responsePhrase":"Stream Replacement",
            "body":base64::engine::general_purpose::STANDARD.encode(b"replacement")})).await;
        assert_eq!(client.result(&session).await, "replacement");
        while !client.events.iter().any(|event| event["method"] == "Network.loadingFinished"
            && event["params"]["requestId"] == final_pause["params"]["networkId"]) {
            let event = client.recv().await; client.events.push(event);
        }
        let stream_response = client.events.iter().find(|event| event["method"] == "Network.responseReceived"
            && event["params"]["requestId"] == final_pause["params"]["networkId"]).unwrap();
        assert_eq!(stream_response["params"]["response"]["statusText"], "Stream Replacement");
        assert_eq!(client.ok(Some(&session), "Network.getResponseBody", json!({"requestId":final_pause["params"]["networkId"]})).await["body"], "replacement",
            "fulfilled request aliases must point at the replacement Page body");
        let old = client.ok(Some(&session), "IO.read", json!({"handle":stream})).await;
        assert_eq!(base64::engine::general_purpose::STANDARD.decode(old["data"].as_str().unwrap()).unwrap(), b"/final");
        client.ok(Some(&session), "IO.close", json!({"handle":stream})).await;

        client.start_fetch(&session, &format!("{base}/response-stage"), false).await;
        let continue_pause = client.pause(&session).await;
        client.ok(Some(&session), "Fetch.continueRequest", json!({"requestId":continue_pause["params"]["requestId"]})).await;
        assert_eq!(client.result(&session).await, "/response-stage");

        client.start_fetch(&session, &format!("{base}/redirect-chain/one"), false).await;
        let first = client.pause(&session).await;
        let chain_network_id = first["params"]["networkId"].clone();
        assert_eq!(first["params"]["responseStatusCode"], 302);
        assert!(first["params"]["redirectedRequestId"].is_null());
        assert_eq!(client.events.iter().filter(|event| event["method"] == "Network.requestWillBeSent"
            && event["params"]["requestId"] == chain_network_id).count(), 1);
        client.ok(Some(&session), "Fetch.continueResponse", json!({"requestId":first["params"]["requestId"]})).await;

        let second = client.pause(&session).await;
        assert_eq!(second["params"]["networkId"], chain_network_id);
        assert_eq!(second["params"]["responseStatusCode"], 302);
        assert_eq!(second["params"]["redirectedRequestId"], first["params"]["requestId"]);
        assert_eq!(client.events.iter().filter(|event| event["method"] == "Network.requestWillBeSent"
            && event["params"]["requestId"] == chain_network_id).count(), 2);
        client.ok(Some(&session), "Fetch.continueResponse", json!({"requestId":second["params"]["requestId"]})).await;

        let third = client.pause(&session).await;
        assert_eq!(third["params"]["networkId"], chain_network_id);
        assert_eq!(third["params"]["responseStatusCode"], 200);
        assert_eq!(third["params"]["redirectedRequestId"], second["params"]["requestId"]);
        assert_eq!(client.events.iter().filter(|event| event["method"] == "Network.requestWillBeSent"
            && event["params"]["requestId"] == chain_network_id).count(), 3,
            "the third request start must precede its response pause");
        client.ok(Some(&session), "Fetch.continueResponse", json!({"requestId":third["params"]["requestId"]})).await;
        assert_eq!(client.result(&session).await, "/final");

        let starts = client.events.iter().filter(|event| event["method"] == "Network.requestWillBeSent"
            && event["params"]["requestId"] == chain_network_id).collect::<Vec<_>>();
        assert_eq!(starts.len(), 3, "every response-only redirect hop needs its own request start: {starts:?}");
        assert!(starts.iter().all(|event| event["sessionId"] == session));
        assert!(starts[0]["params"]["redirectResponse"].is_null());
        assert_eq!(starts[1]["params"]["redirectResponse"]["status"], 302);
        assert_eq!(starts[1]["params"]["redirectResponse"]["url"], format!("{base}/redirect-chain/one"));
        assert_eq!(starts[2]["params"]["redirectResponse"]["status"], 302);
        assert_eq!(starts[2]["params"]["redirectResponse"]["url"], format!("{base}/redirect-chain/two"));

        for expected in ["XHR", "Fetch"] {
            client.ok(Some(&session), "Fetch.disable", json!({})).await;
            client.ok(Some(&session), "Fetch.enable", json!({"patterns":[{
                "urlPattern":format!("{base}/resource-type"), "resourceType":expected, "requestStage":"Response"
            }]})).await;
            let expression = format!(r#"
                globalThis.result = Promise.all([
                  fetch('{base}/resource-type', {{__obscuraResourceType:'XHR'}}).then(r => r.text()),
                  new Promise((resolve, reject) => {{ const x = new XMLHttpRequest();
                    x.open('GET', '{base}/resource-type'); x.onload = () => resolve(x.responseText);
                    x.onerror = reject; x.send(); }})
                ]); 'started'
            "#);
            client.ok(Some(&session), "Runtime.evaluate", json!({"expression":expression,"returnByValue":true})).await;
            let typed = client.pause(&session).await;
            assert_eq!(typed["params"]["resourceType"], expected);
            client.ok(Some(&session), "Fetch.continueResponse", json!({"requestId":typed["params"]["requestId"]})).await;
            assert_eq!(client.result(&session).await, json!(["/resource-type", "/resource-type"]),
                "only the selected resource type should pause");
        }

        client.ok(Some(&session), "Fetch.disable", json!({})).await;
        client.ok(Some(&session), "Fetch.enable", json!({"patterns":[{
            "urlPattern":"*","resourceType":"XHR","requestStage":"Request"
        }]})).await;
        client.start_fetch(&session, &format!("{base}/redirect/xhr-pattern-must-ignore-fetch"), false).await;
        assert_eq!(client.result(&session).await, "/final",
            "a Fetch redirect chain must not pause for an XHR-only request pattern");

        client.ok(Some(&session), "Fetch.enable", json!({"patterns":[{
            "urlPattern":format!("{base}/redirect/url-pattern"),"resourceType":"Fetch","requestStage":"Request"
        }]})).await;
        client.start_fetch(&session, &format!("{base}/redirect/url-pattern"), false).await;
        let matched_redirect_start = client.pause(&session).await;
        client.ok(Some(&session), "Fetch.continueRequest", json!({
            "requestId":matched_redirect_start["params"]["requestId"]
        })).await;
        assert_eq!(client.result(&session).await, "/final",
            "a redirect target outside the URL pattern must not pause");

        client.ok(Some(&session), "Fetch.disable", json!({})).await;
        client.ok(Some(&session), "Fetch.enable", json!({})).await;
        client.start_fetch(&session, &format!("{base}/request-synthetic"), false).await;
        let synthetic = client.pause(&session).await;
        for invalid in [json!({"requestId":synthetic["params"]["requestId"],"url":42}),
            json!({"requestId":synthetic["params"]["requestId"],"method":42}),
            json!({"requestId":synthetic["params"]["requestId"],"interceptResponse":true}),
            json!({"requestId":synthetic["params"]["requestId"],"unknown":true})]
        {
            assert_eq!(client.command(Some(&session), "Fetch.continueRequest", invalid).await["error"]["code"], -32602);
        }
        assert_eq!(client.command(Some(&session), "Fetch.failRequest", json!({"requestId":synthetic["params"]["requestId"],
            "errorReason":"Invented"})).await["error"]["code"], -32602);
        assert_eq!(client.command(Some(&session), "Fetch.fulfillRequest", json!({"requestId":synthetic["params"]["requestId"],
            "responseCode":203,"responsePhrase":42})).await["error"]["code"], -32602);
        client.ok(Some(&session), "Fetch.fulfillRequest", json!({"requestId":synthetic["params"]["requestId"],
            "responseCode":203,"responsePhrase":"Synthetic Complete","body":"c3ludGhldGlj"})).await;
        assert_eq!(client.result(&session).await, "synthetic");
        while !client.events.iter().any(|event| event["method"] == "Network.loadingFinished"
            && event["params"]["requestId"] == synthetic["params"]["networkId"]) {
            let event = client.recv().await; client.events.push(event);
        }
        let response = client.events.iter().find(|event| event["method"] == "Network.responseReceived"
            && event["params"]["requestId"] == synthetic["params"]["networkId"]).unwrap();
        assert_eq!(response["params"]["response"]["statusText"], "Synthetic Complete");

        drop(client); processor.await.unwrap(); fixture_task.abort();
    }).await;
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
                assert!(client.command(Some(&right), "Network.getResponseBody", json!({"requestId":id})).await.get("error").is_some(),
                    "Fetch interception IDs are not Network request IDs");
                assert_eq!(client.ok(Some(&right), "Fetch.getResponseBody", json!({"requestId":id})).await["body"], "right-secret");
                assert!(client.command(None, "Fetch.getResponseBody", json!({"requestId":id})).await.get("error").is_some());
                assert!(client.command(Some(&left), "Fetch.takeResponseBodyAsStream", json!({"requestId":id})).await["error"]["message"]
                    .as_str().unwrap().contains("response_body_access_conflict"));
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
        client.ok(None, "Fetch.fulfillRequest", json!({"requestId":id,"responseCode":200,"body":"c2luZ2xl"})).await;
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
        client.ok(None, "Fetch.enable", json!({"patterns":[{
            "urlPattern":format!("{base}/sessionless-stream"),"requestStage":"Response"
        }]})).await;
        client.start_fetch(&session, &format!("{base}/sessionless-stream"), false).await;
        let stream_pause = loop {
            if let Some(index) = client.events.iter().position(|event| event["method"] == "Fetch.requestPaused"
                && event["sessionId"].is_null() && event["params"]["responseStatusCode"] == 200) {
                break client.events.remove(index);
            }
            let event = client.recv().await; client.events.push(event);
        };
        let stream_counter = stream_pause["params"]["requestId"].as_str().unwrap()
            .strip_prefix("intercept-").unwrap().parse::<u64>().unwrap();
        let stream = client.ok(None, "Fetch.takeResponseBodyAsStream", json!({
            "requestId":stream_pause["params"]["requestId"]
        })).await["stream"].clone();
        for method in ["Fetch.continueRequest", "Fetch.continueResponse"] {
            assert!(client.command(None, method, json!({"requestId":stream_pause["params"]["requestId"]})).await
                ["error"]["message"].as_str().unwrap().contains("only failRequest or fulfillRequest"));
        }
        client.ok(None, "Fetch.disable", json!({})).await;
        assert_eq!(client.result(&session).await, "failed", "disable must abort a sessionless pause after stream transfer");
        assert_eq!(client.ok(None, "IO.read", json!({"handle":stream,"size":0})).await["data"], "",
            "Fetch.disable must not close an already-issued IO handle");
        client.ok(None, "IO.close", json!({"handle":stream})).await;
        let histories = client.ok(None, "Obscura.getNetworkHistories", json!({})).await;
        let history_id = histories["histories"].as_array().unwrap().iter()
            .find(|history| history["live"] == true).unwrap()["historyId"].clone();
        let history = client.ok(None, "Obscura.getNetworkHistory", json!({
            "historyId": history_id,
            "limit": 1000,
        })).await;
        let stream_record = history["records"].as_array().unwrap().iter()
            .find(|record| {
                record["event"]["url"] == format!("{base}/sessionless-stream")
                    && record["responseBody"]["key"].is_string()
            })
            .expect("response-stage stream observation remains in persistent history");
        let body_key = stream_record["responseBody"]["key"].clone();
        let retained = client.ok(None, "Obscura.getNetworkBody", json!({
            "historyId": history_id,
            "bodyKey": body_key,
        })).await;
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(retained["data"].as_str().unwrap()).unwrap(),
            b"/sessionless-stream",
        );
        client.ok(Some(&session), "Fetch.enable", json!({"patterns":[{"urlPattern":"*","requestStage":"Response"}]})).await;
        // A script pauses while its Page is temporarily removed from ctx.pages
        // by the navigation task. Route using the enable owner, not ctx.pages.
        let html = format!("<script>globalThis.result=fetch('{base}/during-nav').then(r=>r.text())</script>");
        client.id += 1;
        client.tx.send(ServerMessage::Cdp(CdpMessage {
            text:json!({"id":client.id,"method":"Page.navigate","sessionId":session,"params":{"url":format!("data:text/html,{html}")}}).to_string(),
            reply_tx:client.reply_tx.clone(),
        })).unwrap();
        let pause = client.pause(&session).await;
        let navigation_counter = pause["params"]["requestId"].as_str().unwrap()
            .strip_prefix("intercept-").unwrap().parse::<u64>().unwrap();
        // op_fetch_start reserves a request-stage ID before the response-stage
        // match is known, so only monotonic Page ownership is observable here.
        assert!(navigation_counter > stream_counter, "navigation retains the monotonic Page pause counter");
        assert_eq!(client.ok(Some(&session), "Fetch.getResponseBody", json!({"requestId":pause["params"]["requestId"]})).await["body"], "/during-nav",
            "navigation-time response pause must retain access to its Page body store");
        client.ok(Some(&session), "Fetch.disable", json!({})).await;
        assert_eq!(client.result(&session).await, "/during-nav");
        client.start_fetch(&session, &format!("{base}/after-nav-disable"), false).await;
        assert_eq!(client.result(&session).await, "/after-nav-disable",
            "disable during navigation must clear the returned Page policy");
        drop(client);
        tokio::time::timeout(std::time::Duration::from_secs(3), processor).await.expect("close during navigation must release pause").unwrap();
        fixture_task.abort();
    }).await;
}

#[tokio::test(flavor = "current_thread")]
async fn target_lifecycle_commands_release_navigation_fetch_pauses_before_defer() {
    tokio::task::LocalSet::new().run_until(async {
        let (base, fixture_task) = fixture().await;
        let (mut client, processor) = client().await;

        let (detached_target, detached_session) = page(&mut client, &base).await;
        let html = format!("<script>globalThis.result=fetch('{base}/detach-during-nav').then(r=>r.text()).catch(()=> 'failed')</script>");
        client.id += 1;
        client.tx.send(ServerMessage::Cdp(CdpMessage {
            text:json!({"id":client.id,"method":"Page.navigate","sessionId":detached_session,
                "params":{"url":format!("data:text/html,{html}")}}).to_string(),
            reply_tx:client.reply_tx.clone(),
        })).unwrap();
        let request_pause = client.pause(&detached_session).await;
        assert!(request_pause["params"]["responseStatusCode"].is_null());
        tokio::time::timeout(std::time::Duration::from_secs(3),
            client.ok(None, "Target.detachFromTarget", json!({"sessionId":detached_session})))
            .await.expect("detach must abort the navigating request pause");
        let reattached = client.ok(None, "Target.attachToTarget", json!({"targetId":detached_target,"flatten":true})).await
            ["sessionId"].as_str().unwrap().to_string();
        assert_eq!(client.result(&reattached).await, "failed");

        let close_target = client.ok(None, "Target.createTarget", json!({"url":format!("{base}/")})).await
            ["targetId"].as_str().unwrap().to_string();
        let close_session = client.ok(None, "Target.attachToTarget", json!({"targetId":close_target,"flatten":true})).await
            ["sessionId"].as_str().unwrap().to_string();
        client.ok(Some(&close_session), "Fetch.enable", json!({"patterns":[{
            "urlPattern":"*","requestStage":"Response"
        }]})).await;
        let html = format!("<script>globalThis.result=fetch('{base}/close-during-nav').then(r=>r.text()).catch(()=> 'failed')</script>");
        client.id += 1;
        client.tx.send(ServerMessage::Cdp(CdpMessage {
            text:json!({"id":client.id,"method":"Page.navigate","sessionId":close_session,
                "params":{"url":format!("data:text/html,{html}")}}).to_string(),
            reply_tx:client.reply_tx.clone(),
        })).unwrap();
        let response_pause = client.pause(&close_session).await;
        assert_eq!(response_pause["params"]["responseStatusCode"], 200);
        tokio::time::timeout(std::time::Duration::from_secs(3),
            client.ok(None, "Target.closeTarget", json!({"targetId":close_target})))
            .await.expect("closeTarget must abort the navigating response pause");

        drop(client);
        tokio::time::timeout(std::time::Duration::from_secs(3), processor).await
            .expect("processor exits after lifecycle navigation coverage").unwrap();
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
            let value: Value = serde_json::from_str(reply.as_str()).unwrap();
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
        "closed-relay".into(), obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145), None, None, true,
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
    let (reply_tx, reply_rx, _) = crate::outbound::channel();
    drop(reply_rx);
    let (resolver, mut resolved) = tokio::sync::oneshot::channel();
    let request = obscura_js::ops::InterceptedRequest {
                stage: obscura_js::ops::InterceptionStage::Request,
                document_generation: 0, document_url: "https://example.test/".into(), redirect_response: None,
                redirected_request_id: None,
                network_id: "fixture-network-id".into(),
                network_start: Arc::new(std::sync::atomic::AtomicU8::new(0)),
                request_raw_headers: None,
                request_body_present: false, request_body_request_id: None, request_body_size: 0,
                transport_request_body_present: false, transport_request_body_request_id: None,
                transport_request_body_size: 0,
        request_id: "intercept-1".into(), url: "https://example.test/".into(), method: "GET".into(),
        headers: HashMap::new(), resource_type: "Fetch".into(), response_status_code: None,
        response_headers: None, response_raw_headers: None, response_body_request_id: None, resolver,
    };
    let mut paused = InterceptedPauses::new();
    emit_intercepted_request(request, "frame", "loader", "https://example.test/", Some("session".into()), &[], None, &reply_tx, &mut paused);
    assert!(paused.is_empty());
    assert!(matches!(resolved.try_recv(), Ok(obscura_js::ops::InterceptResolution::Fail { reason }) if reason == "Aborted"));
}

#[test]
fn closed_routed_pause_does_not_create_a_network_start_observer() {
    let mut ctx = CdpContext::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));
    let page_id = ctx.create_page();
    let session = Some("closed-route-session".to_string());
    ctx.sessions.insert(session.clone().unwrap(), page_id.clone());
    ctx.fetch_intercept.owners.insert(page_id.clone(), session.clone());
    let (resolver, resolved) = tokio::sync::oneshot::channel();
    drop(resolved);
    let request = obscura_js::ops::InterceptedRequest {
        stage: obscura_js::ops::InterceptionStage::Request,
        document_generation: 0, document_url: "https://example.test/".into(), redirect_response: None,
        redirected_request_id: None,
        network_id: "retired-network-id".into(),
        network_start: Arc::new(std::sync::atomic::AtomicU8::new(0)),
        request_raw_headers: None,
        request_body_present: false, request_body_request_id: None, request_body_size: 0,
        transport_request_body_present: false, transport_request_body_request_id: None,
        transport_request_body_size: 0,
        request_id: "intercept-retired".into(), url: "https://example.test/".into(), method: "GET".into(),
        headers: HashMap::new(), resource_type: "Fetch".into(), response_status_code: None,
        response_headers: None, response_raw_headers: None, response_body_request_id: None, resolver,
    };
    let routed = crate::domains::fetch::RoutedInterceptedRequest {
        page_id: page_id.clone(), frame_id: "frame".into(), session_id: session, request,
    };
    let (reply_tx, mut replies, _) = crate::outbound::channel();
    let mut paused = InterceptedPauses::new();
    emit_routed_intercepted_request(routed, &mut ctx, &reply_tx, &mut paused);
    assert!(paused.is_empty());
    assert!(ctx.network_request_sessions.is_empty());
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
        client.ok(Some(&session),"Fetch.fulfillRequest",json!({"requestId":pause["params"]["requestId"],"responseCode":200,"body":"b2s="})).await;
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
