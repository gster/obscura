//! Trusted startup only. There is no author-callable installer or owner mint op.
//! Main and loaded FrameRealm contexts have host-issued document capabilities.
//! Synthetic blank-window facades do not: this module never installs into them.
use deno_core::v8;
use crate::speech_native_bindings::{self as binding, PrivateTarget};
use crate::speech_owner::NativeSpeechOwnerCapability;
use std::panic::{catch_unwind, AssertUnwindSafe};

type Result<T> = std::result::Result<T, &'static str>;

fn key<'s>(scope: &mut v8::HandleScope<'s>, value: &str) -> Result<v8::Local<'s, v8::String>> {
    v8::String::new(scope, value).ok_or("speech string allocation")
}
fn get<'s>(scope: &mut v8::HandleScope<'s>, object: v8::Local<'s, v8::Object>, name: &str) -> Result<v8::Local<'s, v8::Value>> {
    let name = key(scope, name)?;
    object.get(scope, name.into()).ok_or("speech private field")
}
fn function<'s>(scope: &mut v8::HandleScope<'s>, object: v8::Local<'s, v8::Object>, name: &str) -> Result<v8::Local<'s, v8::Function>> {
    get(scope, object, name)?.try_into().map_err(|_| "speech private function")
}
fn object<'s>(scope: &mut v8::HandleScope<'s>, record: v8::Local<'s, v8::Object>, name: &str) -> Result<v8::Local<'s, v8::Object>> {
    get(scope, record, name)?.try_into().map_err(|_| "speech private object")
}
fn put(scope: &mut v8::HandleScope, object: v8::Local<v8::Object>, name: &str, value: v8::Local<v8::Value>, attributes: v8::PropertyAttribute) -> Result<()> {
    let name = key(scope, name)?;
    (object.define_own_property(scope, name.into(), value, attributes) == Some(true))
        .then_some(()).ok_or("speech property rejected")
}
fn throw_type(scope: &mut v8::HandleScope, message: &str) {
    if scope.is_execution_terminating() { return; }
    if let Some(message) = v8::String::new(scope, message) {
        let exception = v8::Exception::type_error(scope, message);
        scope.throw_exception(exception);
    }
}
fn boundary<'s>(scope: &mut v8::HandleScope<'s>, work: impl FnOnce(&mut v8::HandleScope<'s>)) {
    if catch_unwind(AssertUnwindSafe(|| work(scope))).is_err() {
        throw_type(scope, "Speech native callback panicked");
    }
}
fn owner_active(scope: &mut v8::HandleScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    boundary(scope, |scope| {
        let active = deno_core::cppgc::try_unwrap_cppgc_object::<NativeSpeechOwnerCapability>(scope, args.data())
            .is_some_and(|owner| owner.active());
        rv.set_bool(active);
    });
}
fn illegal_constructor(scope: &mut v8::HandleScope, _args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    boundary(scope, |scope| throw_type(scope, "Illegal constructor"));
}

// Window is intentionally separate from the private-object binder. Blink's
// generated getter uses info.This(), checks the Window receiver, and is a
// nonconstructible FunctionTemplate. V8 API receiver conversion maps null and
// undefined to the function realm's global proxy; primitives box and then fail
// this registry brand. Registered genuine globals only, never a blank facade.
fn window_getter(scope: &mut v8::HandleScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    boundary(scope, |scope| {
        if scope.is_execution_terminating() { return; }
        let scope = &mut v8::TryCatch::new(scope);
        let value = window_getter_checked(scope, &args);
        if scope.has_caught() { scope.rethrow(); return; }
        if let Some(value) = value { rv.set(value); }
    });
}
fn window_getter_checked<'s>(scope: &mut v8::HandleScope<'s>, args: &v8::FunctionCallbackArguments) -> Option<v8::Local<'s, v8::Value>> {
    let data = v8::Local::<v8::Array>::try_from(args.data()).ok()?;
    let brand = v8::Local::<v8::Function>::try_from(data.get_index(scope, 0)?).ok()?;
    let getter = v8::Local::<v8::Function>::try_from(data.get_index(scope, 1)?).ok()?;
    let receiver: v8::Local<v8::Value> = args.this().into();
    let undefined = v8::undefined(scope);
    if !brand.call(scope, undefined.into(), &[receiver])?.is_true() {
        throw_type(scope, "Illegal invocation"); return None;
    }
    getter.call(scope, receiver, &[])
}
fn constructor<'s>(scope: &mut v8::HandleScope<'s>, name: &str, prototype: v8::Local<'s, v8::Object>, parent: Option<v8::Local<'s, v8::Object>>) -> Result<v8::Local<'s, v8::Function>> {
    let ctor = v8::Function::builder(illegal_constructor).length(0)
        .constructor_behavior(v8::ConstructorBehavior::Allow).build(scope).ok_or("speech constructor")?;
    let name = key(scope, name)?; ctor.set_name(name);
    if let Some(parent) = parent {
        if ctor.set_prototype(scope, parent.into()) != Some(true) { return Err("speech constructor inheritance"); }
    }
    put(scope, ctor.into(), "prototype", prototype.into(), v8::PropertyAttribute::READ_ONLY | v8::PropertyAttribute::DONT_ENUM | v8::PropertyAttribute::DONT_DELETE)?;
    put(scope, prototype, "constructor", ctor.into(), v8::PropertyAttribute::DONT_ENUM)?;
    Ok(ctor)
}

