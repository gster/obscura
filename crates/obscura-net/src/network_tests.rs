use crate::client::{
    is_forbidden_ip, merge_response_header, request_fetch_site, request_referrer, validate_url,
    CallbackRegistry, ObscuraHttpClient, ObscuraNetError, RequestCredentials, RequestMode,
    RequestInfo, ResourceRequest, ResourceType, Response, SsrfGuardResolver,
};
use crate::cookies::CookieJar;
use primp::dns::{Name, Resolve};
use super::StealthHttpClient;
use std::collections::HashMap;
use std::net::IpAddr;
use std::str::FromStr;
use std::sync::Arc;

/// Cloned worker policy keeps shared configuration and transport settings.
#[tokio::test]
async fn detached_client_shares_configuration_and_keeps_transport_settings() {
    let policy = ObscuraHttpClient::with_full_options(
        Arc::new(CookieJar::new()), Some("http://127.0.0.1:9"), false);
    policy.set_user_agent("Detached/1.0").await;
    policy.set_accept_language("fr-FR,fr;q=0.9").await;
    policy.set_extra_headers(HashMap::from([("x-probe".to_string(), "1".to_string())])).await;
    struct AlwaysContinue;
    #[async_trait::async_trait]
    impl crate::interceptor::RequestInterceptor for AlwaysContinue {
        async fn intercept(&self, _request: &crate::client::RequestInfo) -> crate::interceptor::InterceptAction {
            crate::interceptor::InterceptAction::Continue
        }
    }
    *policy.interceptor.write().await = Some(Arc::new(AlwaysContinue));

    let sibling = policy.detached();

    // shared identity: later mutations must reach the sibling
    assert!(Arc::ptr_eq(&policy.cookie_jar, &sibling.cookie_jar));
    assert!(Arc::ptr_eq(&policy.in_flight, &sibling.in_flight));
    assert!(Arc::ptr_eq(&policy.extra_headers, &sibling.extra_headers));
    assert!(Arc::ptr_eq(&policy.user_agent, &sibling.user_agent));
    assert!(Arc::ptr_eq(&policy.accept_language, &sibling.accept_language));
    assert!(Arc::ptr_eq(&policy.interceptor, &sibling.interceptor));

    policy.set_user_agent("Changed/2.0").await;
    assert_eq!(sibling.user_agent.read().await.as_str(), "Changed/2.0",
        "the worker must observe user-agent changes made after it was created");

    // transport settings carried over verbatim
    assert_eq!(sibling.proxy_url(), Some("http://127.0.0.1:9"),
        "a detached client that loses the proxy would silently scrape from the local IP");
    assert_eq!(sibling.allow_private_network, policy.allow_private_network);
    assert_eq!(sibling.block_trackers, policy.block_trackers);
}
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use url::Url;

fn ip(s: &str) -> IpAddr {
    IpAddr::from_str(s).unwrap()
}

// A response that repeats a header name (Link, Via, WWW-Authenticate, ...)
// must not lose all but the last line. See #913.
#[test]
fn response_headers_preserve_duplicate_values() {
    let mut headers = HashMap::new();
    merge_response_header(&mut headers, "link".into(), "<a>; rel=preload".into());
    merge_response_header(&mut headers, "link".into(), "<b>; rel=preconnect".into());
    assert_eq!(
        headers.get("link").map(String::as_str),
        Some("<a>; rel=preload, <b>; rel=preconnect"),
        "duplicate header lines must be combined per RFC 9110, not dropped"
    );
}

// Set-Cookie must not be comma-folded (RFC 6265); the cookie jar captures
// each line via get_all, so the map keeps it only as a presence signal.
#[test]
fn response_headers_do_not_fold_set_cookie() {
    let mut headers = HashMap::new();
    merge_response_header(&mut headers, "set-cookie".into(), "a=1".into());
    merge_response_header(&mut headers, "set-cookie".into(), "b=2".into());
    assert_eq!(
        headers.get("set-cookie").map(String::as_str),
        Some("b=2"),
        "Set-Cookie must stay a single (last) value, never comma-folded"
    );
}

#[test]
fn ipv4_private_and_special_ranges_are_forbidden() {
    for s in [
        "127.0.0.1",
        "127.5.6.7",
        "10.0.0.1",
        "172.16.0.1",
        "192.168.1.1",
        "169.254.169.254", // cloud-metadata endpoint
        "0.0.0.0",         // unspecified -> localhost (was a bypass)
        "255.255.255.255", // broadcast
        "192.0.2.1",       // documentation
    ] {
        assert!(is_forbidden_ip(ip(s)), "{s} should be forbidden");
    }
}

#[test]
fn public_ipv4_is_allowed() {
    for s in ["1.1.1.1", "8.8.8.8", "93.184.216.34"] {
        assert!(!is_forbidden_ip(ip(s)), "{s} should be allowed");
    }
}

// SEC-401 / #810 — std's is_private() only covers RFC1918, so CGNAT and
// other IANA special-purpose ranges (which host cloud metadata) must be
// blocked explicitly, including their IPv4-mapped IPv6 forms.
#[test]
fn ipv4_cgnat_and_iana_special_ranges_are_forbidden() {
    for s in [
        "100.64.0.1",             // CGNAT / RFC 6598 start
        "100.100.100.200",        // Alibaba Cloud metadata (CGNAT)
        "100.127.255.255",        // CGNAT end
        "198.18.0.1",             // benchmarking / RFC 2544 start
        "198.19.255.255",         // benchmarking end
        "192.88.99.1",            // 6to4 relay anycast / RFC 7526
        "::ffff:100.100.100.200", // v4-mapped CGNAT
    ] {
        assert!(is_forbidden_ip(ip(s)), "{s} should be forbidden");
    }
}

// Addresses just outside those prefixes must stay allowed (no over-block).
#[test]
fn ipv4_addresses_adjacent_to_special_ranges_stay_allowed() {
    for s in [
        "100.63.255.255", // just below 100.64.0.0/10
        "100.128.0.0",    // just above 100.127.255.255
        "198.17.255.255", // just below 198.18.0.0/15
        "198.20.0.0",     // just above 198.19.255.255
        "192.88.98.255",  // just below 192.88.99.0/24
        "192.88.100.0",   // just above 192.88.99.0/24
    ] {
        assert!(!is_forbidden_ip(ip(s)), "{s} should be allowed");
    }
}

