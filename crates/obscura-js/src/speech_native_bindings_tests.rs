//! Discriminating native-binding fixtures; execution evidence is recorded by the host.
//! They install only test interfaces, never replace Speech in the snapshot.
use super::*;
use crate::runtime::ObscuraJsRuntime;
use serde_json::json;

fn runtime() -> ObscuraJsRuntime {
    let mut runtime = ObscuraJsRuntime::new(obscura_net::EffectivePersona::builtin(
        obscura_net::StealthProfile::WindowsChrome145,
    ));
    runtime.set_dom(obscura_dom::parse_html("<html><body></body></html>"));
    runtime.set_url("https://example.test/");
    runtime.run_page_init();
    runtime
}

fn script<'s>(scope: &mut v8::HandleScope<'s>, source: &str) -> v8::Local<'s, v8::Value> {
    let code = v8::String::new(scope, source).unwrap();
    v8::Script::compile(scope, code, None).unwrap().run(scope).unwrap()
}

fn object_property<'s>(scope: &mut v8::HandleScope<'s>, object: v8::Local<v8::Object>, key: &str) -> v8::Local<'s, v8::Value> {
    let key = v8::String::new(scope, key).unwrap();
    object.get(scope, key.into()).unwrap()
}

fn function_property<'s>(scope: &mut v8::HandleScope<'s>, object: v8::Local<v8::Object>, key: &str) -> v8::Local<'s, v8::Function> {
    v8::Local::try_from(object_property(scope, object, key)).unwrap()
}

fn publish(scope: &mut v8::HandleScope, name: &str, value: v8::Local<v8::Value>) {
    let key = v8::String::new(scope, name).unwrap();
    let global = scope.get_current_context().global(scope);
    assert_eq!(global.set(scope, key.into(), value), Some(true));
}

// Registry and functions are constructed only by trusted test host code before
// publication. No test installation function/registry is put in globalThis.
fn registry<'s>(scope: &mut v8::HandleScope<'s>) -> v8::Local<'s, v8::Object> {
    v8::Local::try_from(script(scope, r#"(() => {
        const slots = new WeakMap();
        return {
            has: WeakMap.prototype.has.bind(slots),
            get: WeakMap.prototype.get.bind(slots),
            set: WeakMap.prototype.set.bind(slots),
        };
    })()"#)).unwrap()
}

const FIXTURE: &str = r#"(function (shared) {
    'use strict';
    const has = shared.has, get = shared.get, put = shared.set;
    const toNumber = Number;
    const prototype = {};
    const receiver = Object.create(prototype);
    const retired = {};
    const record = { value: 7, calls: 0, active: true, retired };
    put(receiver, record);
    function semantic(...values) {
        const state = get(this);
        if (!state.active) throw state.retired;
        ++state.calls;
        return { receiver: this, values };
    }
    function getter() { return get(this).value; }
    function setter(value) {
        const state = get(this);
        if (!state.active) throw state.retired;
        const converted = toNumber(value);
        if (!state.active) throw state.retired;
        state.value = converted;
    }
    return {
        prototype, receiver, semantic, getter, setter, check: has,
        public: {
            prototype, receiver,
            // Deliberate test-only exposure to poison an original function's
            // properties. It is not a production registration/authority path.
            semanticForPoisoning: semantic,
            calls: () => record.calls,
            retire: () => { record.active = false; },
            retired,
        },
    };
})"#;

struct Fixture<'s> {
    public: v8::Local<'s, v8::Object>,
    semantic: v8::Local<'s, v8::Function>,
    check: v8::Local<'s, v8::Function>,
    method: v8::Local<'s, v8::Function>,
}

fn install_fixture<'s>(scope: &mut v8::HandleScope<'s>, shared: v8::Local<'s, v8::Object>) -> Fixture<'s> {
    let factory = v8::Local::<v8::Function>::try_from(script(scope, FIXTURE)).unwrap();
    let undefined = v8::undefined(scope);
    let parts = factory.call(scope, undefined.into(), &[shared.into()]).unwrap();
    let parts = v8::Local::<v8::Object>::try_from(parts).unwrap();
    let prototype = v8::Local::<v8::Object>::try_from(object_property(scope, parts, "prototype")).unwrap();
    let semantic = function_property(scope, parts, "semantic");
    let check = function_property(scope, parts, "check");
    let getter = function_property(scope, parts, "getter");
    let setter = function_property(scope, parts, "setter");
    let method = install_method(scope, prototype, "perform", 2,
        PrivateTarget { semantic, accepts_receiver: check }).unwrap();
    install_attribute(scope, prototype, "value",
        PrivateTarget { semantic: getter, accepts_receiver: check },
        Some(PrivateTarget { semantic: setter, accepts_receiver: check })).unwrap();
    install_attribute(scope, prototype, "readonlyValue",
        PrivateTarget { semantic: getter, accepts_receiver: check }, None).unwrap();
    let public = v8::Local::<v8::Object>::try_from(object_property(scope, parts, "public")).unwrap();
    publish(scope, "nativeTest", public.into());
    Fixture { public, semantic, check, method }
}

