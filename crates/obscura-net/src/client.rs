use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use std::net::{IpAddr, Ipv4Addr};

use tokio::sync::{RwLock, watch};
use url::Url;

use crate::cookies::CookieJar;
use crate::interceptor::RequestInterceptor;

pub(crate) fn configured_root_paths() -> Vec<std::path::PathBuf> {
    let mut paths = Vec::new();
    if let Some(path) = std::env::var_os("SSL_CERT_FILE").filter(|path| !path.is_empty()) {
        paths.push(path.into());
    }
    if let Some(directory) = std::env::var_os("SSL_CERT_DIR").filter(|path| !path.is_empty()) {
        match std::fs::read_dir(directory) {
            Ok(entries) => {
                paths.extend(entries.filter_map(|entry| entry.ok().map(|entry| entry.path())));
            }
            Err(error) => {
                tracing::warn!(%error, "failed to read SSL_CERT_DIR");
            }
        }
    }
    paths.sort();
    paths
}

#[derive(Debug, Clone)]
pub struct Response {
    pub url: Url,
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
    pub redirected_from: Vec<Url>,
    /// Computed referrer of the final request, independent of response headers.
    pub request_referrer: Option<Url>,
}

impl Response {
    /// Decode the body as text, honoring the response charset.
    ///
    /// Uses the HTTP `Content-Type` header's `charset=` parameter, then for
    /// HTML responses falls back to sniffing `<meta charset>` in the first
    /// 1KB, then UTF-8. Mirrors browser behaviour per the HTML5 spec.
    pub fn text(&self) -> String {
        if self.is_html() {
            crate::encoding::decode_response(&self.body, self.content_type())
        } else {
            crate::encoding::decode_non_html(&self.body, self.content_type())
        }
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(&name.to_lowercase()).map(|s| s.as_str())
    }

    pub fn content_type(&self) -> Option<&str> {
        self.header("content-type")
    }

    pub fn is_html(&self) -> bool {
        self.content_type()
            .map(|ct| ct.contains("text/html"))
            .unwrap_or(false)
    }
}

/// Fold one response header line into the collected header map.
///
/// A plain `HashMap` insert keeps only the *last* value when a response repeats
/// a header name (`Link`, `Via`, `WWW-Authenticate`, ...), silently dropping the
/// earlier lines. Per RFC 9110 §5.3 duplicate field lines of the same name may
/// be combined into one comma-separated value without changing semantics.
/// `Set-Cookie` is the exception (RFC 6265 forbids folding it): it is captured
/// individually by the cookie jar via `get_all`, so the map keeps it only as a
/// presence signal and last-wins there is fine.
pub(crate) fn merge_response_header(map: &mut HashMap<String, String>, name: String, value: String) {
    if name == "set-cookie" {
        map.insert(name, value);
        return;
    }
    map.entry(name)
        .and_modify(|existing| {
            existing.push_str(", ");
            existing.push_str(&value);
        })
        .or_insert(value);
}

