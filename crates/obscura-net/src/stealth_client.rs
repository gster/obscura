#[path = "stealth_transport.rs"]
mod transport;
use transport::header;

#[cfg(feature = "stealth")]
use std::collections::HashMap;
#[cfg(feature = "stealth")]
use std::error::Error;
#[cfg(feature = "stealth")]
use std::sync::Arc;

#[cfg(feature = "stealth")]
use futures_util::StreamExt;
#[cfg(feature = "stealth")]
use tokio::sync::RwLock;
#[cfg(feature = "stealth")]
use url::Url;

#[cfg(feature = "stealth")]
use crate::cookies::CookieJar;
#[cfg(feature = "stealth")]
use crate::client::{
    CallbackRegistry, InFlightGuard, ObscuraNetError, RequestInfo, RequestMode,
    ReferrerPolicy, ResourceRequest, Response, SsrfGuardResolver, cors_required, env_allows_private_network,
    fetch_file_url, is_forbidden_ip, redirect_taints_origin, request_fetch_site,
    request_referrer, response_too_large, serialized_request_origin, validate_cors_response,
    validate_request_mode, validate_url,
};

impl primp::dns::Resolve for SsrfGuardResolver {
    fn resolve(&self, name: primp::dns::Name) -> primp::dns::Resolving {
        let allow = self.allow_private || env_allows_private_network();
        let host = name.as_str().to_string();
        Box::pin(async move {
            let addrs = resolve_guarded(&host, allow).await?;
            Ok(Box::new(addrs.into_iter()) as primp::dns::Addrs)
        })
    }
}

async fn resolve_guarded(host: &str, allow: bool) -> Result<Vec<std::net::SocketAddr>, Box<dyn Error + Send + Sync>> {
    let addrs: Vec<_> = tokio::net::lookup_host((host, 0)).await?.collect();
    if !allow {
        if let Some(bad) = addrs.iter().find(|sa| is_forbidden_ip(sa.ip())) {
            return Err(format!("SSRF blocked: '{}' resolves to forbidden address {}", host, bad.ip()).into());
        }
    }
    Ok(addrs)
}

#[cfg(feature = "stealth")]
pub const STEALTH_USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/145.0.0.0 Safari/537.36";

// The Windows Chrome145 profile sends this exact
// UA and sec-ch-ua-platform "Windows" on the wire. navigator has to report the
// same identity, otherwise the TLS/HTTP layer and the JS layer disagree and a
// site cross-checks the mismatch as a bot signal.
#[cfg(feature = "stealth")]
pub const STEALTH_NAVIGATOR_PLATFORM: &str = "Win32";
#[cfg(feature = "stealth")]
pub const STEALTH_UA_PLATFORM: &str = "Windows";
#[cfg(feature = "stealth")]
pub const STEALTH_UA_PLATFORM_VERSION: &str = "15.0.0";

#[cfg(feature = "stealth")]
fn stealth_response_header_value<'a>(
    headers: &'a http::header::HeaderMap,
    name: &'static str,
    url: &Url,
) -> Result<Option<&'a str>, ObscuraNetError> {
    let mut values = headers.get_all(name).iter();
    let Some(first) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        return Err(ObscuraNetError::Cors(format!(
            "{} returned multiple {} headers",
            url, name
        )));
    }
    first.to_str().map(Some).map_err(|_| {
        ObscuraNetError::Cors(format!("{} returned an invalid {} header", url, name))
    })
}

#[cfg(feature = "stealth")]
fn validate_stealth_cors_response(
    request: &ResourceRequest,
    target: &Url,
    serialized_origin: &str,
    headers: &http::header::HeaderMap,
) -> Result<(), ObscuraNetError> {
    if !cors_required(request, target) {
        return Ok(());
    }
    let allow_origin =
        stealth_response_header_value(headers, "access-control-allow-origin", target)?;
    let allow_credentials =
        stealth_response_header_value(headers, "access-control-allow-credentials", target)?;
    validate_cors_response(
        request,
        target,
        serialized_origin,
        allow_origin,
        allow_credentials,
    )
}

