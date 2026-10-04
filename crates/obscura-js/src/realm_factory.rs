//! Scope-level primitives for the existing snapshot-created child contexts.
//!
//! Callers provide an already entered isolate's HandleScope. These operations
//! never borrow/re-enter ObscuraJsRuntime, pump tasks, or clear termination.
//! Every supplied handle must belong to that isolate; the main runtime retains
//! ownership of deno callback slots for the entire lifetime of its children.
//! The host wrappers retain their existing startup checks and installation order.
//! This is not yet a synchronous blank-realm entry point: document ownership,
//! admission, origin policy and publication still belong to the loaded path.

use deno_core::v8;

pub(crate) fn restore_snapshot(
    scope: &mut v8::HandleScope<'_, ()>,
) -> Option<v8::Global<v8::Context>> {
    let context = v8::Context::from_snapshot(
        scope,
        1,
        v8::ContextOptions::default(),
    )
    .or_else(|| {
        v8::Context::from_snapshot(
            scope,
            0,
            v8::ContextOptions::default(),
        )
    })?;
    Some(v8::Global::new(scope, context))
}

pub(crate) fn bind_document_state(
    scope: &mut v8::HandleScope<'_, ()>,
    context: &v8::Global<v8::Context>,
    state: crate::ops::SharedState,
) {
    let context = v8::Local::new(scope, context);
    context.set_slot(state);
}

pub(crate) fn take_native_input(
    scope: &mut v8::HandleScope<'_, ()>,
    realm: &v8::Global<v8::Context>,
    name: &str,
) -> Option<v8::Global<v8::Function>> {
    let context = v8::Local::new(scope, realm);
    let scope = &mut v8::ContextScope::new(scope, context);
    let global = context.global(scope);
    let key = v8::String::new(scope, name)?;
    let value = global.get(scope, key.into())?;
    let function = v8::Local::<v8::Function>::try_from(value).ok()?;
    if !global.delete(scope, key.into()).unwrap_or(false) {
        return None;
    }
    Some(v8::Global::new(scope, function))
}

/// The main runtime must outlive every child; only deno_core owns these slots.
/// This preserves the existing borrowed-slot contract, not a new JsRealm.
pub(crate) fn share_deno_context_state(
    scope: &mut v8::HandleScope<'_, ()>,
    main: v8::Global<v8::Context>,
    realm: &v8::Global<v8::Context>,
) {
    use deno_core::{CONTEXT_STATE_SLOT_INDEX, MODULE_MAP_SLOT_INDEX};
    let main_ctx = v8::Local::new(scope, main);
    let realm_ctx = v8::Local::new(scope, realm);
    // SAFETY: these slots on the main context are `Rc::into_raw` pointers
    // set by deno_core at runtime construction; they outlive every frame
    // realm. We only copy (alias) them and never reconstruct the Rc from
    // the frame.
    unsafe {
        let cs = main_ctx.get_aligned_pointer_from_embedder_data(CONTEXT_STATE_SLOT_INDEX);
        let mm = main_ctx.get_aligned_pointer_from_embedder_data(MODULE_MAP_SLOT_INDEX);
        realm_ctx.set_aligned_pointer_in_embedder_data(CONTEXT_STATE_SLOT_INDEX, cs);
        realm_ctx.set_aligned_pointer_in_embedder_data(MODULE_MAP_SLOT_INDEX, mm);
    }
}

/// Already captured native handles; no Runtime, OpState or document borrow.
pub(crate) struct RealmOps {
    pub ops: v8::Global<v8::Value>,
    pub plugin_registry: v8::Global<v8::Value>,
    pub plugin_initializer: v8::Global<v8::Function>,
    pub navigator_registry: v8::Global<v8::Value>,
    pub screen_registry: v8::Global<v8::Value>,
    pub font_face_registry: v8::Global<v8::Value>,
    pub dom_registry: v8::Global<v8::Value>,
    pub canvas_registry: v8::Global<v8::Value>,
    pub navigator_initializer: v8::Global<v8::Function>,
    pub performance_registry: v8::Global<v8::Value>,
    pub checked_receivers: v8::Global<v8::Value>,
    pub performance_initializer: v8::Global<v8::Function>,
    pub response_registry: v8::Global<v8::Value>,
    pub response_initializer: v8::Global<v8::Function>,
    pub unref_op_promise: v8::Global<v8::Function>,
}

