//! Dedicated Worker realms.
//!
//! A Web Worker is an isolated JavaScript execution realm: its own global object
//! (`DedicatedWorkerGlobalScope`), its own ECMAScript intrinsics, and its own event
//! loop dispatch. It communicates with the parent browsing context exclusively via
//! structured message passing (`postMessage`), ensuring no shared mutable state.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use deno_core::op2;
use deno_core::v8;
use deno_core::OpState;

/// Global holding the ops table taken at runtime startup.
pub struct OpsHandoff(pub v8::Global<v8::Value>);

/// Registry of active worker instances.
#[derive(Default)]
pub struct WorkerRegistry {
    pub next_id: u32,
    pub main_context: Option<v8::Global<v8::Context>>,
    pub workers: HashMap<u32, WorkerInstance>,
}

pub struct WorkerInstance {
    pub id: u32,
    pub context: v8::Global<v8::Context>,
    pub terminated: bool,
}

/// Creates a new `v8::Context` for a Worker from the snapshot.
pub fn create_worker_context<'s>(scope: &mut v8::HandleScope<'s>) -> Option<v8::Local<'s, v8::Context>> {
    deno_core::v8::Context::from_snapshot(
        scope,
        1,
        deno_core::v8::ContextOptions::default(),
    )
    .or_else(|| {
        deno_core::v8::Context::from_snapshot(
            scope,
            0,
            deno_core::v8::ContextOptions::default(),
        )
    })
    .or_else(|| {
        Some(deno_core::v8::Context::new(
            scope,
            deno_core::v8::ContextOptions::default(),
        ))
    })
}

/// Aliases the main context's Deno embedder slots so promise rejections and module
/// loaders in the worker realm don't crash deno_core global callbacks.
pub fn share_deno_context_state(
    main_ctx: v8::Local<v8::Context>,
    worker_ctx: v8::Local<v8::Context>,
) {
    use deno_core::{CONTEXT_STATE_SLOT_INDEX, MODULE_MAP_SLOT_INDEX};
    unsafe {
        let cs = main_ctx.get_aligned_pointer_from_embedder_data(CONTEXT_STATE_SLOT_INDEX);
        let mm = main_ctx.get_aligned_pointer_from_embedder_data(MODULE_MAP_SLOT_INDEX);
        worker_ctx.set_aligned_pointer_in_embedder_data(CONTEXT_STATE_SLOT_INDEX, cs);
        worker_ctx.set_aligned_pointer_in_embedder_data(MODULE_MAP_SLOT_INDEX, mm);
    }
}

/// Copies bound op functions into the worker context's `Deno.core.ops`.
pub fn share_ops_with_context(
    scope: &mut v8::HandleScope,
    context: v8::Local<v8::Context>,
    ops_handoff: &v8::Global<v8::Value>,
) -> bool {
    let scope = &mut v8::ContextScope::new(scope, context);
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
    let Some(target) = core
        .get(scope, ops_key.into())
        .and_then(|value| value.to_object(scope))
    else {
        return false;
    };
    let source = v8::Local::new(scope, ops_handoff);
    let Some(source) = source.to_object(scope) else {
        return false;
    };
    let Some(names) = source.get_own_property_names(scope, Default::default()) else {
        return false;
    };
    for index in 0..names.length() {
        let Some(key) = names.get_index(scope, index) else {
            continue;
        };
        let Some(value) = source.get(scope, key) else {
            continue;
        };
        let _ = target.set(scope, key, value);
    }
    for name in [
        "__obscura_native_mouse_handoff",
        "__obscura_native_focus_handoff",
        "__obscura_native_text_handoff",
        "__obscura_native_submit_handoff",
        "__obscura_native_fragment_handoff",
        "__obscura_native_lifecycle_handoff",
    ] {
        if let Some(key) = v8::String::new(scope, name) {
            let _ = global.delete(scope, key.into());
        }
    }
    let _ = global.delete(scope, handoff_key.into());
    true
}