#[derive(Debug, Clone)]
pub struct RequestInfo {
    /// Request payload, before redirects rewrite the method.
    pub body: Vec<u8>,
    pub url: Url,
    pub method: String,
    pub headers: HashMap<String, String>,
    pub resource_type: ResourceType,
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub enum ResourceType {
    Document,
    Script,
    Stylesheet,
    Image,
    Font,
    Xhr,
    Fetch,
    Other,
}

/// Fetch metadata for a browser-owned request. Navigation keeps its existing
/// profile; render resources use this type so they do not masquerade as HTML
/// documents when they move onto the page's asynchronous transport.
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub enum RequestMode {
    Navigate,
    NoCors,
    Cors,
    SameOrigin,
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub enum RequestCredentials {
    Omit,
    SameOrigin,
    Include,
}

impl RequestMode {
    pub(crate) fn header_value(self) -> &'static str {
        match self {
            Self::Navigate => "navigate",
            Self::NoCors => "no-cors",
            Self::Cors => "cors",
            Self::SameOrigin => "same-origin",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum ReferrerPolicy {
    NoReferrer,
    NoReferrerWhenDowngrade,
    SameOrigin,
    Origin,
    StrictOrigin,
    OriginWhenCrossOrigin,
    #[default]
    StrictOriginWhenCrossOrigin,
    UnsafeUrl,
}

impl ReferrerPolicy {
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "no-referrer" => Self::NoReferrer,
            "no-referrer-when-downgrade" => Self::NoReferrerWhenDowngrade,
            "same-origin" => Self::SameOrigin,
            "origin" => Self::Origin,
            "strict-origin" => Self::StrictOrigin,
            "origin-when-cross-origin" => Self::OriginWhenCrossOrigin,
            "strict-origin-when-cross-origin" => Self::StrictOriginWhenCrossOrigin,
            "unsafe-url" => Self::UnsafeUrl,
            _ => return None,
        })
    }

    pub fn from_header(value: &str) -> Option<Self> {
        value.split(',').filter_map(|token| Self::parse(token.trim())).last()
    }

    /// The returned URL becomes the request's source for its next redirect hop.
    pub fn referrer(self, source: Option<&Url>, target: &Url) -> Option<Url> {
        let source = source?;
        if self == Self::NoReferrer
            || !matches!(source.scheme(), "http" | "https")
            || !matches!(target.scheme(), "http" | "https")
        {
            return None;
        }
        let mut full = source.clone();
        let _ = full.set_username("");
        let _ = full.set_password(None);
        full.set_fragment(None);
        let origin = Url::parse(&format!("{}/", full.origin().ascii_serialization())).ok()?;
        if full.as_str().len() > 4096 {
            full = origin.clone();
        }
        let same_origin = source.origin() == target.origin();
        let downgrade = potentially_trustworthy(source) && !potentially_trustworthy(target);
        match self {
            Self::NoReferrer => None,
            Self::NoReferrerWhenDowngrade => (!downgrade).then_some(full),
            Self::SameOrigin => same_origin.then_some(full),
            Self::Origin => Some(origin),
            Self::StrictOrigin => (!downgrade).then_some(origin),
            Self::OriginWhenCrossOrigin => Some(if same_origin { full } else { origin }),
            Self::StrictOriginWhenCrossOrigin => {
                if same_origin { Some(full) } else { (!downgrade).then_some(origin) }
            }
            Self::UnsafeUrl => Some(full),
        }
    }
}

fn potentially_trustworthy(url: &Url) -> bool {
    url.scheme() == "https" || match url.host() {
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        Some(url::Host::Domain(host)) => {
            let host = host.trim_end_matches('.');
            host == "localhost" || host.ends_with(".localhost")
        }
        None => false,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceRequest {
    pub resource_type: ResourceType,
    /// Origin-bearing environment that owns the request. This controls CORS,
    /// credentials, and Sec-Fetch-Site and must remain the document/realm for
    /// every descendant in a module graph.
    pub initiator: Option<Url>,
    /// URL used to derive the Referer header. Usually the same as `initiator`,
    /// but a module dependency is referred by its importing module while its
    /// credentials mode is still relative to the owning document.
    pub referrer: Option<Url>,
    pub referrer_policy: ReferrerPolicy,
    pub mode: RequestMode,
    pub credentials: RequestCredentials,
    /// Hard limit for the decoded response body retained by this request.
    /// Callers can lower it for especially constrained resource consumers.
    pub max_response_bytes: usize,
}

impl ResourceRequest {
    pub fn navigation() -> Self {
        Self {
            resource_type: ResourceType::Document,
            initiator: None,
            referrer: None,
            referrer_policy: ReferrerPolicy::default(),
            mode: RequestMode::Navigate,
            credentials: RequestCredentials::Include,
            max_response_bytes: 64 * 1024 * 1024,
        }
    }

    pub fn subresource(resource_type: ResourceType, initiator: &Url) -> Self {
        let mode = match resource_type {
            ResourceType::Font | ResourceType::Xhr | ResourceType::Fetch => RequestMode::Cors,
            ResourceType::Document => RequestMode::Navigate,
            ResourceType::Script
            | ResourceType::Stylesheet
            | ResourceType::Image
            | ResourceType::Other => RequestMode::NoCors,
        };
        let credentials = match resource_type {
            ResourceType::Document
            | ResourceType::Script
            | ResourceType::Stylesheet
            | ResourceType::Image
            | ResourceType::Other => RequestCredentials::Include,
            ResourceType::Font | ResourceType::Xhr | ResourceType::Fetch => {
                RequestCredentials::SameOrigin
            }
        };
        Self {
            resource_type,
            initiator: Some(initiator.clone()),
            referrer: Some(initiator.clone()),
            referrer_policy: ReferrerPolicy::default(),
            mode,
            credentials,
            max_response_bytes: match resource_type {
                ResourceType::Stylesheet | ResourceType::Font => 16 * 1024 * 1024,
                ResourceType::Script | ResourceType::Other => 32 * 1024 * 1024,
                ResourceType::Document
                | ResourceType::Image
                | ResourceType::Xhr
                | ResourceType::Fetch => 64 * 1024 * 1024,
            },
        }
    }

    /// Fetch profile for JavaScript modules. Unlike classic scripts, module
    /// scripts are CORS-enabled and use `same-origin` credentials by default.
    /// Keep this separate from `subresource(Script, ..)`, whose no-CORS,
    /// include-credentials profile is still correct for classic scripts.
    pub fn module_script(initiator: &Url, referrer: &Url) -> Self {
        Self {
            resource_type: ResourceType::Script,
            initiator: Some(initiator.clone()),
            referrer: Some(referrer.clone()),
            referrer_policy: ReferrerPolicy::default(),
            mode: RequestMode::Cors,
            credentials: RequestCredentials::SameOrigin,
            // OBSCURA_FETCH_MAX_BODY_BYTES (the fetch()/XHR override from #581)
            // also raises this cap: a large SPA bundle otherwise dies silently
            // at 32 MiB while fetch() of the same URL succeeds (#849).
            max_response_bytes: std::env::var("OBSCURA_FETCH_MAX_BODY_BYTES")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(32 * 1024 * 1024),
        }
    }

    pub fn with_max_response_bytes(mut self, max_response_bytes: usize) -> Self {
        self.max_response_bytes = max_response_bytes;
        self
    }

    /// Browser-owned Fetch Metadata for this resource and current redirect hop.
    pub fn fetch_metadata_headers(&self, target: &Url) -> [(&'static str, &'static str); 3] {
        [
            ("sec-fetch-mode", self.mode.header_value()),
            ("sec-fetch-site", request_fetch_site(self, target)),
            ("sec-fetch-dest", self.destination()),
        ]
    }

    pub(crate) fn destination(&self) -> &'static str {
        match self.resource_type {
            ResourceType::Document => "document",
            ResourceType::Script => "script",
            ResourceType::Stylesheet => "style",
            ResourceType::Image => "image",
            ResourceType::Font => "font",
            ResourceType::Xhr | ResourceType::Fetch | ResourceType::Other => "empty",
        }
    }

    pub(crate) fn priority(&self) -> &'static str {
        match self.resource_type {
            ResourceType::Xhr | ResourceType::Fetch => "u=1, i",
            _ => "u=0, i",
        }
    }

    pub(crate) fn accept(&self) -> &'static str {
        match self.resource_type {
            ResourceType::Document => "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,*/*;q=0.8,application/signed-exchange;v=b3;q=0.7",
            ResourceType::Stylesheet => "text/css,*/*;q=0.1",
            // AVIF is intentionally omitted until obscura's decoder can paint
            // it. Advertising a format and then discarding the selected body
            // is less faithful than negotiating the best format we can use.
            ResourceType::Image => "image/webp,image/apng,image/svg+xml,image/*,*/*;q=0.8",
            ResourceType::Script
            | ResourceType::Font
            | ResourceType::Xhr
            | ResourceType::Fetch
            | ResourceType::Other => "*/*",
        }
    }

