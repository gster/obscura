//! Native Window-only Scheduling binding; no author-callable input producer.
use deno_core::v8;
use crate::pending_input::InputOwner;
use std::panic::{catch_unwind, AssertUnwindSafe};

struct OriginalOwner(InputOwner);
impl deno_core::cppgc::GarbageCollected for OriginalOwner {
    fn get_name(&self) -> &'static std::ffi::CStr { c"SchedulingOriginalOwner" }
}
fn throw(scope: &mut v8::HandleScope, message: &str) {
    if scope.is_execution_terminating() { return; }
    if let Some(message) = v8::String::new(scope, message) {
        let error = v8::Exception::type_error(scope, message); scope.throw_exception(error);
    }
}
fn illegal(scope: &mut v8::HandleScope, _: v8::FunctionCallbackArguments, _: v8::ReturnValue) {
    if catch_unwind(AssertUnwindSafe(|| throw(scope, "Illegal constructor"))).is_err() {
        throw(scope, "Scheduling callback failed");
    }
}
fn getter(scope: &mut v8::HandleScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    if catch_unwind(AssertUnwindSafe(|| {
        let Ok(data) = v8::Local::<v8::Array>::try_from(args.data()) else { return; };
        let Some(navigator) = data.get_index(scope, 0) else { return; };
        if !args.this().strict_equals(navigator) { throw(scope, "Illegal invocation"); return; }
        if let Some(value) = data.get_index(scope, 1) { rv.set(value); }
    })).is_err() { throw(scope, "Scheduling callback failed"); }
}
fn pending(scope: &mut v8::HandleScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    if catch_unwind(AssertUnwindSafe(|| {
        let Ok(data) = v8::Local::<v8::Array>::try_from(args.data()) else { return; };
        let Some(service) = data.get_index(scope, 1) else { return; };
        // Original object identity is private callback data; prototype forgery
        // and Proxy wrappers cannot acquire the interface's brand.
        if !args.this().strict_equals(service) { throw(scope, "Illegal invocation"); return; }
        let options = args.get(0);
        if !options.is_null_or_undefined() {
            let Ok(object) = v8::Local::<v8::Object>::try_from(options) else {
                throw(scope, "IsInputPendingOptions must be an object"); return;
            };
            let Some(key) = v8::String::new(scope, "includeContinuous") else { return; };
            let Some(value) = object.get(scope, key.into()) else { return; };
            // Read exactly once, preserving getter exceptions. This keyboard
            // slice has only discrete input, so both boolean values share it.
            let _include_continuous = value.boolean_value(scope);
        }
        let Some(capability) = data.get_index(scope, 2) else { return; };
        let Some(owner) = deno_core::cppgc::try_unwrap_cppgc_object::<OriginalOwner>(scope, capability) else { return; };
        rv.set_bool(owner.0.pending());
    })).is_err() { throw(scope, "Scheduling callback failed"); }
}
fn put(scope: &mut v8::HandleScope, object: v8::Local<v8::Object>, name: &str,
    value: v8::Local<v8::Value>, attributes: v8::PropertyAttribute) -> Result<(), &'static str> {
    let key = v8::String::new(scope, name).ok_or("scheduling allocation")?;
    (object.define_own_property(scope, key.into(), value, attributes) == Some(true))
        .then_some(()).ok_or("scheduling definition")
}
pub(crate) fn install(scope: &mut v8::HandleScope, owner: InputOwner) -> Result<(), &'static str> {
    let context = scope.get_current_context(); let global = context.global(scope);
    let key = v8::String::new(scope, "navigator").ok_or("scheduling navigator")?;
    let navigator: v8::Local<v8::Object> = global.get(scope, key.into()).ok_or("scheduling navigator")?
        .try_into().map_err(|_| "scheduling navigator")?;
    let nav_proto: v8::Local<v8::Object> = navigator.get_prototype(scope).ok_or("navigator prototype")?
        .try_into().map_err(|_| "navigator prototype")?;
    let prototype = v8::Object::new(scope); let service = v8::Object::new(scope);
    if service.set_prototype(scope, prototype.into()) != Some(true) { return Err("scheduling prototype"); }
    let capability = deno_core::cppgc::make_cppgc_object(scope, OriginalOwner(owner));
    let data = v8::Array::new_with_elements(scope, &[navigator.into(), service.into(), capability.into()]);
    if data.set_integrity_level(scope, v8::IntegrityLevel::Frozen) != Some(true) { return Err("scheduling data"); }
    let method = v8::Function::builder(pending).data(data.into()).length(0)
        .constructor_behavior(v8::ConstructorBehavior::Throw).build(scope).ok_or("scheduling callback")?;
    let name = v8::String::new(scope, "isInputPending").ok_or("scheduling name")?; method.set_name(name);
    put(scope, prototype, "isInputPending", method.into(), v8::PropertyAttribute::NONE)?;
    let constructor = v8::Function::builder(illegal).length(0).constructor_behavior(v8::ConstructorBehavior::Allow).build(scope).ok_or("scheduling constructor")?;
    let name = v8::String::new(scope, "Scheduling").ok_or("scheduling name")?; constructor.set_name(name);
    put(scope, constructor.into(), "prototype", prototype.into(), v8::PropertyAttribute::READ_ONLY | v8::PropertyAttribute::DONT_ENUM | v8::PropertyAttribute::DONT_DELETE)?;
    put(scope, prototype, "constructor", constructor.into(), v8::PropertyAttribute::DONT_ENUM)?;
    let tag = v8::Symbol::get_to_string_tag(scope); let name = v8::String::new(scope, "Scheduling").ok_or("scheduling tag")?;
    if prototype.define_own_property(scope, tag.into(), name.into(), v8::PropertyAttribute::READ_ONLY | v8::PropertyAttribute::DONT_ENUM) != Some(true) { return Err("scheduling tag"); }
    let accessor = v8::Function::builder(getter).data(data.into()).length(0)
        .constructor_behavior(v8::ConstructorBehavior::Throw).build(scope).ok_or("scheduling getter")?;
    let name = v8::String::new(scope, "get scheduling").ok_or("scheduling name")?; accessor.set_name(name);
    let undefined = v8::undefined(scope); let mut descriptor = v8::PropertyDescriptor::new_from_get_set(accessor.into(), undefined.into());
    descriptor.set_enumerable(true); descriptor.set_configurable(true);
    let key = v8::String::new(scope, "scheduling").ok_or("scheduling name")?;
    if nav_proto.define_property(scope, key.into(), &descriptor) != Some(true) { return Err("scheduling publication"); }
    put(scope, global, "Scheduling", constructor.into(), v8::PropertyAttribute::DONT_ENUM)
}