/// Copies browser identity globals from `source_ctx` to `target_ctx`.
pub fn copy_identity_to_context(
    scope: &mut v8::HandleScope,
    source_ctx: v8::Local<v8::Context>,
    target_ctx: v8::Local<v8::Context>,
) {
    const IDENTITY_GLOBALS: [&str; 14] = [
        "__obscura_ua",
        "__obscura_platform",
        "__obscura_ua_platform",
        "__obscura_ua_platform_version",
        "__obscura_ua_full_version",
        "__obscura_hardware_concurrency",
        "__obscura_device_memory",
        "__obscura_language",
        "__obscura_languages",
        "__obscura_accept_language",
        "__obscura_network_downlink",
        "__obscura_network_rtt",
        "__obscura_network_effective_type",
        "__obscura_network_save_data",
    ];
    for name in IDENTITY_GLOBALS {
        let (has_value, value) = {
            let scope = &mut v8::ContextScope::new(scope, source_ctx);
            let Some(key) = v8::String::new(scope, name) else {
                continue;
            };
            let global = source_ctx.global(scope);
            let value = global.get(scope, key.into());
            (value.is_some(), value.map(|v| v8::Global::new(scope, v)))
        };
        if has_value {
            if let Some(val) = value {
                let scope = &mut v8::ContextScope::new(scope, target_ctx);
                let Some(key) = v8::String::new(scope, name) else {
                    continue;
                };
                let val = v8::Local::new(scope, val);
                let global = target_ctx.global(scope);
                let _ = global.set(scope, key.into(), val);
            }
        }
    }
}

fn exception_text(
    scope: &mut v8::TryCatch<'_, v8::HandleScope<'_>>,
) -> String {
    match scope.exception() {
        Some(exception) => exception.to_rust_string_lossy(scope),
        None => "unknown error".to_string(),
    }
}

fn extract_exception_message(
    scope: &mut v8::TryCatch<'_, v8::HandleScope<'_>>,
) -> Option<String> {
    if let Some(obj) = scope.exception().and_then(|e| e.to_object(scope)) {
        if let Some(msg_key) = v8::String::new(scope, "message") {
            if let Some(msg_val) = obj.get(scope, msg_key.into()) {
                if msg_val.is_string() {
                    return Some(msg_val.to_rust_string_lossy(scope));
                }
            }
        }
    }
    Some(exception_text(scope))
}

