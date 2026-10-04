use super::*;
use deno_error::JsErrorBox;
use obscura_net::websocket::{OwnerPolicy, Credentials, session::{Owner, Session, Event}};

fn failed(message: &str) -> JsErrorBox { JsErrorBox::generic(message.to_owned()) }
#[derive(Clone)]
struct Policy {
    document: String,
    site: Option<String>,
    csp: crate::csp::ScriptPolicy,
    known: bool,
    owner: Arc<Owner>,
    blocked: Vec<String>,
    headers: Vec<(String,String)>,
}
impl OwnerPolicy for Policy {
    fn authorize(&self, socket: &url::Url, http: &url::Url, jar: &CookieJar) -> Result<Credentials, obscura_net::ObscuraNetError> {
        let denied=||obscura_net::ObscuraNetError::Network("WebSocket owner policy denied".into());
        if !self.owner.active() || !self.known || !self.csp.allows_connection(&self.document, socket)
            || self.blocked.iter().any(|pattern| glob_match(pattern,socket.as_str()) || socket.as_str().contains(pattern)) { return Err(denied()); }
        let origin=url::Url::parse(&self.document).map_err(|_|denied())?;
        if origin.scheme()=="https" && socket.scheme()=="ws" { return Err(denied()); }
        // Fetch's bad-port list applies before both direct and proxy transport.
        if matches!(socket.port_or_known_default(), Some(1|7|9|11|13|15|17|19|20|21|22|23|25|37|42|43|53|69|77|79|87|95|101|102|103|104|109|110|111|113|115|117|119|123|135|137|139|143|161|179|389|427|465|512|513|514|515|526|530|531|532|540|548|554|556|563|587|601|636|989|990|993|995|1719|1720|1723|2049|3659|4045|5060|5061|6000|6566|6665|6666|6667|6668|6669|6697|10080)) { return Err(denied()); }
        Ok(Credentials { origin:origin.origin().ascii_serialization(), cookie_header:jar.subresource_cookie_header(http,self.site.as_deref()), headers:self.headers.clone() })
    }
    fn response_cookies(&self, http:&url::Url, headers:&http::HeaderMap, jar:&CookieJar)->Result<(),obscura_net::ObscuraNetError> {
        if !self.owner.active() { return Err(obscura_net::ObscuraNetError::Network("WebSocket owner retired".into())); }
        for value in headers.get_all("set-cookie") { if let Ok(value)=value.to_str() { jar.set_subresource_cookie(value,http,self.site.as_deref()); } }
        Ok(())
    }
}

// A provisional about:blank document shares its host V8 context, but owns a
// distinct native lifetime and inherited policy. Only its private factory keeps
// this opaque capability; transport resources do not confer the parent's lease.
pub(super) struct BlankOwner {
    state: std::rc::Weak<RefCell<ObscuraState>>,
    host_owner: Arc<Owner>,
    policy: RefCell<Policy>,
    base: String,
    root: NodeId,
    generation: u64,
    speech_owner: crate::speech_owner::NativeSpeechOwnerCapability,
    metas: RefCell<std::collections::HashSet<(u32,u64)>>,
}
impl BlankOwner {
    fn new(state: &SharedState, host_owner: Arc<Owner>, policy: Policy,
        base: String, root: NodeId, generation: u64) -> Self {
        let speech_owner = crate::speech_owner::NativeSpeechOwnerCapability::blank(
            state, &host_owner, &policy.owner,
        );
        Self { state: Rc::downgrade(state), host_owner, policy: RefCell::new(policy),
            base, root, generation, speech_owner, metas: RefCell::new(Default::default()) }
    }
    // Lifecycle-only access to this exact opaque blank document. No state/frame
    // lookup and no fresh lease, even when the original document is retired.
    pub(super) fn speech_owner(&self) -> &crate::speech_owner::NativeSpeechOwnerCapability {
        &self.speech_owner
    }

