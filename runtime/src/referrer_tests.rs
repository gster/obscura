use obscura_net::{
    CookieJar, ObscuraHttpClient, ReferrerPolicy, ResourceRequest, ResourceType, StealthHttpClient,
};
use std::{collections::HashMap, sync::Arc};
use url::Url;

#[test]
fn referrer_policy_matrix_strips_credentials_fragments_and_respects_origins() {
    use ReferrerPolicy::*;
    let source = Url::parse("https://user:secret@app.example/path?q=1#fragment").unwrap();
    let full = Some("https://app.example/path?q=1");
    let origin = Some("https://app.example/");
    for (policy, expected) in [
        (NoReferrer, [None, None, None]),
        (NoReferrerWhenDowngrade, [full, full, None]),
        (SameOrigin, [full, None, None]),
        (Origin, [origin, origin, origin]),
        (StrictOrigin, [origin, origin, None]),
        (OriginWhenCrossOrigin, [full, origin, origin]),
        (StrictOriginWhenCrossOrigin, [full, origin, None]),
        (UnsafeUrl, [full, full, full]),
    ] {
        for (target, expected) in [
            "https://app.example/next",
            "https://other.example/next",
            "http://other.example/next",
        ]
        .into_iter()
        .zip(expected)
        {
            let actual = policy.referrer(Some(&source), &Url::parse(target).unwrap());
            assert_eq!(
                actual.as_ref().map(Url::as_str),
                expected,
                "{policy:?} {target}"
            );
        }
    }
}

#[test]
fn referrer_policy_handles_header_fallbacks_local_trust_and_absent_sources() {
    assert_eq!(
        ReferrerPolicy::from_header("unknown, origin, no-referrer, future"),
        Some(ReferrerPolicy::NoReferrer)
    );
    assert_eq!(ReferrerPolicy::from_header(" , unknown"), None);
    assert_eq!(ReferrerPolicy::parse(""), None);
    assert_eq!(ReferrerPolicy::parse("origin, no-referrer"), None);
    let source = Url::parse("https://app.example/path").unwrap();
    for target in [
        "http://127.0.0.1/",
        "http://[::1]/",
        "http://localhost/",
        "http://test.localhost/",
    ] {
        assert_eq!(
            ReferrerPolicy::StrictOrigin
                .referrer(Some(&source), &Url::parse(target).unwrap())
                .unwrap()
                .as_str(),
            "https://app.example/"
        );
    }
    let target = Url::parse("http://app.example/next").unwrap();
    let local = Url::parse("http://127.0.0.1/private").unwrap();
    assert!(ReferrerPolicy::default()
        .referrer(Some(&local), &target)
        .is_none());
    let mut request = ResourceRequest::subresource(ResourceType::Image, &source);
    request.referrer = None;
    assert_eq!(
        request
            .referrer_policy
            .referrer(request.referrer.as_ref(), &target),
        None
    );
    let long = Url::parse(&format!("https://app.example/{}", "x".repeat(4096))).unwrap();
    assert_eq!(
        ReferrerPolicy::UnsafeUrl
            .referrer(Some(&long), &target)
            .unwrap()
            .as_str(),
        "https://app.example/"
    );
    assert!(ReferrerPolicy::UnsafeUrl
        .referrer(
            Some(&Url::parse("data:text/plain,secret").unwrap()),
            &target
        )
        .is_none());
}

#[tokio::test]
async fn referrer_policy_redirects_do_not_restore_discarded_source_in_either_transport() {
    for stealth in [false, true] {
        let mut replies = Vec::new();
        for header in [
            "unsafe-url\r\nReferrer-Policy: invalid, origin, future",
            "unsafe-url",
            "no-referrer",
            "unsafe-url",
        ] {
            replies.push(format!("HTTP/1.1 302 Found\r\nLocation: /resource\r\nReferrer-Policy: {header}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"));
        }
        replies.push(ok_response("", "complete"));
        let (target, mut received) = http_fixture(replies).await;
        let client = Arc::new(ObscuraHttpClient::with_full_options(
            Arc::new(CookieJar::new()),
            None,
            true,
        ));
        let extra = HashMap::from([("Referer".into(), "https://forged.invalid/private".into())]);
        client.set_extra_headers(extra.clone()).await;
        let source = Url::parse("https://user:secret@source.example/path?q=1#private").unwrap();
        let mut request = ResourceRequest::subresource(ResourceType::Document, &source);
        request.referrer_policy = ReferrerPolicy::UnsafeUrl;
        let response = if stealth {
            let transport =
                StealthHttpClient::with_policy(Arc::new(CookieJar::new()), None, client.clone());
            transport.set_extra_headers(extra).await;
            transport
                .fetch_resource_with_callbacks(&target, request, None)
                .await
        } else {
            client
                .fetch_resource_with_callbacks(&target, request, None)
                .await
        }
        .unwrap();
        assert_eq!(response.body, b"complete");
        assert_eq!(response.request_referrer, None);
        for expected in [
            Some("https://source.example/path?q=1"),
            Some("https://source.example/"),
            Some("https://source.example/"),
            None,
            None,
        ] {
            let raw = received.recv().await.unwrap().to_ascii_lowercase();
            let referer = raw
                .lines()
                .find_map(|line| line.strip_prefix("referer: ").map(str::trim));
            assert_eq!(referer, expected, "stealth={stealth}");
            assert!(raw.contains("sec-fetch-site: cross-site\r\n"));
            assert!(!raw.contains("forged.invalid"));
        }
    }
}