const WORKER_BOOTSTRAP_JS: &str = r#"
(function(workerId, workerUrl) {
    delete globalThis.window;
    delete globalThis.document;
    delete globalThis.location;
    delete globalThis.history;
    delete globalThis.localStorage;
    delete globalThis.sessionStorage;
    delete globalThis.HTMLDocument;
    delete globalThis.Document;
    delete globalThis.Element;
    delete globalThis.HTMLElement;
    delete globalThis.Node;
    delete globalThis.alert;
    delete globalThis.confirm;
    delete globalThis.prompt;
    delete globalThis.parent;
    delete globalThis.top;
    delete globalThis.frames;
    delete globalThis.Window;
    delete globalThis.onmessage;
    delete globalThis.onerror;
    delete globalThis.onmessageerror;
    delete globalThis.addEventListener;
    delete globalThis.removeEventListener;
    delete globalThis.dispatchEvent;
    delete globalThis.EventTarget;

    globalThis.self = globalThis;

    function EventTarget() {
        this._listeners = new Map();
    }
    EventTarget.prototype.addEventListener = function(type, listener, options) {
        if (listener == null) return;
        if (typeof listener !== 'function' && typeof listener.handleEvent !== 'function') return;
        const target = (this == null || this === globalThis) ? globalThis : this;
        type = String(type);
        if (!target._listeners) target._listeners = new Map();
        let list = target._listeners.get(type);
        if (!list) {
            list = [];
            target._listeners.set(type, list);
        }
        if (list.some(e => e.listener === listener)) return;
        const once = !!(options && typeof options === 'object' && options.once);
        list.push({ listener, once });
    };
    EventTarget.prototype.removeEventListener = function(type, listener) {
        const target = (this == null || this === globalThis) ? globalThis : this;
        if (!target._listeners) return;
        type = String(type);
        const list = target._listeners.get(type);
        if (!list) return;
        const idx = list.findIndex(e => e.listener === listener);
        if (idx !== -1) list.splice(idx, 1);
    };
    EventTarget.prototype.dispatchEvent = function(event) {
        if (!event || typeof event.type === 'undefined') {
            throw new TypeError("Failed to execute 'dispatchEvent' on 'EventTarget': parameter 1 is not of type 'Event'.");
        }
        const target = (this == null || this === globalThis) ? globalThis : this;
        const type = String(event.type);
        if (!target._listeners) return true;
        const list = (target._listeners.get(type) || []).slice();
        event.target = target;
        event.currentTarget = target;
        for (const entry of list) {
            if (entry.once) target.removeEventListener(type, entry.listener);
            const fn = entry.listener;
            try {
                if (typeof fn === 'function') fn.call(target, event);
                else fn.handleEvent.call(fn, event);
            } catch (e) {
                console.error(e);
            }
            if (event._immediatePropagationStopped) break;
        }
        event.currentTarget = null;
        return !event.defaultPrevented;
    };
    Object.defineProperty(EventTarget.prototype, Symbol.toStringTag, {
        value: 'EventTarget', configurable: true,
    });
    Object.defineProperty(EventTarget.prototype, 'constructor', {
        value: EventTarget, writable: true, configurable: true,
    });
    globalThis.EventTarget = EventTarget;
    globalThis.addEventListener = EventTarget.prototype.addEventListener;
    globalThis.removeEventListener = EventTarget.prototype.removeEventListener;
    globalThis.dispatchEvent = EventTarget.prototype.dispatchEvent;

    if (globalThis.MessageEvent) {
        globalThis.MessageEvent.prototype.stopImmediatePropagation = function() {
            this._immediatePropagationStopped = true;
        };
    }

    function WorkerGlobalScope() {}
    Object.setPrototypeOf(WorkerGlobalScope.prototype, EventTarget.prototype);
    Object.defineProperty(WorkerGlobalScope.prototype, Symbol.toStringTag, {
        value: 'WorkerGlobalScope', configurable: true,
    });
    Object.defineProperty(WorkerGlobalScope.prototype, 'constructor', {
        value: WorkerGlobalScope, writable: true, configurable: true,
    });
    globalThis.WorkerGlobalScope = WorkerGlobalScope;

    function DedicatedWorkerGlobalScope() {}
    Object.setPrototypeOf(DedicatedWorkerGlobalScope.prototype, WorkerGlobalScope.prototype);
    Object.defineProperty(DedicatedWorkerGlobalScope.prototype, Symbol.toStringTag, {
        value: 'DedicatedWorkerGlobalScope', configurable: true,
    });
    Object.defineProperty(DedicatedWorkerGlobalScope.prototype, 'constructor', {
        value: DedicatedWorkerGlobalScope, writable: true, configurable: true,
    });
    globalThis.DedicatedWorkerGlobalScope = DedicatedWorkerGlobalScope;

    Object.setPrototypeOf(globalThis, DedicatedWorkerGlobalScope.prototype);
    Object.defineProperty(globalThis, Symbol.toStringTag, {
        value: 'DedicatedWorkerGlobalScope', configurable: true,
    });
    Object.defineProperty(globalThis, 'constructor', {
        value: DedicatedWorkerGlobalScope, writable: true, configurable: true,
    });

    function WorkerNavigator() {}
    Object.defineProperty(WorkerNavigator.prototype, Symbol.toStringTag, {
        value: 'WorkerNavigator', configurable: true,
    });
    Object.defineProperty(WorkerNavigator.prototype, 'constructor', {
        value: WorkerNavigator, writable: true, configurable: true,
    });
    const workerNav = Object.create(WorkerNavigator.prototype);
    const navProps = [
        'userAgent', 'platform', 'hardwareConcurrency', 'deviceMemory',
        'language', 'languages', 'onLine', 'userAgentData', 'storage',
        'locks', 'mediaCapabilities', 'permissions', 'gpu',
    ];
    for (const prop of navProps) {
        if (globalThis.navigator && prop in globalThis.navigator) {
            const val = globalThis.navigator[prop];
            Object.defineProperty(WorkerNavigator.prototype, prop, {
                configurable: true, enumerable: true, get() { return val; }
            });
        }
    }
    globalThis.WorkerNavigator = WorkerNavigator;
    globalThis.navigator = workerNav;

    function WorkerLocation() {}
    Object.defineProperty(WorkerLocation.prototype, Symbol.toStringTag, {
        value: 'WorkerLocation', configurable: true,
    });
    Object.defineProperty(WorkerLocation.prototype, 'constructor', {
        value: WorkerLocation, writable: true, configurable: true,
    });
    Object.defineProperty(WorkerLocation.prototype, 'toString', {
        value: function toString() { return this.href; },
        writable: true, configurable: true, enumerable: false,
    });
    Object.defineProperty(WorkerLocation.prototype, 'valueOf', {
        value: function valueOf() { return this.href; },
        writable: true, configurable: true, enumerable: false,
    });
    const workerLoc = Object.create(WorkerLocation.prototype);
    let parsedUrl;
    try {
        parsedUrl = new URL(workerUrl);
    } catch (e) {
        parsedUrl = { href: workerUrl, origin: '', protocol: '', host: '', hostname: '', port: '', pathname: '', search: '', hash: '' };
    }
    for (const k of ['href', 'origin', 'protocol', 'host', 'hostname', 'port', 'pathname', 'search', 'hash']) {
        Object.defineProperty(WorkerLocation.prototype, k, { configurable: true, enumerable: true, get() { return String(parsedUrl[k] || ''); } });
    }
    globalThis.WorkerLocation = WorkerLocation;
    Object.defineProperty(WorkerGlobalScope.prototype, 'location', {
        get() { return workerLoc; },
        configurable: true,
        enumerable: true,
    });
    Object.defineProperty(globalThis, 'location', {
        get() { return workerLoc; },
        configurable: true,
        enumerable: true,
    });

    let _workerOnMessageHandler = null;
    let _workerOnMessageWrapper = null;
    const onmessageDescriptor = {
        configurable: true,
        enumerable: true,
        get() {
            return _workerOnMessageHandler;
        },
        set(fn) {
            _workerOnMessageHandler = (typeof fn === 'function' || (fn && typeof fn.handleEvent === 'function')) ? fn : null;
            if (!_workerOnMessageWrapper) {
                _workerOnMessageWrapper = function(event) {
                    if (typeof _workerOnMessageHandler === 'function') {
                        _workerOnMessageHandler.call(globalThis, event);
                    } else if (_workerOnMessageHandler && typeof _workerOnMessageHandler.handleEvent === 'function') {
                        _workerOnMessageHandler.handleEvent.call(_workerOnMessageHandler, event);
                    }
                };
                globalThis.addEventListener('message', _workerOnMessageWrapper);
            }
        }
    };
    Object.defineProperty(DedicatedWorkerGlobalScope.prototype, 'onmessage', onmessageDescriptor);
    Object.defineProperty(globalThis, 'onmessage', onmessageDescriptor);

    let _workerOnError = null;
    let _workerOnErrorWrapper = null;
    const onerrorDescriptor = {
        configurable: true,
        enumerable: true,
        get() { return _workerOnError; },
        set(val) {
            _workerOnError = typeof val === 'function' || (val && typeof val.handleEvent === 'function') ? val : null;
            if (!_workerOnErrorWrapper) {
                _workerOnErrorWrapper = function(event) {
                    if (typeof _workerOnError === 'function') {
                        _workerOnError.call(globalThis, event);
                    } else if (_workerOnError && typeof _workerOnError.handleEvent === 'function') {
                        _workerOnError.handleEvent.call(_workerOnError, event);
                    }
                };
                globalThis.addEventListener('error', _workerOnErrorWrapper);
            }
        },
    };
    Object.defineProperty(DedicatedWorkerGlobalScope.prototype, 'onerror', onerrorDescriptor);
    Object.defineProperty(globalThis, 'onerror', onerrorDescriptor);

    function _serializeWorkerMsg(msg) {
        try {
            return JSON.stringify({ v: msg }, (key, val) => {
                if (typeof val === 'string' && val.startsWith('__obscura_')) return '__obscura_str_' + val;
                if (val === undefined) return '__obscura_val_undefined__';
                return val;
            });
        } catch (_) {
            throw new DOMException('The object could not be cloned.', 'DataCloneError');
        }
    }

    function _deserializeWorkerMsg(json) {
        try {
            const raw = JSON.parse(json);
            function restore(obj) {
                if (!obj || typeof obj !== 'object') return;
                if (Array.isArray(obj)) {
                    for (let i = 0; i < obj.length; i++) {
                        if (obj[i] === '__obscura_val_undefined__') obj[i] = undefined;
                        else restore(obj[i]);
                    }
                } else {
                    for (const k of Object.keys(obj)) {
                        if (obj[k] === '__obscura_val_undefined__') {
                            obj[k] = undefined;
                        } else if (typeof obj[k] === 'string' && obj[k].startsWith('__obscura_str_')) {
                            obj[k] = obj[k].slice('__obscura_str_'.length);
                        } else {
                            restore(obj[k]);
                        }
                    }
                }
            }
            if (raw && raw.v === '__obscura_val_undefined__') {
                raw.v = undefined;
            } else {
                restore(raw);
            }
            return raw;
        } catch (_) {
            return null;
        }
    }

    DedicatedWorkerGlobalScope.prototype.postMessage = function(msg) {
        const json = _serializeWorkerMsg(msg);
        Deno.core.ops.op_worker_post_to_parent(workerId, json);
    };
    globalThis.postMessage = DedicatedWorkerGlobalScope.prototype.postMessage;

    DedicatedWorkerGlobalScope.prototype.close = function() {
        Deno.core.ops.op_worker_terminate(workerId);
    };
    globalThis.close = DedicatedWorkerGlobalScope.prototype.close;

    DedicatedWorkerGlobalScope.prototype.importScripts = function(...urls) {
        for (const rawUrl of urls) {
            let resolved = String(rawUrl);
            try { resolved = new URL(resolved, globalThis.location?.href || '').href; } catch (e) {}
            const source = Deno.core.ops.op_worker_load_script(resolved);
            if (source === null || source === undefined) {
                throw new DOMException(`Failed to execute 'importScripts': The script at '${resolved}' could not be loaded.`, 'NetworkError');
            }
            (0, eval)(source);
        }
    };
    globalThis.importScripts = DedicatedWorkerGlobalScope.prototype.importScripts;

    globalThis.__obscura_worker_receive = function(json) {
        const payload = _deserializeWorkerMsg(json);
        if (!payload) return;
        const event = new MessageEvent('message', { data: payload.v });
        try { Object.defineProperties(event, { target: { value: globalThis }, currentTarget: { value: globalThis } }); } catch (e) {}
        globalThis.dispatchEvent(event);
    };
})
"#;

