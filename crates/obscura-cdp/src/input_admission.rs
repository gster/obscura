//! Connection-local publication of real native keyboard input.
//! Lock order: routing -> hub -> inbound. No V8 or author work under these locks.
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex};
use obscura_js::pending_input::{InputHub, InputOwner, InputTicket, EntryId};
use obscura_browser::KeyboardInput;
use crate::{dispatch::CdpContext, inbound, server::{CdpMessage, ServerMessage}, types::CdpRequest};

#[derive(Clone, Default)]
pub(crate) struct Admission(Arc<Inner>);
#[derive(Default)]
struct Inner { hub: InputHub, routing: Mutex<Routing> }
#[derive(Default)]
struct Routing {
    next: u64,
    closed: bool,
    routes: HashMap<String, (InputOwner, bool)>,
    barriers: BTreeSet<u64>,
    waiting: BTreeMap<u64, (EntryId, Option<String>)>,
}
pub(crate) struct Published {
    #[cfg(test)]
    pub request_id: Option<u64>,
    #[cfg(test)]
    pub waiting_input: bool,
}
pub(crate) struct KeyboardAdmission { pub input: KeyboardInput, pub ticket: InputTicket }
pub(crate) struct Receipt {
    admission: Admission,
    sequence: u64,
    unfinished: bool,
    pub keyboard: Option<KeyboardAdmission>,
}
fn barrier(method: &str) -> bool {
    matches!(method, "Input.setIgnoreInputEvents" | "Target.attachToTarget" | "Target.detachFromTarget"
        | "Target.createTarget" | "Target.closeTarget" | "Target.disposeBrowserContext"
        | "Page.navigate" | "Page.reload" | "Page.navigateToHistoryEntry" | "Page.setDocumentContent")
}
impl Admission {
    fn promote(routing: &mut Routing, tx: &mut obscura_js::pending_input::Transaction<'_>) {
        let ready: Vec<_> = routing.waiting.keys().copied()
            .filter(|seq| routing.barriers.range(..*seq).next().is_none()).collect();
        for seq in ready {
            if let Some((id, session)) = routing.waiting.remove(&seq) {
                if !tx.contains(id) { continue; }
                match session.as_ref().and_then(|session| routing.routes.get(session)) {
                    Some((owner, false)) => tx.promote(id, owner.id()),
                    _ => tx.cancel(id),
                }
            }
        }
    }
    pub fn send(&self, sender: &inbound::Sender<ServerMessage>, mut message: CdpMessage)
        -> Result<Published, inbound::SendError<ServerMessage>> {
        let parsed = serde_json::from_str::<CdpRequest>(&message.text).ok();
        let canonical = parsed.as_ref().map(crate::canonical_request::resolve);
        let effective = canonical.as_ref().and_then(|plan| plan.effective.as_ref().ok());
        let input = effective.filter(|request| request.method == "Input.dispatchKeyEvent")
            .and_then(|request| crate::domains::input::keyboard_input(&request.params).ok())
            .filter(|input| input.validate().is_ok());
        let is_barrier = effective.is_some_and(|request| barrier(&request.method));
        let session = effective.and_then(|request| request.session_id.clone());
        let mut routing = self.0.routing.lock().unwrap_or_else(|error| error.into_inner());
        routing.next = routing.next.checked_add(1).expect("CDP receipt sequence exhausted");
        let sequence = routing.next;
        let mut tx = self.0.hub.transaction();
        if is_barrier { routing.barriers.insert(sequence); }
        let keyboard = if cfg!(feature = "render") && !routing.closed {
            input.map(|input| {
                let ticket = self.0.hub.ticket(&mut tx, None);
                routing.waiting.insert(sequence, (ticket.id(), session));
                Self::promote(&mut routing, &mut tx);
                KeyboardAdmission { input, ticket }
            })
        } else { None };
        let published = Published {
            #[cfg(test)]
            request_id: parsed.as_ref().map(|request| request.id),
            #[cfg(test)]
            waiting_input: keyboard.is_some() && routing.waiting.contains_key(&sequence),
        };
        message.admission = Some(Receipt { admission: self.clone(), sequence, unfinished: true, keyboard });
        let outcome = sender.send(ServerMessage::Cdp(message));
        match outcome {
            Ok(()) => { drop(tx); drop(routing); Ok(published) }
            Err(error) => {
                let reason = error.reason;
                let mut returned = error.into_message();
                if let ServerMessage::Cdp(message) = &mut returned {
                    if let Some(receipt) = &mut message.admission {
                        if let Some(keyboard) = &mut receipt.keyboard { keyboard.ticket.cancel_locked(&mut tx); }
                        receipt.unfinished = false;
                    }
                }
                routing.waiting.remove(&sequence); routing.barriers.remove(&sequence);
                Self::promote(&mut routing, &mut tx);
                drop(tx); drop(routing);
                // Returned message owns disarmed tickets and is dropped outside all guards.
                Err(inbound::SendError::from_message(returned, reason))
            }
        }
    }
    pub fn refresh(&self, ctx: &CdpContext) {
        // Obtain only native handles; no JsRuntime evaluation or lazy resume.
        let routes: HashMap<_, _> = ctx.sessions.iter().filter_map(|(session, page_id)| {
            let page = ctx.get_page(page_id)?; let owner = page.scheduling_owner()?;
            // Called only on processor boundaries with no navigation task active.
            owner.navigation_pending(page.scheduling_navigation_pending());
            let ignored = ctx.mouse_and_key_input_ignored(&Some(session.clone())).unwrap_or(true);
            Some((session.clone(), (owner, ignored)))
        }).collect();
        let mut routing = self.0.routing.lock().unwrap_or_else(|error| error.into_inner());
        for (owner, _) in routes.values() { owner.bind(&self.0.hub); }
        routing.routes = routes;
        let mut tx = self.0.hub.transaction(); Self::promote(&mut routing, &mut tx);
    }
    fn finish(&self, sequence: u64) {
        let mut routing = self.0.routing.lock().unwrap_or_else(|error| error.into_inner());
        routing.barriers.remove(&sequence); routing.waiting.remove(&sequence);
        let mut tx = self.0.hub.transaction(); Self::promote(&mut routing, &mut tx);
    }
    pub fn close(&self) {
        let mut routing = self.0.routing.lock().unwrap_or_else(|error| error.into_inner());
        routing.closed = true; routing.routes.clear(); routing.waiting.clear(); routing.barriers.clear();
        self.0.hub.close();
    }
}
impl Receipt {
    pub fn complete(mut self, ctx: &CdpContext) {
        self.admission.refresh(ctx);
        self.admission.finish(self.sequence); self.unfinished = false;
    }
}
impl Drop for Receipt {
    fn drop(&mut self) {
        if self.unfinished { self.admission.finish(self.sequence); }
        // Keyboard/ticket fields are dropped after finish releases all locks.
    }
}
pub(crate) struct ConnectionGuard(pub Admission);
impl Drop for ConnectionGuard { fn drop(&mut self) { self.0.close(); } }


