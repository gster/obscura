//! Host-issued document authority for future native Speech work.
//!
//! No JS mint op, V8 roots, callbacks, queues or provider are installed here.
//! The WS-named Owner is the existing document retirement lease, not a new
//! Speech lifetime. Metadata/brands survive retirement; permission does not.
use std::cell::RefCell;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use deno_core::v8;
use obscura_net::websocket::session::Owner;
use crate::ops::{ObscuraState, SharedState};

#[derive(Clone)]
pub(crate) struct NativeSpeechOwnerCapability {
    state: Weak<RefCell<ObscuraState>>,
    host_owner: Arc<Owner>,
    actual_owner: Arc<Owner>,
    inventory_slots: Arc<tokio::sync::Semaphore>,
}

impl deno_core::cppgc::GarbageCollected for NativeSpeechOwnerCapability {
    fn get_name(&self) -> &'static std::ffi::CStr { c"NativeSpeechOwnerCapability" }
}

impl NativeSpeechOwnerCapability {
    // Rust host initialization only. The arguments must be captured together
    // while the host owns this State, before any author code for the document.
    // Do not expose either constructor through op2 or a JS callback.
    pub(crate) fn document(state: &SharedState, owner: &Arc<Owner>) -> Self {
        Self::blank(state, owner, owner)
    }

    // Called at creation of the original opaque BlankOwner, with its already
    // allocated child lease. Never reconstruct from nid/frameId/current global.
    pub(crate) fn blank(state: &SharedState, host_owner: &Arc<Owner>, actual_owner: &Arc<Owner>) -> Self {
        Self { state: Rc::downgrade(state), host_owner: host_owner.clone(), actual_owner: actual_owner.clone(), inventory_slots: Arc::new(tokio::sync::Semaphore::new(64)) }
    }

    pub(crate) fn inventory_permit(&self) -> Result<tokio::sync::OwnedSemaphorePermit, crate::speech_provider::ProviderError> {
        self.inventory_slots.clone().try_acquire_owned().map_err(|_| crate::speech_provider::ProviderError::RequestLimit)
    }

    pub(crate) fn inventory_retirement_signals(&self) -> (tokio::sync::watch::Receiver<bool>, tokio::sync::watch::Receiver<bool>) {
        (self.host_owner.subscribe(), self.actual_owner.subscribe())
    }

    pub(crate) fn same_document(&self, other: &Self) -> bool {
        self.state.ptr_eq(&other.state)
            && Arc::ptr_eq(&self.host_owner, &other.host_owner)
            && Arc::ptr_eq(&self.actual_owner, &other.actual_owner)
    }

    pub(crate) fn active(&self) -> bool {
        let Some(state) = self.state.upgrade() else { return false; };
        // Reentrant borrows fail closed, without changing document lifetime.
        let Ok(state) = state.try_borrow() else { return false; };
        Arc::ptr_eq(&state.websocket_owner, &self.host_owner)
            && self.host_owner.active() && self.actual_owner.active()
    }

    // Host-only cppgc wrapper. The caller must supply an initialized deno_core
    // JsRuntime scope: make_cppgc_object relies on its CppHeap/template store.
    // This is not a JS handoff. Future bindings must receive the ORIGINAL
    // capability before author execution and fail closed if that handoff is
    // absent; they must not recover it by reading a later current State.
    // Wrapping an inactive capability deliberately preserves its native brand.
    pub(crate) fn wrap_for_host<'s>(&self, scope: &mut v8::HandleScope<'s>) -> v8::Local<'s, v8::Object> {
        deno_core::cppgc::make_cppgc_object(scope, self.clone())
    }
}

// Only main/loaded host construction calls this. set_dom captures its new
// lease while already holding the State borrow, after retiring the old lease.
pub(crate) fn issue_document_owner(state: &SharedState) {
    let mut document = state.borrow_mut();
    document.speech_owner = Some(NativeSpeechOwnerCapability::document(state, &document.websocket_owner));
}

