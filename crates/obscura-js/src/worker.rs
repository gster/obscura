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
use crate::worker_queue::{self as queue, Size};

/// Registry of active worker instances.
#[derive(Default)]
pub struct WorkerRegistry {
    pub(crate) resources: std::sync::Arc<queue::Resources>,
    policy: std::sync::Arc<std::sync::Mutex<WorkerPolicy>>,
    pub next_id: u32,
    pub workers: HashMap<u32, WorkerInstance>,
}

#[derive(Default)]
struct WorkerPolicy {
    blocked_urls: Vec<String>,
    intercept_tx: Option<tokio::sync::mpsc::UnboundedSender<crate::ops::InterceptedRequest>>,
    intercept_enabled: bool,
    console_enabled: bool,
    runtime_events_enabled: bool,
}

pub(crate) fn sync_policy(state: &OpState) {
    let page = state.borrow::<Rc<RefCell<crate::ops::ObscuraState>>>().borrow();
    let registry = state.borrow::<Rc<RefCell<WorkerRegistry>>>().borrow();
    let mut policy = registry.policy.lock().unwrap();
    policy.blocked_urls = page.blocked_urls.clone();
    policy.intercept_tx = page.intercept_tx.clone();
    policy.intercept_enabled = page.intercept_enabled;
    policy.console_enabled = page.console_messages_enabled;
    policy.runtime_events_enabled = page.runtime_events_enabled;
}

pub(crate) fn refresh_policy(state: &OpState) {
    if state.try_borrow::<WorkerEndpoint>().is_none() { return; }
    let registry = state.borrow::<Rc<RefCell<WorkerRegistry>>>().borrow();
    let policy = registry.policy.lock().unwrap();
    let mut page = state.borrow::<Rc<RefCell<crate::ops::ObscuraState>>>().borrow_mut();
    page.blocked_urls = policy.blocked_urls.clone();
    page.intercept_tx = policy.intercept_tx.clone();
    page.intercept_enabled = policy.intercept_enabled;
    page.console_messages_enabled = policy.console_enabled;
    page.runtime_events_enabled = policy.runtime_events_enabled;
}

pub struct WorkerInstance {
    commands: queue::Sender<WorkerCommand>,
    events: std::sync::Arc<tokio::sync::Mutex<queue::Receiver<WorkerEvent>>>,
    control: std::sync::Arc<WorkerControl>,
}

#[derive(Default)]
struct WorkerControl {
    terminated: std::sync::atomic::AtomicBool,
    isolate: std::sync::Mutex<Option<v8::IsolateHandle>>,
}
impl WorkerControl {
    fn terminate(&self) {
        self.terminated.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(handle) = self.isolate.lock().unwrap().as_ref() {
            handle.terminate_execution();
        }
    }
    fn stopped(&self) -> bool {
        self.terminated.load(std::sync::atomic::Ordering::SeqCst)
    }
}
impl Drop for WorkerInstance {
    fn drop(&mut self) {
        self.control.terminate();
        let _ = self.commands.send(WorkerCommand::Stop);
    }
}
enum WorkerCommand { Run(String), Message(String), Stop }
impl Size for WorkerCommand {
    fn queued_bytes(&self) -> usize {
        match self { Self::Run(source) | Self::Message(source) => source.len(), Self::Stop => 0 }
    }
}

pub(crate) struct WorkerEndpoint {
    control: std::sync::Arc<WorkerControl>,
    events: queue::Sender<WorkerEvent>,
    closing: std::cell::Cell<bool>,
    blobs: HashMap<String, String>,
}
impl WorkerEndpoint {
    fn send(&self, event: WorkerEvent) {
        if let Err(error) = self.events.send(event) {
            if !self.closing.replace(true) {
                self.events.terminal(WorkerEvent::Script(serde_json::json!({"kind":"error","data":error}).to_string()));
            }
            self.control.terminate();
        }
    }
    fn emit(&self, kind: &str, data: &str) {
        self.send(WorkerEvent::Script(serde_json::json!({"kind": kind, "data": data}).to_string()));
    }
}

enum WorkerEvent {
    Script(String),
    Observations(WorkerObservations),
}

impl Size for WorkerEvent {
    fn queued_bytes(&self) -> usize {
        match self {
            Self::Script(value) => value.len(),
            Self::Observations(value) => {
                let network: usize = value.network.iter().map(|(event, body)| {
                    std::mem::size_of_val(event) + event.request_id.len() + event.url.len() + event.method.len()
                        + event.response_headers.iter().map(|(k,v)| k.len() + v.len()).sum::<usize>()
                        + body.as_ref().map_or(0, |body| body.body.len())
                }).sum();
                let runtime: usize = value.runtime.iter().map(|event| match event {
                    crate::ops::RuntimeEvent::Console(event) => event.kind.len() + event.args.iter().map(|arg| arg.to_string().len()).sum::<usize>(),
                    crate::ops::RuntimeEvent::Exception(event) => event.name.len() + event.description.len() + event.url.len()
                        + event.stack_trace.iter().map(|frame| frame.to_string().len()).sum::<usize>(),
                }).sum();
                network + runtime + value.urls.iter().chain(value.console.iter()).map(String::len).sum::<usize>()
            }
        }
    }
}

#[derive(Default)]
struct WorkerObservations {
    network: Vec<(crate::ops::JsNetworkEvent, Option<crate::ops::StoredNetworkResponseBody>)>,
    urls: Vec<String>,
    console: Vec<String>,
    runtime: Vec<crate::ops::RuntimeEvent>,
}

pub(crate) fn flush_observations(state: &OpState) {
    let Some(endpoint) = state.try_borrow::<WorkerEndpoint>() else { return; };
    let mut worker = state.borrow::<Rc<RefCell<crate::ops::ObscuraState>>>().borrow_mut();
    let events = std::mem::take(&mut worker.js_network_events);
    let mut observations = WorkerObservations::default();
    for event in events {
        let body = worker.network_response_bodies.remove(&event.request_id);
        observations.network.push((event, body));
    }
    worker.network_response_body_order.clear();
    observations.urls = std::mem::take(&mut worker.fetched_urls);
    observations.console = worker.pending_console_messages.drain(..).collect();
    observations.runtime = worker.pending_runtime_events.drain(..).collect();
    // Object handles belong to the worker isolate. Preserve inline values and
    // previews, never pretend they can be dereferenced in the owner's isolate.
    for event in &mut observations.runtime {
        if let crate::ops::RuntimeEvent::Console(event) = event {
            for argument in &mut event.args {
                if let Some(argument) = argument.as_object_mut() { argument.remove("objectId"); }
            }
        }
    }
    if !observations.network.is_empty() || !observations.urls.is_empty() ||
        !observations.console.is_empty() || !observations.runtime.is_empty() {
        endpoint.send(WorkerEvent::Observations(observations));
    }
}