/// All values are captured from trusted snapshot bootstrap, before author code.
/// The returned registry is held by the runtime for loaded contexts only. The
/// factory/registry are intentional isolate-lifetime roots; callbacks introduce
/// no additional Rust Global or strong Rc<State>.
pub(crate) fn install<'s>(
    scope: &mut v8::HandleScope<'s>,
    initializer: v8::Local<'s, v8::Function>,
    owner: &NativeSpeechOwnerCapability,
    shared_registry: Option<v8::Local<'s, v8::Value>>,
) -> Result<v8::Local<'s, v8::Value>> {
    if !owner.active() { return Err("speech original document inactive"); }
    let original = owner.wrap_for_host(scope);
    let active = v8::Function::builder(owner_active).data(original.into()).length(0)
        .constructor_behavior(v8::ConstructorBehavior::Throw).build(scope).ok_or("speech lease callback")?;
    let request = crate::speech_inventory::request_function(scope, owner).map_err(|_| "speech inventory callback")?;
    let config = v8::Object::new(scope);
    put(scope, config, "opaqueOriginalCapability", original.into(), v8::PropertyAttribute::NONE)?;
    put(scope, config, "nativeOwnerActive", active.into(), v8::PropertyAttribute::NONE)?;
    let registry = shared_registry.unwrap_or_else(|| v8::undefined(scope).into());
    put(scope, config, "sharedRegistry", registry, v8::PropertyAttribute::NONE)?;
    put(scope, config, "nativeRequestInventory", request.into(), v8::PropertyAttribute::NONE)?;
    let undefined = v8::undefined(scope);
    let record = initializer.call(scope, undefined.into(), &[config.into()]).ok_or("speech factory failed")?;
    let record = v8::Local::<v8::Object>::try_from(record).map_err(|_| "speech factory result")?;
    let registry = get(scope, record, "registry")?;
    if !registry.is_object() { return Err("speech registry missing"); }
    let service = object(scope, record, "servicePrototype")?;
    let voice = object(scope, record, "voicePrototype")?;
    let service_brand = function(scope, record, "serviceBrand")?;
    let voice_brand = function(scope, record, "voiceBrand")?;
    let semantic = function(scope, record, "getVoices")?;
    binding::install_method(scope, service, "getVoices", 0, PrivateTarget { semantic, accepts_receiver: service_brand }).map_err(|_| "speech getVoices binding")?;
    for name in ["voiceURI", "name", "lang", "localService", "default"] {
        let semantic = function(scope, record, name)?;
        binding::install_attribute(scope, voice, name, PrivateTarget { semantic, accepts_receiver: voice_brand }, None).map_err(|_| "speech voice binding")?;
    }
    let getter = function(scope, record, "onvoiceschangedGet")?;
    let setter = function(scope, record, "onvoiceschangedSet")?;
    binding::install_attribute(scope, service, "onvoiceschanged", PrivateTarget { semantic: getter, accepts_receiver: service_brand }, Some(PrivateTarget { semantic: setter, accepts_receiver: service_brand })).map_err(|_| "speech event handler binding")?;
    let parent = object(scope, record, "serviceConstructorParent")?;
    let synthesis = constructor(scope, "SpeechSynthesis", service, Some(parent))?;
    let voice = constructor(scope, "SpeechSynthesisVoice", voice, None)?;
    let brand = function(scope, record, "windowBrand")?;
    let getter = function(scope, record, "windowGet")?;
    let data = v8::Array::new_with_elements(scope, &[brand.into(), getter.into()]);
    if data.set_integrity_level(scope, v8::IntegrityLevel::Frozen) != Some(true) { return Err("speech window callback data"); }
    let getter = v8::Function::builder(window_getter).data(data.into()).length(0)
        .constructor_behavior(v8::ConstructorBehavior::Throw).build(scope).ok_or("speech window getter")?;
    let name = key(scope, "get speechSynthesis")?; getter.set_name(name);
    let undefined = v8::undefined(scope);
    let mut descriptor = v8::PropertyDescriptor::new_from_get_set(getter.into(), undefined.into());
    descriptor.set_enumerable(true); descriptor.set_configurable(true);
    // No global publication until every private prototype and callback is ready.
    let context = scope.get_current_context();
    let global = context.global(scope);
    put(scope, global, "SpeechSynthesis", synthesis.into(), v8::PropertyAttribute::DONT_ENUM)?;
    put(scope, global, "SpeechSynthesisVoice", voice.into(), v8::PropertyAttribute::DONT_ENUM)?;
    let name = key(scope, "speechSynthesis")?;
    if global.define_property(scope, name.into(), &descriptor) != Some(true) { return Err("speech window publication"); }
    Ok(registry)
}

