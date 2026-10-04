//! Private transport slice. No JS exposure until owner policy/session gates exist.
use std::time::Duration;
use http::{HeaderMap, HeaderValue, StatusCode};
use tokio::sync::watch;
use tokio_tungstenite::{WebSocketStream, tungstenite::{handshake::{client::generate_key, derive_accept_key}, protocol::{Role, WebSocketConfig}}};
use url::Url;
use crate::{CookieJar, ObscuraNetError, stealth_client::transport::Client};

pub const MAX_MESSAGE: usize = 1 << 20;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);

fn failed(reason: &'static str) -> ObscuraNetError { ObscuraNetError::Network(reason.into()) }

/// Must come from actual owner navigation metadata, not JS/header arguments.
/// No production implementation exists in this isolated transport proposal.
/// Missing connect-src/cookie context is an error, never an implicit allow.
pub trait OwnerPolicy: Send + Sync {
    fn authorize(&self, socket_url: &Url, http_url: &Url, jar: &CookieJar) -> Result<Credentials, ObscuraNetError>;
    fn response_cookies(&self, http_url: &Url, headers: &HeaderMap, jar: &CookieJar) -> Result<(), ObscuraNetError>;
}
pub struct Credentials {
    pub origin: String,
    pub cookie_header: String,
    pub headers: Vec<(String,String)>,
}

pub(crate) struct BoundPolicy<'a> {
    pub owner: &'a dyn OwnerPolicy,
    pub headers: std::collections::HashMap<String,String>,
}
impl OwnerPolicy for BoundPolicy<'_> {
    fn authorize(&self,socket:&Url,http:&Url,jar:&CookieJar)->Result<Credentials,ObscuraNetError> {
        let mut credentials=self.owner.authorize(socket,http,jar)?;
        let mut headers=self.headers.clone();
        for (name,value) in &credentials.headers {
            headers.retain(|key,_|!key.eq_ignore_ascii_case(name));headers.insert(name.clone(),value.clone());
        }
        credentials.headers=headers.into_iter().collect();Ok(credentials)
    }
    fn response_cookies(&self,http:&Url,headers:&HeaderMap,jar:&CookieJar)->Result<(),ObscuraNetError> {
        self.owner.response_cookies(http,headers,jar)
    }
}

/// A real upgraded transport, not yet the browser-owned bounded session actor.
/// The future actor must exclusively own this stream and cancel while idle too.
pub struct Opened {
    pub stream: WebSocketStream<primp::Upgraded>,
    pub protocol: String,
    pub url: Url,
}

fn token(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
}
fn protocols_valid(protocols: &[String]) -> bool {
    protocols.len() <= 32 && protocols.iter().map(String::len).sum::<usize>() <= 4096
        && protocols.iter().enumerate().all(|(i,p)| token(p) && !protocols[..i].contains(p))
}
pub(crate) fn mapped_url(url: &Url) -> Result<(Url, Url), ObscuraNetError> {
    if url.as_str().len()>16*1024 || url.fragment().is_some() || url.host().is_none() {
        return Err(failed("Invalid WebSocket URL"));
    }
    let (socket_scheme,http_scheme)=match url.scheme() {
        "ws"|"http" => ("ws","http"), "wss"|"https" => ("wss","https"),
        _ => return Err(failed("Invalid WebSocket scheme")),
    };
    let mut socket=url.clone();socket.set_scheme(socket_scheme).map_err(|_|failed("Invalid WebSocket URL"))?;
    let mut http=url.clone();http.set_scheme(http_scheme).map_err(|_|failed("Invalid WebSocket URL"))?;
    Ok((socket,http))
}
fn singleton<'a>(headers: &'a HeaderMap, name: &'static str) -> Result<Option<&'a str>, ObscuraNetError> {
    let mut values=headers.get_all(name).iter();
    let Some(value)=values.next() else {return Ok(None)};
    if values.next().is_some() {return Err(failed("Duplicate WebSocket response header"))}
    value.to_str().map(|value|Some(value.trim())).map_err(|_|failed("Invalid WebSocket response header"))
}
fn validate_header_budget(headers: &HeaderMap) -> Result<(), ObscuraNetError> {
    if headers.len()>128 || headers.iter().map(|(key,value)|key.as_str().len()+value.as_bytes().len()).sum::<usize>()>32*1024 {
        return Err(failed("WebSocket response headers exceed limit"));
    }
    Ok(())
}
fn validate_response(status: StatusCode, headers: &HeaderMap, key: &str, offered: &[String]) -> Result<String, ObscuraNetError> {
    if status!=StatusCode::SWITCHING_PROTOCOLS {return Err(failed("WebSocket upgrade refused"))}
    if !singleton(headers,"upgrade")?.is_some_and(|value|value.eq_ignore_ascii_case("websocket")) {
        return Err(failed("Missing WebSocket Upgrade"));
    }
    let mut connection=false;
    for value in headers.get_all("connection") {
        let value=value.to_str().map_err(|_|failed("Invalid WebSocket Connection"))?;
        for item in value.split(',') {
            let item=item.trim();if !token(item) {return Err(failed("Invalid WebSocket Connection"))}
            connection |= item.eq_ignore_ascii_case("upgrade");
        }
    }
    if !connection {return Err(failed("Missing WebSocket Connection upgrade token"))}
    if singleton(headers,"sec-websocket-accept")? != Some(derive_accept_key(key.as_bytes()).as_str()) {
        return Err(failed("Invalid WebSocket accept"));
    }
    if headers.contains_key("sec-websocket-extensions") {return Err(failed("Unsolicited WebSocket extension"))}
    match singleton(headers,"sec-websocket-protocol")? {
        Some(protocol) if token(protocol) && offered.iter().any(|value|value==protocol) => Ok(protocol.to_owned()),
        None if offered.is_empty() => Ok(String::new()),
        _ => Err(failed("Invalid WebSocket subprotocol negotiation")),
    }
}

