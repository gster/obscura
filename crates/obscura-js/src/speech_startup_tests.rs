//! Source-prepared integration fixtures. The host records actual build/run gates.
//! Valid speechSynthesis/getVoices invocation is deliberately left to provider
//! tests: these startup tests never launch native inventory discovery.
use super::*;
use serde_json::json;

fn speech_runtime() -> ObscuraJsRuntime {
    let mut runtime = ObscuraJsRuntime::new(obscura_net::EffectivePersona::builtin(
        obscura_net::StealthProfile::MacChrome153,
    ));
    runtime.set_dom(obscura_dom::parse_html("<html><body></body></html>"));
    runtime.set_url("https://example.test/");
    runtime.run_page_init();
    runtime
}

#[test]
fn speech_startup_handoff_is_removed_before_first_document() {
    let mut runtime = ObscuraJsRuntime::new(obscura_net::EffectivePersona::builtin(
        obscura_net::StealthProfile::MacChrome153,
    ));
    assert!(runtime.speech_initializer.is_some());
    assert!(runtime.speech_registry.is_none());
    assert_eq!(runtime.evaluate("['__obscura_speech_handoff','__obscura_speech_primitives'].every(k => !Object.hasOwn(globalThis,k))").unwrap(),json!(true));
}

#[test]
fn speech_startup_uses_actual_document_and_replaces_original_capability() {
    let mut runtime = ObscuraJsRuntime::new(obscura_net::EffectivePersona::builtin(
        obscura_net::StealthProfile::MacChrome153,
    ));
    let constructor_owner = runtime.state.borrow().speech_owner.clone().unwrap();
    runtime.set_dom(obscura_dom::parse_html("<p>first</p>"));
    let first = runtime.state.borrow().speech_owner.clone().unwrap();
    assert!(!constructor_owner.active());
    assert!(!first.same_document(&constructor_owner));
    runtime.run_page_init();
    assert!(runtime.speech_installed_owner.as_ref().unwrap().same_document(&first));
    assert!(first.active());
    let registry = runtime.speech_registry.clone().unwrap();
    runtime.run_page_init();
    assert!(runtime.speech_installed_owner.as_ref().unwrap().same_document(&first));
    assert_eq!(runtime.speech_registry.as_ref().unwrap(), &registry);
    runtime.set_dom(obscura_dom::parse_html("<p>replacement</p>"));
    let replacement = runtime.state.borrow().speech_owner.clone().unwrap();
    assert!(!first.active());
    assert!(replacement.active());
    runtime.run_page_init();
    assert!(runtime.speech_installed_owner.as_ref().unwrap().same_document(&replacement));
    assert!(!runtime.speech_installed_owner.as_ref().unwrap().same_document(&first));
    assert_eq!(runtime.speech_registry.as_ref().unwrap(), &registry);
}

