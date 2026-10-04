//! Script element source checks shared by parser and dynamically inserted scripts.
//! Other CSP directives, reporting, and string compilation are separate concerns.
use base64::Engine as _;
use sha2::Digest as _;
use url::Url;

#[derive(Clone, Debug, Default)]
pub struct ScriptPolicy {
    // Each policy is independently enforced; combining headers cannot relax one.
    lists: Vec<Vec<String>>,
    connect_lists: Vec<Vec<String>>,
}

impl ScriptPolicy {
    pub fn append(&mut self, serialized: &str) {
        for policy in serialized.split(',') {
            let mut directives = std::collections::HashMap::new();
            for directive in policy.split(';') {
                let mut words = directive.split_ascii_whitespace();
                if let Some(name) = words.next() {
                    directives.entry(name.to_ascii_lowercase())
                        .or_insert_with(|| words.map(str::to_owned).collect::<Vec<_>>());
                }
            }
            if let Some(list) = directives.get("connect-src").or_else(|| directives.get("default-src")) {
                self.connect_lists.push(list.clone());
            }
            if let Some(list) = directives.remove("script-src-elem")
                .or_else(|| directives.remove("script-src"))
                .or_else(|| directives.remove("default-src")) {
                self.lists.push(list);
            }
        }
    }

    pub(crate) fn append_connection_policy(&mut self, other: &Self) {
        self.connect_lists.extend(other.connect_lists.iter().cloned());
    }

    pub fn allows_connection(&self, document: &str, source: &Url) -> bool {
        let Ok(document) = Url::parse(document) else { return false; };
        if !matches!(source.scheme(), "ws" | "wss") { return false; }
        self.connect_lists.iter().all(|list| list.iter().any(|token|
            connection_source_matches(token, &document, source)))
    }

    pub fn is_empty(&self) -> bool { self.lists.is_empty() }

    pub fn allows(&self, document: &str, source: Option<&str>, nonce: &str,
        inline: &str, parser_inserted: bool, redirected: bool) -> bool {
        self.lists.iter().all(|list| {
            if !nonce.is_empty() && list.iter().any(|token| {
                source_value(token, "nonce") == Some(nonce)
            }) { return true; }
            if let Some(source) = source {
                if list.iter().any(|s| s.eq_ignore_ascii_case("'strict-dynamic'")) {
                    return !parser_inserted;
                }
                let (Ok(document), Ok(source)) = (Url::parse(document), Url::parse(source)) else { return false; };
                list.iter().any(|token| source_matches(token, &document, &source, redirected))
            } else {
                if !parser_inserted && list.iter().any(|s| s.eq_ignore_ascii_case("'strict-dynamic'")) {
                    return true;
                }
                let has_nonce_or_hash = list.iter().any(|s| ["nonce", "sha256", "sha384", "sha512"]
                    .iter().any(|kind| source_value(s, kind).is_some()));
                if !has_nonce_or_hash && !list.iter().any(|s| s.eq_ignore_ascii_case("'strict-dynamic'"))
                    && list.iter().any(|s| s.eq_ignore_ascii_case("'unsafe-inline'")) { return true; }
                list.iter().any(|token| hash_matches(token, inline))
            }
        })
    }
}

fn source_value<'a>(token: &'a str, kind: &str) -> Option<&'a str> {
    let value = token.strip_prefix('\'')?.strip_suffix('\'')?;
    let (algorithm, value) = value.split_once('-')?;
    algorithm.eq_ignore_ascii_case(kind).then_some(value)
}

fn hash_matches(token: &str, source: &str) -> bool {
    let Some(value) = token.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')) else { return false; };
    let Some((algorithm, expected)) = value.split_once('-') else { return false; };
    let digest = match algorithm.to_ascii_lowercase().as_str() {
        "sha256" => sha2::Sha256::digest(source.as_bytes()).to_vec(),
        "sha384" => sha2::Sha384::digest(source.as_bytes()).to_vec(),
        "sha512" => sha2::Sha512::digest(source.as_bytes()).to_vec(),
        _ => return false,
    };
    let normalized = expected.replace('-', "+").replace('_', "/");
    base64::engine::general_purpose::STANDARD_NO_PAD.decode(normalized.trim_end_matches('='))
        .is_ok_and(|expected| expected == digest)
}

fn scheme_matches(expected: &str, actual: &str) -> bool {
    expected.eq_ignore_ascii_case(actual)
        || (expected.eq_ignore_ascii_case("http") && actual == "https")
        || (expected.eq_ignore_ascii_case("ws") && matches!(actual, "wss" | "http" | "https"))
}