fn setup() -> ObscuraJsRuntime {
    let mut runtime = runtime();
    let main = runtime.runtime().main_context();
    {
        let mut entered = runtime.runtime();
        let scope = &mut v8::HandleScope::with_context(entered.v8_isolate(), main);
        let shared = registry(scope);
        install_fixture(scope, shared);
    }
    runtime
}

#[test]
fn method_and_accessor_descriptors_are_real_api_functions() {
    let mut runtime = setup();
    assert_eq!(runtime.evaluate(r#"(() => {
        const p = nativeTest.prototype;
        const md = Object.getOwnPropertyDescriptor(p, 'perform');
        const ad = Object.getOwnPropertyDescriptor(p, 'value');
        const rd = Object.getOwnPropertyDescriptor(p, 'readonlyValue');
        const fs = [md.value, ad.get, ad.set, rd.get];
        return {
            method: [md.writable, md.enumerable, md.configurable],
            accessor: [ad.enumerable, ad.configurable, 'value' in ad, 'writable' in ad],
            readonly: rd.set === undefined,
            names: fs.map(f => f.name), lengths: fs.map(f => f.length),
            prototype: fs.map(f => 'prototype' in f),
            ownKeys: fs.map(f => Object.getOwnPropertyNames(f).sort()),
            nameFlags: fs.map(f => { const d = Object.getOwnPropertyDescriptor(f, 'name'); return [d.writable,d.enumerable,d.configurable]; }),
            lengthFlags: fs.map(f => { const d = Object.getOwnPropertyDescriptor(f, 'length'); return [d.writable,d.enumerable,d.configurable]; }),
            cannotConstruct: fs.map(f => { try { Reflect.construct(f, []); return false; } catch(e) { return e instanceof TypeError; } }),
            // No marking registration was performed. This is the actual V8 API
            // callable result, never an implementation string replacement.
            nativeCode: fs.map(f => Function.prototype.toString.call(f).includes('[native code]')),
        };
    })()"#).unwrap(), json!({
        "method":[true,true,true],"accessor":[true,true,false,false],"readonly":true,
        "names":["perform","get value","set value","get readonlyValue"],"lengths":[2,0,1,0],
        "prototype":[false,false,false,false],"ownKeys":[["length","name"],["length","name"],["length","name"],["length","name"]],
        "nameFlags":[[false,false,true],[false,false,true],[false,false,true],[false,false,true]],
        "lengthFlags":[[false,false,true],[false,false,true],[false,false,true],[false,false,true]],
        "cannotConstruct":[true,true,true,true],"nativeCode":[true,true,true,true],
    }));
}

#[test]
fn invalid_receivers_never_reach_semantics_or_value_conversion() {
    let mut runtime = setup();
    assert_eq!(runtime.evaluate(r#"(() => {
        const t = nativeTest, method = t.prototype.perform;
        const set = Object.getOwnPropertyDescriptor(t.prototype, 'value').set;
        let conversions = 0;
        const value = { valueOf() { ++conversions; return 9; } };
        const invalid = [undefined,null,1,'x',true,Symbol('x'),1n,globalThis,{},t.prototype,Object.create(t.prototype),new Proxy(t.receiver,{})];
        const rejects = invalid.map(receiver => {
            try { Reflect.apply(method,receiver,[value]); return false; }
            catch(e) { if (!(e instanceof TypeError)) return false; }
            try { Reflect.apply(set,receiver,[value]); return false; }
            catch(e) { return e instanceof TypeError; }
        });
        Object.setPrototypeOf(t.receiver, null);
        const valid = Reflect.apply(method,t.receiver,[1,2,3]);
        return [rejects,conversions,t.calls(),valid.receiver===t.receiver,valid.values];
    })()"#).unwrap(), json!([[true,true,true,true,true,true,true,true,true,true,true,true],0,1,true,[1,2,3]]));
}

