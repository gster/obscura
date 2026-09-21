#[path = "stealth_transport.rs"]
// Visible to the crate so a client can be rebuilt for another runtime; see
// `StealthHttpClient::detached`.
pub(crate) mod transport;
use transport::header;
use crate::observation::RequestTrace;

use std::collections::HashMap;
use std::error::Error;
use std::sync::Arc;

use futures_util::StreamExt;
use tokio::sync::{RwLock, watch};
use url::Url;

use crate::cookies::CookieJar;
use crate::client::{
    CallbackRegistry, InFlightGuard, ObscuraNetError, RequestInfo, RequestMode,
    ReferrerPolicy, ResourceRequest, Response, SsrfGuardResolver, cors_required, env_allows_private_network,
    fetch_file_url, is_forbidden_ip, redirect_taints_origin, request_fetch_site,
    request_referrer, response_too_large, serialized_request_origin, validate_cors_response,
    validate_request_mode, validate_url, response_cache_lifetime, ResourceCacheKey,
    ResourceLoaderState, SharedFetchLeader, SharedFetchOutcome, SharedFetchSender,
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

pub const STEALTH_USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/145.0.0.0 Safari/537.36";

// The Windows Chrome145 profile sends this exact
// UA and sec-ch-ua-platform "Windows" on the wire. navigator has to report the
// same identity, otherwise the TLS/HTTP layer and the JS layer disagree and a
// site cross-checks the mismatch as a bot signal.
pub const STEALTH_NAVIGATOR_PLATFORM: &str = "Win32";
pub const STEALTH_UA_PLATFORM: &str = "Windows";
pub const STEALTH_UA_PLATFORM_VERSION: &str = "15.0.0";

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
/// MacChrome152 / MacChrome153 use primp's matching Chrome build with an
/// explicit macOS identity. The Chrome major version drives everything derived
/// from the UA string (notably the GREASE sec-ch-ua brand, its version and the
/// brand order), so the profile must track a real build rather than a generic
/// "Chrome" persona.
/// ALPS and trust-anchor contents still differ from the reference Chrome.
#[derive(Clone, Copy, Debug, serde::Deserialize, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StealthProfile {
    WindowsChrome145,
    MacChrome152,
    MacChrome153,
}

impl StealthProfile {
    pub fn name(self) -> &'static str {
        match self {
            Self::WindowsChrome145 => "windows_chrome145",
            Self::MacChrome152 => "macos_chrome152",
            Self::MacChrome153 => "macos_chrome153",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "windows_chrome145" => Some(Self::WindowsChrome145),
            "macos_chrome152" => Some(Self::MacChrome152),
            "macos_chrome153" => Some(Self::MacChrome153),
            _ => None,
        }
    }

    pub fn user_agent(self) -> &'static str {
        match self {
            Self::WindowsChrome145 => STEALTH_USER_AGENT,
            Self::MacChrome152 => "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/152.0.0.0 Safari/537.36",
            Self::MacChrome153 => "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/153.0.0.0 Safari/537.36",
        }
    }

    /// The full Chrome version reported by `navigator.userAgentData.uaFullVersion`.
    /// Taken from the real build the profile impersonates; the reduced UA string
    /// alone would report `major.0.0.0`.
    pub fn full_version(self) -> &'static str {
        match self {
            Self::WindowsChrome145 => "145.0.0.0",
            Self::MacChrome152 => "152.0.7977.83",
            Self::MacChrome153 => "153.0.8010.50",
        }
    }
    pub fn platform(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::WindowsChrome145 => (STEALTH_NAVIGATOR_PLATFORM, STEALTH_UA_PLATFORM, STEALTH_UA_PLATFORM_VERSION),
            Self::MacChrome152 => ("MacIntel", "macOS", "26.6.2"),
            Self::MacChrome153 => ("MacIntel", "macOS", "26.6.2"),
        }
    }

}

/// The parameters a transport client is built from. Kept on the client so a
/// sibling can be constructed for a different runtime with the same identity.
#[derive(Clone, Debug)]
pub struct TransportParams {
    pub profile: StealthProfile,
    pub proxy_url: Option<String>,
    pub accept_language: Option<String>,
    pub do_not_track: Option<String>,
}

pub struct StealthHttpClient {
    client: transport::Client,
    allow_private_network: bool,
    pub cookie_jar: Arc<CookieJar>,
    /// Page overrides layered over the owning policy's context headers.
    /// Detached workers share this set; sibling pages get independent sets.
    pub extra_headers: Arc<RwLock<HashMap<String, String>>>,
    pub in_flight: Arc<std::sync::atomic::AtomicU32>,
    resource_loader: Arc<std::sync::Mutex<ResourceLoaderState>>,
    policy: Option<Arc<crate::client::ObscuraHttpClient>>,
    transport: TransportParams,
}

// Header names are case-insensitive. Preserve original names and values
// within each input map; only the lower-priority layer is replaced.
fn overlay_headers(headers: &mut HashMap<String, String>, overrides: &HashMap<String, String>) {
    headers.retain(|name, _| !overrides.keys().any(|key| key.eq_ignore_ascii_case(name)));
    headers.extend(overrides.iter().map(|(name, value)| (name.clone(), value.clone())));
}

impl StealthHttpClient {
    pub fn new(cookie_jar: Arc<CookieJar>, persona: &crate::EffectivePersona) -> Self {
        Self::with_proxy(cookie_jar, None, false, persona)
    }

    pub fn with_proxy(
        cookie_jar: Arc<CookieJar>,
        proxy_url: Option<&str>,
        allow_private_network: bool,
        persona: &crate::EffectivePersona,
    ) -> Self {
        Self::with_options(cookie_jar, proxy_url, None, allow_private_network, persona)
    }

    pub fn with_policy(
        cookie_jar: Arc<CookieJar>,
        proxy_url: Option<&str>,
        policy: Arc<crate::client::ObscuraHttpClient>,
        persona: &crate::EffectivePersona,
    ) -> Self {
        let allow_private_network = policy.allow_private_network;
        Self::with_options(cookie_jar, proxy_url, Some(policy), allow_private_network, persona)
    }

    pub fn with_policy_persona(
        cookie_jar: Arc<CookieJar>,
        proxy_url: Option<&str>,
        policy: Arc<crate::client::ObscuraHttpClient>,
        persona: &crate::EffectivePersona,
    ) -> Self {
        let allow_private_network = policy.allow_private_network;
        let client = transport::Client::new(
            persona.profile(), proxy_url, allow_private_network,
            Some(persona.accept_language()), persona.do_not_track(),
        );
        StealthHttpClient {
            client,
            allow_private_network,
            cookie_jar,
            extra_headers: Arc::new(RwLock::new(HashMap::new())),
            in_flight: Arc::new(std::sync::atomic::AtomicU32::new(0)),
            resource_loader: Arc::new(std::sync::Mutex::new(ResourceLoaderState::default())),
            policy: Some(policy),
            transport: TransportParams {
                profile: persona.profile(),
                proxy_url: proxy_url.map(str::to_owned),
                accept_language: Some(persona.accept_language().to_owned()),
                do_not_track: persona.do_not_track().map(str::to_owned),
            },
        }
    }

