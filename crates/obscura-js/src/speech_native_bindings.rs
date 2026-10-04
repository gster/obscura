//! Host-only native callables for privately branded object interfaces.
//!
//! This is a binding primitive, not Speech synthesis or a DOM authority factory.
//! Only trusted startup Rust may supply the semantic function and brand predicate.
//! There is deliberately no op, global factory, or script-callable registration.
//! The predicate must use a captured private identity registry, accept only real
//! interface objects (never a global/boxed primitive), and run no author code.
//! Lifecycle/lease checks and WebIDL argument conversion belong to semantics.
//!
//! rusty_v8 137 exposes API callback `this` as Object. V8 converts null/undefined
//! to a global proxy and boxes primitives before this callback. Consequently this
//! primitive is NOT a raw-this transparent bridge. It is limited to interfaces
//! whose receiver predicate rejects those converted objects. It must not bind
//! Window getters, primitive-sensitive functions, static APIs, or constructors.
//!
//! Callback data is an unreachable frozen V8 array containing the original
//! semantic function and predicate. V8 owns/traces this graph. No Rust Global,
//! External, unsafe pointer, process registry, or author-selected target is used.

use deno_core::v8;
use std::panic::{catch_unwind, AssertUnwindSafe};

/// Functions captured from private bootstrap state before any author script.
/// A Rust caller must not populate this from a page property or op arguments.
#[derive(Clone, Copy)]
pub(crate) struct PrivateTarget<'s> {
    pub(crate) semantic: v8::Local<'s, v8::Function>,
    pub(crate) accepts_receiver: v8::Local<'s, v8::Function>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InstallError {
    InvalidLength,
    ContextUnavailable,
    AllocationOrException,
    DefinitionRejected,
}

fn throw_error(scope: &mut v8::HandleScope, message: &str, type_error: bool) {
    if scope.is_execution_terminating() { return; }
    let Some(message) = v8::String::new(scope, message) else { return; };
    let error = if type_error {
        v8::Exception::type_error(scope, message)
    } else {
        v8::Exception::error(scope, message)
    };
    scope.throw_exception(error);
}

// Keep Rust unwinding on this side of V8's callback frame. This does not recover
// from allocator aborts; normal argument-vector allocation below is fallible.
fn callback_boundary<'s>(
    scope: &mut v8::HandleScope<'s>,
    work: impl FnOnce(&mut v8::HandleScope<'s>),
) {
    let result = catch_unwind(AssertUnwindSafe(|| work(scope)));
    if result.is_err() {
        // No cancellation of termination and no execution of author code here.
        throw_error(scope, "Native interface callback panicked", false);
    }
}

fn invoke(
    scope: &mut v8::HandleScope,
    args: &v8::FunctionCallbackArguments,
    result: &mut v8::ReturnValue,
) {
    if scope.is_execution_terminating() { return; }
    let scope = &mut v8::TryCatch::new(scope);
    let value = invoke_checked(scope, args);
    if scope.has_caught() {
        // ReThrow preserves the original thrown value, including non-Errors.
        // Do not inspect the value or run another V8 operation after rethrow.
        scope.rethrow();
        return;
    }
    if let Some(value) = value { result.set(value); }
}

fn invoke_checked<'s>(
    scope: &mut v8::HandleScope<'s>,
    args: &v8::FunctionCallbackArguments,
) -> Option<v8::Local<'s, v8::Value>> {
    let data = v8::Local::<v8::Array>::try_from(args.data()).ok()?;
    let semantic = v8::Local::<v8::Function>::try_from(data.get_index(scope, 0)?).ok()?;
    let accepts = v8::Local::<v8::Function>::try_from(data.get_index(scope, 1)?).ok()?;
    let receiver: v8::Local<v8::Value> = args.this().into();
    let undefined = v8::undefined(scope);
    let branded = accepts.call(scope, undefined.into(), &[receiver])?;
    if !branded.is_true() {
        throw_error(scope, "Illegal invocation", true);
        return None;
    }
    let mut parameters = Vec::new();
    if parameters.try_reserve_exact(args.length() as usize).is_err() {
        throw_error(scope, "Native interface argument allocation failed", false);
        return None;
    }
    for index in 0..args.length() { parameters.push(args.get(index)); }
    // Call the captured function itself. Never consult .call/.apply, an object's
    // prototype, or the public method property again. Additional actual arguments
    // are preserved even when there are more than the declared function length.
    semantic.call(scope, receiver, &parameters)
}