fn source_matches(token: &str, document: &Url, source: &Url, redirected: bool) -> bool {
    if token.eq_ignore_ascii_case("'self'") {
        return document.origin() == source.origin()
            || (document.scheme() == "http" && source.scheme() == "https"
                && document.host_str() == source.host_str()
                && document.port().is_none() && source.port().is_none());
    }
    if token == "*" {
        return matches!(source.scheme(), "http" | "https" | "ws" | "wss")
            || (source.scheme() == document.scheme() && source.host_str().is_some());
    }
    if token.starts_with('\'') { return false; }
    if let Some(scheme) = token.strip_suffix(':') {
        return scheme_matches(scheme, source.scheme());
    }
    let (scheme, host_path) = token.split_once("://").unwrap_or((document.scheme(), token));
    if !scheme_matches(scheme, source.scheme()) { return false; }
    let (authority, path) = host_path.split_once('/').unwrap_or((host_path, ""));
    let (host, port) = authority.rsplit_once(':').map_or((authority, None), |(h, p)| (h, Some(p)));
    let actual_host = source.host_str().unwrap_or("");
    let host_matches = if host == "*" { !actual_host.is_empty() }
        else if let Some(suffix) = host.strip_prefix("*.") {
            actual_host.to_ascii_lowercase().ends_with(&format!(".{}", suffix.to_ascii_lowercase()))
        } else { host.eq_ignore_ascii_case(actual_host) };
    if !host_matches { return false; }
    let port_matches = match port {
        Some("*") => true,
        Some(p) => p.parse::<u16>().is_ok_and(|p| source.port_or_known_default() == Some(p)
            || (p == 80 && source.scheme() == "https" && source.port_or_known_default() == Some(443))),
        None => source.port().is_none(),
    };
    if !port_matches { return false; }
    if path.is_empty() || redirected { return true; }
    // Decode individual path segments, so an encoded slash cannot create a segment.
    let expected = format!("/{path}");
    let actual_parts: Vec<_> = source.path().split('/').collect();
    let expected_parts: Vec<_> = expected.split('/').collect();
    let count = if expected.ends_with('/') { expected_parts.len() - 1 } else { expected_parts.len() };
    if actual_parts.len() < count || (!expected.ends_with('/') && actual_parts.len() != count) { return false; }
    expected_parts[..count].iter().zip(&actual_parts[..count]).all(|(a, b)|
        percent_encoding::percent_decode_str(a).collect::<Vec<_>>() == percent_encoding::percent_decode_str(b).collect::<Vec<_>>())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn policy(value: &str) -> ScriptPolicy { let mut p = ScriptPolicy::default(); p.append(value); p }
    #[test]
    fn csp_source_fallback_intersection_and_origin() {
        let p = policy("default-src 'none'; script-src 'self' https://*.cdn.test:*/js/ 'unsafe-inline', script-src https:");
        assert!(p.allows("https://a.test", Some("https://a.test/a.js"), "", "", true, false));
        assert!(p.allows("https://a.test", Some("https://x.cdn.test:9443/js/a.js"), "", "", true, false));
        assert!(!p.allows("https://a.test", Some("https://cdn.test/js/a.js"), "", "", true, false));
        assert!(!p.allows("https://a.test", Some("https://x.cdn.test/other.js"), "", "", true, false));
        assert!(!p.allows("https://a.test", None, "", "x=1", true, false));
        assert!(!policy("script-src 'none'; script-src *").allows("https://a.test", Some("https://b.test/x"), "", "", true, false));
        assert!(policy("script-src 'none'; script-src-elem https:").allows("https://a.test", Some("https://b.test/x"), "", "", true, false));
    }
    #[test]
    fn csp_nonce_hash_and_strict_dynamic() {
        let hash = base64::engine::general_purpose::STANDARD.encode(sha2::Sha256::digest(b"x=1"));
        let p = policy(&format!("script-src 'unsafe-inline' 'nonce-AbC' 'sha256-{hash}'"));
        assert!(p.allows("https://a.test", None, "AbC", "x=2", true, false));
        assert!(!p.allows("https://a.test", None, "abc", "x=2", true, false));
        assert!(p.allows("https://a.test", None, "", "x=1", true, false));
        let p = policy("script-src 'nonce-AbC' 'strict-dynamic' https:");
        assert!(!p.allows("https://a.test", Some("https://b.test/x"), "", "", true, false));
        assert!(p.allows("https://a.test", Some("https://b.test/x"), "", "", false, false));
        assert!(p.allows("https://a.test", None, "", "x=2", false, false));
        assert!(!p.allows("https://a.test", None, "", "x=2", true, false));
        let p = policy(&format!("script-src 'unsafe-inline' 'SHA256-{hash}' 'NONCE-AbC'"));
        assert!(p.allows("https://a.test", None, "", "x=1", true, false));
        assert!(!p.allows("https://a.test", None, "", "x=2", true, false));
        assert!(p.allows("https://a.test", None, "AbC", "x=2", true, false));
        assert!(!p.allows("https://a.test", None, "abc", "x=2", true, false));
    }
}