/// A failed installation cannot leave a legacy or partially installed service.
/// Call with the failure caught and cleared; never recover from termination.
pub(crate) fn unpublish(scope: &mut v8::HandleScope) {
    if scope.is_execution_terminating() { return; }
    let context = scope.get_current_context(); let global = context.global(scope);
    for name in ["speechSynthesis", "SpeechSynthesis", "SpeechSynthesisVoice"] {
        if let Some(key) = v8::String::new(scope, name) { global.delete(scope, key.into()); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::ObscuraJsRuntime;
    use serde_json::json;

    #[test]
    fn speech_window_native_callback_converts_only_to_registered_global_and_preserves_throw() {
        let mut runtime = ObscuraJsRuntime::new(obscura_net::EffectivePersona::builtin(
            obscura_net::StealthProfile::MacChrome153,
        ));
        {
            let main = runtime.runtime().main_context();
            let mut entered = runtime.runtime();
            let scope = &mut v8::HandleScope::new(entered.v8_isolate());
            let context = v8::Local::new(scope, main);
            let scope = &mut v8::ContextScope::new(scope, context);
            // Host-only fixture, no inventory call or registration op. The exact
            // window callback is used; its semantics count successful dispatch.
            let source = v8::String::new(scope, r#"(() => {
                const original = globalThis;
                let calls = 0, shouldThrow = false;
                const sentinel = Symbol('original thrown value');
                return [receiver => receiver === original,
                    function() { calls++; if (shouldThrow) throw sentinel; return this; },
                    { calls: () => calls, throwNext: () => { shouldThrow=true; }, sentinel }];
            })()"#).unwrap();
            let value = v8::Script::compile(scope, source, None).unwrap().run(scope).unwrap();
            let fixture = v8::Local::<v8::Array>::try_from(value).unwrap();
            let brand = fixture.get_index(scope,0).unwrap();
            let semantic = fixture.get_index(scope,1).unwrap();
            let public = fixture.get_index(scope,2).unwrap();
            let data = v8::Array::new_with_elements(scope,&[brand,semantic]);
            assert_eq!(data.set_integrity_level(scope,v8::IntegrityLevel::Frozen),Some(true));
            let getter = v8::Function::builder(window_getter).data(data.into()).length(0)
                .constructor_behavior(v8::ConstructorBehavior::Throw).build(scope).unwrap();
            let global = context.global(scope);
            put(scope,global,"windowGetterForTest",getter.into(),v8::PropertyAttribute::NONE).unwrap();
            put(scope,global,"windowFixtureForTest",public,v8::PropertyAttribute::NONE).unwrap();
        }
        assert_eq!(runtime.evaluate(r#"(() => {
            const getter = windowGetterForTest, fixture = windowFixtureForTest;
            const accepted = [globalThis,null,undefined].every(receiver => Reflect.apply(getter,receiver,[]) === globalThis);
            const before = fixture.calls();
            const rejected = [{},1,'x',true,1n,Symbol('x'),new Proxy(globalThis,{})].every(receiver => {
                try { Reflect.apply(getter,receiver,[]); return false; } catch(error) { return error instanceof TypeError; }
            });
            const noSemantic = fixture.calls() === before;
            fixture.throwNext();
            let exact = false;
            try { Reflect.apply(getter,globalThis,[]); } catch(error) { exact = error === fixture.sentinel; }
            return [accepted,rejected,noSemantic,exact];
        })()"#).unwrap(),json!([true,true,true,true]));
    }
}