    pub(crate) fn sends_credentials_to(&self, target: &Url) -> bool {
        match self.credentials {
            RequestCredentials::Omit => false,
            RequestCredentials::Include => true,
            RequestCredentials::SameOrigin => self
                .initiator
                .as_ref()
                .is_some_and(|initiator| initiator.origin() == target.origin()),
        }
    }
}

pub(crate) struct InFlightGuard {
    counter: Arc<AtomicU32>,
}

impl InFlightGuard {
    pub(crate) fn new(counter: &Arc<AtomicU32>) -> Self {
        counter.fetch_add(1, Ordering::AcqRel);
        Self {
            counter: counter.clone(),
        }
    }
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::AcqRel);
    }
}

pub(crate) fn same_origin(request: &ResourceRequest, target: &Url) -> bool {
    request
        .initiator
        .as_ref()
        .is_some_and(|initiator| initiator.origin() == target.origin())
}

pub(crate) fn cors_required(request: &ResourceRequest, target: &Url) -> bool {
    request.mode == RequestMode::Cors && !same_origin(request, target)
}

/// Serialize the request origin used by both the Origin request header and the
/// response CORS check. A redirect chain that changes origin after it has
/// already left the initiator origin is tainted and serializes to `null`.
pub(crate) fn serialized_request_origin(
    request: &ResourceRequest,
    redirect_tainted: bool,
) -> String {
    if redirect_tainted {
        return "null".to_string();
    }
    request
        .initiator
        .as_ref()
        .filter(|url| matches!(url.scheme(), "http" | "https"))
        .map(|url| url.origin().ascii_serialization())
        .unwrap_or_else(|| "null".to_string())
}