#[test]
fn speech_startup_native_descriptors_constructors_and_invalid_brands() {
    let mut runtime = speech_runtime();
    assert_eq!(runtime.evaluate(r#"(() => {
      const wd = Object.getOwnPropertyDescriptor(globalThis,'speechSynthesis');
      const sp = SpeechSynthesis.prototype, vp = SpeechSynthesisVoice.prototype;
      const method = Object.getOwnPropertyDescriptor(sp,'getVoices');
      const handler = Object.getOwnPropertyDescriptor(sp,'onvoiceschanged');
      const getters = ['voiceURI','name','lang','localService','default'].map(k => Object.getOwnPropertyDescriptor(vp,k));
      const fs = [wd.get,method.value,handler.get,handler.set,...getters.map(d=>d.get)];
      const typeError = fn => { try { fn(); return false; } catch(e) { return e instanceof TypeError; } };
      return {
        window: [wd.enumerable,wd.configurable,wd.set === undefined,wd.get.name,wd.get.length],
        method: [method.writable,method.enumerable,method.configurable,method.value.length],
        handler: [handler.enumerable,handler.configurable,handler.get.length,handler.set.length],
        voices: getters.every(d => d.enumerable && d.configurable && d.set === undefined),
        native: fs.every(f => Function.prototype.toString.call(f).includes('[native code]') && !Object.hasOwn(f,'prototype')),
        invalid: fs.every(f => [null,undefined,{},1,'x',new Proxy(sp,{})].every(r => f === wd.get && r == null ? true : typeError(()=>Reflect.apply(f,r,[])))),
        ctors: [SpeechSynthesis,SpeechSynthesisVoice].every(c => typeError(()=>c()) && typeError(()=>new c()) && c.name && c.length === 0),
        inheritance: Object.getPrototypeOf(SpeechSynthesis) === EventTarget && Object.getPrototypeOf(sp) === EventTarget.prototype,
        prototypes: [SpeechSynthesis,SpeechSynthesisVoice].every(c => { const d = Object.getOwnPropertyDescriptor(c,'prototype'); return !d.writable && !d.enumerable && !d.configurable && c.prototype.constructor === c; }),
      };
    })()"#).unwrap(),json!({
      "window":[true,true,true,"get speechSynthesis",0],"method":[true,true,true,0],
      "handler":[true,true,0,1],"voices":true,"native":true,"invalid":true,
      "ctors":true,"inheritance":true,"prototypes":true,
    }));
}

#[test]
fn speech_startup_retired_owner_cannot_reinstall_or_retire_sibling_owner() {
    let mut runtime = speech_runtime();
    let old = runtime.state.borrow().speech_owner.clone().unwrap();
    runtime.set_dom(obscura_dom::parse_html("<p>new</p>"));
    let current = runtime.state.borrow().speech_owner.clone().unwrap();
    let initializer = runtime.speech_initializer.clone().unwrap();
    let context = runtime.runtime().main_context();
    assert_eq!(runtime.install_realm_speech(&context,&initializer,&old),Err("speech original document inactive"));
    assert!(current.active());
    runtime.run_page_init();
    assert!(current.active());
    assert!(!old.active());
    assert_eq!(runtime.ensure_speech_startup(),Err("SPEECH_STARTUP_FAILED"));
    assert!(runtime.evaluate("typeof SpeechSynthesis").unwrap_err().contains("SPEECH_STARTUP_FAILED"));
}

fn counter_without_author_execution(runtime: &mut ObscuraJsRuntime) -> i64 {
    use deno_core::v8;
    let main = runtime.runtime().main_context();
    let mut entered = runtime.runtime();
    let scope = &mut v8::HandleScope::new(entered.v8_isolate());
    let context = v8::Local::new(scope,main);
    let scope = &mut v8::ContextScope::new(scope,context);
    let global = context.global(scope);
    let key = v8::String::new(scope,"speechFailureCounter").unwrap();
    let value = global.get(scope,key.into()).unwrap();
    value.integer_value(scope).unwrap()
}