#[tokio::test]
async fn referrer_policy_partitions_the_resource_cache() {
    let (target, mut received) = http_fixture(vec![
        ok_response("Cache-Control: max-age=60\r\n", "no-referrer"),
        ok_response("Cache-Control: max-age=60\r\n", "origin"),
    ])
    .await;
    let client = ObscuraHttpClient::with_full_options(Arc::new(CookieJar::new()), None, true);
    let source = Url::parse("https://source.example/path").unwrap();
    for (policy, body) in [
        (ReferrerPolicy::NoReferrer, b"no-referrer".as_slice()),
        (ReferrerPolicy::Origin, b"origin".as_slice()),
    ] {
        let mut request = ResourceRequest::subresource(ResourceType::Image, &source);
        request.referrer_policy = policy;
        let response = client
            .fetch_resource_with_callbacks(&target, request, None)
            .await
            .unwrap();
        assert_eq!(response.body, body);
        assert_eq!(
            response.request_referrer.as_ref().map(Url::as_str),
            (policy == ReferrerPolicy::Origin).then_some("https://source.example/")
        );
        let raw = received.recv().await.unwrap().to_ascii_lowercase();
        assert_eq!(
            raw.contains("referer: https://source.example/\r\n"),
            policy == ReferrerPolicy::Origin
        );
    }
}

#[tokio::test]
async fn navigation_post_keeps_request_context_through_redirect() {
    let (target, mut received) = http_fixture(vec![
        "HTTP/1.1 302 Found\r\nLocation: /final\r\nReferrer-Policy: origin\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into(),
        ok_response("Referrer-Policy: unsafe-url\r\n", "complete"),
    ]).await;
    let source = Url::parse("https://source.example/form?q=1#fragment").unwrap();
    let mut request = ResourceRequest::navigation();
    request.referrer = Some(source.clone());
    request.initiator = Some(source);
    request.referrer_policy = ReferrerPolicy::UnsafeUrl;
    let client = ObscuraHttpClient::with_full_options(Arc::new(CookieJar::new()), None, true);
    let response = client
        .post_form_resource_with_callbacks(&target, "field=value", request, None)
        .await
        .unwrap();
    assert_eq!(
        response.request_referrer.unwrap().as_str(),
        "https://source.example/"
    );
    let first = received.recv().await.unwrap().to_ascii_lowercase();
    let last = received.recv().await.unwrap().to_ascii_lowercase();
    assert!(first.starts_with("post /resource http/1.1\r\n"));
    assert!(first.contains("referer: https://source.example/form?q=1\r\n"));
    assert!(first.ends_with("field=value"));
    assert!(last.starts_with("get /final http/1.1\r\n"));
    assert!(last.contains("referer: https://source.example/\r\n"));
    assert!(first.contains("sec-fetch-site: cross-site\r\n"));
    assert!(last.contains("sec-fetch-site: cross-site\r\n"));
}

#[tokio::test]
async fn fulfilled_response_cannot_supply_navigation_referrer() {
    struct Fixture;
    #[async_trait::async_trait]
    impl obscura_net::interceptor::RequestInterceptor for Fixture {
        async fn intercept(
            &self,
            request: &obscura_net::RequestInfo,
        ) -> obscura_net::interceptor::InterceptAction {
            obscura_net::interceptor::InterceptAction::Fulfill(obscura_net::Response {
                url: request.url.clone(),
                status: 200,
                body: Vec::new(),
                redirected_from: Vec::new(),
                headers: HashMap::from([("referer".into(), "https://forged.invalid/".into())]),
                request_referrer: Some(Url::parse("https://forged.invalid/").unwrap()),
            })
        }
    }
    let jar = Arc::new(CookieJar::new());
    let client = Arc::new(ObscuraHttpClient::with_full_options(
        jar.clone(),
        None,
        true,
    ));
    *client.interceptor.write().await = Some(std::sync::Arc::new(Fixture));
    let transport = StealthHttpClient::with_policy(jar, None, client.clone());
    let source = Url::parse("https://source.example/private").unwrap();
    let target = Url::parse("https://target.example/page").unwrap();
    for policy in [ReferrerPolicy::Origin, ReferrerPolicy::NoReferrer] {
        let mut request = ResourceRequest::navigation();
        request.referrer = Some(source.clone());
        request.initiator = Some(source.clone());
        request.referrer_policy = policy;
        for response in [
            client
                .fetch_resource_with_callbacks(&target, request.clone(), None)
                .await
                .unwrap(),
            transport
                .fetch_resource_with_callbacks(&target, request, None)
                .await
                .unwrap(),
        ] {
            assert_eq!(
                response.request_referrer.as_ref().map(Url::as_str),
                (policy == ReferrerPolicy::Origin).then_some("https://source.example/")
            );
        }
    }
}

