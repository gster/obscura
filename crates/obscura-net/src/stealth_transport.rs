//! Primp transport; browser network policy lives in stealth_client.rs.
use std::sync::Arc;
use std::time::Duration;
use futures_util::{StreamExt, stream::BoxStream};
use url::Url;
use http::header::{HeaderMap, HeaderName, HeaderValue};
use super::{ObscuraNetError, StealthProfile};

struct Resolver {
    allow_private: bool,
    proxy_host: Option<String>,
}

impl primp::dns::Resolve for Resolver {
    fn resolve(&self, name: primp::dns::Name) -> primp::dns::Resolving {
        let host = name.as_str().to_owned();
        // Proxy::all plus no_proxy routes every request through this explicit
        // endpoint. Allow its address without granting private destination access;
        // Obscura still validates each target and redirect before transport.
        let allow = self.allow_private || super::env_allows_private_network()
            || self.proxy_host.as_deref() == Some(host.as_str());
        Box::pin(async move {
            let addrs = super::resolve_guarded(&host, allow).await?;
            Ok(Box::new(addrs.into_iter()) as primp::dns::Addrs)
        })
    }
}

pub(super) struct Client {
    client: std::sync::OnceLock<Result<primp::Client, String>>,
    defaults: HeaderMap,
    profile: StealthProfile,
    proxy: Option<String>,
    allow_private: bool,
    server_padding_request: Option<u16>,
}

impl Client {
    #[cfg(test)]
    pub fn new(
        profile: StealthProfile,
        proxy: Option<&str>,
        allow_private: bool,
        accept_language: Option<&str>,
        do_not_track: Option<&str>,
    ) -> Self {
        Self::with_browser_flavor(profile, proxy, allow_private, accept_language, do_not_track, crate::BrowserFlavor::Chrome)
    }

    #[cfg(test)]
    pub fn with_browser_flavor(
        profile: StealthProfile,
        proxy: Option<&str>,
        allow_private: bool,
        accept_language: Option<&str>,
        do_not_track: Option<&str>,
        browser_flavor: crate::BrowserFlavor,
    ) -> Self {
        Self::with_tls_options(profile, proxy, allow_private, accept_language, do_not_track, browser_flavor, None)
    }

    pub fn with_tls_options(
        profile: StealthProfile,
        proxy: Option<&str>,
        allow_private: bool,
        accept_language: Option<&str>,
        do_not_track: Option<&str>,
        browser_flavor: crate::BrowserFlavor,
        server_padding_request: Option<u16>,
    ) -> Self {
        let mut headers = HeaderMap::new();
        let major = profile.full_version().split('.').next().unwrap().parse().expect("profile major version");
        let brands = browser_flavor.sec_ch_ua(major);
        for (name, value) in [
            ("user-agent", profile.user_agent()),
            ("sec-ch-ua", brands.as_str()),
            ("sec-ch-ua-mobile", "?0"), ("sec-ch-ua-platform", match profile { StealthProfile::MacChrome152 | StealthProfile::MacChrome153 => "\"macOS\"", StealthProfile::WindowsChrome145 => "\"Windows\"" }),
            ("accept", "*/*"),
            ("accept-language", accept_language.unwrap_or("en-US,en;q=0.9")),
            ("accept-encoding", "gzip, deflate, br, zstd"), ("priority", "u=0, i"),
        ] { headers.insert(name, HeaderValue::from_str(value).expect("valid persona header")); }
        if let Some(value) = do_not_track {
            headers.insert("dnt", HeaderValue::from_str(value).expect("valid persona DNT"));
        }
        Self {
            client: std::sync::OnceLock::new(), defaults: headers, profile,
            proxy: proxy.map(str::to_owned), allow_private, server_padding_request,
        }
    }

    // Worker/frame inheritance must not initialize a network pool or scan the
    // CA store unless that identity actually sends a request on this runtime.
    fn build(&self) -> Result<primp::Client, String> {
        let (browser, os) = match self.profile {
            StealthProfile::WindowsChrome145 => (primp::Impersonate::ChromeV145, primp::ImpersonateOS::Windows),
            StealthProfile::MacChrome152 => (primp::Impersonate::ChromeV152, primp::ImpersonateOS::MacOS),
            StealthProfile::MacChrome153 => (primp::Impersonate::ChromeV153, primp::ImpersonateOS::MacOS),
        };
        let mut builder = primp::Client::builder()
            .impersonate(browser)
            .impersonate_os(os)
            .tls_server_padding_request(self.server_padding_request)
            .no_proxy()
            .redirect(primp::redirect::Policy::none())
            .retry(primp::retry::never())
            .dns_resolver(Arc::new(Resolver {
                allow_private: self.allow_private,
                proxy_host: self.proxy.as_deref().and_then(|s| Url::parse(s).ok())
                    .and_then(|u| u.host_str().map(|h| h.trim_matches(['[', ']']).to_owned())),
            }))
            .timeout(Duration::from_secs(30));
        if let Some(proxy) = self.proxy.as_deref() {
            builder = builder.proxy(primp::Proxy::all(proxy)
                .map_err(|error| format!("Invalid proxy {proxy}: {error}"))?);
        }
        for path in crate::client::configured_root_paths() {
            let bytes = match std::fs::read(&path) {
                Ok(bytes) => bytes,
                Err(error) => {
                    tracing::warn!(%error, path = %path.display(), "failed to read CA certificate file");
                    continue;
                }
            };
            match primp::Certificate::from_pem_bundle(&bytes) {
                Ok(bundle) if !bundle.is_empty() => {
                    for certificate in bundle { builder = builder.add_root_certificate(certificate); }
                }
                pem => match primp::Certificate::from_der(&bytes) {
                    Ok(certificate) => builder = builder.add_root_certificate(certificate),
                    Err(error) => {
                        let pem_error = match pem {
                            Err(error) => error.to_string(),
                            Ok(_) => "PEM bundle contains no certificates".to_owned(),
                        };
                        tracing::warn!(%error, %pem_error, path = %path.display(), "failed to parse CA certificate file");
                    }
                },
            }
        }
        let mut client = builder.build().map_err(|error| format!("Failed to build primp client: {error}"))?;
        client.headers_mut().clear();
        Ok(client)
    }