    fn retire(&self) { self.policy.borrow().owner.retire(); }
}
impl deno_core::cppgc::GarbageCollected for BlankOwner {
    fn get_name(&self)->&'static std::ffi::CStr { c"WebSocketBlankOwner" }
}
impl Drop for BlankOwner { fn drop(&mut self) {self.policy.get_mut().owner.retire();} }
#[op2]
#[cppgc]
pub(super) fn op_websocket_blank_owner(scope:&mut v8::HandleScope,state:&OpState,
    creator:v8::Local<v8::Function>,#[cppgc] parent:Option<&BlankOwner>,nid:u32,#[string] document_url:&str,
)->Result<BlankOwner,JsErrorBox> {
    let state=posted_task_owner(scope,state,creator).ok_or_else(||failed("Blank owner unavailable"))?;
    let gs=state.try_borrow().map_err(|_|failed("Blank owner unavailable"))?;
    let root=NodeId::new(nid);
    let dom=gs.dom.as_ref().ok_or_else(||failed("Blank document unavailable"))?;
    let generation=dom.node_generation(root).ok_or_else(||failed("Blank document unavailable"))?;
    if !dom.get_node(root).is_some_and(|n|matches!(&n.data,obscura_dom::NodeData::Document)) {return Err(failed("Blank document unavailable"));}
    let (mut policy,base)=if let Some(parent)=parent {
        if !Arc::ptr_eq(&parent.host_owner,&gs.websocket_owner) {return Err(failed("Blank parent retired"));}
        (parent.policy.borrow().clone(),parent.base.clone())
    } else {
        let mut csp=gs.script_policy.clone();csp.append_connection_policy(&gs.websocket_dynamic_policy);
        (Policy {document:gs.websocket_origin.clone().unwrap_or_else(||gs.url.clone()),site:gs.websocket_site.clone(),
            csp,known:gs.websocket_policy_known,owner:gs.websocket_owner.clone(),blocked:gs.blocked_urls.clone(),headers:Vec::new()},gs.url.clone())
    };
    if !policy.owner.active() {return Err(failed("Blank parent retired"));}
    // A fetched provisional shim can be observed before the host publishes its
    // real FrameRealm. It has no response-bound authority and must never borrow
    // the creator's policy/origin. Only initial blank/srcdoc inherits context.
    let inherits=url::Url::parse(document_url).ok().is_some_and(|url|
        url.scheme()=="about" && matches!(url.path(),"blank"|"srcdoc"));
    if !inherits {policy.known=false;}
    policy.owner=Owner::child(&policy.owner);
    Ok(BlankOwner::new(&state, gs.websocket_owner.clone(), policy, base, root, generation))
}
#[op2(nofast)]
pub(super) fn op_websocket_blank_retire(#[cppgc] owner:&BlankOwner) {owner.retire();}
#[op2]
#[string]
pub(super) fn op_websocket_blank_base(#[cppgc] owner:&BlankOwner)->String {owner.base.clone()}
#[op2(nofast)]
pub(super) fn op_websocket_blank_meta(#[cppgc] owner:&BlankOwner,nid:u32) {
    if !owner.policy.borrow().owner.active() {return;}
    let Some(state)=owner.state.upgrade() else {return;};let Ok(state)=state.try_borrow() else {return;};
    let Some(dom)=state.dom.as_ref() else {return;};let id=NodeId::new(nid);
    if dom.node_generation(owner.root)!=Some(owner.generation) || !dom.is_connected(id) {return;}
    let Some(generation)=dom.node_generation(id) else {return;};
    if owner.metas.borrow().contains(&(nid,generation)) {return;}
    let ancestors=dom.ancestors(id);
    if !ancestors.contains(&owner.root) || !ancestors.iter().any(|id|dom.get_node(*id).is_some_and(|n|n.as_element().is_some_and(|name|name.local.as_ref()=="head"))) {return;}
    let Some(node)=dom.get_node(id) else {return;};
    if !node.as_element().is_some_and(|name|name.local.as_ref()=="meta"&&name.ns.as_ref()=="http://www.w3.org/1999/xhtml")
        || !node.get_attribute("http-equiv").is_some_and(|v|v.eq_ignore_ascii_case("content-security-policy")) {return;}
    if let Some(content)=node.get_attribute("content") {owner.policy.borrow_mut().csp.append(content);owner.metas.borrow_mut().insert((nid,generation));}
}

pub(super) struct WebSocketHandle {
    state: std::rc::Weak<RefCell<ObscuraState>>,
    owner: Arc<Owner>,
    host_owner: Arc<Owner>,
    url: url::Url,
    protocols: Vec<String>,
    policy: Policy,
    session: RefCell<Option<Arc<Session>>>,
    started: Cell<bool>,
    closed: Cell<bool>,
    cancel: tokio::sync::watch::Sender<bool>,
}
impl deno_core::cppgc::GarbageCollected for WebSocketHandle {
    fn get_name(&self)->&'static std::ffi::CStr { c"WebSocketHandle" }
}
impl Drop for WebSocketHandle { fn drop(&mut self) { if let Some(session)=self.session.get_mut() { session.cancel(); } } }

#[op2]
#[cppgc]
pub(super) fn op_websocket_create(scope:&mut v8::HandleScope, state:&OpState,
    owner:v8::Local<v8::Function>, #[string] url:&str, #[string] protocols:&str, #[cppgc] blank:Option<&BlankOwner>,
)->Result<WebSocketHandle,JsErrorBox> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let owner=posted_task_owner(scope,state,owner).ok_or_else(||failed("WebSocket owner unavailable"))?;
        let gs=owner.try_borrow().map_err(|_|failed("WebSocket owner unavailable"))?;
        let base=url::Url::parse(blank.map_or(gs.url.as_str(),|owner|owner.base.as_str())).map_err(|_|JsErrorBox::type_error("Invalid base URL"))?;
        let mut url=base.join(url).map_err(|_|JsErrorBox::type_error("Invalid WebSocket URL"))?;
        let scheme=match url.scheme() { "http"|"ws"=>"ws", "https"|"wss"=>"wss", _=>return Err(JsErrorBox::type_error("Invalid WebSocket scheme")) };
        url.set_scheme(scheme).map_err(|_|JsErrorBox::type_error("Invalid WebSocket scheme"))?;
        if url.fragment().is_some() { return Err(JsErrorBox::type_error("Invalid WebSocket URL")); }
        let protocols:Vec<String>=serde_json::from_str(protocols).map_err(|_|JsErrorBox::type_error("Invalid protocols"))?;
        let mut csp=gs.script_policy.clone();csp.append_connection_policy(&gs.websocket_dynamic_policy);
        let policy=Policy {document:gs.websocket_origin.clone().unwrap_or_else(||gs.url.clone()),site:gs.websocket_site.clone(),
            csp,known:gs.websocket_policy_known,owner:gs.websocket_owner.clone(),blocked:gs.blocked_urls.clone(),headers:Vec::new()};
        let policy=if let Some(blank)=blank {
            if !Arc::ptr_eq(&blank.host_owner,&gs.websocket_owner) {return Err(failed("Blank owner retired"));}
            blank.policy.borrow().clone()
        } else {policy};
        if !policy.owner.active() {return Err(failed("WebSocket owner retired"));}
        Ok(WebSocketHandle { state:Rc::downgrade(&owner),owner:policy.owner.clone(),host_owner:gs.websocket_owner.clone(),url,protocols,policy,
            session:RefCell::new(None),started:Cell::new(false),closed:Cell::new(false),cancel:tokio::sync::watch::channel(false).0 })
    })).unwrap_or_else(|_|Err(failed("WebSocket owner unavailable")))
}

