use crate::runtime::ObscuraJsRuntime;
use deno_core::{v8, JsRuntime, RuntimeOptions};

fn browser_runtime() -> ObscuraJsRuntime {
    let mut runtime = ObscuraJsRuntime::new(obscura_net::EffectivePersona::builtin(
        obscura_net::StealthProfile::MacChrome153,
    ));
    runtime.set_dom(obscura_dom::parse_html("<html><body></body></html>"));
    runtime.set_url("https://example.com/test");
    runtime.run_page_init();
    runtime
}

const WEBIDL_GETTER_PROBE: &str = r#"function getterMetadataMatches(proto, key) {
    const descriptor = Object.getOwnPropertyDescriptor(proto, key);
    const get = descriptor.get;
    const name = Object.getOwnPropertyDescriptor(get, 'name');
    const length = Object.getOwnPropertyDescriptor(get, 'length');
    let nonConstructible = false;
    try { Reflect.construct(function() {}, [], get); }
    catch (error) { nonConstructible = error instanceof TypeError; }
    return descriptor.enumerable && descriptor.configurable && descriptor.set === undefined &&
        name.value === 'get ' + key && !name.writable && !name.enumerable && name.configurable &&
        length.value === 0 && !length.writable && !length.enumerable && length.configurable &&
        !Object.hasOwn(get, 'prototype') && nonConstructible &&
        Function.prototype.toString.call(get) === 'function get ' + key + '() { [native code] }';
}"#;

#[test]
fn text_metrics_getters_keep_webidl_metadata_and_genuine_receiver_values() {
    let mut runtime = browser_runtime();
    runtime.execute_script("webidl-getter-probe", WEBIDL_GETTER_PROBE).unwrap();
    assert_eq!(runtime.evaluate(r#"(() => {
        const ctx = document.createElement('canvas').getContext('2d');
        ctx.font = '31.25px serif';
        const metrics = ctx.measureText('iii');
        const wide = ctx.measureText('WWW');
        const proto = TextMetrics.prototype;
        const keys = ['width', 'actualBoundingBoxLeft', 'actualBoundingBoxRight',
            'actualBoundingBoxAscent', 'actualBoundingBoxDescent',
            'fontBoundingBoxAscent', 'fontBoundingBoxDescent'];
        const rejects = fn => { try { fn(); return false; } catch(error) { return error instanceof TypeError; } };
        const fake = [proto, {}, null, undefined, Object.create(proto), new Proxy(metrics, {})];
        const branded = keys.every(key => {
            const get = Object.getOwnPropertyDescriptor(proto, key).get;
            return get.call(metrics) === metrics[key] && fake.every(value => rejects(() => get.call(value)));
        });
        const width = Object.getOwnPropertyDescriptor(proto, 'width').get;
        const originalWidth = metrics.width;
        Object.setPrototypeOf(metrics, null);
        const stillBranded = width.call(metrics) === originalWidth;
        return {metadata: keys.every(key => getterMetadataMatches(proto, key)), branded, stillBranded,
            ownKeys: Reflect.ownKeys(metrics), illegalConstructor: rejects(() => new TextMetrics()),
            realAdvances: originalWidth > 0 && wide.width > originalWidth * 2,
            privateBindings: !Object.hasOwn(globalThis, '__obscura_core_handoff') &&
                !Object.hasOwn(globalThis, 'registerNativeBindings'),
            constructorSource: Function.prototype.toString.call(TextMetrics)};
    })()"#).unwrap(), serde_json::json!({
        "metadata":true, "branded":true, "stillBranded":true, "ownKeys":[],
        "illegalConstructor":true, "realAdvances":true, "privateBindings":true,
        "constructorSource":"function TextMetrics() { [native code] }",
    }));
}

#[test]
fn navigator_brand_rejects_forged_receivers_for_all_attributes_and_operations() {
    let mut runtime = browser_runtime();
    assert_eq!(runtime.evaluate(r#"(() => {
        const proto = Navigator.prototype;
        const fake = [proto, {}, null, undefined, Object.create(proto), new Proxy(navigator, {})];
        const attributes = Object.getOwnPropertyNames(proto).filter(key =>
            typeof Object.getOwnPropertyDescriptor(proto, key).get === 'function');
        const rejects = fn => { try { fn(); return false; } catch(e) { return e instanceof TypeError; } };
        const allBranded = attributes.every(key => {
            const get = Object.getOwnPropertyDescriptor(proto, key).get;
            return fake.every(receiver => rejects(() => get.call(receiver))) &&
                !Object.hasOwn(get, 'prototype');
        });
        const operations = ['getGamepads', 'sendBeacon', 'javaEnabled', 'canShare',
            'registerProtocolHandler', 'unregisterProtocolHandler', 'vibrate'];
        const operationsBranded = operations.every(key => fake.every(receiver =>
            rejects(() => proto[key].call(receiver, 'web+test', 'https://example.com/%s'))));
        const genuine = Object.getOwnPropertyDescriptor(proto, 'webdriver').get;
        const savedPrototype = Object.getPrototypeOf(navigator);
        Object.setPrototypeOf(navigator, null);
        const stillBranded = genuine.call(navigator) === false;
        Object.setPrototypeOf(navigator, savedPrototype);
        return [attributes.length > 20, allBranded, operationsBranded, stillBranded,
            navigator.webdriver, Object.getOwnPropertyNames(navigator),
            Object.hasOwn(globalThis, '__obscura_navigator_registry_handoff')];
    })()"#).unwrap(), serde_json::json!([true, true, true, true, false, [], false]));
}