#[test]
fn remaining_non_global_ipv4_ranges_are_forbidden_without_blocking_exceptions() {
    for s in [
        "0.1.2.3",
        "192.0.0.8",
        "192.0.0.192",
        "224.0.0.1",
        "239.255.255.255",
        "240.0.0.1",
        "255.255.255.254",
    ] {
        assert!(is_forbidden_ip(ip(s)), "{s} should be forbidden");
    }

    // IANA marks these specific protocol anycast addresses and these
    // special-purpose /24s globally reachable. Blocking them would be a
    // network compatibility regression, not an SSRF hardening win.
    for s in [
        "192.0.0.9",
        "192.0.0.10",
        "192.31.196.1",
        "192.52.193.1",
        "192.175.48.1",
    ] {
        assert!(!is_forbidden_ip(ip(s)), "{s} should be allowed");
    }
}

#[test]
fn ipv6_loopback_ula_linklocal_and_mapped_are_forbidden() {
    for s in [
        "::1",                    // loopback
        "::",                     // unspecified
        "fc00::1",                // unique-local (was a bypass)
        "fd12:3456:789a::1",      // unique-local
        "fe80::1",                // link-local
        "::ffff:127.0.0.1",       // v4-mapped loopback (was a bypass)
        "::ffff:169.254.169.254", // v4-mapped metadata
    ] {
        assert!(is_forbidden_ip(ip(s)), "{s} should be forbidden");
    }
}

#[test]
fn public_ipv6_is_allowed() {
    for s in [
        "2606:4700:4700::1111", // Cloudflare DNS
        "3ffe::1",               // below 3fff::/20
        "3fff:1000::1",          // above 3fff:fff::/20
    ] {
        assert!(!is_forbidden_ip(ip(s)), "{s} should be allowed");
    }
}

#[test]
fn ipv6_translation_cannot_hide_forbidden_ipv4() {
    for s in [
        "2002:7f00:1::",       // 6to4 loopback
        "2002:a9fe:a9fe::",    // 6to4 link-local metadata
        "2002:6464:64c8::",    // 6to4 CGNAT metadata
        "64:ff9b::7f00:1",     // NAT64 loopback
        "64:ff9b::a9fe:a9fe",  // NAT64 link-local metadata
        "64:ff9b::6464:64c8",  // NAT64 CGNAT metadata
        "64:ff9b:1::1",        // local-use translation prefix
    ] {
        assert!(is_forbidden_ip(ip(s)), "{s} should be forbidden");
    }

    for s in ["2002:808:808::", "64:ff9b::808:808"] {
        assert!(!is_forbidden_ip(ip(s)), "{s} should be allowed");
    }
}

#[test]
fn native_non_global_ipv6_ranges_are_forbidden() {
    for s in [
        "100::1",       // discard-only
        "2001:db8::1",  // documentation
        "3fff::1",      // documentation
        "ff02::1",      // link-local multicast
        "ff0e::1",      // global-scope multicast
    ] {
        assert!(is_forbidden_ip(ip(s)), "{s} should be forbidden");
    }
}

#[test]
fn validate_url_blocks_unspecified_and_allows_public() {
    // 0.0.0.0 previously slipped through validate_url's literal-host check.
    assert!(validate_url(&Url::parse("http://0.0.0.0:8080/").unwrap(), false).is_err());
    assert!(validate_url(&Url::parse("http://127.0.0.1/").unwrap(), false).is_err());
    assert!(validate_url(&Url::parse("http://example.com/").unwrap(), false).is_ok());
    assert!(
        validate_url(&Url::parse("http://[64:ff9b::7f00:1]/").unwrap(), false).is_err()
    );
    assert!(
        validate_url(&Url::parse("http://[2002:a9fe:a9fe::]/").unwrap(), false).is_err()
    );
    assert!(validate_url(&Url::parse("http://192.0.0.9/").unwrap(), false).is_ok());
    assert!(
        validate_url(&Url::parse("http://[64:ff9b::808:808]/").unwrap(), false).is_ok()
    );
    // The allow flag bypasses the guard (local-dev escape hatch).
    assert!(validate_url(&Url::parse("http://127.0.0.1/").unwrap(), true).is_ok());
}

#[test]
fn resource_profiles_use_type_specific_fetch_metadata() {
    let document = Url::parse("https://app.example/page?q=1#fragment").unwrap();
    let image = ResourceRequest::subresource(ResourceType::Image, &document);
    assert_eq!(image.mode, RequestMode::NoCors);
    assert_eq!(image.credentials, RequestCredentials::Include);
    assert_eq!(image.destination(), "image");
    assert!(image.accept().starts_with("image/webp"));

    let stylesheet = ResourceRequest::subresource(ResourceType::Stylesheet, &document);
    assert_eq!(stylesheet.destination(), "style");
    assert_eq!(stylesheet.accept(), "text/css,*/*;q=0.1");

    let font = ResourceRequest::subresource(ResourceType::Font, &document);
    assert_eq!(font.mode, RequestMode::Cors);
    assert_eq!(font.credentials, RequestCredentials::SameOrigin);
    assert_eq!(font.destination(), "font");
    assert_eq!(font.accept(), "*/*");

    assert!(image.sends_credentials_to(
        &Url::parse("https://cdn.example/image.png").unwrap()
    ));
    assert!(font.sends_credentials_to(
        &Url::parse("https://app.example/font.woff2").unwrap()
    ));
    assert!(!font.sends_credentials_to(
        &Url::parse("https://cdn.example/font.woff2").unwrap()
    ));

    let module = ResourceRequest::module_script(&document, &document);
    assert_eq!(module.resource_type, ResourceType::Script);
    assert_eq!(module.mode, RequestMode::Cors);
    assert_eq!(module.credentials, RequestCredentials::SameOrigin);
    assert_eq!(module.destination(), "script");
    assert_eq!(module.accept(), "*/*");
    assert!(module.sends_credentials_to(
        &Url::parse("https://app.example/chunk.js").unwrap()
    ));
    assert!(!module.sends_credentials_to(
        &Url::parse("https://cdn.example/chunk.js").unwrap()
    ));
}