impl WorkerObservations {
    fn deliver(self, parent: &mut crate::ops::ObscuraState) {
        for (event, body) in self.network {
            if let Some(body) = body {
                parent.network_response_body_order.push_back(event.request_id.clone());
                parent.network_response_bodies.insert(event.request_id.clone(), body);
            }
            parent.js_network_events.push(event);
        }
        while parent.network_response_body_order.len() > crate::ops::response_body_entry_limit() {
            if let Some(id) = parent.network_response_body_order.pop_front() {
                parent.network_response_bodies.remove(&id);
            }
        }
        let excess = parent.js_network_events.len().saturating_sub(4096);
        parent.js_network_events.drain(..excess);
        parent.fetched_urls.extend(self.urls);
        let excess = parent.fetched_urls.len().saturating_sub(16384);
        parent.fetched_urls.drain(..excess);
        parent.pending_console_messages.extend(self.console);
        parent.pending_runtime_events.extend(self.runtime);
        while parent.pending_console_messages.len() > 1024 { parent.pending_console_messages.pop_front(); }
        while parent.pending_runtime_events.len() > 1024 { parent.pending_runtime_events.pop_front(); }
    }
}

struct WorkerConfig {
    policy: std::sync::Arc<std::sync::Mutex<WorkerPolicy>>,
    resources: std::sync::Arc<queue::Resources>,
    url: String,
    globals: serde_json::Map<String, serde_json::Value>,
    blobs: HashMap<String, String>,
    identity: Option<crate::ops::DeviceIdentity>,
    cookies: Option<std::sync::Arc<obscura_net::CookieJar>>,
    http: Option<std::sync::Arc<obscura_net::ObscuraHttpClient>>,
    callbacks: Option<std::sync::Arc<obscura_net::CallbackRegistry>>,
    #[cfg(feature = "stealth")]
    stealth: Option<std::sync::Arc<obscura_net::StealthHttpClient>>,
    blocked_urls: Vec<String>,
    referrer_policy: obscura_net::ReferrerPolicy,
    intercept_tx: Option<tokio::sync::mpsc::UnboundedSender<crate::ops::InterceptedRequest>>,
    intercept_enabled: bool,
    intercept_counter: std::sync::Arc<std::sync::atomic::AtomicU64>,
    response_counter: std::sync::Arc<std::sync::atomic::AtomicU64>,
    in_flight: std::sync::Arc<std::sync::atomic::AtomicU32>,
    console_enabled: bool,
    runtime_events_enabled: bool,
}

fn exception_text(
    scope: &mut v8::TryCatch<'_, v8::HandleScope<'_>>,
) -> String {
    match scope.exception() {
        Some(exception) => exception.to_rust_string_lossy(scope),
        None => "unknown error".to_string(),
    }
}