#[test]
fn navigator_borrowed_members_use_genuine_cross_realm_receiver_state() {
    let mut runtime = browser_runtime();
    let child = runtime.create_realm_context().unwrap();
    runtime.share_deno_context_state_with_realm(&child);
    assert!(runtime.share_ops_with_realm(&child));
    runtime.share_security_token_with_realm(&child);
    let main = runtime.runtime().main_context();
    {
        let mut entered = runtime.runtime();
        let scope = &mut v8::HandleScope::with_context(entered.v8_isolate(), main);
        let parent = scope.get_current_context().global(scope);
        let child_global = v8::Local::new(scope, &child).global(scope);
        let key = v8::String::new(scope, "navigator").unwrap();
        let parent_nav = parent.get(scope, key.into()).unwrap();
        let child_nav = child_global.get(scope, key.into()).unwrap();
        let other = v8::String::new(scope, "otherNavigator").unwrap();
        assert!(parent.set(scope, other.into(), child_nav).unwrap());
        assert!(child_global.set(scope, other.into(), parent_nav).unwrap());
    }
    runtime.evaluate("globalThis.__obscura_language = 'parent-language'").unwrap();
    runtime.eval_in_realm(&child, "globalThis.__obscura_language = 'child-language'").unwrap();
    let probe = r#"JSON.stringify((() => {
        const get = name => Object.getOwnPropertyDescriptor(Navigator.prototype, name).get;
        return [get('language').call(otherNavigator),
            get('plugins').call(otherNavigator) === otherNavigator.plugins,
            get('mediaCapabilities').call(otherNavigator) === otherNavigator.mediaCapabilities,
            Navigator.prototype.javaEnabled.call(otherNavigator),
            Object.hasOwn(globalThis, '__obscura_navigator_registry_handoff')];
    })())"#;
    assert_eq!(runtime.evaluate(probe).unwrap(), serde_json::json!("[\"child-language\",true,true,false,false]"));
    assert_eq!(runtime.eval_in_realm(&child, probe).unwrap(), "[\"parent-language\",true,true,false,false]");
}

#[tokio::test(flavor = "current_thread")]
async fn navigator_promise_operations_reject_bad_receivers() {
    let mut runtime = browser_runtime();
    runtime.execute_script("navigator-promises", r#"
        globalThis.receiverResults = null;
        Promise.all(['getBattery', 'share'].map(key =>
            Navigator.prototype[key].call({}).then(() => false, error => error instanceof TypeError)
        )).then(value => { receiverResults = value; });
    "#).unwrap();
    runtime.run_event_loop_bounded(100).await.unwrap();
    assert_eq!(runtime.evaluate("receiverResults").unwrap(), serde_json::json!([true, true]));
}

// Compare host defaults directly to V8's CallSite display, independent of any
// fingerprinting site's regular expressions or scoring rules.
const STACK_PROBE: &str = r#"function stackProbe() {
    function makeError(fn) { try { fn instanceof fn; } catch (error) { return error; } }
    const methods = [Date.prototype.getTime, Math.acos, Function.prototype.toString];
    if (typeof Document !== 'undefined') methods.push(Document.prototype.createElement);
    const results = methods.flatMap(fn => [fn, new Proxy(fn, {})]).map(fn => {
        const error = makeError(fn);
        const first = error.stack.split('\n')[1];
        Error.prepareStackTrace = (error, frames) => frames[0].toString();
        let original;
        try { original = makeError(fn).stack; } finally { delete Error.prepareStackTrace; }
        return error instanceof TypeError && first === '    at ' + original;
    });
    const marker = {};
    let gets = 0, calls = 0, valid = false;
    Object.defineProperty(Error, 'prepareStackTrace', { configurable: true, get() {
        gets++;
        return function(error, frames) {
            calls++;
            valid = this === Error && error.message === 'custom' && frames.length > 0;
            return marker;
        };
    } });
    const custom = new Error('custom');
    const customValue = custom.stack === marker && custom.stack === marker;
    delete Error.prepareStackTrace;
    return {nativeFrames: results.every(Boolean), gets, calls, valid, customValue,
        defaultHookAbsent: !Object.hasOwn(Error, 'prepareStackTrace')};
}"#;

fn expected_stack_probe() -> serde_json::Value {
    serde_json::json!({"nativeFrames":true, "gets":1, "calls":1, "valid":true,
        "customValue":true, "defaultHookAbsent":true})
}

#[test]
fn browser_stack_preserves_v8_frames_and_user_prepare_stack_trace() {
    let mut runtime = browser_runtime();
    runtime.execute_script("stack-probe", STACK_PROBE).unwrap();
    assert_eq!(runtime.evaluate("stackProbe()").unwrap(), expected_stack_probe());
}