#[op2(fast)]
pub fn op_worker_create(
    scope: &mut v8::HandleScope,
    state: &OpState,
    #[string] url: &str,
) -> u32 {
    let Some(registry_rc) = state.try_borrow::<Rc<RefCell<WorkerRegistry>>>().cloned() else {
        return 0;
    };

    let main_ctx = scope.get_current_context();
    {
        let mut reg = registry_rc.borrow_mut();
        if reg.main_context.is_none() {
            reg.main_context = Some(v8::Global::new(scope, main_ctx));
        }
    }

    let Some(worker_ctx) = create_worker_context(scope) else {
        return 0;
    };

    share_deno_context_state(main_ctx, worker_ctx);

    if let Some(ops_handoff) = state.try_borrow::<OpsHandoff>() {
        share_ops_with_context(scope, worker_ctx, &ops_handoff.0);
    }

    copy_identity_to_context(scope, main_ctx, worker_ctx);

    let worker_id = {
        let mut reg = registry_rc.borrow_mut();
        reg.next_id = reg.next_id.saturating_add(1);
        reg.next_id
    };

    // Run bootstrap init in the worker realm
    let escaped_url = serde_json::to_string(url).unwrap_or_else(|_| "\"\"".to_string());
    let init_script = format!("({WORKER_BOOTSTRAP_JS})({worker_id}, {escaped_url});");
    {
        let scope = &mut v8::ContextScope::new(scope, worker_ctx);
        let scope = &mut v8::TryCatch::new(scope);
        if let Some(code) = v8::String::new(scope, &init_script) {
            if let Some(script) = v8::Script::compile(scope, code, None) {
                if script.run(scope).is_none() {
                    let err = exception_text(scope);
                    eprintln!("worker init script error: {}", err);
                }
            } else {
                let err = exception_text(scope);
                eprintln!("worker init script compile error: {}", err);
            }
        }
    }

    {
        let mut reg = registry_rc.borrow_mut();
        reg.workers.insert(
            worker_id,
            WorkerInstance {
                id: worker_id,
                context: v8::Global::new(scope, worker_ctx),
                terminated: false,
            },
        );
    }

    worker_id
}