// Connection scheme-sources compare the raw ws/wss URL. In particular http:
// does NOT authorize ws:. Only 'self' relates HTTP document and socket origins.
fn connection_source_matches(token: &str, document: &Url, source: &Url) -> bool {
    if token.eq_ignore_ascii_case("'self'") {
        let scheme = matches!((document.scheme(),source.scheme()), ("http","ws"|"wss") | ("https","wss"));
        let port = document.port_or_known_default() == source.port_or_known_default()
            || (document.port().is_none() && source.port().is_none());
        return scheme && port && document.host_str().is_some() && document.host_str() == source.host_str();
    }
    if token == "*" { return true; }
    if token.starts_with('\'') { return false; }
    let scheme_matches = |expected:&str| expected.eq_ignore_ascii_case(source.scheme())
        || (expected.eq_ignore_ascii_case("ws") && source.scheme()=="wss");
    if let Some(scheme) = token.strip_suffix(':') { return scheme_matches(scheme); }
    let (scheme, rest) = token.split_once("://").unwrap_or((document.scheme(),token));
    if !scheme_matches(scheme) { return false; }
    let (authority,path) = rest.split_once('/').unwrap_or((rest,""));
    let (host,port) = if authority.starts_with('[') {
        let Some(end)=authority.find(']') else { return false; };
        let tail=&authority[end+1..];
        if !tail.is_empty() && !tail.starts_with(':') { return false; }
        (&authority[..=end],tail.strip_prefix(':'))
    } else { authority.rsplit_once(':').map_or((authority,None),|(h,p)|(h,Some(p))) };
    let actual=source.host_str().unwrap_or("");
    let host_matches=if host=="*" {!actual.is_empty()} else if let Some(suffix)=host.strip_prefix("*.") {
        actual.to_ascii_lowercase().ends_with(&format!(".{}",suffix.to_ascii_lowercase()))
    } else {host.eq_ignore_ascii_case(actual)};
    let port_matches=match port {Some("*")=>true,Some(port)=>port.parse::<u16>().is_ok_and(|port|
        source.port_or_known_default()==Some(port) || (port==80&&source.scheme()=="wss"&&source.port_or_known_default()==Some(443))),None=>source.port().is_none()};
    if !host_matches || !port_matches { return false; }
    if path.is_empty() { return true; }
    let expected=format!("/{path}");let expected_parts:Vec<_>=expected.split('/').collect();let actual_parts:Vec<_>=source.path().split('/').collect();
    let count=if expected.ends_with('/') {expected_parts.len()-1} else {expected_parts.len()};
    actual_parts.len()>=count && (expected.ends_with('/') || actual_parts.len()==count)
        && expected_parts[..count].iter().zip(&actual_parts[..count]).all(|(a,b)|
            percent_encoding::percent_decode_str(a).collect::<Vec<_>>()==percent_encoding::percent_decode_str(b).collect::<Vec<_>>())
}

#[cfg(test)]
#[test]
fn websocket_connect_src_matches_saved_chrome153_local_oracle() {
    let document="http://127.0.0.1:50031/page";
    for (policies,cross_host,allow) in [
        (vec!["connect-src 'none'"],false,false),(vec!["connect-src 'self'"],false,true),
        (vec!["connect-src ws:"],false,true),(vec!["connect-src http:"],false,false),
        (vec!["connect-src https:"],false,false),(vec!["connect-src http://127.0.0.1:*"],false,false),
        (vec!["connect-src ws://127.0.0.1:*"],false,true),(vec!["default-src 'self'"],false,true),
        (vec!["connect-src 'self'","connect-src 'none'"],false,false),
        (vec!["connect-src 'none'; connect-src *"],false,false),
        (vec!["connect-src 'self'"],true,false),(vec!["connect-src *"],true,true),
    ] {
        let mut policy=ScriptPolicy::default();for item in &policies {policy.append(item);}
        let source=Url::parse(if cross_host {"ws://localhost:50031/ws/test"} else {"ws://127.0.0.1:50031/ws/test"}).unwrap();
        assert_eq!(policy.allows_connection(document,&source),allow,"{policies:?} cross_host={cross_host}");
    }
}
