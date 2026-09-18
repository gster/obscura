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

pub(super) struct Client(primp::Client, HeaderMap);

impl Client {
    pub fn new(
        profile: StealthProfile,
        proxy: Option<&str>,
        allow_private: bool,
        accept_language: Option<&str>,
        do_not_track: Option<&str>,
    ) -> Self {
        let (browser, os) = match profile {
            StealthProfile::WindowsChrome145 => (primp::Impersonate::ChromeV145, primp::ImpersonateOS::Windows),
            StealthProfile::MacChrome152 => (primp::Impersonate::ChromeV152, primp::ImpersonateOS::MacOS),
            StealthProfile::MacChrome153 => (primp::Impersonate::ChromeV153, primp::ImpersonateOS::MacOS),
        };
        let mut builder = primp::Client::builder()
            .impersonate(browser)
            .impersonate_os(os)
            .no_proxy()
            .redirect(primp::redirect::Policy::none())
            .retry(primp::retry::never())
            .dns_resolver(Arc::new(Resolver {
                allow_private,
                proxy_host: proxy.and_then(|s| Url::parse(s).ok())
                    .and_then(|u| u.host_str().map(|h| h.trim_matches(['[', ']']).to_owned())),
            }))
            .timeout(Duration::from_secs(30));
        if let Some(proxy) = proxy {
            builder = builder.proxy(primp::Proxy::all(proxy).expect("validated proxy URL"));
        }
        for path in crate::client::configured_root_paths() {
            match std::fs::read(&path).ok().and_then(|pem| primp::Certificate::from_pem_bundle(&pem).ok()) {
                Some(certs) => for cert in certs { builder = builder.add_root_certificate(cert); },
                None => tracing::warn!(path = %path.display(), "failed to read configured certificate roots"),
            }
        }
        let mut client = builder.build().expect("failed to build primp stealth client");
        // Impersonation presets include navigation and prefetch fields. Only
        // identity defaults belong here; Obscura supplies per-request semantics.
        let headers = client.headers_mut();
        headers.clear();
        for (name, value) in [
            ("user-agent", profile.user_agent()),
            ("sec-ch-ua", match profile {
                StealthProfile::MacChrome152 => r#""Chromium";v="152", "Not?A_Brand";v="24", "Google Chrome";v="152""#,
                // Chrome 153 reordered the brands (Google Chrome first) and changed
                // the GREASE token to "Not_A Brand";v="8". Values match primp's own
                // captures of the real builds.
                StealthProfile::MacChrome153 => r#""Google Chrome";v="153", "Not_A Brand";v="8", "Chromium";v="153""#,
                StealthProfile::WindowsChrome145 => r#""Not:A-Brand";v="99", "Google Chrome";v="145", "Chromium";v="145""#,
            }),
            ("sec-ch-ua-mobile", "?0"), ("sec-ch-ua-platform", match profile { StealthProfile::MacChrome152 | StealthProfile::MacChrome153 => "\"macOS\"", StealthProfile::WindowsChrome145 => "\"Windows\"" }),
            ("accept", "*/*"),
            ("accept-language", accept_language.unwrap_or("en-US,en;q=0.9")),
            ("accept-encoding", "gzip, deflate, br, zstd"), ("priority", "u=0, i"),
        ] { headers.insert(name, HeaderValue::from_str(value).expect("valid persona header")); }
        if let Some(value) = do_not_track {
            headers.insert("dnt", HeaderValue::from_str(value).expect("valid persona DNT"));
        }
        let defaults = std::mem::take(client.headers_mut());
        Self(client, defaults)
    }

    pub async fn send(&self, method: http::Method, url: &Url, headers: HeaderMap, body: &[u8]) -> Result<Response, ObscuraNetError> {
        let client = &self.0;
        let defaults = &self.1;
        // primp orders H2 fields itself; construct the same order for H1.
        let mut merged = headers;
        let preflight = method == http::Method::OPTIONS
            && merged.contains_key("access-control-request-method");
        for (name, value) in defaults {
            // CORS preflights do not carry browser client hints. Keep ordinary
            // OPTIONS requests unchanged and do not let primp re-add defaults.
            if preflight && name.as_str().starts_with("sec-ch-") { continue; }
            if !merged.contains_key(name) { merged.insert(name.clone(), value.clone()); }
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
        let mut request = client.request(method, url.as_str()).headers(headers);
        if !body.is_empty() { request = request.body(body.to_vec()); }
        request.send().await.map(Response).map_err(|e| network_error(url, e))
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

pub(super) struct Response(primp::Response);

impl Response {
    pub fn status(&self) -> http::StatusCode { self.0.status() }
    pub fn headers(&self) -> &HeaderMap { self.0.headers() }
    pub fn content_length(&self) -> Option<u64> { self.0.content_length() }
    pub fn bytes_stream(self) -> BoxStream<'static, Result<bytes::Bytes, ObscuraNetError>> {
        self.0.bytes_stream().map(|v| v.map_err(|e| ObscuraNetError::Network(e.to_string()))).boxed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

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