#[tokio::test]
async fn speech_startup_failure_is_sticky_across_author_entries_and_queued_jobs() {
    let mut runtime = speech_runtime();
    let frame = crate::frame::FrameRealm::new(&mut runtime,1,0,"https://example.test/frame","<p>frame</p>").unwrap();
    runtime.execute_script("poison-next-document",r#"
        globalThis.speechFailureCounter = 0;
        Object.defineProperty(globalThis,'SpeechSynthesisVoice',{value:17,configurable:false});
        Promise.resolve().then(() => { speechFailureCounter++; });
    "#).unwrap();
    runtime.set_dom(obscura_dom::parse_html("<p>replacement</p>"));
    let owner = runtime.state.borrow().speech_owner.clone().unwrap();
    runtime.run_page_init();
    assert_eq!(runtime.ensure_speech_startup(),Err("SPEECH_STARTUP_FAILED"));
    // A checkpoint before startup failed may already have run the promise.
    // Freeze the observed counter at the failure boundary, then require that
    // every rejected entry leaves it unchanged. Never assume a zero baseline.
    let at_failure = counter_without_author_execution(&mut runtime);
    assert!(owner.active()); // Binding failure does not shut down sibling WS.
    let expression = "speechFailureCounter++";
    assert!(runtime.evaluate(expression).unwrap_err().contains("SPEECH_STARTUP_FAILED"));
    assert!(runtime.evaluate_with_timeout(expression,std::time::Duration::from_millis(20)).unwrap_err().contains("SPEECH_STARTUP_FAILED"));
    assert!(runtime.execute_script("after-failure",expression).unwrap_err().contains("SPEECH_STARTUP_FAILED"));
    assert!(runtime.evaluate_for_cdp(expression,true,false).await.unwrap_err().contains("SPEECH_STARTUP_FAILED"));
    assert!(runtime.store_object(expression).unwrap_err().contains("SPEECH_STARTUP_FAILED"));
    assert!(frame.evaluate(&mut runtime,"parent.speechFailureCounter++").unwrap_err().contains("SPEECH_STARTUP_FAILED"));
    assert!(runtime.prepare_inline_module("speechFailureCounter++","https://example.test/m.js",10).await.is_err());
    assert!(runtime.run_event_loop().await.unwrap_err().contains("SPEECH_STARTUP_FAILED"));
    assert!(runtime.run_autonomous_event_loop_turn().await.unwrap_err().contains("SPEECH_STARTUP_FAILED"));
    runtime.resolve_promises().await;
    let mut predicate_calls = 0;
    assert!(!runtime.resolve_promises_until(|_| { predicate_calls += 1; true },20).await);
    assert_eq!(predicate_calls,0);
    runtime.cancel_termination();
    runtime.set_dom(obscura_dom::parse_html("<p>retry</p>"));
    runtime.run_page_init();
    assert!(runtime.evaluate(expression).unwrap_err().contains("SPEECH_STARTUP_FAILED"));
    // A host read of a known own data property does not execute author code.
    assert_eq!(counter_without_author_execution(&mut runtime),at_failure);
}

#[test]
fn speech_startup_page_initializer_is_host_captured_and_one_shot() {
    let mut runtime = ObscuraJsRuntime::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::MacChrome153));
    runtime.execute_script("initializer-spy","globalThis.initializerSpy=0;globalThis.__obscura_init=()=>initializerSpy++;").unwrap();
    runtime.set_dom(obscura_dom::parse_html("<p>first</p>"));
    runtime.run_page_init();
    assert!(runtime.page_initialized);
    assert!(runtime.speech_installed_owner.is_some());
    runtime.run_page_init();
    runtime.set_dom(obscura_dom::parse_html("<p>replacement</p>"));
    runtime.run_page_init();
    assert_eq!(runtime.evaluate("initializerSpy === 0").unwrap(),json!(true));
    assert!(runtime.ensure_speech_startup().is_ok());
}

#[test]
fn speech_startup_page_initializer_preserves_real_watchdog_termination() {
    use deno_core::v8;
    let mut runtime = ObscuraJsRuntime::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::MacChrome153));
    let initializer = {
        let main = runtime.runtime().main_context();
        let mut entered = runtime.runtime();
        let scope = &mut v8::HandleScope::with_context(entered.v8_isolate(),main);
        let source = v8::String::new(scope,"(function(){for(;;){}})").unwrap();
        let value = v8::Script::compile(scope,source,None).unwrap().run(scope).unwrap();
        let function = v8::Local::<v8::Function>::try_from(value).unwrap();
        v8::Global::new(scope,function)
    };
    runtime.page_initializer = Some(initializer); // private host fixture only.
    runtime.set_dom(obscura_dom::parse_html("<p>watchdog</p>"));
    // Check rethrow while an actual outer TryCatch is active. V8 may normalize
    // its top-level termination flag after leaving an otherwise uncaught entry.
    let watchdog = runtime.arm_watchdog(std::time::Duration::from_millis(50));
    {
        let initializer = runtime.page_initializer.clone().unwrap();
        let main = runtime.runtime().main_context();
        let mut entered = runtime.runtime();
        let scope = &mut v8::HandleScope::with_context(entered.v8_isolate(),main);
        let scope = &mut v8::TryCatch::new(scope);
        let function = v8::Local::new(scope,initializer);
        assert_eq!(ObscuraJsRuntime::call_page_initializer(scope,function),Err("trusted page initialization terminated"));
        assert!(scope.has_terminated());
        assert!(!scope.can_continue());
    }
    assert!(runtime.disarm_watchdog(watchdog));
    runtime.cancel_termination();
    let watchdog = runtime.arm_watchdog(std::time::Duration::from_millis(50));
    runtime.run_page_init();
    assert!(runtime.disarm_watchdog(watchdog));
    runtime.cancel_termination();
    assert_eq!(runtime.ensure_speech_startup(),Err("SPEECH_STARTUP_FAILED"));
    assert!(runtime.evaluate("1+1").unwrap_err().contains("SPEECH_STARTUP_FAILED"));
}