    fn with_options(
        cookie_jar: Arc<CookieJar>,
        proxy_url: Option<&str>,
        policy: Option<Arc<crate::client::ObscuraHttpClient>>,
        allow_private_network: bool,
        persona: &crate::EffectivePersona,
    ) -> Self {
        let client = transport::Client::new(
            persona.profile(), proxy_url, allow_private_network,
            Some(persona.accept_language()), persona.do_not_track(),
        );
        StealthHttpClient {
            client,
            allow_private_network,
            cookie_jar,
            extra_headers: Arc::new(RwLock::new(HashMap::new())),
            in_flight: Arc::new(std::sync::atomic::AtomicU32::new(0)),
            resource_loader: Arc::new(std::sync::Mutex::new(ResourceLoaderState::default())),
            policy,
            transport: TransportParams {
                profile: persona.profile(),
                proxy_url: proxy_url.map(str::to_owned),
                accept_language: Some(persona.accept_language().to_owned()),
                do_not_track: persona.do_not_track().map(str::to_owned),
            },
        }
    }

    pub fn transport_params(&self) -> &TransportParams {
        &self.transport
    }

    /// Cookie and policy bindings may change, but wire identity is fixed by
    /// the runtime's compiled persona.
    pub fn matches_persona(&self, persona: &crate::EffectivePersona) -> bool {
        self.transport.profile == persona.profile()
            && self.transport.accept_language.as_deref() == Some(persona.accept_language())
            && self.transport.do_not_track.as_deref() == persona.do_not_track()
    }

    /// The policy owner for a runtime binding. Transport-only clients still
    /// need the same URL/private-network gate before scripted interception.
    pub fn policy_client(&self) -> Arc<crate::client::ObscuraHttpClient> {
        self.policy.clone().unwrap_or_else(|| {
            let mut policy = crate::client::ObscuraHttpClient::with_full_options(
                self.cookie_jar.clone(), self.transport.proxy_url.as_deref(), self.allow_private_network,
            );
            policy.block_trackers = self.block_trackers();
            Arc::new(policy)
        })
    }

    /// Change the jar without changing this transport's proxy or policy.
    pub fn with_cookie_binding(&self, cookie_jar: Arc<CookieJar>) -> Self {
        let mut client = self.detached();
        client.cookie_jar = cookie_jar.clone();
        client.resource_loader = Arc::new(std::sync::Mutex::new(ResourceLoaderState::default()));
        if let Some(policy) = &self.policy {
            let mut policy = policy.detached();
            policy.cookie_jar = cookie_jar;
            client.policy = Some(Arc::new(policy));
        }
        client
    }

    /// Rebind policy/cookies without changing persona or request accounting.
    /// Used by standalone runtime setup setters, including after lazy binding.
    pub fn with_policy_binding(&self, cookie_jar: Arc<CookieJar>, policy: Arc<crate::client::ObscuraHttpClient>) -> Self {
        let mut transport = self.transport.clone();
        transport.proxy_url = policy.proxy_url().map(str::to_owned);
        StealthHttpClient {
            client: transport::Client::new(
                transport.profile, transport.proxy_url.as_deref(), policy.allow_private_network,
                transport.accept_language.as_deref(), transport.do_not_track.as_deref(),
            ),
            allow_private_network: policy.allow_private_network,
            cookie_jar,
            extra_headers: self.extra_headers.clone(),
            in_flight: self.in_flight.clone(),
            resource_loader: Arc::new(std::sync::Mutex::new(ResourceLoaderState::default())),
            policy: Some(policy),
            transport,
        }
    }

    /// A client with this one's identity but its own connection pool.
    ///
    /// A connection pool belongs to the tokio runtime that drives it: a pooled
    /// connection handed to a *different* runtime is already dead, so the first
    /// reuse fails with a broken pipe ("connection closed because of a broken
    /// pipe"). Workers run on their own thread and runtime, so they must build
    /// their own transport instead of sharing the page's.
    ///
    /// Cookies, in-flight accounting, header overrides and the policy
    /// (interceptor, blocked trackers) stay shared - only the pool is new.
    pub fn detached(&self) -> Self {
        let params = &self.transport;
        StealthHttpClient {
            client: transport::Client::new(
                params.profile,
                params.proxy_url.as_deref(),
                self.allow_private_network,
                params.accept_language.as_deref(),
                params.do_not_track.as_deref(),
            ),
            allow_private_network: self.allow_private_network,
            cookie_jar: self.cookie_jar.clone(),
            extra_headers: self.extra_headers.clone(),
            in_flight: self.in_flight.clone(),
            resource_loader: self.resource_loader.clone(),
            policy: self.policy.clone(),
            transport: params.clone(),
        }
    }

    async fn request_headers(&self) -> HashMap<String, String> {
        let mut headers = match &self.policy {
            Some(policy) => policy.extra_headers.read().await.clone(),
            None => HashMap::new(),
        };
        overlay_headers(&mut headers, &*self.extra_headers.read().await);
        headers
    }

    fn block_trackers(&self) -> bool {
        self.policy.as_ref().map_or(true, |p| p.block_trackers)
    }