// No Drop: dropping metadata must not retire a lease shared with WebSocket,
// sibling services or descendant documents. No Rc<State> or V8 Global is held.

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> SharedState {
        Rc::new(RefCell::new(ObscuraState::new(obscura_net::EffectivePersona::builtin(
            obscura_net::StealthProfile::MacChrome153,
        ))))
    }
    fn capability(state: &SharedState) -> NativeSpeechOwnerCapability {
        state.borrow().speech_owner.as_ref().unwrap().clone()
    }

    #[test]
    fn speech_owner_retirement_does_not_erase_metadata_or_revive_on_reissue() {
        let state = state();
        issue_document_owner(&state);
        let old = capability(&state);
        assert!(old.active());
        crate::ops::retire_document_referrer(&mut state.borrow_mut());
        // Referrer metadata can be author-minted again. It conveys no speech authority.
        state.borrow_mut().document_referrer_record = Some(Rc::new(()));
        issue_document_owner(&state);
        assert!(!old.active());
        assert!(!capability(&state).active());
        assert_eq!(deno_core::cppgc::GarbageCollected::get_name(&old), c"NativeSpeechOwnerCapability");
    }

    #[test]
    fn speech_owner_original_state_identity_cannot_authorize_reset_host_owner() {
        let state = state();
        issue_document_owner(&state);
        let old = capability(&state);
        let original_host = state.borrow().websocket_owner.clone();
        // Intentionally leave old lease active to discriminate the pointer check
        // from a test that passes only because Owner::retire was called.
        state.borrow_mut().websocket_owner = Arc::new(Owner::new(original_host.budget.clone()));
        issue_document_owner(&state);
        assert!(original_host.active());
        assert!(!old.active());
        assert!(capability(&state).active());
        assert!(old.state.ptr_eq(&Rc::downgrade(&state)));
    }

    #[test]
    fn speech_owner_metadata_does_not_keep_state_alive_or_retire_shared_owner() {
        let state = state();
        issue_document_owner(&state);
        let owner = state.borrow().websocket_owner.clone();
        let weak = Rc::downgrade(&state);
        let cap = capability(&state);
        assert_eq!(Rc::strong_count(&state), 1);
        drop(cap.clone());
        assert!(owner.active());
        assert!(cap.active());
        drop(state);
        assert!(weak.upgrade().is_none());
        assert!(!cap.active());
        // State Drop, not capability Drop, owns document retirement.
        assert!(!owner.active());
    }

    #[test]
    fn speech_owner_reentrant_borrow_fails_closed_without_retirement() {
        let state = state();
        issue_document_owner(&state);
        let cap = capability(&state);
        let guard = state.borrow_mut();
        assert!(!cap.active());
        assert!(guard.websocket_owner.active());
        drop(guard);
        assert!(cap.active());
    }

    #[test]
    fn speech_owner_main_replacement_retires_original_before_host_reissue() {
        let runtime = crate::runtime::ObscuraJsRuntime::new(obscura_net::EffectivePersona::builtin(
            obscura_net::StealthProfile::MacChrome153,
        ));
        let old = capability(&runtime.state);
        assert!(old.active());
        runtime.set_dom(obscura_dom::parse_html("<html><body>new</body></html>"));
        let new = capability(&runtime.state);
        assert!(!old.active());
        assert!(new.active());
        assert!(!Arc::ptr_eq(&old.actual_owner, &new.actual_owner));
        // Even malicious restoration of the old host pointer cannot revive the
        // actual old lease retired by the real replacement boundary.
        runtime.state.borrow_mut().websocket_owner = old.host_owner.clone();
        assert!(!old.active());
        assert!(!new.active());
    }

    #[test]
    fn speech_owner_cppgc_brand_survives_retirement_and_plain_object_is_rejected() {
        let mut runtime = crate::runtime::ObscuraJsRuntime::new(obscura_net::EffectivePersona::builtin(
            obscura_net::StealthProfile::MacChrome153,
        ));
        let cap = capability(&runtime.state);
        let state = runtime.state.clone();
        let mut entered = runtime.runtime();
        let context = entered.main_context();
        let scope = &mut v8::HandleScope::with_context(entered.v8_isolate(), context);
        let object = cap.wrap_for_host(scope);
        let plain = v8::Object::new(scope);
        assert!(deno_core::cppgc::try_unwrap_cppgc_object::<NativeSpeechOwnerCapability>(scope, plain.into()).is_none());
        let original = deno_core::cppgc::try_unwrap_cppgc_object::<NativeSpeechOwnerCapability>(scope, object.into()).unwrap();
        assert!(original.active());
        crate::ops::retire_document_referrer(&mut state.borrow_mut());
        let retained = deno_core::cppgc::try_unwrap_cppgc_object::<NativeSpeechOwnerCapability>(scope, object.into()).unwrap();
        assert!(!retained.active());
        assert!(Arc::ptr_eq(&original.actual_owner, &retained.actual_owner));
    }

    #[test]
    fn speech_owner_loaded_frame_is_issued_before_init_and_retired_by_frame_drop() {
        let mut runtime = crate::runtime::ObscuraJsRuntime::new(obscura_net::EffectivePersona::builtin(
            obscura_net::StealthProfile::MacChrome153,
        ));
        runtime.set_dom(obscura_dom::parse_html("<html><body></body></html>"));
        runtime.set_url("https://example.com/main");
        runtime.run_page_init();
        let parent = capability(&runtime.state);
        let frame = crate::frame::FrameRealm::new(&mut runtime, 91, 0,
            "https://example.com/child", "<html><body>child</body></html>").unwrap();
        let child_state = runtime.realm_states().borrow().by_frame_id(91).unwrap();
        let child = capability(&child_state);
        assert!(parent.active() && child.active());
        assert!(!Arc::ptr_eq(&parent.actual_owner, &child.actual_owner));
        drop(frame);
        assert!(!child.active());
        assert!(parent.active());
        // Retain old State to distinguish synchronous lease retirement from weak death.
        assert!(child.state.upgrade().is_some());
    }
}