#[op2(async)]
#[string]
pub(super) async fn op_websocket_connect(#[cppgc] handle:&WebSocketHandle)->Result<String,JsErrorBox> {
    if handle.started.replace(true) { return Err(failed("WebSocket already started")); }
    let owner=handle.state.upgrade().ok_or_else(||failed("WebSocket owner retired"))?;
    let (client,mut policy,interception)= {
        let mut gs=owner.try_borrow_mut().map_err(|_|failed("WebSocket owner unavailable"))?;
        if !Arc::ptr_eq(&gs.websocket_owner,&handle.host_owner) || !handle.owner.active() || handle.closed.get() { return Err(failed("WebSocket owner retired")); }
        let policy=handle.policy.clone();
        // Response-stage interception cannot synthesize an upgraded transport.
        // Until the response capability is exposed, a matching gate fails closed.
        if gs.intercept_response_patterns.iter().any(|p|p.resource_type.as_deref().is_none_or(|r|r=="WebSocket") && glob_match(&p.url_pattern,handle.url.as_str())) {
            return Err(failed("WebSocket response interception unavailable"));
        }
        let client=gs.ensure_persona_transport();
        let mut mapped=handle.url.clone();
        let scheme=if mapped.scheme()=="wss" {"https"} else {"http"};
        mapped.set_scheme(scheme).map_err(|_|failed("Invalid WebSocket URL"))?;
        client.validate_websocket_target(&handle.url)
            .map_err(|_|failed("WebSocket URL denied"))?;
        policy.authorize(&handle.url,&mapped,&client.cookie_jar).map_err(|_|failed("WebSocket owner policy denied"))?;
        let intercept=if gs.intercept_enabled && (gs.intercept_request_patterns.is_empty() || gs.intercept_request_patterns.iter().any(|p|
            p.resource_type.as_deref().is_none_or(|r|r=="WebSocket") && glob_match(&p.url_pattern,handle.url.as_str()))) {
            gs.intercept_tx.clone().map(|tx|(tx,gs.intercept_counter.fetch_add(1,std::sync::atomic::Ordering::Relaxed)+1,gs.network_document_generation,gs.url.clone()))
        } else {None};
        (client,policy,intercept)
    };
    drop(owner);
    let mut target=handle.url.clone();
    if let Some((tx,id,generation,document))=interception {
        let (answer,response)=tokio::sync::oneshot::channel();
        tx.send(InterceptedRequest {
            stage:InterceptionStage::Request,document_generation:generation,document_url:document,
            redirect_response:None,redirected_request_id:None,network_id:format!("websocket-{id}"),
            network_start:Arc::new(std::sync::atomic::AtomicU8::new(0)),request_raw_headers:None,
            request_body_present:false,request_body_request_id:None,request_body_size:0,
            transport_request_body_present:false,transport_request_body_request_id:None,transport_request_body_size:0,
            request_id:format!("intercept-{id}"),url:target.to_string(),method:"GET".into(),headers:HashMap::new(),
            resource_type:"WebSocket".into(),response_status_code:None,response_headers:None,response_raw_headers:None,
            response_body_request_id:None,resolver:answer,
        }).map_err(|_|failed("WebSocket interception unavailable"))?;
        let mut retired=handle.owner.subscribe();
        let mut cancelled=handle.cancel.subscribe();
        let resolution=tokio::select! {
            _=retired.changed()=>return Err(failed("WebSocket owner retired")),
            _=cancelled.changed()=>return Err(failed("WebSocket closed")),
            response=tokio::time::timeout(std::time::Duration::from_secs(30),response)=>response.map_err(|_|failed("WebSocket interception timeout"))?.map_err(|_|failed("WebSocket interception closed"))?,
        };
        match resolution {
            InterceptResolution::Continue {url,method,headers,body} => {
                if method.as_deref().is_some_and(|m|m!="GET") || body.is_some() { return Err(failed("WebSocket method/body override unsupported")); }
                if let Some(headers)=headers { policy.headers=headers.into_iter().collect(); }
                if let Some(url)=url { target=url::Url::parse(&url).map_err(|_|failed("Invalid intercepted URL"))?; }
            }
            InterceptResolution::ContinueWithHeaders {url,method,headers,body} => {
                if method.as_deref().is_some_and(|m|m!="GET") || body.is_some() { return Err(failed("WebSocket method/body override unsupported")); }
                policy.headers=headers;
                if let Some(url)=url { target=url::Url::parse(&url).map_err(|_|failed("Invalid intercepted URL"))?; }
            }
            // A synthetic 101 has no upgraded stream. Fail honestly.
            _=>return Err(failed("WebSocket intercepted request failed")),
        }
    }
    if !handle.owner.active() || handle.closed.get() { return Err(failed("WebSocket owner retired")); }
    let session=obscura_net::websocket::session::start(client,handle.owner.clone(),Arc::new(policy),target,handle.protocols.clone()).map_err(failed)?;
    *handle.session.borrow_mut()=Some(session);
    Ok(handle.url.to_string())
}