    async fn intercept(
        &self,
        info: &mut RequestInfo,
        body: Option<&[u8]>,
        fields: Option<&mut Vec<(String, String)>>,
    ) -> Result<Option<Response>, ObscuraNetError> {
        validate_url(&info.url, self.allow_private_network)?;
        if let Some(policy) = &self.policy {
            let interceptor = policy.interceptor.read().await.clone();
            if let Some(interceptor) = interceptor {
                if let Some(body) = body {
                    info.body.extend_from_slice(body);
                }
                if let Some(fields) = fields.as_ref() {
                    info.raw_headers = Some(crate::HeaderCapture {
                        capture_stage: "requestPolicy", encoding: "base64",
                        fields: fields.iter().map(|(name, value)| crate::RawHeader {
                            name: name.as_bytes().to_vec(), value: value.as_bytes().to_vec(),
                        }).collect(),
                    });
                }
                let action = interceptor.intercept(info).await;
                // Request bodies can be large. The interceptor has already
                // observed every byte, so do not retain the copy across I/O.
                info.body = Vec::new();
                match action {
                    crate::interceptor::InterceptAction::Continue => {}
                    crate::interceptor::InterceptAction::Block => return Err(ObscuraNetError::Blocked(info.url.to_string())),
                    crate::interceptor::InterceptAction::Fulfill(response) => return Ok(Some(response)),
                    crate::interceptor::InterceptAction::ModifyHeaders(headers) => {
                        if let Some(fields) = fields {
                            fields.retain(|(name, _)| !headers.keys().any(|key| key.eq_ignore_ascii_case(name)));
                            fields.extend(headers.iter().map(|(name, value)| (name.clone(), value.clone())));
                        }
                        info.headers.extend(headers);
                    }
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
        self.fetch_with_profile(url, ResourceRequest::navigation(), callbacks, None)
            .await
    }

    pub async fn fetch_resource_with_callbacks(
        &self,
        url: &Url,
        request: ResourceRequest,
        callbacks: Option<&CallbackRegistry>,
    ) -> Result<Response, ObscuraNetError> {
        self.fetch_with_profile(url, request, callbacks, None).await
    }

    pub async fn fetch_resource_traced(
        &self, url: &Url, request: ResourceRequest, callbacks: Option<&CallbackRegistry>, trace: &RequestTrace,
    ) -> Result<Response, ObscuraNetError> {
        let result = self.fetch_with_profile(url, request, callbacks, Some(trace)).await;
        if let Err(error) = &result { trace.fail(&error.to_string()); }
        result
    }

    async fn fetch_with_profile(
        &self,
        url: &Url,
        request: ResourceRequest,
        callbacks: Option<&CallbackRegistry>,
        trace: Option<&RequestTrace>,
    ) -> Result<Response, ObscuraNetError> {
        let Some(cache_key) = self.resource_cache_key(url, &request).await else {
            return self.fetch_method_with_profile(url, request, callbacks, http::Method::GET, &[], trace).await;
        };

        enum Acquisition {
            Cached(Response),
            Follower(watch::Receiver<Option<SharedFetchOutcome>>),
            Leader(SharedFetchSender),
        }

        let acquisition = {
            let mut loader = self.resource_loader.lock().unwrap();
            if let Some(response) = loader.cache.get(&cache_key) {
                Acquisition::Cached(response)
            } else if let Some(sender) = loader.shared_fetches.get(&cache_key) {
                Acquisition::Follower(sender.subscribe())
            } else {
                let (sender, _receiver) = watch::channel(None);
                loader.shared_fetches.insert(cache_key.clone(), sender.clone());
                Acquisition::Leader(sender)
            }
        };

        match acquisition {
            Acquisition::Cached(response) => {
                if let Some(trace) = trace {
                    trace.start()?;
                    let mut observed = response.clone();
                    // This logical requester did not prepare or send the
                    // cached request. Preserve the response capture but never
                    // attribute the transport leader's request headers to it.
                    observed.request_raw_headers = None;
                    trace.response(&observed, true);
                }
                self.fire_logical_resource_callbacks(callbacks, url, &request, &response).await;
                Ok(response)
            }
            Acquisition::Follower(mut receiver) => loop {
                if let Some(trace) = trace { trace.start()?; }
                let outcome = { receiver.borrow().clone() };
                if let Some(outcome) = outcome {
                    break match outcome {
                        SharedFetchOutcome::Cacheable(response) => {
                            if let Some(trace) = trace {
                                let mut observed = response.clone();
                                observed.request_raw_headers = None;
                                trace.response(&observed, true);
                            }
                self.fire_logical_resource_callbacks(callbacks, url, &request, &response).await;
                            Ok(response)
                        }
                        SharedFetchOutcome::RetryUncoalesced => {
                            self.fetch_method_with_profile(url, request, callbacks, http::Method::GET, &[], trace).await
                        }
                    };
                }
                if receiver.changed().await.is_err() {
                    break self.fetch_method_with_profile(url, request, callbacks, http::Method::GET, &[], trace).await;
                }
            },
            Acquisition::Leader(sender) => {
                let leader = SharedFetchLeader {
                    loader: &self.resource_loader,
                    key: cache_key.clone(),
                    sender,
                    finished: false,
                };
                let result = self.fetch_method_with_profile(
                    url, request, callbacks, http::Method::GET, &[], trace,
                ).await;
                let outcome = match &result {
                    Ok(response) => match response_cache_lifetime(response) {
                        Some(lifetime) => {
                            self.resource_loader.lock().unwrap().cache.insert(
                                cache_key, response.clone(), lifetime,
                            );
                            SharedFetchOutcome::Cacheable(response.clone())
                        }
                        None => SharedFetchOutcome::RetryUncoalesced,
                    },
                    Err(_) => SharedFetchOutcome::RetryUncoalesced,
                };
                leader.finish(outcome);
                result
            }
        }
    }

    async fn resource_cache_key(
        &self,
        url: &Url,
        request: &ResourceRequest,
    ) -> Option<ResourceCacheKey> {
        if request.resource_type == crate::client::ResourceType::Document
            || !matches!(url.scheme(), "http" | "https")
        {
            return None;
        }
        if let Some(policy) = &self.policy {
            if policy.interceptor.read().await.is_some() {
                return None;
            }
        }
        let mut extra_headers = self.request_headers().await.iter()
            .map(|(name, value)| (name.to_ascii_lowercase(), value.clone()))
            .collect::<Vec<_>>();
        extra_headers.sort();
        if extra_headers.iter().any(|(name, value)| {
            name == "authorization"
                || name == "cookie"
                || (name == "cache-control"
                    && (value.to_ascii_lowercase().contains("no-cache")
                        || value.to_ascii_lowercase().contains("no-store")))
        }) {
            return None;
        }
        if request.sends_credentials_to(url) && !self.cookie_jar.get_cookie_header(url).is_empty() {
            return None;
        }
        Some(ResourceCacheKey {
            url: url.to_string(),
            resource_type: request.resource_type,
            mode: request.mode,
            credentials: request.credentials,
            initiator: request.initiator.as_ref().map(ToString::to_string),
            referrer: request.referrer.as_ref().map(ToString::to_string),
            referrer_policy: request.referrer_policy,
            user_agent: self.transport.profile.user_agent().to_string(),
            extra_headers,
            max_response_bytes: request.max_response_bytes,
        })
    }

    async fn fire_logical_resource_callbacks(
        &self,
        callbacks: Option<&CallbackRegistry>,
        url: &Url,
        request: &ResourceRequest,
        response: &Response,
    ) {
        let Some(callbacks) = callbacks else { return; };
        let request_info = RequestInfo {
            raw_headers: response.request_raw_headers.clone(),
            body: Vec::new(),
            url: url.clone(),
            method: http::Method::GET.to_string(),
            headers: response.request_raw_headers.as_ref().map(|h| h.text_headers()).unwrap_or_default(),
            resource_type: request.resource_type,
        };
        callbacks.fire_request(&request_info).await;
        callbacks.fire_response(&request_info, response).await;
    }

    pub async fn post_form_resource_with_callbacks(
        &self, url: &Url, body: &str, request: ResourceRequest,
        callbacks: Option<&CallbackRegistry>,
    ) -> Result<Response, ObscuraNetError> {
        self.fetch_method_with_profile(url, request, callbacks, http::Method::POST, body.as_bytes(), None).await
    }

    pub async fn post_form_resource_traced(
        &self, url: &Url, body: &str, request: ResourceRequest,
        callbacks: Option<&CallbackRegistry>, trace: &RequestTrace,
    ) -> Result<Response, ObscuraNetError> {
        let result = self.fetch_method_with_profile(
            url, request, callbacks, http::Method::POST, body.as_bytes(), Some(trace),
        ).await;
        if let Err(error) = &result { trace.fail(&error.to_string()); }
        result
    }

    async fn fetch_method_with_profile(
        &self, url: &Url, mut request: ResourceRequest,
        callbacks: Option<&CallbackRegistry>, mut method: http::Method, initial_body: &[u8],
        trace: Option<&RequestTrace>,
    ) -> Result<Response, ObscuraNetError> {
        let _in_flight = InFlightGuard::new(&self.in_flight);
        let mut request_body = initial_body.to_vec();
        validate_url(url, self.allow_private_network)?;
        validate_request_mode(&request, url)?;
        if url.scheme() == "file" {
            if let Some(mut response) = self.intercept(&mut RequestInfo {raw_headers: None, body: Vec::new(), url: url.clone(), method: "GET".into(), headers: HashMap::new(), resource_type: request.resource_type}, None, None).await? {
                response.request_referrer = None;
                return Ok(response);
            }
            if let Some(trace) = trace { trace.start()?; }
            let response = fetch_file_url(url, request.max_response_bytes).await?;
            if let Some(trace) = trace { trace.response(&response, true); }
            return Ok(response);
        }

        let mut current_url = url.clone();

        let mut redirects = Vec::new();
        let mut redirect_tainted = false;
        let mut request_callback_fired = false;

        // Follow up to 20 redirects (Fetch spec). `hop == 20` is the 21st
        // response and must fail the current, already-started hop rather than
        // first publishing it as a successful redirect.
        for hop in 0..=20 {
            if !redirects.is_empty() {
                if let Some(trace) = trace {
                    trace.begin(current_url.as_str(), method.as_str(), None,
                        (method == http::Method::POST).then_some(request_body.as_slice()))
                        .map_err(|error| ObscuraNetError::Network(error.to_string()))?;
                }
            }
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
                        raw_headers: None,
                        request_raw_headers: None,
                        request_referrer: None,
                    });
                }
            }

            let mut request_info = RequestInfo {
                raw_headers: None,
                body: Vec::new(),
                url: current_url.clone(), method: method.to_string(),
                headers: self.request_headers().await, resource_type: request.resource_type,
            };
            if let Some(mut response) = self.intercept(&mut request_info, Some(&request_body), None).await? {
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

            let (transport, prepared) = self.client.request(method.clone(), &current_url, headers, &request_body, std::time::Duration::from_secs(30))?;
            request_info.raw_headers = Some(crate::HeaderCapture::from_headers("transportRequest", prepared.headers()));
            if let Some(trace) = trace {
                trace.prepared(request_info.raw_headers.clone().unwrap(), &request_body)
                    ?;
            }
            request_info.headers = request_info.raw_headers.as_ref().unwrap().text_headers();
            if !request_callback_fired {
                if let Some(callbacks) = callbacks {
                    request_info.body = request_body.clone();
                    callbacks.fire_request(&request_info).await;
                    request_info.body = Vec::new();
                }
                request_callback_fired = true;
            }

            let resp = self.client.send_prepared(transport, prepared).await?;

            let status = resp.status();
            let cors_result = validate_stealth_cors_response(
                &request, &current_url, &request_origin, resp.headers(),
            );
            if trace.is_none() { cors_result.as_ref().map_err(|error| ObscuraNetError::Cors(error.to_string()))?; }

            if cors_result.is_ok() && request.sends_credentials_to(&current_url) {
                for val in resp.headers().get_all("set-cookie") {
                    if let Ok(s) = val.to_str() {
                        self.cookie_jar.set_cookie(s, &current_url);
                    }
                }
            }

            let raw_headers = crate::HeaderCapture::from_headers("transportResponse", resp.headers());
            let request_raw_headers = resp.request_headers.clone();
            let response_headers = raw_headers.text_headers();

            let mut traced_response = None;
            let mut resp = Some(resp);
            if let Some(trace) = trace {
                let mut response = Response { url: current_url.clone(), status: status.as_u16(),
                    headers: response_headers.clone(), body: Vec::new(), redirected_from: redirects.clone(),
                    raw_headers: Some(raw_headers.clone()), request_raw_headers: Some(request_raw_headers.clone()),
                    request_referrer: request.referrer.clone() };
                trace.response(&response, false);
                response.body = read_stealth_body_limited(resp.take().unwrap(), &current_url, request.max_response_bytes).await?;
                traced_response = Some(response);
            }

            if let Err(error) = cors_result {
                if let (Some(trace), Some(response)) = (trace, traced_response.as_ref()) {
                    let message = error.to_string();
                    trace.response_with_error(response, true, Some(&message));
                }
                return Err(error);
            }

            if status.is_redirection() {
                if let Some(location) = raw_headers.fields.iter().find(|field| field.name.eq_ignore_ascii_case(b"location")) {
                    let redirect_result = (|| {
                        let location_str = std::str::from_utf8(&location.value).map_err(|_| {
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
                        if hop == 20 {
                            return Err(ObscuraNetError::TooManyRedirects(url.to_string()));
                        }
                        Ok(next_url)
                    })();
                    let next_url = match redirect_result {
                        Ok(next_url) => next_url,
                        Err(error) => {
                            if let (Some(trace), Some(response)) = (trace, traced_response.as_ref()) {
                                let message = error.to_string();
                                trace.response_with_error(response, true, Some(&message));
                            }
                            return Err(error);
                        }
                    };
                    if let (Some(trace), Some(response)) = (trace, traced_response.as_ref()) {
                        trace.response(response, true);
                    }
                    redirect_tainted |=
                        redirect_taints_origin(&request, &current_url, &next_url);
                    redirects.push(current_url.clone());
                    if let Some(policy) = raw_headers.fields.iter().filter(|field| field.name.eq_ignore_ascii_case(b"referrer-policy"))
                        .filter_map(|field| std::str::from_utf8(&field.value).ok().and_then(ReferrerPolicy::from_header)).last()
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

            if let (Some(trace), Some(response)) = (trace, traced_response.as_ref()) {
                trace.response(response, true);
            }
            let body = match traced_response {
                Some(response) => response.body,
                None => read_stealth_body_limited(resp.unwrap(), &current_url, request.max_response_bytes).await?,
            };

            let response = Response {
                url: current_url,
                status: status.as_u16(),
                headers: response_headers,
                body,
                redirected_from: redirects,
                raw_headers: Some(raw_headers),
                request_raw_headers: Some(request_raw_headers),
                request_referrer: request.referrer,
            };
            if let Some(callbacks) = callbacks {
                request_info.body = request_body.clone();
                callbacks.fire_response(&request_info, &response).await;
                request_info.body = Vec::new();
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
        self.send_single_with_limit(method, url, headers, body, send_cookies, store_cookies, 64 * 1024 * 1024, std::time::Duration::from_secs(30)).await
    }

    /// Scripted fetch uses its configured streaming body limit on every hop.
    pub async fn send_single_with_limit(
        &self,
        method: &str,
        url: &Url,
        headers: &HashMap<String, String>,
        body: &[u8],
        send_cookies: bool,
        store_cookies: bool,
        max_response_bytes: usize,
        timeout: std::time::Duration,
    ) -> Result<Response, ObscuraNetError> {
        self.send_single_observed(method, url, headers, body, send_cookies, store_cookies, max_response_bytes, timeout, None).await
    }

    pub async fn send_single_observed(
        &self, method: &str, url: &Url, headers: &HashMap<String, String>, body: &[u8],
        send_cookies: bool, store_cookies: bool, max_response_bytes: usize,
        timeout: std::time::Duration,
        observation: Option<(&CallbackRegistry, crate::client::ResourceType)>,
    ) -> Result<Response, ObscuraNetError> {
        let fields: Vec<_> = headers.iter().map(|(name, value)| (name.clone(), value.clone())).collect();
        self.send_single_observed_fields(method, url, &fields, body, send_cookies, store_cookies,
            max_response_bytes, timeout, observation).await
    }

    /// Ordered request overrides. Repeated values survive case-insensitive
    /// context/Page overlay; HTTP names/order normalize at the primp boundary.
    pub async fn send_single_observed_fields(
        &self, method: &str, url: &Url, fields: &[(String, String)], body: &[u8],
        send_cookies: bool, store_cookies: bool, max_response_bytes: usize,
        timeout: std::time::Duration,
        observation: Option<(&CallbackRegistry, crate::client::ResourceType)>,
    ) -> Result<Response, ObscuraNetError> {
        self.send_single_traced_fields(method, url, fields, body, send_cookies, store_cookies,
            max_response_bytes, timeout, observation, None).await
    }

    pub async fn send_single_traced_fields(
        &self, method: &str, url: &Url, fields: &[(String, String)], body: &[u8],
        send_cookies: bool, store_cookies: bool, max_response_bytes: usize,
        timeout: std::time::Duration,
        observation: Option<(&CallbackRegistry, crate::client::ResourceType)>,
        trace: Option<&RequestTrace>,
    ) -> Result<Response, ObscuraNetError> {
        let in_flight = InFlightGuard::new(&self.in_flight);
        if let Some(host) = url.host_str() {
            if self.block_trackers() && crate::blocklist::is_blocked(host) {
                tracing::debug!("Blocked tracker: {}", url);
                return Ok(Response {
                    status: 0,
                    url: url.clone(),
                    headers: HashMap::new(),
                    body: Vec::new(),
                    redirected_from: Vec::new(),
                    raw_headers: None,
                    request_raw_headers: None,
                    request_referrer: None,
                });
            }
        }

        let mut request_headers = self.request_headers().await;
        request_headers.retain(|name, _| !fields.iter().any(|(key, _)| key.eq_ignore_ascii_case(name)));
        let mut request_fields: Vec<_> = request_headers.into_iter().collect();
        request_fields.extend_from_slice(fields);
        let mut info = RequestInfo {raw_headers: None, body: Vec::new(), url: url.clone(), method: method.to_string(),
            headers: request_fields.iter().cloned().collect(), resource_type: crate::client::ResourceType::Fetch};
        if let Some(response) = self.intercept(&mut info, Some(body), Some(&mut request_fields)).await? { return Ok(response); }

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
        for (k, v) in &request_fields {
            header(&mut headers, k, v)?;
        }
        let request_referrer = request_fields.iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("referer"))
            .and_then(|(_, value)| Url::parse(value).ok());
        let (transport, prepared) = self.client.request(req_method, url, headers, body, timeout)?;
        if let Some(trace) = trace {
            trace.prepared(crate::HeaderCapture::from_headers("transportRequest", prepared.headers()), body)
                ?;
        }
        if let Some((callbacks, resource_type)) = observation {
            if callbacks.has_request_callbacks().await {
                info.raw_headers = Some(crate::HeaderCapture::from_headers("transportRequest", prepared.headers()));
                info.headers = info.raw_headers.as_ref().unwrap().text_headers();
                info.resource_type = resource_type;
                info.body = body.to_vec();
                callbacks.fire_request(&info).await;
            }
        }
        drop(info);
        let resp = self.client.send_prepared(transport, prepared).await?;

        let status = resp.status();
        if store_cookies {
            for val in resp.headers().get_all("set-cookie") {
                if let Ok(s) = val.to_str() {
                    self.cookie_jar.set_cookie(s, url);
                }
            }
        }
        let raw_headers = crate::HeaderCapture::from_headers("transportResponse", resp.headers());
        let request_raw_headers = resp.request_headers.clone();
        let response_headers = raw_headers.text_headers();
        if let Some(trace) = trace {
            trace.response(&Response { url: url.clone(), status: status.as_u16(),
                headers: response_headers.clone(), body: Vec::new(), redirected_from: Vec::new(),
                raw_headers: Some(raw_headers.clone()), request_raw_headers: Some(request_raw_headers.clone()),
                request_referrer: request_referrer.clone() }, false);
        }
        let resp_body = read_stealth_body_limited(resp, url, max_response_bytes).await?;
        drop(in_flight);

        Ok(Response {
            url: url.clone(),
            status: status.as_u16(),
            headers: response_headers,
            body: resp_body,
            redirected_from: Vec::new(),
            raw_headers: Some(raw_headers),
            request_raw_headers: Some(request_raw_headers),
            request_referrer,
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

#[cfg(test)]
mod tests {
    struct LifecycleBoundaryObserver {
        started: std::sync::Arc<std::sync::atomic::AtomicBool>,
        terminals: std::sync::Arc<std::sync::Mutex<Vec<(String, Option<String>)>>>,
    }

    impl crate::observation::RequestLifecycleObserver for LifecycleBoundaryObserver {
        fn started(
            &self,
            request_id: &str,
            _: crate::ResourceType,
            _: usize,
            exchange: &crate::observation::Exchange,
        ) -> Result<(), crate::ObscuraNetError> {
            assert!(exchange.request_headers.is_some(), "prepared raw headers precede admission");
            assert_eq!(request_id, "ordinary-boundary");
            self.started.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }

        fn terminal(
            &self,
            request_id: &str,
            _: crate::ResourceType,
            _: usize,
            _: &crate::observation::Exchange,
            _: Option<&[u8]>,
            error: Option<&str>,
        ) {
            self.terminals.lock().unwrap().push((
                request_id.to_string(), error.map(str::to_string),
            ));
        }
    }

    #[tokio::test]
    async fn traced_ordinary_start_is_accepted_before_transport_send() {
        use std::io::{Read as _, Write as _};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let proxy = format!("http://{}", listener.local_addr().unwrap());
        let started = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let observed_at_wire = started.clone();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0u8; 2048];
            let _ = socket.read(&mut request).unwrap();
            assert!(observed_at_wire.load(std::sync::atomic::Ordering::SeqCst),
                "request bytes reached the peer before Started admission");
            socket.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/css\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
            ).unwrap();
        });
        let terminals = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let observer = std::sync::Arc::new(LifecycleBoundaryObserver {
            started: started.clone(),
            terminals: terminals.clone(),
        });
        let policy = std::sync::Arc::new(crate::client::ObscuraHttpClient::new());
        let client = super::StealthHttpClient::with_policy(
            std::sync::Arc::new(crate::cookies::CookieJar::new()),
            Some(&proxy),
            policy,
            &default_persona(),
        );
        let url = url::Url::parse("http://lifecycle.test/style.css").unwrap();
        let trace = crate::observation::RequestTrace::new(
            std::sync::Arc::new(std::sync::Mutex::new(Default::default())),
            std::sync::Arc::new(std::sync::Mutex::new(Default::default())),
            "ordinary-boundary".into(),
        ).observe(observer, crate::ResourceType::Stylesheet);
        trace.begin(url.as_str(), "GET", None, None).unwrap();
        client.fetch_resource_traced(
            &url,
            crate::ResourceRequest::subresource(crate::ResourceType::Stylesheet, &url),
            None,
            &trace,
        ).await.unwrap();
        server.join().unwrap();

        assert!(started.load(std::sync::atomic::Ordering::SeqCst));
        assert_eq!(terminals.lock().unwrap().as_slice(), [
            ("ordinary-boundary".to_string(), None),
        ]);
    }

    #[derive(Debug)]
    struct RedirectTerminal {
        hop: usize,
        status: u16,
        raw_body: Vec<u8>,
        error: Option<String>,
    }

    struct RedirectLifecycleObserver {
        starts: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        terminals: std::sync::Arc<std::sync::Mutex<Vec<RedirectTerminal>>>,
    }

    impl crate::observation::RequestLifecycleObserver for RedirectLifecycleObserver {
        fn started(
            &self,
            _: &str,
            _: crate::ResourceType,
            _: usize,
            _: &crate::observation::Exchange,
        ) -> Result<(), crate::ObscuraNetError> {
            self.starts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }

        fn terminal(
            &self,
            _: &str,
            _: crate::ResourceType,
            hop: usize,
            exchange: &crate::observation::Exchange,
            body: Option<&[u8]>,
            error: Option<&str>,
        ) {
            self.terminals.lock().unwrap().push(RedirectTerminal {
                hop,
                status: exchange.response.as_ref().map_or(0, |response| response.status),
                raw_body: body.unwrap_or_default().to_vec(),
                error: error.map(str::to_string),
            });
        }
    }

    async fn traced_redirect_failure(location: &str) -> (
        crate::ObscuraNetError,
        usize,
        Vec<RedirectTerminal>,
    ) {
        use std::io::{Read as _, Write as _};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let proxy = format!("http://{}", listener.local_addr().unwrap());
        let location = location.to_string();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0u8; 2048];
            let _ = socket.read(&mut request).unwrap();
            let response = format!(
                "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 4\r\nConnection: close\r\n\r\nbody",
            );
            socket.write_all(response.as_bytes()).unwrap();
        });
        let starts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let terminals = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let observer = std::sync::Arc::new(RedirectLifecycleObserver {
            starts: starts.clone(),
            terminals: terminals.clone(),
        });
        let policy = std::sync::Arc::new(crate::client::ObscuraHttpClient::new());
        let client = super::StealthHttpClient::with_policy(
            std::sync::Arc::new(crate::cookies::CookieJar::new()),
            Some(&proxy),
            policy,
            &default_persona(),
        );
        let url = url::Url::parse("http://redirect.test/style.css").unwrap();
        let trace = crate::observation::RequestTrace::new(
            std::sync::Arc::new(std::sync::Mutex::new(Default::default())),
            std::sync::Arc::new(std::sync::Mutex::new(Default::default())),
            "redirect-boundary".into(),
        ).observe(observer, crate::ResourceType::Stylesheet);
        trace.begin(url.as_str(), "GET", None, None).unwrap();
        let error = client.fetch_resource_traced(
            &url,
            crate::ResourceRequest::subresource(crate::ResourceType::Stylesheet, &url),
            None,
            &trace,
        ).await.unwrap_err();
        server.join().unwrap();
        let count = starts.load(std::sync::atomic::Ordering::SeqCst);
        let captured = std::mem::take(&mut *terminals.lock().unwrap());
        (error, count, captured)
    }

    #[tokio::test]
    async fn invalid_redirect_is_one_failed_terminal_with_complete_response() {
        let (error, starts, terminals) = traced_redirect_failure("http://[::1").await;
        assert!(error.to_string().contains("Invalid redirect URL"));
        assert_eq!(starts, 1);
        assert_eq!(terminals.len(), 1);
        assert_eq!(terminals[0].hop, 0);
        assert_eq!(terminals[0].status, 302);
        assert_eq!(terminals[0].raw_body, b"body");
        assert!(terminals[0].error.as_deref()
            .is_some_and(|error| error.contains("Invalid redirect URL")));
    }

    #[tokio::test]
    async fn blocked_redirect_is_one_failed_terminal_with_complete_response() {
        let (error, starts, terminals) =
            traced_redirect_failure("http://127.0.0.1/private").await;
        assert!(error.to_string().contains("private/internal IP"));
        assert_eq!(starts, 1);
        assert_eq!(terminals.len(), 1);
        assert_eq!(terminals[0].status, 302);
        assert_eq!(terminals[0].raw_body, b"body");
        assert!(terminals[0].error.is_some());
    }

    #[tokio::test]
    async fn redirect_limit_fails_only_the_last_started_hop() {
        use std::io::{Read as _, Write as _};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let proxy = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            for hop in 0..=20 {
                let (mut socket, _) = listener.accept().unwrap();
                let mut request = [0u8; 2048];
                let _ = socket.read(&mut request).unwrap();
                let body = format!("hop-{hop}");
                let response = format!(
                    "HTTP/1.1 302 Found\r\nLocation: /loop\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len(),
                );
                socket.write_all(response.as_bytes()).unwrap();
            }
        });
        let starts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let terminals = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let observer = std::sync::Arc::new(RedirectLifecycleObserver {
            starts: starts.clone(),
            terminals: terminals.clone(),
        });
        let client = super::StealthHttpClient::with_policy(
            std::sync::Arc::new(crate::cookies::CookieJar::new()),
            Some(&proxy),
            std::sync::Arc::new(crate::client::ObscuraHttpClient::new()),
            &default_persona(),
        );
        let url = url::Url::parse("http://redirect.test/loop").unwrap();
        let trace = crate::observation::RequestTrace::new(
            std::sync::Arc::new(std::sync::Mutex::new(Default::default())),
            std::sync::Arc::new(std::sync::Mutex::new(Default::default())),
            "redirect-limit".into(),
        ).observe(observer, crate::ResourceType::Stylesheet);
        trace.begin(url.as_str(), "GET", None, None).unwrap();
        let error = client.fetch_resource_traced(
            &url,
            crate::ResourceRequest::subresource(crate::ResourceType::Stylesheet, &url),
            None,
            &trace,
        ).await.unwrap_err();
        server.join().unwrap();

        assert!(matches!(error, crate::ObscuraNetError::TooManyRedirects(_)));
        assert_eq!(starts.load(std::sync::atomic::Ordering::SeqCst), 21);
        let terminals = terminals.lock().unwrap();
        assert_eq!(terminals.len(), 21);
        assert!(terminals[..20].iter().all(|terminal| terminal.error.is_none()));
        assert!(terminals[20].error.as_deref()
            .is_some_and(|error| error.contains("Too many redirects")));
        assert_eq!(terminals[20].raw_body, b"hop-20");
    }