#[cfg(test)]
mod tests {
    use crate::runtime::ObscuraJsRuntime;
    #[test]
    fn native_scheduling_brand_dictionary_and_document_open_lifetime() {
        let mut runtime = ObscuraJsRuntime::new(obscura_net::EffectivePersona::builtin(
            obscura_net::StealthProfile::MacChrome153));
        runtime.run_page_init();
        let result = runtime.evaluate(r#"(() => {
            const service=navigator.scheduling, fn=service.isInputPending;
            const sentinel={}, results=[];
            let reads=0;
            try {fn.call(Object.create(Scheduling.prototype),{get includeContinuous(){reads++;}});} catch(e){results.push(e instanceof TypeError);}
            results.push(reads===0);
            try {fn.call(service,{get includeContinuous(){reads++;throw sentinel;}});} catch(e){results.push(e===sentinel);}
            results.push(reads===1);
            for(const value of [false,1,'x',Symbol('x')]) {try{fn.call(service,value);results.push(false);}catch(e){results.push(e instanceof TypeError);}}
            results.push(fn.call(service,null)===false,fn.call(service,undefined)===false);
            results.push(fn.call(service,{includeContinuous:{valueOf(){throw sentinel;}}})===false);
            results.push(Function.prototype.toString.call(fn).includes('[native code]'));
            results.push(!Object.prototype.hasOwnProperty.call(fn,'prototype'));
            const before=navigator.scheduling;document.open();document.write('<body>new</body>');document.close();
            results.push(navigator.scheduling===before);
            return results.every(Boolean);
        })()"#).unwrap();
        assert_eq!(result, serde_json::json!(true));
    }
}