pub(crate) fn redirect_taints_origin(
    request: &ResourceRequest,
    current: &Url,
    next: &Url,
) -> bool {
    current.origin() != next.origin()
        && request
            .initiator
            .as_ref()
            .is_none_or(|initiator| initiator.origin() != current.origin())
}

pub(crate) fn validate_request_mode(
    request: &ResourceRequest,
    target: &Url,
) -> Result<(), ObscuraNetError> {
    if request.mode == RequestMode::SameOrigin && !same_origin(request, target) {
        return Err(ObscuraNetError::Cors(format!(
            "same-origin request blocked for {}",
            target
        )));
    }
    Ok(())
}

pub(crate) fn validate_cors_response(
    request: &ResourceRequest,
    target: &Url,
    serialized_origin: &str,
    allow_origin: Option<&str>,
    allow_credentials: Option<&str>,
) -> Result<(), ObscuraNetError> {
    if !cors_required(request, target) {
        return Ok(());
    }

    let allow_origin = allow_origin.ok_or_else(|| {
        ObscuraNetError::Cors(format!(
            "{} did not include Access-Control-Allow-Origin for origin {}",
            target, serialized_origin
        ))
    })?;
    if request.credentials != RequestCredentials::Include && allow_origin == "*" {
        return Ok(());
    }
    if allow_origin != serialized_origin {
        return Err(ObscuraNetError::Cors(format!(
            "{} returned Access-Control-Allow-Origin {:?}, expected {:?}",
            target, allow_origin, serialized_origin
        )));
    }
    if request.credentials == RequestCredentials::Include
        && allow_credentials != Some("true")
    {
        return Err(ObscuraNetError::Cors(format!(
            "credentialed response from {} requires Access-Control-Allow-Credentials: true",
            target
        )));
    }
    Ok(())
}

pub(crate) fn response_too_large(url: &Url, limit: usize) -> ObscuraNetError {
    ObscuraNetError::ResponseTooLarge {
        url: url.to_string(),
        limit,
    }
}

pub(crate) fn request_fetch_site(request: &ResourceRequest, target: &Url) -> &'static str {
    let Some(initiator) = request.initiator.as_ref() else {
        return "none";
    };
    if initiator.origin() == target.origin() {
        "same-origin"
    } else {
        // A public-suffix-aware `same-site` classification will be added with
        // the page resource scheduler. Until then, cross-site is the safe
        // conservative value; it never overstates ambient trust.
        "cross-site"
    }
}

pub(crate) fn request_referrer(request: &ResourceRequest, target: &Url) -> Option<String> {
    request.referrer_policy.referrer(request.referrer.as_ref(), target)
        .map(|url| url.to_string())
}

pub type RequestCallback = Arc<dyn Fn(&RequestInfo) + Send + Sync>;
pub type ResponseCallback = Arc<dyn Fn(&RequestInfo, &Response) + Send + Sync>;

/// Page-scoped store for the passive on_request/on_response callbacks (issue
/// #408). Each `Page` owns one, so a callback never fires for another page's
/// requests and dies with its page. The HTTP client itself stays
/// callback-free; page-driven fetches pass the page's registry in. Ids keep
/// the `u64` shape #416 established on `Page::on_request`/`on_response`.
pub struct CallbackRegistry {
    on_request: RwLock<Vec<(u64, RequestCallback)>>,
    on_response: RwLock<Vec<(u64, ResponseCallback)>>,
    id_counter: std::sync::atomic::AtomicU64,
}

impl CallbackRegistry {
    pub fn new() -> Self {
        CallbackRegistry {
            on_request: RwLock::new(Vec::new()),
            on_response: RwLock::new(Vec::new()),
            id_counter: std::sync::atomic::AtomicU64::new(1),
        }
    }