    #[cfg(test)]
    pub async fn send(&self, method: http::Method, url: &Url, headers: HeaderMap, body: &[u8]) -> Result<Response, ObscuraNetError> {
        self.send_with_timeout(method, url, headers, body, Duration::from_secs(30)).await
    }

    #[cfg(test)]
    pub async fn send_with_timeout(&self, method: http::Method, url: &Url, headers: HeaderMap, body: &[u8], timeout: Duration) -> Result<Response, ObscuraNetError> {
        let (client, request) = self.request(method, url, headers, body, timeout)?;
        self.send_prepared(client, request).await
    }

    pub(super) async fn send_prepared(&self, client: &primp::Client, request: primp::Request) -> Result<Response, ObscuraNetError> {
        let request_headers = crate::HeaderCapture::from_headers("transportRequest", request.headers());
        let url = request.url().clone();
        client.execute(request).await.map(|response| Response { response, request_headers }).map_err(|error| network_error(&url, error))
    }

    pub(super) fn request(&self, method: http::Method, url: &Url, headers: HeaderMap, body: &[u8], timeout: Duration) -> Result<(&primp::Client, primp::Request), ObscuraNetError> {
        self.request_inner(method, url, headers, body, None, None, None, timeout)
    }

    pub(super) fn resource_request(&self, method: http::Method, url: &Url, headers: HeaderMap, body: &[u8], resource_type: crate::ResourceType, script_priority: Option<crate::ScriptPriority>, timeout: Duration) -> Result<(&primp::Client, primp::Request), ObscuraNetError> {
        self.request_inner(method, url, headers, body, None, Some(resource_type), script_priority, timeout)
    }

    #[cfg(test)]
    pub(super) fn browser_request(&self, method: http::Method, url: &Url, headers: HeaderMap, body: Option<&[u8]>, resource_type: crate::ResourceType, timeout: Duration) -> Result<(&primp::Client, primp::Request), ObscuraNetError> {
        self.browser_prioritized_request(method, url, headers, body, resource_type, None, timeout)
    }

    pub(super) fn browser_prioritized_request(&self, method: http::Method, url: &Url, headers: HeaderMap, body: Option<&[u8]>, resource_type: crate::ResourceType, script_priority: Option<crate::ScriptPriority>, timeout: Duration) -> Result<(&primp::Client, primp::Request), ObscuraNetError> {
        self.request_inner(method, url, headers, body.unwrap_or_default(), Some(body.is_some()), Some(resource_type), script_priority, timeout)
    }

    fn request_inner(&self, method: http::Method, url: &Url, headers: HeaderMap, body: &[u8], body_present: Option<bool>, resource_type: Option<crate::ResourceType>, script_priority: Option<crate::ScriptPriority>, timeout: Duration) -> Result<(&primp::Client, primp::Request), ObscuraNetError> {
        let script_priority = script_priority.filter(|_| resource_type == Some(crate::ResourceType::Script));
        let client = self.client.get_or_init(|| self.build()).as_ref()
            .map_err(|error| network_error(url, error))?;
        let defaults = &self.defaults;
        // primp orders H2 fields itself; construct the same order for H1.
        let mut merged = headers;
        if resource_type == Some(crate::ResourceType::Image) && !merged.contains_key("priority") {
            merged.insert("priority", HeaderValue::from_static("i"));
        }
        let preflight = method == http::Method::OPTIONS
            && merged.contains_key("access-control-request-method");
        for (name, value) in defaults {
            if script_priority.is_some() && name == "priority" { continue; }
            // CORS preflights do not carry browser client hints. Keep ordinary
            // OPTIONS requests unchanged and do not let primp re-add defaults.
            if preflight && name.as_str().starts_with("sec-ch-") { continue; }
            if !merged.contains_key(name) { merged.insert(name.clone(), value.clone()); }
        }
        // H2 does not synthesize this field from DATA framing. The body is a
        // complete byte slice, so publish its byte length before capture/send.
        // Preserve framing explicitly supplied by native callers/interceptors.
        let browser_empty_length = body_present.is_some()
            && (body_present == Some(true) || method == http::Method::POST || method == http::Method::PUT);
        if (!body.is_empty() || browser_empty_length) && !merged.contains_key(http::header::CONTENT_LENGTH)
            && !merged.contains_key(http::header::TRANSFER_ENCODING) {
            merged.insert(http::header::CONTENT_LENGTH, HeaderValue::from(body.len()));
        }
        let mut headers = HeaderMap::new();
        for name in ["sec-ch-ua", "sec-ch-ua-mobile", "sec-ch-ua-platform",
            "upgrade-insecure-requests", "user-agent", "accept", "sec-fetch-site",
            "sec-fetch-mode", "sec-fetch-user", "sec-fetch-dest", "accept-encoding",
            "accept-language", "dnt", "priority"] {
            for value in merged.get_all(name) { headers.append(name, value.clone()); }
            merged.remove(name);
        }
        headers.extend(merged);
        let mut request = client.request(method, url.as_str()).headers(headers).timeout(timeout);
        if !body.is_empty() || body_present == Some(true) { request = request.body(body.to_vec()); }
        if let Some(weight) = resource_type.and_then(browser_headers_weight) {
            request = request.http2_headers_weight(weight);
        }
        if let Some(priority) = script_priority {
            request = request.http2_headers_weight(priority.headers_weight())
                .http2_extensible_priority(priority.urgency(), false);
        }
        if self.profile == StealthProfile::MacChrome153 {
            // Chromium's dependency band is a separate H2-only surface. Use
            // known loader metadata; otherwise keep the profile's highest
            // default rather than infer a band from a caller's frame weight.
            let band = match (resource_type, script_priority) {
                (Some(crate::ResourceType::Fetch | crate::ResourceType::Xhr), _) |
                (Some(crate::ResourceType::Script), Some(crate::ScriptPriority::High)) => 1,
                (Some(crate::ResourceType::Script), Some(crate::ScriptPriority::Low)) |
                (Some(crate::ResourceType::Image), _) => 3,
                _ => 0,
            };
            request = request.http2_headers_priority_band(band);
        }
        let request = request.build().map_err(|error| network_error(url, error))?;
        Ok((client, request))
    }
}