#[test]
fn subresource_referrer_and_fetch_site_follow_default_browser_policy() {
    let source = Url::parse("https://user:secret@app.example/path?q=1#frag").unwrap();
    let request = ResourceRequest::subresource(ResourceType::Image, &source);
    let same_origin = Url::parse("https://app.example/image.png").unwrap();
    let cross_origin = Url::parse("https://cdn.example/image.png").unwrap();
    let downgrade = Url::parse("http://cdn.example/image.png").unwrap();

    assert_eq!(request_fetch_site(&request, &same_origin), "same-origin");
    assert_eq!(request_fetch_site(&request, &cross_origin), "cross-site");
    assert_eq!(
        request_referrer(&request, &same_origin).as_deref(),
        Some("https://app.example/path?q=1")
    );
    assert_eq!(
        request_referrer(&request, &cross_origin).as_deref(),
        Some("https://app.example/")
    );
    assert_eq!(request_referrer(&request, &downgrade), None);
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
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
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

fn redirect_to_self() -> String {
    "HTTP/1.1 302 Found\r\nLocation: /resource\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        .to_string()
}

#[tokio::test]
async fn primp_sends_cookies_in_path_and_creation_order_after_restore() {
    let (mut target, mut received) = http_fixture(vec![ok_response("", "ordered")]).await;
    target.set_path("/account/details");
    let original = CookieJar::new();
    original.set_cookie("session=root; Path=/", &target);
    original.set_cookie("first=old; Path=/account", &target);
    original.set_cookie("session=scoped; Path=/account", &target);
    original.set_cookie("first=updated; Path=/account", &target);
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("cookies.json");
    original.save_to_file(&file).unwrap();
    let restored = Arc::new(CookieJar::new());
    restored.load_from_file(&file).unwrap();
    let client = primp_client(restored, None, true);

    assert_eq!(client.fetch(&target).await.unwrap().body, b"ordered");
    let request = received.recv().await.unwrap();
    let cookie = request.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("cookie").then(|| value.trim())
    });
    assert_eq!(cookie, Some("first=updated; session=scoped; session=root"));
}

// WPT fetch/api/redirect/redirect-count: the 20th redirect must still be
// followed, the 21st must fail. Guards the `0..=max_redirects` boundary in
// fetch_with_profile_uncached; `0..max_redirects` regressed this to 19.
#[tokio::test]
async fn navigation_follows_twenty_redirects_but_not_twenty_one() {
    let mut pass: Vec<String> = (0..20).map(|_| redirect_to_self()).collect();
    pass.push(ok_response("", "arrived"));
    let (target, _rx) = http_fixture(pass).await;
    let client = primp_client(Arc::new(CookieJar::new()), None, true);
    let response = client
        .fetch(&target)
        .await
        .expect("the 20th redirect must be followed");
    assert_eq!(response.body, b"arrived");

    let fail: Vec<String> = (0..21).map(|_| redirect_to_self()).collect();
    let (target, _rx) = http_fixture(fail).await;
    let client = primp_client(Arc::new(CookieJar::new()), None, true);
    let err = client
        .fetch(&target)
        .await
        .expect_err("the 21st redirect must be too many");
    assert!(matches!(err, ObscuraNetError::TooManyRedirects(_)), "got {err:?}");
}

#[tokio::test]
async fn cross_origin_font_sends_origin_and_omits_cross_origin_cookies() {
    let (target, mut received) = http_fixture(vec![ok_response(
        "Access-Control-Allow-Origin: *\r\nSet-Cookie: rejected=1; Path=/\r\n",
        "font",
    )])
    .await;
    let initiator = Url::parse("http://127.0.0.1:1/page").unwrap();
    let jar = Arc::new(CookieJar::new());
    jar.set_cookie("seed=1; Path=/", &target);
    let client = primp_client(jar.clone(), None, true);

    let response = client
        .fetch_resource_with_callbacks(
            &target,
            ResourceRequest::subresource(ResourceType::Font, &initiator),
            None,
        )
        .await
        .unwrap();
    assert_eq!(response.body, b"font");
    let request = received.recv().await.unwrap().to_ascii_lowercase();
    assert!(request.contains("origin: http://127.0.0.1:1\r\n"));
    assert!(request.contains("sec-fetch-mode: cors\r\n"));
    assert!(request.contains("sec-fetch-dest: font\r\n"));
    assert!(!request.contains("cookie:"));
    assert_eq!(jar.get_cookie_header(&target), "seed=1");
}

// #849 — the OBSCURA_FETCH_MAX_BODY_BYTES override #581 gave fetch()/XHR
// must also reach the module-script cap; a large SPA bundle otherwise dies
// silently at a hardcoded 32 MiB while fetch() of the same URL succeeds.
// (nextest runs each test in its own process, so set_var cannot race.)
#[test]
fn module_script_cap_honours_the_fetch_body_env_override() {
    let u = Url::parse("https://example.com/app.mjs").unwrap();

    std::env::remove_var("OBSCURA_FETCH_MAX_BODY_BYTES");
    assert_eq!(
        ResourceRequest::module_script(&u, &u).max_response_bytes,
        32 * 1024 * 1024,
        "default module cap stays 32 MiB"
    );

    std::env::set_var("OBSCURA_FETCH_MAX_BODY_BYTES", "134217728");
    assert_eq!(
        ResourceRequest::module_script(&u, &u).max_response_bytes,
        128 * 1024 * 1024,
        "the env override must reach the module-script cap"
    );

    // Garbage stays on the default rather than panicking.
    std::env::set_var("OBSCURA_FETCH_MAX_BODY_BYTES", "not-a-number");
    assert_eq!(
        ResourceRequest::module_script(&u, &u).max_response_bytes,
        32 * 1024 * 1024,
    );
    std::env::remove_var("OBSCURA_FETCH_MAX_BODY_BYTES");
}