#[cfg(feature = "stealth")]
async fn read_stealth_body_limited(
    response: transport::Response,
    url: &Url,
    limit: usize,
) -> Result<Vec<u8>, ObscuraNetError> {
    if response
        .headers()
        .get("content-length")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .is_some_and(|length| length > limit as u64)
    {
        return Err(response_too_large(url, limit));
    }

    let capacity = response
        .content_length()
        .and_then(|length| usize::try_from(length).ok())
        .unwrap_or(0)
        .min(limit);
    let stream = response.bytes_stream();
    futures_util::pin_mut!(stream);
    let mut body = Vec::with_capacity(capacity);
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| {
            ObscuraNetError::Network(format!("Failed to read body: {}", error))
        })?;
        if chunk.len() > limit.saturating_sub(body.len()) {
            return Err(response_too_large(url, limit));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Browser identity is independent of transport preset availability.
/// MacChrome152 uses primp Chrome152 with explicit macOS identity.
/// ALPS and trust-anchor contents still differ from the reference Chrome.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StealthProfile {
    #[default]
    WindowsChrome145,
    MacChrome152,
}

impl StealthProfile {
    pub fn user_agent(self) -> &'static str {
        match self {
            Self::WindowsChrome145 => STEALTH_USER_AGENT,
            Self::MacChrome152 => "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/152.0.0.0 Safari/537.36",
        }
    }
    pub fn platform(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::WindowsChrome145 => (STEALTH_NAVIGATOR_PLATFORM, STEALTH_UA_PLATFORM, STEALTH_UA_PLATFORM_VERSION),
            Self::MacChrome152 => ("MacIntel", "macOS", "26.6.2"),
        }
    }
}

#[cfg(feature = "stealth")]
pub struct StealthHttpClient {
    client: transport::Client,
    allow_private_network: bool,
    pub cookie_jar: Arc<CookieJar>,
    pub extra_headers: RwLock<HashMap<String, String>>,
    pub in_flight: Arc<std::sync::atomic::AtomicU32>,
    policy: Option<Arc<crate::client::ObscuraHttpClient>>,
}

#[cfg(feature = "stealth")]
impl StealthHttpClient {
    pub fn new(cookie_jar: Arc<CookieJar>) -> Self {
        Self::with_proxy(cookie_jar, None, false)
    }

    pub fn with_proxy(cookie_jar: Arc<CookieJar>, proxy_url: Option<&str>, allow_private_network: bool) -> Self {
        Self::with_options(cookie_jar, proxy_url, None, allow_private_network, StealthProfile::default())
    }

    pub fn with_policy(cookie_jar: Arc<CookieJar>, proxy_url: Option<&str>, policy: Arc<crate::client::ObscuraHttpClient>) -> Self {
        let allow_private_network = policy.allow_private_network;
        Self::with_options(cookie_jar, proxy_url, Some(policy), allow_private_network, StealthProfile::default())
    }

    pub fn with_policy_profile(cookie_jar: Arc<CookieJar>, proxy_url: Option<&str>, policy: Arc<crate::client::ObscuraHttpClient>, profile: StealthProfile) -> Self {
        let allow_private_network = policy.allow_private_network;
        Self::with_options(cookie_jar, proxy_url, Some(policy), allow_private_network, profile)
    }

    pub fn with_policy_profile_persona(
        cookie_jar: Arc<CookieJar>,
        proxy_url: Option<&str>,
        policy: Arc<crate::client::ObscuraHttpClient>,
        profile: StealthProfile,
        accept_language: &str,
        do_not_track: Option<&str>,
    ) -> Self {
        let allow_private_network = policy.allow_private_network;
        let client = transport::Client::new(
            profile, proxy_url, allow_private_network,
            Some(accept_language), do_not_track,
        );
        StealthHttpClient {
            client,
            allow_private_network,
            cookie_jar,
            extra_headers: RwLock::new(HashMap::new()),
            in_flight: Arc::new(std::sync::atomic::AtomicU32::new(0)),
            policy: Some(policy),
        }
    }

    fn with_options(cookie_jar: Arc<CookieJar>, proxy_url: Option<&str>, policy: Option<Arc<crate::client::ObscuraHttpClient>>, allow_private_network: bool, profile: StealthProfile) -> Self {
        let client = transport::Client::new(profile, proxy_url, allow_private_network, None, None);

        StealthHttpClient {
            client,
            allow_private_network,
            cookie_jar,
            extra_headers: RwLock::new(HashMap::new()),
            in_flight: Arc::new(std::sync::atomic::AtomicU32::new(0)),
            policy,
        }
    }

    fn block_trackers(&self) -> bool {
        self.policy.as_ref().map_or(true, |p| p.block_trackers)
    }