pub(crate) fn install_ops(
    scope: &mut v8::HandleScope<'_, ()>,
    realm: &v8::Global<v8::Context>,
    bindings: RealmOps,
) -> bool {
    let RealmOps { ops, plugin_registry, plugin_initializer, navigator_registry, screen_registry, font_face_registry, dom_registry, canvas_registry, navigator_initializer, performance_registry, checked_receivers, performance_initializer, response_registry, response_initializer, unref_op_promise } = bindings;
    let context = v8::Local::new(scope, realm);
    let scope = &mut v8::ContextScope::new(scope, context);
    let scope = &mut v8::TryCatch::new(scope);

    let plugin_initializer = v8::Local::new(scope, plugin_initializer);
    let registry = v8::Local::new(scope, plugin_registry);
    let receiver = v8::undefined(scope);
    if plugin_initializer.call(scope, receiver.into(), &[registry]).is_none() {
        return false;
    }
    let navigator_initializer = v8::Local::new(scope, navigator_initializer);
    let registry = v8::Local::new(scope, navigator_registry);
    if navigator_initializer.call(scope, receiver.into(), &[registry]).is_none() {
        return false;
    }
    let performance_initializer = v8::Local::new(scope, performance_initializer);
    let registry = v8::Local::new(scope, performance_registry);
    if performance_initializer.call(scope, receiver.into(), &[registry]).is_none() { return false; }

    let response_initializer = v8::Local::new(scope, response_initializer);
    let registry = v8::Local::new(scope, response_registry);
    if response_initializer.call(scope, receiver.into(), &[registry]).is_none() { return false; }

    let Some(handoff_key) = v8::String::new(scope, "__obscura_core_handoff") else {
        return false;
    };
    let Some(ops_key) = v8::String::new(scope, "ops") else {
        return false;
    };
    let global = context.global(scope);
    let Some(core) = global.get(scope, handoff_key.into()) else {
        return false;
    };
    let Some(core) = core.to_object(scope) else {
        return false;
    };
    // Shared op functions create promises with the main deno-core private id
    // symbol. Use that core's unref helper as well, not the child's snapshot.
    let Some(unref_key) = v8::String::new(scope, "initializePromiseUnref") else { return false; };
    let Some(initialize) = core.get(scope, unref_key.into())
        .and_then(|value| v8::Local::<v8::Function>::try_from(value).ok()) else { return false; };
    let unref = v8::Local::new(scope, unref_op_promise);
    if initialize.call(scope, receiver.into(), &[unref.into()]).is_none() { return false; }
    let Some(screen_key) = v8::String::new(scope, "initializeScreenRegistry") else {
        return false;
    };
    let Some(initialize) = core.get(scope, screen_key.into())
        .and_then(|value| v8::Local::<v8::Function>::try_from(value).ok()) else {
        return false;
    };
    let registry = v8::Local::new(scope, screen_registry);
    if initialize.call(scope, receiver.into(), &[registry]).is_none() {
        return false;
    }
    let Some(canvas_key) = v8::String::new(scope, "initializeCanvasRegistry") else {
        return false;
    };
    let Some(initialize) = core.get(scope, canvas_key.into())
        .and_then(|value| v8::Local::<v8::Function>::try_from(value).ok()) else {
        return false;
    };
    let registry = v8::Local::new(scope, canvas_registry);
    if initialize.call(scope, receiver.into(), &[registry]).is_none() {
        return false;
    }
    let Some(font_face_key) = v8::String::new(scope, "initializeFontFaceRegistry") else {
        return false;
    };
    let Some(initialize) = core.get(scope, font_face_key.into())
        .and_then(|value| v8::Local::<v8::Function>::try_from(value).ok()) else {
        return false;
    };
    let registry = v8::Local::new(scope, font_face_registry);
    if initialize.call(scope, receiver.into(), &[registry]).is_none() {
        return false;
    }
    let Some(dom_key) = v8::String::new(scope, "initializeDOMReceiverRegistry") else { return false; };
    let Some(initialize) = core.get(scope, dom_key.into())
        .and_then(|value| v8::Local::<v8::Function>::try_from(value).ok()) else { return false; };
    let registry = v8::Local::new(scope, dom_registry);
    if initialize.call(scope, receiver.into(), &[registry]).is_none() { return false; }
    // `Deno.core.ops` is non-writable and non-configurable, so the table
    // cannot be swapped wholesale: V8 reports success and changes nothing.
    // Copy the bound op functions into the realm's existing table instead.
    let Some(target) = core
        .get(scope, ops_key.into())
        .and_then(|value| value.to_object(scope))
    else {
        return false;
    };
    let source = v8::Local::new(scope, ops);
    let Some(source) = source.to_object(scope) else {
        return false;
    };
    let Some(names) = source.get_own_property_names(scope, Default::default()) else {
        return false;
    };
    let mut copied = 0;
    for index in 0..names.length() {
        let Some(key) = names.get_index(scope, index) else {
            continue;
        };
        let Some(value) = source.get(scope, key) else {
            continue;
        };
        if target.set(scope, key, value).unwrap_or(false) {
            copied += 1;
        }
    }
    // The snapshot has no document wrappers yet. Connect its private
    // receiver routing before initialization or any frame script runs.
    let Some(binder_key) = v8::String::new(scope, "__obscura_bind_checked_receivers_handoff") else { return false; };
    let Some(binder) = global.get(scope, binder_key.into())
        .and_then(|value| v8::Local::<v8::Function>::try_from(value).ok()) else { return false; };
    if !global.delete(scope, binder_key.into()).unwrap_or(false) { return false; }
    let Some(registry_key) = v8::String::new(scope, "__obscura_checked_receivers_handoff") else { return false; };
    if !global.delete(scope, registry_key.into()).unwrap_or(false) { return false; }
    let registry = v8::Local::new(scope, checked_receivers);
    let receiver = v8::undefined(scope).into();
    if binder.call(scope, receiver, &[registry]).is_none() { return false; }
    // Child realms cannot expose native input authority either.
    for name in ["__obscura_native_mouse_handoff", "__obscura_native_wheel_handoff", "__obscura_native_scroll_handoff", "__obscura_native_focus_handoff", "__obscura_native_text_handoff", "__obscura_native_keyboard_handoff", "__obscura_native_submit_handoff", "__obscura_native_fragment_handoff", "__obscura_native_lifecycle_handoff"] {
        let Some(input_key) = v8::String::new(scope, name) else {
            return false;
        };
        if !global.delete(scope, input_key.into()).unwrap_or(false) {
            return false;
        }
    }
    // The child realm must not expose the handoff to frame script either.
    global.delete(scope, handoff_key.into());
    copied > 0
}