#[tokio::test(flavor = "current_thread")]
async fn worker_navigator_brand_and_default_stack_use_browser_semantics() {
    let mut runtime = browser_runtime();
    let source = format!(r#"{}
        {}
        const proto = WorkerNavigator.prototype;
        const rejects = fn => {{ try {{ fn(); return false; }} catch(e) {{ return e instanceof TypeError; }} }};
        const fake = [proto, {{}}, null, undefined, Object.create(proto), new Proxy(navigator, {{}})];
        const keys = Object.getOwnPropertyNames(proto).filter(key =>
            typeof Object.getOwnPropertyDescriptor(proto, key).get === 'function');
        const branded = keys.every(key => {{
            const get = Object.getOwnPropertyDescriptor(proto, key).get;
            return fake.every(receiver => rejects(() => get.call(receiver))) && get.call(navigator) === navigator[key];
        }});
        const metadata = keys.every(key => getterMetadataMatches(proto, key));
        const noWindowAudio = ['AudioBuffer', 'AudioParam', 'AudioNode', 'AudioDestinationNode',
            'AudioScheduledSourceNode', 'OscillatorNode', 'GainNode', 'DynamicsCompressorNode', 'BiquadFilterNode',
            'AnalyserNode', 'BaseAudioContext', 'AudioContext', 'OfflineAudioContext',
            'AudioWorkletNode', 'ScriptProcessorNode', 'webkitAudioContext',
            'webkitOfflineAudioContext'].every(key => !(key in globalThis));
        const get = Object.getOwnPropertyDescriptor(proto, 'language').get;
        const language = navigator.language;
        Object.setPrototypeOf(navigator, null);
        const stillBranded = get.call(navigator) === language;
        Object.setPrototypeOf(navigator, proto);
        postMessage({{branded, metadata, stillBranded, noWindowAudio,
            constructorSource: Function.prototype.toString.call(WorkerNavigator),
            privateBindings: !Object.hasOwn(globalThis, '__obscura_core_handoff') &&
                !Object.hasOwn(globalThis, 'registerNativeBindings'),
            ownKeys: Reflect.ownKeys(navigator), illegalConstructor: rejects(() => new WorkerNavigator()),
            noWebdriver: !('webdriver' in navigator), stack: stackProbe()}});
    "#, STACK_PROBE, WEBIDL_GETTER_PROBE);
    let source = serde_json::to_string(&source).unwrap();
    runtime.execute_script("worker-browser-compat", &format!(r#"
        globalThis.workerCompat = null;
        const url = URL.createObjectURL(new Blob([{}], {{type:'application/javascript'}}));
        const worker = new Worker(url);
        worker.onmessage = e => {{ workerCompat = e.data; worker.terminate(); URL.revokeObjectURL(url); }};
    "#, source)).unwrap();
    runtime.run_event_loop_bounded(1000).await.unwrap();
    assert_eq!(runtime.evaluate("workerCompat").unwrap(), serde_json::json!({
        "branded":true, "metadata":true, "stillBranded":true, "noWindowAudio":true,
        "ownKeys":[], "privateBindings":true,
        "constructorSource":"function WorkerNavigator() { [native code] }",
        "illegalConstructor":true, "noWebdriver":true, "stack":expected_stack_probe(),
    }));
}

struct StackSourceMapLoader;

impl deno_core::ModuleLoader for StackSourceMapLoader {
    fn resolve(&self, _: &str, _: &str, _: deno_core::ResolutionKind)
        -> Result<deno_core::ModuleSpecifier, deno_core::error::ModuleLoaderError> { unreachable!() }
    fn load(&self, _: &deno_core::ModuleSpecifier, _: Option<&deno_core::ModuleSpecifier>,
        _: bool, _: deno_core::RequestedModuleType) -> deno_core::ModuleLoadResponse { unreachable!() }
    fn get_source_map(&self, name: &str) -> Option<std::borrow::Cow<'_, [u8]>> {
        (name == "https://example.test/generated.js").then(|| std::borrow::Cow::Borrowed(
            br#"{"version":3,"sources":["https://example.test/original.ts"],"names":[],"mappings":"AAUG"}"#.as_slice()
        ))
    }
}

#[test]
fn browser_stack_keeps_source_maps_eval_origins_and_exception_causes() {
    let mut runtime = JsRuntime::new(RuntimeOptions {
        module_loader: Some(std::rc::Rc::new(StackSourceMapLoader)),
        ..Default::default()
    });
    runtime.v8_isolate().set_prepare_stack_trace_callback(
        deno_core::error::prepare_stack_trace_callback_with_v8_display,
    );
    let deno_core::error::CoreError::Js(error) = runtime.execute_script(
        "https://example.test/generated.js", "throw new Error('outer', {cause: new TypeError('inner')});",
    ).unwrap_err() else { panic!("expected JavaScript error"); };
    assert_eq!(error.message.as_deref(), Some("outer"));
    assert!(error.stack.as_ref().unwrap().contains("https://example.test/original.ts:11:4"));
    let frame = &error.frames[0];
    assert_eq!(frame.file_name.as_deref(), Some("https://example.test/original.ts"));
    assert_eq!((frame.line_number, frame.column_number), (Some(11), Some(4)));
    let cause = error.cause.as_ref().unwrap();
    assert_eq!(cause.name.as_deref(), Some("TypeError"));
    assert_eq!(cause.message.as_deref(), Some("inner"));
    assert!(!cause.frames.is_empty());

    let deno_core::error::CoreError::Js(eval_error) = runtime.execute_script(
        "https://example.test/generated.js", "eval('throw new Error(\"eval\")');",
    ).unwrap_err() else { panic!("expected eval error"); };
    let eval_origin = eval_error.frames[0].eval_origin.as_ref().unwrap();
    assert!(eval_origin.contains("https://example.test/original.ts:11:4"));
    assert!(eval_error.stack.as_ref().unwrap().contains(eval_origin));
}

#[tokio::test(flavor = "current_thread")]
async fn mutation_observer_generic_wrapper_delivers_and_disconnects() {
    let mut runtime = browser_runtime();
    runtime.execute_script("observer-wrapper", r#"
        globalThis.observerWrapperResult = null;
        (async () => {
            const Original = MutationObserver;
            const methods = ['observe', 'disconnect', 'takeRecords'];
            const metadata = methods.every(name => {
                const descriptor = Object.getOwnPropertyDescriptor(Original.prototype, name);
                return descriptor.enumerable && descriptor.writable && descriptor.configurable &&
                    !Object.hasOwn(descriptor.value, 'prototype');
            });
            // Generic native-class wrapping: discover the original instance's
            // enumerable members and forward operations to that instance.
            const sample = new Original(() => {});
            const originals = new WeakMap();
            function Wrapped(...args) { originals.set(this, new Original(...args)); }
            for (const name in sample) {
                if (typeof sample[name] === 'function') {
                    Wrapped.prototype[name] = function(...args) {
                        return originals.get(this)[name](...args);
                    };
                }
            }
            const host = document.createElement('div');
            document.body.appendChild(host);
            const records = [];
            const observer = new Wrapped(batch => records.push(...batch));
            observer.observe(host, {childList: true});
            const first = document.createElement('span');
            host.appendChild(first);
            await Promise.resolve();
            const delivered = records.length === 1 && records[0].type === 'childList' &&
                records[0].target === host && records[0].addedNodes[0] === first;
            const takenNode = document.createElement('b');
            host.appendChild(takenNode);
            const taken = observer.takeRecords();
            await Promise.resolve();
            const drained = taken.length === 1 && taken[0].addedNodes[0] === takenNode &&
                records.length === 1 && observer.takeRecords().length === 0;
            host.appendChild(document.createElement('i'));
            observer.disconnect();
            host.appendChild(document.createElement('u'));
            await Promise.resolve();
            const disconnected = records.length === 1 && observer.takeRecords().length === 0;
            observer.observe(host, {childList: true});
            const last = document.createElement('em');
            host.appendChild(last);
            await Promise.resolve();
            const resumed = records.length === 2 && records[1].addedNodes[0] === last;
            observer.disconnect();
            observerWrapperResult = {metadata, delivered, drained, disconnected, resumed};
        })().catch(error => { observerWrapperResult = {error: String(error)}; });
    "#).unwrap();
    runtime.run_event_loop_bounded(100).await.unwrap();
    assert_eq!(runtime.evaluate("observerWrapperResult").unwrap(), serde_json::json!({
        "metadata":true, "delivered":true, "drained":true, "disconnected":true, "resumed":true,
    }));
}

#[cfg(feature = "render")]
fn frame_geometry_html(css: &str) -> String {
    format!("<html><head><style>#box{{position:absolute;{css}}}</style></head><body><div id=box>box</div></body></html>")
}

#[cfg(feature = "render")]
#[tokio::test(flavor = "current_thread")]
async fn loaded_frame_geometry_uses_receiver_document_and_viewport() {
    use crate::frame::FrameRealm;
    let mut runtime = browser_runtime();
    runtime.set_dom(obscura_dom::parse_html(&frame_geometry_html(
        "left:11px;top:13px;width:111px;height:21px;",
    )));
    let first = FrameRealm::new(&mut runtime, 1, 0, "https://example.com/one",
        &frame_geometry_html("left:37px;top:29px;width:50%;height:43px;")).unwrap();
    first.set_viewport(&mut runtime, 322.0, 180.0).unwrap();
    let second = FrameRealm::new(&mut runtime, 2, 0, "https://example.com/two",
        &frame_geometry_html("left:7px;top:9px;width:33px;height:25px;")).unwrap();
    second.set_viewport(&mut runtime, 200.0, 100.0).unwrap();
    runtime.execute_script("frame-receiver-probe", r#"
        globalThis.parentBox = document.getElementById('box');
        globalThis.firstBox = __obscura_frameObjects[1].document.getElementById('box');
        globalThis.secondBox = __obscura_frameObjects[2].document.getElementById('box');
        globalThis.rectTuple = node => {
            const r = Element.prototype.getBoundingClientRect.call(node);
            return [r.x, r.y, r.width, r.height];
        };
        globalThis.foreignStyle = getComputedStyle(firstBox);
    "#).unwrap();
    assert_eq!(runtime.evaluate(r#"[
        parentBox._nid === firstBox._nid && firstBox._nid === secondBox._nid,
        rectTuple(parentBox), rectTuple(firstBox), rectTuple(secondBox),
        firstBox.getClientRects().length, firstBox.clientWidth,
        parseFloat(foreignStyle.width), parseFloat(getComputedStyle(secondBox).width)
    ]"#).unwrap(), serde_json::json!([
        true, [11,13,111,21], [37,29,161,43], [7,9,33,25], 1, 161, 161, 33,
    ]));
    // Changing the frame viewport must invalidate an already prepared child
    // layout; its 50% width cannot retain either the old or parent viewport.
    first.set_viewport(&mut runtime, 400.0, 220.0).unwrap();
    assert_eq!(runtime.evaluate("[rectTuple(firstBox),parseFloat(foreignStyle.width),rectTuple(parentBox)]").unwrap(),
        serde_json::json!([[37,29,200,43],200,[11,13,111,21]]));
    assert_eq!(first.evaluate(&mut runtime, "[innerWidth,innerHeight]").unwrap(), serde_json::json!([400,220]));
    first.execute_script(&mut runtime, r#"
        document.getElementById('box').style.width = '66px';
        setTimeout(() => {
            const r = document.getElementById('box').getBoundingClientRect();
            globalThis.laterGeometry = [r.x, r.y, r.width, r.height];
        }, 0);
    "#).unwrap();
    runtime.run_event_loop_bounded(100).await.unwrap();
    assert_eq!(runtime.evaluate("[parseFloat(foreignStyle.width),rectTuple(firstBox),rectTuple(secondBox),rectTuple(parentBox)]").unwrap(),
        serde_json::json!([66,[37,29,66,43],[7,9,33,25],[11,13,111,21]]));
    assert_eq!(first.evaluate(&mut runtime, "laterGeometry").unwrap(), serde_json::json!([37,29,66,43]));

    // IO mixes arenas with the same local node id. RO's existing instanceof
    // validation is realm-local, so observe each node in its own realm.
    runtime.execute_script("frame-observer-probe", r#"
        globalThis.frameResizes = [];
        globalThis.frameIntersections = [];
        globalThis.frameResizeObserver = new ResizeObserver(entries => {
            for (const entry of entries) frameResizes.push([
                entry.target === parentBox ? 'parent' : entry.target === firstBox ? 'first' : 'second',
                entry.contentRect.width, entry.contentRect.height]);
        });
        globalThis.frameIntersectionObserver = new IntersectionObserver(entries => {
            for (const entry of entries) frameIntersections.push([
                entry.target === parentBox ? 'parent' : entry.target === firstBox ? 'first' : 'second',
                entry.boundingClientRect.width, entry.boundingClientRect.height]);
        });
        frameResizeObserver.observe(parentBox);
        for (const target of [parentBox,firstBox,secondBox]) {
            frameIntersectionObserver.observe(target);
        }
    "#).unwrap();
    let child_resize_probe = r#"
        globalThis.localResizes = [];
        globalThis.localResizeObserver = new ResizeObserver(entries => {
            for (const entry of entries) localResizes.push([entry.contentRect.width,entry.contentRect.height]);
        });
        localResizeObserver.observe(document.getElementById('box'));
    "#;
    first.execute_script(&mut runtime, child_resize_probe).unwrap();
    second.execute_script(&mut runtime, child_resize_probe).unwrap();
    runtime.run_event_loop_bounded(100).await.unwrap();
    assert_eq!(runtime.evaluate("[frameResizes.sort(),frameIntersections.sort()]").unwrap(), serde_json::json!([
        [["parent",111,21]],
        [["first",66,43],["parent",111,21],["second",33,25]],
    ]));
    assert_eq!(first.evaluate(&mut runtime, "localResizes").unwrap(), serde_json::json!([[66,43]]));
    assert_eq!(second.evaluate(&mut runtime, "localResizes").unwrap(), serde_json::json!([[33,25]]));
    first.execute_script(&mut runtime, "localResizeObserver.disconnect()").unwrap();
    second.execute_script(&mut runtime, "localResizeObserver.disconnect()").unwrap();
    runtime.evaluate("(frameResizeObserver.disconnect(),frameIntersectionObserver.disconnect())").unwrap();
}

#[cfg(feature = "render")]
#[test]
fn retired_loaded_frame_geometry_does_not_reuse_parent_or_replacement_cache() {
    use crate::frame::FrameRealm;
    let mut runtime = browser_runtime();
    runtime.set_dom(obscura_dom::parse_html(&frame_geometry_html(
        "left:11px;top:13px;width:111px;height:21px;",
    )));
    let frame = FrameRealm::new(&mut runtime, 1, 0, "https://example.com/old",
        &frame_geometry_html("left:37px;top:29px;width:81px;height:43px;")).unwrap();
    frame.set_viewport(&mut runtime, 320.0, 180.0).unwrap();
    runtime.execute_script("save-frame-node", r#"
        globalThis.oldBox = __obscura_frameObjects[1].document.getElementById('box');
        globalThis.oldStyle = getComputedStyle(oldBox);
        globalThis.oldWidth = oldBox.getBoundingClientRect().width;
        delete __obscura_frameObjects[1];
    "#).unwrap();
    assert_eq!(runtime.evaluate("[oldWidth,parseFloat(oldStyle.width)]").unwrap(), serde_json::json!([81,81]));
    drop(frame);
    assert_eq!(runtime.evaluate("[oldBox.getClientRects().length,oldBox.getBoundingClientRect().width,oldBox.clientWidth]").unwrap(),
        serde_json::json!([0,0,0]));
    let replacement = FrameRealm::new(&mut runtime, 1, 0, "https://example.com/new",
        &frame_geometry_html("left:5px;top:6px;width:55px;height:35px;")).unwrap();
    replacement.set_viewport(&mut runtime, 240.0, 120.0).unwrap();
    assert_eq!(runtime.evaluate(r#"(() => {
        const fresh = __obscura_frameObjects[1].document.getElementById('box');
        const parent = document.getElementById('box');
        return [oldBox._nid === fresh._nid && fresh._nid === parent._nid,
            oldBox.getClientRects().length,
            Element.prototype.getBoundingClientRect.call(oldBox).width,
            fresh.getBoundingClientRect().width, parent.getBoundingClientRect().width,
            parseFloat(getComputedStyle(fresh).width)];
    })()"#).unwrap(), serde_json::json!([true,0,0,55,111,55]));
}

#[test]
fn canvas_colors_clamp_without_changing_opaque_pixels_and_composite_alpha_once() {
    let mut rt = browser_runtime();
    let value = rt.evaluate(r#"(() => {
        const c = document.createElement('canvas'); c.width = 8; c.height = 8;
        const ctx = c.getContext('2d');
        const pixel = () => Array.from(ctx.getImageData(0, 0, 1, 1).data);
        const samples = [];
        for (const color of ['rgba(17,83,201,255)', 'rgba(-10,83,300,1)', 'rgba(17,83,201,-.5)', '#1358']) {
            ctx.clearRect(0,0,8,8); ctx.fillStyle=color; ctx.fillRect(0,0,1,1); samples.push(pixel());
        }
        ctx.fillStyle='#abcdef';
        for (const invalid of ['rgb(1,2,3)junk', '#ggg', 'rgb(1%,2,3%)']) ctx.fillStyle=invalid;
        const retained = ctx.fillStyle;
        ctx.clearRect(0,0,8,8); ctx.fillStyle='rgba(255,0,0,.5)'; ctx.fillRect(0,0,1,1);
        const transparent = pixel();
        ctx.fillStyle='rgba(0,0,255,.5)'; ctx.fillRect(0,0,1,1);
        const layered = pixel();
        ctx.clearRect(0,0,8,8); ctx.globalCompositeOperation='multiply';
        ctx.fillStyle='rgba(255,0,0,.5)'; ctx.fillRect(0,0,1,1);
        const multiplyTransparent = pixel();
        ctx.globalAlpha = .25; ctx.globalAlpha = NaN; ctx.globalAlpha = 2; ctx.globalAlpha = -1;
        const validAlpha = ctx.globalAlpha;
        ctx.save(); ctx.fillStyle='blue'; ctx.globalAlpha=1; ctx.restore();
        const restored = ctx.fillStyle === 'rgba(255, 0, 0, 0.5)' && ctx.globalAlpha === .25;
        c.width=8;
        const reset = ctx.fillStyle === '#000000' && ctx.globalAlpha === 1 && pixel().every(v=>v===0);
        ctx.fillStyle='rgba(255,0,0,.501)'; ctx.globalAlpha=.999;
        ctx.fillRect(0,0,1,1); const beforeSave=pixel();
        ctx.save(); ctx.fillStyle='blue'; ctx.globalAlpha=1; ctx.restore();
        ctx.clearRect(0,0,8,8); ctx.fillRect(0,0,1,1);
        const preciseRestore = beforeSave.every((value,index)=>value===pixel()[index]) && beforeSave[3]===128;
        ctx.globalAlpha=1; ctx.globalCompositeOperation='source-over';
        let paletteCorrect=true;
        for (let pass=0; pass<3; pass++) for (let i=0; i<32; i++) {
            const rgb=[i*7,255-i*5,i*3];
            ctx.fillStyle=`rgb(${rgb.join(',')})`; ctx.fillRect(0,0,1,1);
            paletteCorrect &&= pixel().every((value,index)=>value===[...rgb,255][index]);
        }
        ctx.fillStyle='rgb(128,64,200)'; ctx.fillRect(0,0,1,1);
        ctx.globalCompositeOperation='multiply';
        ctx.fillStyle='rgb(64,200,128)'; ctx.fillRect(0,0,1,1);
        const multiplyOpaque=pixel();
        return {samples, retained, transparent, layered, multiplyTransparent, validAlpha, restored, reset, preciseRestore, paletteCorrect, multiplyOpaque};
    })()"#).unwrap();
    assert_eq!(value, serde_json::json!({
        "samples":[[17,83,201,255],[0,83,255,255],[0,0,0,0],[17,51,85,136]],
        "retained":"#abcdef", "transparent":[255,0,0,128], "layered":[85,0,170,192],
        "multiplyTransparent":[255,0,0,128], "validAlpha":0.25, "restored":true, "reset":true, "preciseRestore":true,
        "paletteCorrect":true, "multiplyOpaque":[32,50,100,255],
    }));
}

#[test]
fn screen_getters_preserve_persona_values_and_use_private_brands() {
    let mut runtime = browser_runtime();
    let persona = runtime.persona();
    let spec = persona.to_spec();
    assert_eq!(runtime.evaluate("[screen.width,screen.height,screen.availWidth,screen.availHeight,screen.colorDepth,screen.pixelDepth]").unwrap(),
        serde_json::json!([persona.screen_width(), persona.screen_height(), spec.screen_avail_width, spec.screen_avail_height,
            spec.screen_color_depth, spec.screen_color_depth]));
    runtime.execute_script("screen-getter-metadata", WEBIDL_GETTER_PROBE).unwrap();
    assert_eq!(runtime.evaluate(r#"(() => {
        const proto = Screen.prototype, saved = Object.getPrototypeOf(screen);
        const keys = ['width','height','availWidth','availHeight','colorDepth','pixelDepth','availTop','availLeft','isExtended','orientation'];
        const fakes = [proto, {}, null, undefined, Object.create(proto), new Proxy(screen, {}), document, new EventTarget()];
        const rejects = fn => { try { fn(); return false; } catch(e) { return e instanceof TypeError; } };
        const values = keys.map(key => screen[key]);
        const metadata = keys.every(key => getterMetadataMatches(proto, key));
        const branded = keys.every(key => fakes.every(fake => rejects(() => Object.getOwnPropertyDescriptor(proto, key).get.call(fake))));
        Object.setPrototypeOf(screen, null);
        const stillBranded = keys.every((key,i) => Object.getOwnPropertyDescriptor(proto,key).get.call(screen) === values[i]);
        Object.setPrototypeOf(screen, saved);
        return {metadata, branded, stillBranded, illegal: rejects(() => new Screen()) && rejects(() => Screen()) &&
            rejects(() => new (class extends Screen {})()), ownKeys: Reflect.ownKeys(screen),
            eventTarget: Object.getPrototypeOf(proto) === EventTarget.prototype && screen instanceof EventTarget,
            privateHandoff: !Object.hasOwn(globalThis,'__obscura_core_handoff') && !Object.hasOwn(globalThis,'initializeScreenRegistry'),
            constructorLength: Screen.length, constants: [screen.availLeft, screen.availTop, screen.isExtended]};
    })()"#).unwrap(), serde_json::json!({"metadata":true,"branded":true,"stillBranded":true,"illegal":true,"ownKeys":[],
        "eventTarget":true,"privateHandoff":true,"constructorLength":0,"constants":[0,0,false]}));
}

#[test]
fn screen_onchange_is_a_branded_event_handler_with_stable_order() {
    let mut runtime = browser_runtime();
    assert_eq!(runtime.evaluate(r#"(() => {
        const d = Object.getOwnPropertyDescriptor(Screen.prototype, 'onchange'), calls = [];
        const reject = fn => { try { fn(); return false; } catch(e) { return e instanceof TypeError; } };
        const fake = [Screen.prototype, {}, Object.create(Screen.prototype), new Proxy(screen, {}), new EventTarget(), null];
        const branded = fake.every(value => reject(() => d.get.call(value)) && reject(() => d.set.call(value, () => {})));
        const setterMeta = d.enumerable && d.configurable && d.set.name === 'set onchange' && d.set.length === 1 &&
            !Object.hasOwn(d.set, 'prototype') && reject(() => Reflect.construct(function(){},[],d.set));
        screen.addEventListener('change', () => calls.push('first'));
        screen.onchange = () => calls.push('old');
        screen.addEventListener('change', () => calls.push('last'));
        const handler = function(event) { calls.push(this === screen && event.target === screen ? 'handler' : 'wrong'); return false; };
        screen.onchange = handler;
        const stable = screen.onchange === handler;
        const event = new Event('change', {cancelable:true});
        const result = screen.dispatchEvent(event);
        screen.onchange = null;
        screen.dispatchEvent(new Event('change'));
        screen.onchange = 123;
        return [branded, setterMeta, stable, result, event.defaultPrevented, event.isTrusted, calls.join(','),
            screen.onchange, Reflect.ownKeys(screen), reject(() => d.set.call(screen))];
    })()"#).unwrap(), serde_json::json!([true,true,true,false,true,false,"first,handler,last,first,last",null,[],true]));
}

#[tokio::test(flavor = "current_thread")]
async fn screen_override_updates_genuine_object_and_queues_change() {
    let mut runtime = browser_runtime();
    runtime.execute_script("screen-change", r#"
        globalThis.screenChanges = [];
        globalThis.originalScreen = screen;
        globalThis.originalScreenPrototype = Object.getPrototypeOf(screen);
        globalThis.screenWidthGetter = Object.getOwnPropertyDescriptor(Screen.prototype,'width').get;
        screen.onchange = function(event) { screenChanges.push([this === originalScreen,event.target === originalScreen,event.isTrusted,
            screenWidthGetter.call(originalScreen), Object.getOwnPropertyNames(originalScreen)]); };
        Object.setPrototypeOf(originalScreen, null);
        __obscura_set_screen_override(900, 700, true);
        globalThis.changeWasQueued = screenChanges.length === 0;
    "#).unwrap();
    runtime.run_event_loop_bounded(100).await.unwrap();
    assert_eq!(runtime.evaluate("[screen === originalScreen, changeWasQueued, screenChanges]").unwrap(),
        serde_json::json!([true,true,[[true,true,true,900,[]]]]));
    runtime.execute_script("screen-repeat", "__obscura_set_screen_override(900,700,true); Object.setPrototypeOf(screen,originalScreenPrototype);").unwrap();
    runtime.run_event_loop_bounded(100).await.unwrap();
    assert_eq!(runtime.evaluate("[screenChanges.length,screen.width,screen.height,screen.availWidth,screen.availHeight,Reflect.ownKeys(screen)]").unwrap(),
        serde_json::json!([1,900,700,900,700,[]]));
}

#[test]
fn screen_secure_members_follow_page_secure_context_exposure() {
    for (url, expected) in [("https://example.com/",true), ("file:///tmp/screen-test.html",true),
        ("http://127.0.0.1/screen",true), ("http://example.com/",false), ("about:blank",false)] {
        let mut runtime = ObscuraJsRuntime::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::MacChrome153));
        runtime.set_dom(obscura_dom::parse_html("<html><body></body></html>"));
        runtime.set_url(url); runtime.run_page_init();
        assert_eq!(runtime.evaluate("[isSecureContext,Object.hasOwn(Screen.prototype,'isExtended'),Object.hasOwn(Screen.prototype,'onchange'),typeof screen.width,screen instanceof EventTarget]").unwrap(),
            serde_json::json!([expected,expected,expected,"number",true]), "{url}");
    }
}