    async fn intercept(&self, info: &mut RequestInfo) -> Result<Option<Response>, ObscuraNetError> {
        validate_url(&info.url, self.allow_private_network)?;
        if let Some(policy) = &self.policy {
            if let Some(interceptor) = policy.interceptor.read().await.as_ref() {
                match interceptor.intercept(info).await {
                    crate::interceptor::InterceptAction::Continue => {}
                    crate::interceptor::InterceptAction::Block => return Err(ObscuraNetError::Blocked(info.url.to_string())),
                    crate::interceptor::InterceptAction::Fulfill(response) => return Ok(Some(response)),
                    crate::interceptor::InterceptAction::ModifyHeaders(headers) => info.headers.extend(headers),
                }
            }
        }
        Ok(None)
    }

    pub async fn fetch(&self, url: &Url) -> Result<Response, ObscuraNetError> {
        self.fetch_with_callbacks(url, None).await
    }

    pub async fn fetch_with_callbacks(
        &self,
        url: &Url,
        callbacks: Option<&CallbackRegistry>,
    ) -> Result<Response, ObscuraNetError> {
        self.fetch_with_profile(url, ResourceRequest::navigation(), callbacks)
            .await
    }

    pub async fn fetch_resource_with_callbacks(
        &self,
        url: &Url,
        request: ResourceRequest,
        callbacks: Option<&CallbackRegistry>,
    ) -> Result<Response, ObscuraNetError> {
        self.fetch_with_profile(url, request, callbacks).await
    }

    async fn fetch_with_profile(
        &self,
        url: &Url,
        request: ResourceRequest,
        callbacks: Option<&CallbackRegistry>,
    ) -> Result<Response, ObscuraNetError> {
        self.fetch_method_with_profile(url, request, callbacks, http::Method::GET, &[]).await
    }

    pub async fn post_form_resource_with_callbacks(
        &self, url: &Url, body: &str, request: ResourceRequest,
        callbacks: Option<&CallbackRegistry>,
    ) -> Result<Response, ObscuraNetError> {
        self.fetch_method_with_profile(url, request, callbacks, http::Method::POST, body.as_bytes()).await
    }