#[cfg(test)]
mod tests {
    use super::*;
    fn message(text: &str) -> CdpMessage {
        let (reply_tx, _rx, _usage) = crate::outbound::channel();
        CdpMessage { text:text.into(),reply_tx,admission:None }
    }
    #[test]
    fn failed_queue_publication_rolls_back_without_drop_reentry() {
        let admission=Admission::default();let owner=InputOwner::default();owner.bind(&admission.0.hub);
        admission.0.routing.lock().unwrap().routes.insert("s".into(),(owner.clone(),false));
        let (sender,receiver)=inbound::channel::<ServerMessage>();drop(receiver);
        let result=admission.send(&sender,message(r#"{"id":1,"method":"Input.dispatchKeyEvent","sessionId":"s","params":{"type":"rawKeyDown","key":"x"}}"#));
        assert!(result.is_err());assert!(!owner.pending());drop(result);assert!(!owner.pending());
    }
    #[cfg(feature = "render")]
    #[test]
    fn unfinished_barrier_blocks_then_promotes_before_dispatch() {
        let admission=Admission::default();let owner=InputOwner::default();owner.bind(&admission.0.hub);
        admission.0.routing.lock().unwrap().routes.insert("s".into(),(owner.clone(),false));
        let (sender,mut receiver)=inbound::channel::<ServerMessage>();
        admission.send(&sender,message(r#"{"id":1,"method":"Input.setIgnoreInputEvents","sessionId":"s","params":{"ignore":false}}"#)).unwrap();
        admission.send(&sender,message(r#"{"id":2,"method":"Input.dispatchKeyEvent","sessionId":"s","params":{"type":"rawKeyDown","key":"x"}}"#)).unwrap();
        assert!(!owner.pending());
        // Dropping an unapplied barrier resolves it as no state change.
        drop(receiver.try_recv().unwrap());assert!(owner.pending());
        drop(receiver.try_recv().unwrap());assert!(!owner.pending());
        assert!(admission.0.routing.lock().unwrap().waiting.is_empty());
    }
    #[cfg(not(feature = "render"))]
    #[test]
    fn no_render_never_registers_keyboard() {
        let admission=Admission::default();let owner=InputOwner::default();owner.bind(&admission.0.hub);
        admission.0.routing.lock().unwrap().routes.insert("s".into(),(owner.clone(),false));
        let (sender,mut receiver)=inbound::channel::<ServerMessage>();
        admission.send(&sender,message(r#"{"id":1,"method":"Input.dispatchKeyEvent","sessionId":"s","params":{"type":"rawKeyDown"}}"#)).unwrap();
        assert!(!owner.pending());drop(receiver.try_recv().unwrap());
    }
    #[cfg(feature = "render")]
    #[test]
    fn canonical_wrapped_key_and_malformed_wrapper_match_admission() {
        let admission=Admission::default();let owner=InputOwner::default();owner.bind(&admission.0.hub);
        admission.0.routing.lock().unwrap().routes.insert("right".into(),(owner.clone(),false));
        let (sender,mut receiver)=inbound::channel::<ServerMessage>();
        let inner=serde_json::json!({"id":2,"method":"Input.dispatchKeyEvent","sessionId":"wrong","params":{"type":"rawKeyDown","key":"x"}});
        let outer=serde_json::json!({"id":1,"method":"Target.sendMessageToTarget","params":{"sessionId":"right","message":inner.to_string()}});
        admission.send(&sender,message(&outer.to_string())).unwrap();assert!(owner.pending());
        drop(receiver.try_recv().unwrap());assert!(!owner.pending());
        admission.send(&sender,message(r#"{"id":3,"method":"Target.sendMessageToTarget","params":{"sessionId":"right","message":"{invalid"}}"#)).unwrap();
        assert!(!owner.pending());drop(receiver.try_recv().unwrap());
    }
    #[cfg(feature = "render")]
    #[test]
    fn later_prefix_barrier_survives_earlier_and_unrelated_receipt_cleanup() {
        let admission=Admission::default();let owner=InputOwner::default();owner.bind(&admission.0.hub);
        admission.0.routing.lock().unwrap().routes.insert("s".into(),(owner.clone(),false));
        let (sender,mut receiver)=inbound::channel::<ServerMessage>();
        for text in [
            r#"{"id":1,"method":"Input.setIgnoreInputEvents","sessionId":"s","params":{"ignore":false}}"#,
            r#"{"id":2,"method":"Runtime.evaluate","sessionId":"s","params":{"expression":"1"}}"#,
            r#"{"id":3,"method":"Input.setIgnoreInputEvents","sessionId":"s","params":{"ignore":false}}"#,
            r#"{"id":4,"method":"Input.dispatchKeyEvent","sessionId":"s","params":{"type":"rawKeyDown"}}"#,
        ] {admission.send(&sender,message(text)).unwrap();}
        assert!(!owner.pending());drop(receiver.try_recv().unwrap());assert!(!owner.pending());
        drop(receiver.try_recv().unwrap());assert!(!owner.pending());
        drop(receiver.try_recv().unwrap());assert!(owner.pending());
        drop(receiver.try_recv().unwrap());assert!(!owner.pending());
    }

}