    fn next_id(&self) -> u64 {
        self.id_counter
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    /// Register a request callback; the returned id detaches it via
    /// `remove_request`. Sync like the pre-registry push path: registration
    /// happens from `Page` setup where no reader holds the lock, so
    /// `try_write` cannot fail there.
    pub fn add_request(&self, cb: RequestCallback) -> u64 {
        let id = self.next_id();
        if let Ok(mut v) = self.on_request.try_write() {
            v.push((id, cb));
        }
        id
    }

    /// Register a response callback; see `add_request`.
    pub fn add_response(&self, cb: ResponseCallback) -> u64 {
        let id = self.next_id();
        if let Ok(mut v) = self.on_response.try_write() {
            v.push((id, cb));
        }
        id
    }

    /// Detach a request callback. Returns true when the id was found and
    /// removed, so a double detach is a visible no-op.
    pub fn remove_request(&self, id: u64) -> bool {
        match self.on_request.try_write() {
            Ok(mut v) => {
                let before = v.len();
                v.retain(|(cid, _)| *cid != id);
                v.len() != before
            }
            Err(_) => false,
        }
    }

    /// Detach a response callback; see `remove_request`.
    pub fn remove_response(&self, id: u64) -> bool {
        match self.on_response.try_write() {
            Ok(mut v) => {
                let before = v.len();
                v.retain(|(cid, _)| *cid != id);
                v.len() != before
            }
            Err(_) => false,
        }
    }

    /// True when at least one request callback is registered. Lets fire sites
    /// skip building a `RequestInfo` when nobody listens.
    pub async fn has_request_callbacks(&self) -> bool {
        !self.on_request.read().await.is_empty()
    }

    /// True when at least one response callback is registered.
    pub async fn has_response_callbacks(&self) -> bool {
        !self.on_response.read().await.is_empty()
    }

    pub async fn fire_request(&self, info: &RequestInfo) {
        for (_, cb) in self.on_request.read().await.iter() {
            cb(info);
        }
    }

    pub async fn fire_response(&self, info: &RequestInfo, resp: &Response) {
        for (_, cb) in self.on_response.read().await.iter() {
            cb(info, resp);
        }
    }
}

impl Default for CallbackRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Process-wide opt-in via env var. Older flow that issue #4 introduced. The
/// new `--allow-private-network` CLI flag (issue #33) sets a per-client field
/// that is OR'd with this so existing scripts and Docker setups that pin the
/// env var keep working unchanged.
pub fn env_allows_private_network() -> bool {
    matches!(
        std::env::var("OBSCURA_ALLOW_PRIVATE_NETWORK")
            .ok()
            .as_deref()
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("1") | Some("true") | Some("yes") | Some("on")
    )
}

/// True when `ip` must never be the target of an outbound request from the
/// engine: loopback, RFC1918 private, link-local (incl. the 169.254.169.254
/// cloud-metadata endpoint), broadcast, documentation, the unspecified address
/// (0.0.0.0 / ::, which the OS routes to localhost), IPv6 unique-local
/// (fc00::/7), and any IPv4-mapped/compatible IPv6 form of the above.
/// Centralizes the SSRF deny-set so the literal-host check and the
/// DNS-resolution check (`SsrfGuardResolver`) can never disagree.
pub fn is_forbidden_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_unspecified()
                || v4.is_multicast()
                || o[0] == 0
                // std's is_private() covers only RFC1918, so add the IANA
                // special-purpose ranges that also host internal services and
                // are common SSRF targets:
                //   100.64.0.0/10  CGNAT / RFC6598 — cloud metadata (e.g.
                //                  Alibaba 100.100.100.200) lives here.
                //   198.18.0.0/15  benchmarking / RFC2544.
                //   192.88.99.0/24 6to4 relay anycast / RFC7526.
                || (o[0] == 100 && (64..=127).contains(&o[1]))
                || (o[0] == 198 && (o[1] == 18 || o[1] == 19))
                || (o[0] == 192 && o[1] == 88 && o[2] == 99)
                // Most of 192.0.0.0/24 is special-purpose and not globally
                // reachable. Keep the two globally reachable PCP anycast
                // assignments usable rather than blocking the entire /24.
                || (o[0] == 192
                    && o[1] == 0
                    && o[2] == 0
                    && o[3] != 9
                    && o[3] != 10)
                // 240.0.0.0/4 is reserved (255.255.255.255 was already
                // covered by is_broadcast()).
                || o[0] >= 240
        }
        IpAddr::V6(v6) => {
            if v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_unique_local()
                || v6.is_unicast_link_local()
                || v6.is_multicast()
            {
                return true;
            }
            // Unwrap IPv4-mapped (::ffff:a.b.c.d) and IPv4-compatible (::a.b.c.d)
            // forms and re-check the embedded v4 so e.g. [::ffff:127.0.0.1] or
            // [::ffff:169.254.169.254] cannot slip past the v6 arm.
            if let Some(v4) = v6.to_ipv4_mapped().or_else(|| v6.to_ipv4()) {
                return is_forbidden_ip(IpAddr::V4(v4));
            }

            let s = v6.segments();
            // IPv4/IPv6 translation prefix (RFC 6052). Only /96 has a fixed
            // embedded-address position; the local-use /48 is therefore
            // blocked outright below.
            if s[0] == 0x64
                && s[1] == 0xff9b
                && s[2] == 0
                && s[3] == 0
                && s[4] == 0
                && s[5] == 0
            {
                return is_forbidden_ip(IpAddr::V4(Ipv4Addr::new(
                    (s[6] >> 8) as u8,
                    s[6] as u8,
                    (s[7] >> 8) as u8,
                    s[7] as u8,
                )));
            }
            // 6to4 carries its IPv4 endpoint in bits 16..48.
            if s[0] == 0x2002 {
                return is_forbidden_ip(IpAddr::V4(Ipv4Addr::new(
                    (s[1] >> 8) as u8,
                    s[1] as u8,
                    (s[2] >> 8) as u8,
                    s[2] as u8,
                )));
            }

            // Discard-only, local-use NAT64, and documentation prefixes.
            (s[0] == 0x100 && s[1] == 0 && s[2] == 0 && s[3] == 0)
                || (s[0] == 0x64 && s[1] == 0xff9b && s[2] == 1)
                || (s[0] == 0x2001 && s[1] == 0x0db8)
                || (s[0] == 0x3fff && s[1] & 0xf000 == 0)
        }
    }
}