#[tokio::test]
async fn cross_origin_module_uses_cors_script_profile_without_credentials() {
    let (target, mut received) = http_fixture(vec![ok_response(
        "Access-Control-Allow-Origin: *\r\nSet-Cookie: rejected=1; Path=/\r\n",
        "export default 1;",
    )])
    .await;
    let initiator = Url::parse("http://127.0.0.1:1/page").unwrap();
    let importing_module = target.join("/parent.js").unwrap();
    let jar = Arc::new(CookieJar::new());
    jar.set_cookie("seed=1; Path=/", &target);
    let client = primp_client(jar.clone(), None, true);

    let response = client
        .fetch_resource_with_callbacks(
            &target,
            ResourceRequest::module_script(&initiator, &importing_module),
            None,
        )
        .await
        .unwrap();
    assert_eq!(response.body, b"export default 1;");
    let request = received.recv().await.unwrap().to_ascii_lowercase();
    assert!(request.contains("origin: http://127.0.0.1:1\r\n"));
    assert!(request.contains("sec-fetch-mode: cors\r\n"));
    assert!(request.contains("sec-fetch-dest: script\r\n"));
    assert!(request.contains(&format!("referer: {}\r\n", importing_module)));
    assert!(!request.contains("cookie:"));
    assert_eq!(jar.get_cookie_header(&target), "seed=1");
}

#[tokio::test]
async fn credentialed_cors_rejects_wildcard_and_accepts_exact_origin() {
    let initiator = Url::parse("http://127.0.0.1:1/page").unwrap();
    let wildcard = ok_response(
        "Access-Control-Allow-Origin: *\r\nAccess-Control-Allow-Credentials: true\r\n",
        "blocked",
    );
    let exact = ok_response(
        "Access-Control-Allow-Origin: http://127.0.0.1:1\r\nAccess-Control-Allow-Credentials: true\r\nSet-Cookie: accepted=1; Path=/\r\n",
        "allowed",
    );
    let (target, mut received) = http_fixture(vec![wildcard, exact]).await;
    let jar = Arc::new(CookieJar::new());
    jar.set_cookie("seed=1; Path=/", &target);
    let client = primp_client(jar.clone(), None, true);
    let mut request = ResourceRequest::subresource(ResourceType::Image, &initiator);
    request.mode = RequestMode::Cors;
    request.credentials = RequestCredentials::Include;

    let error = client
        .fetch_resource_with_callbacks(&target, request.clone(), None)
        .await
        .unwrap_err();
    assert!(matches!(error, ObscuraNetError::Cors(_)));
    client
        .fetch_resource_with_callbacks(&target, request, None)
        .await
        .unwrap();

    let first = received.recv().await.unwrap().to_ascii_lowercase();
    let second = received.recv().await.unwrap().to_ascii_lowercase();
    assert!(first.contains("cookie: seed=1\r\n"));
    assert!(second.contains("cookie: seed=1\r\n"));
    let cookies = jar.get_cookie_header(&target);
    assert!(cookies.contains("seed=1"));
    assert!(cookies.contains("accepted=1"));
}

#[tokio::test]
async fn same_origin_font_needs_no_cors_header_and_sends_cookies() {
    let (target, mut received) = http_fixture(vec![ok_response("", "same")]).await;
    let mut initiator = target.clone();
    initiator.set_path("/page");
    let jar = Arc::new(CookieJar::new());
    jar.set_cookie("same=1; Path=/", &target);
    let client = primp_client(jar, None, true);
    client
        .fetch_resource_with_callbacks(
            &target,
            ResourceRequest::subresource(ResourceType::Font, &initiator),
            None,
        )
        .await
        .unwrap();
    let request = received.recv().await.unwrap().to_ascii_lowercase();
    assert!(!request.contains("origin:"));
    assert!(request.contains("cookie: same=1\r\n"));
}

#[tokio::test]
async fn accept_language_override_reaches_the_wire() {
    let (target, mut received) = http_fixture(vec![ok_response("", "ok")]).await;
    let jar = Arc::new(CookieJar::new());
    let policy = Arc::new(ObscuraHttpClient::with_full_options(jar.clone(), None, true));
    let mut spec = crate::PersonaSpec::preset(super::StealthProfile::WindowsChrome145);
    spec.language = Some("de-DE".to_string());
    spec.languages = Some(vec!["de-DE".to_string(), "de".to_string()]);
    spec.accept_language = Some("de-DE,de;q=0.9".to_string());
    let persona = spec.compile().unwrap();
    let client = StealthHttpClient::with_policy_persona(jar, None, policy, &persona);

    client.fetch(&target).await.unwrap();

    let request = received.recv().await.unwrap();
    assert!(
        request
            .lines()
            .any(|line| line.eq_ignore_ascii_case("accept-language: de-DE,de;q=0.9")),
        "request did not contain the configured Accept-Language header: {request}"
    );
}

#[tokio::test]
async fn response_limits_reject_content_length_and_streamed_overflow() {
    let advertised = "HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\n";
    let chunked = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n4\r\nabcd\r\n4\r\nefgh\r\n0\r\n\r\n";
    let (target, _) = http_fixture(vec![advertised.to_string(), chunked.to_string()]).await;
    let client = primp_client(
        Arc::new(CookieJar::new()),
        None,
        true,
    );
    let initiator = target.clone();
    let request = ResourceRequest::subresource(ResourceType::Image, &initiator)
        .with_max_response_bytes(6);

    for _ in 0..2 {
        let error = client
            .fetch_resource_with_callbacks(&target, request.clone(), None)
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            ObscuraNetError::ResponseTooLarge { limit: 6, .. }
        ));
        assert_eq!(client.active_requests(), 0);
    }
}

async fn hanging_fixture() -> (Url, tokio::sync::oneshot::Receiver<()>) {
    use tokio::io::AsyncReadExt;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let Ok((mut stream, _)) = listener.accept().await else {
            return;
        };
        let mut buffer = [0u8; 2048];
        let _ = stream.read(&mut buffer).await;
        let _ = started_tx.send(());
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
    });
    (
        Url::parse(&format!("http://{address}/hang")).unwrap(),
        started_rx,
    )
}

async fn cancelled_shared_fetch_fixture(
) -> (Url, tokio::sync::oneshot::Receiver<()>, Arc<AtomicUsize>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(AtomicUsize::new(0));
    let observed = requests.clone();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let mut first_stream = None;
        let mut started_tx = Some(started_tx);
        for index in 0..3 {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let mut request = [0u8; 2048];
            let _ = stream.read(&mut request).await;
            observed.fetch_add(1, Ordering::SeqCst);
            if index == 0 {
                if let Some(started_tx) = started_tx.take() {
                    let _ = started_tx.send(());
                }
                // Hold the transport open until the leader task is
                // cancelled. The next two connections prove both the
                // waiting follower retry and a fresh cache leader work.
                first_stream = Some(stream);
                continue;
            }
            let body = "shared";
            let response = format!(
                "HTTP/1.1 200 OK\r\nCache-Control: public, max-age=3600\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len(),
            );
            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.shutdown().await;
        }
        drop(first_stream);
    });
    (
        Url::parse(&format!("http://{address}/shared.js")).unwrap(),
        started_rx,
        requests,
    )
}