    async fn fetch_method_with_profile(
        &self, url: &Url, mut request: ResourceRequest,
        callbacks: Option<&CallbackRegistry>, mut method: http::Method, initial_body: &[u8],
    ) -> Result<Response, ObscuraNetError> {
        let mut request_body = initial_body.to_vec();
        validate_url(url, self.allow_private_network)?;
        validate_request_mode(&request, url)?;
        if url.scheme() == "file" {
            if let Some(mut response) = self.intercept(&mut RequestInfo {body: Vec::new(), url: url.clone(), method: "GET".into(), headers: HashMap::new(), resource_type: request.resource_type}).await? {
                response.request_referrer = None;
                return Ok(response);
            }
            return fetch_file_url(url, request.max_response_bytes).await;
        }

        let mut current_url = url.clone();

        let mut redirects = Vec::new();
        let mut redirect_tainted = false;
        let mut request_callback_fired = false;

        // Follow up to 20 redirects (Fetch spec + the reqwest path): 0..=20 makes
        // 21 requests, so the 20th hop is followed and only the 21st fails.
        for _ in 0..=20 {
            validate_request_mode(&request, &current_url)?;
            if let Some(host) = current_url.host_str() {
                if self.block_trackers() && crate::blocklist::is_blocked(host) {
                    tracing::debug!("Blocked tracker: {}", current_url);
                    return Ok(Response {
                        status: 0,
                        url: current_url,
                        headers: HashMap::new(),
                        body: Vec::new(),
                        redirected_from: Vec::new(),
                        request_referrer: None,
                    });
                }
            }

            let mut request_info = RequestInfo {
                body: request_body.clone(),
                url: current_url.clone(), method: method.to_string(),
                headers: self.extra_headers.read().await.clone(), resource_type: request.resource_type,
            };
            if let Some(mut response) = self.intercept(&mut request_info).await? {
                response.request_referrer = request.referrer_policy.referrer(request.referrer.as_ref(), &current_url);
                return Ok(response);
            }
            let mut headers = http::header::HeaderMap::new();
            header(&mut headers, "accept", request.accept())?;
            header(&mut headers, "sec-fetch-site", request_fetch_site(&request, &current_url))?;
            header(&mut headers, "sec-fetch-mode", request.mode.header_value())?;
            header(&mut headers, "sec-fetch-dest", request.destination())?;
            header(&mut headers, "priority", request.priority())?;
            if request.mode == RequestMode::Navigate {
                header(&mut headers, "upgrade-insecure-requests", "1")?;
                header(&mut headers, "sec-fetch-user", "?1")?;
            }
            let referer = request_referrer(&request, &current_url);
            request.referrer = referer.as_deref().and_then(|value| Url::parse(value).ok());
            if let Some(referer) = referer {
                header(&mut headers, "referer", &referer)?;
            }
            let request_origin = serialized_request_origin(&request, redirect_tainted);

            let cookie_header = if request.sends_credentials_to(&current_url) {
                self.cookie_jar.get_cookie_header(&current_url)
            } else {
                String::new()
            };
            if !cookie_header.is_empty() {
                header(&mut headers, "cookie", &cookie_header)?;
            }

            for (k, v) in request_info.headers.iter() {
                if k.eq_ignore_ascii_case("origin") || k.eq_ignore_ascii_case("referer") {
                    continue;
                }
                // Script-supplied fetch/XHR headers replace browser defaults.
                // Appending produced duplicate Accept and Content-Type fields,
                // which Chrome does not send and strict APIs may reject.
                headers.remove(k.as_str());
                header(&mut headers, k, v)?;
            }
            if cors_required(&request, &current_url) || method == http::Method::POST {
                header(&mut headers, "origin", &request_origin)?;
            }
            if method == http::Method::POST && !headers.contains_key("content-type") {
                header(&mut headers, "content-type", "application/x-www-form-urlencoded")?;
            }

            if !request_callback_fired {
                if let Some(callbacks) = callbacks {
                    callbacks.fire_request(&request_info).await;
                }
                request_callback_fired = true;
            }

            let in_flight = InFlightGuard::new(&self.in_flight);
            let resp = self.client.send(method.clone(), &current_url, headers, &request_body).await?;

            let status = resp.status();
            validate_stealth_cors_response(
                &request,
                &current_url,
                &request_origin,
                resp.headers(),
            )?;

            if request.sends_credentials_to(&current_url) {
                for val in resp.headers().get_all("set-cookie") {
                    if let Ok(s) = val.to_str() {
                        self.cookie_jar.set_cookie(s, &current_url);
                    }
                }
            }

            let mut response_headers: HashMap<String, String> = HashMap::new();
            for (k, v) in resp.headers().iter() {
                crate::client::merge_response_header(
                    &mut response_headers,
                    k.as_str().to_lowercase(),
                    v.to_str().unwrap_or("").to_string(),
                );
            }

            if status.is_redirection() {
                if let Some(location) = resp.headers().get("location") {
                    let location_str = location.to_str().map_err(|_| {
                        ObscuraNetError::Network("Invalid redirect Location".into())
                    })?;
                    let mut next_url = current_url.join(location_str).map_err(|e| {
                        ObscuraNetError::Network(format!("Invalid redirect URL: {}", e))
                    })?;
                    if next_url.fragment().is_none() {
                        next_url.set_fragment(current_url.fragment());
                    }
                    validate_url(&next_url, self.allow_private_network)?;
                    validate_request_mode(&request, &next_url)?;
                    redirect_tainted |=
                        redirect_taints_origin(&request, &current_url, &next_url);
                    redirects.push(current_url.clone());
                    if let Some(policy) = resp.headers().get_all("referrer-policy").iter()
                        .filter_map(|value| value.to_str().ok().and_then(ReferrerPolicy::from_header)).last()
                    {
                        request.referrer_policy = policy;
                    }
                    if method == http::Method::POST && matches!(status.as_u16(), 301 | 302 | 303) {
                        method = http::Method::GET;
                        request_body.clear();
                    }
                    current_url = next_url;
                    continue;
                }
            }

            let body = read_stealth_body_limited(resp, &current_url, request.max_response_bytes)
                .await?;
            drop(in_flight);

            let response = Response {
                url: current_url,
                status: status.as_u16(),
                headers: response_headers,
                body,
                redirected_from: redirects,
                request_referrer: request.referrer,
            };
            if let Some(callbacks) = callbacks {
                callbacks.fire_response(&request_info, &response).await;
            }
            return Ok(response);
        }

        Err(ObscuraNetError::TooManyRedirects(url.to_string()))
    }