#[op2(async)]
#[string]
pub(super) async fn op_websocket_next(#[cppgc] handle:&WebSocketHandle)->Option<String> {
    let session=handle.session.borrow().clone()?;
    let delivery=session.next().await?;
    let value=match &delivery.event {
        Event::Open(protocol)=>serde_json::json!({"type":"open","protocol":protocol}),
        Event::Text(_)=>serde_json::json!({"type":"message","text":true}),
        Event::Binary(_)=>serde_json::json!({"type":"message","text":false}),
        Event::Error=>serde_json::json!({"type":"error"}),
        Event::Close{code,reason,clean}=>serde_json::json!({"type":"close","code":code,"reason":reason,"clean":clean}),
    };
    let value=value.to_string();
    session.hold_delivery(delivery);
    Some(value)
}
#[op2(nofast)]
pub(super) fn op_websocket_send(#[cppgc] handle:&WebSocketHandle, #[buffer] bytes:&[u8], text:bool)->Result<(),JsErrorBox> {
    if bytes.len()>obscura_net::websocket::MAX_MESSAGE { return Err(JsErrorBox::range_error("WebSocket message exceeds limit")); }
    handle.session.borrow().as_ref().ok_or_else(||failed("WebSocket is not open"))?.send(bytes,text).map_err(failed)
}
#[op2(nofast)]
pub(super) fn op_websocket_close(#[cppgc] handle:&WebSocketHandle, code:u32, #[string] reason:String)->Result<(),JsErrorBox> {
    handle.closed.set(true);
    handle.cancel.send_replace(true);
    if let Some(session)=handle.session.borrow().as_ref() { session.close((code!=0).then_some(code as u16),reason).map_err(failed)?; }
    Ok(())
}
#[op2(nofast)]
#[number]
pub(super) fn op_websocket_buffered(#[cppgc] handle:&WebSocketHandle)->u64 {
    handle.session.borrow().as_ref().map_or(0,|session|session.buffered())
}