#[tokio::test]
async fn cancellation_returns_active_requests_to_zero() {
    let (target, started) = hanging_fixture().await;
    let client = Arc::new(primp_client(
        Arc::new(CookieJar::new()),
        None,
        true,
    ));
    let task = tokio::spawn({
        let client = client.clone();
        async move { client.fetch(&target).await }
    });
    started.await.unwrap();
    assert_eq!(client.active_requests(), 1);
    task.abort();
    let _ = task.await;
    assert_eq!(client.active_requests(), 0);
}

#[tokio::test]
async fn cancelled_shared_subresource_leader_wakes_follower_and_clears_slot() {
    let (target, started, network_requests) = cancelled_shared_fetch_fixture().await;
    let initiator = target.join("/page.html").unwrap();
    let request = ResourceRequest::subresource(ResourceType::Script, &initiator);
    let client = Arc::new(primp_client(
        Arc::new(CookieJar::new()),
        None,
        true,
    ));

    let leader = tokio::spawn({
        let client = client.clone();
        let target = target.clone();
        let request = request.clone();
        async move {
            client
                .fetch_resource_with_callbacks(&target, request, None)
                .await
        }
    });
    started.await.unwrap();

    let follower = tokio::spawn({
        let client = client.clone();
        let target = target.clone();
        let request = request.clone();
        async move {
            client
                .fetch_resource_with_callbacks(&target, request, None)
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let follower_is_waiting = client
                .resource_loader
                .lock()
                .unwrap()
                .shared_fetches
                .values()
                .next()
                .is_some_and(|sender| sender.receiver_count() > 0);
            if follower_is_waiting {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("follower did not join the shared fetch");

    leader.abort();
    let _ = leader.await;
    let response = tokio::time::timeout(Duration::from_secs(2), follower)
        .await
        .expect("follower remained blocked after leader cancellation")
        .unwrap()
        .unwrap();
    assert_eq!(response.body, b"shared");

    // The follower intentionally retried without populating the cache.
    // A subsequent request must be able to install a fresh leader, and
    // its successful response is then reusable.
    client
        .fetch_resource_with_callbacks(&target, request.clone(), None)
        .await
        .unwrap();
    client
        .fetch_resource_with_callbacks(&target, request, None)
        .await
        .unwrap();
    assert_eq!(network_requests.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn transport_timeout_returns_active_requests_to_zero() {
    let (target, started) = hanging_fixture().await;
    let client = primp_client(
        Arc::new(CookieJar::new()),
        None,
        true,
    );
    let headers = HashMap::new();
    let fetch = client.send_single_with_limit(
        "GET", &target, &headers, &[], true, true, 1024, Duration::from_millis(25),
    );
    let (_, result) = tokio::join!(started, fetch);
    assert!(result.is_err());
    assert_eq!(client.active_requests(), 0);
}

#[tokio::test]
async fn callbacks_fire_once_across_redirects() {
    let redirect = "HTTP/1.1 302 Found\r\nLocation: /final\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
    let (target, _) = http_fixture(vec![redirect.to_string(), ok_response("", "done")]).await;
    let client = primp_client(
        Arc::new(CookieJar::new()),
        None,
        true,
    );
    let callbacks = CallbackRegistry::new();
    let requests = Arc::new(AtomicUsize::new(0));
    let responses = Arc::new(AtomicUsize::new(0));
    let request_count = requests.clone();
    callbacks.add_request(Arc::new(move |_| {
        request_count.fetch_add(1, Ordering::SeqCst);
    }));
    let response_count = responses.clone();
    callbacks.add_response(Arc::new(move |_, _| {
        response_count.fetch_add(1, Ordering::SeqCst);
    }));

    client
        .fetch_with_callbacks(&target, Some(&callbacks))
        .await
        .unwrap();
    assert_eq!(requests.load(Ordering::SeqCst), 1);
    assert_eq!(responses.load(Ordering::SeqCst), 1);
}

fn callback_request(path: &str, body: Vec<u8>) -> RequestInfo {
    RequestInfo {
        body,
        url: Url::parse(&format!("https://callback.test/{path}")).unwrap(),
        method: "POST".into(),
        headers: HashMap::from([
            ("Authorization".into(), "Bearer complete-secret".into()),
            ("Cookie".into(), "session=complete-secret".into()),
        ]),
        raw_headers: Some(crate::HeaderCapture {
            capture_stage: "transportRequest",
            encoding: "base64",
            fields: vec![
                crate::RawHeader { name: b"X-Binary".to_vec(), value: vec![0, 0xff, b'A'] },
                crate::RawHeader { name: b"Set-Cookie".to_vec(), value: b"first=complete".to_vec() },
                crate::RawHeader { name: b"Set-Cookie".to_vec(), value: b"second=complete".to_vec() },
            ],
        }),
        resource_type: ResourceType::Fetch,
    }
}

#[tokio::test]
async fn request_callback_can_register_a_callback_without_losing_observations() {
    let callbacks = Arc::new(CallbackRegistry::new());
    let weak = Arc::downgrade(&callbacks);
    let installed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let observations = Arc::new(std::sync::Mutex::new(Vec::new()));

    let first_observations = observations.clone();
    let first_installed = installed.clone();
    callbacks.add_request(Arc::new(move |request| {
        first_observations.lock().unwrap().push((
            "first",
            request.url.path().to_string(),
            request.body.clone(),
            request.raw_headers.clone(),
        ));
        if !first_installed.swap(true, Ordering::SeqCst) {
            let late_observations = first_observations.clone();
            weak.upgrade().expect("registry still owned").add_request(Arc::new(move |request| {
                late_observations.lock().unwrap().push((
                    "late",
                    request.url.path().to_string(),
                    request.body.clone(),
                    request.raw_headers.clone(),
                ));
            }));
        }
    }));

    let first = callback_request("first", vec![0, 1, 0xff]);
    callbacks.fire_request(&first).await;
    let second = callback_request("second", (0_u8..=255).collect());
    callbacks.fire_request(&second).await;

    let observations = observations.lock().unwrap();
    assert_eq!(
        observations.iter().map(|(owner, path, _, _)| (*owner, path.as_str())).collect::<Vec<_>>(),
        vec![("first", "/first"), ("first", "/second"), ("late", "/second")],
        "registration during dispatch starts with the next complete observation"
    );
    assert_eq!(observations[0].2, vec![0, 1, 0xff]);
    assert_eq!(observations[1].2, (0_u8..=255).collect::<Vec<_>>());
    assert_eq!(observations[1].3, second.raw_headers);
    assert_eq!(observations[2].3, second.raw_headers);
}

#[tokio::test]
async fn response_callback_can_remove_itself_without_a_false_not_found_result() {
    let callbacks = Arc::new(CallbackRegistry::new());
    let weak = Arc::downgrade(&callbacks);
    let callback_id = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let removals = Arc::new(std::sync::Mutex::new(Vec::new()));
    let observations = Arc::new(std::sync::Mutex::new(Vec::new()));

    let observed_removals = removals.clone();
    let observed_responses = observations.clone();
    let observed_id = callback_id.clone();
    let id = callbacks.add_response(Arc::new(move |request, response| {
        observed_responses.lock().unwrap().push((
            request.body.clone(),
            response.body.clone(),
            response.raw_headers.clone(),
        ));
        observed_removals.lock().unwrap().push(
            weak.upgrade().expect("registry still owned")
                .remove_response(observed_id.load(Ordering::SeqCst)),
        );
    }));
    callback_id.store(id, Ordering::SeqCst);

    let request = callback_request("response", vec![0, 0xff]);
    let response = Response {
        url: Url::parse("https://callback.test/response").unwrap(),
        status: 200,
        headers: HashMap::from([("set-cookie".into(), "second=complete".into())]),
        raw_headers: Some(crate::HeaderCapture {
            capture_stage: "transportResponse",
            encoding: "base64",
            fields: vec![
                crate::RawHeader { name: b"Set-Cookie".to_vec(), value: b"first=complete".to_vec() },
                crate::RawHeader { name: b"Set-Cookie".to_vec(), value: b"second=complete".to_vec() },
                crate::RawHeader { name: b"X-Binary".to_vec(), value: vec![0xff, 0] },
            ],
        }),
        body: (0_u8..=255).rev().collect(),
        redirected_from: vec![Url::parse("https://callback.test/original").unwrap()],
        request_referrer: Some(Url::parse("https://callback.test/referrer").unwrap()),
        request_raw_headers: request.raw_headers.clone(),
    };
    callbacks.fire_response(&request, &response).await;
    callbacks.fire_response(&request, &response).await;

    assert_eq!(*removals.lock().unwrap(), vec![true]);
    let observations = observations.lock().unwrap();
    assert_eq!(observations.len(), 1, "self-removal applies to the next observation");
    assert_eq!(observations[0].0, request.body);
    assert_eq!(observations[0].1, response.body);
    assert_eq!(observations[0].2, response.raw_headers);
}

#[test]
fn callback_capture_destructors_reenter_after_the_registry_lock_is_released() {
    struct ReenterOnDrop {
        callbacks: std::sync::Weak<CallbackRegistry>,
        response: bool,
        completed: std::sync::mpsc::SyncSender<u64>,
    }

    impl Drop for ReenterOnDrop {
        fn drop(&mut self) {
            let callbacks = self.callbacks.upgrade().expect("registry still owned");
            let id = if self.response {
                callbacks.add_response(Arc::new(|_, _| {}))
            } else {
                callbacks.add_request(Arc::new(|_| {}))
            };
            self.completed.send(id).unwrap();
        }
    }

    let callbacks = Arc::new(CallbackRegistry::new());
    for response in [false, true] {
        let (completed_tx, completed_rx) = std::sync::mpsc::sync_channel(1);
        let reentry = ReenterOnDrop {
            callbacks: Arc::downgrade(&callbacks),
            response,
            completed: completed_tx,
        };
        let id = if response {
            callbacks.add_response(Arc::new(move |_, _| {
                std::hint::black_box(&reentry);
            }))
        } else {
            callbacks.add_request(Arc::new(move |_| {
                std::hint::black_box(&reentry);
            }))
        };
        let owned_callbacks = callbacks.clone();
        let (removed_tx, removed_rx) = std::sync::mpsc::sync_channel(1);
        let worker = std::thread::spawn(move || {
            let removed = if response {
                owned_callbacks.remove_response(id)
            } else {
                owned_callbacks.remove_request(id)
            };
            removed_tx.send(removed).unwrap();
        });

        assert_eq!(
            completed_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            id + 1,
            "captured destructor could not reenter the registry"
        );
        assert!(removed_rx.recv_timeout(Duration::from_secs(1)).unwrap());
        worker.join().unwrap();
    }
}

async fn cacheable_resource_fixture(
    status: u16,
    headers: &'static str,
) -> (Url, Arc<AtomicUsize>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(AtomicUsize::new(0));
    let observed = requests.clone();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let observed = observed.clone();
            tokio::spawn(async move {
                let mut request = [0u8; 2048];
                let _ = stream.read(&mut request).await;
                observed.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(80)).await;
                let body = "globalThis.__sharedRuns=(globalThis.__sharedRuns||0)+1;";
                let response = format!(
                    "HTTP/1.1 {status} Test\r\nContent-Type: application/javascript\r\nContent-Length: {}\r\n{headers}Connection: close\r\n\r\n{body}",
                    body.len(),
                );
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.shutdown().await;
            });
        }
    });
    (
        Url::parse(&format!("http://{address}/shared.js")).unwrap(),
        requests,
    )
}

#[tokio::test]
async fn cacheable_identical_subresources_share_one_in_flight_request() {
    let (url, network_requests) = cacheable_resource_fixture(
        200,
        "Cache-Control: public, max-age=3600\r\nVary: Accept-Language\r\n",
    )
    .await;
    let initiator = url.join("/page.html").unwrap();
    let client = Arc::new(primp_client(
        Arc::new(CookieJar::new()),
        None,
        true,
    ));
    let callbacks = Arc::new(CallbackRegistry::new());
    let callback_requests = Arc::new(AtomicUsize::new(0));
    let callback_responses = Arc::new(AtomicUsize::new(0));
    let observed_requests = callback_requests.clone();
    callbacks.add_request(Arc::new(move |_| {
        observed_requests.fetch_add(1, Ordering::SeqCst);
    }));
    let observed_responses = callback_responses.clone();
    callbacks.add_response(Arc::new(move |_, _| {
        observed_responses.fetch_add(1, Ordering::SeqCst);
    }));

    let mut fetches = tokio::task::JoinSet::new();
    for _ in 0..32 {
        let client = client.clone();
        let callbacks = callbacks.clone();
        let url = url.clone();
        let request = ResourceRequest::subresource(ResourceType::Script, &initiator);
        fetches.spawn(async move {
            client
                .fetch_resource_with_callbacks(&url, request, Some(&callbacks))
                .await
                .unwrap()
        });
    }
    let mut responses = Vec::new();
    while let Some(response) = fetches.join_next().await {
        responses.push(response.unwrap());
    }

    assert_eq!(responses.len(), 32);
    assert!(responses.iter().all(|response| response.status == 200));
    assert_eq!(network_requests.load(Ordering::SeqCst), 1);
    assert_eq!(callback_requests.load(Ordering::SeqCst), 32);
    assert_eq!(callback_responses.load(Ordering::SeqCst), 32);
}

#[tokio::test]
async fn coalesced_followers_and_cache_hits_preserve_document_scoped_activity() {
    let (url, network_requests) = cacheable_resource_fixture(
        200,
        "Cache-Control: public, max-age=3600\r\n",
    )
    .await;
    let initiator = url.join("/page.html").unwrap();
    let client = Arc::new(primp_client(
        Arc::new(CookieJar::new()),
        None,
        true,
    ));
    let generation = client.begin_network_document();

    let mut fetches = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let client = client.clone();
        let url = url.clone();
        let request = ResourceRequest::subresource(ResourceType::Script, &initiator)
            .with_network_activity_generation(Some(generation));
        fetches.spawn(async move {
            client
                .fetch_resource_with_callbacks(&url, request, None)
                .await
                .unwrap()
        });
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        if client.network_activity_snapshot().active == 8 {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "followers were not counted");
        tokio::task::yield_now().await;
    }
    while let Some(response) = fetches.join_next().await {
        assert_eq!(response.unwrap().status, 200);
    }
    assert_eq!(network_requests.load(Ordering::SeqCst), 1);
    assert_eq!(client.network_activity_snapshot().active, 0);

    let successor = client.begin_network_document();
    let before_stale_hit = client.network_activity_snapshot();
    let stale_request = ResourceRequest::subresource(ResourceType::Script, &initiator)
        .with_network_activity_generation(Some(generation));
    client.fetch_resource_with_callbacks(&url, stale_request, None)
        .await.unwrap();
    assert_eq!(
        client.network_activity_snapshot(),
        before_stale_hit,
        "an old document cache hit must not reset the successor quiet window",
    );

    let before_current_hit = client.network_activity_snapshot();
    let current_request = ResourceRequest::subresource(ResourceType::Script, &initiator)
        .with_network_activity_generation(Some(successor));
    client.fetch_resource_with_callbacks(&url, current_request, None)
        .await.unwrap();
    let after_current_hit = client.network_activity_snapshot();
    assert!(after_current_hit.epoch > before_current_hit.epoch);
    assert!(after_current_hit.below_zero_since > before_current_hit.below_zero_since);
    assert_eq!(network_requests.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn cacheable_identical_module_scripts_share_one_in_flight_request() {
    let (url, network_requests) = cacheable_resource_fixture(
        200,
        "Cache-Control: public, max-age=3600\r\n",
    )
    .await;
    let initiator = url.join("/app.js").unwrap();
    let client = Arc::new(primp_client(
        Arc::new(CookieJar::new()),
        None,
        true,
    ));

    let mut fetches = tokio::task::JoinSet::new();
    for _ in 0..16 {
        let client = client.clone();
        let url = url.clone();
        let request = ResourceRequest::module_script(&initiator, &initiator);
        fetches.spawn(async move {
            client
                .fetch_resource_with_callbacks(&url, request, None)
                .await
                .unwrap()
        });
    }
    let mut responses = Vec::new();
    while let Some(response) = fetches.join_next().await {
        responses.push(response.unwrap());
    }

    assert_eq!(responses.len(), 16);
    assert!(responses.iter().all(|response| response.status == 200));
    assert_eq!(network_requests.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn distinct_subresource_urls_do_not_coalesce() {
    let (url, network_requests) =
        cacheable_resource_fixture(200, "Cache-Control: public, max-age=3600\r\n").await;
    let initiator = url.join("/page.html").unwrap();
    let client = Arc::new(primp_client(
        Arc::new(CookieJar::new()),
        None,
        true,
    ));

    let mut fetches = tokio::task::JoinSet::new();
    for index in 0..24 {
        let client = client.clone();
        let url = url.join(&format!("/distinct/{index}.js")).unwrap();
        let request = ResourceRequest::subresource(ResourceType::Script, &initiator);
        fetches.spawn(async move {
            client
                .fetch_resource_with_callbacks(&url, request, None)
                .await
                .unwrap()
        });
    }
    let mut responses = Vec::new();
    while let Some(response) = fetches.join_next().await {
        responses.push(response.unwrap());
    }

    assert_eq!(responses.len(), 24);
    assert_eq!(network_requests.load(Ordering::SeqCst), 24);
}

#[tokio::test]
async fn no_store_vary_star_and_error_responses_are_not_reused() {
    for (status, headers) in [
        (200, "Cache-Control: no-store\r\n"),
        (200, "Cache-Control: public, max-age=3600\r\nVary: *\r\n"),
        (500, "Cache-Control: public, max-age=3600\r\n"),
    ] {
        let (url, network_requests) = cacheable_resource_fixture(status, headers).await;
        let initiator = url.join("/page.html").unwrap();
        let client =
            primp_client(Arc::new(CookieJar::new()), None, true);
        let request = ResourceRequest::subresource(ResourceType::Script, &initiator);
        client
            .fetch_resource_with_callbacks(&url, request.clone(), None)
            .await
            .unwrap();
        client
            .fetch_resource_with_callbacks(&url, request, None)
            .await
            .unwrap();
        assert_eq!(
            network_requests.load(Ordering::SeqCst),
            2,
            "status={status} headers={headers:?}",
        );
    }
}

#[tokio::test]
async fn authorization_and_cookie_bearing_requests_bypass_resource_cache() {
    for header in [
        ("Authorization", "Bearer secret"),
        ("Cookie", "session=secret"),
    ] {
        let (url, network_requests) =
            cacheable_resource_fixture(200, "Cache-Control: public, max-age=3600\r\n").await;
        let initiator = url.join("/page.html").unwrap();
        let client =
            primp_client(Arc::new(CookieJar::new()), None, true);
        client
            .set_extra_headers(HashMap::from([(
                header.0.to_string(),
                header.1.to_string(),
            )]))
            .await;
        let request = ResourceRequest::subresource(ResourceType::Script, &initiator);
        client
            .fetch_resource_with_callbacks(&url, request.clone(), None)
            .await
            .unwrap();
        client
            .fetch_resource_with_callbacks(&url, request, None)
            .await
            .unwrap();
        assert_eq!(
            network_requests.load(Ordering::SeqCst),
            2,
            "header={header:?}"
        );
    }
}

#[tokio::test]
async fn resolver_blocks_hostname_that_resolves_to_loopback() {
    // localtest.me is a public DNS name that resolves to 127.0.0.1 — the
    // canonical DNS-rebinding test. The guard must reject it. If DNS is
    // unavailable the lookup itself errors (also Err), so the assertion
    // holds either way.
    let r = SsrfGuardResolver::new(false);
    let res = r.resolve(Name::from_str("localtest.me").unwrap()).await;
    assert!(res.is_err(), "localtest.me -> 127.0.0.1 must be blocked");
}

#[tokio::test]
async fn resolver_does_not_ssrf_block_public_host() {
    // A public host must not be SSRF-blocked. Tolerate a no-network sandbox
    // by only failing on an actual SSRF rejection, not a lookup failure.
    let r = SsrfGuardResolver::new(false);
    match r.resolve(Name::from_str("example.com").unwrap()).await {
        Ok(_) => {}
        Err(e) => assert!(
            !e.to_string().contains("SSRF blocked"),
            "example.com wrongly SSRF-blocked: {e}"
        ),
    }
}

/// Mint a throwaway CA plus a 127.0.0.1 leaf it signed, and serve one
/// canned HTTPS response with the leaf on an ephemeral port. Returns the
/// port and the CA certificate as PEM.
async fn https_fixture_with_private_ca() -> (u16, String) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let ca_key = rcgen::KeyPair::generate().unwrap();
    let mut ca_params = rcgen::CertificateParams::new(Vec::new()).unwrap();
    ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let ca_cert = ca_params.self_signed(&ca_key).unwrap();

    let leaf_key = rcgen::KeyPair::generate().unwrap();
    let leaf_params =
        rcgen::CertificateParams::new(vec!["127.0.0.1".to_string()]).unwrap();
    let leaf_cert = leaf_params.signed_by(&leaf_key, &ca_cert, &ca_key).unwrap();

    let certs = vec![tokio_rustls::rustls::pki_types::CertificateDer::from(
        leaf_cert.der().to_vec(),
    )];
    let key = tokio_rustls::rustls::pki_types::PrivateKeyDer::Pkcs8(
        tokio_rustls::rustls::pki_types::PrivatePkcs8KeyDer::from(
            leaf_key.serialize_der(),
        ),
    );
    let config = tokio_rustls::rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(stream).await else {
                    return; // Handshake rejection is the point of one test.
                };
                let mut buf = [0u8; 1024];
                let _ = tls.read(&mut buf).await;
                let body = "private ca ok";
                let resp = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = tls.write_all(resp.as_bytes()).await;
                let _ = tls.shutdown().await;
            });
        }
    });

    (port, ca_cert.pem())
}

