//! One bounded interpretation for admission and execution of existing wrappers.
//! Only reply metadata survives each unwrap; nested encoded payloads are not
//! accumulated in a second stack. These work limits are not a measured RSS bound.
use std::borrow::Cow;
use serde_json::json;
use crate::types::{CdpRequest, CdpResponse, CdpEvent};

// Per decoded message uses the actual wire message cap; cumulative decoding
// uses the existing inbound-byte cap. The depth cap also bounds reply nesting.
const MAX_WRAPPER_DEPTH: usize = 32;
const MAX_MESSAGE_BYTES: usize = crate::inbound::DEFAULT_MAX_MESSAGE_BYTES;
const MAX_DECODED_BYTES: usize = crate::inbound::DEFAULT_MAX_BYTES;

pub(crate) struct Wrapper {
    id: u64,
    response_session: Option<String>,
    route_session: Option<String>,
}
pub(crate) struct CanonicalRequest<'a> {
    pub effective: Result<Cow<'a, CdpRequest>, CdpResponse>,
    pub wrappers: Vec<Wrapper>,
}
pub(crate) fn resolve(request: &CdpRequest) -> CanonicalRequest<'_> {
    resolve_bounded(request, MAX_WRAPPER_DEPTH, MAX_MESSAGE_BYTES, MAX_DECODED_BYTES)
}
fn resolve_bounded(request: &CdpRequest, max_depth: usize, max_message: usize, max_decoded: usize) -> CanonicalRequest<'_> {
    let mut current = Cow::Borrowed(request);
    let mut wrappers = Vec::new();
    let mut decoded = 0usize;
    loop {
        if current.method != "Target.sendMessageToTarget" {
            return CanonicalRequest { effective: Ok(current), wrappers };
        }
        let failure = |code, message: String| CdpResponse::error(current.id, code, message, current.session_id.clone());
        let Some(message) = current.params.get("message").and_then(|value| value.as_str()) else {
            return CanonicalRequest { effective: Err(failure(-32602, "sendMessageToTarget requires a message string".into())), wrappers };
        };
        decoded = match decoded.checked_add(message.len()) {
            Some(total) if total <= max_decoded && message.len() <= max_message && wrappers.len() < max_depth => total,
            _ => return CanonicalRequest { effective: Err(failure(-32602, "sendMessageToTarget decoding limit exceeded".into())), wrappers },
        };
        let mut inner: CdpRequest = match serde_json::from_str(message) {
            Ok(inner) => inner,
            Err(error) => return CanonicalRequest { effective: Err(failure(-32700,
                format!("sendMessageToTarget message is not a valid CDP request: {error}"))), wrappers },
        };
        let route_session = current.params.get("sessionId").and_then(|value| value.as_str()).map(str::to_owned);
        // Match each original recursive layer exactly: wrapper params wins over
        // the inner request session. A wrapper's own envelope session does not
        // otherwise become the leaf session or the received-message target.
        inner.session_id = route_session.clone().or(inner.session_id);
        wrappers.push(Wrapper { id: current.id, response_session: current.session_id.clone(), route_session });
        current = Cow::Owned(inner);
    }
}
/// Restore the original outer acknowledgements and inner-response events.
/// A malformed deeper wrapper is a response at that layer, then normal outer
/// wrappers emit their events and succeed, just as recursive dispatch did.
pub(crate) fn wrap_response(mut response: CdpResponse, wrappers: Vec<Wrapper>) -> (CdpResponse, Vec<CdpEvent>) {
    let mut events = Vec::with_capacity(wrappers.len());
    for wrapper in wrappers.into_iter().rev() {
        let serialized = match serde_json::to_string(&response) {
            Ok(serialized) => serialized,
            Err(error) => {
                response = CdpResponse::error(wrapper.id, -32603,
                    format!("could not serialize inner CDP response: {error}"), wrapper.response_session);
                continue;
            }
        };
        events.push(CdpEvent {
            method: "Target.receivedMessageFromTarget".into(),
            params: json!({"sessionId":wrapper.route_session.clone().unwrap_or_default(),
                "message":serialized,"targetId":wrapper.route_session.unwrap_or_default()}),
            session_id: wrapper.response_session.clone(),
        });
        response = CdpResponse::success(wrapper.id, json!({}), wrapper.response_session);
    }
    (response, events)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(id: u64, method: &str, session: Option<&str>) -> CdpRequest {
        CdpRequest { id, method:method.into(), params:json!({}), session_id:session.map(str::to_owned) }
    }
    fn wrapped(id: u64, envelope_session: Option<&str>, route: Option<&str>, inner: &CdpRequest) -> CdpRequest {
        CdpRequest { id, method:"Target.sendMessageToTarget".into(),
            params:json!({"sessionId":route,"message":serde_json::to_string(&json!({
                "id":inner.id,"method":inner.method,"params":inner.params,"sessionId":inner.session_id})).unwrap()}),
            session_id:envelope_session.map(str::to_owned) }
    }
    #[test]
    fn nested_session_precedence_and_each_response_layer_are_preserved() {
        let leaf=request(3,"Input.dispatchKeyEvent",Some("leaf-session"));
        let middle=wrapped(2,Some("middle-envelope"),Some("middle-route"),&leaf);
        let outer=wrapped(1,Some("outer-envelope"),Some("outer-route"),&middle);
        let plan=resolve(&outer);let effective=plan.effective.unwrap();
        assert_eq!(effective.session_id.as_deref(),Some("middle-route"));
        let (response,events)=wrap_response(CdpResponse::success(effective.id,json!({"delivered":true}),effective.session_id.clone()),plan.wrappers);
        assert_eq!(response.id,1);assert_eq!(response.session_id.as_deref(),Some("outer-envelope"));
        assert_eq!(events.len(),2);assert_eq!(events[0].session_id.as_deref(),Some("outer-route"));
        assert_eq!(events[0].params["sessionId"],"middle-route");
        let middle_ack:serde_json::Value=serde_json::from_str(events[1].params["message"].as_str().unwrap()).unwrap();
        assert_eq!(middle_ack["id"],2);assert_eq!(middle_ack["sessionId"],"outer-route");
    }
    #[test]
    fn envelope_session_is_not_implicitly_inherited_by_leaf() {
        let leaf=request(2,"Input.dispatchKeyEvent",Some("leaf-session"));
        let outer=wrapped(1,Some("envelope-only"),None,&leaf);
        assert_eq!(resolve(&outer).effective.unwrap().session_id.as_deref(),Some("leaf-session"));
    }
    #[test]
    fn malformed_nested_request_errors_at_its_own_layer() {
        let mut bad=request(2,"Target.sendMessageToTarget",Some("bad-envelope"));bad.params=json!({"message":"{bad"});
        let outer=wrapped(1,None,Some("route"),&bad);let plan=resolve(&outer);
        let failure=plan.effective.unwrap_err();assert_eq!(failure.id,2);assert_eq!(failure.error.as_ref().unwrap().code,-32700);
        let (response,events)=wrap_response(failure,plan.wrappers);assert!(response.error.is_none());assert_eq!(response.id,1);assert_eq!(events.len(),1);
    }
    #[test]
    fn depth_message_and_cumulative_work_limits_precede_leaf_execution() {
        let inner=wrapped(2,None,Some("route"),&request(3,"Input.dispatchKeyEvent",None));
        let outer=wrapped(1,None,Some("route"),&inner);
        let depth=resolve_bounded(&outer,1,MAX_MESSAGE_BYTES,MAX_DECODED_BYTES);
        assert_eq!(depth.effective.unwrap_err().id,2);
        assert!(resolve_bounded(&outer,MAX_WRAPPER_DEPTH,1,MAX_DECODED_BYTES).effective.is_err());
        let first=outer.params["message"].as_str().unwrap().len();
        assert_eq!(resolve_bounded(&outer,MAX_WRAPPER_DEPTH,MAX_MESSAGE_BYTES,first).effective.unwrap_err().id,2);
        assert!(resolve(&outer).effective.is_ok());
    }
}