    fn default_persona() -> crate::EffectivePersona {
        crate::EffectivePersona::builtin(super::StealthProfile::WindowsChrome145)
    }

    fn persona(
        profile: super::StealthProfile,
        accept_language: &str,
        do_not_track: Option<&str>,
    ) -> crate::EffectivePersona {
        let mut spec = crate::PersonaSpec::preset(profile);
        let language = accept_language.split(',').next().unwrap().to_string();
        spec.language = Some(language.clone());
        spec.languages = Some(vec![language]);
        spec.accept_language = Some(accept_language.to_string());
        spec.do_not_track = do_not_track.map(str::to_string);
        spec.compile().unwrap()
    }

    /// A detached client exists so a worker on its own tokio runtime does not
    /// reuse the page's connection pool: a pooled connection is driven by the
    /// runtime that created it, and the first reuse from another runtime fails
    /// with a broken pipe. Detaching must therefore keep every *shared* piece of
    /// identity (cookies, in-flight accounting, header overrides) and the whole
    /// transport configuration (profile and proxy), or scraping silently loses
    /// the proxy.
    #[test]
    fn detached_client_shares_identity_and_keeps_transport_configuration() {
        use std::sync::Arc as StdArc;
        let policy = StdArc::new(crate::client::ObscuraHttpClient::new());
        let jar = StdArc::new(crate::cookies::CookieJar::new());
        let proxy = "http://127.0.0.1:9";
        let persona = persona(super::StealthProfile::MacChrome153, "en-US,en;q=0.9", Some("0"));
        let original = super::StealthHttpClient::with_policy_persona(
            jar.clone(), Some(proxy), policy, &persona,
        );
        original.extra_headers.blocking_write().insert("x-probe".into(), "1".into());

        let sibling = original.detached();

        // identity stays shared
        assert!(StdArc::ptr_eq(&original.cookie_jar, &sibling.cookie_jar));
        assert!(StdArc::ptr_eq(&original.in_flight, &sibling.in_flight));
        assert!(StdArc::ptr_eq(&original.extra_headers, &sibling.extra_headers),
            "header overrides must reach the worker's requests");
        assert_eq!(sibling.extra_headers.blocking_read().get("x-probe").map(String::as_str), Some("1"));

        // transport configuration is carried over verbatim
        let params = sibling.transport_params();
        assert_eq!(params.profile, super::StealthProfile::MacChrome153);
        assert_eq!(params.proxy_url.as_deref(), Some(proxy));
        assert_eq!(params.accept_language.as_deref(), Some("en-US,en;q=0.9"));
        assert_eq!(params.do_not_track.as_deref(), Some("0"));

        // the sibling has its own transport, not a clone of the same one
        assert_ne!(original.transport_params().profile, super::StealthProfile::WindowsChrome145);
    }

