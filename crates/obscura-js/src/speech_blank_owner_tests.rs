// Source-only discriminating tests for the existing opaque BlankOwner lifetime.
use super::*;

fn state() -> SharedState {
    Rc::new(RefCell::new(ObscuraState::new(obscura_net::EffectivePersona::builtin(
        obscura_net::StealthProfile::MacChrome153,
    ))))
}
fn blank(state: &SharedState, parent: Option<&BlankOwner>) -> BlankOwner {
    let host_owner = state.borrow().websocket_owner.clone();
    let actual = Owner::child(&parent.map_or_else(||host_owner.clone(), |p|p.policy.borrow().owner.clone()));
    BlankOwner::new(state, host_owner,
        Policy { document: "about:blank".into(), site: None,
            csp: Default::default(), known: true, owner: actual, blocked: Vec::new(), headers: Vec::new() },
        "about:blank".into(), NodeId::new(1), 1)
}

#[test]
fn speech_blank_original_lease_recursive_retirement_preserves_sibling_and_host_ws() {
    let state = state();
    let host_ws_lease = state.borrow().websocket_owner.clone();
    let first = blank(&state, None);
    let descendant = blank(&state, Some(&first));
    let sibling = blank(&state, None);
    let old_cap = first.speech_owner().clone();
    let descendant_cap = descendant.speech_owner().clone();
    assert!(old_cap.active() && descendant_cap.active() && sibling.speech_owner().active());
    drop(old_cap.clone());
    assert!(first.policy.borrow().owner.active());
    first.retire();
    assert!(!old_cap.active() && !descendant_cap.active());
    assert!(host_ws_lease.active() && sibling.speech_owner().active());
    assert!(sibling.policy.borrow().owner.active());
    let replacement = blank(&state, None);
    assert!(replacement.speech_owner().active());
    assert!(!old_cap.active());
    // Dropping Speech metadata never calls Owner::retire on the sibling/host.
    drop(old_cap);
    assert!(host_ws_lease.active() && sibling.speech_owner().active());
    host_ws_lease.retire();
    assert!(!sibling.speech_owner().active());
    assert!(!replacement.speech_owner().active());
}

#[test]
fn speech_blank_metadata_keeps_brand_after_original_blank_drop_but_not_permission() {
    let state = state();
    let original = blank(&state, None);
    let retained = original.speech_owner().clone();
    assert_eq!(Rc::strong_count(&state), 1);
    assert!(retained.active());
    drop(original);
    assert!(!retained.active());
    assert_eq!(deno_core::cppgc::GarbageCollected::get_name(&retained), c"NativeSpeechOwnerCapability");
    assert!(state.borrow().websocket_owner.active());
}