#[tokio::test(flavor="current_thread")]
async fn speech_delivery_tasks_preserve_microtasks_but_do_not_reactivate_previous_idb_transaction() {
    let mut runtime=speech_runtime();
    let created=runtime.call_function_on_for_cdp(r#"async()=>{
      const open=indexedDB.open('speech-tasks');open.onupgradeneeded=()=>open.result.createObjectStore('s');
      globalThis.speechTaskDb=await new Promise(resolve=>open.onsuccess=()=>resolve(open.result));return true;
    }"#,None,&[],true,true).await.unwrap();assert_eq!(created.value,Some(json!(true)));
    let initializer=runtime.speech_initializer.clone().unwrap();
    {
        // Host-only already-available delivery fixture. It exercises the same
        // initializer/dispatch path without exposing an inventory installer.
        use deno_core::v8;
        let mut entered=runtime.runtime();
        let scope=&mut entered.handle_scope();
        let code=v8::String::new(scope,r#"({nativeOwnerActive:()=>true,nativeRequestInventory:after=>Promise.resolve({
          revision:after+1,voices:[],done:after===0})})"#).unwrap();
        let config=v8::Script::compile(scope,code,None).unwrap().run(scope).unwrap();
        let initializer=v8::Local::new(scope,initializer);let receiver=v8::undefined(scope);
        let result=initializer.call(scope,receiver.into(),&[config]).unwrap().to_object(scope).unwrap();
        let code=v8::String::new(scope,r#"parts=>{
          Object.defineProperty(parts.servicePrototype,'getVoices',{value:parts.getVoices});
          Object.defineProperty(parts.servicePrototype,'onvoiceschanged',{get:parts.onvoiceschangedGet,set:parts.onvoiceschangedSet});
          return parts.service;
        }"#).unwrap();
        let install=v8::Script::compile(scope,code,None).unwrap().run(scope).unwrap();
        let install=v8::Local::<v8::Function>::try_from(install).unwrap();
        let service=install.call(scope,receiver.into(),&[result.into()]).unwrap();
        let key=v8::String::new(scope,"speechTaskFixture").unwrap();let global=scope.get_current_context().global(scope);
        assert_eq!(global.set(scope,key.into(),service),Some(true));
    }
    let result=runtime.call_function_on_for_cdp(r#"async()=>{
      let events=0,store,microtaskAllowed=false;
      const result=await new Promise(resolve=>{
        speechTaskFixture.onvoiceschanged=()=>{
          if(++events===1){store=speechTaskDb.transaction('s','readwrite').objectStore('s');
            queueMicrotask(()=>{try{store.put('first task',1);microtaskAllowed=true;}catch(e){microtaskAllowed=false;}});
          }else{let inactive;try{store.put('second task',2);inactive=false;}catch(e){inactive=e.name==='TransactionInactiveError';}
            resolve([microtaskAllowed,inactive]);}
        };speechTaskFixture.getVoices();
      });speechTaskDb.close();return result;
    }"#,None,&[],true,true).await.unwrap();
    assert!(!result.thrown,"{}",result.description);
    assert_eq!(result.value,Some(json!([true,true])));
}