#[op2(reentrant)]
#[string]
pub fn op_worker_run(
    scope: &mut v8::HandleScope,
    state: &OpState,
    worker_id: u32,
    #[string] source: &str,
) -> Option<String> {
    let registry_rc = state.try_borrow::<Rc<RefCell<WorkerRegistry>>>()?.clone();
    let worker_context = {
        let registry = registry_rc.borrow();
        let worker = registry.workers.get(&worker_id)?;
        if worker.terminated {
            return None;
        }
        worker.context.clone()
    };
    let worker_ctx = v8::Local::new(scope, &worker_context);
    let scope = &mut v8::ContextScope::new(scope, worker_ctx);
    let scope = &mut v8::TryCatch::new(scope);

    let code = v8::String::new(scope, source)?;
    let script = match v8::Script::compile(scope, code, None) {
        Some(s) => s,
        None => return extract_exception_message(scope),
    };
    match script.run(scope) {
        Some(_) => None,
        None => extract_exception_message(scope),
    }
}

#[op2(fast)]
pub fn op_worker_post_to_worker(
    scope: &mut v8::HandleScope,
    state: &OpState,
    worker_id: u32,
    #[string] json: &str,
) {
    let Some(registry_rc) = state.try_borrow::<Rc<RefCell<WorkerRegistry>>>().cloned() else {
        return;
    };
    let worker_context = {
        let registry = registry_rc.borrow();
        let Some(worker) = registry.workers.get(&worker_id) else {
            return;
        };
        if worker.terminated {
            return;
        }
        worker.context.clone()
    };
    let worker_ctx = v8::Local::new(scope, &worker_context);
    let scope = &mut v8::ContextScope::new(scope, worker_ctx);
    let scope = &mut v8::TryCatch::new(scope);

    let global = worker_ctx.global(scope);
    if let Some(key) = v8::String::new(scope, "__obscura_worker_receive") {
        if let Some(func_val) = global.get(scope, key.into()) {
            if let Ok(func) = v8::Local::<v8::Function>::try_from(func_val) {
                if let Some(json_val) = v8::String::new(scope, json) {
                    let undefined = v8::undefined(scope).into();
                    func.call(scope, undefined, &[json_val.into()]);
                }
            }
        }
    }
}