#[test]
fn original_target_and_all_arguments_survive_property_poisoning() {
    let mut runtime = setup();
    assert_eq!(runtime.evaluate(r#"(() => {
        const t=nativeTest, method=t.prototype.perform, original=t.semanticForPoisoning;
        const apply=Reflect.apply, fnCall=Function.prototype.call, fnApply=Function.prototype.apply, has=WeakMap.prototype.has;
        const token={}, sym=Symbol();
        try {
            original.call=original.apply=() => { throw 'hijacked target'; };
            Object.setPrototypeOf(original, null);
            t.semanticForPoisoning=() => { throw 'replaced target'; };
            Object.defineProperty(t.prototype,'perform',{get(){throw 'public lookup';},configurable:true});
            Function.prototype.call=Function.prototype.apply=() => { throw 'mutable intrinsic'; };
            WeakMap.prototype.has=() => true;
            const r=apply(method,t.receiver,[undefined,null,token,sym,5]);
            let rejects=false;
            try { apply(method,{},[]); } catch(e) { rejects=e instanceof TypeError; }
            return [r.receiver===t.receiver,r.values.length,r.values[0]===undefined,r.values[1]===null,r.values[2]===token,r.values[3]===sym,r.values[4]===5,rejects];
        } finally {
            Function.prototype.call=fnCall;Function.prototype.apply=fnApply;WeakMap.prototype.has=has;
        }
    })()"#).unwrap(), json!([true,5,true,true,true,true,true,true]));
}

#[test]
fn setter_forwards_exact_exception_and_semantics_rechecks_retirement() {
    let mut runtime = setup();
    assert_eq!(runtime.evaluate(r#"(() => {
        const t=nativeTest, set=Object.getOwnPropertyDescriptor(t.prototype,'value').set;
        const token={}; let same=false;
        try { Reflect.apply(set,t.receiver,[{valueOf(){throw token;}}]); } catch(e) { same=e===token; }
        const before=t.receiver.value;
        let retired=false;
        try { Reflect.apply(set,t.receiver,[{valueOf(){t.retire();return 91;}}]); } catch(e) { retired=e===t.retired; }
        return [same,before,retired,t.receiver.value,t.calls()];
    })()"#).unwrap(), json!([true,7,true,7,0]));
}

#[test]
fn main_and_snapshot_child_share_private_brands_but_keep_function_error_realm() {
    let mut runtime = runtime();
    let child = runtime.create_realm_context().unwrap();
    runtime.share_deno_context_state_with_realm(&child);
    assert!(runtime.share_ops_with_realm(&child));
    runtime.share_security_token_with_realm(&child);
    let main = runtime.runtime().main_context();
    {
        let mut entered = runtime.runtime();
        let scope = &mut v8::HandleScope::with_context(entered.v8_isolate(), main);
        let shared = registry(scope);
        let parent = install_fixture(scope, shared);
        let child_context = v8::Local::new(scope, &child);
        let child_public = {
            let scope = &mut v8::ContextScope::new(scope, child_context);
            let fixture = install_fixture(scope, shared);
            publish(scope, "otherNativeTest", parent.public.into());
            script(scope, "globalThis.savedTypeError = TypeError");
            fixture.public
        };
        publish(scope, "otherNativeTest", child_public.into());
    }
    assert_eq!(runtime.evaluate(r#"(() => {
        const parent=nativeTest,child=otherNativeTest;
        const r=Reflect.apply(child.prototype.perform,parent.receiver,[11]);
        const v=Reflect.apply(parent.prototype.perform,child.receiver,[22]);
        parent.receiver.value=41;child.receiver.value=42;
        return [r.receiver===parent.receiver,v.receiver===child.receiver,parent.calls(),child.calls(),parent.receiver.value,child.receiver.value];
    })()"#).unwrap(), json!([true,true,1,1,41,42]));
    assert_eq!(runtime.eval_in_realm(&child, r#"JSON.stringify((() => {
        const childMethod=nativeTest.prototype.perform;
        const parentMethod=otherNativeTest.prototype.perform;
        const original=TypeError;
        globalThis.TypeError=function ReplacedTypeError(){};
        try {
            let childError,parentError;
            try { Reflect.apply(childMethod,{},[]); } catch(e) { childError=e; }
            try { Reflect.apply(parentMethod,{},[]); } catch(e) { parentError=e; }
            return [childError instanceof original, parentError instanceof original];
        } finally { globalThis.TypeError=original; }
    })())"#).unwrap(), "[true,false]");
}

fn raw_receiver(scope: &mut v8::HandleScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    callback_boundary(scope, |_scope| rv.set(args.this().into()));
}

#[test]
fn pinned_v8_api_receiver_conversion_is_not_claimed_raw_this() {
    let mut runtime = runtime();
    let main = runtime.runtime().main_context();
    {
        let mut entered = runtime.runtime();
        let scope = &mut v8::HandleScope::with_context(entered.v8_isolate(), main);
        let function = v8::Function::builder(raw_receiver).constructor_behavior(v8::ConstructorBehavior::Throw).build(scope).unwrap();
        publish(scope,"rawReceiverForTest",function.into());
    }
    assert_eq!(runtime.evaluate(r#"(() => {
        const f=rawReceiverForTest,o={},p=new Proxy(o,{});
        return [Reflect.apply(f,null,[])===globalThis,Reflect.apply(f,undefined,[])===globalThis,
            typeof Reflect.apply(f,7,[]),Reflect.apply(f,7,[]).valueOf()===7,
            typeof Reflect.apply(f,Symbol(),[]),Reflect.apply(f,o,[])===o,Reflect.apply(f,p,[])===p];
    })()"#).unwrap(), json!([true,true,"object",true,"object",true,true]));
}