// The configured-roots tests mutate process environment variables. They are only correct under
// `cargo nextest` (one process per test), the same constraint the whole
// workspace already has.

#[tokio::test]
async fn configured_roots_trust_a_private_ca_via_ssl_cert_file() {
    let (port, ca_pem) = https_fixture_with_private_ca().await;
    let ca_file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(ca_file.path(), ca_pem).unwrap();
    std::env::set_var("SSL_CERT_FILE", ca_file.path());

    let client =
        primp_client(Arc::new(CookieJar::new()), None, true);
    let url = Url::parse(&format!("https://127.0.0.1:{port}/")).unwrap();
    let resp = client.fetch(&url).await.expect("private CA in SSL_CERT_FILE must be trusted");
    assert_eq!(resp.status, 200);
    assert_eq!(resp.text(), "private ca ok");
}

#[tokio::test]
async fn configured_roots_trust_a_private_ca_via_ssl_cert_dir() {
    let (port, ca_pem) = https_fixture_with_private_ca().await;
    let ca_dir = tempfile::tempdir().unwrap();
    std::fs::write(ca_dir.path().join("private-ca.pem"), ca_pem).unwrap();
    std::env::set_var("SSL_CERT_DIR", ca_dir.path());

    let client =
        primp_client(Arc::new(CookieJar::new()), None, true);
    let url = Url::parse(&format!("https://127.0.0.1:{port}/")).unwrap();
    let resp = client
        .fetch(&url)
        .await
        .expect("private CA in SSL_CERT_DIR must be trusted");
    assert_eq!(resp.status, 200);
    assert_eq!(resp.text(), "private ca ok");
}

#[tokio::test]
async fn private_ca_is_still_rejected_without_ssl_cert_file() {
    // The same fixture that the SSL_CERT_FILE test trusts must fail here. The
    // listener is reachable (same setup), so an Err can only be TLS.
    let (port, _ca_pem) = https_fixture_with_private_ca().await;
    let client =
        primp_client(Arc::new(CookieJar::new()), None, true);
    let url = Url::parse(&format!("https://127.0.0.1:{port}/")).unwrap();
    assert!(client.fetch(&url).await.is_err(), "unknown CA must be rejected");
}

fn primp_client(jar: Arc<CookieJar>, proxy: Option<&str>, allow_private: bool) -> StealthHttpClient {
    let policy = Arc::new(ObscuraHttpClient::with_full_options(jar.clone(), proxy, allow_private));
    StealthHttpClient::with_policy(
        jar,
        proxy,
        policy,
        &crate::EffectivePersona::builtin(super::StealthProfile::WindowsChrome145),
    )
}