pub(crate) fn extract_exception_message(
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

pub(crate) const WORKER_BOOTSTRAP_JS: &str = r#"
(function(workerId, workerUrl, ops) {
    const _origNavigator = globalThis.navigator;
    const _windowOnlyProps = [
        "window","document","location","history","localStorage","sessionStorage",
        "navigator","constructor","self","origin","isSecureContext","crossOriginIsolated",
        "onlanguagechange","onrejectionhandled","onunhandledrejection",
        "HTMLDocument","Document","Element","HTMLElement","Node","alert","confirm",
        "prompt","parent","top","frames","Window","Screen","screen","ScreenOrientation","screenLeft",
        "screenTop","screenX","screenY","innerHeight","innerWidth","outerHeight",
        "outerWidth","pageXOffset","pageYOffset","scrollX","scrollY","scroll",
        "scrollBy","scrollTo","open","opener","print","stop","focus","blur",
        "frameElement","getComputedStyle","matchMedia","getSelection","visualViewport","VisualViewport",
        "Audio","Image","onbeforeunload","onunload","onresize","onscroll",
        "onpopstate","onhashchange","addEventListener","removeEventListener",
        "dispatchEvent","EventTarget","onmessage","onerror","onmessageerror",
        "HTMLAnchorElement","HTMLAreaElement","HTMLAudioElement","HTMLBRElement","HTMLBodyElement",
        "HTMLButtonElement","HTMLCanvasElement","HTMLCollection","HTMLAllCollection","HTMLDataListElement","HTMLDetailsElement",
        "HTMLDialogElement","HTMLDivElement","HTMLFieldSetElement","HTMLFormElement","HTMLHRElement",
        "HTMLHeadElement","HTMLHeadingElement","HTMLHtmlElement","HTMLIFrameElement","HTMLImageElement",
        "HTMLInputElement","HTMLLIElement","HTMLLabelElement","HTMLLegendElement","HTMLLinkElement",
        "HTMLMediaElement","HTMLMetaElement","HTMLOListElement","HTMLOptionElement","HTMLParagraphElement",
        "HTMLPreElement","HTMLProgressElement","HTMLScriptElement","HTMLSelectElement","HTMLSlotElement",
        "HTMLSpanElement","HTMLStyleElement","HTMLTableElement","HTMLTemplateElement","HTMLTextAreaElement",
        "HTMLTrackElement","HTMLUListElement","HTMLUnknownElement","HTMLVideoElement",
        "HTMLSourceElement","HTMLObjectElement","HTMLEmbedElement","HTMLParamElement","HTMLOutputElement","HTMLFrameSetElement",
        "SVGCircleElement","SVGClipPathElement","SVGComponentTransferFunctionElement","SVGDefsElement",
        "SVGElement","SVGEllipseElement","SVGFEBlendElement","SVGFECompositeElement","SVGFEDisplacementMapElement",
        "SVGFEMorphologyElement","SVGFETurbulenceElement","SVGFilterElement","SVGGElement","SVGGeometryElement",
        "SVGGradientElement","SVGGraphicsElement","SVGImageElement","SVGLength","SVGLineElement",
        "SVGLinearGradientElement","SVGMaskElement","SVGPathElement","SVGPoint","SVGPolygonElement",
        "SVGPolylineElement","SVGPreserveAspectRatio","SVGRadialGradientElement","SVGRect","SVGRectElement",
        "SVGSVGElement","SVGScriptElement","SVGStopElement","SVGTextContentElement","SVGTextElement",
        "SVGTransform","SVGUseElement",
        "SVGTextPathElement","SVGPatternElement","SVGMPathElement","SVGFEImageElement","SVGAnimationElement",
        "MutationObserver","IntersectionObserver","IntersectionObserverEntry","ResizeObserver","ResizeObserverEntry","ResizeObserverSize",
        "NodeFilter","TreeWalker","NodeList","NamedNodeMap","DocumentFragment","CharacterData","Comment","CDATASection","ProcessingInstruction","Text",
        "XMLDocument","XMLSerializer","XPathResult","DOMParser","Range","Selection","StaticRange",
        "CSSRule","CSSRuleList","CSSStyleDeclaration","CSSStyleRule","CSSStyleSheet","StyleSheetList",
        "Animation","AnimationEvent","Attr","AudioBuffer","AudioContext","CSS",
        "CanvasRenderingContext2D","ClipboardEvent","CompositionEvent","ContentIndex",
        "CustomElementRegistry","DOMRectList","DOMStringMap","DOMTokenList","DataTransfer",
        "DataTransferItem","DataTransferItemList","DeviceOrientationEvent","DocumentTimeline",
        "DocumentType","ElementInternals","FocusEvent","FormDataEvent","HashChangeEvent",
        "History","InputEvent","KeyboardEvent","KeyframeEffect","MediaQueryList","MediaStream",
        "MediaStreamTrack","MimeType","MimeTypeArray","MouseEvent","Navigator",
        "OfflineAudioContext","PageTransitionEvent","Plugin","PluginArray","PointerEvent",
        "PopStateEvent","RTCIceCandidate","RTCPeerConnection","RTCSessionDescription",
        "ServiceWorkerContainer","ShadowRoot","SharedArrayBuffer","SharedWorker",
        "SpeechRecognition","SpeechSynthesisUtterance","Storage","StorageEvent",
        "SubmitEvent","TextTrack","TextTrackCue","TextTrackCueList","TextTrackList",
        "ToggleEvent","TransitionEvent","UIEvent","VTTCue","ValidityState","WheelEvent",
        "PictureInPictureWindow","RemotePlayback","MediaDevices","Geolocation",
        "PaymentRequest","PresentationAvailability","PresentationConnection","PresentationConnectionList","PresentationRequest",
        "CookieStore","cookieStore",
        "cancelIdleCallback","chrome","clientInformation","customElements",
        "devicePixelRatio","length","navigation","requestFileSystem","requestIdleCallback",
        "speechSynthesis","webkitAudioContext","webkitSpeechRecognition",
        "onabort","onbeforeprint","onblur","oncancel","oncanplay","oncanplaythrough",
        "onchange","onclick","onclose","oncontextmenu","oncuechange","ondblclick",
        "ondrag","ondragend","ondragenter","ondragleave","ondragover","ondragstart",
        "ondrop","ondurationchange","onemptied","onended","onfocus","onfocusin",
        "onfocusout","onformdata","ongotpointercapture","oninput","oninvalid",
        "onkeydown","onkeypress","onkeyup","onload","onloadeddata","onloadedmetadata",
        "onloadstart","onlostpointercapture","onmousedown","onmouseenter","onmouseleave",
        "onmousemove","onmouseout","onmouseover","onmouseup","onoffline","ononline",
        "onpagehide","onpageshow","onpaste","onpause","onplay","onplaying",
        "onpointercancel","onpointerdown","onpointerenter","onpointerleave",
        "onpointermove","onpointerout","onpointerover","onpointerup","onprogress",
        "onratechange","onreset","onseeked","onseeking","onselect","onstalled",
        "onstorage","onsubmit","onsuspend","ontimeupdate","ontoggle","onvolumechange",
        "onwaiting","onwheel",
        "onanimationiteration","onanimationend","RTCDtlsTransport","MediaRecorder",
        "onwebkittransitionend","BatteryManager","ScriptProcessorNode","onwebkitanimationstart",
        "Sensor","onanimationstart","ondevicemotion","onwebkitanimationiteration",
        "ontransitionend","onmousewheel","onappinstalled","AudioScheduledSourceNode",
        "ondeviceorientationabsolute","onwebkitanimationend","onselectionchange","MIDIInput",
        "AudioWorkletNode","onafterprint","onselectstart","onsearch","MIDIAccess",
        "ondeviceorientation","ServiceWorker","RTCDTMFSender","onbeforeinstallprompt",
        "BaseAudioContext","MIDIPort","RTCIceTransport","MediaKeySession","onauxclick",
        "ApplicationCache"
    ];
    for (const name of _windowOnlyProps) {
        delete globalThis[name];
    }
    for (let i = 0; i < 50; i++) {
        delete globalThis[i];
    }
    try { delete globalThis.Deno; } catch(e) {}

    let closing = false;
    function _wrapTimer(fn) {
        return (callback, delay, ...args) => fn(() => {
            if (!closing) {
                if (typeof callback === 'function') callback(...args);
                else (0, eval)(String(callback));
            }
        }, delay);
    }
    const _wrappedSetTimeout = _wrapTimer(globalThis.setTimeout);
    const _wrappedSetInterval = _wrapTimer(globalThis.setInterval);
    const _wrappedClearTimeout = globalThis.clearTimeout;
    const _wrappedClearInterval = globalThis.clearInterval;

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
    Object.defineProperty(DedicatedWorkerGlobalScope.prototype, 'TEMPORARY', {
        value: 0, writable: false, enumerable: true, configurable: false,
    });
    Object.defineProperty(DedicatedWorkerGlobalScope.prototype, 'PERSISTENT', {
        value: 1, writable: false, enumerable: true, configurable: false,
    });
    globalThis.DedicatedWorkerGlobalScope = DedicatedWorkerGlobalScope;

    Object.setPrototypeOf(globalThis, DedicatedWorkerGlobalScope.prototype);
    Object.defineProperty(globalThis, Symbol.toStringTag, {
        value: 'DedicatedWorkerGlobalScope', configurable: true,
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
        'appCodeName', 'appName', 'appVersion', 'platform', 'product', 'userAgent',
        'language', 'languages', 'onLine', 'hardwareConcurrency', 'deviceMemory',
        'userAgentData', 'locks', 'storage', 'mediaCapabilities', 'permissions',
        'gpu', 'connection',
    ];
    for (const prop of navProps) {
        if (_origNavigator && prop in _origNavigator) {
            const val = _origNavigator[prop];
            Object.defineProperty(WorkerNavigator.prototype, prop, {
                configurable: true, enumerable: true, get() { return val; }
            });
        } else if (prop === 'appCodeName') {
            Object.defineProperty(WorkerNavigator.prototype, prop, {
                configurable: true, enumerable: true, get() { return 'Mozilla'; }
            });
        } else if (prop === 'appName') {
            Object.defineProperty(WorkerNavigator.prototype, prop, {
                configurable: true, enumerable: true, get() { return 'Netscape'; }
            });
        } else if (prop === 'appVersion') {
            Object.defineProperty(WorkerNavigator.prototype, prop, {
                configurable: true, enumerable: true, get() {
                    return _origNavigator && _origNavigator.userAgent
                        ? _origNavigator.userAgent.replace(/^Mozilla\//, '')
                        : '5.0';
                }
            });
        } else if (prop === 'product') {
            Object.defineProperty(WorkerNavigator.prototype, prop, {
                configurable: true, enumerable: true, get() { return 'Gecko'; }
            });
        }
    }
    globalThis.WorkerNavigator = WorkerNavigator;

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

    function _isTrustworthy(str) {
        try {
            const u = new URL(str);
            if (u.protocol === 'https:' || u.protocol === 'wss:' || u.protocol === 'file:') return true;
            const h = u.hostname.toLowerCase();
            if (h === 'localhost' || h.endsWith('.localhost') || h === '127.0.0.1' || h === '::1' || h === '[::1]') return true;
            if (/^127(?:\.(?:25[0-5]|2[0-4]\d|1\d\d|[1-9]\d|\d)){3}$/.test(h)) return true;
            if (u.protocol === 'blob:') return _isTrustworthy(u.origin);
            return false;
        } catch (e) {
            return false;
        }
    }

    const _workerOrigin = (parsedUrl.protocol === 'blob:' || parsedUrl.protocol === 'data:' || !workerLoc.origin || workerLoc.origin === 'null')
        ? (globalThis.__obscura_creator_origin || workerLoc.origin || 'null')
        : workerLoc.origin;

    const _workerIsSecure = typeof globalThis.__obscura_is_secure_context === 'boolean'
        ? globalThis.__obscura_is_secure_context
        : _isTrustworthy(workerUrl);

    const _workerIsIsolated = Boolean(globalThis.__obscura_cross_origin_isolated);

    function _getWorkerOrigin() { return _workerOrigin; }
    function _setWorkerOrigin(val) {
        Object.defineProperty(this, 'origin', { value: val, writable: true, enumerable: true, configurable: true });
    }
    function _getWorkerIsSecure() { return _workerIsSecure; }
    function _getWorkerIsIsolated() { return _workerIsIsolated; }

    Object.defineProperty(WorkerGlobalScope.prototype, 'self', {
        get() { return globalThis; }, configurable: true, enumerable: true,
    });
    Object.defineProperty(WorkerGlobalScope.prototype, 'location', {
        get() { return workerLoc; }, configurable: true, enumerable: true,
    });
    Object.defineProperty(WorkerGlobalScope.prototype, 'navigator', {
        get() { return workerNav; }, configurable: true, enumerable: true,
    });
    Object.defineProperty(WorkerGlobalScope.prototype, 'origin', {
        get: _getWorkerOrigin, set: _setWorkerOrigin, configurable: true, enumerable: true,
    });
    Object.defineProperty(WorkerGlobalScope.prototype, 'isSecureContext', {
        get: _getWorkerIsSecure, configurable: true, enumerable: true,
    });
    Object.defineProperty(WorkerGlobalScope.prototype, 'crossOriginIsolated', {
        get: _getWorkerIsIsolated, configurable: true, enumerable: true,
    });

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
    Object.defineProperty(WorkerGlobalScope.prototype, 'onerror', onerrorDescriptor);

    let _workerOnLanguageChange = null;
    Object.defineProperty(WorkerGlobalScope.prototype, 'onlanguagechange', {
        configurable: true,
        enumerable: true,
        get() { return _workerOnLanguageChange; },
        set(val) {
            _workerOnLanguageChange = typeof val === 'function' || (val && typeof val.handleEvent === 'function') ? val : null;
        },
    });

    let _workerOnRejectionHandled = null;
    Object.defineProperty(WorkerGlobalScope.prototype, 'onrejectionhandled', {
        configurable: true,
        enumerable: true,
        get() { return _workerOnRejectionHandled; },
        set(val) {
            _workerOnRejectionHandled = typeof val === 'function' || (val && typeof val.handleEvent === 'function') ? val : null;
        },
    });

    let _workerOnUnhandledRejection = null;
    Object.defineProperty(WorkerGlobalScope.prototype, 'onunhandledrejection', {
        configurable: true,
        enumerable: true,
        get() { return _workerOnUnhandledRejection; },
        set(val) {
            _workerOnUnhandledRejection = typeof val === 'function' || (val && typeof val.handleEvent === 'function') ? val : null;
        },
    });

    for (const p of ['performance', 'crypto', 'indexedDB', 'caches', 'scheduler']) {
        if (p in globalThis) {
            const val = globalThis[p];
            Object.defineProperty(WorkerGlobalScope.prototype, p, {
                configurable: true, enumerable: true,
                get() { return val; },
                set(v) { Object.defineProperty(this, p, { value: v, writable: true, configurable: true, enumerable: true }); }
            });
            delete globalThis[p];
        }
    }

    let _workerFonts = null;
    Object.defineProperty(WorkerGlobalScope.prototype, 'fonts', {
        configurable: true,
        enumerable: true,
        get() {
            if (!_workerFonts && typeof globalThis.FontFaceSet === 'function') {
                _workerFonts = new globalThis.FontFaceSet();
            }
            return _workerFonts;
        },
    });
    Object.defineProperty(WorkerGlobalScope.prototype, 'trustedTypes', {
        configurable: true,
        enumerable: true,
        get() { return undefined; },
    });

    WorkerGlobalScope.prototype.importScripts = function(...urls) {
        for (const rawUrl of urls) {
            let resolved = String(rawUrl);
            try { resolved = new URL(resolved, (globalThis.location && globalThis.location.href) || '').href; } catch (e) {}
            const source = ops.op_worker_load_script(resolved);
            if (source === null || source === undefined) {
                throw new DOMException(`Failed to execute 'importScripts': The script at '${resolved}' could not be loaded.`, 'NetworkError');
            }
            ops.op_worker_run_script(resolved, source);
        }
    };

    WorkerGlobalScope.prototype.setTimeout = _wrappedSetTimeout;
    WorkerGlobalScope.prototype.setInterval = _wrappedSetInterval;
    WorkerGlobalScope.prototype.clearTimeout = _wrappedClearTimeout;
    WorkerGlobalScope.prototype.clearInterval = _wrappedClearInterval;
    delete globalThis.setTimeout;
    delete globalThis.setInterval;
    delete globalThis.clearTimeout;
    delete globalThis.clearInterval;

    for (const m of ['fetch', 'atob', 'btoa', 'queueMicrotask', 'reportError', 'structuredClone', 'createImageBitmap']) {
        if (m in globalThis) {
            WorkerGlobalScope.prototype[m] = globalThis[m];
            delete globalThis[m];
        }
    }

    const _workerConstructors = [
        "WebSocketStream", "WebSocketError", "RestrictionTarget", "RTCTransformEvent", "RTCRtpScriptTransformer",
        "RTCDataChannel", "QuotaExceededError", "PushSubscriptionOptions", "PushSubscription", "PushManager",
        "PeriodicSyncManager", "Origin", "CropTarget", "BackgroundFetchRegistration", "BackgroundFetchRecord",
        "BackgroundFetchManager", "XMLHttpRequestUpload", "WritableStreamDefaultWriter", "WritableStreamDefaultController",
        "WebGLVertexArrayObject", "WebGLUniformLocation", "WebGLTransformFeedback", "WebGLTexture", "WebGLSync",
        "WebGLShaderPrecisionFormat", "WebGLShader", "WebGLSampler", "WebGLRenderbuffer", "WebGLQuery",
        "WebGLProgram", "WebGLObject", "WebGLFramebuffer", "WebGLContextEvent", "WebGLBuffer",
        "WebGLActiveInfo", "VideoFrame", "VideoColorSpace", "UserActivation", "TrustedTypePolicyFactory",
        "TrustedTypePolicy", "TrustedScriptURL", "TrustedScript", "TrustedHTML", "TransformStreamDefaultController",
        "TextMetrics", "TaskSignal", "TaskPriorityChangeEvent", "TaskController", "SyncManager",
        "Subscriber", "SourceBufferList", "SourceBuffer", "SecurityPolicyViolationEvent", "ReportingObserver",
        "ReportBody", "ReadableStreamDefaultReader", "ReadableStreamDefaultController", "ReadableStreamBYOBRequest",
        "ReadableStreamBYOBReader", "ReadableByteStreamController", "RTCEncodedVideoFrame", "RTCEncodedAudioFrame",
        "Permissions", "PermissionStatus", "PerformanceServerTiming", "PerformanceResourceTiming",
        "PerformanceObserverEntryList", "PerformanceMeasure", "PerformanceMark", "PerformanceEntry",
        "Performance", "OffscreenCanvasRenderingContext2D", "Observable", "NavigatorUAData",
        "MediaSourceHandle", "MediaSource", "MediaCapabilities", "ImageBitmapRenderingContext",
        "IDBVersionChangeEvent", "IDBTransaction", "IDBRequest", "IDBRecord", "IDBOpenDBRequest",
        "IDBObjectStore", "IDBIndex", "IDBFactory", "IDBDatabase", "IDBCursorWithValue",
        "IDBCursor", "FileReaderSync", "EncodedVideoChunk", "EncodedAudioChunk", "DecompressionStream",
        "DOMStringList", "DOMRectReadOnly", "DOMQuad", "DOMPointReadOnly", "DOMMatrixReadOnly",
        "CountQueuingStrategy", "CompressionStream", "CloseEvent", "CanvasPattern", "CanvasGradient",
        "CSSSkewY", "CSSSkewX", "ByteLengthQueuingStrategy", "AudioData",
        "webkitRequestFileSystemSync", "webkitResolveLocalFileSystemSyncURL", "webkitResolveLocalFileSystemURL",
        "AudioDecoder", "AudioEncoder", "Cache", "CacheStorage", "CreateMonitor",
        "FileSystemSyncAccessHandle", "GPU", "GPUAdapter", "GPUAdapterInfo", "GPUBindGroup",
        "GPUBindGroupLayout", "GPUBuffer", "GPUBufferUsage", "GPUCanvasContext", "GPUColorWrite",
        "GPUCommandBuffer", "GPUCommandEncoder", "GPUCompilationInfo", "GPUCompilationMessage",
        "GPUComputePassEncoder", "GPUComputePipeline", "GPUDevice", "GPUDeviceLostInfo",
        "GPUError", "GPUExternalTexture", "GPUInternalError", "GPUMapMode", "GPUOutOfMemoryError",
        "GPUPipelineError", "GPUPipelineLayout", "GPUQuerySet", "GPUQueue", "GPURenderBundle",
        "GPURenderBundleEncoder", "GPURenderPassEncoder", "GPURenderPipeline", "GPUSampler",
        "GPUShaderModule", "GPUShaderStage", "GPUSupportedFeatures", "GPUSupportedLimits",
        "GPUTexture", "GPUTextureUsage", "GPUTextureView", "GPUUncapturedErrorEvent", "GPUValidationError",
        "IdleDetector", "ImageDecoder", "ImageTrack", "ImageTrackList", "NavigationPreloadManager",
        "ServiceWorkerRegistration", "StorageManager", "VideoDecoder", "VideoEncoder", "WGSLLanguageFeatures",
        "WebTransport", "WebTransportBidirectionalStream", "WebTransportDatagramDuplexStream", "WebTransportError",
        "BarcodeDetector", "FileSystemDirectoryHandle", "FileSystemFileHandle", "FileSystemHandle",
        "FileSystemWritableFileStream", "FileSystemObserver", "HID", "HIDConnectionEvent",
        "HIDDevice", "HIDInputReportEvent", "Lock", "LockManager", "PressureObserver",
        "PressureRecord", "Serial", "SerialPort", "StorageBucket", "StorageBucketManager",
        "USB", "USBAlternateInterface", "USBConfiguration", "USBConnectionEvent", "USBDevice",
        "USBEndpoint", "USBInTransferResult", "USBInterface", "USBIsochronousInTransferPacket",
        "USBIsochronousInTransferResult", "USBIsochronousOutTransferPacket", "USBIsochronousOutTransferResult",
        "USBOutTransferResult"
    ];
    for (const name of _workerConstructors) {
        if (!(name in globalThis)) {
            const ctor = function() {
                throw new TypeError("Failed to construct '" + name + "': Please use the 'new' operator, this DOM object cannot be constructed.");
            };
            Object.defineProperty(ctor, 'name', { value: name, configurable: true });
            try { Object.defineProperty(ctor.prototype, Symbol.toStringTag, { value: name, configurable: true }); } catch (e) {}
            Object.defineProperty(globalThis, name, { value: ctor, writable: true, configurable: true, enumerable: false });
        }
    }
    if (!('onrtctransform' in globalThis)) {
        globalThis.onrtctransform = null;
    }

    let _workerName = '';
    Object.defineProperty(globalThis, 'name', {
        configurable: true,
        enumerable: true,
        get() { return _workerName; },
        set(v) { Object.defineProperty(this, 'name', { value: String(v), writable: true, configurable: true, enumerable: true }); }
    });

    let _workerOnMessageHandler = null;
    let _workerOnMessageWrapper = null;
    const onmessageDescriptor = {
        configurable: true,
        enumerable: true,
        get() { return _workerOnMessageHandler; },
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
    Object.defineProperty(globalThis, 'onmessage', onmessageDescriptor);

    let _workerOnMessageErrorHandler = null;
    let _workerOnMessageErrorWrapper = null;
    const onmessageerrorDescriptor = {
        configurable: true,
        enumerable: true,
        get() { return _workerOnMessageErrorHandler; },
        set(val) {
            _workerOnMessageErrorHandler = typeof val === 'function' || (val && typeof val.handleEvent === 'function') ? val : null;
            if (!_workerOnMessageErrorWrapper) {
                _workerOnMessageErrorWrapper = function(event) {
                    if (typeof _workerOnMessageErrorHandler === 'function') {
                        _workerOnMessageErrorHandler.call(globalThis, event);
                    } else if (_workerOnMessageErrorHandler && typeof _workerOnMessageErrorHandler.handleEvent === 'function') {
                        _workerOnMessageErrorHandler.handleEvent.call(_workerOnMessageErrorHandler, event);
                    }
                };
                globalThis.addEventListener('messageerror', _workerOnMessageErrorWrapper);
            }
        },
    };
    Object.defineProperty(globalThis, 'onmessageerror', onmessageerrorDescriptor);

    function _serializeWorkerMsg(msg, options) {
        const transfers = options == null ? [] : Array.from(
            typeof options[Symbol.iterator] === 'function' ? options : (options.transfer || []));
        return ops.op_worker_serialize(msg, transfers, message => {
            throw new DOMException(message, 'DataCloneError');
        });
    }
    function _deserializeWorkerMsg(data) {
        return {v: ops.op_worker_deserialize(data)};
    }

    globalThis.postMessage = function(msg, options = undefined) {
        const json = _serializeWorkerMsg(msg, options);
        ops.op_worker_post_to_parent(workerId, json);
    };

    globalThis.close = function() {
        closing = true;
        ops.op_worker_close();
    };

    globalThis.cancelAnimationFrame = function(id) {
        globalThis.clearTimeout(id);
    };
    globalThis.requestAnimationFrame = function(cb) {
        return globalThis.setTimeout(cb, 16);
    };

    const _workerReceiveSymbol = Symbol.for('__obscura_worker_receive');
    globalThis[_workerReceiveSymbol] = function(json) {
        if (closing) return;
        const payload = _deserializeWorkerMsg(json);
        if (!payload) return;
        const event = new MessageEvent('message', { data: payload.v });
        try { Object.defineProperties(event, { target: { value: globalThis }, currentTarget: { value: globalThis } }); } catch (e) {}
        globalThis.dispatchEvent(event);
    };

    for (const k of Object.getOwnPropertyNames(globalThis)) {
        if (k.startsWith('__obscura_')) {
            try { delete globalThis[k]; } catch (e) {}
        }
    }
})
"#;

#[op2(nofast, reentrant)]
pub fn op_worker_create(scope: &mut v8::HandleScope, state: &OpState, #[string] url: &str) -> u32 {
    let Some(registry) = state.try_borrow::<Rc<RefCell<WorkerRegistry>>>().cloned() else { return 0; };
    let resources = registry.borrow().resources.clone();
    let Ok(lease) = resources.worker() else { return 0; };
    if state.try_borrow::<WorkerEndpoint>().is_some() { refresh_policy(state); }
    else { sync_policy(state); }
    let scope = &mut v8::TryCatch::new(scope);
    let context = scope.get_current_context();
    let global = context.global(scope);
    let mut globals = serde_json::Map::new();
    for name in ["__obscura_ua", "__obscura_platform", "__obscura_ua_platform",
        "__obscura_ua_platform_version", "__obscura_ua_full_version", "__obscura_ua_architecture",
        "__obscura_language", "__obscura_languages", "__obscura_accept_language",
        "__obscura_webgl_vendor", "__obscura_webgl_renderer",
        "__obscura_hw", "__obscura_mem", "__obscura_network_downlink", "__obscura_network_rtt",
        "__obscura_network_effective_type", "__obscura_network_save_data"] {
        let key = v8::String::new(scope, name).unwrap();
        if let Some(value) = global.get(scope, key.into()).filter(|v| !v.is_undefined()) {
            if let Ok(value) = deno_core::serde_v8::from_v8::<serde_json::Value>(scope, value) {
                globals.insert(name.to_string(), value);
            }
        }
    }
    let key = v8::String::new(scope, "__blobStore").unwrap();
    let blobs = global.get(scope, key.into()).and_then(|value|
        deno_core::serde_v8::from_v8::<HashMap<String, String>>(scope, value).ok()).unwrap_or_default();
    if scope.has_caught() || scope.has_terminated() {
        scope.rethrow();
        return 0;
    }
    let parent = state.borrow::<Rc<RefCell<crate::ops::ObscuraState>>>().borrow();
    if let Some(doc_url) = parent.dom.as_ref().and_then(obscura_dom::DomTree::document_url) {
        if let Ok(parsed) = url::Url::parse(&doc_url) {
            globals.insert("__obscura_creator_origin".into(), serde_json::Value::String(parsed.origin().ascii_serialization()));
            let host = parsed.host_str().unwrap_or("");
            let is_secure = matches!(parsed.scheme(), "https" | "wss" | "file")
                || host == "localhost"
                || host.ends_with(".localhost")
                || host == "127.0.0.1"
                || host == "::1"
                || host == "[::1]"
                || (host.starts_with("127.") && host.split('.').count() == 4);
            globals.insert("__obscura_is_secure_context".into(), serde_json::Value::Bool(is_secure));
        }
    }
    globals.insert("__obscura_cross_origin_isolated".into(), serde_json::Value::Bool(false));
    // The worker runs on its own thread and its own tokio runtime, so it must
    // NOT reuse the page's transport: a connection pool belongs to the runtime
    // that drives it, and a pooled connection carried across runtimes is already
    // dead - its first reuse fails with a broken pipe. Build a sibling transport
    // that keeps the same identity (cookies, profile, proxy, policy) but owns a
    // fresh pool.
    // Same reasoning for the non-stealth client: a pool belongs to the runtime
    // that drives it, so the worker needs its own instance too.
    let worker_http = parent.http_client.as_ref()
        .map(|client| std::sync::Arc::new(client.detached()));
    #[cfg(feature = "stealth")]
    let worker_stealth = parent.stealth_client.as_ref().map(|client| {
        std::sync::Arc::new(obscura_net::StealthHttpClient::detached(client))
    });
    let config = WorkerConfig { policy: registry.borrow().policy.clone(), resources: resources.clone(), url: url.into(), globals, blobs,
        identity: parent.device_identity.clone(), cookies: parent.cookie_jar.clone(),
        http: worker_http, callbacks: parent.callbacks.clone(),
        #[cfg(feature = "stealth")]
        stealth: worker_stealth,
        blocked_urls: parent.blocked_urls.clone(), referrer_policy: parent.referrer_policy,
        intercept_tx: parent.intercept_tx.clone(), intercept_enabled: parent.intercept_enabled,
        intercept_counter: parent.intercept_counter.clone(), response_counter: parent.network_response_body_counter.clone(),
        in_flight: parent.page_in_flight.clone(), console_enabled: parent.console_messages_enabled,
        runtime_events_enabled: parent.runtime_events_enabled,
    };
    drop(parent);
    let (commands, command_rx) = queue::channel(resources.clone());
    let (events, event_rx) = queue::channel(resources);
    let control = std::sync::Arc::new(WorkerControl::default());
    let child_control = control.clone();
    let mut registry = registry.borrow_mut();
    registry.next_id = registry.next_id.saturating_add(1);
    let id = registry.next_id;
    if std::thread::Builder::new().name(format!("obscura-worker-{id}")).spawn(move || {
        let _lease = lease;
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build();
        if let Ok(runtime) = runtime {
            runtime.block_on(run_worker(id, config, command_rx, events, child_control));
        }
    }).is_err() { return 0; }
    registry.workers.insert(id, WorkerInstance { commands,
        events: std::sync::Arc::new(tokio::sync::Mutex::new(event_rx)), control });
    id
}

async fn run_worker(id: u32, config: WorkerConfig,
    mut commands: queue::Receiver<WorkerCommand>,
    events: queue::Sender<WorkerEvent>, control: std::sync::Arc<WorkerControl>) {
    if control.stopped() { return; }
    let locale = config.globals.get("__obscura_language").and_then(|v| v.as_str()).unwrap_or("en-US");
    let mut rt = crate::runtime::ObscuraJsRuntime::with_base_url_proxy_and_locale(&config.url, None, locale);
    {
        let mut handle = control.isolate.lock().unwrap();
        *handle = Some(rt.isolate_handle());
        if control.stopped() { return; }
    }
    {
        let mut state = rt.state.borrow_mut();
        state.url = config.url.clone();
        state.device_identity = config.identity;
        state.cookie_jar = config.cookies;
        state.http_client = config.http;
        state.callbacks = config.callbacks;
        #[cfg(feature = "stealth")]
        { state.stealth_client = config.stealth; }
        state.blocked_urls = config.blocked_urls;
        state.referrer_policy = config.referrer_policy;
        state.intercept_tx = config.intercept_tx;
        state.intercept_enabled = config.intercept_enabled;
        state.intercept_counter = config.intercept_counter;
        state.network_response_body_counter = config.response_counter;
        state.page_in_flight = config.in_flight;
        state.console_messages_enabled = config.console_enabled;
        state.runtime_events_enabled = config.runtime_events_enabled;
    }
    {
        let state = rt.runtime().op_state();
        let state = state.borrow();
        let mut registry = state.borrow::<Rc<RefCell<WorkerRegistry>>>().borrow_mut();
        registry.resources = config.resources;
        registry.policy = config.policy;
    }
    let endpoint = WorkerEndpoint { control: control.clone(), events, closing: std::cell::Cell::new(false), blobs: config.blobs };
    rt.runtime().op_state().borrow_mut().put(endpoint);
    let setup = format!("Object.assign(globalThis, {});", serde_json::Value::Object(config.globals));
    if rt.execute_script("worker-identity", &setup).is_err() { return; }
    rt.run_page_init();
    if let Err(error) = rt.initialize_worker_scope(id, &config.url) {
        rt.runtime().op_state().borrow().borrow::<WorkerEndpoint>().emit("error", &error);
        return;
    }
    let mut idle = true;
    loop {
        flush_observations(&rt.runtime().op_state().borrow());
        if control.stopped() || rt.runtime().op_state().borrow().borrow::<WorkerEndpoint>().closing.get() { break; }
        let command = if idle { commands.recv().await.map(queue::Queued::into_inner) } else {
            tokio::select! {
                command = commands.recv() => command.map(queue::Queued::into_inner),
                result = rt.run_autonomous_event_loop_turn() => {
                    match result {
                        Ok(done) => idle = done,
                        Err(error) => {
                            rt.runtime().op_state().borrow().borrow::<WorkerEndpoint>().emit("error", &error);
                            if control.stopped() { break; }
                            idle = false;
                        }
                    }
                    continue;
                }
            }
        };
        if control.stopped() { break; }
        let result = match command {
            Some(WorkerCommand::Run(source)) => rt.execute_worker_script(&source),
            Some(WorkerCommand::Message(json)) => rt.execute_worker_script(&format!("(globalThis[Symbol.for('__obscura_worker_receive')] || globalThis.__obscura_worker_receive)({});", serde_json::to_string(&json).unwrap())),
            Some(WorkerCommand::Stop) | None => break,
        };
        if let Err(error) = result {
            rt.runtime().op_state().borrow().borrow::<WorkerEndpoint>().emit("error", &error);
        }
        idle = false;
    }
    // Clear the cross-thread handle before dropping its owning isolate.
    *control.isolate.lock().unwrap() = None;
}

#[op2]
#[string]
pub fn op_worker_run(state: &OpState, worker_id: u32, #[string] source: &str) -> Option<String> {
    let registry = state.borrow::<Rc<RefCell<WorkerRegistry>>>().borrow();
    let worker = registry.workers.get(&worker_id)?;
    worker.commands.send(WorkerCommand::Run(source.into())).err().map(str::to_owned)
}

#[op2(fast)]
pub fn op_worker_post_to_worker(state: &OpState, worker_id: u32, #[string] json: &str) -> Result<(), deno_error::JsErrorBox> {
    if let Some(worker) = state.borrow::<Rc<RefCell<WorkerRegistry>>>().borrow().workers.get(&worker_id) {
        worker.commands.send(WorkerCommand::Message(json.into())).map_err(deno_error::JsErrorBox::generic)?;
    }
    Ok(())
}

#[op2(async)]
#[string]
pub async fn op_worker_next_event(state: Rc<RefCell<OpState>>, worker_id: u32) -> Option<String> {
    let events = {
        let state = state.borrow();
        let registry = state.borrow::<Rc<RefCell<WorkerRegistry>>>().borrow();
        registry.workers.get(&worker_id)?.events.clone()
    };
    loop {
        let event = events.lock().await.recv().await?.into_inner();
        match event {
            WorkerEvent::Script(data) => return Some(data),
            WorkerEvent::Observations(observations) => {
                let state = state.borrow();
                let mut parent = state.borrow::<Rc<RefCell<crate::ops::ObscuraState>>>().borrow_mut();
                observations.deliver(&mut parent);
            }
        }
    }
}

#[op2(fast)]
pub fn op_worker_post_to_parent(state: &OpState, _worker_id: u32, #[string] json: &str) {
    flush_observations(state);
    if let Some(endpoint) = state.try_borrow::<WorkerEndpoint>() {
        endpoint.emit("message", json);
    }
}

#[op2(fast)]
pub fn op_worker_close(state: &OpState) {
    if let Some(endpoint) = state.try_borrow::<WorkerEndpoint>() { endpoint.closing.set(true); }
}

#[op2(fast)]
pub fn op_worker_terminate(state: &OpState, worker_id: u32) {
    state.borrow::<Rc<RefCell<WorkerRegistry>>>().borrow_mut().workers.remove(&worker_id);
}

#[op2]
#[string]
pub fn op_worker_load_script(state: &OpState, #[string] url: &str) -> Option<String> {
    state.try_borrow::<WorkerEndpoint>()?.blobs.get(url).cloned()
}

#[op2(reentrant)]
#[string]
pub fn op_worker_run_script(
    scope: &mut v8::HandleScope,
    #[string] url: &str,
    #[string] source: &str,
) -> Option<String> {
    let source_str = v8::String::new(scope, source)?;
    let name_str = v8::String::new(scope, url)?;
    let origin = v8::ScriptOrigin::new(
        scope,
        name_str.into(),
        0,
        0,
        false,
        0,
        None,
        false,
        false,
        false,
        None,
    );
    let tc = &mut v8::TryCatch::new(scope);
    let script = v8::Script::compile(tc, source_str, Some(&origin));
    let Some(script) = script else {
        if tc.has_caught() {
            tc.rethrow();
            return None;
        }
        return Some("Worker script compilation failed".into());
    };
    if script.run(tc).is_none() {
        if tc.has_caught() {
            tc.rethrow();
            return None;
        }
        return Some("Worker script execution failed".into());
    }
    None
}

struct WorkerSerializer<'s> {
    error: v8::Local<'s, v8::Function>,
}
impl v8::ValueSerializerImpl for WorkerSerializer<'_> {
    fn throw_data_clone_error<'s>(&self, scope: &mut v8::HandleScope<'s>, message: v8::Local<'s, v8::String>) {
        let scope = &mut v8::TryCatch::new(scope);
        let receiver = v8::undefined(scope);
        self.error.call(scope, receiver.into(), &[message.into()]);
        if scope.has_caught() || scope.has_terminated() { scope.rethrow(); }
    }
    fn get_shared_array_buffer_id<'s>(&self, scope: &mut v8::HandleScope<'s>, _: v8::Local<'s, v8::SharedArrayBuffer>) -> Option<u32> {
        let message = v8::String::new(scope, "SharedArrayBuffer messaging is not supported").unwrap();
        self.throw_data_clone_error(scope, message);
        None
    }
    fn get_wasm_module_transfer_id(&self, scope: &mut v8::HandleScope<'_>, _: v8::Local<v8::WasmModuleObject>) -> Option<u32> {
        let message = v8::String::new(scope, "WebAssembly.Module messaging is not supported").unwrap();
        self.throw_data_clone_error(scope, message);
        None
    }
}