    #[test]
    fn pages_keep_separate_in_flight_counters_while_detached_workers_share() {
        use std::sync::Arc as StdArc;
        let policy = StdArc::new(crate::client::ObscuraHttpClient::new());
        let persona = persona(super::StealthProfile::MacChrome153, "en-US,en;q=0.9", None);
        let first = super::StealthHttpClient::with_policy_persona(
            StdArc::new(crate::cookies::CookieJar::new()),
            None,
            policy.clone(),
            &persona,
        );
        let second = super::StealthHttpClient::with_policy_persona(
            first.cookie_jar.clone(),
            None,
            policy.clone(),
            &persona,
        );
        let worker = first.detached();

        assert!(!StdArc::ptr_eq(&policy.in_flight, &first.in_flight));
        assert!(!StdArc::ptr_eq(&first.in_flight, &second.in_flight));
        assert!(StdArc::ptr_eq(&first.in_flight, &worker.in_flight));
        first
            .in_flight
            .store(3, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(first.active_requests(), 3);
        assert_eq!(worker.active_requests(), 3);
        assert_eq!(second.active_requests(), 0);
        assert_eq!(policy.active_requests(), 0);
    }

    struct CaptureHeaders(std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, String>>>);

    #[async_trait::async_trait]
    impl crate::interceptor::RequestInterceptor for CaptureHeaders {
        async fn intercept(&self, request: &crate::client::RequestInfo) -> crate::interceptor::InterceptAction {
            *self.0.lock().unwrap() = request.headers.clone();
            crate::interceptor::InterceptAction::Block
        }
    }

    #[tokio::test]
    async fn layered_headers_reach_cache_keys_and_interceptors() {
        let policy = std::sync::Arc::new(crate::client::ObscuraHttpClient::new());
        policy.set_extra_headers([("X-Context".into(), "Original Raw".into()), ("X-Shared".into(), "Context".into())].into_iter().collect()).await;
        let client = super::StealthHttpClient::with_policy(
            std::sync::Arc::new(crate::cookies::CookieJar::new()), None, policy.clone(),
            &default_persona(),
        );
        client.set_extra_headers([("x-shared".into(), "Page Raw".into())].into_iter().collect()).await;
        let url = url::Url::parse("https://example.com/image").unwrap();
        let resource = crate::client::ResourceRequest::subresource(crate::client::ResourceType::Image, &url);
        let original = client.resource_cache_key(&url, &resource).await.unwrap();
        policy.set_extra_headers([("X-Context".into(), "Updated Raw".into()), ("X-Shared".into(), "Context".into())].into_iter().collect()).await;
        let updated = client.resource_cache_key(&url, &resource).await.unwrap();
        assert_ne!(original, updated, "dynamic context headers must partition cached responses");
        let captured = std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
        *policy.interceptor.write().await = Some(std::sync::Arc::new(CaptureHeaders(captured.clone())));
        client.fetch_resource_with_callbacks(&url, resource, None).await.expect_err("interceptor blocks before I/O");
        assert_eq!(*captured.lock().unwrap(), [
            ("X-Context".into(), "Updated Raw".into()), ("x-shared".into(), "Page Raw".into()),
        ].into_iter().collect());
        client.send_single("GET", &url, &[("X-SHARED".into(), "Request Raw".into())].into_iter().collect(), &[], false, false)
            .await.expect_err("interceptor blocks before I/O");
        assert_eq!(*captured.lock().unwrap(), [
            ("X-Context".into(), "Updated Raw".into()), ("X-SHARED".into(), "Request Raw".into()),
        ].into_iter().collect());
    }

    struct CaptureOrderedHeaders;

    #[async_trait::async_trait]
    impl crate::interceptor::RequestInterceptor for CaptureOrderedHeaders {
        async fn intercept(&self, request: &RequestInfo) -> crate::interceptor::InterceptAction {
            let raw = request.raw_headers.as_ref().unwrap();
            assert_eq!(raw.capture_stage, "requestPolicy");
            let fields: Vec<_> = raw.fields.iter().filter(|field| field.name.eq_ignore_ascii_case(b"x-test"))
                .map(|field| (field.name.as_slice(), field.value.as_slice())).collect();
            assert_eq!(fields, [(b"X-Test".as_slice(), b"one".as_slice()), (b"x-test".as_slice(), b"two".as_slice())]);
            assert_eq!(request.body, b"complete body");
            crate::interceptor::InterceptAction::ModifyHeaders(HashMap::from([("X-POLICY".into(), "new".into())]))
        }
    }

    #[tokio::test]
    async fn ordered_request_fields_preserve_context_page_and_native_policy_overlays() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let proxy = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut request = Vec::new();
            loop {
                let mut buf = [0; 2048];
                let count = socket.read(&mut buf).unwrap(); assert!(count > 0);
                request.extend_from_slice(&buf[..count]);
                if let Some(end) = request.windows(4).position(|s| s == b"\r\n\r\n") {
                    if request.len() >= end + 4 + b"complete body".len() { break; }
                }
            }
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok").unwrap();
            String::from_utf8(request).unwrap()
        });
        let policy = Arc::new(crate::client::ObscuraHttpClient::new());
        policy.set_extra_headers(HashMap::from([
            ("X-Context".into(), "retained".into()), ("x-test".into(), "context".into()),
        ])).await;
        *policy.interceptor.write().await = Some(Arc::new(CaptureOrderedHeaders));
        let client = StealthHttpClient::with_policy(
            Arc::new(CookieJar::new()), Some(&proxy), policy, &default_persona(),
        );
        client.set_extra_headers(HashMap::from([("X-TEST".into(), "page".into())])).await;
        let fields = [("X-Test", "one"), ("x-test", "two"), ("X-Policy", "old"), ("x-policy", "also-old")]
            .map(|(name, value)| (name.into(), value.into()));
        let response = client.send_single_observed_fields("POST", &Url::parse("http://headers.test/").unwrap(),
            &fields, b"complete body", false, false, 1024, Duration::from_secs(5), None).await.unwrap();
        let raw = response.request_raw_headers.unwrap();
        let values = |name: &[u8]| raw.fields.iter().filter(|field| field.name == name)
            .map(|field| field.value.as_slice()).collect::<Vec<_>>();
        assert_eq!(values(b"x-test"), [b"one".as_slice(), b"two".as_slice()]);
        assert_eq!(values(b"x-context"), [b"retained".as_slice()]);
        assert_eq!(values(b"x-policy"), [b"new".as_slice()]);
        let wire = server.join().unwrap();
        assert!(wire.contains("x-test: one\r\nx-test: two\r\n"));
        assert!(wire.contains("x-context: retained\r\n"));
        assert!(wire.contains("x-policy: new\r\n"));
        assert!(!wire.contains("old"));
    }

    struct CaptureBody(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    #[async_trait::async_trait]
    impl crate::interceptor::RequestInterceptor for CaptureBody {
        async fn intercept(
            &self,
            request: &crate::client::RequestInfo,
        ) -> crate::interceptor::InterceptAction {
            *self.0.lock().unwrap() = request.body.clone();
            crate::interceptor::InterceptAction::Block
        }
    }

    #[tokio::test]
    async fn scripted_request_interceptor_observes_the_complete_body() {
        let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let policy = std::sync::Arc::new(crate::client::ObscuraHttpClient::new());
        *policy.interceptor.write().await = Some(std::sync::Arc::new(CaptureBody(captured.clone())));
        let client = super::StealthHttpClient::with_policy(
            std::sync::Arc::new(crate::cookies::CookieJar::new()),
            None,
            policy,
            &default_persona(),
        );
        let payload = b"complete raw request body\0\x80\xff";

        client
            .send_single(
                "POST",
                &url::Url::parse("https://example.com/collect").unwrap(),
                &std::collections::HashMap::new(),
                payload,
                false,
                false,
            )
            .await
            .expect_err("capture interceptor blocks before network I/O");

        assert_eq!(captured.lock().unwrap().as_slice(), payload);
    }

    struct PausedInterceptor {
        entered: tokio::sync::Notify,
        release: tokio::sync::Notify,
    }

    #[async_trait::async_trait]
    impl crate::interceptor::RequestInterceptor for PausedInterceptor {
        async fn intercept(
            &self,
            _request: &crate::client::RequestInfo,
        ) -> crate::interceptor::InterceptAction {
            self.entered.notify_one();
            self.release.notified().await;
            crate::interceptor::InterceptAction::Block
        }
    }

    #[tokio::test]
    async fn native_interception_is_counted_until_completion_or_cancellation() {
        for scripted in [false, true] {
            for cancel in [false, true] {
                let interceptor = Arc::new(PausedInterceptor {
                    entered: tokio::sync::Notify::new(),
                    release: tokio::sync::Notify::new(),
                });
                let policy = Arc::new(crate::client::ObscuraHttpClient::new());
                *policy.interceptor.write().await = Some(interceptor.clone());
                let client = StealthHttpClient::with_policy(
                    Arc::new(CookieJar::new()), None, policy, &default_persona(),
                );
                let url = Url::parse("https://example.com/paused").unwrap();
                let mut request = Box::pin(async {
                    if scripted {
                        client.send_single("POST", &url, &std::collections::HashMap::new(), b"raw\0\x80\xff", false, false).await
                    } else {
                        client.fetch(&url).await
                    }
                });
                tokio::select! {
                    _ = interceptor.entered.notified() => {}
                    result = &mut request => panic!("request completed before release: {result:?}"),
                    _ = tokio::time::sleep(Duration::from_secs(5)) => panic!("interceptor was not reached"),
                }
                assert_eq!(client.active_requests(), 1, "native interception must keep networkidle pending");
                if cancel {
                    drop(request);
                } else {
                    interceptor.release.notify_one();
                    assert!(matches!(request.await, Err(crate::client::ObscuraNetError::Blocked(_))));
                }
                assert_eq!(client.active_requests(), 0, "completed or cancelled requests must release their guard");
            }
        }
    }

    #[tokio::test]
    async fn interceptor_body_copy_is_released_after_observation() {
        let captured = Arc::new(std::sync::Mutex::new(Vec::new()));
        let policy = Arc::new(crate::client::ObscuraHttpClient::new());
        *policy.interceptor.write().await = Some(Arc::new(CaptureBody(captured.clone())));
        let client = StealthHttpClient::with_policy(
            Arc::new(CookieJar::new()), None, policy, &default_persona(),
        );
        let payload = b"raw\0\x80\xff";
        let mut info = crate::client::RequestInfo {
            raw_headers: None,
            url: Url::parse("https://example.com/body").unwrap(),
            method: "POST".into(),
            headers: std::collections::HashMap::new(),
            resource_type: crate::client::ResourceType::Fetch,
            body: Vec::new(),
        };
        assert!(client.intercept(&mut info, Some(payload), None).await.is_err());
        assert_eq!(captured.lock().unwrap().as_slice(), payload);
        assert!(info.body.is_empty());
        assert_eq!(info.body.capacity(), 0, "the temporary observation copy must release its allocation");
    }

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
        let client = StealthHttpClient::with_proxy(
            Arc::new(CookieJar::new()), None, true, &default_persona(),
        );
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
    async fn send_single_defaults_to_wildcard_accept_and_preserves_explicit() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                let mut bytes = Vec::new();
                loop {
                    let mut buf = [0; 4096];
                    let n = stream.read(&mut buf).unwrap();
                    assert!(n > 0);
                    bytes.extend_from_slice(&buf[..n]);
                    if bytes.windows(4).any(|v| v == b"\r\n\r\n") { break; }
                }
                stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok").unwrap();
                requests.push(String::from_utf8(bytes).unwrap().to_ascii_lowercase());
            }
            requests
        });
        let url = Url::parse(&format!("http://{address}/data")).unwrap();
        let client = StealthHttpClient::with_proxy(
            Arc::new(CookieJar::new()), None, true, &default_persona(),
        );

        // First request: no explicit accept header. Must send `accept: */*`.
        let empty_headers = std::collections::HashMap::new();
        client.send_single("GET", &url, &empty_headers, &[], false, false).await.unwrap();

        // Second request: explicit uppercase Accept header. Must keep it without duplicates.
        let mut custom_headers = std::collections::HashMap::new();
        custom_headers.insert("Accept".to_string(), "application/xml".to_string());
        client.send_single("GET", &url, &custom_headers, &[], false, false).await.unwrap();

        let requests = server.join().unwrap();
        assert_eq!(requests[0].matches("accept:").count(), 1, "{}", requests[0]);
        assert!(requests[0].contains("accept: */*\r\n"), "{}", requests[0]);

        assert_eq!(requests[1].matches("accept:").count(), 1, "{}", requests[1]);
        assert!(requests[1].contains("accept: application/xml\r\n"), "{}", requests[1]);
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
            let client = StealthHttpClient::with_proxy(
                Arc::new(CookieJar::new()), None, true, &default_persona(),
            );
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
    use crate::client::{ObscuraNetError, RequestInfo, SsrfGuardResolver};
    use std::collections::HashMap;
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
    async fn all_profiles_send_consistent_identity_without_prefetch_defaults() {
        for (profile, platform, brands) in [
            (super::StealthProfile::WindowsChrome145, "Windows", r#""Not:A-Brand";v="99", "Google Chrome";v="145", "Chromium";v="145""#),
            (super::StealthProfile::MacChrome152, "macOS", r#""Chromium";v="152", "Not?A_Brand";v="24", "Google Chrome";v="152""#),
            // Chrome 153 changed both the GREASE brand name/version and the brand
            // order versus 152. These goldens come from primp's own captures of
            // the real builds, so a profile that drifts from them fails loudly.
            (super::StealthProfile::MacChrome153, "macOS", r#""Google Chrome";v="153", "Not_A Brand";v="8", "Chromium";v="153""#),
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
        let guarded = StealthHttpClient::with_policy(
            jar, Some(&proxy), policy,
            &crate::EffectivePersona::builtin(super::StealthProfile::MacChrome152),
        );
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
            client: super::transport::Client::new(
                super::StealthProfile::WindowsChrome145,
                None,
                true,
                None,
                None,
            ),
            allow_private_network: true,
            cookie_jar: Arc::new(CookieJar::new()),
            extra_headers: std::sync::Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            in_flight: Arc::new(std::sync::atomic::AtomicU32::new(0)),
            resource_loader: Arc::new(std::sync::Mutex::new(crate::client::ResourceLoaderState::default())),
            policy: None,
            transport: super::TransportParams {
                profile: super::StealthProfile::WindowsChrome145,
                proxy_url: None,
                accept_language: None,
                do_not_track: None,
            },
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
        let client = StealthHttpClient::with_policy(cookie_jar, None, policy, &default_persona());
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
        let client = StealthHttpClient::with_proxy(
            Arc::new(CookieJar::new()), None, true, &default_persona(),
        );
        let url = Url::parse(&format!("http://localhost:{port}/")).unwrap();

        let resp = client
            .fetch(&url)
            .await
            .expect("loopback hostname must be reachable with the opt-in");
        assert_eq!(resp.status, 200);
    }
}

#[cfg(test)]
#[path = "network_tests.rs"]
mod network_tests;