fn callback(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut result: v8::ReturnValue,
) {
    callback_boundary(scope, |scope| invoke(scope, &args, &mut result));
}

fn callable<'s>(
    scope: &mut v8::HandleScope<'s>,
    name: &str,
    length: i32,
    target: PrivateTarget<'s>,
) -> Result<v8::Local<'s, v8::Function>, InstallError> {
    if length < 0 { return Err(InstallError::InvalidLength); }
    // The actual semantic function selects the binding realm. This is a trusted
    // bootstrap capture, not an author-supplied owner function granting a lease.
    let context = target.semantic.get_creation_context(scope)
        .ok_or(InstallError::ContextUnavailable)?;
    let scope = &mut v8::ContextScope::new(scope, context);
    let data = v8::Array::new_with_elements(scope, &[
        target.semantic.into(), target.accepts_receiver.into(),
    ]);
    if data.set_integrity_level(scope, v8::IntegrityLevel::Frozen) != Some(true) {
        return Err(InstallError::DefinitionRejected);
    }
    let function = v8::Function::builder(callback)
        .data(data.into())
        .length(length)
        .constructor_behavior(v8::ConstructorBehavior::Throw)
        .build(scope)
        .ok_or(InstallError::AllocationOrException)?;
    let name = v8::String::new(scope, name).ok_or(InstallError::AllocationOrException)?;
    function.set_name(name);
    Ok(function)
}

/// Install a writable/enumerable/configurable prototype method. Call before
/// exposing the prototype. Failure aborts that realm's private installation;
/// the host must not publish a partially initialized prototype.
pub(crate) fn install_method<'s>(
    scope: &mut v8::HandleScope<'s>,
    prototype: v8::Local<'s, v8::Object>,
    name: &str,
    length: i32,
    target: PrivateTarget<'s>,
) -> Result<v8::Local<'s, v8::Function>, InstallError> {
    let function = callable(scope, name, length, target)?;
    let key = v8::String::new(scope, name).ok_or(InstallError::AllocationOrException)?;
    if prototype.define_own_property(scope, key.into(), function.into(), v8::PropertyAttribute::NONE) != Some(true) {
        return Err(InstallError::DefinitionRejected);
    }
    Ok(function)
}

/// Install an enumerable/configurable attribute with native getter length0 and
/// optional setter length1. Getter/setter targets have the same private brand
/// contract as methods. Readonly means an actually absent setter.
pub(crate) fn install_attribute<'s>(
    scope: &mut v8::HandleScope<'s>,
    prototype: v8::Local<'s, v8::Object>,
    name: &str,
    getter: PrivateTarget<'s>,
    setter: Option<PrivateTarget<'s>>,
) -> Result<(), InstallError> {
    let get = callable(scope, &format!("get {name}"), 0, getter)?;
    let set: v8::Local<v8::Value> = match setter {
        Some(target) => callable(scope, &format!("set {name}"), 1, target)?.into(),
        None => v8::undefined(scope).into(),
    };
    let mut descriptor = v8::PropertyDescriptor::new_from_get_set(get.into(), set);
    descriptor.set_enumerable(true);
    descriptor.set_configurable(true);
    let key = v8::String::new(scope, name).ok_or(InstallError::AllocationOrException)?;
    if prototype.define_property(scope, key.into(), &descriptor) != Some(true) {
        return Err(InstallError::DefinitionRejected);
    }
    Ok(())
}

#[cfg(test)]
#[path = "speech_native_bindings_tests.rs"]
mod tests;