#[tokio::test]
async fn fulfilled_file_response_has_no_navigation_referrer() {
    struct Fixture;
    #[async_trait::async_trait]
    impl obscura_net::interceptor::RequestInterceptor for Fixture {
        async fn intercept(
            &self,
            request: &obscura_net::RequestInfo,
        ) -> obscura_net::interceptor::InterceptAction {
            obscura_net::interceptor::InterceptAction::Fulfill(obscura_net::Response {
                url: request.url.clone(),
                status: 200,
                headers: HashMap::new(),
                body: Vec::new(),
                redirected_from: Vec::new(),
                request_referrer: Some(Url::parse("https://forged.invalid/").unwrap()),
            })
        }
    }
    let jar = Arc::new(CookieJar::new());
    let client = Arc::new(ObscuraHttpClient::with_full_options(
        jar.clone(),
        None,
        true,
    ));
    *client.interceptor.write().await = Some(std::sync::Arc::new(Fixture));
    let transport = StealthHttpClient::with_policy(jar, None, client);
    let response = transport
        .fetch(&Url::parse("file:///fixture-not-read").unwrap())
        .await
        .unwrap();
    assert_eq!(response.request_referrer, None);
}

#[tokio::test(flavor = "current_thread")]
async fn scripted_transports_follow_document_and_redirect_policies() {
    for stealth in [false, true] {
        let html = "<!doctype html><pre id='result'>pending</pre><script>fetch('/redirect',{headers:{Referer:'https://forged.invalid/'}}).then(r=>r.text()).then(text=>document.getElementById('result').textContent=text)</script>";
        let mut responses = vec![ok_response("Content-Type: text/html\r\nReferrer-Policy: no-referrer\r\nReferrer-Policy: unsafe-url, future\r\n", html)];
        for policy in ["origin", "no-referrer", "unsafe-url"] {
            responses.push(format!("HTTP/1.1 302 Found\r\nLocation: /redirect\r\nReferrer-Policy: {policy}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"));
        }
        responses.push(ok_response("", "complete"));
        let (url, mut received) = http_fixture(responses).await;
        let context = Arc::new(obscura_browser::BrowserContext::with_storage_and_network(
            "script-referrer".into(),
            None,
            stealth,
            None,
            None,
            true,
        ));
        let mut page = obscura_browser::Page::new("script-referrer".into(), context);
        page.navigate(url.as_str()).await.unwrap();
        page.settle(2000).await;
        assert_eq!(
            page.evaluate("document.getElementById('result').textContent"),
            serde_json::json!("complete")
        );
        let origin = format!("{}/", url.origin().ascii_serialization());
        for expected in [None, Some(url.as_str()), Some(origin.as_str()), None, None] {
            let raw = tokio::time::timeout(std::time::Duration::from_secs(2), received.recv())
                .await
                .unwrap()
                .unwrap()
                .to_ascii_lowercase();
            let actual = raw
                .lines()
                .find_map(|line| line.strip_prefix("referer: ").map(str::trim));
            assert_eq!(actual, expected, "stealth={stealth}");
            assert!(!raw.contains("forged.invalid"));
        }
    }
}

async fn http_fixture(
    responses: Vec<String>,
) -> (Url, tokio::sync::mpsc::UnboundedReceiver<String>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (request_tx, request_rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        for response in responses {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let mut request = Vec::new();
            let mut buffer = [0u8; 2048];
            loop {
                let Ok(read) = stream.read(&mut buffer).await else {
                    return;
                };
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
                if let Some(end) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..end]).to_ascii_lowercase();
                    let body_length = headers
                        .lines()
                        .find_map(|line| {
                            line.strip_prefix("content-length:")
                                .and_then(|value| value.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if request.len() >= end + 4 + body_length {
                        break;
                    }
                }
            }
            let _ = request_tx.send(String::from_utf8_lossy(&request).into_owned());
            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.shutdown().await;
        }
    });
    (
        Url::parse(&format!("http://{address}/resource")).unwrap(),
        request_rx,
    )
}

fn ok_response(headers: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}