#[test]
fn screen_borrowed_accessors_and_handlers_accept_real_loaded_frame_objects() {
    let mut runtime = browser_runtime();
    let frame = crate::frame::FrameRealm::new(&mut runtime, 61, 0, "https://example.com/frame", "<html><body>real frame</body></html>").unwrap();
    frame.execute_script(&mut runtime, "__obscura_set_screen_override(710,510,true);").unwrap();
    assert_eq!(runtime.evaluate(r#"(() => {
        const child = __obscura_frameObjects[61].window;
        const own = Object.getOwnPropertyDescriptor(Screen.prototype,'width').get;
        const other = Object.getOwnPropertyDescriptor(child.Screen.prototype,'width').get;
        const handler = Object.getOwnPropertyDescriptor(Screen.prototype,'onchange');
        const calls = [];
        const fn = function(event) { calls.push(this === child.screen && event.target === child.screen); };
        handler.set.call(child.screen, fn);
        const getterAgrees = handler.get.call(child.screen) === fn;
        child.screen.dispatchEvent(new child.Event('change'));
        handler.set.call(child.screen, null);
        child.screen.dispatchEvent(new child.Event('change'));
        let fake = false;
        try { other.call(new Proxy(screen,{})); } catch(e) { fake = e instanceof child.TypeError; }
        const previous = Object.getPrototypeOf(child.screen);
        Object.setPrototypeOf(child.screen, null);
        const changedPrototype = own.call(child.screen) === 710;
        Object.setPrototypeOf(child.screen, previous);
        return [child.Screen !== Screen, own.call(child.screen), other.call(screen) === screen.width, getterAgrees,
            calls, fake, changedPrototype, Reflect.ownKeys(child.screen),
            !Object.hasOwn(child,'__obscura_core_handoff'), !Object.hasOwn(child,'initializeScreenRegistry')];
    })()"#).unwrap(), serde_json::json!([true,710,true,true,[true],true,true,[],true,true]));
    assert_eq!(frame.evaluate(&mut runtime,"[screen.width,screen.height,document.body.textContent]").unwrap(), serde_json::json!([710,510,"real frame"]));
}

