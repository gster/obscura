//! Host-only handoff to a genuine async op. Original capability is V8-traced data.
use deno_core::{op2, v8};
use deno_error::JsErrorBox;
use crate::speech_owner::NativeSpeechOwnerCapability;
use crate::speech_protocol::WebVoice;
use crate::speech_provider::{Provider, ProviderError};
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

#[derive(Debug)]
pub(crate) enum InventoryInstallError { AlreadyCaptured, MissingOp, AllocationOrException }
struct CapturedRequestOp(v8::Global<v8::Function>);

/// Called exactly once from trusted take_ops_handoff before any author script.
pub(crate) fn capture_request_op(scope: &mut v8::HandleScope, original_ops: v8::Local<v8::Object>) -> Result<(), InventoryInstallError> {
    if scope.get_slot::<CapturedRequestOp>().is_some() { return Err(InventoryInstallError::AlreadyCaptured); }
    let key = v8::String::new(scope, "op_speech_request_inventory").ok_or(InventoryInstallError::AllocationOrException)?;
    let value = original_ops.get(scope, key.into()).ok_or(InventoryInstallError::MissingOp)?;
    let function = v8::Local::<v8::Function>::try_from(value).map_err(|_| InventoryInstallError::MissingOp)?;
    let captured = v8::Global::new(scope, function);
    scope.set_slot(CapturedRequestOp(captured));
    Ok(())
}
/// Host calls while the isolate is still alive, before JsRuntime disposal.
pub(crate) fn clear_request_op(isolate: &mut v8::Isolate) { isolate.remove_slot::<CapturedRequestOp>(); }

fn invoke(scope: &mut v8::HandleScope, args: v8::FunctionCallbackArguments, mut result: v8::ReturnValue) {
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let scope = &mut v8::TryCatch::new(scope);
        let run = |scope: &mut v8::HandleScope| -> Option<v8::Global<v8::Value>> {
            let data = v8::Local::<v8::Array>::try_from(args.data()).ok()?;
            let function = v8::Local::<v8::Function>::try_from(data.get_index(scope, 0)?).ok()?;
            let original = data.get_index(scope, 1)?;
            let receiver = v8::undefined(scope);
            let after = if args.length() == 0 { v8::Integer::new(scope, -1).into() } else { args.get(0) };
            let promise = function.call(scope, receiver.into(), &[original, after])?;
            Some(v8::Global::new(scope, promise))
        };
        let value = run(scope);
        if scope.has_caught() { scope.rethrow(); return; }
        if let Some(value) = value { result.set(v8::Local::new(scope, value)); }
    }));
    if outcome.is_err() && !scope.is_execution_terminating() {
        if let Some(message) = v8::String::new(scope, "Speech inventory callback panicked") {
            let error = v8::Exception::error(scope, message); scope.throw_exception(error);
        }
    }
}

pub(crate) fn request_function<'s>(scope: &mut v8::HandleScope<'s>, owner: &NativeSpeechOwnerCapability) -> Result<v8::Local<'s, v8::Function>, InventoryInstallError> {
    let captured = scope.get_slot::<CapturedRequestOp>().map(|v| v.0.clone()).ok_or(InventoryInstallError::MissingOp)?;
    let function = v8::Local::new(scope, captured);
    let original = owner.wrap_for_host(scope);
    let data = v8::Array::new_with_elements(scope, &[function.into(), original.into()]);
    if data.set_integrity_level(scope, v8::IntegrityLevel::Frozen) != Some(true) { return Err(InventoryInstallError::AllocationOrException); }
    v8::Function::builder(invoke).data(data.into()).length(0)
        .constructor_behavior(v8::ConstructorBehavior::Throw).build(scope)
        .ok_or(InventoryInstallError::AllocationOrException)
}

// Catch every async poll, not merely future construction, before returning to V8.
struct PanicGuard<F: Future>(Pin<Box<F>>);
impl<F: Future> Future for PanicGuard<F> {
    type Output = Result<F::Output, ProviderError>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.0.as_mut().poll(cx))) {
            Ok(Poll::Ready(value)) => Poll::Ready(Ok(value)),
            Ok(Poll::Pending) => Poll::Pending,
            Err(_) => Poll::Ready(Err(ProviderError::Panicked)),
        }
    }
}

pub(crate) async fn request(owner: &NativeSpeechOwnerCapability) -> Result<Vec<WebVoice>, ProviderError> {
    if !owner.active() { return Err(ProviderError::OwnerRetired); }
    let permit = owner.inventory_permit()?;
    request_admitted(owner, Provider::shared(), permit).await
}