// Copy the value before detaching transfer buffers. A failed clone must not
// consume the sender's buffers. Only owned bytes cross the thread boundary.
#[op2(reentrant)]
#[string]
pub fn op_worker_serialize(
    scope: &mut v8::HandleScope,
    value: v8::Local<v8::Value>,
    transfers: v8::Local<v8::Array>,
    error: v8::Local<v8::Function>,
) -> String {
    use base64::Engine;
    use v8::{ValueSerializerHelper, ValueSerializerImpl};
    let mut buffers = Vec::new();
    for index in 0..transfers.length() {
        let Some(value) = transfers.get_index(scope, index) else { return String::new(); };
        let buffer = v8::Local::<v8::ArrayBuffer>::try_from(value).ok();
        if buffer.is_none_or(|b| !b.is_detachable() || b.was_detached() || buffers.contains(&b)) {
            let message = v8::String::new(scope, "Invalid or duplicate transferable ArrayBuffer").unwrap();
            WorkerSerializer { error }.throw_data_clone_error(scope, message);
            return String::new();
        }
        buffers.push(buffer.unwrap());
    }
    let serializer = v8::ValueSerializer::new(scope, Box::new(WorkerSerializer { error }));
    serializer.write_header();
    let scope = &mut v8::TryCatch::new(scope);
    let result = serializer.write_value(scope.get_current_context(), value);
    if scope.has_caught() || scope.has_terminated() {
        scope.rethrow();
        return String::new();
    }
    if result != Some(true) {
        let message = v8::String::new(scope, "The object could not be cloned").unwrap();
        WorkerSerializer { error }.throw_data_clone_error(scope, message);
        return String::new();
    }
    if buffers.iter().any(|buffer| buffer.was_detached()) {
        let message = v8::String::new(scope, "ArrayBuffer was detached during serialization").unwrap();
        WorkerSerializer { error }.throw_data_clone_error(scope, message);
        return String::new();
    }
    for buffer in buffers { buffer.detach(None); }
    base64::engine::general_purpose::STANDARD.encode(serializer.release())
}

struct WorkerDeserializer;
impl v8::ValueDeserializerImpl for WorkerDeserializer {}

#[op2]
pub fn op_worker_deserialize<'s>(scope: &mut v8::HandleScope<'s>, #[string] data: &str) -> v8::Local<'s, v8::Value> {
    use base64::Engine;
    use v8::ValueDeserializerHelper;
    let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(data) else { return v8::undefined(scope).into(); };
    let deserializer = v8::ValueDeserializer::new(scope, Box::new(WorkerDeserializer), &bytes);
    if deserializer.read_header(scope.get_current_context()) != Some(true) { return v8::undefined(scope).into(); }
    deserializer.read_value(scope.get_current_context()).unwrap_or_else(|| v8::undefined(scope).into())
}