#[op2(fast)]
pub fn op_worker_post_to_parent(
    scope: &mut v8::HandleScope,
    state: &OpState,
    worker_id: u32,
    #[string] json: &str,
) {
    let Some(registry_rc) = state.try_borrow::<Rc<RefCell<WorkerRegistry>>>().cloned() else {
        return;
    };
    let main_context = registry_rc.borrow().main_context.clone();
    let Some(main_ctx) = main_context else { return };

    let json_val = v8::String::new(scope, json);
    let main_ctx = v8::Local::new(scope, &main_ctx);
    let scope = &mut v8::ContextScope::new(scope, main_ctx);
    let scope = &mut v8::TryCatch::new(scope);

    let global = main_ctx.global(scope);
    if let Some(key) = v8::String::new(scope, "__obscura_worker_dispatch_to_page") {
        if let Some(func_val) = global.get(scope, key.into()) {
            if let Ok(func) = v8::Local::<v8::Function>::try_from(func_val) {
                let id_val = v8::Integer::new_from_unsigned(scope, worker_id);
                let undefined = v8::undefined(scope).into();
                if let Some(json_v) = json_val {
                    func.call(scope, undefined, &[id_val.into(), json_v.into()]);
                }
            }
        }
    }
}

#[op2(fast)]
pub fn op_worker_terminate(state: &OpState, worker_id: u32) {
    if let Some(registry_rc) = state.try_borrow::<Rc<RefCell<WorkerRegistry>>>() {
        let mut registry = registry_rc.borrow_mut();
        if let Some(worker) = registry.workers.get_mut(&worker_id) {
            worker.terminated = true;
        }
        registry.workers.remove(&worker_id);
    }
}

#[op2(reentrant)]
#[string]
pub fn op_worker_load_script(
    scope: &mut v8::HandleScope,
    state: &OpState,
    #[string] url: &str,
) -> Option<String> {
    let registry_rc = state.try_borrow::<Rc<RefCell<WorkerRegistry>>>()?.clone();
    let main_context = registry_rc.borrow().main_context.clone()?;
    let main_ctx = v8::Local::new(scope, &main_context);
    let scope = &mut v8::ContextScope::new(scope, main_ctx);

    let global = main_ctx.global(scope);
    let key = v8::String::new(scope, "__blobStore")?;
    let store = global.get(scope, key.into())?.to_object(scope)?;
    let url_key = v8::String::new(scope, url)?;
    let val = store.get(scope, url_key.into())?;
    if val.is_string() {
        Some(val.to_rust_string_lossy(scope))
    } else {
        None
    }
}