    /// One request with no redirect following, for scripted fetch()/XHR. The
    /// caller supplies the Fetch credentials decision for this redirect hop,
    /// while this method preserves the Chrome transport fingerprint.
    pub async fn send_single(
        &self,
        method: &str,
        url: &Url,
        headers: &HashMap<String, String>,
        body: &[u8],
        send_cookies: bool,
        store_cookies: bool,
    ) -> Result<Response, ObscuraNetError> {
        if let Some(host) = url.host_str() {
            if self.block_trackers() && crate::blocklist::is_blocked(host) {
                tracing::debug!("Blocked tracker: {}", url);
                return Ok(Response {
                    status: 0,
                    url: url.clone(),
                    headers: HashMap::new(),
                    body: Vec::new(),
                    redirected_from: Vec::new(),
                    request_referrer: None,
                });
            }
        }

        let mut info = RequestInfo {body: Vec::new(), url: url.clone(), method: method.to_string(), headers: headers.clone(), resource_type: crate::client::ResourceType::Fetch};
        if let Some(response) = self.intercept(&mut info).await? { return Ok(response); }

        let req_method = method
            .parse::<http::Method>()
            .map_err(|e| ObscuraNetError::Network(format!("invalid method '{}': {}", method, e)))?;
        let mut headers = http::header::HeaderMap::new();

        if send_cookies {
            let cookie_header = self.cookie_jar.get_cookie_header(url);
            if !cookie_header.is_empty() {
                header(&mut headers, "cookie", &cookie_header)?;
            }
        }
        for (k, v) in self.extra_headers.read().await.iter() {
            header(&mut headers, k, v)?;
        }
        for (k, v) in info.headers.iter() {
            header(&mut headers, k, v)?;
        }
        let in_flight = InFlightGuard::new(&self.in_flight);
        let resp = self.client.send(req_method, url, headers, body).await?;

        let status = resp.status();
        if store_cookies {
            for val in resp.headers().get_all("set-cookie") {
                if let Ok(s) = val.to_str() {
                    self.cookie_jar.set_cookie(s, url);
                }
            }
        }
        let mut response_headers = HashMap::new();
        for (name, value) in resp.headers() {
            crate::client::merge_response_header(&mut response_headers,
                name.as_str().to_lowercase(), value.to_str().unwrap_or("").to_owned());
        }
        let resp_body = read_stealth_body_limited(resp, url, 64 * 1024 * 1024).await?;
        drop(in_flight);

        Ok(Response {
            url: url.clone(),
            status: status.as_u16(),
            headers: response_headers,
            body: resp_body,
            redirected_from: Vec::new(),
            request_referrer: info.headers.iter().find(|(name, _)| name.eq_ignore_ascii_case("referer"))
                .and_then(|(_, value)| Url::parse(value).ok()),
        })
    }

    pub async fn set_extra_headers(&self, headers: HashMap<String, String>) {
        *self.extra_headers.write().await = headers;
    }