pub(crate) fn copy_identity(
    scope: &mut v8::HandleScope<'_, ()>,
    main: v8::Global<v8::Context>,
    realm: &v8::Global<v8::Context>,
) {
    const IDENTITY_GLOBALS: [&str; 14] = [
        "__obscura_ua",
        "__obscura_ua_brands",
        "__obscura_platform",
        "__obscura_ua_platform",
        "__obscura_ua_platform_version",
        "__obscura_ua_full_version",
        "__obscura_ua_architecture",
        "__obscura_do_not_track",
        "__obscura_language",
        "__obscura_languages",
        "__obscura_webgl_vendor",
        "__obscura_webgl_renderer",
        "__obscura_geo_lat",
        "__obscura_geo_lon",
    ];

    let main_context = v8::Local::new(scope, main);
    let mut carried = Vec::new();
    {
        let scope = &mut v8::ContextScope::new(scope, main_context);
        let global = main_context.global(scope);
        for name in IDENTITY_GLOBALS {
            let Some(key) = v8::String::new(scope, name) else {
                continue;
            };
            match global.get(scope, key.into()) {
                Some(value) if !value.is_undefined() => {
                    carried.push((name, v8::Global::new(scope, value)));
                }
                _ => {}
            }
        }
    }

    let realm_context = v8::Local::new(scope, realm);
    let scope = &mut v8::ContextScope::new(scope, realm_context);
    let global = realm_context.global(scope);
    for (name, value) in carried {
        let Some(key) = v8::String::new(scope, name) else {
            continue;
        };
        let value = v8::Local::new(scope, value);
        global.set(scope, key.into(), value);
    }
}

/// Caller retains the loaded-frame origin decision. This is not blank policy.
pub(crate) fn share_security_token(
    scope: &mut v8::HandleScope<'_, ()>,
    main: v8::Global<v8::Context>,
    realm: &v8::Global<v8::Context>,
) {
    let main = v8::Local::new(scope, main);
    let realm = v8::Local::new(scope, realm);
    let token = main.get_security_token(scope);
    realm.set_security_token(token);
}