/// DNS resolver that performs the lookup and then rejects the whole request if
/// ANY resolved address is in the SSRF deny-set. This closes the DNS-rebinding
/// bypass a host-string check alone cannot: a public name that resolves to
/// 127.0.0.1 / 169.254.169.254 / an RFC1918 address is blocked at connect time,
/// using the very addresses the client will dial. When private access is
/// permitted (`--allow-private-network` or `OBSCURA_ALLOW_PRIVATE_NETWORK`) the
/// lookup passes through unfiltered.
///
/// The primp transport implements the resolver in `stealth_client.rs`.
pub struct SsrfGuardResolver {
    pub(crate) allow_private: bool,
}

impl SsrfGuardResolver {
    pub fn new(allow_private: bool) -> Self {
        Self { allow_private }
    }
}

pub(crate) fn validate_url(url: &Url, allow_private_network: bool) -> Result<(), ObscuraNetError> {
    let allow_private_network = allow_private_network || env_allows_private_network();
    let scheme = url.scheme();
    if scheme != "http" && scheme != "https" && scheme != "file" {
        return Err(ObscuraNetError::Network(format!(
            "Forbidden URL scheme '{}' - only http, https, and file are allowed",
            scheme
        )));
    }

    if scheme == "file" || allow_private_network {
        return Ok(());
    }

    if let Some(host) = url.host() {
        match host {
            url::Host::Ipv4(ip) => {
                if is_forbidden_ip(IpAddr::V4(ip)) {
                    return Err(ObscuraNetError::Network(format!(
                        "Access to private/internal IP address {} is not allowed",
                        ip
                    )));
                }
            }
            url::Host::Ipv6(ip) => {
                if is_forbidden_ip(IpAddr::V6(ip)) {
                    return Err(ObscuraNetError::Network(format!(
                        "Access to private/internal IPv6 address {} is not allowed",
                        ip
                    )));
                }
            }
            url::Host::Domain(domain) => {
                let lower_domain = domain.to_lowercase();
                if lower_domain == "localhost"
                    || lower_domain.ends_with(".localhost")
                    || lower_domain == "127.0.0.1"
                    || lower_domain == "::1"
                {
                    return Err(ObscuraNetError::Network(format!(
                        "Access to localhost domain '{}' is not allowed",
                        domain
                    )));
                }
            }
        }
    }

    Ok(())
}

pub(crate) async fn fetch_file_url(
    url: &Url,
    max_response_bytes: usize,
) -> Result<Response, ObscuraNetError> {
    let path = url
        .to_file_path()
        .map_err(|_| ObscuraNetError::Network("Invalid file URL".to_string()))?;
    if let Ok(metadata) = tokio::fs::metadata(&path).await {
        if metadata.len() > max_response_bytes as u64 {
            return Err(response_too_large(url, max_response_bytes));
        }
    }
    let body = tokio::fs::read(&path)
        .await
        .map_err(|e| ObscuraNetError::Network(format!("Failed to read file: {}", e)))?;
    if body.len() > max_response_bytes {
        return Err(response_too_large(url, max_response_bytes));
    }

    let mut headers = HashMap::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        let ct = match ext.to_lowercase().as_str() {
            "html" | "htm" => "text/html",
            "css" => "text/css",
            "js" | "mjs" => "application/javascript",
            "json" => "application/json",
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "gif" => "image/gif",
            "svg" => "image/svg+xml",
            "webp" => "image/webp",
            "ico" => "image/x-icon",
            _ => "application/octet-stream",
        };
        headers.insert("content-type".to_string(), ct.to_string());
    }

    Ok(Response {
        url: url.clone(),
        status: 200,
        headers,
        body,
        redirected_from: Vec::new(),
        request_referrer: None,
    })
}

