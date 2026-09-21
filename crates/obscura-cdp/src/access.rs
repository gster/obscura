//! Admission policy for the CDP HTTP discovery and WebSocket control plane.
//!
//! This is deliberately separate from page-network SSRF policy. It runs on
//! the accept thread before a socket consumes a connection slot or creates V8
//! state. Request bytes are only inspected for admission; page/CDP capture
//! continues to retain its complete raw data independently.

use std::net::IpAddr;

use http::uri::Authority;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

const MIN_BEARER_TOKEN_BYTES: usize = 32;
const MAX_BEARER_TOKEN_BYTES: usize = 4096;

/// Configuration supplied by a CDP server host.
///
/// The bearer token is intentionally excluded from `Debug` and is compiled to
/// a SHA-256 digest before the accept loop starts. Existing loopback callers
/// can use `Default`; non-loopback listeners require explicit hosts and a token
/// unless `allow_unauthenticated_remote` is deliberately enabled.
#[derive(Clone, Default)]
pub struct CdpAccessOptions {
    allowed_hosts: Vec<String>,
    allowed_origins: Vec<String>,
    bearer_token: Option<String>,
    advertised_websocket_url: Option<String>,
    allow_unauthenticated_remote: bool,
}

impl CdpAccessOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_allowed_hosts(mut self, allowed_hosts: Vec<String>) -> Self {
        self.allowed_hosts = allowed_hosts;
        self
    }

    pub fn with_allowed_origins(mut self, allowed_origins: Vec<String>) -> Self {
        self.allowed_origins = allowed_origins;
        self
    }

    pub fn with_bearer_token(mut self, bearer_token: Option<String>) -> Self {
        self.bearer_token = bearer_token;
        self
    }

    pub fn with_advertised_websocket_url(
        mut self,
        advertised_websocket_url: Option<String>,
    ) -> Self {
        self.advertised_websocket_url = advertised_websocket_url;
        self
    }

    pub fn allow_unauthenticated_remote(mut self, allow: bool) -> Self {
        self.allow_unauthenticated_remote = allow;
        self
    }

    /// Validate the configured policy against a concrete listener without
    /// starting the server. Useful for supervisors that proxy to worker
    /// processes and need failures before spawning them.
    pub fn validate_for_bind(&self, host: &str, port: u16) -> anyhow::Result<()> {
        let bind_ip: IpAddr = host
            .parse()
            .map_err(|error| anyhow::anyhow!("invalid --host {host:?}: {error}"))?;
        self.clone().compile(bind_ip, port).map(|_| ())
    }

    pub(crate) fn compile(self, bind_ip: IpAddr, port: u16) -> anyhow::Result<CdpAccessPolicy> {
        let mut allowed_hosts = Vec::new();
        if bind_ip.is_loopback() && self.allowed_hosts.is_empty() {
            allowed_hosts.push(canonical_authority(&format_authority(bind_ip, port), None)?);
            allowed_hosts.push(canonical_authority(&format!("localhost:{port}"), None)?);
        } else {
            for host in self.allowed_hosts {
                let authority = canonical_authority(&host, None)?;
                if !allowed_hosts.contains(&authority) {
                    allowed_hosts.push(authority);
                }
            }
        }
        if allowed_hosts.is_empty() {
            anyhow::bail!(
                "non-loopback CDP bind requires at least one --allow-host HOST[:PORT]"
            );
        }

        let bearer_digest = match self.bearer_token {
            Some(token) => {
                validate_bearer_token(&token)?;
                Some(Sha256::digest(token.as_bytes()).into())
            }
            None => None,
        };
        if !bind_ip.is_loopback()
            && bearer_digest.is_none()
            && !self.allow_unauthenticated_remote
        {
            anyhow::bail!(
                "non-loopback CDP bind requires OBSCURA_CDP_TOKEN or --auth-token-file; \
                 use --allow-unauthenticated-remote only behind a separately authenticated boundary"
            );
        }

        let mut allowed_origins = Vec::new();
        for origin in self.allowed_origins {
            let origin = canonical_origin(&origin)?;
            if !allowed_origins.contains(&origin) {
                allowed_origins.push(origin);
            }
        }

        let advertised = self
            .advertised_websocket_url
            .as_deref()
            .map(AdvertisedWebSocketUrl::parse)
            .transpose()?;

        Ok(CdpAccessPolicy {
            allowed_hosts,
            allowed_origins,
            bearer_digest,
            advertised,
        })
    }
}