fn browser_headers_weight(resource_type: crate::ResourceType) -> Option<u16> {
    match resource_type {
        crate::ResourceType::Fetch | crate::ResourceType::Xhr => Some(220),
        crate::ResourceType::Image => Some(147),
        // Script weights need loader scheduling metadata, not just a type.
        _ => None,
    }
}

fn network_error(url: &Url, error: impl std::fmt::Display) -> ObscuraNetError {
    ObscuraNetError::Network(format!("{}: {}", url, error))
}

pub(super) fn header(headers: &mut HeaderMap, name: &str, value: &str) -> Result<(), ObscuraNetError> {
    let name = HeaderName::from_bytes(name.as_bytes()).map_err(|e| ObscuraNetError::Network(e.to_string()))?;
    let value = HeaderValue::from_str(value).map_err(|e| ObscuraNetError::Network(e.to_string()))?;
    headers.append(name, value);
    Ok(())
}

pub(super) struct Response {
    response: primp::Response,
    pub request_headers: crate::HeaderCapture,
}

impl Response {
    pub fn status(&self) -> http::StatusCode { self.response.status() }
    pub fn headers(&self) -> &HeaderMap {
        self.response.encoded_headers().unwrap_or_else(|| self.response.headers())
    }
    pub fn content_length(&self) -> Option<u64> { self.response.content_length() }
    pub fn bytes_stream(self) -> BoxStream<'static, Result<bytes::Bytes, ObscuraNetError>> {
        self.response.bytes_stream().map(|v| v.map_err(|e| ObscuraNetError::Network(e.to_string()))).boxed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    #[test]
    fn classic_script_priority_respects_loader_hints_and_protocol_bounds() {
        use crate::ScriptPriority::{High, Low};
        for hint in ["auto", "high", "low", "invalid", " high "] {
            assert_eq!(crate::ScriptPriority::classic(true, hint, false), High);
            assert_eq!(crate::ScriptPriority::classic(false, hint, true), High);
            assert_eq!(crate::ScriptPriority::classic(false, hint, false), if hint == "high" { High } else { Low });
        }
        assert_eq!(crate::ScriptPriority::classic(false, "HIGH", false), High);
        let client = primp::Client::new();
        for urgency in 0..=7 {
            assert!(client.get("http://localhost/").http2_extensible_priority(urgency, false).build().is_ok());
        }
        assert!(client.get("http://localhost/").http2_extensible_priority(8, false).build().is_err());
        for request in [crate::ResourceRequest::navigation(), crate::ResourceRequest::subresource(crate::ResourceType::Script,
            &Url::parse("https://example.com/").unwrap())] { assert_eq!(request.script_priority, None); }
    }

