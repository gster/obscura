//! These exercise the scope seam directly; the existing frame tests cover the
//! unchanged loaded-frame orchestration. No production JS entry point is added.
use deno_core::v8;

use crate::realm_factory;
use crate::runtime::ObscuraJsRuntime;

fn runtime() -> ObscuraJsRuntime {
    ObscuraJsRuntime::new(obscura_net::EffectivePersona::builtin(
        obscura_net::StealthProfile::WindowsChrome145,
    ))
}

fn global_value<'s>(scope: &mut v8::HandleScope<'s>, name: &str) -> Option<v8::Local<'s, v8::Value>> {
    let context = scope.get_current_context();
    let global = context.global(scope);
    let key = v8::String::new(scope, name)?;
    global.get(scope, key.into())
}

// V8 enters this callback while the host already holds its runtime borrow.
// It receives only a HandleScope, so restoring nested contexts cannot call back
// through &mut ObscuraJsRuntime or an aliased mutable runtime pointer.
fn restore_from_callback(scope: &mut v8::HandleScope, _: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let result = (|| -> Option<bool> {
        let outer = scope.get_current_context();
        let main = v8::Global::new(scope, outer);
        let parent_array = global_value(scope, "Array")?;
        let child = realm_factory::restore_snapshot(scope)?;
        realm_factory::share_deno_context_state(scope, main.clone(), &child);
        let child_context = v8::Local::new(scope, &child);
        // Read actual deno callback slots, not a JS stand-in for context state.
        let slots_match = unsafe {
            [deno_core::CONTEXT_STATE_SLOT_INDEX, deno_core::MODULE_MAP_SLOT_INDEX]
                .into_iter().all(|index| {
                    let expected = outer.get_aligned_pointer_from_embedder_data(index);
                    !expected.is_null()
                        && child_context.get_aligned_pointer_from_embedder_data(index) == expected
                })
        };
        let distinct_intrinsics = {
            let scope = &mut v8::ContextScope::new(scope, child_context);
            let child_array = global_value(scope, "Array")?;
            let child_array_object = child_array.to_object(scope)?;
            let prototype_key = v8::String::new(scope, "prototype")?;
            let child_prototype = child_array_object.get(scope, prototype_key.into())?;
            let grandchild = realm_factory::restore_snapshot(scope)?;
            realm_factory::share_deno_context_state(scope, main, &grandchild);
            let grandchild_context = v8::Local::new(scope, &grandchild);
            let grandchild_distinct = {
                let scope = &mut v8::ContextScope::new(scope, grandchild_context);
                let grandchild_array = global_value(scope, "Array")?;
                !grandchild_array.strict_equals(child_array)
                    && !grandchild_array.strict_equals(parent_array)
            };
            child_prototype.is_array()
                && !child_array.strict_equals(parent_array)
                && grandchild_distinct
                && scope.get_current_context() == child_context
        };
        Some(slots_match && distinct_intrinsics && scope.get_current_context() == outer)
    })().unwrap_or(false);
    let result = v8::Boolean::new(scope, result);
    rv.set(result.into());
}

#[test]
fn scope_factory_restores_nested_real_contexts_inside_native_callback() {
    let mut runtime = runtime();
    let main = runtime.runtime().main_context();
    let mut entered = runtime.runtime();
    let scope = &mut v8::HandleScope::with_context(entered.v8_isolate(), main);
    let before = scope.get_current_context();
    let callback = v8::Function::new(scope, restore_from_callback).unwrap();
    let receiver = v8::undefined(scope);
    let result = callback.call(scope, receiver.into(), &[]).unwrap();
    assert!(result.is_true(), "scope-only creation must produce real distinct intrinsics and valid callback slots");
    assert_eq!(scope.get_current_context(), before);
}

#[test]
fn scope_factory_handoff_is_single_use_and_failed_delete_stays_in_original_context() {
    let mut runtime = runtime();
    let main = runtime.runtime().main_context();
    let mut entered = runtime.runtime();
    let scope = &mut v8::HandleScope::with_context(entered.v8_isolate(), main.clone());
    let before = scope.get_current_context();
    let first = realm_factory::restore_snapshot(scope).unwrap();
    let second = realm_factory::restore_snapshot(scope).unwrap();
    realm_factory::share_deno_context_state(scope, main.clone(), &first);
    realm_factory::share_deno_context_state(scope, main, &second);
    let handoff = "__obscura_native_lifecycle_handoff";
    let initializer = realm_factory::take_native_input(scope, &first, handoff).unwrap();
    assert!(realm_factory::take_native_input(scope, &first, handoff).is_none());
    assert!(realm_factory::take_native_input(scope, &second, handoff).is_some());

    let key_name = "factory_test_nonconfigurable_handoff";
    {
        let context = v8::Local::new(scope, &first);
        let scope = &mut v8::ContextScope::new(scope, context);
        let global = context.global(scope);
        let key = v8::String::new(scope, key_name).unwrap();
        let value = v8::Local::new(scope, &initializer);
        assert_eq!(global.define_own_property(scope, key.into(), value.into(), v8::PropertyAttribute::DONT_DELETE), Some(true));
    }
    assert!(realm_factory::take_native_input(scope, &first, key_name).is_none());
    {
        let context = v8::Local::new(scope, &first);
        let scope = &mut v8::ContextScope::new(scope, context);
        let saved = global_value(scope, key_name).unwrap();
        let initializer = v8::Local::new(scope, &initializer);
        assert!(saved.strict_equals(initializer.into()));
    }
    assert!(realm_factory::take_native_input(scope, &second, key_name).is_none());
    assert_eq!(scope.get_current_context(), before);
    assert!(global_value(scope, key_name).unwrap().is_undefined());
}