impl std::fmt::Debug for CdpAccessOptions {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CdpAccessOptions")
            .field("allowed_hosts", &self.allowed_hosts)
            .field("allowed_origins", &self.allowed_origins)
            .field("bearer_token_configured", &self.bearer_token.is_some())
            .field("advertised_websocket_url", &self.advertised_websocket_url)
            .field(
                "allow_unauthenticated_remote",
                &self.allow_unauthenticated_remote,
            )
            .finish()
    }
}

#[derive(Clone)]
pub(crate) struct CdpAccessPolicy {
    allowed_hosts: Vec<String>,
    allowed_origins: Vec<String>,
    bearer_digest: Option<[u8; 32]>,
    advertised: Option<AdvertisedWebSocketUrl>,
}

#[derive(Clone)]
struct AdvertisedWebSocketUrl {
    base: String,
    browser_origin: String,
}

impl AdvertisedWebSocketUrl {
    fn parse(raw: &str) -> anyhow::Result<Self> {
        let url = url::Url::parse(raw)
            .map_err(|error| anyhow::anyhow!("invalid --advertise-websocket-url: {error}"))?;
        if !matches!(url.scheme(), "ws" | "wss")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || !matches!(url.path(), "" | "/")
            || url.query().is_some()
            || url.fragment().is_some()
        {
            anyhow::bail!(
                "--advertise-websocket-url must be ws://HOST[:PORT] or wss://HOST[:PORT]"
            );
        }
        let scheme = url.scheme();
        let authority = authority_from_url(&url)?;
        let browser_scheme = if scheme == "wss" { "https" } else { "http" };
        Ok(Self {
            base: format!("{scheme}://{authority}"),
            browser_origin: canonical_origin(&format!("{browser_scheme}://{authority}"))?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RequestRoute {
    Version,
    List,
    Protocol,
    WebSocket,
}

#[derive(Debug)]
pub(crate) struct AuthorizedRequest {
    pub route: RequestRoute,
    pub authority: String,
}

impl AuthorizedRequest {
    pub fn websocket_url(&self, policy: &CdpAccessPolicy, path: &str) -> String {
        match &policy.advertised {
            Some(advertised) => format!("{}{path}", advertised.base),
            None => format!("ws://{}{path}", self.authority),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AccessFailureKind {
    BadRequest,
    Unauthorized,
    Forbidden,
    Misdirected,
    NotFound,
    MethodNotAllowed,
    UpgradeRequired,
    RequestHeaderFieldsTooLarge,
}

#[derive(Debug)]
pub(crate) struct AccessFailure {
    pub kind: AccessFailureKind,
    pub reason: &'static str,
}

impl AccessFailure {
    pub fn response(&self) -> Vec<u8> {
        let (status, extra) = match self.kind {
            AccessFailureKind::BadRequest => ("400 Bad Request", ""),
            AccessFailureKind::Unauthorized => (
                "401 Unauthorized",
                "WWW-Authenticate: Bearer realm=\"obscura-cdp\"\r\n",
            ),
            AccessFailureKind::Forbidden => ("403 Forbidden", ""),
            AccessFailureKind::Misdirected => ("421 Misdirected Request", ""),
            AccessFailureKind::NotFound => ("404 Not Found", ""),
            AccessFailureKind::MethodNotAllowed => {
                ("405 Method Not Allowed", "Allow: GET\r\n")
            }
            AccessFailureKind::UpgradeRequired => {
                ("426 Upgrade Required", "Upgrade: websocket\r\n")
            }
            AccessFailureKind::RequestHeaderFieldsTooLarge => {
                ("431 Request Header Fields Too Large", "")
            }
        };
        format!(
            "HTTP/1.1 {status}\r\nContent-Length: 0\r\nCache-Control: no-store\r\nConnection: close\r\n\
             X-Obscura-Reason: {}\r\n{extra}\r\n",
            self.reason
        )
        .into_bytes()
    }

    pub(crate) fn request_header_fields_too_large() -> Self {
        failure(
            AccessFailureKind::RequestHeaderFieldsTooLarge,
            "request-head-too-large",
        )
    }
}

impl CdpAccessPolicy {
    pub(crate) fn authorize(&self, request_head: &[u8]) -> Result<AuthorizedRequest, AccessFailure> {
        let mut headers = [httparse::EMPTY_HEADER; 64];
        let mut request = httparse::Request::new(&mut headers);
        match request.parse(request_head) {
            Ok(httparse::Status::Complete(_)) => {}
            Ok(httparse::Status::Partial) => {
                return Err(failure(AccessFailureKind::BadRequest, "invalid-request-head"));
            }
            Err(httparse::Error::TooManyHeaders) => {
                return Err(AccessFailure::request_header_fields_too_large());
            }
            Err(_) => {
                return Err(failure(AccessFailureKind::BadRequest, "invalid-request-head"));
            }
        }
        if request.method != Some("GET") {
            return Err(failure(
                AccessFailureKind::MethodNotAllowed,
                "method-not-allowed",
            ));
        }
        if request.version != Some(1) {
            return Err(failure(AccessFailureKind::BadRequest, "http-version"));
        }
        let target = request
            .path
            .ok_or_else(|| failure(AccessFailureKind::BadRequest, "missing-target"))?;
        let route = route_for_target(target)
            .ok_or_else(|| failure(AccessFailureKind::NotFound, "route-not-found"))?;

        let host_values = header_values(request.headers, "host");
        if host_values.len() != 1 {
            return Err(failure(
                AccessFailureKind::BadRequest,
                if host_values.is_empty() {
                    "missing-host"
                } else {
                    "duplicate-host"
                },
            ));
        }
        let host = ascii_header_value(host_values[0], "invalid-host")?;
        let authority = canonical_authority(host, None)
            .map_err(|_| failure(AccessFailureKind::BadRequest, "invalid-host"))?;
        if !self.allowed_hosts.iter().any(|allowed| allowed == &authority) {
            return Err(failure(
                AccessFailureKind::Misdirected,
                "host-not-allowed",
            ));
        }

        let authorization = header_values(request.headers, "authorization");
        if authorization.len() > 1 {
            return Err(failure(
                AccessFailureKind::BadRequest,
                "duplicate-authorization",
            ));
        }
        if let Some(expected) = self.bearer_digest {
            let Some(value) = authorization.first() else {
                return Err(failure(
                    AccessFailureKind::Unauthorized,
                    "authentication-failed",
                ));
            };
            let value = ascii_header_value(value, "invalid-authorization")?;
            let Some((scheme, token)) = value.split_once(' ') else {
                return Err(failure(
                    AccessFailureKind::Unauthorized,
                    "authentication-failed",
                ));
            };
            if !scheme.eq_ignore_ascii_case("bearer")
                || token.is_empty()
                || token.as_bytes().iter().any(|byte| byte.is_ascii_whitespace())
            {
                return Err(failure(
                    AccessFailureKind::Unauthorized,
                    "authentication-failed",
                ));
            }
            let candidate: [u8; 32] = Sha256::digest(token.as_bytes()).into();
            if !constant_time_equal(&expected, &candidate) {
                return Err(failure(
                    AccessFailureKind::Unauthorized,
                    "authentication-failed",
                ));
            }
        }

        let origins = header_values(request.headers, "origin");
        if origins.len() > 1 {
            return Err(failure(
                AccessFailureKind::BadRequest,
                "duplicate-origin",
            ));
        }
        if let Some(raw_origin) = origins.first() {
            let raw_origin = ascii_header_value(raw_origin, "invalid-origin")?;
            if raw_origin == "null" {
                return Err(failure(AccessFailureKind::Forbidden, "origin-not-allowed"));
            }
            let origin = canonical_origin(raw_origin)
                .map_err(|_| failure(AccessFailureKind::Forbidden, "origin-not-allowed"))?;
            let same_origin = match &self.advertised {
                Some(advertised) => advertised.browser_origin.clone(),
                None => canonical_origin(&format!("http://{authority}"))
                    .expect("validated authority must form an origin"),
            };
            if origin != same_origin
                && !self
                    .allowed_origins
                    .iter()
                    .any(|allowed| allowed == &origin)
            {
                return Err(failure(AccessFailureKind::Forbidden, "origin-not-allowed"));
            }
        }

        if route == RequestRoute::WebSocket && !has_websocket_upgrade(request.headers) {
            return Err(failure(
                AccessFailureKind::UpgradeRequired,
                "websocket-upgrade-required",
            ));
        }

        Ok(AuthorizedRequest { route, authority })
    }
}

fn failure(kind: AccessFailureKind, reason: &'static str) -> AccessFailure {
    AccessFailure { kind, reason }
}

fn header_values<'a>(headers: &'a [httparse::Header<'a>], name: &str) -> Vec<&'a [u8]> {
    headers
        .iter()
        .filter(|header| header.name.eq_ignore_ascii_case(name))
        .map(|header| header.value)
        .collect()
}

fn has_websocket_upgrade(headers: &[httparse::Header<'_>]) -> bool {
    let upgrade = header_values(headers, "upgrade");
    if upgrade.len() != 1 {
        return false;
    }
    let Ok(upgrade) = std::str::from_utf8(upgrade[0]) else {
        return false;
    };
    if !upgrade.trim().eq_ignore_ascii_case("websocket") {
        return false;
    }
    header_values(headers, "connection").iter().any(|value| {
        std::str::from_utf8(value).is_ok_and(|value| {
            value
                .split(',')
                .any(|token| token.trim().eq_ignore_ascii_case("upgrade"))
        })
    })
}

fn ascii_header_value<'a>(value: &'a [u8], reason: &'static str) -> Result<&'a str, AccessFailure> {
    let value = std::str::from_utf8(value)
        .map_err(|_| failure(AccessFailureKind::BadRequest, reason))?
        .trim_matches([' ', '\t']);
    if value.is_empty()
        || value
            .as_bytes()
            .iter()
            .any(|byte| byte.is_ascii_control())
    {
        return Err(failure(AccessFailureKind::BadRequest, reason));
    }
    Ok(value)
}

fn route_for_target(target: &str) -> Option<RequestRoute> {
    match target {
        "/json/version" | "/json/version/" => Some(RequestRoute::Version),
        "/json" | "/json/" | "/json/list" => Some(RequestRoute::List),
        "/json/protocol" => Some(RequestRoute::Protocol),
        "/devtools/browser" => Some(RequestRoute::WebSocket),
        path if path
            .strip_prefix("/devtools/browser/")
            .is_some_and(valid_target_id) =>
        {
            Some(RequestRoute::WebSocket)
        }
        path if path
            .strip_prefix("/devtools/page/")
            .is_some_and(valid_target_id) =>
        {
            Some(RequestRoute::WebSocket)
        }
        _ => None,
    }
}

fn valid_target_id(id: &str) -> bool {
    !id.is_empty() && !id.contains(['/', '?', '#'])
}

fn validate_bearer_token(token: &str) -> anyhow::Result<()> {
    if token.len() < MIN_BEARER_TOKEN_BYTES {
        anyhow::bail!("CDP bearer token must contain at least {MIN_BEARER_TOKEN_BYTES} bytes");
    }
    if token.len() > MAX_BEARER_TOKEN_BYTES {
        anyhow::bail!("CDP bearer token must not exceed {MAX_BEARER_TOKEN_BYTES} bytes");
    }
    if !token.is_ascii()
        || token
            .as_bytes()
            .iter()
            .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
    {
        anyhow::bail!("CDP bearer token must be visible ASCII without whitespace");
    }
    Ok(())
}

fn canonical_authority(raw: &str, default_port: Option<u16>) -> anyhow::Result<String> {
    if raw.contains('@') {
        anyhow::bail!("CDP Host authority must not contain userinfo");
    }
    let parsed: Authority = raw
        .parse()
        .map_err(|error| anyhow::anyhow!("invalid CDP Host authority {raw:?}: {error}"))?;
    let parsed_host = parsed.host();
    let host = if parsed_host.starts_with('[') {
        let ip: std::net::Ipv6Addr = parsed_host[1..parsed_host.len() - 1]
            .parse()
            .map_err(|error| anyhow::anyhow!("invalid IPv6 CDP Host: {error}"))?;
        format!("[{ip}]")
    } else {
        let host = parsed_host
            .strip_suffix('.')
            .unwrap_or(parsed_host)
            .to_ascii_lowercase();
        if host.is_empty() || host == "*" {
            anyhow::bail!("CDP Host authority must name a concrete host");
        }
        match host.parse::<std::net::Ipv4Addr>() {
            Ok(ip) => ip.to_string(),
            Err(_) => host,
        }
    };
    // `Authority::port[_u16]()` intentionally returns `None` both when a
    // port is absent and when an explicit numeric port exceeds u16. Preserve
    // exact-authority semantics by inspecting the suffix ourselves.
    let port = match &parsed.as_str()[parsed_host.len()..] {
        "" => default_port,
        suffix if suffix.starts_with(':') && suffix.len() > 1 => Some(
            suffix[1..]
                .parse::<u16>()
                .map_err(|_| anyhow::anyhow!("invalid CDP Host port in {raw:?}"))?,
        ),
        _ => anyhow::bail!("invalid CDP Host authority {raw:?}"),
    };
    Ok(match port {
        Some(port) => format!("{host}:{port}"),
        None => host,
    })
}

fn canonical_origin(raw: &str) -> anyhow::Result<String> {
    let url = url::Url::parse(raw)
        .map_err(|error| anyhow::anyhow!("invalid CDP Origin {raw:?}: {error}"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || !matches!(url.path(), "" | "/")
        || url.query().is_some()
        || url.fragment().is_some()
    {
        anyhow::bail!("CDP Origin must be an http(s) origin without path, query, or fragment");
    }
    Ok(url.origin().ascii_serialization())
}

fn authority_from_url(url: &url::Url) -> anyhow::Result<String> {
    let host = url
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("advertised WebSocket URL has no host"))?;
    let host = if host.starts_with('[') && host.ends_with(']') {
        host.to_ascii_lowercase()
    } else if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_ascii_lowercase()
    };
    Ok(match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host,
    })
}

fn format_authority(ip: IpAddr, port: u16) -> String {
    match ip {
        IpAddr::V4(ip) => format!("{ip}:{port}"),
        IpAddr::V6(ip) => format!("[{ip}]:{port}"),
    }
}

fn constant_time_equal(left: &[u8; 32], right: &[u8; 32]) -> bool {
    bool::from(left.ct_eq(right))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";

    fn head(host: &str, extra: &str, target: &str) -> Vec<u8> {
        format!("GET {target} HTTP/1.1\r\nHost: {host}\r\n{extra}\r\n").into_bytes()
    }

    fn ws_head(host: &str, extra: &str, target: &str) -> Vec<u8> {
        head(
            host,
            &format!("Upgrade: websocket\r\nConnection: Upgrade\r\n{extra}"),
            target,
        )
    }

    fn loopback_options() -> CdpAccessPolicy {
        CdpAccessOptions::new()
            .compile("127.0.0.1".parse().unwrap(), 9222)
            .unwrap()
    }

    #[test]
    fn loopback_defaults_accept_only_exact_local_authorities() {
        let policy = loopback_options();
        for host in ["127.0.0.1:9222", "LOCALHOST:9222"] {
            let authorized = policy
                .authorize(&head(host, "", "/json/version"))
                .unwrap();
            assert_eq!(authorized.route, RequestRoute::Version);
        }
        let denied = policy
            .authorize(&head("attacker.test:9222", "", "/json/version"))
            .unwrap_err();
        assert_eq!(denied.kind, AccessFailureKind::Misdirected);
        assert_eq!(
            policy
                .authorize(&head("127.0.0.1", "", "/json/version"))
                .unwrap_err()
                .kind,
            AccessFailureKind::Misdirected
        );
        assert_eq!(
            policy
                .authorize(&head("localhost:65536", "", "/json/version"))
                .unwrap_err()
                .kind,
            AccessFailureKind::BadRequest
        );
        assert!(canonical_authority("localhost:65536", None).is_err());
    }

    #[test]
    fn missing_duplicate_and_malformed_host_are_bad_requests() {
        let policy = loopback_options();
        for request in [
            b"GET /json/version HTTP/1.1\r\n\r\n".as_slice(),
            b"GET /json/version HTTP/1.1\r\nHost: 127.0.0.1:9222\r\nHost: localhost:9222\r\n\r\n".as_slice(),
            b"GET /json/version HTTP/1.1\r\nHost: bad host\r\n\r\n".as_slice(),
        ] {
            assert_eq!(
                policy.authorize(request).unwrap_err().kind,
                AccessFailureKind::BadRequest
            );
        }
    }

    #[test]
    fn bearer_auth_is_required_and_compared_without_reflection() {
        let policy = CdpAccessOptions::new()
            .with_bearer_token(Some(TOKEN.into()))
            .compile("127.0.0.1".parse().unwrap(), 9222)
            .unwrap();
        assert_eq!(
            policy
                .authorize(&head("127.0.0.1:9222", "", "/json/version"))
                .unwrap_err()
                .kind,
            AccessFailureKind::Unauthorized
        );
        assert_eq!(
            policy
                .authorize(&ws_head(
                    "127.0.0.1:9222",
                    "Authorization: Bearer wrong-token-value-that-is-long-enough\r\n",
                    "/json/version",
                ))
                .unwrap_err()
                .kind,
            AccessFailureKind::Unauthorized
        );
        policy
            .authorize(&head(
                "127.0.0.1:9222",
                &format!("Authorization: Bearer {TOKEN}\r\n"),
                "/json/version",
            ))
            .unwrap();
    }

    #[test]
    fn browser_origin_must_be_same_origin_or_explicitly_allowed() {
        let policy = CdpAccessOptions::new()
            .with_allowed_origins(vec!["https://control.example.test".into()])
            .compile("127.0.0.1".parse().unwrap(), 9222)
            .unwrap();
        for origin in ["http://127.0.0.1:9222", "https://control.example.test"] {
            policy
                .authorize(&ws_head(
                    "127.0.0.1:9222",
                    &format!("Origin: {origin}\r\n"),
                    "/devtools/browser",
                ))
                .unwrap();
        }
        for origin in ["null", "https://attacker.test"] {
            assert_eq!(
                policy
                    .authorize(&ws_head(
                        "127.0.0.1:9222",
                        &format!("Origin: {origin}\r\n"),
                        "/devtools/browser",
                    ))
                    .unwrap_err()
                    .kind,
                AccessFailureKind::Forbidden
            );
        }
    }

    #[test]
    fn remote_bind_requires_hosts_and_auth_without_explicit_unsafe_opt_in() {
        let remote: IpAddr = "0.0.0.0".parse().unwrap();
        assert!(CdpAccessOptions::new().compile(remote, 9222).is_err());
        assert!(CdpAccessOptions::new()
            .with_allowed_hosts(vec!["cdp.example.test:443".into()])
            .compile(remote, 9222)
            .is_err());
        CdpAccessOptions::new()
            .with_allowed_hosts(vec!["cdp.example.test:443".into()])
            .with_bearer_token(Some(TOKEN.into()))
            .compile(remote, 9222)
            .unwrap();
        CdpAccessOptions::new()
            .with_allowed_hosts(vec!["cdp.example.test:443".into()])
            .allow_unauthenticated_remote(true)
            .compile(remote, 9222)
            .unwrap();
    }

    #[test]
    fn advertised_url_is_separate_from_validated_host() {
        let policy = CdpAccessOptions::new()
            .with_allowed_hosts(vec!["internal.test:9222".into()])
            .with_bearer_token(Some(TOKEN.into()))
            .with_advertised_websocket_url(Some("wss://cdp.example.test:443".into()))
            .compile("0.0.0.0".parse().unwrap(), 9222)
            .unwrap();
        let request = policy
            .authorize(&head(
                "internal.test:9222",
                &format!(
                    "Authorization: Bearer {TOKEN}\r\nOrigin: https://cdp.example.test\r\n"
                ),
                "/json/version",
            ))
            .unwrap();
        assert_eq!(
            request.websocket_url(&policy, "/devtools/browser"),
            "wss://cdp.example.test/devtools/browser"
        );
    }

    #[test]
    fn strict_routes_and_methods_do_not_fall_through_to_websocket() {
        let policy = loopback_options();
        assert_eq!(
            policy
                .authorize(&head("127.0.0.1:9222", "", "/json/version?x=1"))
                .unwrap_err()
                .kind,
            AccessFailureKind::NotFound
        );
        for target in ["/json/list/", "/json/protocol/"] {
            assert_eq!(
                policy
                    .authorize(&head("127.0.0.1:9222", "", target))
                    .unwrap_err()
                    .kind,
                AccessFailureKind::NotFound
            );
        }
        let post = b"POST /devtools/browser HTTP/1.1\r\nHost: 127.0.0.1:9222\r\n\r\n";
        assert_eq!(
            policy.authorize(post).unwrap_err().kind,
            AccessFailureKind::MethodNotAllowed
        );
    }

    #[test]
    fn ipv6_authorities_are_canonical_and_bracketed_once() {
        let policy = CdpAccessOptions::new()
            .compile("::1".parse().unwrap(), 9222)
            .unwrap();
        let request = policy
            .authorize(&head("[0:0:0:0:0:0:0:1]:9222", "", "/json/version"))
            .unwrap();
        assert_eq!(request.authority, "[::1]:9222");

        let advertised = AdvertisedWebSocketUrl::parse("ws://[::1]:9333").unwrap();
        assert_eq!(advertised.base, "ws://[::1]:9333");
        assert_eq!(advertised.browser_origin, "http://[::1]:9333");
    }

    #[test]
    fn access_options_debug_and_rejections_never_reflect_the_token() {
        let options = CdpAccessOptions::new().with_bearer_token(Some(TOKEN.into()));
        assert!(!format!("{options:?}").contains(TOKEN));
        let policy = options
            .compile("127.0.0.1".parse().unwrap(), 9222)
            .unwrap();
        let rejected = policy
            .authorize(&head(
                "127.0.0.1:9222",
                "Authorization: Bearer definitely-wrong-token-value\r\n",
                "/json/version",
            ))
            .unwrap_err();
        assert_eq!(rejected.kind, AccessFailureKind::Unauthorized);
        assert!(!rejected.response().windows(TOKEN.len()).any(|part| part == TOKEN.as_bytes()));
    }

    #[test]
    fn duplicate_auth_and_excessive_headers_fail_before_cdp() {
        let policy = CdpAccessOptions::new()
            .with_bearer_token(Some(TOKEN.into()))
            .compile("127.0.0.1".parse().unwrap(), 9222)
            .unwrap();
        let duplicate = head(
            "127.0.0.1:9222",
            &format!(
                "Authorization: Bearer {TOKEN}\r\nAuthorization: Bearer {TOKEN}\r\n"
            ),
            "/json/version",
        );
        assert_eq!(
            policy.authorize(&duplicate).unwrap_err().kind,
            AccessFailureKind::BadRequest
        );

        let mut excessive = String::from("GET /json/version HTTP/1.1\r\nHost: 127.0.0.1:9222\r\n");
        for index in 0..65 {
            excessive.push_str(&format!("X-Test-{index}: value\r\n"));
        }
        excessive.push_str("\r\n");
        assert_eq!(
            policy.authorize(excessive.as_bytes()).unwrap_err().kind,
            AccessFailureKind::RequestHeaderFieldsTooLarge
        );
    }
}