    #[tokio::test]
    async fn classic_script_priority_never_generates_http1_field_or_removes_explicit_fields() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = Url::parse(&format!("http://{}/script.js", listener.local_addr().unwrap())).unwrap();
        let server = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for _ in 0..8 {
                let (mut stream, _) = listener.accept().unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                let mut bytes = Vec::new();
                while !bytes.ends_with(b"\r\n\r\n") { let mut byte = [0]; stream.read_exact(&mut byte).unwrap(); bytes.push(byte[0]); }
                requests.push(String::from_utf8(bytes).unwrap().to_ascii_lowercase());
                stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
            }
            requests
        });
        let persona = crate::EffectivePersona::builtin(StealthProfile::WindowsChrome145);
        let browser = super::super::StealthHttpClient::with_proxy(Arc::new(crate::CookieJar::new()), None, true, &persona);
        for priority in [crate::ScriptPriority::High, crate::ScriptPriority::Low] {
            for value in [None, Some("u=7"), Some("u=0, i"), Some("")] {
                let fields = value.map(|value| vec![("priority".into(), value.into())]).unwrap_or_default();
                let response = browser.send_browser_prioritized_traced_fields("GET", &url, &fields, None,
                    crate::ResourceType::Script, Some(priority), false, false, 1024, Duration::from_secs(5), None, None).await.unwrap();
                assert_eq!(response.status, 200);
            }
        }
        for (index, request) in server.join().unwrap().iter().enumerate() {
            let fields: Vec<_> = request.lines().filter_map(|line| line.strip_prefix("priority:")).map(str::trim).collect();
            let expected: &[&str] = match index % 4 { 0 => &[], 1 => &["u=7"], 2 => &["u=0, i"], _ => &[""] };
            assert_eq!(fields, expected);
        }
    }

    #[test]
    fn browser_h2_weight_uses_typed_default_resource_priority() {
        for resource_type in [crate::ResourceType::Fetch, crate::ResourceType::Xhr] {
            assert_eq!(browser_headers_weight(resource_type), Some(220));
        }
        assert_eq!(browser_headers_weight(crate::ResourceType::Image), Some(147));
        for resource_type in [crate::ResourceType::Document, crate::ResourceType::Script,
            crate::ResourceType::Stylesheet, crate::ResourceType::Font,
            crate::ResourceType::Other] {
            assert_eq!(browser_headers_weight(resource_type), None);
        }
    }

    #[test]
    fn image_priority_field_uses_incremental_default_but_preserves_explicit_value() {
        let initiator = Url::parse("https://example.com/page").unwrap();
        let image = crate::ResourceRequest::subresource(crate::ResourceType::Image, &initiator);
        assert_eq!(image.priority(), "i");
        let client = Client::new(StealthProfile::MacChrome153, None, true, None, None);
        let url = Url::parse("https://example.com/image.png").unwrap();
        let (_, default) = client.browser_prioritized_request(http::Method::GET, &url,
            HeaderMap::new(), None, crate::ResourceType::Image, None, Duration::from_secs(5)).unwrap();
        assert_eq!(default.headers().get("priority").unwrap(), "i");
        let mut headers = HeaderMap::new();
        headers.insert("priority", HeaderValue::from_static("u=0"));
        let (_, explicit) = client.browser_prioritized_request(http::Method::GET, &url,
            headers, None, crate::ResourceType::Image, None, Duration::from_secs(5)).unwrap();
        assert_eq!(explicit.headers().get("priority").unwrap(), "u=0");
        let (_, native) = client.request(http::Method::GET, &url, HeaderMap::new(), &[], Duration::from_secs(5)).unwrap();
        assert_ne!(native.headers().get("priority").map(|value| value.as_bytes()), Some(b"i".as_slice()));
    }

    #[test]
    fn per_request_h2_weight_validates_protocol_bounds() {
        let client = primp::Client::builder().no_proxy().build().unwrap();
        for weight in [0, 257, u16::MAX] {
            assert!(client.get("http://localhost/").http2_headers_weight(weight).build().is_err());
        }
        for weight in [1, 147, 220, 256] {
            let request = client.get("http://localhost/").http2_headers_weight(weight).build().unwrap();
            assert!(!request.headers().contains_key("priority"));
            assert!(request.try_clone().is_some());
        }
    }

    #[tokio::test]
    async fn chrome153_h2_dependencies_follow_active_loader_bands_on_wire() {
        fn write_frame(stream: &mut std::net::TcpStream, kind: u8, flags: u8, sid: u32, payload: &[u8]) {
            let length = (payload.len() as u32).to_be_bytes();
            stream.write_all(&length[1..]).unwrap();
            stream.write_all(&[kind, flags]).unwrap();
            stream.write_all(&sid.to_be_bytes()).unwrap();
            stream.write_all(payload).unwrap();
        }
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut preface = [0; 24];
            stream.read_exact(&mut preface).unwrap();
            assert_eq!(&preface, b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n");
            write_frame(&mut stream, 4, 0, 0, &[]);
            let mut rows = Vec::new();
            while rows.len() < 5 {
                let mut head = [0; 9];
                stream.read_exact(&mut head).unwrap();
                let length = u32::from_be_bytes([0, head[0], head[1], head[2]]) as usize;
                let kind = head[3]; let flags = head[4];
                let sid = u32::from_be_bytes(head[5..9].try_into().unwrap()) & 0x7fffffff;
                let mut payload = vec![0; length];
                stream.read_exact(&mut payload).unwrap();
                match kind {
                    4 if flags & 1 == 0 => write_frame(&mut stream, 4, 1, 0, &[]),
                    1 => {
                        assert_ne!(flags & 4, 0);
                        assert_ne!(flags & 32, 0);
                        let parent = u32::from_be_bytes(payload[..4].try_into().unwrap()) & 0x7fffffff;
                        rows.push((sid, parent, u16::from(payload[4]) + 1, payload[0] & 0x80 != 0));
                    }
                    _ => {}
                }
            }
            for (sid, _, _, _) in &rows { write_frame(&mut stream, 1, 5, *sid, &[0x88]); }
            rows
        });
        let client = Client::with_tls_options(StealthProfile::MacChrome153, None, true,
            Some("en-US"), None, crate::BrowserFlavor::Chromium, None);
        let transport = primp::Client::builder().no_proxy().http2_prior_knowledge()
            .http2_headers_priority(Some((255, 0, true))).build().unwrap();
        client.client.set(Ok(transport)).unwrap();
        let url = Url::parse(&format!("http://{address}/")).unwrap();
        let make = |resource_type, script_priority| client.browser_prioritized_request(
            http::Method::GET, &url, HeaderMap::new(), None, resource_type,
            script_priority, Duration::from_secs(5),
        ).unwrap();
        let (a_client, a) = make(crate::ResourceType::Document, None);
        let (b_client, b) = make(crate::ResourceType::Script, Some(crate::ScriptPriority::High));
        let (c_client, c) = make(crate::ResourceType::Script, Some(crate::ScriptPriority::Low));
        let (d_client, d) = make(crate::ResourceType::Fetch, None);
        let (e_client, e) = make(crate::ResourceType::Image, None);
        let (a, b, c, d, e) = tokio::join!(
            client.send_prepared(a_client, a), client.send_prepared(b_client, b),
            client.send_prepared(c_client, c), client.send_prepared(d_client, d),
            client.send_prepared(e_client, e),
        );
        for response in [a, b, c, d, e] { assert_eq!(response.unwrap().status(), http::StatusCode::OK); }
        let rows = server.join().unwrap();
        let mut weights: Vec<_> = rows.iter().map(|row| row.2).collect();
        weights.sort_unstable();
        assert_eq!(weights, [147, 147, 220, 220, 256]);
        let mut bands: [Vec<u32>; 4] = std::array::from_fn(|_| Vec::new());
        for (index, (id, parent, weight, exclusive)) in rows.iter().copied().enumerate() {
            assert_eq!(id, (index * 2 + 1) as u32);
            assert!(exclusive);
            let band = match weight { 256 => 0, 220 => 1, 147 => 3, _ => unreachable!() };
            let expected = (0..=band).rev().find_map(|band| bands[band].last().copied()).unwrap_or(0);
            assert_eq!(parent, expected, "stream {id} weight {weight}");
            bands[band].push(id);
        }
    }

    #[tokio::test]
    async fn per_request_h2_weight_does_not_change_http1_fields() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut received = Vec::new();
            while !received.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                let mut bytes = [0; 1024];
                let count = stream.read(&mut bytes).unwrap();
                assert!(count > 0);
                received.extend_from_slice(&bytes[..count]);
            }
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
            String::from_utf8(received).unwrap()
        });
        let client = primp::Client::builder().no_proxy().build().unwrap();
        assert_eq!(client.get(format!("http://{address}/")).header("priority", "u=7")
            .http2_headers_weight(220).send().await.unwrap().status(), http::StatusCode::OK);
        let received = server.join().unwrap();
        assert!(received.starts_with("GET / HTTP/1.1\r\n"));
        assert_eq!(received.to_ascii_lowercase().matches("priority: u=7\r\n").count(), 1);
        assert!(!received.to_ascii_lowercase().contains("weight"));
    }

    #[tokio::test]
    async fn per_request_h2_weight_reaches_one_connection_without_leaking() {
        fn write_frame(stream: &mut std::net::TcpStream, kind: u8, flags: u8, sid: u32, payload: &[u8]) {
            let length = (payload.len() as u32).to_be_bytes();
            stream.write_all(&length[1..]).unwrap();
            stream.write_all(&[kind, flags]).unwrap();
            stream.write_all(&sid.to_be_bytes()).unwrap();
            stream.write_all(payload).unwrap();
        }
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut preface = [0; 24];
            stream.read_exact(&mut preface).unwrap();
            assert_eq!(&preface, b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n");
            write_frame(&mut stream, 4, 0, 0, &[]);
            let mut rows: Vec<(u16, bool, u32, bool)> = Vec::new();
            let mut data_bytes = 0;
            while rows.len() < 26 || !rows.last().unwrap().3 {
                let mut head = [0; 9];
                stream.read_exact(&mut head).unwrap();
                let length = u32::from_be_bytes([0, head[0], head[1], head[2]]) as usize;
                let kind = head[3]; let flags = head[4];
                let sid = u32::from_be_bytes(head[5..9].try_into().unwrap()) & 0x7fffffff;
                let mut payload = vec![0; length];
                stream.read_exact(&mut payload).unwrap();
                match kind {
                    4 if flags & 1 == 0 => write_frame(&mut stream, 4, 1, 0, &[]),
                    1 => {
                        assert_ne!(flags & 4, 0, "small fixture must finish HEADERS");
                        assert_ne!(flags & 32, 0, "HEADERS priority flag");
                        assert_eq!(u32::from_be_bytes(payload[..4].try_into().unwrap()) & 0x7fffffff, 0);
                        rows.push((u16::from(payload[4]) + 1, payload[0] & 0x80 != 0, sid, flags & 1 != 0));
                    }
                    0 => { data_bytes += payload.len(); if flags & 1 != 0 { rows.last_mut().unwrap().3 = true; } }
                    _ => {}
                }
                if matches!(kind, 0 | 1) && flags & 1 != 0 {
                    if rows.len() == 6 {
                        // A same-origin 302 proves extension replay onto a rebuilt hop.
                        let mut block = vec![0x08, 0x03, b'3', b'0', b'2', 0x0f, 0x1f, 0x05];
                        block.extend_from_slice(b"/next");
                        write_frame(&mut stream, 1, 5, sid, &block);
                    } else { write_frame(&mut stream, 1, 5, sid, &[0x88]); }
                }
            }
            (rows, data_bytes)
        });
        let client = primp::Client::builder().no_proxy().http2_prior_knowledge()
            .http2_headers_priority(Some((255, 0, true))).build().unwrap();
        let url = format!("http://{address}/");
        for weight in [Some(220), None, Some(1), Some(256), Some(147)] {
            let mut request = client.get(&url);
            if let Some(weight) = weight { request = request.http2_headers_weight(weight); }
            let request = request.build().unwrap();
            let request = request.try_clone().unwrap();
            assert_eq!(client.execute(request).await.unwrap().status(), http::StatusCode::OK);
        }
        assert_eq!(client.get(&url).http2_headers_weight(220).send().await.unwrap().status(), http::StatusCode::OK);
        assert_eq!(client.post(&url).body(vec![b'x'; 355]).send().await.unwrap().status(), http::StatusCode::OK);
        let persona = crate::EffectivePersona::builtin(StealthProfile::WindowsChrome145);
        let browser = super::super::StealthHttpClient::with_proxy(Arc::new(crate::CookieJar::new()), None, true, &persona);
        browser.client.client.set(Ok(client.clone())).unwrap();
        let parsed = Url::parse(&url).unwrap();
        for resource_type in [crate::ResourceType::Fetch, crate::ResourceType::Xhr,
            crate::ResourceType::Script, crate::ResourceType::Document, crate::ResourceType::Other] {
            let response = browser.send_browser_traced_fields("GET", &parsed,
                &[("priority".into(), "u=7".into())], None, resource_type,
                false, false, 1024, Duration::from_secs(5), None, None).await.unwrap();
            assert_eq!(response.status, 200);
            assert_eq!(response.request_raw_headers.unwrap().text_headers().get("priority").map(String::as_str), Some("u=7"));
        }
        for resource_type in [crate::ResourceType::Fetch, crate::ResourceType::Xhr,
            crate::ResourceType::Script, crate::ResourceType::Document] {
            let (transport, request) = browser.client.resource_request(http::Method::GET,
                &parsed, HeaderMap::new(), &[], resource_type, None, Duration::from_secs(5)).unwrap();
            assert_eq!(browser.client.send_prepared(transport, request).await.unwrap().status(), http::StatusCode::OK);
        }
        for priority in [crate::ScriptPriority::High, crate::ScriptPriority::Low] {
            for value in [None, Some("u=7"), Some("u=0, i"), Some("")] {
                let fields = value.map(|value| vec![("priority".into(), value.into())]).unwrap_or_default();
                let response = browser.send_browser_prioritized_traced_fields("GET", &parsed, &fields, None,
                    crate::ResourceType::Script, Some(priority), false, false, 1024, Duration::from_secs(5), None, None).await.unwrap();
                assert_eq!(response.status, 200);
                // Prepared capture is before H2 field generation. Wire fields are
                // covered separately with the validated HPACK receiver.
                assert_eq!(response.request_raw_headers.unwrap().text_headers().get("priority").map(String::as_str), value);
            }
        }
        assert_eq!(client.get(&url).send().await.unwrap().status(), http::StatusCode::OK);
        let (rows, data_bytes) = server.join().unwrap();
        assert_eq!(rows.iter().map(|row| row.0).collect::<Vec<_>>(),
            [220, 256, 1, 256, 147, 220, 220, 256, 220, 220, 256, 256, 256, 220, 220, 256, 256,
                220, 220, 220, 220, 147, 147, 147, 147, 256]);
        assert!(rows.iter().all(|row| row.1 && row.3));
        assert_eq!(rows.iter().map(|row| row.2).collect::<Vec<_>>(), (1..52).step_by(2).collect::<Vec<_>>());
        assert_eq!(data_bytes, 355);
    }

    #[tokio::test]
    async fn raw_headers_preserve_repeated_and_non_utf8_transport_fields() {
        for proxied in [false, true] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let server = std::thread::spawn(move || {
                let (mut socket, _) = listener.accept().unwrap();
                socket.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                let mut bytes = Vec::new();
                while !bytes.windows(4).any(|v| v == b"\r\n\r\n") {
                    let mut buffer = [0; 4096];
                    let count = socket.read(&mut buffer).unwrap();
                    assert!(count > 0);
                    bytes.extend_from_slice(&buffer[..count]);
                }
                socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\nSet-Cookie: first=Raw+/=; Path=/\r\nSet-Cookie: second=Keep; Path=/\r\nX-Repeated: first\r\nX-Repeated: second\r\nX-Bytes: \x80\xff\r\n\r\n").unwrap();
                bytes
            });
            let proxy = proxied.then(|| format!("http://{address}"));
            let client = Client::new(StealthProfile::WindowsChrome145, proxy.as_deref(), true, None, None);
            let url = Url::parse(&if proxied { "http://headers.test/".into() } else { format!("http://{address}/") }).unwrap();
            let mut headers = HeaderMap::new();
            for value in [b"first".as_slice(), b"\x80\xff".as_slice()] {
                headers.append("x-repeated", HeaderValue::from_bytes(value).unwrap());
            }
            headers.append("authorization", HeaderValue::from_static("Bearer Raw+/=123"));
            headers.append("cookie", HeaderValue::from_static("session=Raw+/=123"));
            let response = client.send(http::Method::GET, &url, headers, &[]).await.unwrap();
            let request = &response.request_headers;
            assert_eq!(request.capture_stage, "transportRequest");
            let values = |capture: &crate::HeaderCapture, name: &[u8]| capture.fields.iter()
                .filter(|field| field.name == name).map(|field| field.value.clone()).collect::<Vec<_>>();
            assert_eq!(values(request, b"x-repeated"), [b"first".to_vec(), b"\x80\xff".to_vec()]);
            assert!(!request.text_headers().contains_key("x-repeated"), "a partial text projection must not hide an invalid duplicate");
            assert_eq!(values(request, b"authorization"), [b"Bearer Raw+/=123".to_vec()]);
            assert_eq!(values(request, b"cookie"), [b"session=Raw+/=123".to_vec()]);
            assert_eq!(values(request, b"user-agent"), [StealthProfile::WindowsChrome145.user_agent().as_bytes().to_vec()]);
            let raw = crate::HeaderCapture::from_headers("transportResponse", response.headers());
            assert_eq!(values(&raw, b"set-cookie"), [b"first=Raw+/=; Path=/".to_vec(), b"second=Keep; Path=/".to_vec()]);
            assert_eq!(values(&raw, b"x-repeated"), [b"first".to_vec(), b"second".to_vec()]);
            assert_eq!(values(&raw, b"x-bytes"), [b"\x80\xff".to_vec()]);
            assert!(!raw.text_headers().contains_key("x-bytes"));
            let wire = server.join().unwrap();
            for value in [b"x-repeated: first\r\n".as_slice(), b"x-repeated: \x80\xff\r\n".as_slice(), b"authorization: Bearer Raw+/=123\r\n".as_slice(), b"cookie: session=Raw+/=123\r\n".as_slice()] {
                assert!(wire.windows(value.len()).any(|part| part == value), "missing field: {value:?}");
            }
        }
    }

    #[test]
    fn browser_body_presence_controls_empty_length_without_changing_native() {
        let client = Client::new(StealthProfile::WindowsChrome145, None, true, None, None);
        let url = Url::parse("https://127.0.0.1/").unwrap();
        for method in [http::Method::POST, http::Method::PUT, http::Method::PATCH, http::Method::DELETE] {
            for body in [None, Some(&[][..]), Some("é".as_bytes())] {
                let (_, request) = client.browser_request(method.clone(), &url, HeaderMap::new(), body, crate::ResourceType::Fetch, Duration::from_secs(5)).unwrap();
                let expected = match body {
                    Some(bytes) => Some(bytes.len().to_string()),
                    None if method == http::Method::POST || method == http::Method::PUT => Some("0".into()),
                    None => None,
                };
                assert_eq!(request.headers().get(http::header::CONTENT_LENGTH).map(|value| value.to_str().unwrap().to_owned()), expected);
            }
            let (_, native) = client.request(method, &url, HeaderMap::new(), &[], Duration::from_secs(5)).unwrap();
            assert!(!native.headers().contains_key(http::header::CONTENT_LENGTH));
        }
        for method in [http::Method::GET, http::Method::HEAD, http::Method::OPTIONS] {
            let (_, request) = client.browser_request(method, &url, HeaderMap::new(), None, crate::ResourceType::Fetch, Duration::from_secs(5)).unwrap();
            assert!(!request.headers().contains_key(http::header::CONTENT_LENGTH));
        }
        for (name, value) in [(http::header::CONTENT_LENGTH, "17"), (http::header::TRANSFER_ENCODING, "chunked")] {
            let mut headers = HeaderMap::new();
            headers.insert(name.clone(), HeaderValue::from_static(value));
            let (_, request) = client.browser_request(http::Method::POST, &url, headers, Some(&[]), crate::ResourceType::Fetch, Duration::from_secs(5)).unwrap();
            assert_eq!(request.headers().get(&name).unwrap(), value);
            if name == http::header::TRANSFER_ENCODING { assert!(!request.headers().contains_key(http::header::CONTENT_LENGTH)); }
        }
    }

    #[test]
    fn known_body_length_is_in_prepared_request_for_every_protocol() {
        let client = Client::new(StealthProfile::WindowsChrome145, None, true, None, None);
        let url = Url::parse("http://127.0.0.1/").unwrap();
        let body = "ordinary UTF-8 body: é 中文".as_bytes();
        for method in [http::Method::POST, http::Method::PUT, http::Method::PATCH, http::Method::DELETE] {
            let (_, request) = client.request(method, &url, HeaderMap::new(), body, Duration::from_secs(5)).unwrap();
            assert_eq!(request.headers().get(http::header::CONTENT_LENGTH).unwrap().to_str().unwrap(), body.len().to_string());
            assert_eq!(request.body().unwrap().as_bytes().unwrap(), body);
        }
        for method in [http::Method::GET, http::Method::HEAD, http::Method::OPTIONS] {
            let (_, request) = client.request(method, &url, HeaderMap::new(), &[], Duration::from_secs(5)).unwrap();
            assert!(!request.headers().contains_key(http::header::CONTENT_LENGTH));
        }
    }

    #[test]
    fn known_body_length_preserves_explicit_framing() {
        let client = Client::new(StealthProfile::WindowsChrome145, None, true, None, None);
        let url = Url::parse("http://127.0.0.1/").unwrap();
        for (name, value) in [(http::header::CONTENT_LENGTH, "7"), (http::header::TRANSFER_ENCODING, "chunked")] {
            let mut headers = HeaderMap::new();
            headers.insert(name.clone(), HeaderValue::from_static(value));
            let (_, request) = client.request(http::Method::POST, &url, headers, b"fixture", Duration::from_secs(5)).unwrap();
            assert_eq!(request.headers().get(&name).unwrap(), value);
            if name == http::header::TRANSFER_ENCODING {
                assert!(!request.headers().contains_key(http::header::CONTENT_LENGTH));
            } else {
                assert!(!request.headers().contains_key(http::header::TRANSFER_ENCODING));
            }
        }
    }

    #[tokio::test]
    async fn known_body_length_and_bytes_reach_http1_server() {
        let bodies = [vec![b'x'; 355], "é 中文".as_bytes().to_vec(), vec![0, 128, 255, 13, 10]];
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let expected = bodies.clone();
        let server = std::thread::spawn(move || {
            for body in expected {
                let (mut socket, _) = listener.accept().unwrap();
                socket.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                let mut wire = Vec::new();
                let header_end = loop {
                    let mut buffer = [0; 4096];
                    let count = socket.read(&mut buffer).unwrap();
                    assert!(count > 0);
                    wire.extend_from_slice(&buffer[..count]);
                    if let Some(end) = wire.windows(4).position(|part| part == b"\r\n\r\n") { break end + 4; }
                };
                while wire.len() < header_end + body.len() {
                    let mut buffer = [0; 4096];
                    let count = socket.read(&mut buffer).unwrap();
                    assert!(count > 0);
                    wire.extend_from_slice(&buffer[..count]);
                }
                let headers = std::str::from_utf8(&wire[..header_end]).unwrap().to_ascii_lowercase();
                assert!(headers.contains(&format!("\r\ncontent-length: {}\r\n", body.len())));
                assert!(!headers.contains("transfer-encoding:"));
                assert_eq!(&wire[header_end..], body);
                socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
            }
        });
        let client = Client::new(StealthProfile::WindowsChrome145, None, true, None, None);
        let url = Url::parse(&format!("http://{address}/")).unwrap();
        for body in bodies {
            let response = client.send(http::Method::POST, &url, HeaderMap::new(), &body).await.unwrap();
            assert_eq!(response.status(), http::StatusCode::OK);
        }
        server.join().unwrap();
    }

    #[test]
    fn scripted_request_timeout_overrides_default_for_preflight_and_redirect_hops() {
        let client = Client::new(StealthProfile::WindowsChrome145, None, true, None, None);
        let url = Url::parse("http://127.0.0.1/").unwrap();
        for timeout in [Duration::from_millis(25), Duration::from_secs(60)] {
            for method in [http::Method::OPTIONS, http::Method::GET, http::Method::POST] {
                let mut headers = HeaderMap::new();
                if method == http::Method::OPTIONS {
                    headers.insert("access-control-request-method", HeaderValue::from_static("POST"));
                }
                let (_, request) = client.request(method, &url, headers, &[], timeout).unwrap();
                assert_eq!(request.timeout(), Some(&timeout));
            }
        }
    }

    #[tokio::test]
    async fn configured_pem_and_der_roots_authorize_primp_https() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let ca_key = rcgen::KeyPair::generate().unwrap();
        let mut params = rcgen::CertificateParams::new(Vec::new()).unwrap();
        params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        let ca = params.self_signed(&ca_key).unwrap();
        let leaf_key = rcgen::KeyPair::generate().unwrap();
        let leaf = rcgen::CertificateParams::new(vec!["127.0.0.1".into()]).unwrap()
            .signed_by(&leaf_key, &ca, &ca_key).unwrap();
        let config = tokio_rustls::rustls::ServerConfig::builder().with_no_client_auth()
            .with_single_cert(vec![leaf.der().clone()],
                tokio_rustls::rustls::pki_types::PrivatePkcs8KeyDer::from(leaf_key.serialize_der()).into()).unwrap();
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = Url::parse(&format!("https://{}/", listener.local_addr().unwrap())).unwrap();
        let server = tokio::spawn(async move {
            let mut captured = Vec::new();
            for _ in 0..3 {
                let (stream, _) = listener.accept().await.unwrap();
                let mut tls = acceptor.accept(stream).await.unwrap();
                let mut request = Vec::new();
                while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                    let mut buffer = [0; 4096];
                    let n = tls.read(&mut buffer).await.unwrap();
                    assert!(n > 0); request.extend_from_slice(&buffer[..n]);
                }
                tls.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\n\x00\x80\xff\x10").await.unwrap();
                tls.shutdown().await.unwrap();
                captured.push(request);
            }
            captured
        });
        let directory = tempfile::tempdir().unwrap();
        let pem = directory.path().join("bundle.pem");
        std::fs::write(&pem, format!("{}{}", ca.pem(), ca.pem())).unwrap();
        let der_dir = directory.path().join("der");
        std::fs::create_dir(&der_dir).unwrap();
        let der = der_dir.join("root.der");
        std::fs::write(&der, ca.der()).unwrap();
        for (name, path) in [("SSL_CERT_FILE", pem), ("SSL_CERT_FILE", der), ("SSL_CERT_DIR", der_dir)] {
            std::env::remove_var("SSL_CERT_FILE");
            std::env::remove_var("SSL_CERT_DIR");
            std::env::set_var(name, path);
            let client = Client::new(StealthProfile::WindowsChrome145, None, true, None, None);
            let response = client.send(http::Method::GET, &url, HeaderMap::new(), &[]).await.unwrap();
            let body = super::super::read_stealth_body_limited(response, &url, 1024).await.unwrap();
            assert_eq!(body, [0, 128, 255, 16]);
        }
        std::env::remove_var("SSL_CERT_FILE");
        std::env::remove_var("SSL_CERT_DIR");
        for request in server.await.unwrap() {
            assert!(std::str::from_utf8(&request).unwrap().to_ascii_lowercase().contains(
                &format!("user-agent: {}\r\n", StealthProfile::WindowsChrome145.user_agent().to_ascii_lowercase())));
        }
    }

    #[tokio::test]
    async fn cors_preflight_omits_client_hints_on_wire() {
        for (method, preflight) in [(http::Method::OPTIONS, true), (http::Method::OPTIONS, false), (http::Method::GET, false)] {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut bytes = Vec::new();
            while !bytes.windows(4).any(|v| v == b"\r\n\r\n") {
                let mut buf = [0; 4096];
                let n = socket.read(&mut buf).unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&buf[..n]);
            }
            socket.write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n").unwrap();
            String::from_utf8(bytes).unwrap().to_ascii_lowercase()
        });
        let client = Client::new(StealthProfile::MacChrome152, None, true, None, Some("1"));
        let mut headers = HeaderMap::new();
        if preflight { headers.insert("access-control-request-method", HeaderValue::from_static("POST")); }
        headers.insert("origin", HeaderValue::from_static("https://example.com"));
        client.send(method, &Url::parse(&format!("http://{address}/api")).unwrap(), headers, &[]).await.unwrap();
        let request = server.join().unwrap();
        assert_eq!(request.contains("sec-ch-ua"), !preflight, "unexpected client hints for preflight={preflight}");
        assert!(request.contains("chrome/152.0.0.0"));
        }
    }
}