pub(crate) async fn open(client: &Client, jar: &CookieJar, allow_private: bool,
    url: &Url, protocols: &[String], owner: Option<&dyn OwnerPolicy>, cancel: &mut watch::Receiver<bool>,
) -> Result<Opened, ObscuraNetError> {
    let owner=owner.ok_or_else(||failed("WebSocket owner connect policy unavailable"))?;
    if *cancel.borrow() || cancel.has_changed().is_err() {return Err(failed("WebSocket owner retired"))}
    if !protocols_valid(protocols) {return Err(failed("Invalid WebSocket subprotocol list"))}
    let (socket_url,http_url)=mapped_url(url)?;
    crate::client::validate_url(&http_url,allow_private)?;
    // Target DNS validation stays in the exact existing primp Resolver. Remote
    // proxy DNS has the same explicitly documented trust boundary as HTTP.
    let credentials=owner.authorize(&socket_url,&http_url,jar)?;
    if credentials.origin.len()>4096 || credentials.cookie_header.len()>32*1024 {return Err(failed("WebSocket owner headers exceed limit"))}
    let key=generate_key();
    let mut headers=HeaderMap::new();
    if credentials.headers.len()>64 || credentials.headers.iter().map(|(k,v)|k.len()+v.len()).sum::<usize>()>32*1024 { return Err(failed("WebSocket headers exceed limit")); }
    for (name,value) in &credentials.headers {
        let name=http::header::HeaderName::from_bytes(name.as_bytes()).map_err(|_|failed("Invalid WebSocket header"))?;
        if matches!(name.as_str(),"cookie"|"origin"|"host"|"connection"|"upgrade"|"content-length"|"transfer-encoding") || name.as_str().starts_with("sec-") || name.as_str().starts_with("proxy-") {
            return Err(failed("WebSocket protected header override"));
        }
        headers.append(name,HeaderValue::from_str(value).map_err(|_|failed("Invalid WebSocket header"))?);
    }
    for (name,value) in [("upgrade","websocket"),("connection","Upgrade"),("sec-websocket-version","13"),("sec-fetch-mode","websocket")] {
        headers.insert(name,HeaderValue::from_static(value));
    }
    headers.insert("sec-websocket-key",HeaderValue::from_str(&key).map_err(|_|failed("Invalid WebSocket key"))?);
    headers.insert("origin",HeaderValue::from_str(&credentials.origin).map_err(|_|failed("Invalid WebSocket owner origin"))?);
    if !credentials.cookie_header.is_empty() {
        headers.insert("cookie",HeaderValue::from_str(&credentials.cookie_header).map_err(|_|failed("Invalid WebSocket cookie header"))?);
    }
    if !protocols.is_empty() {
        headers.insert("sec-websocket-protocol",HeaderValue::from_str(&protocols.join(", ")).map_err(|_|failed("Invalid WebSocket protocols"))?);
    }
    let handshake=async {
        let response=client.websocket_response(&http_url,headers).await?;
        validate_header_budget(response.headers())?;
        owner.response_cookies(&http_url,response.headers(),jar)?;
        let protocol=validate_response(response.status(),response.headers(),&key,protocols)?;
        let stream=response.upgrade().await.map_err(|_|failed("WebSocket stream upgrade failed"))?;
        let config=WebSocketConfig::default().read_buffer_size(16*1024).write_buffer_size(0)
            .max_write_buffer_size(2*MAX_MESSAGE+1024).max_message_size(Some(MAX_MESSAGE)).max_frame_size(Some(MAX_MESSAGE));
        let stream=WebSocketStream::from_raw_socket(stream,Role::Client,Some(config)).await;
        Ok(Opened {stream,protocol,url:socket_url})
    };
    tokio::select! {
        biased;
        _=cancel.changed()=>Err(failed("WebSocket owner retired")),
        result=tokio::time::timeout(HANDSHAKE_TIMEOUT,handshake)=>result.map_err(|_|failed("WebSocket handshake timed out"))?,
    }
}

#[cfg(test)]
#[path="websocket_tests.rs"]
mod tests;

#[path="websocket_session.rs"]
pub mod session;