/// Shared network policy. Sending requires a persona-owned `StealthHttpClient`.
pub struct ObscuraHttpClient {
    proxy_url: Option<String>,
    pub cookie_jar: Arc<CookieJar>,
    // These are shared (`Arc`) rather than owned so a detached client keeps
    // seeing later configuration changes, exactly like the stealth client's
    // `extra_headers`.
    pub user_agent: Arc<RwLock<String>>,
    pub accept_language: Arc<RwLock<String>>,
    pub extra_headers: Arc<RwLock<HashMap<String, String>>>,
    pub interceptor: Arc<RwLock<Option<Arc<dyn RequestInterceptor + Send + Sync>>>>,
    pub in_flight: Arc<std::sync::atomic::AtomicU32>,
    pub block_trackers: bool,
    /// When true, `validate_url` lets localhost / RFC1918 / link-local addresses
    /// through in addition to the `OBSCURA_ALLOW_PRIVATE_NETWORK` env var.
    /// Set via `--allow-private-network` on the CLI (issue #33).
    pub allow_private_network: bool,
}

const RESOURCE_CACHE_MAX_ENTRIES: usize = 256;
const RESOURCE_CACHE_MAX_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub(crate) struct ResourceCacheKey {
    pub(crate) url: String,
    pub(crate) resource_type: ResourceType,
    pub(crate) mode: RequestMode,
    pub(crate) credentials: RequestCredentials,
    pub(crate) initiator: Option<String>,
    pub(crate) referrer: Option<String>,
    pub(crate) referrer_policy: ReferrerPolicy,
    pub(crate) user_agent: String,
    pub(crate) extra_headers: Vec<(String, String)>,
    pub(crate) max_response_bytes: usize,
}

#[derive(Clone)]
struct ResourceCacheEntry {
    response: Response,
    expires_at: Instant,
}

#[derive(Default)]
pub(crate) struct ResourceCache {
    entries: HashMap<ResourceCacheKey, ResourceCacheEntry>,
    insertion_order: VecDeque<ResourceCacheKey>,
    body_bytes: usize,
}

#[derive(Default)]
pub(crate) struct ResourceLoaderState {
    pub(crate) cache: ResourceCache,
    pub(crate) shared_fetches: HashMap<ResourceCacheKey, SharedFetchSender>,
}

#[derive(Clone)]
pub(crate) enum SharedFetchOutcome {
    Cacheable(Response),
    RetryUncoalesced,
}

pub(crate) type SharedFetchSender = watch::Sender<Option<SharedFetchOutcome>>;

pub(crate) struct SharedFetchLeader<'a> {
    pub(crate) loader: &'a std::sync::Mutex<ResourceLoaderState>,
    pub(crate) key: ResourceCacheKey,
    pub(crate) sender: SharedFetchSender,
    pub(crate) finished: bool,
}

impl SharedFetchLeader<'_> {
    pub(crate) fn finish(mut self, outcome: SharedFetchOutcome) {
        self.loader.lock().unwrap().shared_fetches.remove(&self.key);
        let _ = self.sender.send(Some(outcome));
        self.finished = true;
    }
}

impl Drop for SharedFetchLeader<'_> {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        self.loader.lock().unwrap().shared_fetches.remove(&self.key);
        let _ = self.sender.send(Some(SharedFetchOutcome::RetryUncoalesced));
    }
}

impl ResourceCache {
    pub(crate) fn get(&mut self, key: &ResourceCacheKey) -> Option<Response> {
        let entry = self.entries.get(key)?;
        if entry.expires_at <= Instant::now() {
            let expired = self.entries.remove(key)?;
            self.body_bytes = self.body_bytes.saturating_sub(expired.response.body.len());
            return None;
        }
        Some(entry.response.clone())
    }