#[op2(fast)]
pub(super) fn op_websocket_active(#[cppgc] handle:&WebSocketHandle)->bool { handle.owner.active() }
#[op2(nofast)]
pub(super) fn op_websocket_ack(#[cppgc] handle:&WebSocketHandle) { if let Some(session)=handle.session.borrow().as_ref() {session.acknowledge();} }

#[op2]
#[buffer]
pub(super) fn op_websocket_payload(#[cppgc] handle:&WebSocketHandle)->Vec<u8> {
    handle.session.borrow().as_ref().map_or_else(Vec::new,|s|s.payload())
}

#[cfg(test)]
#[path="websocket_tests.rs"]
mod tests;

#[op2(nofast)]
pub(super) fn op_websocket_meta(scope:&mut v8::HandleScope,state:&OpState,owner:v8::Local<v8::Function>,nid:u32) {
    let Some(owner)=posted_task_owner(scope,state,owner) else {return;};
    let Ok(mut state)=owner.try_borrow_mut() else {return;};
    if !state.websocket_owner.active() {return;}
    let Some(dom)=state.dom.as_ref() else {return;};let id=NodeId::new(nid);
    let Some(generation)=dom.node_generation(id) else {return;};
    if state.websocket_meta_nodes.contains(&(nid,generation)) || !dom.is_connected(id) {return;}
    let Some(node)=dom.get_node(id) else {return;};
    if !node.as_element().is_some_and(|name|name.local.as_ref()=="meta"&&name.ns.as_ref()=="http://www.w3.org/1999/xhtml")
        || !node.get_attribute("http-equiv").is_some_and(|v|v.eq_ignore_ascii_case("content-security-policy"))
        || !dom.ancestors(id).iter().any(|id|dom.get_node(*id).is_some_and(|n|n.as_element().is_some_and(|name|name.local.as_ref()=="head"))) {return;}
    let Some(content)=node.get_attribute("content").map(str::to_owned) else {return;};
    state.websocket_meta_nodes.insert((nid,generation));state.websocket_dynamic_policy.append(&content);
}

#[cfg(test)]
#[path="speech_blank_owner_tests.rs"]
mod speech_owner_tests;