#[tokio::test(flavor = "current_thread")]
async fn screen_remains_window_only_after_worker_initialization() {
    let mut runtime = browser_runtime();
    runtime.execute_script("screen-worker", r#"
        globalThis.screenWorkerResult = null;
        const source = 'postMessage([typeof Screen, typeof screen, typeof ScreenOrientation, Object.hasOwn(globalThis,"__obscura_core_handoff"), Object.hasOwn(globalThis,"initializeScreenRegistry")])';
        const url = URL.createObjectURL(new Blob([source],{type:'application/javascript'}));
        const worker = new Worker(url);
        worker.onmessage = event => { screenWorkerResult = event.data; worker.terminate(); URL.revokeObjectURL(url); };
    "#).unwrap();
    runtime.run_event_loop_bounded(1000).await.unwrap();
    assert_eq!(runtime.evaluate("screenWorkerResult").unwrap(),serde_json::json!(["undefined","undefined","undefined",false,false]));
}


#[tokio::test(flavor = "current_thread")]
async fn screen_http_change_event_does_not_expose_secure_members() {
    let mut runtime = ObscuraJsRuntime::new(obscura_net::EffectivePersona::builtin(
        obscura_net::StealthProfile::MacChrome153,
    ));
    runtime.set_dom(obscura_dom::parse_html("<html><body></body></html>"));
    runtime.set_url("http://example.com/screen");
    runtime.run_page_init();
    runtime.execute_script("screen-http-change", r#"
        globalThis.httpScreenChanges = [];
        screen.addEventListener('change', function(event) {
            httpScreenChanges.push([this === screen, event.target === screen, event.isTrusted, screen.width]);
        });
        __obscura_set_screen_override(910,710,true);
        globalThis.httpChangeQueued = httpScreenChanges.length === 0;
    "#).unwrap();
    runtime.run_event_loop_bounded(100).await.unwrap();
    assert_eq!(runtime.evaluate(r#"[
        isSecureContext, Object.hasOwn(Screen.prototype,'onchange'), Object.hasOwn(Screen.prototype,'isExtended'),
        httpChangeQueued, httpScreenChanges, screen.width, screen.height
    ]"#).unwrap(), serde_json::json!([false,false,false,true,[[true,true,true,910]],910,710]));
}

#[tokio::test(flavor = "current_thread")]
async fn screen_override_before_page_init_remains_silent() {
    let mut runtime = ObscuraJsRuntime::new(obscura_net::EffectivePersona::builtin(
        obscura_net::StealthProfile::MacChrome153,
    ));
    runtime.set_dom(obscura_dom::parse_html("<html><body></body></html>"));
    runtime.set_url("https://example.com/screen");
    runtime.execute_script("screen-before-init", r#"
        globalThis.initialScreenChanges = [];
        screen.addEventListener('change', event => initialScreenChanges.push(event.isTrusted));
        __obscura_set_screen_override(920,720,true);
    "#).unwrap();
    runtime.run_event_loop_bounded(100).await.unwrap();
    assert_eq!(runtime.evaluate("initialScreenChanges").unwrap(),serde_json::json!([]));
    runtime.run_page_init();
    runtime.run_event_loop_bounded(100).await.unwrap();
    assert_eq!(runtime.evaluate("initialScreenChanges").unwrap(),serde_json::json!([]));
    runtime.execute_script("screen-after-init", "__obscura_set_screen_override(921,721,true);").unwrap();
    runtime.run_event_loop_bounded(100).await.unwrap();
    assert_eq!(runtime.evaluate("initialScreenChanges").unwrap(),serde_json::json!([true]));
}

// Private native regression draft. Append within browser_compat_tests.rs module.
// UNCOMPILED/UNRUN. No network or renderer is required.
#[tokio::test(flavor = "current_thread")]
async fn screen_retired_frame_override_cannot_borrow_replacement_task_owner() {
    use crate::frame::FrameRealm;
    let mut runtime = browser_runtime();
    let old = FrameRealm::new(&mut runtime, 67, 0, "https://example.com/old-screen",
        "<html><body>old</body></html>").unwrap();
    old.execute_script(&mut runtime, r#"
        globalThis.retirementChanges = [];
        screen.addEventListener('change', function(event) {
            retirementChanges.push([this === screen, event.target === screen, event.isTrusted, screen.width]);
        });
        __obscura_set_screen_override(701,501,true);
    "#).unwrap();
    runtime.execute_script("retain-old-screen-owner", r#"
        const child = __obscura_frameObjects[67].window;
        globalThis.retiredScreenObject = child.screen;
        globalThis.retiredScreenOverride = child.__obscura_set_screen_override;
        globalThis.retiredScreenChanges = child.retirementChanges;
        delete __obscura_frameObjects[67];
    "#).unwrap();
    // There is already a pending task with a weak reference to the OLD state.
    // Retirement must cancel it even though the same numeric ID is now reused.
    drop(old);
    let fresh = FrameRealm::new(&mut runtime, 67, 0, "https://example.com/new-screen",
        "<html><body>new</body></html>").unwrap();
    fresh.execute_script(&mut runtime, r#"
        globalThis.successorChanges = [];
        screen.addEventListener('change', function(event) {
            successorChanges.push([this === screen, event.target === screen, event.isTrusted, screen.width]);
        });
    "#).unwrap();
    runtime.run_event_loop_bounded(100).await.unwrap();
    assert_eq!(runtime.evaluate("retiredScreenChanges").unwrap(), serde_json::json!([]),
        "a change queued before retirement must be cancelled");
    // Crucial second stage: the old closure enqueues AFTER its frame retired.
    // Its old numeric ID must not resolve to the successor's active state.
    runtime.execute_script("invoke-retained-old-screen-override",
        "retiredScreenOverride(702,502,true);").unwrap();
    fresh.execute_script(&mut runtime, "__obscura_set_screen_override(703,503,true);").unwrap();
    runtime.run_event_loop_bounded(100).await.unwrap();
    assert_eq!(fresh.evaluate(&mut runtime, "successorChanges").unwrap(),
        serde_json::json!([[true,true,true,703]]), "a live successor must still deliver a real change");
    assert_eq!(runtime.evaluate("retiredScreenChanges").unwrap(), serde_json::json!([]),
        "an old closure must not schedule trusted events using a replacement's task owner");
    assert_eq!(runtime.evaluate("[retiredScreenObject === __obscura_frameObjects[67].window.screen, __obscura_frameObjects[67].window.screen.width]").unwrap(),
        serde_json::json!([false,703]), "screen state itself must remain separately branded and owned");
}

#[test]
fn screen_brands_survive_public_intrinsic_replacement() {
    let mut runtime = browser_runtime();
    let frame = crate::frame::FrameRealm::new(&mut runtime, 68, 0, "https://example.com/screen-intrinsics",
        "<html><body>real child</body></html>").unwrap();
    assert_eq!(runtime.evaluate(r#"(() => {
        const child = __obscura_frameObjects[68].window;
        const own = Object.getOwnPropertyDescriptor(Screen.prototype,'width').get;
        const other = Object.getOwnPropertyDescriptor(child.Screen.prototype,'width').get;
        const width = screen.width, childWidth = child.screen.width;
        const get = WeakMap.prototype.get, bind = Function.prototype.bind;
        let publicCalls = 0;
        try {
            WeakMap.prototype.get = function() { publicCalls++; throw new Error('public weakmap get'); };
            Function.prototype.bind = function() { publicCalls++; throw new Error('public bind'); };
            let proxyRejected = false;
            try { own.call(new Proxy(screen,{})); } catch(error) { proxyRejected = error instanceof TypeError; }
            return [own.call(screen) === width, own.call(child.screen) === childWidth,
                other.call(screen) === width, other.call(child.screen) === childWidth, proxyRejected, publicCalls];
        } finally { WeakMap.prototype.get = get; Function.prototype.bind = bind; }
    })()"#).unwrap(), serde_json::json!([true,true,true,true,true,0]));
    assert_eq!(frame.evaluate(&mut runtime, "document.body.textContent").unwrap(),serde_json::json!("real child"));
}