async fn request_admitted(owner: &NativeSpeechOwnerCapability, provider: Provider, _owner_permit: tokio::sync::OwnedSemaphorePermit) -> Result<Vec<WebVoice>, ProviderError> {
    if !owner.active() { return Err(ProviderError::OwnerRetired); }
    let (mut host_cancel, mut actual_cancel) = owner.inventory_retirement_signals();
    let mut subscription = provider.subscribe()?;
    if !owner.active() { return Err(ProviderError::OwnerRetired); }
    let snapshot = tokio::select! {
        biased;
        _ = host_cancel.changed() => return Err(ProviderError::OwnerRetired),
        _ = actual_cancel.changed() => return Err(ProviderError::OwnerRetired),
        result = subscription.ready() => result?,
    };
    // No strong State/RefCell borrow crossed await; this checks original identity.
    if !owner.active() { return Err(ProviderError::OwnerRetired); }
    Ok(snapshot.voices.iter().map(|voice| voice.web.clone()).collect())
}

#[derive(serde::Serialize)]
pub(crate) struct WebDelivery {
    pub revision: i32,
    pub voices: Option<Vec<WebVoice>>,
    pub done: bool,
}
pub(crate) async fn request_stream(owner: &NativeSpeechOwnerCapability, after: i32) -> Result<WebDelivery, ProviderError> {
    if !owner.active() { return Err(ProviderError::OwnerRetired); }
    let permit = owner.inventory_permit()?;
    request_stream_admitted(owner, Provider::shared(), permit, after).await
}
async fn request_stream_admitted(owner: &NativeSpeechOwnerCapability, provider: Provider,
    _owner_permit: tokio::sync::OwnedSemaphorePermit, after: i32) -> Result<WebDelivery, ProviderError> {
    if !owner.active() { return Err(ProviderError::OwnerRetired); }
    let (mut host_cancel, mut actual_cancel) = owner.inventory_retirement_signals();
    let mut subscription = provider.subscribe_after(after)?;
    if !owner.active() { return Err(ProviderError::OwnerRetired); }
    let delivery = tokio::select! {
        biased;
        _ = host_cancel.changed() => return Err(ProviderError::OwnerRetired),
        _ = actual_cancel.changed() => return Err(ProviderError::OwnerRetired),
        result = subscription.next() => result?,
    };
    if !owner.active() { return Err(ProviderError::OwnerRetired); }
    Ok(WebDelivery { revision: delivery.revision, done: delivery.done,
        voices: delivery.snapshot.map(|value| value.voices.iter().map(|voice| voice.web.clone()).collect()) })
}
#[op2(async)]
#[serde]
pub(crate) async fn op_speech_request_inventory(#[cppgc] original: &NativeSpeechOwnerCapability, #[smi] after: i32) -> Result<WebDelivery, JsErrorBox> {
    PanicGuard(Box::pin(request_stream(original, after))).await.and_then(|value| value)
        .map_err(|error| JsErrorBox::generic(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::{ObscuraState, SharedState};
    use std::{cell::RefCell, rc::Rc};
    fn owner() -> (SharedState, NativeSpeechOwnerCapability) {
        let state = Rc::new(RefCell::new(ObscuraState::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::MacChrome153))));
        crate::speech_owner::issue_document_owner(&state);
        let owner = state.borrow().speech_owner.as_ref().unwrap().clone(); (state, owner)
    }
    #[test]
    fn speech_inventory_per_owner_limit_is_shared_by_original_capability_clones() {
        let (_state, owner) = owner(); let clone = owner.clone();
        let mut permits: Vec<_> = (0..64).map(|_| owner.inventory_permit().unwrap()).collect();
        assert!(matches!(clone.inventory_permit(), Err(ProviderError::RequestLimit)));
        permits.pop(); assert!(clone.inventory_permit().is_ok());
    }
    #[tokio::test(flavor = "current_thread")]
    async fn speech_inventory_retired_owner_fails_before_starting_provider_and_keeps_no_state_root() {
        let (state, owner) = owner(); let weak = Rc::downgrade(&state);
        let (host, actual) = owner.inventory_retirement_signals();
        crate::ops::retire_document_referrer(&mut state.borrow_mut());
        assert!(*host.borrow() && *actual.borrow());
        assert!(matches!(request(&owner).await, Err(ProviderError::OwnerRetired)));
        drop(state); assert!(weak.upgrade().is_none());
    }
    #[tokio::test(flavor = "current_thread")]
    async fn speech_inventory_retirement_wins_against_late_real_protocol_delivery() {
        let (state, owner) = owner();
        let (provider, sender) = Provider::controlled();
        let permit = owner.inventory_permit().unwrap();
        let future = request_admitted(&owner, provider, permit); tokio::pin!(future);
        std::future::poll_fn(|cx| {
            assert!(matches!(future.as_mut().poll(cx), Poll::Pending)); Poll::Ready(())
        }).await;
        assert_eq!(Rc::strong_count(&state), 1);
        crate::ops::retire_document_referrer(&mut state.borrow_mut());
        let snapshot = crate::speech_protocol::decode(include_bytes!("testdata/speech_native_protocol.json")).unwrap();
        sender.send_replace(crate::speech_provider::InventoryState::Ready(snapshot));
        assert!(matches!(future.await, Err(ProviderError::OwnerRetired)));
        let permits: Vec<_> = (0..64).map(|_| owner.inventory_permit().unwrap()).collect();
        assert_eq!(permits.len(), 64);
    }
    #[tokio::test(flavor = "current_thread")]
    async fn speech_inventory_stream_retirement_cancels_next_without_erasing_first() {
        let (state, owner) = owner();
        let (provider, sender) = Provider::controlled();
        let snapshot = crate::speech_protocol::decode(include_bytes!("testdata/speech_native_protocol.json")).unwrap();
        sender.send_replace(crate::speech_provider::InventoryState::Ready(snapshot));
        let first = request_stream_admitted(&owner, provider.clone(), owner.inventory_permit().unwrap(), -1).await.unwrap();
        assert!(first.voices.is_some());
        sender.send_replace(crate::speech_provider::InventoryState::Pending);
        let future = request_stream_admitted(&owner, provider, owner.inventory_permit().unwrap(), 0); tokio::pin!(future);
        std::future::poll_fn(|cx| { assert!(matches!(future.as_mut().poll(cx), Poll::Pending)); Poll::Ready(()) }).await;
        assert_eq!(Rc::strong_count(&state), 1);
        crate::ops::retire_document_referrer(&mut state.borrow_mut());
        assert!(matches!(future.await, Err(ProviderError::OwnerRetired)));
        assert!(first.voices.is_some());
        let permits: Vec<_> = (0..64).map(|_| owner.inventory_permit().unwrap()).collect();
        assert_eq!(permits.len(), 64);
    }
    #[tokio::test(flavor = "current_thread")]
    async fn speech_inventory_catches_panics_after_await() {
        let guarded = PanicGuard(Box::pin(async { tokio::task::yield_now().await; panic!("private injected async panic"); }));
        assert!(matches!(guarded.await, Err(ProviderError::Panicked)));
    }
    #[test]
    fn speech_inventory_request_callable_pins_original_capability_and_op_function() {
        let mut runtime = crate::runtime::ObscuraJsRuntime::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::MacChrome153));
        let owner = runtime.state.borrow().speech_owner.as_ref().unwrap().clone();
        let mut entered = runtime.runtime();
        clear_request_op(entered.v8_isolate());
        let context = entered.main_context();
        let scope = &mut v8::HandleScope::with_context(entered.v8_isolate(), context);
        assert!(matches!(request_function(scope, &owner), Err(InventoryInstallError::MissingOp)));
        fn private_probe(scope: &mut v8::HandleScope, args: v8::FunctionCallbackArguments, mut result: v8::ReturnValue) {
            let cap = deno_core::cppgc::try_unwrap_cppgc_object::<NativeSpeechOwnerCapability>(scope, args.get(0));
            result.set_bool(cap.is_some_and(|cap| cap.active()));
        }
        let op = v8::Function::builder(private_probe).build(scope).unwrap();
        let ops = v8::Object::new(scope);
        let key = v8::String::new(scope, "op_speech_request_inventory").unwrap();
        ops.set(scope, key.into(), op.into());
        capture_request_op(scope, ops).unwrap();
        assert!(matches!(capture_request_op(scope, ops), Err(InventoryInstallError::AlreadyCaptured)));
        let request = request_function(scope, &owner).unwrap();
        let forged = v8::Object::new(scope);
        let undefined = v8::undefined(scope);
        // Replacing the old ops property and passing a forged arg cannot redirect
        // the captured function or replace the capability in its frozen data.
        ops.set(scope, key.into(), undefined.into());
        assert!(request.call(scope, forged.into(), &[forged.into()]).unwrap().is_true());
        clear_request_op(scope);
    }
}