    pub fn active_requests(&self) -> u32 {
        self.in_flight.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn is_network_idle(&self) -> bool {
        self.active_requests() == 0
    }
}

#[cfg(all(test, feature = "stealth"))]
mod tests {
    #[tokio::test]
    async fn xhr_post_replaces_accept_and_content_type_defaults() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut bytes = Vec::new();
            loop {
                let mut buf = [0; 4096];
                let n = stream.read(&mut buf).unwrap();
                assert!(n > 0);
                bytes.extend_from_slice(&buf[..n]);
                if bytes.windows(4).any(|v| v == b"\r\n\r\n") && bytes.ends_with(b"{}") { break; }
            }
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok").unwrap();
            String::from_utf8(bytes).unwrap().to_ascii_lowercase()
        });
        let url = Url::parse(&format!("http://{address}/api")).unwrap();
        let client = StealthHttpClient::with_proxy(Arc::new(CookieJar::new()), None, true);
        client.set_extra_headers([
            ("accept".into(), "application/json".into()),
            ("content-type".into(), "application/json".into()),
        ].into_iter().collect()).await;
        let profile = crate::client::ResourceRequest::subresource(
            crate::client::ResourceType::Xhr,
            &url,
        );
        client.post_form_resource_with_callbacks(&url, "{}", profile, None).await.unwrap();
        let request = server.join().unwrap();
        assert_eq!(request.matches("accept:").count(), 1, "{request}");
        assert!(request.contains("accept: application/json\r\n"), "{request}");
        assert_eq!(request.matches("content-type:").count(), 1, "{request}");
        assert!(request.contains("content-type: application/json\r\n"), "{request}");
        assert!(request.contains("priority: u=1, i\r\n"), "{request}");
    }

    #[tokio::test]
    async fn form_redirects_preserve_or_drop_body_and_keep_cookie_origin() {
        for status in [302, 303, 307, 308] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let server = std::thread::spawn(move || {
                let mut requests = Vec::new();
                for hop in 0..2 {
                    let (mut stream, _) = listener.accept().unwrap();
                    stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                    let mut bytes = Vec::new();
                    loop {
                        let mut buf = [0; 4096];
                        let n = stream.read(&mut buf).unwrap();
                        assert!(n > 0);
                        bytes.extend_from_slice(&buf[..n]);
                        if let Some(end) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
                            let headers = String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();
                            let len = headers.lines().find_map(|s| s.strip_prefix("content-length: "))
                                .map(|s| s.parse::<usize>().unwrap()).unwrap_or(0);
                            if bytes.len() >= end + 4 + len { break; }
                        }
                    }
                    requests.push(String::from_utf8(bytes).unwrap().to_ascii_lowercase());
                    let response = if hop == 0 {
                        format!("HTTP/1.1 {status} Redirect\r\nLocation: /done\r\nSet-Cookie: session=fixture; Path=/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    } else { "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok".into() };
                    stream.write_all(response.as_bytes()).unwrap();
                }
                requests
            });
            let url = Url::parse(&format!("http://{address}/login")).unwrap();
            let mut profile = crate::client::ResourceRequest::navigation();
            profile.initiator = Some(url.clone());
            profile.referrer = Some(url.clone());
            let client = StealthHttpClient::with_proxy(Arc::new(CookieJar::new()), None, true);
            let result = client.post_form_resource_with_callbacks(&url, "name=value", profile, None).await.unwrap();
            assert_eq!(result.status, 200);
            let requests = server.join().unwrap();
            assert!(requests[0].contains(&format!("origin: http://{address}\r\n")));
            assert!(requests[0].ends_with("name=value"));
            assert!(requests[1].contains("cookie: session=fixture\r\n"));
            if status == 307 || status == 308 {
                assert!(requests[1].starts_with("post /done "));
                assert!(requests[1].ends_with("name=value"));
            } else {
                assert!(requests[1].starts_with("get /done "));
                assert!(!requests[1].contains("content-type:"));
                assert!(!requests[1].contains("name=value"));
            }
        }
    }

    use std::io::{Read, Write};
    use std::sync::Arc;
    use std::time::Duration;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use url::Url;

    use super::StealthHttpClient;
    use crate::client::{ObscuraNetError, SsrfGuardResolver};
    use crate::cookies::CookieJar;
    use primp::dns::{Name, Resolve};

    // Mirrors client::ssrf_tests::resolver_blocks_hostname_that_resolves_to_loopback.
    // Both transports must agree: a host-string check alone cannot see that a
    // public name points inward, so the stealth client needs the same resolver.
    #[tokio::test]
    async fn stealth_resolver_blocks_hostname_that_resolves_to_loopback() {
        let resolver = SsrfGuardResolver::new(false);
        let res = resolver.resolve("localtest.me".parse::<Name>().unwrap()).await;
        assert!(res.is_err(), "localtest.me -> 127.0.0.1 must be blocked");
    }

    #[tokio::test]
    async fn stealth_resolver_does_not_block_public_host() {
        // Tolerate a no-network sandbox: only an actual SSRF rejection fails.
        let resolver = SsrfGuardResolver::new(false);
        match resolver.resolve("example.com".parse::<Name>().unwrap()).await {
            Ok(_) => {}
            Err(e) => assert!(
                !e.to_string().contains("SSRF blocked"),
                "example.com wrongly SSRF-blocked: {e}"
            ),
        }
    }

    const PLAIN_BODY: &str = "<!DOCTYPE html><html><body><p id=\"mark\">gzip ok</p></body></html>";

    // gzip (level 9) of PLAIN_BODY, hardcoded so the fixture needs no
    // compression dependency. A wrong byte fails the assert below.
    const GZIP_BODY: &[u8] = &[
        0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x03, 0xb3, 0x51,
        0x74, 0xf1, 0x77, 0x0e, 0x89, 0x0c, 0x70, 0x55, 0xc8, 0x28, 0xc9, 0xcd,
        0xb1, 0xb3, 0x81, 0x90, 0x49, 0xf9, 0x29, 0x95, 0x76, 0x36, 0x05, 0x0a,
        0x99, 0x29, 0xb6, 0x4a, 0xb9, 0x89, 0x45, 0xd9, 0x4a, 0x76, 0xe9, 0x55,
        0x99, 0x05, 0x0a, 0xf9, 0xd9, 0x36, 0xfa, 0x05, 0x76, 0x36, 0xfa, 0x10,
        0x69, 0x7d, 0xb0, 0x5a, 0x00, 0x80, 0x3d, 0x1c, 0x5f, 0x41, 0x00, 0x00,
        0x00,
    ];

    fn reset_fixture(respond_after_reset: bool) -> (u16, std::thread::JoinHandle<usize>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let attempts = if respond_after_reset { 2 } else { 1 };
            for attempt in 0..attempts {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut request = Vec::new();
                let mut buf = [0u8; 1024];
                while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                    let read = stream.read(&mut buf).unwrap();
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&buf[..read]);
                }

                if attempt == 0 {
                    let socket = socket2::Socket::from(stream);
                    socket.set_linger(Some(Duration::ZERO)).unwrap();
                    drop(socket);
                } else {
                    stream
                        .write_all(
                            b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok",
                        )
                        .unwrap();
                }
            }
            attempts
        });
        (port, server)
    }

    #[tokio::test]
    async fn stealth_get_does_not_replay_connection_reset() {
        let (port, server) = reset_fixture(false);
        let client = super::transport::Client::new(super::StealthProfile::WindowsChrome145, None, true, None, None);
        let url = Url::parse(&format!("http://127.0.0.1:{port}/")).unwrap();
        assert!(client.send(http::Method::GET, &url, http::header::HeaderMap::new(), &[]).await.is_err());
        assert_eq!(server.join().unwrap(), 1);
    }

    #[tokio::test]
    async fn both_profiles_send_consistent_identity_without_prefetch_defaults() {
        for (profile, platform, brands) in [
            (super::StealthProfile::WindowsChrome145, "Windows", r#""Not:A-Brand";v="99", "Google Chrome";v="145", "Chromium";v="145""#),
            (super::StealthProfile::MacChrome152, "macOS", r#""Chromium";v="152", "Not?A_Brand";v="24", "Google Chrome";v="152""#),
        ] {
            let server = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = Url::parse(&format!("http://{}/", server.local_addr().unwrap())).unwrap();
            let client = super::transport::Client::new(profile, None, true, None, None);
            let receive = async {
                let (mut socket, _) = server.accept().await.unwrap();
                let mut request = Vec::new();
                while !request.windows(4).any(|v| v == b"\r\n\r\n") {
                    let mut buf = [0; 4096];
                    let n = socket.read(&mut buf).await.unwrap();
                    assert!(n > 0);
                    request.extend_from_slice(&buf[..n]);
                }
                let request = String::from_utf8(request).unwrap();
                assert!(request.contains(&format!("user-agent: {}\r\n", profile.user_agent())));
                assert!(request.contains(&format!("sec-ch-ua-platform: \"{platform}\"\r\n")));
                assert!(request.contains(&format!("sec-ch-ua: {brands}\r\n")));
                assert!(!request.contains("sec-purpose:"));
                socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await.unwrap();
            };
            let exchange = async {
                let (response, _) = tokio::join!(client.send(http::Method::GET, &url, http::header::HeaderMap::new(), &[]), receive);
                assert_eq!(response.unwrap().status(), http::StatusCode::OK);
            };
            tokio::time::timeout(Duration::from_secs(5), exchange).await.unwrap();
        }
    }

    #[tokio::test]
    async fn macos_transport_rejects_localhost() {
        let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        server.set_nonblocking(true).unwrap();
        let url = Url::parse(&format!("http://localhost:{}/", server.local_addr().unwrap().port())).unwrap();
        let client = super::transport::Client::new(super::StealthProfile::MacChrome152, None, false, None, None);
        let result = tokio::time::timeout(std::time::Duration::from_secs(2),
            client.send(http::Method::GET, &url, http::header::HeaderMap::new(), &[])).await;
        assert!(matches!(server.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock),
            "loopback lookup bypassed the custom SSRF resolver");
        assert!(matches!(result, Ok(Err(_))), "custom DNS rejection must fail before connecting");
    }

    #[tokio::test]
    async fn macos_explicit_loopback_proxy_is_reachable() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let server = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy = format!("http://{}", server.local_addr().unwrap());
        let client = super::transport::Client::new(super::StealthProfile::MacChrome152, Some(&proxy), false, None, None);
        let url = Url::parse("https://proxy-destination.invalid/").unwrap();
        let exchange = async {
            let request = client.send(http::Method::GET, &url, http::header::HeaderMap::new(), &[]);
            let receive = async {
                let (mut socket, _) = server.accept().await.unwrap();
                let mut buf = [0; 4096];
                let n = socket.read(&mut buf).await.unwrap();
                assert!(std::str::from_utf8(&buf[..n]).unwrap()
                    .starts_with("CONNECT proxy-destination.invalid:443 HTTP/1.1"));
                socket.write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n").await.unwrap();
            };
            let (response, _) = tokio::join!(request, receive);
            assert!(response.is_err());
        };
        tokio::time::timeout(std::time::Duration::from_secs(2), exchange).await.unwrap();
        let jar = Arc::new(CookieJar::new());
        let policy = Arc::new(crate::client::ObscuraHttpClient::with_full_options(jar.clone(), Some(&proxy), false));
        let guarded = StealthHttpClient::with_policy_profile(jar, Some(&proxy), policy, super::StealthProfile::MacChrome152);
        assert!(guarded.fetch(&Url::parse("http://127.0.0.1:1/").unwrap()).await.is_err());
        assert!(tokio::time::timeout(std::time::Duration::from_millis(20), server.accept()).await.is_err());
    }

    #[tokio::test]
    async fn macos_post_does_not_retry_connection_reset() {
        let (port, server) = reset_fixture(false);
        let client = super::transport::Client::new(super::StealthProfile::MacChrome152, None, true, None, None);
        let url = Url::parse(&format!("http://127.0.0.1:{port}/")).unwrap();
        let result = client.send(http::Method::POST, &url, http::header::HeaderMap::new(), b"payload").await;
        assert!(result.is_err());
        assert_eq!(server.join().unwrap(), 1);
    }

    #[tokio::test]
    async fn stealth_post_does_not_retry_connection_reset() {
        let (port, server) = reset_fixture(false);
        let client = StealthHttpClient {
            client: super::transport::Client::new(super::StealthProfile::WindowsChrome145, None, true, None, None),
            allow_private_network: true,
            cookie_jar: Arc::new(CookieJar::new()),
            extra_headers: tokio::sync::RwLock::new(std::collections::HashMap::new()),
            in_flight: Arc::new(std::sync::atomic::AtomicU32::new(0)),
            policy: None,
        };
        let url = Url::parse(&format!("http://127.0.0.1:{port}/")).unwrap();
        let error = client
            .send_single(
                "POST",
                &url,
                &std::collections::HashMap::new(),
                b"payload",
                false,
                false,
            )
            .await
            .expect_err("POST must not be retried after a connection reset");

        assert!(matches!(error, ObscuraNetError::Network(_)));
        assert_eq!(server.join().unwrap(), 1);
    }

    /// Serve one `Content-Encoding: gzip` response on an ephemeral port.
    async fn gzip_fixture() -> u16 {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut buf = [0u8; 1024];
                    let _ = stream.read(&mut buf).await;
                    let head = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: text/html; charset=utf-8\r\ncontent-encoding: gzip\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                        GZIP_BODY.len()
                    );
                    let _ = stream.write_all(head.as_bytes()).await;
                    let _ = stream.write_all(GZIP_BODY).await;
                    let _ = stream.shutdown().await;
                });
            }
        });

        port
    }

    // The emulation profile advertises gzip, so origins compress. Without the
    // decoder the raw gzip bytes reach the HTML parser as document text.
    #[tokio::test]
    async fn stealth_client_decodes_gzip_response() {
        let port = gzip_fixture().await;
        let cookie_jar = Arc::new(CookieJar::new());
        let policy = Arc::new(crate::client::ObscuraHttpClient::with_full_options(
            cookie_jar.clone(), None, true,
        ));
        let client = StealthHttpClient::with_policy(cookie_jar, None, policy);
        let url = Url::parse(&format!("http://127.0.0.1:{port}/")).unwrap();

        let resp = client.fetch(&url).await.expect("fixture must be reachable");
        assert_eq!(resp.status, 200);
        assert_eq!(resp.text(), PLAIN_BODY, "gzip body must be decompressed");
    }

    // #793: the opt-in must reach the DNS resolver. `validate_url` already
    // honours it for the localhost host, so a hostname target exercises the
    // resolver itself; before the fix the resolver was pinned to block and
    // refused loopback hostnames even with the flag set. Only the allowed
    // leg is asserted: CI sets OBSCURA_ALLOW_PRIVATE_NETWORK, which also
    // lifts the default block.
    #[tokio::test]
    async fn stealth_client_honors_allow_private_network_for_loopback_hostnames() {
        let port = gzip_fixture().await;
        let client = StealthHttpClient::with_proxy(Arc::new(CookieJar::new()), None, true);
        let url = Url::parse(&format!("http://localhost:{port}/")).unwrap();

        let resp = client
            .fetch(&url)
            .await
            .expect("loopback hostname must be reachable with the opt-in");
        assert_eq!(resp.status, 200);
    }
}