fn injected_panic(scope: &mut v8::HandleScope, _args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    callback_boundary(scope, |_scope| panic!("injected native callback panic"));
}

#[test]
fn callback_panic_becomes_an_error_without_unwinding_into_v8() {
    let mut runtime = runtime();
    let main = runtime.runtime().main_context();
    {
        let mut entered = runtime.runtime();
        let scope = &mut v8::HandleScope::with_context(entered.v8_isolate(), main);
        let function = v8::Function::builder(injected_panic).constructor_behavior(v8::ConstructorBehavior::Throw).build(scope).unwrap();
        publish(scope,"panicForTest",function.into());
    }
    assert_eq!(runtime.evaluate("(() => { try { panicForTest(); return false; } catch(e) { return e instanceof Error && e.message === 'Native interface callback panicked'; } })()").unwrap(), json!(true));
    assert_eq!(runtime.evaluate("1+2").unwrap(),json!(3.0));
}

fn terminate_now(scope: &mut v8::HandleScope, _args: v8::FunctionCallbackArguments, _rv: v8::ReturnValue) {
    scope.terminate_execution();
}

#[test]
fn termination_remains_termination_and_is_never_replaced_with_success() {
    let mut runtime = runtime();
    let main = runtime.runtime().main_context();
    {
        let mut entered = runtime.runtime();
        let scope = &mut v8::HandleScope::with_context(entered.v8_isolate(), main);
        let terminating = v8::Function::new(scope,terminate_now).unwrap();
        publish(scope,"terminateForTest",terminating.into());
        let shared=registry(scope);
        let fixture=install_fixture(scope,shared);
        let target=v8::Local::<v8::Function>::try_from(script(scope,"(function(){terminateForTest();for(let i=0;i<1000000;i++){}return 99;})")).unwrap();
        let prototype=v8::Local::<v8::Object>::try_from(object_property(scope,fixture.public,"prototype")).unwrap();
        install_method(scope,prototype,"terminate",0,PrivateTarget{semantic:target,accepts_receiver:fixture.check}).unwrap();
        {
            let direct = &mut v8::TryCatch::new(scope);
            let undefined = v8::undefined(direct);
            assert!(target.call(direct, undefined.into(), &[]).is_none(),
                "direct semantic control must consume the termination request");
            assert!(direct.has_terminated());
            direct.cancel_terminate_execution();
        }
        let scope=&mut v8::TryCatch::new(scope);
        let code=v8::String::new(scope,"nativeTest.receiver.terminate()").unwrap();
        let compiled=v8::Script::compile(scope,code,None).unwrap();
        assert!(compiled.run(scope).is_none());
        assert!(scope.has_terminated());
        // Host test cleanup only. The production binding never calls this.
        scope.cancel_terminate_execution();
    }
    assert_eq!(runtime.evaluate("2+3").unwrap(),json!(5.0));
}

#[test]
fn unreachable_native_function_graph_does_not_keep_semantics_or_registry_alive() {
    let mut runtime=runtime();
    let main=runtime.runtime().main_context();
    let weak;
    {
        let mut entered=runtime.runtime();
        let scope=&mut v8::HandleScope::with_context(entered.v8_isolate(),main);
        let shared=registry(scope);
        let fixture=install_fixture(scope,shared);
        weak=(v8::Weak::new(scope,fixture.semantic),v8::Weak::new(scope,fixture.method),v8::Weak::new(scope,fixture.check),v8::Weak::new(scope,shared));
        let key=v8::String::new(scope,"nativeTest").unwrap();
        let global=scope.get_current_context().global(scope);
        assert_eq!(global.delete(scope,key.into()),Some(true));
    }
    {
        let mut entered = runtime.runtime();
        // rusty_v8 defers local-handle scope destruction until the parent is
        // touched. A fresh scope removes the old fixture's zombie local roots.
        let scope = &mut v8::HandleScope::new(entered.v8_isolate());
        for _ in 0..3 { scope.low_memory_notification(); }
    }
    assert!(weak.0.is_empty() && weak.1.is_empty() && weak.2.is_empty() && weak.3.is_empty(),
        "unreachable callback data must not be retained through a Rust strong registry/Global");
}