    pub(crate) fn insert(&mut self, key: ResourceCacheKey, response: Response, lifetime: Duration) {
        let response_bytes = response.body.len();
        if response_bytes > RESOURCE_CACHE_MAX_BYTES {
            return;
        }
        if let Some(previous) = self.entries.remove(&key) {
            self.body_bytes = self.body_bytes.saturating_sub(previous.response.body.len());
            self.insertion_order.retain(|queued| queued != &key);
        }
        while self.entries.len() >= RESOURCE_CACHE_MAX_ENTRIES
            || self.body_bytes.saturating_add(response_bytes) > RESOURCE_CACHE_MAX_BYTES
        {
            let Some(oldest) = self.insertion_order.pop_front() else {
                break;
            };
            if let Some(entry) = self.entries.remove(&oldest) {
                self.body_bytes = self.body_bytes.saturating_sub(entry.response.body.len());
            }
        }
        self.body_bytes = self.body_bytes.saturating_add(response_bytes);
        self.insertion_order.push_back(key.clone());
        self.entries.insert(
            key,
            ResourceCacheEntry {
                response,
                expires_at: Instant::now() + lifetime,
            },
        );
    }
}

pub(crate) fn response_cache_lifetime(response: &Response) -> Option<Duration> {
    if !(200..300).contains(&response.status)
        || !response.redirected_from.is_empty()
        || response.header("set-cookie").is_some()
        || response
            .header("vary")
            .is_some_and(|vary| vary.split(',').any(|name| name.trim() == "*"))
    {
        return None;
    }
    let cache_control = response.header("cache-control")?;
    let mut max_age = None;
    for directive in cache_control.split(',').map(str::trim) {
        let lower = directive.to_ascii_lowercase();
        if lower == "no-store" || lower == "no-cache" {
            return None;
        }
        if let Some(value) = lower.strip_prefix("max-age=") {
            max_age = value.trim_matches('"').parse::<u64>().ok();
        }
    }
    max_age
        .filter(|seconds| *seconds > 0)
        .map(Duration::from_secs)
}

impl ObscuraHttpClient {
    pub fn new() -> Self {
        Self::with_cookie_jar(Arc::new(CookieJar::new()))
    }

    pub fn with_cookie_jar(cookie_jar: Arc<CookieJar>) -> Self {
        Self::with_options(cookie_jar, None)
    }

    pub fn with_options(cookie_jar: Arc<CookieJar>, proxy_url: Option<&str>) -> Self {
        Self::with_full_options(cookie_jar, proxy_url, false)
    }

    pub fn with_full_options(
        cookie_jar: Arc<CookieJar>,
        proxy_url: Option<&str>,
        allow_private_network: bool,
    ) -> Self {
        ObscuraHttpClient {
            proxy_url: proxy_url.map(|s| s.to_string()),
            cookie_jar,
            user_agent: Arc::new(RwLock::new(
                "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/145.0.0.0 Safari/537.36".to_string(),
            )),
            accept_language: Arc::new(RwLock::new("en-US,en;q=0.9".to_string())),
            extra_headers: Arc::new(RwLock::new(HashMap::new())),
            interceptor: Arc::new(RwLock::new(None)),
            in_flight: Arc::new(std::sync::atomic::AtomicU32::new(0)),
            block_trackers: false,
            allow_private_network,
        }
    }

    /// Share policy state with a worker or a replacement cookie binding.
    /// Connection pools and resource caches belong to `StealthHttpClient`.
    pub fn detached(&self) -> Self {
        ObscuraHttpClient {
            proxy_url: self.proxy_url.clone(),
            cookie_jar: self.cookie_jar.clone(),
            user_agent: self.user_agent.clone(),
            accept_language: self.accept_language.clone(),
            extra_headers: self.extra_headers.clone(),
            interceptor: self.interceptor.clone(),
            in_flight: self.in_flight.clone(),
            block_trackers: self.block_trackers,
            allow_private_network: self.allow_private_network,
        }
    }

    /// The configured upstream proxy for the persona-owned transport.
    pub fn proxy_url(&self) -> Option<&str> {
        self.proxy_url.as_deref()
    }

    pub async fn set_user_agent(&self, ua: &str) {
        *self.user_agent.write().await = ua.to_string();
    }

    pub async fn set_accept_language(&self, accept_language: &str) {
        *self.accept_language.write().await = accept_language.to_string();
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

impl Default for ObscuraHttpClient {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ObscuraNetError {
    #[error("Network error: {0}")]
    Network(String),

    #[error("Too many redirects: {0}")]
    TooManyRedirects(String),

    #[error("Request blocked: {0}")]
    Blocked(String),

    #[error("CORS error: {0}")]
    Cors(String),

    #[error("Response body exceeded {limit} byte limit: {url}")]
    ResponseTooLarge { url: String, limit: usize },
}
