use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;
use std::sync::Arc;

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use deno_core::op2;
use deno_core::Extension;
use deno_core::JsBuffer;
use deno_core::v8;
use deno_core::OpState;
use obscura_dom::{DomTree, NodeData, NodeId};
use obscura_dom::tree::{AttachShadowError, ShadowRootMode};
#[cfg(feature = "render")]
use obscura_net::RequestCredentials;
use obscura_net::StealthHttpClient;
use obscura_net::{
    RequestMode, CallbackRegistry, CookieJar, ObscuraHttpClient, RequestInfo, ResourceType, ResourceRequest, ReferrerPolicy,
};
use tokio::sync::Mutex;

#[cfg(feature = "render")]
use serde::Deserialize;

use crate::import_map::ImportMap;
use crate::write_stream::DocumentWriteStream;

pub type InterceptCallback = Arc<
    Mutex<
        Option<Box<dyn Fn(String, String, String) -> Option<(u16, String, String)> + Send + Sync>>,
    >,
>;

#[derive(Debug)]
pub enum InterceptResolution {
    Continue {
        url: Option<String>,
        method: Option<String>,
        headers: Option<HashMap<String, String>>,
        /// Exact request body bytes; None leaves the original body unchanged.
        body: Option<Vec<u8>>,
    },
    /// Ordered CDP request fields. HTTP normalizes names and field ordering at
    /// the transport boundary; repeated values retain their relative order.
    ContinueWithHeaders {
        url: Option<String>,
        method: Option<String>,
        headers: Vec<(String, String)>,
        body: Option<Vec<u8>>,
    },
    /// Resume a response-stage pause. The body has already been captured in
    /// full; CDP may replace only the response status and headers here.
    ContinueResponse {
        status: Option<u16>,
        status_text: Option<String>,
        headers: Option<HashMap<String, String>>,
        raw_headers: Option<obscura_net::HeaderCapture>,
    },
    Fulfill {
        status: u16,
        headers: HashMap<String, String>,
        /// Lossy UTF-8 view of the fulfilled body, for text consumers.
        body: String,
        /// The exact fulfilled body as standard base64. CDP delivers the
        /// fulfillRequest body base64-encoded; carrying it through unchanged
        /// lets the bootstrap fetch layer reconstruct the exact bytes
        /// (`_base64ToUint8Array`) instead of a `from_utf8_lossy` corruption
        /// of any non-UTF-8 payload (image, font, protobuf). See #912.
        body_base64: String,
        body_supplied: bool,
    },
    /// CDP-supplied response fields, preserving repeats and original bytes.
    /// This is a synthetic capture, not headers observed at the transport.
    FulfillWithHeaders {
        status: u16,
        status_text: Option<String>,
        headers: HashMap<String, String>,
        raw_headers: obscura_net::HeaderCapture,
        body: String,
        body_base64: String,
        body_supplied: bool,
    },
    Fail {
        reason: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterceptionStage {
    Request,
    Response,
}

#[derive(Debug, Clone)]
pub struct FetchRequestPattern {
    pub url_pattern: String,
    pub resource_type: Option<String>,
}

fn cdp_resource_type(resource_type: ResourceType) -> &'static str {
    match resource_type {
        ResourceType::Document => "Document", ResourceType::Script => "Script",
        ResourceType::Stylesheet => "Stylesheet", ResourceType::Image => "Image",
        ResourceType::Font => "Font", ResourceType::Xhr => "XHR",
        ResourceType::Fetch => "Fetch", ResourceType::Other => "Other",
    }
}

fn request_patterns_match(patterns: &[FetchRequestPattern], url: &str, resource_type: ResourceType) -> bool {
    let resource_type = cdp_resource_type(resource_type);
    patterns.is_empty() || patterns.iter().any(|pattern|
        pattern.resource_type.as_deref().is_none_or(|expected| expected == resource_type)
            && glob_match(&pattern.url_pattern, url))
}

pub struct InterceptedRequest {
    pub stage: InterceptionStage,
    pub document_generation: u64,
    pub document_url: String,
    pub redirect_response: Option<obscura_net::observation::Exchange>,
    pub redirected_request_id: Option<String>,
    pub network_id: String,
    pub network_start: Arc<std::sync::atomic::AtomicU8>,
    pub request_raw_headers: Option<obscura_net::HeaderCapture>,
    pub request_body_size: usize,
    pub request_id: String,
    pub url: String,
    pub method: String,
    pub headers: HashMap<String, String>,
    pub resource_type: String,
    pub response_status_code: Option<u16>,
    pub response_headers: Option<HashMap<String, String>>,
    pub response_raw_headers: Option<obscura_net::HeaderCapture>,
    pub response_body_request_id: Option<String>,
    pub resolver: tokio::sync::oneshot::Sender<InterceptResolution>,
}

#[derive(Debug, Clone)]
pub struct StoredNetworkResponseBody {
    pub body: String,
    pub base64_encoded: bool,
}

pub(crate) fn stored_network_response_body(bytes: &[u8]) -> StoredNetworkResponseBody {
    match std::str::from_utf8(bytes) {
        Ok(body) => StoredNetworkResponseBody {
            body: body.to_owned(),
            base64_encoded: false,
        },
        Err(_) => StoredNetworkResponseBody {
            body: BASE64.encode(bytes),
            base64_encoded: true,
        },
    }
}

/// A network request made from page JS (fetch()/XHR/dynamic resource) recorded
/// so the CDP layer can emit Network.requestWillBeSent / responseReceived for
/// it. Static navigation subresources go through Page::record_network_event;
/// this is the parallel channel for script-initiated requests, which run in the
/// V8 op layer and would otherwise never surface as CDP Network events (#406).
#[derive(Debug, Clone)]
pub struct JsNetworkEvent {
    pub document_generation: u64,
    pub document_url: String,
    pub initiator_request_id: Option<String>,
    /// A terminal failure never produces loadingFinished, even with a real response.
    pub pending: bool,
    pub error: Option<String>,
    pub request_body_size: usize,
    pub request_post_data: Option<String>,
    pub request_started: bool,
    pub redirect: bool,
    pub response_body_request_id: Option<String>,
    /// Matches the `fetch-{N}` id under which the body is stored, so CDP
    /// Network.getResponseBody resolves for the same request.
    pub request_id: String,
    pub url: String,
    pub method: String,
    pub resource_type: ResourceType,
    pub status: u16,
    pub status_text: String,
    pub response_headers: HashMap<String, String>,
    pub raw_headers: Option<obscura_net::HeaderCapture>,
    pub request_raw_headers: Option<obscura_net::HeaderCapture>,
    pub body_size: usize,
    pub timestamp: f64,
    pub request_timestamp: f64,
    pub request_prepared_timestamp: Option<f64>,
    pub response_headers_timestamp: Option<f64>,
}

#[cfg(feature = "render")]
pub use obscura_render::ImageRequestProfile;

/// A live Canvas2D backing store retained from V8. `JsBuffer` owns a shared
/// reference to the ArrayBuffer backing store, so the pixels stay valid while
/// the canvas wrapper and native page state share it. Paint only borrows these
/// bytes synchronously while JavaScript is not executing.
#[cfg(feature = "render")]
pub(crate) struct CanvasBackingSurface {
    pub width: u32,
    pub height: u32,
    pub pixels: JsBuffer,
}

#[derive(Clone, Debug, Default)]
pub enum HistoryNavigation {
    #[default]
    Push,
    Replace,
    Reload,
    Traverse(u64),
}

#[derive(Clone)]
pub struct SessionHistoryEntry {
    pub id: u64,
    pub document: u64,
    pub url: String,
    pub data: Option<Vec<u8>>,
    pub scroll: String,
    pub position: Option<String>,
    pub request: ResourceRequest,
    pub post: bool,
}

pub type SharedSessionHistory = Rc<RefCell<SessionHistory>>;

pub struct SessionHistory {
    pub entries: Vec<SessionHistoryEntry>,
    pub index: usize,
    pub epoch: u64,
    next_id: u64,
    initial: bool,
}

impl Default for SessionHistory {
    fn default() -> Self {
        Self {
            entries: vec![SessionHistoryEntry {
                id: 0,
                document: 0,
                url: "about:blank".into(),
                data: None,
                scroll: "auto".into(),
                position: None,
                request: ResourceRequest::navigation(),
                post: false,
            }],
            index: 0,
            epoch: 0,
            next_id: 1,
            initial: true,
        }
    }
}

impl SessionHistory {
    pub fn current(&self) -> &SessionHistoryEntry {
        &self.entries[self.index]
    }
    pub fn save_position(&mut self, position: Option<String>) {
        self.entries[self.index].position = position;
    }
    fn allocate_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .expect("session history id exhausted");
        id
    }
    fn describe(&self) -> serde_json::Value {
        let e = self.current();
        serde_json::json!({"id":e.id.to_string(),"url":e.url,"data":e.data,
            "scroll":e.scroll,"position":e.position,"length":self.entries.len()})
    }
    pub fn commit_document(
        &mut self,
        url: &str,
        kind: &HistoryNavigation,
        request: ResourceRequest,
        post: bool,
        redirected: bool,
    ) -> Result<(), &'static str> {
        let target = match kind {
            HistoryNavigation::Traverse(id) => Some(
                self.entries
                    .iter()
                    .position(|e| e.id == *id)
                    .ok_or("HISTORY_ENTRY_GONE")?,
            ),
            HistoryNavigation::Reload => Some(self.index),
            _ => None,
        };
        self.epoch = self.epoch.checked_add(1).ok_or("HISTORY_EPOCH_EXHAUSTED")?;
        if let Some(index) = target {
            self.index = index;
            self.entries[index].url = url.into();
            if redirected {
                // Redirects create a new document state, even at the same URL.
                self.entries[index].document = self.epoch;
                self.entries[index].data = None;
            }
        } else {
            let entry = SessionHistoryEntry {
                id: self.allocate_id(),
                document: self.epoch,
                url: url.into(),
                data: None,
                scroll: self.current().scroll.clone(),
                position: None,
                request,
                post,
            };
            if self.initial || matches!(kind, HistoryNavigation::Replace) {
                self.entries[self.index] = entry;
            } else {
                self.entries.truncate(self.index + 1);
                self.entries.push(entry);
                self.index += 1;
            }
        }
        self.initial = false;
        Ok(())
    }
}

pub struct PendingNavigation {
    pub history: HistoryNavigation,
    pub url: String,
    pub method: String,
    pub body: String,
    pub request: ResourceRequest,
}

/// Immutable embedding-owned device description, shared by every page realm.
#[derive(Clone, serde::Serialize)]
pub struct DeviceIdentity {
    pub seed: u32,
    pub hardware_concurrency: u32,
    pub device_memory: f64,
    pub screen_width: u32,
    pub screen_height: u32,
    pub screen_color_depth: u32,
}

pub type SharedWebStorage = Arc<std::sync::Mutex<HashMap<String, Vec<(String, String)>>>>;

pub struct ObscuraState {
    pub persona: obscura_net::EffectivePersona,
    pub dom: Option<DomTree>,
    pub url: String,
    /// WHATWG canonical name of the document's character encoding (e.g.
    /// "UTF-8", "EUC-JP"). Backs `document.characterSet` and the URL query
    /// encoding override for `<a>`/`<area>` hrefs in legacy-charset documents.
    pub encoding: String,
    pub title: String,
    /// URL of the document that initiated this document's navigation. Direct
    /// browser/API navigations leave this empty; document-initiated
    /// navigations set it to the source document URL.
    pub referrer: String,
    pub referrer_policy: ReferrerPolicy,
    pub device_identity: Option<DeviceIdentity>,
    pub blocked_urls: Vec<String>,
    pub cookie_jar: Option<Arc<CookieJar>>,
    pub local_storage: SharedWebStorage,
    pub session_storage: SharedWebStorage,
    pub opaque_storage: SharedWebStorage,
    pub http_client: Option<Arc<ObscuraHttpClient>>,
    /// The owning page's passive on_request/on_response callbacks (issue
    /// #408). Page-scoped, so scripted fetch()/XHR observation stays local to
    /// the page that registered it.
    pub callbacks: Option<Arc<CallbackRegistry>>,
    /// Persona-owned transport shared by scripted and resource requests.
    pub stealth_client: Option<Arc<StealthHttpClient>>,
    pub session_history: SharedSessionHistory,
    pub history_epoch: u64,
    pub restoring_history_scroll: bool,
    pub pending_navigation: Option<PendingNavigation>,
    pub same_document_navigation: bool,
    pub intercept_tx: Option<tokio::sync::mpsc::UnboundedSender<InterceptedRequest>>,
    pub intercept_counter: Arc<std::sync::atomic::AtomicU64>,
    pub intercept_enabled: bool,
    pub intercept_request_patterns: Vec<FetchRequestPattern>,
    /// URL patterns enabled specifically at the CDP Fetch response stage.
    /// Empty keeps the old request-only interception behavior.
    pub intercept_response_patterns: Vec<FetchRequestPattern>,
    // Queue of (binding_name, payload) calls made by page JS via the
    // `op_binding_called` op. Drained by the CDP layer after each dispatch
    // and emitted as `Runtime.bindingCalled` events.
    pub pending_binding_calls: Vec<(String, String)>,
    // Console calls, uncaught exceptions, and opt-in diagnostics, in occurrence order.
    // The CDP layer drains this after commands and autonomous event-loop turns.
    pub pending_runtime_events: VecDeque<RuntimeEvent>,
    pub runtime_events_enabled: bool,
    /// Emit native script/storage diagnostics only when explicitly requested.
    pub diagnostic_events_enabled: bool,
    pub pending_console_messages: VecDeque<String>,
    pub console_messages_enabled: bool,
    pub runtime_exception_counter: u64,
    pub network_response_bodies: Arc<std::sync::Mutex<obscura_net::response_body::ResponseBodyStore>>,
    pub network_response_body_counter: Arc<std::sync::atomic::AtomicU64>,
    // Absolute URLs requested via JS fetch() / XHR (op_fetch_url), in request
    // order. Surfaced by `--dump assets` so resources pulled in by script, not
    // just static DOM attributes, are listed (issue #301).
    pub fetched_urls: Vec<String>,
    // Network events for script-initiated requests (fetch/XHR/dynamic resource),
    // drained by the Page into its network_events so the CDP layer emits
    // Network.requestWillBeSent / responseReceived for them (issue #406).
    pub js_network_events: Vec<JsNetworkEvent>,
    pub network_document_generation: u64,
    pub network_document_url: String,
    pub network_teardown_events: Arc<std::sync::Mutex<Vec<JsNetworkEvent>>>,
    pub network_teardown_notify: Arc<tokio::sync::Notify>,
    pub(crate) fetch_cancellations: HashMap<String, (tokio::sync::watch::Sender<Option<String>>, Option<String>)>,
    // Frame documents that have been fetched and are waiting for a realm.
    // Building one needs the whole runtime, which an op cannot reach, so
    // `op_frame_document_ready` queues here and the Page drains it between
    // event loop turns. Same shape as `pending_binding_calls`.
    pub pending_frames: Vec<PendingFrame>,
    /// Total URL and HTML bytes held by `pending_frames`.
    pub pending_frame_bytes: usize,
    pub frame_id_counter: u32,
    /// Which frame this state belongs to; 0 is the page's own realm.
    pub frame_id: u32,
    // postMessage traffic between realms, waiting to be delivered. A realm
    // cannot reach another realm's context on its own, so the message is queued
    // here and the Page dispatches it, the same way frames themselves are
    // built. Queued on the *page's* state whichever realm sent it, so one drain
    // sees the traffic of the whole tree.
    pub pending_frame_messages: Vec<PendingFrameMessage>,
    /// Bytes of payload currently queued above, tracked rather than summed so
    /// the cap costs nothing per message.
    pub pending_frame_message_bytes: usize,
    /// Requests initiated by this runtime only. Browser contexts share their
    /// transport client across pages, so the client's aggregate counter cannot
    /// be used as a page-readiness signal.
    pub page_in_flight: Arc<std::sync::atomic::AtomicU32>,
    /// Monotonic generation for observable changes to the connected document.
    /// The browser settle policy samples this to distinguish useful deferred
    /// rendering work from unrelated long-lived timers.
    pub activity_generation: u64,
    /// Monotonic identity of the currently installed document. Async resource
    /// completions use this to discard bytes and lifecycle results belonging
    /// to a navigation that has already been replaced.
    pub document_generation: u64,
    /// Input-local document identity. Unlike `document_generation`, this also
    /// advances for `document.open()`, whose replacement reuses the same
    /// document object and may reuse DOM node ids.
    pub input_document_epoch: Cell<u64>,
    pub document_lifecycle: u8,
    /// Cached document base URL. Computing it walks the tree and runs the selector engine, and
    /// the JS layer asks for it on every relative URL, including the URL parts of `<a>`.
    /// Interior mutability so the read path keeps its shared borrow.
    pub base_url_cache: RefCell<Option<BaseUrlCache>>,
    /// Final image/font-aware layout shared by CSSOM geometry and screenshots.
    /// DOM/style/viewport changes clear this value but retain resource bytes.
    #[cfg(feature = "render")]
    pub prepared_render: Option<obscura_render::PreparedRender>,
    /// CSS media type selected for the next retained layout. Live pages use
    /// screen; PDF export switches to print for one synchronous capture and
    /// restores screen before returning.
    #[cfg(feature = "render")]
    pub render_media: obscura_render::CssMediaType,
    /// Explicit document-timeline sample used by the next style/layout flush.
    /// Captures set this to either deterministic T=0 or live document time.
    #[cfg(feature = "render")]
    pub animation_sample: obscura_render::AnimationSample,
    #[cfg(feature = "render")]
    pub animation_timeline: obscura_render::AnimationTimelineState,
    #[cfg(feature = "render")]
    pub animation_timeline_origin: std::time::Instant,
    /// Host/HTML task epoch for document-timeline sampling. Geometry and
    /// computed-style reads within one task share one frozen animation frame.
    #[cfg(feature = "render")]
    pub animation_task_generation: u64,
    #[cfg(feature = "render")]
    pub animation_sampled_task_generation: u64,
    /// Connected mutations awaiting dependency-indexed retained style refresh.
    /// Tree changes carry stable node/parent ids so a later geometry read can
    /// coalesce framework DOM churn into one conservative local cascade.
    #[cfg(feature = "render")]
    pub pending_style_mutations: Vec<obscura_render::RetainedStyleMutation>,
    /// Page-lifetime raw image/font bytes. A new document resets this cache;
    /// relayout of the same document reuses it without refetching.
    #[cfg(feature = "render")]
    pub render_resources: obscura_render::RenderResourceCache,
    /// Waiters sharing an asynchronous HTMLImageElement request. The key keeps
    /// navigation identity and request credentials separate so neither stale
    /// pages nor incompatible CORS profiles share a completion.
    #[cfg(feature = "render")]
    pub render_image_in_flight:
        HashMap<(u64, String, ImageRequestProfile), Vec<tokio::sync::oneshot::Sender<()>>>,
    /// Page-transport loads for resources that cache-only layout or paint
    /// missed. The owning page fetches them and sends the outcome here; the
    /// runtime applies results at its own event-loop turns and at every
    /// promise wait, so a script polling geometry sees them. `document_generation`
    /// in each result fences a previous document's late answer.
    #[cfg(feature = "render")]
    pub render_resource_tx: tokio::sync::mpsc::UnboundedSender<RenderResourceLoad>,
    #[cfg(feature = "render")]
    pub render_resource_rx: tokio::sync::mpsc::UnboundedReceiver<RenderResourceLoad>,
    /// Resources currently loading through the page transport, so repeated
    /// misses never duplicate a request.
    #[cfg(feature = "render")]
    pub render_resource_in_flight:
        std::collections::HashSet<(String, Option<ImageRequestProfile>, bool)>,
    /// Applied transport responses the page still has to report as
    /// Network events (recording needs the page, not the runtime).
    #[cfg(feature = "render")]
    pub render_resource_events: Vec<RenderResourceEvent>,
    /// `Fetch.enable` URL patterns mirrored from the owning page, so the
    /// renderer's resource loads follow the same interception policy as the
    /// page's own subresource fetches (a matching URL is not fetched here).
    #[cfg(feature = "render")]
    pub intercept_block_patterns: Vec<String>,
    /// Background transport tasks of this document, one page-wide
    /// concurrency limit shared by all of them, a wake-up for waiters, and
    /// requests that could not be started outside a Tokio context.
    #[cfg(feature = "render")]
    pub render_resource_tasks: Vec<tokio::task::JoinHandle<()>>,
    #[cfg(feature = "render")]
    pub render_resource_limiter: Arc<tokio::sync::Semaphore>,
    #[cfg(feature = "render")]
    pub render_resource_notify: Arc<tokio::sync::Notify>,
    #[cfg(feature = "render")]
    pub render_resource_backlog: Vec<(String, Option<ImageRequestProfile>, bool)>,
    /// One exact-key compiled author stylesheet for this document. Connected
    /// mutations still discard `prepared_render`; the next prepare reuses only
    /// parsing/indexing when ordered CSS source and viewport remain identical.
    #[cfg(feature = "render")]
    pub stylesheet_cache: obscura_render::StylesheetCache,
    /// Script-created faces in this document's `FontFaceSet`. This is separate
    /// from the DOM so the bridge does not manufacture a selector-visible
    /// `<style>` element merely to feed the renderer.
    #[cfg(feature = "render")]
    pub dynamic_fonts: Vec<obscura_render::DynamicFontFace>,
    /// Live Canvas2D backing stores keyed by stable DOM identity. Pixel damage
    /// updates this resource independently of retained style/layout geometry.
    #[cfg(feature = "render")]
    pub(crate) canvas_surfaces: HashMap<NodeId, CanvasBackingSurface>,
    #[cfg(feature = "render")]
    pub viewport: (f32, f32),
    /// Root scrolling offset in CSS pixels. With render enabled this is
    /// clamped against the cached document overflow and is the single source
    /// read by CSSOM geometry and screenshot paint.
    #[cfg(feature = "render")]
    pub scroll_offset: (f32, f32),
    /// Element scroll offsets persist by DOM identity across relayout. Dense
    /// renderer ScrollIds are rebuild-local and are resolved only into the
    /// cached snapshot below.
    #[cfg(feature = "render")]
    pub element_scroll_offsets: HashMap<NodeId, (f32, f32)>,
    #[cfg(feature = "render")]
    pub scroll_generation: u64,
    /// Explicit CSSOM scroll changes; DOM cleanup and layout clamping do not count.
    #[cfg(feature = "render")]
    pub script_scroll_generation: u64,
    #[cfg(feature = "render")]
    pub resolved_scroll: Option<(u64, obscura_render::ResolvedScrollState)>,
    /// Window-global import-map state shared by parser-discovered scripts,
    /// dynamically inserted import maps, and the module loader.
    pub(crate) import_map: Rc<RefCell<ImportMap>>,
    /// HTML's per-script "already started" flag.  This is native page state,
    /// rather than wrapper state, because it must survive moves and clones and
    /// because fragment parsing can create nodes before a JS wrapper exists.
    pub(crate) already_started_scripts: RefCell<HashSet<NodeId>>,
    /// The document's input stream for `document.write()`, created on the first call.
    /// Why the calls share one parser is in `write_stream`.
    pub(crate) write_stream: RefCell<Option<crate::write_stream::DocumentWriteStream>>,
}

/// A frame document waiting to be given a realm.
pub struct PendingFrame {
    pub frame_id: u32,
    pub url: String,
    pub html: String,
    pub viewport_width: u64,
    pub viewport_height: u64,
    /// The frame that holds this one; 0 when the page does.
    pub parent_frame_id: u32,
}

/// One `postMessage` in flight between two realms.
pub struct PendingFrameMessage {
    /// Where it is going. 0 is the page's realm.
    pub target_frame_id: u32,
    /// Where it came from, so the receiver can reply through `event.source`.
    pub source_frame_id: u32,
    /// The sender's origin, for `event.origin`.
    pub origin: String,
    /// The origin the sender restricted delivery to (postMessage's
    /// `targetOrigin`). `"*"` means any origin; `"/"` means the receiver must be
    /// same-origin as the sender; anything else is matched against the
    /// receiver's own origin, and a mismatch drops the message. An empty string
    /// means the sender did not specify one and delivery stays permissive.
    pub target_origin: String,
    /// The payload, JSON encoded. Structured clone is not available across
    /// realms here, and JSON covers what postMessage is used for in practice:
    /// a widget reporting a result. Anything it cannot encode is rejected by
    /// the sender rather than silently arriving as null.
    pub data_json: String,
}

impl ObscuraState {
    /// Bind the injected persona once, unless the owning page already installed
    /// its transport. Never derive identity from mutable JS values.
    pub(crate) fn ensure_persona_transport(&mut self) -> Arc<StealthHttpClient> {
        if let Some(client) = &self.stealth_client {
            return client.clone();
        }
        let jar = self.cookie_jar.get_or_insert_with(|| Arc::new(CookieJar::new())).clone();
        let policy = self.http_client.get_or_insert_with(|| {
            Arc::new(ObscuraHttpClient::with_cookie_jar(jar.clone()))
        }).clone();
        let client = Arc::new(StealthHttpClient::with_policy_persona(
            jar, policy.proxy_url(), policy.clone(), &self.persona,
        ));
        self.stealth_client = Some(client.clone());
        client
    }

    pub fn new(persona: obscura_net::EffectivePersona) -> Self {
        #[cfg(feature = "render")]
        let (render_resource_tx, render_resource_rx) = tokio::sync::mpsc::unbounded_channel();
        ObscuraState {
            persona,
            dom: None,
            url: "about:blank".to_string(),
            encoding: "UTF-8".to_string(),
            title: String::new(),
            referrer: String::new(),
            referrer_policy: ReferrerPolicy::default(),
            device_identity: None,
            blocked_urls: Vec::new(),
            cookie_jar: None,
            local_storage: Default::default(),
            session_storage: Default::default(),
            opaque_storage: Default::default(),
            http_client: None,
            callbacks: None,
            stealth_client: None,
            session_history: Rc::new(RefCell::new(SessionHistory::default())),
            history_epoch: 0,
            restoring_history_scroll: false,
            pending_navigation: None,
            same_document_navigation: false,
            intercept_tx: None,
            intercept_counter: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            intercept_enabled: false,
            intercept_request_patterns: Vec::new(),
            intercept_response_patterns: Vec::new(),
            pending_binding_calls: Vec::new(),
            pending_runtime_events: VecDeque::new(),
            runtime_events_enabled: false,
            diagnostic_events_enabled: std::env::var("OBSCURA_CDP_DIAGNOSTICS").ok().as_deref() == Some("1"),
            pending_console_messages: VecDeque::new(),
            console_messages_enabled: false,
            runtime_exception_counter: 0,
            network_response_bodies: Arc::new(std::sync::Mutex::new(Default::default())),
            network_response_body_counter: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            fetched_urls: Vec::new(),
            js_network_events: Vec::new(),
            network_document_generation: 0, network_document_url: String::new(),
            network_teardown_events: Arc::new(std::sync::Mutex::new(Vec::new())),
            network_teardown_notify: Arc::new(tokio::sync::Notify::new()),
            fetch_cancellations: HashMap::new(),
            pending_frames: Vec::new(),
            pending_frame_bytes: 0,
            frame_id_counter: 0,
            frame_id: 0,
            pending_frame_messages: Vec::new(),
            pending_frame_message_bytes: 0,
            page_in_flight: Arc::new(std::sync::atomic::AtomicU32::new(0)),
            activity_generation: 0,
            document_generation: 0,
            input_document_epoch: Cell::new(0),
            document_lifecycle: 0,
            base_url_cache: RefCell::new(None),
            #[cfg(feature = "render")]
            prepared_render: None,
            #[cfg(feature = "render")]
            render_media: obscura_render::CssMediaType::Screen,
            #[cfg(feature = "render")]
            animation_sample: obscura_render::AnimationSample::default(),
            #[cfg(feature = "render")]
            animation_timeline: obscura_render::AnimationTimelineState::default(),
            #[cfg(feature = "render")]
            animation_timeline_origin: std::time::Instant::now(),
            #[cfg(feature = "render")]
            animation_task_generation: 0,
            #[cfg(feature = "render")]
            animation_sampled_task_generation: 0,
            #[cfg(feature = "render")]
            pending_style_mutations: Vec::new(),
            #[cfg(feature = "render")]
            render_resources: obscura_render::RenderResourceCache::default(),
            #[cfg(feature = "render")]
            render_image_in_flight: HashMap::new(),
            #[cfg(feature = "render")]
            render_resource_tx,
            #[cfg(feature = "render")]
            render_resource_rx,
            #[cfg(feature = "render")]
            render_resource_in_flight: std::collections::HashSet::new(),
            #[cfg(feature = "render")]
            render_resource_events: Vec::new(),
            #[cfg(feature = "render")]
            intercept_block_patterns: Vec::new(),
            #[cfg(feature = "render")]
            render_resource_tasks: Vec::new(),
            #[cfg(feature = "render")]
            render_resource_limiter: Arc::new(tokio::sync::Semaphore::new(
                RENDER_RESOURCE_CONCURRENCY,
            )),
            #[cfg(feature = "render")]
            render_resource_notify: Arc::new(tokio::sync::Notify::new()),
            #[cfg(feature = "render")]
            render_resource_backlog: Vec::new(),
            #[cfg(feature = "render")]
            stylesheet_cache: obscura_render::StylesheetCache::default(),
            #[cfg(feature = "render")]
            dynamic_fonts: Vec::new(),
            #[cfg(feature = "render")]
            canvas_surfaces: HashMap::new(),
            #[cfg(feature = "render")]
            viewport: (1280.0, 720.0),
            #[cfg(feature = "render")]
            scroll_offset: (0.0, 0.0),
            #[cfg(feature = "render")]
            element_scroll_offsets: HashMap::new(),
            #[cfg(feature = "render")]
            scroll_generation: 0,
            #[cfg(feature = "render")]
            script_scroll_generation: 0,
            #[cfg(feature = "render")]
            resolved_scroll: None,
            import_map: Rc::new(RefCell::new(ImportMap::default())),
            already_started_scripts: RefCell::new(HashSet::new()),
            write_stream: RefCell::new(None),
        }
    }
}

#[derive(Debug, Clone)]
pub struct RuntimeConsoleEvent {
    pub kind: String,
    pub args: Vec<serde_json::Value>,
    pub timestamp: f64,
}

#[derive(Debug, Clone)]
pub struct RuntimeExceptionEvent {
    pub exception_id: u64,
    pub name: String,
    pub description: String,
    pub url: String,
    pub line_number: i64,
    pub column_number: i64,
    pub stack_trace: Vec<serde_json::Value>,
    pub timestamp: f64,
}

#[derive(Debug, Clone)]
pub struct RuntimeScriptEvent {
    pub url: String,
    pub source_bytes: usize,
    pub source_sha256: String,
    pub started_at: f64,
    pub finished_at: f64,
    pub duration_ms: f64,
    pub outcome: String,
}

#[derive(Debug, Clone)]
pub struct RuntimeStorageEvent {
    pub origin: String,
    pub area: String,
    pub operation: String,
    pub key: Option<String>,
    pub old_bytes: Option<usize>,
    pub new_bytes: Option<usize>,
    pub old_sha256: Option<String>,
    pub new_sha256: Option<String>,
    pub timestamp: f64,
}

#[derive(Debug, Clone)]
pub struct RuntimeCookieEvent {
    pub origin: String,
    pub name: String,
    pub assignment_bytes: usize,
    pub assignment_sha256: String,
    pub timestamp: f64,
}

#[derive(Debug, Clone)]
pub enum RuntimeEvent {
    Console(RuntimeConsoleEvent),
    Exception(RuntimeExceptionEvent),
    Script(RuntimeScriptEvent),
    Storage(RuntimeStorageEvent),
    Cookie(RuntimeCookieEvent),
}

pub(crate) fn node_is_script(dom: &DomTree, node_id: NodeId) -> bool {
    dom.with_node(node_id, |node| {
        node.as_element()
            .map(|name| name.local.as_ref().eq_ignore_ascii_case("script"))
            .unwrap_or(false)
    })
    .unwrap_or(false)
}

fn script_nodes_including_template_contents(dom: &DomTree, root: NodeId) -> Vec<NodeId> {
    let mut scripts = Vec::new();
    let mut stack = vec![root];
    while let Some(node_id) = stack.pop() {
        if node_is_script(dom, node_id) {
            scripts.push(node_id);
        }
        let template_contents = dom
            .with_node(node_id, |node| match &node.data {
                NodeData::Element {
                    template_contents, ..
                } => *template_contents,
                _ => None,
            })
            .flatten();
        if let Some(contents) = template_contents {
            stack.push(contents);
        }
        let children = dom.children(node_id);
        for child in children.into_iter().rev() {
            stack.push(child);
        }
    }
    scripts
}

pub(crate) fn mark_script_subtree_started(state: &ObscuraState, root: NodeId) {
    let Some(dom) = state.dom.as_ref() else {
        return;
    };
    let scripts = script_nodes_including_template_contents(dom, root);
    state.already_started_scripts.borrow_mut().extend(scripts);
}

fn propagate_script_start_state(
    dom: &DomTree,
    source_root: NodeId,
    cloned_root: NodeId,
    started: &RefCell<HashSet<NodeId>>,
) {
    let mut pairs = vec![(source_root, cloned_root)];
    let mut additions = Vec::new();
    let current = started.borrow();
    while let Some((source, cloned)) = pairs.pop() {
        if current.contains(&source) {
            additions.push(cloned);
        }

        let source_template = dom
            .with_node(source, |node| match &node.data {
                NodeData::Element {
                    template_contents, ..
                } => *template_contents,
                _ => None,
            })
            .flatten();
        let cloned_template = dom
            .with_node(cloned, |node| match &node.data {
                NodeData::Element {
                    template_contents, ..
                } => *template_contents,
                _ => None,
            })
            .flatten();
        if let (Some(source_contents), Some(cloned_contents)) = (source_template, cloned_template) {
            pairs.push((source_contents, cloned_contents));
        }

        let source_children = dom.children(source);
        let cloned_children = dom.children(cloned);
        for pair in source_children.into_iter().zip(cloned_children).rev() {
            pairs.push(pair);
        }
    }
    drop(current);
    started.borrow_mut().extend(additions);
}

/// Hard cap on a single JS fetch/XHR response body buffered fully in memory.
/// `op_fetch_url` reads the whole body, then makes a UTF-8 copy and a base64
/// copy of it, so an unbounded body OOMs the process. This bounds the initial
/// read; raw response retention separately spools large bodies to disk.
/// Configurable via `OBSCURA_FETCH_MAX_BODY_BYTES`.
fn fetch_max_body_bytes() -> usize {
    std::env::var("OBSCURA_FETCH_MAX_BODY_BYTES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(100 * 1024 * 1024)
}

/// Cap on the append-only `fetched_urls` asset list. A page can otherwise loop
/// `fetch()`/XHR and grow it without bound on the process heap (where V8's
/// heap-limit guard never sees it). It only feeds the CLI's `--dump assets`
/// listing, so keeping a bounded most-recent window is enough.
const MAX_FETCHED_URLS: usize = 16384;

/// Push `item` onto `list`, evicting the oldest entries so it never holds more
/// than `max`. Mirrors the front-drain used for `js_network_events`.
fn push_capped(list: &mut Vec<String>, item: String, max: usize) {
    list.push(item);
    if list.len() > max {
        let overflow = list.len() - max;
        list.drain(0..overflow);
    }
}

pub type SharedState = Rc<RefCell<ObscuraState>>;

/// Which document belongs to which realm.
///
/// An op has to read the state of the realm that *called* it. Making a realm
/// "current" around the host's own calls into it is not enough: a frame's
/// deferred work, a timer firing or a promise settling, re-enters JavaScript
/// from the event loop, where nothing had the chance to swap anything. Without
/// this a frame's `setTimeout` callback runs with the frame's globals but
/// writes to the *parent's* DOM.
#[derive(Default)]
pub struct RealmStates {
    entries: Vec<(v8::Global<v8::Context>, u32, SharedState)>,
    retired_documents: HashMap<u32, std::rc::Weak<RefCell<ObscuraState>>>,
}

impl RealmStates {
    pub fn register(
        &mut self,
        context: v8::Global<v8::Context>,
        frame_id: u32,
        state: SharedState,
    ) {
        self.retired_documents.retain(|_, state| state.strong_count() > 0);
        self.entries.push((context, frame_id, state));
    }

    pub fn forget(&mut self, context: &v8::Global<v8::Context>) {
        self.retired_documents.retain(|_, state| state.strong_count() > 0);
        for (known, id, state) in &self.entries {
            if known == context {
                self.retired_documents.insert(*id, Rc::downgrade(state));
            }
        }
        self.entries.retain(|(known, _, _)| known != context);
    }

    fn by_document_frame_id(&self, frame_id: u32) -> Option<SharedState> {
        self.by_frame_id(frame_id).or_else(|| {
            self.retired_documents.get(&frame_id).and_then(std::rc::Weak::upgrade)
        })
    }

    fn by_frame_id(&self, frame_id: u32) -> Option<SharedState> {
        self.entries
            .iter()
            .find(|(_, id, _)| *id == frame_id)
            .map(|(_, _, state)| state.clone())
    }
}

/// The document of the realm a DOM call came from, named rather than inferred.
///
/// A wrapper's methods live on its own realm's prototypes, so the code running
/// for `parentPage.frameDoc.title` is the *frame's* getter even though the
/// caller is the page. Inferring the realm from the running context therefore
/// answers the wrong question for any cross-realm access, and would silently
/// read the page's document. Each realm's bootstrap closure knows its own frame
/// id and passes it, which is both correct here and cheaper than asking V8:
/// a page with no frames resolves on `frame_id == 0` alone.
pub fn frame_state(op_state: &OpState, frame_id: u32) -> Option<SharedState> {
    if frame_id == 0 {
        return Some(op_state.borrow::<SharedState>().clone());
    }
    let registry = op_state.try_borrow::<Rc<RefCell<RealmStates>>>()?;
    // A retired document can still be held by page JavaScript. Never route an
    // unknown child id to the parent: even a title read would cross documents.
    registry.borrow().by_document_frame_id(frame_id)
}

/// The state of the realm running right now, or the page's when the caller is
/// the page itself.
///
/// A page with no frames pays only an `is_empty` check: looking up the current
/// context is not free, and `op_dom` is the hottest op in the system.
pub fn realm_state(scope: &mut v8::HandleScope, op_state: &OpState) -> SharedState {
    let page = || op_state.borrow::<SharedState>().clone();
    let registry = match op_state.try_borrow::<Rc<RefCell<RealmStates>>>() {
        Some(registry) => registry.clone(),
        None => return page(),
    };
    let registry = registry.borrow();
    if registry.entries.is_empty() {
        return page();
    }
    // Not `get_current_context`: an op is a native function bound in the page
    // realm, so V8 reports that realm as current no matter who called it. This
    // one answers "whose code is running", which is the question.
    let current = scope.get_entered_or_microtask_context();
    registry
        .entries
        .iter()
        .find(|(context, _, _)| *context == current)
        .map(|(_, _, state)| state.clone())
        .unwrap_or_else(page)
}

#[derive(Clone, Copy, Debug, Default)]
struct RenderMutationImpact {
    connected: bool,
    actual_change: bool,
}

fn node_is_connected(dom: &DomTree, node: NodeId) -> bool {
    dom.is_connected(node)
}

#[cfg(feature = "render")]
fn shadow_including_connected_nodes(dom: &DomTree) -> HashSet<NodeId> {
    let mut connected = HashSet::new();
    let mut stack = vec![dom.document()];
    while let Some(node) = stack.pop() {
        if !connected.insert(node) {
            continue;
        }
        stack.extend(dom.children(node));
        if let Some(shadow_children) = dom.shadow_children(node) {
            stack.extend(shadow_children);
        }
    }
    connected
}

/// Classify whether a DOM command can make the retained document layout
/// stale. DOM construction is commonly performed in detached subtrees, and
/// frameworks also assign an attribute its current value. Neither operation
/// changes the rendered document. Chromium dirties layout when the mutation
/// reaches a connected style/layout owner, not merely because a mutating API
/// was entered.
fn render_mutation_impact(
    dom: &DomTree,
    cmd: &str,
    arg1: &str,
    arg2: &str,
) -> RenderMutationImpact {
    let node = |value: &str| value.parse::<u32>().ok().map(NodeId::new);
    match cmd {
        "set_form_value" | "set_form_checked" | "set_form_indeterminate" => {
            let Some(target) = node(arg1) else {
                return RenderMutationImpact::default();
            };
            let actual_change = match cmd {
                "set_form_value" => !dom.form_control_value_matches(target, arg2),
                "set_form_checked" => {
                    dom.form_control_checked(target) != Some(arg2 == "true")
                }
                _ => dom.form_control_indeterminate(target) != (arg2 == "true"),
            };
            RenderMutationImpact { connected: node_is_connected(dom, target), actual_change }
        }
        "set_attribute" => {
            let Some(target) = node(arg1) else {
                return RenderMutationImpact::default();
            };
            let Some((name, value)) = arg2.split_once('\0') else {
                return RenderMutationImpact::default();
            };
            let old = dom
                .with_node(target, |node| node.get_attribute(name).map(str::to_owned))
                .flatten();
            RenderMutationImpact {
                connected: node_is_connected(dom, target),
                actual_change: old.as_deref() != Some(value),
            }
        }
        "set_attribute_ns" => {
            let Some(target) = node(arg1) else {
                return RenderMutationImpact::default();
            };
            let mut parts = arg2.splitn(3, '\0');
            let namespace = parts.next().unwrap_or("");
            let qualified = parts.next().unwrap_or("");
            let value = parts.next().unwrap_or("");
            let local = qualified
                .split_once(':')
                .map(|(_, local)| local)
                .unwrap_or(qualified);
            let old = dom
                .with_node(target, |node| {
                    node.get_attribute_ns(namespace, local).map(str::to_owned)
                })
                .flatten();
            RenderMutationImpact {
                connected: node_is_connected(dom, target),
                actual_change: old.as_deref() != Some(value),
            }
        }
        "remove_attribute" => {
            let Some(target) = node(arg1) else {
                return RenderMutationImpact::default();
            };
            let existed = dom
                .with_node(target, |node| node.get_attribute(arg2).is_some())
                .unwrap_or(false);
            RenderMutationImpact {
                connected: node_is_connected(dom, target),
                actual_change: existed,
            }
        }
        "remove_attribute_ns" => {
            let Some(target) = node(arg1) else {
                return RenderMutationImpact::default();
            };
            let (namespace, local) = arg2.split_once('\0').unwrap_or(("", arg2));
            let existed = dom
                .with_node(target, |node| {
                    node.get_attribute_ns(namespace, local).is_some()
                })
                .unwrap_or(false);
            RenderMutationImpact {
                connected: node_is_connected(dom, target),
                actual_change: existed,
            }
        }
        "append_child" => {
            let (Some(parent), Some(child)) = (node(arg1), node(arg2)) else {
                return RenderMutationImpact::default();
            };
            if dom.get_node(parent).is_none() || dom.get_node(child).is_none() {
                return RenderMutationImpact::default();
            }
            let old_parent = dom.get_node(child).and_then(|node| node.parent);
            let already_last =
                old_parent == Some(parent) && dom.children(parent).last().copied() == Some(child);
            RenderMutationImpact {
                // Moving a connected node into a detached subtree removes its
                // old box, while attaching a detached node creates a new one.
                connected: node_is_connected(dom, parent) || node_is_connected(dom, child),
                actual_change: !already_last,
            }
        }
        "remove_child" => {
            let Some(child) = node(arg1) else {
                return RenderMutationImpact::default();
            };
            RenderMutationImpact {
                connected: node_is_connected(dom, child),
                actual_change: dom.get_node(child).and_then(|node| node.parent).is_some(),
            }
        }
        "insert_before" => {
            let (Some(new_node), Some(reference)) = (node(arg1), node(arg2)) else {
                return RenderMutationImpact::default();
            };
            if dom.get_node(new_node).is_none() {
                return RenderMutationImpact::default();
            }
            let Some(reference_parent) = dom.get_node(reference).and_then(|node| node.parent)
            else {
                return RenderMutationImpact::default();
            };
            let new_was_connected = node_is_connected(dom, new_node);
            let already_immediately_before =
                dom.get_node(reference).and_then(|node| node.prev_sibling) == Some(new_node);
            RenderMutationImpact {
                connected: node_is_connected(dom, reference_parent) || new_was_connected,
                actual_change: new_node != reference && !already_immediately_before,
            }
        }
        "set_inner_html" | "set_inner_html_context" => {
            let Some(target) = node(arg1) else {
                return RenderMutationImpact::default();
            };
            RenderMutationImpact {
                connected: node_is_connected(dom, target),
                // Parsing normalizes source text, so a cheap string comparison
                // cannot prove equality. Connected replacement remains dirty.
                actual_change: dom.get_node(target).is_some(),
            }
        }
        "set_text_content" => {
            let Some(target) = node(arg1) else {
                return RenderMutationImpact::default();
            };
            let changed = dom
                .with_node(target, |node| match &node.data {
                    NodeData::Text { contents } | NodeData::Comment { contents } => {
                        contents.as_str() != arg2
                    }
                    NodeData::ProcessingInstruction { data, .. } => data.as_str() != arg2,
                    // Element/DocumentFragment textContent replaces their
                    // child structure, which can change style even when the
                    // flattened text is equal (for example `<b>x</b>` -> `x`).
                    _ => {
                        let children = dom.children(target);
                        match children.as_slice() {
                            [] => !arg2.is_empty(),
                            [child] => dom
                                .with_node(*child, |child| match &child.data {
                                    NodeData::Text { contents } => contents.as_str() != arg2,
                                    _ => true,
                                })
                                .unwrap_or(true),
                            _ => true,
                        }
                    }
                })
                .unwrap_or(false);
            RenderMutationImpact {
                connected: node_is_connected(dom, target),
                actual_change: changed,
            }
        }
        _ => RenderMutationImpact::default(),
    }
}

#[cfg(feature = "render")]
fn retained_style_mutation(
    dom: &DomTree,
    cmd: &str,
    arg1: &str,
    arg2: &str,
) -> Option<obscura_render::RetainedStyleMutation> {
    let node = NodeId::new(arg1.parse::<u32>().ok()?);
    // The retained planner and document stylesheet cache are intentionally
    // light-tree scoped. A mutation inside a connected shadow tree must still
    // invalidate rendering, but cannot be represented by that document-local
    // dirty set until scoped stylesheet invalidation is retained separately.
    if dom.containing_shadow_root(node).is_some() {
        return None;
    }
    match cmd {
        "set_attribute" => {
            let (name, value) = arg2.split_once('\0')?;
            if obscura_render::dom::retained_attribute_mutation_kind(dom, node, name)
                == obscura_render::dom::RetainedAttributeMutationKind::Full
            {
                return None;
            }
            let keeps_selector_value = !name.eq_ignore_ascii_case("style");
            Some(obscura_render::AttributeStyleMutation {
                node,
                name: name.to_string(),
                old_value: keeps_selector_value
                    .then(|| {
                        dom.with_node(node, |node| {
                            node.get_attribute(name).map(str::to_owned)
                        })
                        .flatten()
                    })
                    .flatten(),
                new_value: keeps_selector_value.then(|| value.to_string()),
            }
            .into())
        }
        "remove_attribute" => {
            if obscura_render::dom::retained_attribute_mutation_kind(dom, node, arg2)
                == obscura_render::dom::RetainedAttributeMutationKind::Full
            {
                return None;
            }
            let keeps_selector_value = !arg2.eq_ignore_ascii_case("style");
            Some(obscura_render::AttributeStyleMutation {
                node,
                name: arg2.to_string(),
                old_value: keeps_selector_value
                    .then(|| {
                        dom.with_node(node, |node| {
                            node.get_attribute(arg2).map(str::to_owned)
                        })
                        .flatten()
                    })
                    .flatten(),
                new_value: None,
            }
            .into())
        }
        "append_child" => {
            let child = NodeId::new(arg2.parse::<u32>().ok()?);
            dom.get_node(node)?;
            let old_parent = dom.get_node(child)?.parent;
            Some(
                obscura_render::TreeStyleMutation::Insert {
                    node: child,
                    old_parent,
                    new_parent: node,
                }
                .into(),
            )
        }
        "remove_child" => {
            let old_parent = dom.get_node(node)?.parent?;
            Some(
                obscura_render::TreeStyleMutation::Remove { node, old_parent }.into(),
            )
        }
        "insert_before" => {
            let reference = NodeId::new(arg2.parse::<u32>().ok()?);
            let new_parent = dom.get_node(reference)?.parent?;
            if dom.containing_shadow_root(new_parent).is_some() {
                return None;
            }
            let old_parent = dom.get_node(node)?.parent;
            Some(
                obscura_render::TreeStyleMutation::Insert {
                    node,
                    old_parent,
                    new_parent,
                }
                .into(),
            )
        }
        "set_text_content" => match &dom.get_node(node)?.data {
            NodeData::Text { .. } => Some(
                obscura_render::TreeStyleMutation::Text {
                    node,
                    parent: dom.get_node(node)?.parent,
                }
                .into(),
            ),
            // Element/fragment textContent replaces a child list. That can
            // flip :empty and structural/relational selectors, so the local
            // text fast path cannot describe the mutation safely.
            _ => None,
        },
        _ => None,
    }
}

#[cfg(feature = "render")]
// Modern hydration can touch thousands of distinct connected nodes before the
// first rendering opportunity. Keep a bounded safety valve for adversarial
// churn, but do not force a whole-document cascade at the scale of an ordinary
// React/Framer commit.
const MAX_PENDING_STYLE_MUTATIONS: usize = 4_096;

/// Queue one retained-style invalidation without letting animation frameworks
/// evict the whole prepared render merely because they rewrite the same inline
/// style more than once before the next rendering opportunity.
///
/// Rendering observes the attribute state at flush boundaries. Repeated writes
/// to the same node/name therefore retain the first old value and final new
/// value; intermediate values were never rendered and cannot affect selector
/// matching. Inline style uses the same rule without storing serialized values.
#[cfg(feature = "render")]
pub(crate) fn queue_retained_style_mutation(
    pending: &mut Vec<obscura_render::RetainedStyleMutation>,
    mutation: obscura_render::RetainedStyleMutation,
) -> bool {
    let is_resource = matches!(mutation, obscura_render::RetainedStyleMutation::Resource);
    let has_resource = pending
        .iter()
        .any(|queued| matches!(queued, obscura_render::RetainedStyleMutation::Resource));
    if is_resource && has_resource {
        return true;
    }
    if let obscura_render::RetainedStyleMutation::Animation { node } = &mutation {
        if pending.iter().any(|queued| {
            matches!(
                queued,
                obscura_render::RetainedStyleMutation::Animation { node: current }
                    if current == node
            )
        }) {
            return true;
        }
    }
    if let obscura_render::RetainedStyleMutation::WaapiAnimation { node } = &mutation {
        if pending.iter().any(|queued| {
            matches!(
                queued,
                obscura_render::RetainedStyleMutation::WaapiAnimation { node: current }
                    if current == node
            )
        }) {
            return true;
        }
    }
    if let obscura_render::RetainedStyleMutation::Attribute(next) = &mutation {
        if let Some(obscura_render::RetainedStyleMutation::Attribute(current)) =
            pending.iter_mut().find(|queued| {
                matches!(
                    queued,
                    obscura_render::RetainedStyleMutation::Attribute(current)
                        if current.node == next.node
                            && current.name.eq_ignore_ascii_case(&next.name)
                )
            })
        {
            current.new_value.clone_from(&next.new_value);
            return true;
        }
    }

    // Resource refresh is a singleton trigger, not style damage. Keep the
    // bounded safety limit on actual selector/tree/animation invalidations
    // without making a late image discard an exactly-full retained batch.
    let style_damage_len = pending.len() - usize::from(has_resource);
    if !is_resource && style_damage_len >= MAX_PENDING_STYLE_MUTATIONS {
        return false;
    }
    pending.push(mutation);
    true
}

/// Page-wide limit on concurrent background render-resource requests: the
/// bound the navigation warmup stream always had, now shared by every load
/// of the document however many scans or layout misses queue them.
#[cfg(feature = "render")]
pub const RENDER_RESOURCE_CONCURRENCY: usize = 16;

/// One finished page-transport load for the renderer cache.
#[cfg(feature = "render")]
#[derive(Debug)]
pub struct RenderResourceLoad {
    /// `document_generation` the request was made for.
    pub generation: u64,
    pub url: String,
    pub profile: Option<ImageRequestProfile>,
    pub is_font: bool,
    /// Final URL, status, headers and body of the response; `None` when the
    /// request failed or was blocked.
    pub response: Option<RenderResourceResponse>,
}

#[cfg(feature = "render")]
#[derive(Debug)]
pub struct RenderResourceResponse {
    pub url: String,
    pub status: u16,
    pub headers: std::collections::HashMap<String, String>,
    pub raw_headers: Option<obscura_net::HeaderCapture>,
    pub request_raw_headers: Option<obscura_net::HeaderCapture>,
    pub body: Arc<[u8]>,
}

/// A transport response the runtime applied; the page turns it into the
/// Network events a client expects for a subresource.
#[cfg(feature = "render")]
#[derive(Debug)]
pub struct RenderResourceEvent {
    pub is_font: bool,
    pub response: RenderResourceResponse,
}

/// Whether this runtime has an asynchronous transport, standalone or Page-owned.
#[cfg(feature = "render")]
pub(crate) fn has_transport(state: &ObscuraState) -> bool {
    state.http_client.is_some() || state.stealth_client.is_some()
}

/// Build the renderer resource cache for this runtime. A runtime owned by a
/// page must never let layout or paint invoke a caller-supplied synchronous
/// loader: that loader is outside the page's proxy, cookies, interception and
/// URL-blocking policy. Page runtimes start cache-only and are fed through the
/// page transport. The renderer's default cache is also cache-only and never
/// opens a network connection.
#[cfg(feature = "render")]
pub(crate) fn fresh_render_resources(state: &ObscuraState) -> obscura_render::RenderResourceCache {
    let mut cache = obscura_render::RenderResourceCache::default();
    if has_transport(state) {
        cache.set_sync_loading_enabled(false);
    }
    cache
}

/// Rebuild resource-dependent geometry while retaining the previous computed
/// style graph. Image intrinsic sizes and font metrics can reflow the whole
/// document, but neither changes selector matching or computed declarations.
/// Coalescing this marker also makes one shared image response invalidate once
/// rather than once for every HTMLImageElement waiter.
#[cfg(feature = "render")]
pub(crate) fn invalidate_render_resource_geometry(state: &mut ObscuraState) {
    if state.prepared_render.is_some()
        && !queue_retained_style_mutation(
            &mut state.pending_style_mutations,
            obscura_render::RetainedStyleMutation::Resource,
        )
    {
        state.prepared_render = None;
        state.pending_style_mutations.clear();
    }
    state.resolved_scroll = None;
}

#[cfg(feature = "render")]
fn render_timing_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("OBSCURA_RENDER_TIMING").is_some())
}

#[cfg(feature = "render")]
fn is_render_mutation_command(cmd: &str) -> bool {
    matches!(
        cmd,
        "set_attribute"
            | "set_form_value"
            | "set_form_checked"
            | "set_form_indeterminate"
            | "remove_attribute"
            | "set_attribute_ns"
            | "remove_attribute_ns"
            | "append_child"
            | "remove_child"
            | "insert_before"
            | "set_inner_html"
            | "set_inner_html_context"
            | "set_text_content"
    )
}

fn fragment_context_and_html(arg: &str) -> (html5ever::QualName, &str) {
    let mut parts = arg.splitn(3, '\0');
    let first = parts.next().unwrap_or("body");
    let second = parts.next();
    let third = parts.next();
    let (namespace, qualified, html) = match (second, third) {
        // Namespace-aware encoding used by the current bootstrap.
        (Some(qualified), Some(html)) => (first, qualified, html),
        // Backward-compatible encoding for older snapshots: `local\0html`.
        (Some(html), None) => ("http://www.w3.org/1999/xhtml", first, html),
        (None, None) => ("http://www.w3.org/1999/xhtml", "body", first),
        (None, Some(_)) => unreachable!(),
    };
    let (prefix, local) = match qualified.split_once(':') {
        Some((prefix, local)) if !prefix.is_empty() && !local.is_empty() => {
            (Some(html5ever::Prefix::from(prefix)), local)
        }
        _ => (None, if qualified.is_empty() { "body" } else { qualified }),
    };
    (
        html5ever::QualName::new(
            prefix,
            html5ever::Namespace::from(namespace),
            html5ever::LocalName::from(local),
        ),
        html,
    )
}

#[op2(fast)]
fn op_script_mark_started(state: &OpState, nid: u32) -> bool {
    let shared = state.borrow::<SharedState>().clone();
    let state = shared.borrow();
    let Some(dom) = state.dom.as_ref() else {
        return false;
    };
    let node_id = NodeId::new(nid);
    if !node_is_script(dom, node_id) {
        return false;
    }
    state.already_started_scripts.borrow_mut().insert(node_id);
    true
}

/// Atomically claim an executable script.  A false result means the node was
/// created inert by an HTML-string API or has already been prepared once.
#[op2(fast)]
fn op_script_try_start(state: &OpState, nid: u32) -> bool {
    let shared = state.borrow::<SharedState>().clone();
    let state = shared.borrow();
    let Some(dom) = state.dom.as_ref() else {
        return false;
    };
    let node_id = NodeId::new(nid);
    if !node_is_script(dom, node_id) {
        return false;
    }
    let newly_started = state.already_started_scripts.borrow_mut().insert(node_id);
    newly_started
}

/// Attach one native shadow-tree scope without making it part of the light
/// tree. Layout intentionally remains unaware of the detached root until
/// scoped style, slot assignment, and composed-tree paint are implemented.
#[op2(fast)]
fn op_shadow_attach(state: &OpState, host_nid: u32, #[string] mode: String) -> i32 {
    let mode = match mode.as_str() {
        "open" => ShadowRootMode::Open,
        "closed" => ShadowRootMode::Closed,
        _ => return -1,
    };
    let shared = state.borrow::<SharedState>().clone();
    let state = shared.borrow();
    let Some(dom) = state.dom.as_ref() else {
        return -1;
    };
    match dom.attach_shadow_root(NodeId::new(host_nid), mode) {
        Ok(root) => root.raw() as i32,
        Err(AttachShadowError::HostAlreadyHasShadowRoot) => -2,
        Err(_) => -1,
    }
}

/// Return native host-owned shadow identity as `root-id\0mode`. Closed roots
/// are included here; the Web-facing `Element.shadowRoot` getter applies mode
/// visibility in bootstrap.js.
#[op2]
#[string]
fn op_shadow_root_info(state: &OpState, host_nid: u32) -> String {
    let shared = state.borrow::<SharedState>().clone();
    let state = shared.borrow();
    let Some(dom) = state.dom.as_ref() else {
        return String::new();
    };
    dom.shadow_root(NodeId::new(host_nid))
        .and_then(|root| dom.shadow_root_info(root))
        .map(|shadow| {
            let mode = match shadow.mode {
                ShadowRootMode::Open => "open",
                ShadowRootMode::Closed => "closed",
            };
            format!("{}\0{mode}", shadow.id.raw())
        })
        .unwrap_or_default()
}

#[op2]
#[string]
fn op_dom(
    state: &OpState,
    #[string] cmd: String,
    #[string] arg1: String,
    #[string] arg2: String,
    frame_id: u32,
) -> String {
    let Some(shared) = frame_state(state, frame_id) else { return "null".into(); };
    // Anti-panic boundary: a panic in a DOM op would unwind through deno_core
    // into V8's FFI frame, where V8_Fatal calls abort(3) and takes the whole
    // engine (and every CDP client) down. Catch it so one malformed selector or
    // inconsistent tree node degrades to a null result for that single call.
    // No per-call clone: on the happy path this is just a landing pad, so the
    // hot DOM path (querySelector/getAttribute/...) pays nothing measurable.
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        op_dom_inner(shared, cmd, arg1, arg2)
    }))
    .unwrap_or_else(|_| {
        tracing::error!("op_dom panicked; returning null");
        "null".to_string()
    })
}

/// User input changes selector matching without changing content attributes.
pub(crate) fn invalidate_input_render(state: &mut ObscuraState) {
    state.activity_generation = state.activity_generation.wrapping_add(1);
    #[cfg(feature = "render")]
    {
        state.prepared_render = None;
        state.pending_style_mutations.clear();
        state.resolved_scroll = None;
    }
}

pub(crate) fn input_focusable(state: &mut ObscuraState, id: NodeId) -> bool {
    if !state.dom.as_ref().is_some_and(|dom| dom.can_focus(id)) {
        return false;
    }
    #[cfg(feature = "render")]
    {
        let loading = state.render_resources.set_sync_loading_enabled(false);
        let ready = ensure_resolved_scroll(state).is_some();
        state.render_resources.set_sync_loading_enabled(loading);
        if !ready {
            return false;
        }
        let dom = state.dom.as_ref().unwrap();
        let layout = state.prepared_render.as_ref().unwrap().layout();
        if !layout.rects.contains_key(&id) {
            return false;
        }
        for node in std::iter::once(id).chain(dom.ancestors(id)) {
            if let Some(hidden) = layout
                .styles
                .get(&node)
                .and_then(|style| style.visibility_hidden)
            {
                return !hidden;
            }
        }
    }
    true
}

fn focus_dom_op(shared: &SharedState, cmd: &str, arg1: &str, arg2: &str) -> String {
    let mut state = shared.borrow_mut();
    let Some(dom) = state.dom.as_ref() else {
        return "null".into();
    };
    let previous = dom.input_state();
    if cmd == "focus_state" {
        return serde_json::json!([
            previous.focused.map(|id| id.raw() as i64).unwrap_or(-1),
            previous.focus_generation
        ])
        .to_string();
    }
    let node = arg1.parse::<u32>().ok().map(NodeId::new);
    if cmd == "focusable" {
        return node
            .is_some_and(|id| input_focusable(&mut state, id))
            .to_string();
    }
    let valid_target = arg1 == "-1" || node.is_some_and(|id| input_focusable(&mut state, id));
    let next = if valid_target {
        arg2.parse::<u64>()
            .ok()
            .and_then(|generation| state.dom.as_ref().unwrap().set_focused(node, generation))
    } else {
        None
    };
    let applied = next.is_some();
    let current = next.unwrap_or_else(|| state.dom.as_ref().unwrap().input_state());
    if previous != current {
        invalidate_input_render(&mut state);
    }
    serde_json::json!([
        applied,
        current.focused.map(|id| id.raw() as i64).unwrap_or(-1),
        current.focus_generation
    ])
    .to_string()
}

fn text_dom_op(shared: &SharedState, cmd: &str, arg1: &str, arg2: &str) -> String {
    let mut state = shared.borrow_mut();
    let Some(dom) = state.dom.as_ref() else {
        return "null".into();
    };
    let Ok(id) = arg1.parse::<u32>().map(NodeId::new) else {
        return "null".into();
    };
    if cmd == "text_take_change" {
        return dom.take_text_change(id).to_string();
    }
    let value = match cmd {
        "text_state" => dom.text_control(id),
        "text_value_set" => dom.set_text_value(id, arg2),
        "text_reset" => dom.reset_text_control(id),
        "text_selection_set" => serde_json::from_str::<(u32, u32, String)>(arg2)
            .ok()
            .and_then(|(start, end, direction)| {
                if !dom.text_control_kind(id)?.supports_selection() {
                    return None;
                }
                dom.set_text_selection(id, start, end, &direction)
            }),
        _ => None,
    };
    let connected = dom.is_connected(id);
    let Some(value) = value else {
        return "null".into();
    };
    if cmd != "text_state" && connected {
        invalidate_input_render(&mut state);
    }
    serde_json::json!({"value":value.value,"default_value":value.default_value,"selection":[value.start,value.end,value.direction],
        "selection_supported":value.kind.supports_selection(),"generation":value.generation}).to_string()
}

fn checked_dom_op(shared: &SharedState, cmd: &str, arg1: &str, arg2: &str) -> String {
    let mut state = shared.borrow_mut();
    let Some(dom) = state.dom.as_ref() else {
        return "null".into();
    };
    let Ok(id) = arg1.parse::<u32>().map(NodeId::new) else {
        return "null".into();
    };
    if cmd == "label_forwarding" {
        return dom.label_forwarding(id).to_string();
    }
    if cmd == "form_owner" {
        return serde_json::json!(dom.form_owner(id).map(NodeId::raw)).to_string();
    }
    if cmd == "form_controls" {
        return serde_json::json!(dom
            .form_controls(id)
            .into_iter()
            .map(NodeId::raw)
            .collect::<Vec<_>>())
        .to_string();
    }
    let value = match cmd {
        "checked_state" => dom.checked_state(id),
        "checked_set" => arg2
            .parse::<bool>()
            .ok()
            .and_then(|value| dom.set_checked(id, value)),
        "indeterminate_set" => arg2
            .parse::<bool>()
            .ok()
            .and_then(|value| dom.set_indeterminate(id, value)),
        "checked_reset" => dom.reset_checked(id),
        _ => None,
    };
    let Some(value) = value else {
        return "null".into();
    };
    let kind = dom.input_type(id);
    let connected = dom.is_connected(id);
    if cmd != "checked_state" && connected {
        invalidate_input_render(&mut state);
    }
    serde_json::json!({"checked":value.checked,"default_checked":value.default_checked,"dirty":value.dirty,
        "indeterminate":value.indeterminate,"type":kind}).to_string()
}

/// HTML form serialization normalizes lone CR/LF before percent encoding.
pub fn encode_form_text(entries: &[(String, String)]) -> String {
    fn crlf(value: &str) -> String {
        value.replace("\r\n", "\n").replace('\r', "\n").replace('\n', "\r\n")
    }
    let mut encoded = url::form_urlencoded::Serializer::new(String::new());
    for (name, value) in entries {
        encoded.append_pair(&crlf(name), &crlf(value));
    }
    encoded.finish()
}

/// Plan from current native DOM facts; callers commit only a complete request.
fn form_navigation(
    state: &ObscuraState,
    form: NodeId,
    submitter: Option<NodeId>,
    entries: &[(String, String)],
) -> Result<Option<PendingNavigation>, &'static str> {
    let dom = state.dom.as_ref().ok_or("FORM_INVALID")?;
    if !dom.is_html_element(form, "form") {
        return Err("FORM_INVALID");
    }
    if !dom.is_connected(form) {
        return Ok(None);
    }
    let form_node = dom.get_node(form).ok_or("FORM_INVALID")?;
    let button = submitter.and_then(|id| dom.get_node(id));
    let attr = |name, override_name| {
        button
            .as_ref()
            .and_then(|node| node.get_attribute(override_name))
            .or_else(|| form_node.get_attribute(name))
            .unwrap_or("")
    };
    let method = attr("method", "formmethod");
    if method.eq_ignore_ascii_case("dialog") {
        return Err("INPUT_ELEMENT_UNSUPPORTED");
    }
    let post = method.eq_ignore_ascii_case("post");
    let enctype = attr("enctype", "formenctype");
    if post
        && ["multipart/form-data", "text/plain"]
            .iter()
            .any(|value| enctype.eq_ignore_ascii_case(value))
    {
        return Err("INPUT_ELEMENT_UNSUPPORTED");
    }
    if form_node
        .get_attribute("accept-charset")
        .is_some_and(|value| !value.is_empty() && !value.eq_ignore_ascii_case("utf-8"))
    {
        return Err("INPUT_ELEMENT_UNSUPPORTED");
    }
    let base_target = dom
        .query_selector("base[target]")
        .ok()
        .flatten()
        .and_then(|id| dom.get_node(id))
        .and_then(|node| node.get_attribute("target").map(str::to_string));
    let target = button
        .as_ref()
        .and_then(|node| node.get_attribute("formtarget"))
        .or_else(|| form_node.get_attribute("target"))
        .or(base_target.as_deref())
        .unwrap_or("");
    if !(target.is_empty()
        || target.eq_ignore_ascii_case("_self")
        || (state.frame_id == 0
            && (target.eq_ignore_ascii_case("_top") || target.eq_ignore_ascii_case("_parent"))))
    {
        return Err("INPUT_ELEMENT_UNSUPPORTED");
    }
    let source = dom
        .document_url()
        .and_then(|value| url::Url::parse(&value).ok())
        .ok_or("INPUT_ELEMENT_UNSUPPORTED")?;
    let action = attr("action", "formaction");
    let mut url = if action.is_empty() {
        source.clone()
    } else {
        let base = document_base_url(state).ok_or("INPUT_ELEMENT_UNSUPPORTED")?;
        url::Url::parse(&base)
            .and_then(|base| base.join(action))
            .map_err(|_| "INPUT_ELEMENT_UNSUPPORTED")?
    };
    if !matches!(url.scheme(), "http" | "https") || url.fragment().is_some() {
        return Err("INPUT_ELEMENT_UNSUPPORTED");
    }
    let encoded = encode_form_text(entries);
    let mut request = ResourceRequest::navigation();
    request.referrer_policy = state.referrer_policy;
    if form_node.get_attribute("rel").is_some_and(|value| {
        value
            .split_ascii_whitespace()
            .any(|token| token.eq_ignore_ascii_case("noreferrer"))
    }) {
        request.referrer_policy = ReferrerPolicy::NoReferrer;
    }
    request.referrer = Some(source.clone());
    request.initiator = Some(source);
    if !post {
        url.set_query(Some(&encoded));
    }
    Ok(Some(PendingNavigation {
        history: HistoryNavigation::Push,
        url: url.to_string(),
        method: if post { "POST" } else { "GET" }.into(),
        body: if post { encoded } else { String::new() },
        request,
    }))
}

/// Check supported native submission without dispatching events or planning navigation.
pub(crate) fn preflight_form_submission(
    state: &ObscuraState,
    form: NodeId,
    button: NodeId,
) -> Result<(), &'static str> {
    let dom = state.dom.as_ref().ok_or("NO_DOCUMENT")?;
    let form_node = dom.get_node(form).ok_or("INPUT_TARGET_CHANGED")?;
    let submitter = dom.get_node(button).ok_or("INPUT_TARGET_CHANGED")?;
    if form_node.get_attribute("novalidate").is_none()
        && submitter.get_attribute("formnovalidate").is_none()
    {
        for control in dom.form_controls(form) {
            control_validity(state, control)?;
        }
    }
    let entries = dom.form_text_entries(form, Some(button))?;
    form_navigation(state, form, Some(button), &entries)?;
    Ok(())
}

/// Return native facts; ECMAScript pattern matching stays in the protected V8 closure.
fn control_validity(state: &ObscuraState, id: NodeId) -> Result<serde_json::Value, &'static str> {
    let dom = state.dom.as_ref().ok_or("FORM_INVALID")?;
    let node = dom.get_node(id).ok_or("FORM_INVALID")?;
    let input = dom.is_html_element(id, "input");
    let text = dom.text_control(id);
    let kind = dom.input_type(id).unwrap_or_default();
    let button = dom.is_html_element(id, "button");
    let readonly = text.is_some() && node.get_attribute("readonly").is_some();
    let candidate = (input || text.is_some() || button || dom.is_html_element(id, "select"))
        && !dom.is_disabled(id)
        && !readonly
        && !dom
            .ancestors(id)
            .iter()
            .any(|id| dom.is_html_element(*id, "datalist"))
        && !(input && matches!(kind.as_str(), "hidden" | "reset" | "button"))
        && !(button && !dom.is_submit_button(id));
    let supported = text.is_some()
        || button
        || !input && !dom.is_html_element(id, "select")
        || matches!(
            kind.as_str(),
            "checkbox" | "radio" | "hidden" | "reset" | "button" | "submit" | "image"
        );
    if candidate && !supported {
        return Err("INPUT_ELEMENT_UNSUPPORTED");
    }
    let custom = dom.custom_validity(id);
    let required = node.get_attribute("required").is_some();
    let mut missing = false;
    let mut too_long = false;
    let mut too_short = false;
    let mut value = String::new();
    let mut pattern = None;
    let mut multiple = false;
    if let Some(text) = text {
        value = text.value;
        missing = required && value.is_empty() && !readonly;
        let length = value.encode_utf16().count();
        let bound = |attribute| node.get_attribute(attribute).and_then(|value| {
            let value = value.trim_start_matches([' ', '\t', '\n', '\r', '\u{000c}']);
            let value = value.strip_prefix('+').unwrap_or(value);
            let digits: String = value.chars().take_while(char::is_ascii_digit).collect();
            digits.parse::<usize>().ok()
        });
        if text.dirty && text.last_user_edit {
            too_long = bound("maxlength").is_some_and(|max| length > max);
            too_short = !value.is_empty() && bound("minlength").is_some_and(|min| length < min);
        }
        if input {
            pattern = node.get_attribute("pattern").map(str::to_string);
        }
        multiple = node.get_attribute("multiple").is_some();
    } else if kind == "checkbox" {
        missing = required && !dom.checked_state(id).is_some_and(|state| state.checked);
    } else if kind == "radio" {
        missing = dom.radio_value_missing(id);
    }
    Ok(
        serde_json::json!({"candidate":candidate,"valueMissing":missing,"tooLong":too_long,
        "tooShort":too_short,"customError":!custom.is_empty(),"customMessage":custom,
        "value":value,"kind":kind,"multiple":multiple,"pattern":pattern,
        "urlValid":kind != "url" || value.is_empty() || url::Url::parse(&value).is_ok()}),
    )
}

fn op_dom_inner(shared: SharedState, cmd: String, arg1: String, arg2: String) -> String {
    if cmd == "web_storage" {
        let state = shared.borrow();
        let mut origin = url::Url::parse(&state.url).ok().map(|u| u.origin().ascii_serialization())
            .unwrap_or_else(|| "null".into());
        let trace = state.runtime_events_enabled && state.diagnostic_events_enabled;
        let storage = if origin == "null" {
            origin = format!("{arg1}:null");
            &state.opaque_storage
        } else if arg1 == "local" { &state.local_storage } else { &state.session_storage };
        let args: Vec<String> = serde_json::from_str(&arg2).unwrap_or_default();
        let Some(operation) = args.first().map(String::as_str) else { return "null".into(); };
        let mut storage = storage.lock().unwrap_or_else(|e| e.into_inner());
        let event_origin = origin.clone();
        let entries = storage.entry(origin).or_default();
        let key = args.get(1).map(String::as_str).unwrap_or("");
        let index = entries.iter().position(|(k, _)| k == key);
        let mut mutation: Option<(&str, Option<String>, Option<String>, Option<String>)> = None;
        let result = match operation {
            "get" => index.map(|i| serde_json::json!(entries[i].1)).unwrap_or(serde_json::Value::Null),
            "keys" => serde_json::json!(entries.iter().map(|(k, _)| k).collect::<Vec<_>>()),
            "clear" => {
                if trace && !entries.is_empty() { mutation = Some(("clear", None, None, None)); }
                entries.clear(); serde_json::Value::Null
            }
            "remove" => {
                if let Some(i) = index {
                    let old = entries.remove(i).1;
                    if trace { mutation = Some(("remove", Some(key.to_owned()), Some(old), None)); }
                }
                serde_json::Value::Null
            }
            "set" => {
                let value = args.get(2).cloned().unwrap_or_default();
                let size: usize = entries.iter().filter(|(k, _)| k != key)
                    .map(|(k, v)| k.encode_utf16().count() + v.encode_utf16().count()).sum();
                if size + key.encode_utf16().count() + value.encode_utf16().count() > 2_621_440 {
                    serde_json::json!({"error":"QuotaExceededError"})
                } else {
                    if trace {
                        let old = index.map(|i| entries[i].1.clone());
                        if old.as_deref() != Some(value.as_str()) {
                            mutation = Some(("set", Some(key.to_owned()), old, Some(value.clone())));
                        }
                    }
                    if let Some(i) = index { entries[i].1 = value; }
                    else { entries.push((key.to_owned(), value)); }
                    serde_json::Value::Null
                }
            }
            _ => serde_json::Value::Null,
        };
        drop(storage);
        drop(state);
        if let Some((operation, key, old, new)) = mutation {
            use sha2::Digest as _;
            let digest = |value: &String| format!("{:x}", sha2::Sha256::digest(value.as_bytes()));
            let event = RuntimeStorageEvent {
                origin: event_origin, area: arg1, operation: operation.to_string(), key,
                old_bytes: old.as_ref().map(String::len), new_bytes: new.as_ref().map(String::len),
                old_sha256: old.as_ref().map(digest), new_sha256: new.as_ref().map(digest),
                timestamp: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default().as_secs_f64() * 1_000.0,
            };
            let mut state = shared.borrow_mut();
            if state.pending_runtime_events.len() >= 1_024 { state.pending_runtime_events.pop_front(); }
            state.pending_runtime_events.push_back(RuntimeEvent::Storage(event));
        }
        return result.to_string();
    }
    if matches!(
        cmd.as_str(),
        "control_validity" | "custom_validity_set" | "validation_controls" | "form_no_validate"
    ) {
        let state = shared.borrow();
        let result = (|| {
            let dom = state.dom.as_ref().ok_or("FORM_INVALID")?;
            let id = arg1
                .parse::<u32>()
                .map(NodeId::new)
                .map_err(|_| "FORM_INVALID")?;
            let node = dom.get_node(id).ok_or("FORM_INVALID")?;
            if cmd == "custom_validity_set" {
                dom.set_custom_validity(id, &arg2);
                return Ok(serde_json::Value::Null);
            }
            if cmd == "validation_controls" {
                return Ok(serde_json::json!(if dom.is_html_element(id, "form") {
                    dom.form_controls(id)
                        .into_iter()
                        .map(NodeId::raw)
                        .collect::<Vec<_>>()
                } else {
                    vec![id.raw()]
                }));
            }
            if cmd == "form_no_validate" {
                let button = arg2
                    .parse::<u32>()
                    .ok()
                    .map(NodeId::new)
                    .and_then(|id| dom.get_node(id));
                return Ok(serde_json::json!(
                    node.get_attribute("novalidate").is_some()
                        || button
                            .is_some_and(|node| node.get_attribute("formnovalidate").is_some())
                ));
            }
            control_validity(&state, id)
        })();
        return result
            .unwrap_or_else(|error| serde_json::json!({"error":error}))
            .to_string();
    }

    if cmd == "form_submit_begin" || cmd == "form_submit_end" {
        let state = shared.borrow();
        let result = (|| {
            let dom = state.dom.as_ref().ok_or("FORM_INVALID")?;
            let form = arg1
                .parse::<u32>()
                .map(NodeId::new)
                .map_err(|_| "FORM_INVALID")?;
            if cmd == "form_submit_end" {
                dom.end_form_submission(form);
                return Ok(false);
            }
            let submitter = if arg2.is_empty() {
                None
            } else {
                Some(
                    arg2.parse::<u32>()
                        .map(NodeId::new)
                        .map_err(|_| "FORM_SUBMITTER_TYPE")?,
                )
            };
            dom.begin_form_submission(form, submitter)
        })();
        return match result {
            Ok(started) => serde_json::json!({"started": started}).to_string(),
            Err(error) => serde_json::json!({"error": error}).to_string(),
        };
    }

    if matches!(
        cmd.as_str(),
        "form_entries_begin" | "form_entries_end" | "form_entries_active"
    ) {
        let state = shared.borrow();
        let result = (|| {
            let dom = state.dom.as_ref().ok_or("FORM_INVALID")?;
            let form = arg1
                .parse::<u32>()
                .map(NodeId::new)
                .map_err(|_| "FORM_INVALID")?;
            if cmd == "form_entries_active" {
                return Ok(serde_json::json!(dom.constructing_form_entries(form)));
            }
            if cmd == "form_entries_end" {
                dom.end_form_entries(form);
                return Ok(serde_json::Value::Null);
            }
            let submitter = if arg2.is_empty() {
                None
            } else {
                Some(
                    arg2.parse::<u32>()
                        .map(NodeId::new)
                        .map_err(|_| "FORM_SUBMITTER_TYPE")?,
                )
            };
            dom.begin_form_entries(form, submitter)
                .map(|entries| serde_json::json!(entries))
        })();
        return result
            .unwrap_or_else(|error| serde_json::json!({"error":error}))
            .to_string();
    }

    if cmd == "form_navigate" {
        let mut state = shared.borrow_mut();
        let result = (|| {
            let form = arg1
                .parse::<u32>()
                .map(NodeId::new)
                .map_err(|_| "FORM_INVALID")?;
            let (submitter, entries): (Option<u32>, Vec<(String, String)>) =
                serde_json::from_str(&arg2).map_err(|_| "FORM_ENTRIES_INVALID")?;
            form_navigation(&state, form, submitter.map(NodeId::new), &entries)
        })();
        return match result {
            Ok(navigation) => {
                if let Some(navigation) = navigation {
                    state.url = navigation.url.clone();
                    state.pending_navigation = Some(navigation);
                    state.same_document_navigation = false;
                }
                "{}".into()
            }
            Err(error) => serde_json::json!({"error": error}).to_string(),
        };
    }

    if cmd == "device_identity" {
        return serde_json::to_string(&shared.borrow().device_identity).unwrap();
    }
    if matches!(cmd.as_str(), "location_url_get" | "location_url_resolve") {
        let state = shared.borrow();
        let current = state
            .dom
            .as_ref()
            .and_then(DomTree::document_url)
            .unwrap_or_else(|| "about:blank".into());
        let Ok(mut url) = url::Url::parse(&current) else {
            return serde_json::json!({"error":"SyntaxError"}).to_string();
        };
        if cmd == "location_url_get" {
            return url_components(&url).to_string();
        }
        let before = url.clone();
        match arg1.as_str() {
            "href" => {
                let parsed = document_base_url(&state)
                    .and_then(|base| url::Url::parse(&base).ok())
                    .and_then(|base| base.join(&arg2).ok());
                let Some(next) = parsed else {
                    return serde_json::json!({"error":"SyntaxError"}).to_string();
                };
                url = next;
            }
            "hash" => {
                url.set_fragment(Some(arg2.strip_prefix('#').unwrap_or(&arg2)));
                if url.fragment().unwrap_or("") == before.fragment().unwrap_or("") {
                    return serde_json::json!({"noop":true}).to_string();
                }
            }
            "protocol" => {
                // The URL crate reports ignored scheme transitions as Err too.
                // Location throws only for invalid syntax, then navigates the
                // URL that the setter actually produced.
                let scheme: String = arg2
                    .split(':')
                    .next()
                    .unwrap_or("")
                    .chars()
                    .filter(|ch| !matches!(ch, '\t' | '\n' | '\r'))
                    .collect();
                let mut bytes = scheme.bytes();
                if !bytes.next().is_some_and(|ch| ch.is_ascii_alphabetic())
                    || !bytes
                        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'+' | b'-' | b'.'))
                {
                    return serde_json::json!({"error":"SyntaxError"}).to_string();
                }
                let _ = url::quirks::set_protocol(&mut url, &scheme);
                if !matches!(url.scheme(), "http" | "https") {
                    return serde_json::json!({"noop":true}).to_string();
                }
            }
            "host" | "hostname" | "pathname" if url.cannot_be_a_base() => {
                return serde_json::json!({"noop":true}).to_string();
            }
            "host" => {
                let _ = url::quirks::set_host(&mut url, &arg2);
            }
            "hostname" => {
                let _ = url::quirks::set_hostname(&mut url, &arg2);
            }
            "port" => {
                if url.host_str().is_none_or(str::is_empty) || url.scheme() == "file" {
                    return serde_json::json!({"noop":true}).to_string();
                }
                let _ = url::quirks::set_port(&mut url, &arg2);
            }
            "pathname" => url::quirks::set_pathname(&mut url, &arg2),
            "search" => url::quirks::set_search(&mut url, &arg2),
            _ => return serde_json::json!({"error":"SyntaxError"}).to_string(),
        }
        return serde_json::json!({"url":url.as_str()}).to_string();
    }

    #[cfg(feature = "render")]
    if cmd == "history_scroll_capture" {
        return serde_json::json!(crate::runtime::capture_history_scroll(&mut shared.borrow_mut())).to_string();
    }
    #[cfg(feature = "render")]
    if cmd == "history_scroll_restore" {
        return match crate::runtime::restore_history_scroll(&mut shared.borrow_mut(), &arg1) {
            Ok(events) => serde_json::json!({"events":events.into_iter().map(|(kind,node)| (kind,node.raw())).collect::<Vec<_>>()}),
            Err(error) => serde_json::json!({"error":error}),
        }.to_string();
    }
    #[cfg(feature = "render")]
    if cmd == "fragment_landing" {
        let mut state = shared.borrow_mut();
        return match crate::runtime::prepare_fragment_landing(&mut state, &arg1, arg2 != "manual") {
            Ok((focus, events)) => serde_json::json!({"focus":focus,"events":events.into_iter()
                .map(|(kind,node)| (kind,node.raw())).collect::<Vec<_>>()})
            .to_string(),
            Err(error) => serde_json::json!({"error":error}).to_string(),
        };
    }
    if cmd == "fragment_url_resolve" {
        let state = shared.borrow();
        let resolved = (|| {
            let mut current = url::Url::parse(&state.dom.as_ref()?.document_url()?).ok()?;
            let next = url::Url::parse(&document_base_url(&state)?)
                .ok()?
                .join(&arg1)
                .ok()?;
            if !matches!(next.scheme(), "http" | "https") || next.fragment().is_none() {
                return None;
            }
            let mut comparison = next.clone();
            current.set_fragment(None);
            comparison.set_fragment(None);
            (current == comparison).then(|| next.to_string())
        })();
        return serde_json::json!(resolved).to_string();
    }

    if cmd == "history_entry" {
        let mut state = shared.borrow_mut();
        let current_url = state
            .dom
            .as_ref()
            .and_then(DomTree::document_url)
            .unwrap_or_else(|| state.url.clone());
        let session = state.session_history.clone();
        let mut history = session.borrow_mut();
        if history.initial {
            history.entries[0].url = current_url.clone();
        }
        let result = (|| -> Result<serde_json::Value, &'static str> {
            match arg1.as_str() {
                "get" => {}
                "scroll_for" => {
                    let id = arg2.parse::<u64>().map_err(|_| "HISTORY_ENTRY_INVALID")?;
                    return Ok(history
                        .entries
                        .iter()
                        .find(|entry| {
                            entry.id == id && entry.document == history.current().document
                        })
                        .map(|entry| serde_json::json!(entry.scroll))
                        .unwrap_or(serde_json::Value::Null));
                }
                "scroll" => {
                    if matches!(arg2.as_str(), "auto" | "manual") {
                        let index = history.index;
                        history.entries[index].scroll = arg2.clone();
                    }
                }
                "push" | "replace" => {
                    #[derive(serde::Deserialize)]
                    struct Update {
                        url: String,
                        data: Vec<u8>,
                        fragment: bool,
                    }
                    let update: Update =
                        serde_json::from_str(&arg2).map_err(|_| "HISTORY_STATE_INVALID")?;
                    let old = url::Url::parse(&current_url).map_err(|_| "SecurityError")?;
                    let next = url::Url::parse(&update.url).map_err(|_| "SecurityError")?;
                    if next.origin() != old.origin()
                        || next.scheme() != old.scheme()
                        || next.username() != old.username()
                        || next.password() != old.password()
                    {
                        return Err("SecurityError");
                    }
                    let mut entry = history.current().clone();
                    entry.id = history.allocate_id();
                    entry.url = next.to_string();
                    entry.data = Some(update.data);
                    entry.position = None;
                    if arg1 == "replace" {
                        let index = history.index;
                        history.entries[index] = entry;
                    } else {
                        #[cfg(feature = "render")]
                        history.save_position(crate::runtime::capture_history_scroll(&mut state));
                        let index = history.index;
                        history.entries.truncate(index + 1);
                        history.entries.push(entry);
                        history.index += 1;
                    }
                    history.initial = false;
                    if let Some(dom) = &state.dom {
                        dom.set_document_url(next.as_str());
                    }
                    state.url = next.to_string();
                    state.same_document_navigation = true;
                    if update.fragment {
                        state.pending_navigation = None;
                    }
                    invalidate_input_render(&mut state);
                }
                "traverse" => {
                    let delta = arg2.parse::<i32>().map_err(|_| "HISTORY_DELTA_INVALID")?;
                    let next = history.index as i64 + i64::from(delta);
                    if next < 0 || next >= history.entries.len() as i64 {
                        return Ok(serde_json::Value::Null);
                    }
                    let entry = history.entries[next as usize].clone();
                    if delta == 0 || entry.document != history.current().document {
                        if entry.post {
                            return Err("HISTORY_POST_REQUIRES_AUTHORIZATION");
                        }
                        state.pending_navigation = Some(PendingNavigation {
                            url: entry.url.clone(),
                            method: "GET".into(),
                            body: String::new(),
                            request: entry.request,
                            history: if delta == 0 {
                                HistoryNavigation::Reload
                            } else {
                                HistoryNavigation::Traverse(entry.id)
                            },
                        });
                        state.url = entry.url;
                        state.same_document_navigation = false;
                        return Ok(serde_json::json!({"cross_document":true}));
                    }
                    #[cfg(feature = "render")]
                    history.save_position(crate::runtime::capture_history_scroll(&mut state));
                    history.index = next as usize;
                    if let Some(dom) = &state.dom {
                        dom.set_document_url(&entry.url);
                    }
                    state.url = entry.url;
                    state.same_document_navigation = true;
                    invalidate_input_render(&mut state);
                }
                _ => return Err("HISTORY_ACTION_INVALID"),
            }
            Ok(history.describe())
        })();
        return result
            .unwrap_or_else(|error| serde_json::json!({"error":error}))
            .to_string();
    }

    if cmd == "history_url_resolve" {
        let state = shared.borrow();
        let current = state
            .dom
            .as_ref()
            .and_then(DomTree::document_url)
            .unwrap_or_else(|| state.url.clone());
        let resolved = (|| {
            let current = url::Url::parse(&current).ok()?;
            let next = if arg1.is_empty() {
                current.clone()
            } else {
                url::Url::parse(&document_base_url(&state)?)
                    .ok()?
                    .join(&arg1)
                    .ok()?
            };
            (next.origin() == current.origin()
                && next.scheme() == current.scheme()
                && next.username() == current.username()
                && next.password() == current.password())
            .then_some(next)
        })();
        let Some(url) = resolved else {
            return "null".into();
        };
        let next = url.to_string();
        return serde_json::json!(next).to_string();
    }

    if matches!(
        cmd.as_str(),
        "form_reset_begin" | "form_reset_apply" | "form_reset_end" | "attribute_value"
    ) {
        let mut state = shared.borrow_mut();
        let Some(dom) = state.dom.as_ref() else {
            return "null".into();
        };
        let Ok(id) = arg1.parse::<u32>().map(NodeId::new) else {
            return "null".into();
        };
        return match cmd.as_str() {
            "attribute_value" => serde_json::json!(dom.attribute_value(id)).to_string(),
            "form_reset_begin" => serde_json::json!(dom.begin_form_reset(id)).to_string(),
            "form_reset_end" => {
                dom.end_form_reset(id);
                "null".into()
            }
            _ => {
                let applied = dom.reset_form_controls(id);
                if applied {
                    invalidate_input_render(&mut state);
                }
                applied.to_string()
            }
        };
    }

    if matches!(
        cmd.as_str(),
        "checked_state"
            | "checked_set"
            | "indeterminate_set"
            | "checked_reset"
            | "form_owner"
            | "form_controls"
            | "label_forwarding"
    ) {
        return checked_dom_op(&shared, &cmd, &arg1, &arg2);
    }

    if matches!(cmd.as_str(), "text_state"|"text_value_set"|"text_selection_set"|"text_reset"|"text_take_change") {
        return text_dom_op(&shared, &cmd, &arg1, &arg2);
    }
    if matches!(cmd.as_str(), "focus_state" | "focusable" | "focus_set") {
        return focus_dom_op(&shared, &cmd, &arg1, &arg2);
    }

    {
        // Scroll offsets belong to a node at its current tree position.
        // Temporary box/style loss keeps that latent state, but DOM removal,
        // reparenting, and subtree replacement reset the affected identities,
        // matching Chromium's lifecycle behavior.
        #[cfg(feature = "render")]
        let reset_nodes = {
            let state = shared.borrow();
            let mut roots = Vec::new();
            if let Some(dom) = state.dom.as_ref() {
                match cmd.as_str() {
                    "remove_child" => {
                        if let Ok(node) = arg1.parse::<u32>() {
                            roots.push(NodeId::new(node));
                        }
                    }
                    "append_child" => {
                        if let Ok(node) = arg2.parse::<u32>() {
                            let node = NodeId::new(node);
                            if dom.get_node(node).and_then(|node| node.parent).is_some() {
                                roots.push(node);
                            }
                        }
                    }
                    "insert_before" => {
                        if let Ok(node) = arg1.parse::<u32>() {
                            let node = NodeId::new(node);
                            if dom.get_node(node).and_then(|node| node.parent).is_some() {
                                roots.push(node);
                            }
                        }
                    }
                    "set_inner_html" | "set_inner_html_context" | "set_text_content" => {
                        if let Ok(node) = arg1.parse::<u32>() {
                            roots.extend(dom.children(NodeId::new(node)));
                        }
                    }
                    _ => {}
                }
                roots
                    .into_iter()
                    .flat_map(|root| {
                        let mut nodes = vec![root];
                        nodes.extend(dom.descendants(root));
                        nodes
                    })
                    .collect::<HashSet<_>>()
            } else {
                HashSet::new()
            }
        };
        // Any changed attribute on a connected node can participate in an
        // author selector. Detached subtree construction, failed operations,
        // and no-op value assignments cannot change live layout and preserve
        // the prepared render. The next relevant mutation invalidates once;
        // subsequent writes are coalesced until geometry is read again.
        let mut state = shared.borrow_mut();
        let impact = state
            .dom
            .as_ref()
            .map(|dom| render_mutation_impact(dom, &cmd, &arg1, &arg2))
            .unwrap_or_default();
        #[cfg(feature = "render")]
        let retained_style_mutation = state
            .dom
            .as_ref()
            .and_then(|dom| retained_style_mutation(dom, &cmd, &arg1, &arg2));
        let invalidate = impact.connected && impact.actual_change;
        if invalidate {
            state.activity_generation = state.activity_generation.wrapping_add(1);
        }
        #[cfg(feature = "render")]
        if !reset_nodes.is_empty() {
            state
                .element_scroll_offsets
                .retain(|node, _| !reset_nodes.contains(node));
            state.scroll_generation = state.scroll_generation.wrapping_add(1);
            if invalidate {
                state.animation_timeline.remove_subtree(reset_nodes.iter());
            }
        }
        #[cfg(feature = "render")]
        let had_prepared_render = state.prepared_render.is_some();
        #[cfg(feature = "render")]
        if invalidate {
            let mutation_time_ms = (state.animation_timeline_origin.elapsed().as_secs_f64()
                * 1_000.0)
                .min(f64::from(f32::MAX)) as f32;
            // Keep animation birth epochs local to the changed subtree. A
            // single document-global timestamp made a later unrelated write
            // restart every not-yet-sampled animation at the same instant.
            let direct_root = match cmd.as_str() {
                "append_child" => arg2.parse::<u32>().ok(),
                "insert_before"
                | "set_attribute"
                | "remove_attribute"
                | "set_attribute_ns"
                | "remove_attribute_ns" => arg1.parse::<u32>().ok(),
                _ => None,
            }
            .map(NodeId::new);
            let direct_nodes = direct_root
                .and_then(|root| {
                    state.dom.as_ref().map(|dom| {
                        std::iter::once(root)
                            .chain(dom.descendants(root))
                            .collect::<Vec<_>>()
                    })
                })
                .unwrap_or_default();
            for node in direct_nodes {
                state
                    .animation_timeline
                    .note_start_candidate(node, mutation_time_ms);
            }
            let scope_root = match cmd.as_str() {
                "append_child" => arg1.parse::<u32>().ok().map(NodeId::new),
                "insert_before" => arg2
                    .parse::<u32>()
                    .ok()
                    .map(NodeId::new)
                    .and_then(|reference| {
                        state.dom.as_ref()?.get_node(reference)?.parent
                    }),
                "remove_child" => arg1
                    .parse::<u32>()
                    .ok()
                    .map(NodeId::new)
                    .and_then(|child| state.dom.as_ref()?.get_node(child)?.parent),
                "set_inner_html" | "set_inner_html_context" | "set_text_content" => {
                    arg1.parse::<u32>().ok().map(NodeId::new)
                }
                _ => None,
            };
            if let Some(root) = scope_root {
                state
                    .animation_timeline
                    .note_subtree_start_candidate(root, mutation_time_ms);
            }
            if let Some(mutation) = retained_style_mutation {
                let retained = state.prepared_render.is_some()
                    && queue_retained_style_mutation(
                        &mut state.pending_style_mutations,
                        mutation,
                    );
                if !retained {
                    state.prepared_render = None;
                    state.pending_style_mutations.clear();
                }
            } else {
                state.prepared_render = None;
                state.pending_style_mutations.clear();
            }
            state.resolved_scroll = None;
        }
        #[cfg(feature = "render")]
        if had_prepared_render && is_render_mutation_command(&cmd) && render_timing_enabled() {
            static MUTATION_SEQUENCE: std::sync::atomic::AtomicU64 =
                std::sync::atomic::AtomicU64::new(0);
            let sequence = MUTATION_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            let detail = match cmd.as_str() {
                "set_attribute" => arg2.split_once('\0').map(|(name, _)| name).unwrap_or(""),
                "remove_attribute" => arg2.as_str(),
                _ => "",
            };
            eprintln!(
                "[timing] render-cache mutation sequence={} cmd={} node={} detail={} connected={} actual_change={} invalidated={}",
                sequence, cmd, arg1, detail, impact.connected, impact.actual_change, invalidate
            );
        }
    }
    let gs = shared.borrow();
    // Workers have environment URLs without a DOM document. Resolve these
    // native URL queries before the DOM-only operations below.
    match cmd.as_str() {
        "document_url" => return serde_json::to_string(
            &gs.dom.as_ref().and_then(DomTree::document_url).unwrap_or_else(|| gs.url.clone()),
        ).unwrap_or("\"\"".into()),
        // The base for relative URLs. It differs from document_url exactly when the page carries
        // a <base href>, and that is the point: HTML resolves against the base, not the document.
        "document_base_url" => return serde_json::to_string(
            &document_base_url_memoized(&gs).unwrap_or_else(|| gs.url.clone()),
        )
        .unwrap_or("\"\"".into()),
        // The unresolved attribute. After history.pushState only JS knows the URL, so only JS
        // can resolve a relative base against it.
        "document_base_href" => {
            return serde_json::to_string(&document_base_href_memoized(&gs).unwrap_or_default())
                .unwrap_or("\"\"".into())
        }
        _ => {}
    }
    let dom = match &gs.dom {
        Some(d) => d,
        None => return "null".to_string(),
    };

    match cmd.as_str() {
        "get_form_state" => {
            let nid = NodeId::new(arg1.parse().unwrap_or(u32::MAX));
            dom.form_control_state(nid)
                .map(|control| {
                    serde_json::json!({
                        "value": control.value,
                        "checked": control.checked,
                        "indeterminate": control.indeterminate,
                    })
                    .to_string()
                })
                .unwrap_or_else(|| "null".to_string())
        }
        "set_form_value" | "set_form_checked" | "set_form_indeterminate" => {
            let nid = NodeId::new(arg1.parse().unwrap_or(u32::MAX));
            dom.update_form_control_state(nid, |control| match cmd.as_str() {
                "set_form_value" => control.value = Some(arg2.clone()),
                "set_form_checked" => control.checked = Some(arg2 == "true"),
                _ => control.indeterminate = arg2 == "true",
            });
            "null".to_string()
        }
        "ancestor_path" => {
            let Some(id) = arg1.parse::<u32>().ok().map(NodeId::new).filter(|id| dom.get_node(*id).is_some()) else { return "[]".into(); };
            let nodes = std::iter::once(id).chain(dom.ancestors(id)).map(|id| id.raw()).collect::<Vec<_>>();
            serde_json::to_string(&nodes).unwrap_or_else(|_| "[]".into())
        }
        "document_ready_state" => serde_json::to_string(match gs.document_lifecycle {
            0 => "loading",
            1 | 2 => "interactive",
            _ => "complete",
        })
        .unwrap(),
        "scroll_event_identity" | "scroll_event_identity_exact" => {
            let result = (|| {
                let node = NodeId::new(arg1.parse::<u32>().ok()?);
                dom.get_node(node)?;
                let node = if cmd == "scroll_event_identity"
                    && (dom.is_html_element(node, "html") || dom.is_html_element(node, "body")) {
                    dom.document()
                } else {
                    node
                };
                Some((
                    gs.document_generation.to_string(),
                    node.raw(),
                    dom.node_generation(node)?.to_string(),
                    gs.input_document_epoch.get().to_string(),
                ))
            })();
            serde_json::to_string(&result).unwrap()
        }
        "document_node_id" => dom.document().index().to_string(),
        "document_title" => {
            // The DOM is authoritative after parsing. In particular, script
            // changes through title.textContent must be reflected by
            // document.title, not hidden behind the navigation-time snapshot.
            let title = dom
                .query_selector("title")
                .ok()
                .flatten()
                .map(|title_id| {
                    dom.text_content(title_id)
                        .split(|ch| matches!(ch, '\t' | '\n' | '\u{000C}' | '\r' | ' '))
                        .filter(|part| !part.is_empty())
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .unwrap_or_default();
            serde_json::to_string(&title).unwrap_or("\"\"".into())
        }
        "document_referrer" => serde_json::to_string(&gs.referrer).unwrap_or("\"\"".into()),
        "document_encoding" => serde_json::to_string(&gs.encoding).unwrap_or("\"UTF-8\"".into()),
        "document_element" => {
            for cid in dom.children(dom.document()) {
                if let Some(n) = dom.get_node(cid) {
                    if n.as_element()
                        .map(|name| name.local.as_ref() == "html")
                        .unwrap_or(false)
                    {
                        return cid.index().to_string();
                    }
                }
            }
            "-1".into()
        }
        "document_doctype" => {
            for cid in dom.children(dom.document()) {
                if let Some(n) = dom.get_node(cid) {
                    if let obscura_dom::NodeData::Doctype {
                        name,
                        public_id,
                        system_id,
                    } = &n.data
                    {
                        return serde_json::json!({
                            "name": name,
                            "publicId": public_id,
                            "systemId": system_id,
                            "nodeId": cid.index(),
                        })
                        .to_string();
                    }
                }
            }
            "null".into()
        }
        "get_element_by_id" => {
            // Verify the indexed node is in the live document. The id_index is best-effort:
            // it only registers nodes at creation time and doesn't update on reparent, so
            // it can point to a detached clone while the live node is elsewhere in the tree.
            let doc = dom.document();
            let nid = dom.get_element_by_id(&arg1);
            let live = nid.filter(|&n| dom.ancestors(n).contains(&doc));
            match live {
                Some(n) => n.index().to_string(),
                None => {
                    // Fall back to full scan for the live document.
                    let sel = format!(
                        "[id=\"{}\"]",
                        arg1.replace('\\', "\\\\").replace('"', "\\\"")
                    );
                    dom.query_selector(&sel)
                        .ok()
                        .flatten()
                        .map(|id| id.index().to_string())
                        .unwrap_or("-1".into())
                }
            }
        }
        "query_selector" => dom
            .query_selector(&arg1)
            .ok()
            .flatten()
            .map(|id| id.index().to_string())
            .unwrap_or("-1".into()),
        "query_selector_all" => {
            let ids: Vec<i32> = dom
                .query_selector_all(&arg1)
                .ok()
                .map(|ids| ids.iter().map(|id| id.index() as i32).collect())
                .unwrap_or_default();
            serde_json::to_string(&ids).unwrap_or("[]".into())
        }
        "query_selector_scoped" => {
            let root_nid = arg1.parse::<u32>().unwrap_or(0);
            dom.query_selector_from(NodeId::new(root_nid), &arg2)
                .ok()
                .flatten()
                .map(|id| id.index().to_string())
                .unwrap_or("-1".into())
        }
        "query_selector_all_scoped" => {
            let root_nid = arg1.parse::<u32>().unwrap_or(0);
            let ids: Vec<i32> = dom
                .query_selector_all_from(NodeId::new(root_nid), &arg2)
                .ok()
                .map(|ids| ids.iter().map(|id| id.index() as i32).collect())
                .unwrap_or_default();
            serde_json::to_string(&ids).unwrap_or("[]".into())
        }
        "matches_selector" => {
            let nid = NodeId::new(arg1.parse::<u32>().unwrap_or(0));
            dom.matches_selector(nid, &arg2)
                .unwrap_or(false)
                .to_string()
        }
        "node_type" => {
            let nid = arg1.parse::<u32>().unwrap_or(0);
            dom.with_node(NodeId::new(nid), |n| match &n.data {
                NodeData::Document => "9",
                NodeData::Element { .. } => "1",
                NodeData::Text { .. } => "3",
                NodeData::Comment { .. } => "8",
                NodeData::Doctype { .. } => "10",
                NodeData::ProcessingInstruction { .. } => "7",
            })
            .unwrap_or("0")
            .into()
        }
        "node_name" => {
            let nid = arg1.parse::<u32>().unwrap_or(0);
            let name: String = dom
                .with_node(NodeId::new(nid), |n| match &n.data {
                    NodeData::Document => "#document".to_string(),
                    NodeData::Element { name, .. } => name.local.as_ref().to_ascii_uppercase(),
                    NodeData::Text { .. } => "#text".to_string(),
                    NodeData::Comment { .. } => "#comment".to_string(),
                    NodeData::Doctype { name, .. } => name.clone(),
                    NodeData::ProcessingInstruction { target, .. } => target.clone(),
                })
                .unwrap_or_default();
            serde_json::to_string(&name).unwrap_or("\"\"".into())
        }
        "text_content" => {
            let nid = arg1.parse::<u32>().unwrap_or(0);
            serde_json::to_string(&dom.text_content(NodeId::new(nid))).unwrap_or("\"\"".into())
        }
        "parent_node" | "first_child" | "last_child" | "next_sibling" | "prev_sibling" => {
            let nid = arg1.parse::<u32>().unwrap_or(0);
            dom.with_node(NodeId::new(nid), |n| match cmd.as_str() {
                "parent_node" => n.parent,
                "first_child" => n.first_child,
                "last_child" => n.last_child,
                "next_sibling" => n.next_sibling,
                "prev_sibling" => n.prev_sibling,
                _ => None,
            })
            .flatten()
            .map(|id| id.index().to_string())
            .unwrap_or("-1".into())
        }
        "next_in_subtree" => {
            let root = NodeId::new(arg1.parse::<u32>().unwrap_or(0));
            let current = NodeId::new(arg2.parse::<u32>().unwrap_or(0));
            dom.next_in_subtree(root, current)
                .map(|id| id.index().to_string())
                .unwrap_or("-1".into())
        }
        // Reverse document order within a subtree, for NodeIterator's backward
        // walk (which prunes nothing, so the whole step fits in the DOM layer).
        "prev_in_subtree" => {
            let root = NodeId::new(arg1.parse::<u32>().unwrap_or(0));
            let current = NodeId::new(arg2.parse::<u32>().unwrap_or(0));
            dom.prev_in_subtree(root, current)
                .map(|id| id.index().to_string())
                .unwrap_or("-1".into())
        }
        // Step past a whole subtree rather than into it: NodeFilter.FILTER_REJECT
        // prunes the rejected node's descendants, unlike FILTER_SKIP.
        "next_after_subtree" => {
            let root = NodeId::new(arg1.parse::<u32>().unwrap_or(0));
            let current = NodeId::new(arg2.parse::<u32>().unwrap_or(0));
            dom.next_after_subtree(root, current)
                .map(|id| id.index().to_string())
                .unwrap_or("-1".into())
        }
        "child_nodes" => {
            let nid = arg1.parse::<u32>().unwrap_or(0);
            let ids: Vec<i32> = dom
                .children(NodeId::new(nid))
                .iter()
                .map(|id| id.index() as i32)
                .collect();
            serde_json::to_string(&ids).unwrap_or("[]".into())
        }
        // Nodes directly assigned to an HTML <slot> (named slot assignment; the
        // first same-name slot in the shadow tree wins). `null` when the node is
        // not an HTML slot inside a shadow tree, so JS can tell "no slot" from
        // "slot without assignments" and fall back to the slot's own children.
        "assigned_nodes" => {
            let nid = arg1.parse::<u32>().unwrap_or(0);
            match dom.assigned_nodes(NodeId::new(nid)) {
                Some(ids) => {
                    let ids: Vec<i32> = ids.iter().map(|id| id.index() as i32).collect();
                    serde_json::to_string(&ids).unwrap_or("[]".into())
                }
                None => "null".into(),
            }
        }
        "tag_name" => {
            let nid = arg1.parse::<u32>().unwrap_or(0);
            let name = dom
                .with_node(NodeId::new(nid), |n| {
                    n.as_element().map(|name| {
                        if name.ns == html5ever::ns!(html) {
                            name.local.as_ref().to_ascii_uppercase()
                        } else {
                            match &name.prefix {
                                Some(prefix) => format!("{}:{}", prefix, name.local),
                                None => name.local.to_string(),
                            }
                        }
                    })
                })
                .flatten()
                .unwrap_or_default();
            serde_json::to_string(&name).unwrap_or("\"\"".into())
        }
        "local_name" => {
            let nid = arg1.parse::<u32>().unwrap_or(0);
            let name = dom
                .with_node(NodeId::new(nid), |n| {
                    n.as_element().map(|name| name.local.to_string())
                })
                .flatten()
                .unwrap_or_default();
            serde_json::to_string(&name).unwrap_or("\"\"".into())
        }
        // The tree builder already assigns foreign content (an <svg>/<math>
        // subtree) its own namespace; expose it so JS does not have to guess
        // the namespace from the tag name.
        "namespace_uri" => {
            let nid = arg1.parse::<u32>().unwrap_or(0);
            let ns = dom
                .with_node(NodeId::new(nid), |n| {
                    n.as_element().map(|name| name.ns.as_ref().to_string())
                })
                .flatten()
                .unwrap_or_default();
            serde_json::to_string(&ns).unwrap_or("\"\"".into())
        }
        "get_attribute" => {
            let nid = arg1.parse::<u32>().unwrap_or(0);
            let val = dom
                .with_node(NodeId::new(nid), |n| {
                    n.get_attribute(&arg2).map(|s| s.to_string())
                })
                .flatten();
            serde_json::to_string(&val).unwrap_or("null".into())
        }
        "attribute_names" => {
            let nid = arg1.parse::<u32>().unwrap_or(0);
            let names: Vec<String> = dom
                .with_node(NodeId::new(nid), |n| {
                    n.attrs()
                        .map(|a| a.iter().map(|x| x.qualified_name()).collect())
                        .unwrap_or_default()
                })
                .unwrap_or_default();
            serde_json::to_string(&names).unwrap_or("[]".into())
        }
        "set_attribute" => {
            let nid = arg1.parse::<u32>().unwrap_or(0);
            let node_id = NodeId::new(nid);
            if let Some((name, value)) = arg2.split_once('\0') {
                if name == "id" {
                    let old_id = dom
                        .with_node(node_id, |n| n.get_attribute("id").map(|s| s.to_string()))
                        .flatten();
                    dom.with_node_mut(node_id, |n| n.set_attribute(name, value.to_string()));
                    dom.update_id_index(node_id, old_id.as_deref(), Some(value));
                } else {
                    dom.with_node_mut(node_id, |n| n.set_attribute(name, value.to_string()));
                }
                if name == "href" && dom.refresh_base_href(node_id) {
                    drop(gs);
                    invalidate_input_render(&mut shared.borrow_mut());
                }
            }
            "true".into()
        }
        "inner_html" => {
            let nid = arg1.parse::<u32>().unwrap_or(0);
            serde_json::to_string(&dom.inner_html(NodeId::new(nid))).unwrap_or("\"\"".into())
        }
        "outer_html" => {
            let nid = arg1.parse::<u32>().unwrap_or(0);
            serde_json::to_string(&dom.outer_html(NodeId::new(nid))).unwrap_or("\"\"".into())
        }
        "append_child" => {
            // Reject if either nid failed to parse (was "undefined"/empty) — those
            // default to 0 which is the document root, and silently operating on it
            // corrupts the tree. Require both args to be valid positive integers.
            let parent = match arg1.parse::<u32>() {
                Ok(n) => n,
                Err(_) => return "false".into(),
            };
            let child = match arg2.parse::<u32>() {
                Ok(n) => n,
                Err(_) => return "false".into(),
            };
            let parent = NodeId::new(parent);
            let child = NodeId::new(child);
            dom.append_child(parent, child);
            (dom.get_node(child).and_then(|node| node.parent) == Some(parent)).to_string()
        }
        "remove_child" => {
            let child = match arg1.parse::<u32>() {
                Ok(n) => n,
                Err(_) => return "false".into(),
            };
            let child = NodeId::new(child);
            let had_parent = dom.get_node(child).is_some_and(|node| node.parent.is_some());
            dom.remove_child(child);
            (had_parent && dom.get_node(child).is_some_and(|node| node.parent.is_none())).to_string()
        }
        "insert_before" => {
            let new_node = match arg1.parse::<u32>() {
                Ok(n) => n,
                Err(_) => return "false".into(),
            };
            let ref_node = match arg2.parse::<u32>() {
                Ok(n) => n,
                Err(_) => return "false".into(),
            };
            let ref_node = NodeId::new(ref_node);
            let new_node = NodeId::new(new_node);
            let expected_parent = dom.get_node(ref_node).and_then(|node| node.parent);
            dom.insert_before(ref_node, new_node);
            (expected_parent.is_some()
                && dom.get_node(new_node).and_then(|node| node.parent) == expected_parent)
                .to_string()
        }
        "remove_attribute" => {
            let nid = arg1.parse::<u32>().unwrap_or(0);
            dom.with_node_mut(NodeId::new(nid), |n| {
                if let NodeData::Element { attrs, .. } = &mut n.data {
                    attrs.retain(|a| !a.qualified_name_eq(&arg2));
                }
            });
            "true".into()
        }
        // Namespace-aware attribute ops. arg2 packs the pieces with a NUL:
        //   get/remove: "<namespace>\0<localName>"
        //   set:        "<namespace>\0<qualifiedName>\0<value>"
        "get_attribute_ns" => {
            let nid = arg1.parse::<u32>().unwrap_or(0);
            let (ns, local) = arg2.split_once('\0').unwrap_or(("", arg2.as_str()));
            let val = dom
                .with_node(NodeId::new(nid), |n| n.get_attribute_ns(ns, local).map(|s| s.to_string()))
                .flatten();
            serde_json::to_string(&val).unwrap_or("null".into())
        }
        "set_attribute_ns" => {
            let nid = arg1.parse::<u32>().unwrap_or(0);
            let node_id = NodeId::new(nid);
            let mut parts = arg2.splitn(3, '\0');
            let ns = parts.next().unwrap_or("");
            let qualified = parts.next().unwrap_or("");
            let value = parts.next().unwrap_or("");
            if !qualified.is_empty() {
                let local = qualified
                    .split_once(':')
                    .map(|(_, local)| local)
                    .unwrap_or(qualified);
                if ns.is_empty() && local == "id" {
                    let old_id = dom
                        .with_node(node_id, |n| n.get_attribute("id").map(str::to_owned))
                        .flatten();
                    dom.with_node_mut(node_id, |n| {
                        n.set_attribute_ns(ns, qualified, value.to_string())
                    });
                    dom.update_id_index(node_id, old_id.as_deref(), Some(value));
                } else {
                    dom.with_node_mut(node_id, |n| {
                        n.set_attribute_ns(ns, qualified, value.to_string())
                    });
                }
                if ns.is_empty() && local == "href" && dom.refresh_base_href(node_id) {
                    drop(gs);
                    invalidate_input_render(&mut shared.borrow_mut());
                }
            }
            "true".into()
        }
        "remove_attribute_ns" => {
            let nid = arg1.parse::<u32>().unwrap_or(0);
            let node_id = NodeId::new(nid);
            let (ns, local) = arg2.split_once('\0').unwrap_or(("", arg2.as_str()));
            if ns.is_empty() && local == "id" {
                let old_id = dom
                    .with_node(node_id, |n| n.get_attribute("id").map(str::to_owned))
                    .flatten();
                dom.with_node_mut(node_id, |n| n.remove_attribute_ns(ns, local));
                dom.update_id_index(node_id, old_id.as_deref(), None);
            } else {
                dom.with_node_mut(node_id, |n| n.remove_attribute_ns(ns, local));
            }
            "true".into()
        }
        "set_inner_html" => {
            let nid = match arg1.parse::<u32>() {
                Ok(n) if n > 0 => n,
                // nid=0 is the document root; never allow innerHTML to clear it.
                // nid parse failure (e.g. "undefined") also falls here.
                _ => return "false".into(),
            };
            let target = NodeId::new(nid);
            let children = dom.children(target);
            for child in children {
                dom.detach(child);
            }
            if !arg2.is_empty() {
                let context_name = dom
                    .with_node(target, |node| match &node.data {
                        NodeData::Element { name, .. } => Some(name.clone()),
                        _ => None,
                    })
                    .flatten();
                let fragment = match context_name {
                    Some(name) => obscura_dom::parse_fragment_with_context(&arg2, name),
                    None => obscura_dom::parse_fragment(&arg2),
                };
                let import_root = fragment.fragment_root();
                dom.import_children_from(target, &fragment, import_root);
                for child in dom.children(target) {
                    mark_script_subtree_started(&gs, child);
                }
            }
            "true".into()
        }
        "set_inner_html_context" => {
            let nid = match arg1.parse::<u32>() {
                Ok(n) if n > 0 => n,
                _ => return "false".into(),
            };
            let target = NodeId::new(nid);
            let (context_name, html) = fragment_context_and_html(&arg2);
            for child in dom.children(target) {
                dom.detach(child);
            }
            if !html.is_empty() {
                let fragment = obscura_dom::parse_fragment_with_context(html, context_name);
                let import_root = fragment.fragment_root();
                dom.import_children_from(target, &fragment, import_root);
                for child in dom.children(target) {
                    mark_script_subtree_started(&gs, child);
                }
            }
            "true".into()
        }
        // Range.createContextualFragment has a deliberately different script
        // policy from innerHTML: scripts remain eligible and are prepared when
        // the returned fragment is inserted into a connected document.
        "set_fragment_html_executable" => {
            let nid = match arg1.parse::<u32>() {
                Ok(n) if n > 0 => n,
                _ => return "false".into(),
            };
            let target = NodeId::new(nid);
            let (context_name, html) = fragment_context_and_html(&arg2);
            for child in dom.children(target) {
                dom.detach(child);
            }
            if !html.is_empty() {
                let fragment = obscura_dom::parse_fragment_with_context(html, context_name);
                let import_root = fragment.fragment_root();
                dom.import_children_from(target, &fragment, import_root);
            }
            "true".into()
        }
        // document.write() feeds the document's input stream, so the calls
        // share one parser and one tokenizer state. Returns the nodes that
        // became complete with this call, for the caller to run scripts among.
        // Returns [[parent, node], …], parents before children. A `parent` of 0 means the node
        // belongs at the insertion point, which the caller knows. Nothing is inserted here:
        // that must go through Node.appendChild on the JS side, because that call also reports
        // the mutation, registers window named access, and loads a written stylesheet.
        "document_write" => {
            let mut slot = gs.write_stream.borrow_mut();
            let stream = slot.get_or_insert_with(DocumentWriteStream::new);
            let pairs: Vec<[i32; 2]> = stream
                .write(&arg2, dom)
                .iter()
                .map(|placement| {
                    [
                        placement.parent.map_or(0, |id| id.index() as i32),
                        placement.node.index() as i32,
                    ]
                })
                .collect();
            serde_json::to_string(&pairs).unwrap_or("[]".into())
        }
        // document.open() discards what the input stream holds and starts over.
        "document_write_reset" => {
            *gs.write_stream.borrow_mut() = None;
            gs.input_document_epoch.set(gs.input_document_epoch.get().wrapping_add(1));
            "true".into()
        }
        "input_document_epoch" => gs.input_document_epoch.get().to_string(),
        "set_text_content" => {
            let nid = arg1.parse::<u32>().unwrap_or(0);
            dom.with_node_mut(NodeId::new(nid), |n| match &mut n.data {
                NodeData::Text { contents } => {
                    *contents = arg2.clone();
                }
                NodeData::Comment { contents } => {
                    *contents = arg2.clone();
                }
                NodeData::ProcessingInstruction { data, .. } => {
                    *data = arg2.clone();
                }
                _ => {}
            });
            "true".into()
        }
        // A <template>'s children live in a separate contents document, so this
        // is the only route to them from JS. Allocates one on demand for
        // templates built via createElement.
        "template_contents" => {
            let nid = arg1.parse::<u32>().unwrap_or(0);
            dom.template_contents(NodeId::new(nid))
                .map(|id| id.index().to_string())
                .unwrap_or("-1".into())
        }
        "create_document_fragment" => dom.new_node(NodeData::Document).index().to_string(),
        "clone_node" => {
            let nid = match arg1.parse::<u32>() {
                Ok(n) => n,
                Err(_) => return "-1".into(),
            };
            let source = NodeId::new(nid);
            match dom.clone_node(source, arg2 == "true") {
                Some(cloned) => {
                    propagate_script_start_state(dom, source, cloned, &gs.already_started_scripts);
                    cloned.index().to_string()
                }
                None => "-1".into(),
            }
        }
        "create_element" => dom
            .new_node(NodeData::Element {
                name: html5ever::QualName::new(
                    None,
                    html5ever::ns!(html),
                    html5ever::LocalName::from(arg1.as_str()),
                ),
                attrs: vec![],
                template_contents: None,
                mathml_annotation_xml_integration_point: false,
            })
            .index()
            .to_string(),
        "create_element_ns" => {
            let (namespace, qualified) = arg1.split_once('\0').unwrap_or(("", arg1.as_str()));
            let (prefix, local) = match qualified.split_once(':') {
                Some((prefix, local)) if !prefix.is_empty() && !local.is_empty() => {
                    (Some(html5ever::Prefix::from(prefix)), local)
                }
                None if !qualified.is_empty() => (None, qualified),
                _ => return "-1".into(),
            };
            dom.new_node(NodeData::Element {
                name: html5ever::QualName::new(
                    prefix,
                    html5ever::Namespace::from(namespace),
                    html5ever::LocalName::from(local),
                ),
                attrs: vec![],
                template_contents: None,
                mathml_annotation_xml_integration_point: false,
            })
            .index()
            .to_string()
        }
        "create_text_node" => dom
            .new_node(NodeData::Text {
                contents: arg1.clone(),
            })
            .index()
            .to_string(),
        "create_comment_node" => dom
            .new_node(NodeData::Comment {
                contents: arg1.clone(),
            })
            .index()
            .to_string(),
        "create_processing_instruction" => {
            // arg1 = target, arg2 = data
            dom.new_node(NodeData::ProcessingInstruction {
                target: arg1.clone(),
                data: arg2.clone(),
            })
            .index()
            .to_string()
        }
        "create_doctype" => {
            // arg1 = name, arg2 = public_id. system_id stored only in the
            // JS wrapper since neither current WPT test reads it back from
            // the underlying tree.
            dom.new_node(NodeData::Doctype {
                name: arg1.clone(),
                public_id: arg2.clone(),
                system_id: String::new(),
            })
            .index()
            .to_string()
        }
        "pi_target" => {
            let nid = arg1.parse::<u32>().unwrap_or(0);
            let val = dom
                .with_node(NodeId::new(nid), |n| match &n.data {
                    NodeData::ProcessingInstruction { target, .. } => Some(target.clone()),
                    _ => None,
                })
                .flatten()
                .unwrap_or_default();
            serde_json::to_string(&val).unwrap_or("\"\"".into())
        }
        "doctype_name" => {
            let nid = arg1.parse::<u32>().unwrap_or(0);
            let val = dom
                .with_node(NodeId::new(nid), |n| match &n.data {
                    NodeData::Doctype { name, .. } => Some(name.clone()),
                    _ => None,
                })
                .flatten()
                .unwrap_or_default();
            serde_json::to_string(&val).unwrap_or("\"\"".into())
        }
        "doctype_public_id" => {
            let nid = arg1.parse::<u32>().unwrap_or(0);
            let val = dom
                .with_node(NodeId::new(nid), |n| match &n.data {
                    NodeData::Doctype { public_id, .. } => Some(public_id.clone()),
                    _ => None,
                })
                .flatten()
                .unwrap_or_default();
            serde_json::to_string(&val).unwrap_or("\"\"".into())
        }
        "element_children" => {
            let nid = arg1.parse::<u32>().unwrap_or(0);
            let ids: Vec<i32> = dom
                .children(NodeId::new(nid))
                .iter()
                .filter(|&&id| dom.get_node(id).map(|n| n.is_element()).unwrap_or(false))
                .map(|id| id.index() as i32)
                .collect();
            serde_json::to_string(&ids).unwrap_or("[]".into())
        }
        "has_child_nodes" => {
            let nid = arg1.parse::<u32>().unwrap_or(0);
            dom.with_node(NodeId::new(nid), |n| n.first_child.is_some())
                .unwrap_or(false)
                .to_string()
        }
        "contains" => {
            let nid = arg1.parse::<u32>().unwrap_or(0);
            let other = arg2.parse::<u32>().unwrap_or(0);
            dom.descendants(NodeId::new(nid))
                .contains(&NodeId::new(other))
                .to_string()
        }
        // Connectivity is maintained incrementally by DomTree. Exposing the
        // cached bit avoids an ancestor op crossing for every level when JS
        // builds a deep detached subtree.
        "is_connected" => {
            let nid = arg1.parse::<u32>().unwrap_or(0);
            dom.is_connected(NodeId::new(nid)).to_string()
        }
        // Index of a node among its parent's children. Walks prev siblings in
        // Rust, avoiding the per-step JS->op round trips a Range comparison
        // would otherwise make.
        "node_index" => {
            let nid = arg1.parse::<u32>().unwrap_or(0);
            node_child_index(dom, NodeId::new(nid)).to_string()
        }
        // Document (preorder) tree order of two nodes: -1 if a precedes b, 1 if
        // a follows b, 0 if equal. Used by the Range boundary-point algorithms.
        "compare_order" => {
            let a = NodeId::new(arg1.parse::<u32>().unwrap_or(0));
            let b = NodeId::new(arg2.parse::<u32>().unwrap_or(0));
            compare_node_order(dom, a, b).to_string()
        }
        // Root (topmost ancestor) of a node, in one op rather than an O(depth)
        // walk of parentNode ops from JS.
        "node_root" => {
            let mut cur = NodeId::new(arg1.parse::<u32>().unwrap_or(0));
            while let Some(p) = dom.with_node(cur, |x| x.parent).flatten() {
                cur = p;
            }
            cur.index().to_string()
        }
        _ => "null".into(),
    }
}

/// Index of `n` among its parent's children (0-based).
fn node_child_index(dom: &DomTree, n: NodeId) -> usize {
    let mut i = 0usize;
    let mut cur = dom.with_node(n, |x| x.prev_sibling).flatten();
    while let Some(p) = cur {
        i += 1;
        cur = dom.with_node(p, |x| x.prev_sibling).flatten();
    }
    i
}

/// Ancestor chain of `n` from the root down to `n` (root first).
fn node_ancestors_root_first(dom: &DomTree, n: NodeId) -> Vec<NodeId> {
    let mut v = vec![n];
    let mut cur = n;
    while let Some(p) = dom.with_node(cur, |x| x.parent).flatten() {
        v.push(p);
        cur = p;
    }
    v.reverse();
    v
}

/// Preorder (document) order comparison of two nodes: -1 before, 1 after, 0 same.
fn compare_node_order(dom: &DomTree, a: NodeId, b: NodeId) -> i32 {
    if a == b {
        return 0;
    }
    let aa = node_ancestors_root_first(dom, a);
    let bb = node_ancestors_root_first(dom, b);
    // Different roots: order is undefined per spec; keep it stable by node id.
    if aa[0] != bb[0] {
        return if a.index() < b.index() { -1 } else { 1 };
    }
    let mut i = 0usize;
    while i < aa.len() && i < bb.len() && aa[i] == bb[i] {
        i += 1;
    }
    if i >= aa.len() {
        return -1; // a is an ancestor of b -> a precedes
    }
    if i >= bb.len() {
        return 1; // b is an ancestor of a -> a follows
    }
    if node_child_index(dom, aa[i]) < node_child_index(dom, bb[i]) {
        -1
    } else {
        1
    }
}

#[op2(fast)]
fn op_runtime_events_enabled(state: &OpState) -> bool {
    crate::worker::refresh_policy(state);
    state.borrow::<SharedState>().borrow().runtime_events_enabled
}

#[op2(fast)]
fn op_console_msg(
    state: &OpState,
    #[string] level: &str,
    #[string] msg: &str,
    #[string] args_json: &str,
) {
    crate::worker::refresh_policy(state);
    match level {
        "warn" | "warning" => tracing::warn!(target: "obscura::console", "{}", msg),
        "error" => tracing::error!(target: "obscura::console", "{}", msg),
        _ => tracing::info!(target: "obscura::console", "{}", msg),
    }

    let page = state.borrow::<SharedState>().clone();
    let mut page = page.borrow_mut();
    if page.console_messages_enabled {
        if page.pending_console_messages.len() >= 1_024 {
            page.pending_console_messages.pop_front();
        }
        page.pending_console_messages
            .push_back(format!("[{level}] {msg}"));
    }
    if !page.runtime_events_enabled {
        return;
    }
    let Ok(args) = serde_json::from_str::<Vec<serde_json::Value>>(args_json) else {
        return;
    };
    if page.pending_runtime_events.len() >= 1_024 {
        page.pending_runtime_events.pop_front();
    }
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
        * 1_000.0;
    page.pending_runtime_events
        .push_back(RuntimeEvent::Console(RuntimeConsoleEvent {
            kind: level.to_string(),
            args,
            timestamp,
        }));
}

pub(crate) fn fetch_timeout() -> std::time::Duration {
    let timeout_ms = std::env::var("OBSCURA_FETCH_TIMEOUT_MS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(30_000);
    std::time::Duration::from_millis(timeout_ms)
}

/// Cap on the number of redirect hops op_fetch_url will follow.
///
/// The Fetch standard fixes the number at 20. HTTP-redirect fetch returns
/// a network error as soon as a request's redirect count *reaches* 20,
/// and only increments it afterwards. So the twentieth hop must still
/// succeed and the twenty-first must fail:
/// https://fetch.spec.whatwg.org/#http-redirect-fetch
///
/// Redirects are
/// followed by hand in this file, one hop per loop iteration, so that
/// each hop can be checked against the SSRF rules again.
const FETCH_REDIRECT_LIMIT: usize = 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FetchCredentials {
    Omit,
    SameOrigin,
    Include,
}

impl FetchCredentials {
    fn parse(value: &str) -> Self {
        match value {
            "omit" => Self::Omit,
            "include" => Self::Include,
            _ => Self::SameOrigin,
        }
    }

    fn allows(self, page_origin: &str, request_url: &str) -> bool {
        match self {
            Self::Omit => false,
            Self::Include => true,
            Self::SameOrigin => request_origin(request_url)
                .map(|origin| origin == page_origin)
                .unwrap_or(false),
        }
    }
}

fn request_origin(request_url: &str) -> Option<String> {
    url::Url::parse(request_url)
        .ok()
        .map(|url| url.origin().ascii_serialization())
}

fn scripted_fetch_metadata(source: &str, target: &str, mode: &str, resource_type: ResourceType) -> Vec<(&'static str, &'static str)> {
    let (Ok(source), Ok(target)) = (url::Url::parse(source), url::Url::parse(target)) else {
        return Vec::new();
    };
    let mut request = ResourceRequest::subresource(resource_type, &source);
    request.mode = match mode {
        "no-cors" => RequestMode::NoCors,
        "same-origin" => RequestMode::SameOrigin,
        _ => RequestMode::Cors,
    };
    let mut headers = request.fetch_metadata_headers(&target).to_vec();
    // Chromium marks fetch()/XHR as an incremental, urgency-1 request.  The
    // stealth transport has a navigation-safe `u=0, i` default, so this must
    // travel with the per-request metadata and replace that default.
    headers.push(("priority", "u=1, i"));
    headers
}

// Fetch appends Origin to scripted non-GET/HEAD requests even when same-origin.
// In no-cors mode the document's referrer policy can require an opaque origin.
// Same-origin CORS scripts (destination: script) also carry Origin in Chromium.
fn fetch_origin_header<'a>(
    method: &str,
    source: &'a str,
    target: &str,
    mode: &str,
    policy: ReferrerPolicy,
    destination: Option<&str>,
) -> Option<&'a str> {
    let cross_origin = request_origin(target).is_some_and(|origin| origin != source);
    let is_script_dest = matches!(destination, Some("script"));
    if mode == "cors" && (cross_origin || is_script_dest) {
        return Some(source);
    }
    if matches!(method, "GET" | "HEAD") {
        return None;
    }
    let downgrade = source.starts_with("https://") && target.starts_with("http://");
    let opaque = mode != "cors" && match policy {
        ReferrerPolicy::NoReferrer => true,
        ReferrerPolicy::SameOrigin => cross_origin,
        ReferrerPolicy::NoReferrerWhenDowngrade | ReferrerPolicy::StrictOrigin
            | ReferrerPolicy::StrictOriginWhenCrossOrigin => downgrade,
        _ => false,
    };
    Some(if opaque { "null" } else { source })
}

fn cors_response_allows(
    credentials: FetchCredentials,
    page_origin: &str,
    allowed_origin: &str,
    allow_credentials: &str,
) -> bool {
    if credentials == FetchCredentials::Include {
        allowed_origin == page_origin && allow_credentials == "true"
    } else {
        allowed_origin == "*" || allowed_origin == page_origin
    }
}

fn is_cors_safelisted_method(method: &http::Method) -> bool {
    matches!(method.as_str(), "GET" | "HEAD" | "POST")
}

fn is_cors_unsafe_request_header_byte(byte: u8) -> bool {
    (byte < 0x20 && byte != b'\t')
        || matches!(
            byte,
            b'"' | b'(' | b')' | b':' | b'<' | b'>' | b'?' | b'@' | b'[' | b'\\'
                | b']' | b'{' | b'}' | 0x7f
        )
}

fn is_http_token_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'*' | b'+' | b'-' | b'.'
                | b'^' | b'_' | b'`' | b'|' | b'~'
        )
}

fn is_cors_safelisted_content_type(value: &str) -> bool {
    if value.bytes().any(is_cors_unsafe_request_header_byte) {
        return false;
    }

    // A MIME type must have a valid type/subtype before its parameters. This
    // is deliberately narrower than merely splitting at ';': malformed values
    // must not turn an application/json request into a simple request.
    let essence = value
        .split_once(';')
        .map_or(value, |(essence, _)| essence)
        .trim_matches([' ', '\t']);
    let Some((type_, subtype)) = essence.split_once('/') else {
        return false;
    };
    if type_.is_empty()
        || subtype.is_empty()
        || !type_.bytes().all(is_http_token_byte)
        || !subtype.bytes().all(is_http_token_byte)
    {
        return false;
    }

    essence.eq_ignore_ascii_case("application/x-www-form-urlencoded")
        || essence.eq_ignore_ascii_case("multipart/form-data")
        || essence.eq_ignore_ascii_case("text/plain")
}

fn decimal_is_at_most(left: &str, right: &str) -> bool {
    let left = left.trim_start_matches('0');
    let right = right.trim_start_matches('0');
    left.len() < right.len() || (left.len() == right.len() && left <= right)
}

fn is_cors_safelisted_range(value: &str) -> bool {
    let Some(range) = value.strip_prefix("bytes=") else {
        return false;
    };
    let Some((start, end)) = range.split_once('-') else {
        return false;
    };
    if start.is_empty()
        || !start.bytes().all(|byte| byte.is_ascii_digit())
        || !end.bytes().all(|byte| byte.is_ascii_digit())
    {
        return false;
    }
    end.is_empty() || decimal_is_at_most(start, end)
}

fn is_cors_safelisted_request_header(name: &str, value: &str) -> bool {
    if value.len() > 128 {
        return false;
    }
    if name.eq_ignore_ascii_case("accept") {
        return !value.bytes().any(is_cors_unsafe_request_header_byte);
    }
    if name.eq_ignore_ascii_case("accept-language")
        || name.eq_ignore_ascii_case("content-language")
    {
        return value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b' ' | b'*' | b',' | b'-' | b'.' | b';' | b'=')
        });
    }
    if name.eq_ignore_ascii_case("content-type") {
        return is_cors_safelisted_content_type(value);
    }
    if name.eq_ignore_ascii_case("range") {
        return is_cors_safelisted_range(value);
    }
    false
}

/// Return the sorted, lowercase header names that must be authorized by a
/// CORS preflight. The aggregate safelist cap is observable only on unusual
/// requests and does not add work to same-origin requests.
fn cors_unsafe_request_header_names(headers: &HashMap<String, String>) -> Vec<String> {
    let mut unsafe_names = Vec::new();
    let mut safelist_value_size = 0usize;

    for (name, value) in headers {
        if is_cors_safelisted_request_header(name, value) {
            safelist_value_size = safelist_value_size.saturating_add(value.len());
        } else {
            unsafe_names.push(name.to_ascii_lowercase());
        }
    }
    if safelist_value_size > 1024 {
        unsafe_names.extend(
            headers
                .iter()
                .filter(|(name, value)| is_cors_safelisted_request_header(name, value))
                .map(|(name, _)| name.to_ascii_lowercase()),
        );
    }
    unsafe_names.sort_unstable();
    unsafe_names.dedup();
    unsafe_names
}

fn parse_cors_header_list<'a>(
    headers: &'a http::header::HeaderMap,
    name: &'static str,
) -> Option<Vec<&'a str>> {
    let mut items = Vec::new();
    for value in headers.get_all(name).iter() {
        let value = value.to_str().ok()?;
        for item in value.split(',') {
            let item = item.trim_matches([' ', '\t']);
            if item.is_empty() || !item.bytes().all(is_http_token_byte) {
                return None;
            }
            items.push(item);
        }
    }
    Some(items)
}

fn preflight_allows_method(method: &http::Method, allowed: &[&str], credentialed: bool) -> bool {
    is_cors_safelisted_method(method)
        || allowed.iter().any(|allowed| {
            *allowed == method.as_str() || (*allowed == "*" && !credentialed)
        })
}

fn preflight_allows_header(name: &str, allowed: &[&str], credentialed: bool) -> bool {
    allowed
        .iter()
        .any(|allowed| allowed.eq_ignore_ascii_case(name))
        || (!name.eq_ignore_ascii_case("authorization")
            && !credentialed
            && allowed.contains(&"*"))
}

/// Build the JS-facing result for an intercepted request a CDP client chose to
/// fulfill (`Fetch.fulfillRequest`). Mirrors the normal fetch result contract:
/// `body` is a lossy text view and `bodyBase64` carries the exact bytes, which
/// the bootstrap fetch layer prefers (`_base64ToUint8Array`) so a binary
/// fulfilled body is delivered intact rather than `from_utf8_lossy`-corrupted.
/// See #912.
fn intercept_fulfill_response(
    status: u16,
    headers: HashMap<String, String>,
    body: &str,
    body_base64: &str,
    url: &str,
) -> serde_json::Value {
    serde_json::json!({
        "status": status,
        "body": body,
        "bodyBase64": body_base64,
        "url": url,
        "headers": headers,
    })
}

/// One logical request, including redirect hops. The trace is scoped to this
/// operation; Page/Worker transports never keep a global last-request slot.
pub(crate) struct NetworkRequest {
    state: SharedState,
    pub id: String,
    pub trace: obscura_net::observation::RequestTrace,
    network_start: Arc<std::sync::atomic::AtomicU8>,
    hop_starts: Vec<Arc<std::sync::atomic::AtomicU8>>,
    hop_interceptions: Vec<Option<String>>,
    pub interception_id: Option<String>,
    response_interception_id: Option<String>,
    response_status_texts: Vec<String>,
    pub initiator_request_id: Option<String>,
    resource_type: ResourceType,
    finished: bool,
    emitted_exchanges: usize,
    document_generation: u64,
    document_url: String,
}

impl NetworkRequest {
    pub(crate) fn new(state: SharedState, id: Option<String>, url: &str, method: &str,
        headers: Option<obscura_net::HeaderCapture>, body_size: usize, resource_type: ResourceType,
    ) -> Self {
        let id = id.unwrap_or_else(|| {
            let state = state.borrow();
            let id = state.network_response_body_counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            format!("fetch-{id}")
        });
        let trace = obscura_net::observation::RequestTrace::new(state.borrow().network_response_bodies.clone(), id.clone());
        trace.begin(url, method, headers, body_size);
        let document_generation = state.borrow().network_document_generation;
        let document_url = state.borrow().network_document_url.clone();
        Self { document_generation, document_url, state, id, trace, network_start: Arc::new(std::sync::atomic::AtomicU8::new(0)), hop_starts: Vec::new(), hop_interceptions: Vec::new(), interception_id: None, response_interception_id: None, response_status_texts: Vec::new(), initiator_request_id: None, resource_type, finished: false, emitted_exchanges: 0 }
    }

    fn set_response_status_text(&mut self, index: usize, status_text: Option<String>) {
        let Some(status_text) = status_text else { return; };
        while self.response_status_texts.len() <= index {
            self.response_status_texts.push(String::new());
        }
        self.response_status_texts[index] = status_text;
    }

    fn start_before_preflight(&mut self) {
        let start = self.hop_starts.last().unwrap_or(&self.network_start).clone();
        if start.load(std::sync::atomic::Ordering::SeqCst) == 1 { return; }
        // Publish completed redirects and the next start in one synchronous
        // batch before a preflight can yield. finish() must not replay them.
        let redirects = self.trace.completed_since(self.emitted_exchanges);
        for exchange in redirects {
            let event = self.exchange_event(self.emitted_exchanges, exchange, true, None);
            self.state.borrow_mut().js_network_events.push(event);
            self.emitted_exchanges += 1;
        }
        let Some(exchange) = self.trace.last() else { return; };
        self.state.borrow_mut().js_network_events.push(JsNetworkEvent {
            document_generation: self.document_generation, document_url: self.document_url.clone(),
            initiator_request_id: None,
            pending: true, error: None, request_id: self.id.clone(), url: exchange.url.clone(), method: exchange.method.clone(),
            resource_type: self.resource_type, status: 0, status_text: String::new(), response_headers: HashMap::new(), raw_headers: None,
            request_raw_headers: exchange.request_headers.clone(), request_body_size: exchange.request_body_size,
            request_post_data: exchange.request_post_data.clone(),
            request_started: false, redirect: false, response_body_request_id: None, body_size: 0,
            timestamp: exchange.request_started_at,
            request_timestamp: exchange.request_started_at,
            request_prepared_timestamp: exchange.request_prepared_at,
            response_headers_timestamp: exchange.response_headers_at,
        });
        start.store(1, std::sync::atomic::Ordering::SeqCst);
    }

    pub(crate) fn finish(&mut self, error: Option<String>) {
        let failure = error.is_some();
        self.finish_into(error, failure);
    }

    fn finish_into(&mut self, error: Option<String>, teardown: bool) {
        if self.finished { return; }
        self.finished = true;
        let exchanges = self.trace.take();
        let count = exchanges.len();
        let mut events = Vec::new();
        for (index, exchange) in exchanges.into_iter().enumerate().skip(self.emitted_exchanges) {
            events.push(self.exchange_event(index, exchange, index + 1 < count, error.clone()));
        }
        let mut state = self.state.borrow_mut();
        if teardown {
            let pending = std::mem::take(&mut state.js_network_events);
            let mut queue = state.network_teardown_events.lock().unwrap_or_else(|e| e.into_inner());
            queue.extend(pending);
            queue.extend(events);
            let excess = queue.len().saturating_sub(4096);
            queue.drain(..excess);
            state.network_teardown_notify.notify_one();
        } else {
            state.js_network_events.extend(events);
            let excess = state.js_network_events.len().saturating_sub(4096);
            state.js_network_events.drain(..excess);
        }
    }
    fn exchange_event(&self, index: usize, exchange: obscura_net::observation::Exchange,
        redirect: bool, error: Option<String>,
    ) -> JsNetworkEvent {
        let state = self.state.borrow();
        let response = exchange.response;
        let body_captured = exchange.body_complete && response.is_some();
        let body_id = exchange.body_request_id;
        if !redirect && body_captured {
            let mut store = state.network_response_bodies.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(body_id) = &body_id {
                let _ = store.alias(body_id, &self.id);
            }
        }
        if body_captured {
            let alias = if index == 0 { self.interception_id.as_ref() } else { self.hop_interceptions.get(index - 1).and_then(Option::as_ref) };
            if let (Some(body_id), Some(alias)) = (&body_id, alias) {
                let _ = state.network_response_bodies.lock().unwrap_or_else(|e| e.into_inner()).alias(body_id, alias);
            }
        }
        JsNetworkEvent {
            document_generation: self.document_generation, document_url: self.document_url.clone(),
            initiator_request_id: self.initiator_request_id.clone(),
            pending: false,
            request_id: self.id.clone(), url: exchange.url, method: exchange.method,
            resource_type: self.resource_type,
            status: response.as_ref().map_or(0, |r| r.status),
            status_text: self.response_status_texts.get(index).cloned().unwrap_or_default(),
            response_headers: response.as_ref().map(|r| r.headers.clone()).unwrap_or_default(),
            raw_headers: response.as_ref().and_then(|r| r.raw_headers.clone()),
            request_raw_headers: exchange.request_headers.or_else(|| response.as_ref().and_then(|r| r.request_raw_headers.clone())),
            body_size: exchange.body_size,
            timestamp: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs_f64(),
            request_timestamp: exchange.request_started_at,
            request_prepared_timestamp: exchange.request_prepared_at,
            response_headers_timestamp: exchange.response_headers_at,
            error: if redirect { None } else { error },
            request_body_size: exchange.request_body_size,
            request_post_data: exchange.request_post_data,
            request_started: if index == 0 { self.network_start.swap(2, std::sync::atomic::Ordering::SeqCst) == 1 } else {
                self.hop_starts.get(index - 1).is_some_and(|start| start.swap(2, std::sync::atomic::Ordering::SeqCst) == 1)
            }, redirect,
            response_body_request_id: if redirect { body_id } else { body_captured.then(|| self.id.clone()) },
        }
    }

}

impl Drop for NetworkRequest {
    fn drop(&mut self) { self.finish_into(Some("Aborted".into()), true); }
}

fn request_header_capture(headers: impl IntoIterator<Item = (String, String)>) -> obscura_net::HeaderCapture {
    obscura_net::HeaderCapture { capture_stage: "scriptRequest", encoding: "base64",
        fields: headers.into_iter().map(|(name, value)| obscura_net::RawHeader {
            name: name.into_bytes(), value: value.into_bytes(),
        }).collect() }
}

#[op2]
#[string]
fn op_fetch_start(state: &mut OpState) -> String {
    let shared = state.borrow::<SharedState>().clone();
    let mut state = shared.borrow_mut();
    let interception_id = state.intercept_enabled.then(|| {
        let id = state.intercept_counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        format!("intercept-{id}")
    });
    let id = state.network_response_body_counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    let id = format!("fetch-{id}");
    state.fetch_cancellations.insert(id.clone(), (tokio::sync::watch::channel(None).0, interception_id));
    id
}

#[op2(fast)]
fn op_fetch_abort(state: &mut OpState, #[string] id: String, #[string] reason: String) {
    let shared = state.borrow::<SharedState>().clone();
    if let Some((sender, _)) = shared.borrow().fetch_cancellations.get(&id) {
        sender.send_replace(Some(reason));
    };
}

#[op2(fast)]
fn op_fetch_cleanup(state: &mut OpState, #[string] id: String) {
    state.borrow::<SharedState>().borrow_mut().fetch_cancellations.remove(&id);
}

#[op2(async)]
#[string]
async fn op_fetch_url(
    state: Rc<RefCell<OpState>>,
    #[string] url: String,
    #[string] method: String,
    #[string] headers_json: String,
    #[buffer] body: JsBuffer,
    #[string] origin: String,
    #[string] mode: String,
    #[string] credentials: String,
    #[string] options: Option<String>,
) -> Result<String, deno_error::JsErrorBox> {
    let options_json = options.as_deref().and_then(|value| serde_json::from_str::<serde_json::Value>(value).ok());
    let destination = options_json.as_ref().and_then(|value| value["destination"].as_str()).map(str::to_owned)
        .or_else(|| options.filter(|value| !value.starts_with('{')));
    let request_id = options_json.as_ref().and_then(|value| value["requestId"].as_str()).map(str::to_owned);
    let shared = state.borrow().borrow::<SharedState>().clone();
    let resource_type = if options_json.as_ref().and_then(|value| value["resourceType"].as_str()) == Some("XHR") {
        ResourceType::Xhr
    } else if matches!(destination.as_deref(), Some("script" | "worker")) {
        ResourceType::Script
    } else { ResourceType::Fetch };
    let fields = serde_json::from_str::<HashMap<String, String>>(&headers_json).unwrap_or_default();
    let mut observation = NetworkRequest::new(shared.clone(), request_id, &url, &method,
        Some(request_header_capture(fields)), body.len(), resource_type);
    observation.interception_id = shared.borrow().fetch_cancellations.get(&observation.id).and_then(|(_, id)| id.clone());
    let mut cancel = shared.borrow().fetch_cancellations.get(&observation.id).map(|(sender, _)| sender.subscribe());
    let result = {
        let operation = fetch_url_inner(state.clone(), url, method, headers_json, body.to_vec(), origin,
            mode, credentials, destination, resource_type, &mut observation);
        tokio::pin!(operation);
        if let Some(cancel) = cancel.as_mut() {
            tokio::select! {
                biased;
                reason = async {
                    loop {
                        if let Some(reason) = cancel.borrow().clone() { break reason; }
                        if cancel.changed().await.is_err() { break "Aborted".into(); }
                    }
                } => Err(deno_error::JsErrorBox::generic(reason)),
                result = &mut operation => result,
            }
        } else { operation.await }
    };
    let error = match &result {
        Err(error) => Some(error.to_string()),
        Ok(result) => serde_json::from_str::<serde_json::Value>(result).ok().and_then(|value| {
            if value["blocked"] == true || value["corsBlocked"] == true || value["status"] == 0 {
                Some(value["error"].as_str().or(value["corsError"].as_str()).unwrap_or("Blocked").to_string())
            } else { None }
        }),
    };
    observation.finish(error);
    shared.borrow_mut().fetch_cancellations.remove(&observation.id);
    crate::worker::flush_observations(&state.borrow());
    result
}

async fn fetch_url_inner(
    state: Rc<RefCell<OpState>>, url: String, method: String, headers_json: String, body: Vec<u8>,
    origin: String, mode: String, credentials: String, destination: Option<String>, resource_type: ResourceType,
    observation: &mut NetworkRequest,
) -> Result<String, deno_error::JsErrorBox> {
    crate::worker::refresh_policy(&state.borrow());
    tracing::debug!(
        "op_fetch_url called: {} {} (intercept check pending)",
        method,
        url
    );

    let (page_in_flight, intercept_tx, callbacks, http_client, stealth_client, referrer, referrer_policy) = {
        let state_borrow = state.borrow();
        let gs = state_borrow.borrow::<SharedState>().clone();
        let mut gs = gs.borrow_mut();
        for pattern in &gs.blocked_urls {
            if pattern == "*" || url.contains(pattern) || glob_match(pattern, &url) {
                return Ok(serde_json::json!({
                    "status": 0,
                    "body": "",
                    "url": url,
                    "headers": {},
                    "blocked": true,
                })
                .to_string());
            }
        }
        // Record the resource the page pulled in via fetch()/XHR so `--dump
        // assets` can list it (issue #301). URL is already absolute here.
        push_capped(&mut gs.fetched_urls, url.clone(), MAX_FETCHED_URLS);
        let stealth_client = gs.ensure_persona_transport();
        tracing::debug!(
            "op_fetch_url: intercept_enabled={}, has_tx={}",
            gs.intercept_enabled,
            gs.intercept_tx.is_some()
        );
        let request_matches = request_patterns_match(&gs.intercept_request_patterns, &url, resource_type);
        let itx = if gs.intercept_enabled && request_matches {
            let id = observation.interception_id.clone().unwrap_or_else(|| {
                let id = gs.intercept_counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                format!("intercept-{id}")
            });
            gs.intercept_tx.clone().map(|tx| (tx, id))
        } else {
            None
        };
        (
            Arc::clone(&gs.page_in_flight),
            itx,
            gs.callbacks.clone(),
            gs.http_client.clone(),
            stealth_client,
            gs.dom.as_ref().and_then(DomTree::document_url).and_then(|url| url::Url::parse(&url).ok()),
            gs.referrer_policy,
        )
    };
    // The private-network opt-in is a BrowserContext policy, not only a
    // process-wide environment setting.  Navigation already honours the
    // context's configured HTTP client; scripted fetch/XHR must use the same
    // policy for its initial URL and every URL it can reach below.
    let allow_private_network = http_client
        .as_ref()
        .is_some_and(|client| client.allow_private_network);
    if let Ok(parsed_url) = url::Url::parse(&url) {
        if let Err(e) = validate_fetch_url(&parsed_url, allow_private_network) {
            return Ok(serde_json::json!({
                "status": 0,
                "body": "",
                "url": url,
                "headers": {},
                "blocked": true,
                "error": e,
            })
            .to_string());
        }
    }
    page_in_flight.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut page_in_flight_guard = Some(PageInFlightGuard(page_in_flight));

    // Slots the interception channel can override via Continue so a consumer
    // can rewrite url/method/headers/body before the request goes out.
    let mut override_url: Option<String> = None;
    let mut override_method: Option<String> = None;
    let mut override_headers: Option<Vec<(String, String)>> = None;
    let mut override_body: Option<Vec<u8>> = None;

    if let Some((tx, request_id)) = intercept_tx {
        let custom_headers: HashMap<String, String> =
            serde_json::from_str(&headers_json).unwrap_or_default();
        let (resolve_tx, resolve_rx) = tokio::sync::oneshot::channel();
        let intercepted = InterceptedRequest {
            stage: InterceptionStage::Request,
            document_generation: observation.document_generation, document_url: observation.document_url.clone(), redirect_response: None,
            redirected_request_id: None,
            network_id: observation.id.clone(),
            network_start: observation.network_start.clone(),
            request_raw_headers: Some(request_header_capture(custom_headers.clone())),
            request_body_size: body.len(),
            request_id: request_id.clone(),
            url: url.clone(),
            method: method.clone(),
            headers: custom_headers.clone(),
            resource_type: cdp_resource_type(resource_type).into(),
            response_status_code: None, response_headers: None, response_raw_headers: None,
            response_body_request_id: None,
            resolver: resolve_tx,
        };
        if tx.send(intercepted).is_ok() {
            observation.interception_id = Some(request_id.clone());
            let resolution = resolve_rx.await;
            let (raw_headers, status_text) = match &resolution {
                Ok(InterceptResolution::FulfillWithHeaders { raw_headers, status_text, .. }) => {
                    (Some(raw_headers.clone()), status_text.clone())
                }
                _ => (None, None),
            };
            match resolution {
                Ok(InterceptResolution::Fulfill {
                    status,
                    headers: h,
                    body: b,
                    body_base64: bb,
                    ..
                } | InterceptResolution::FulfillWithHeaders {
                    status,
                    headers: h,
                    body: b,
                    body_base64: bb,
                    ..
                }) => {
                    let mut result = intercept_fulfill_response(status, h.clone(), &b, &bb, &url);
                    // Fulfill is synthetic and keeps its existing JS/CORS/redirect
                    // semantics. Capture exactly the bytes handed to JS, without
                    // inventing a transport header capture or response-stage pause.
                    // Internal text producers omit base64; CDP's empty body has
                    // both an empty text view and empty base64.
                    let bytes = if bb.is_empty() { Ok(b.into_bytes()) } else { BASE64.decode(&bb) };
                    if let (Ok(response_url), Ok(bytes)) = (url::Url::parse(&url), bytes) {
                        let response = obscura_net::Response {
                            url: response_url, status, headers: h, body: bytes,
                            raw_headers, request_raw_headers: None,
                            redirected_from: Vec::new(), request_referrer: None,
                        };
                        observation.set_response_status_text(
                            observation.trace.len().saturating_sub(1), status_text,
                        );
                        observation.trace.response(&response, status != 0);
                        observation.finish(None);
                        let shared = state.borrow().borrow::<SharedState>().clone();
                        let _ = shared.borrow().network_response_bodies.lock()
                            .unwrap_or_else(|e| e.into_inner()).alias(&observation.id, &request_id);
                        result["requestId"] = serde_json::Value::String(observation.id.clone());
                    }
                    return Ok(result.to_string());
                }
                Ok(InterceptResolution::Fail { reason }) => {
                    return Ok(serde_json::json!({
                        "status": 0,
                        "body": "",
                        "url": url,
                        "headers": {},
                        "blocked": true,
                        "error": reason,
                    })
                    .to_string());
                }
                Ok(InterceptResolution::Continue {
                    url,
                    method,
                    headers,
                    body,
                }) => {
                    override_url = url;
                    override_method = method;
                    override_headers = headers.map(|headers| headers.into_iter().collect());
                    override_body = body;
                    tracing::debug!(
                        "Interception: continue (overrides url={} method={} headers={} body={})",
                        override_url.is_some(),
                        override_method.is_some(),
                        override_headers.is_some(),
                        override_body.is_some()
                    );
                }
                Ok(InterceptResolution::ContinueWithHeaders { url, method, headers, body }) => {
                    override_url = url;
                    override_method = method;
                    override_headers = Some(headers);
                    override_body = body;
                }
                Ok(InterceptResolution::ContinueResponse { .. }) => {
                    return Err(deno_error::JsErrorBox::generic("continueResponse cannot resolve a request-stage pause"));
                }
                Err(_) => return Err(deno_error::JsErrorBox::generic("Aborted: interception resolver closed")),
            }
        } else {
            return Err(deno_error::JsErrorBox::generic("Aborted: interception channel closed"));
        }
    }

    // Apply interception overrides (shadow the params for the rest of the op).
    // A Continue rewrite of the URL must pass the same SSRF / private-network
    // gate as the original request (checked above) and as redirects (checked
    // below). Without this re-validation a rewrite to an internal address would
    // bypass validate_fetch_url entirely.
    observation.trace.take();
    observation.trace.begin(override_url.as_deref().unwrap_or(&url), override_method.as_deref().unwrap_or(&method),
        Some(request_header_capture(override_headers.clone().unwrap_or_else(||
            serde_json::from_str::<HashMap<String, String>>(&headers_json).unwrap_or_default().into_iter().collect()))),
        override_body.as_ref().unwrap_or(&body).len());
    let url = if let Some(new_url) = override_url {
        if let Ok(parsed) = url::Url::parse(&new_url) {
            if let Err(reason) = validate_fetch_url(&parsed, allow_private_network) {
                return Ok(serde_json::json!({
                    "status": 0,
                    "body": "",
                    "url": new_url,
                    "blocked": true,
                    "error": format!("Intercept rewrite to forbidden URL blocked: {}", reason),
                })
                .to_string());
            }
        }
        new_url
    } else {
        url
    };
    let method = override_method.unwrap_or(method);
    let body = override_body.unwrap_or(body);

    let initial_request_origin = request_origin(&url).unwrap_or_default();
    let page_origin = if origin.is_empty() {
        initial_request_origin.clone()
    } else {
        origin.clone()
    };
    let credentials = FetchCredentials::parse(&credentials);

    let req_method: http::Method = method.parse().unwrap_or(http::Method::GET);

    let mut custom_headers = override_headers.unwrap_or_else(|| {
        serde_json::from_str::<HashMap<String, String>>(&headers_json).unwrap_or_default().into_iter().collect()
    });
    custom_headers.retain(|(key, _)| !key.eq_ignore_ascii_case("referer") && !key.eq_ignore_ascii_case("origin") && !key.to_ascii_lowercase().starts_with("sec-"));

    drop(page_in_flight_guard.take());
    observation.trace.request_body(&body);
    scripted_preflight(state.clone(), &stealth_client, &url, &method, &custom_headers,
        &page_origin, &mode, credentials, referrer.as_ref(), referrer_policy, observation).await?;

    // Redirects and the response body stay inside the persona-owned transport.
    drop(page_in_flight_guard.take());
    stealth_fetch_all(
        state.clone(), stealth_client, url, req_method.as_str().to_string(),
        custom_headers, body, page_origin, mode, credentials, destination,
        resource_type, callbacks, allow_private_network, referrer, referrer_policy, observation,
    ).await
}

async fn scripted_preflight(
    state: Rc<RefCell<OpState>>, stealth_client: &StealthHttpClient, url: &str, method: &str,
    custom_headers: &[(String, String)], page_origin: &str, mode: &str, credentials: FetchCredentials,
    referrer: Option<&url::Url>, referrer_policy: ReferrerPolicy, observation: &mut NetworkRequest,
) -> Result<(), deno_error::JsErrorBox> {
    let is_cross_origin = request_origin(url).is_some_and(|origin| origin != page_origin);
    let req_method: http::Method = method.parse().unwrap_or(http::Method::GET);
    let unsafe_header_names = if is_cross_origin && mode == "cors" {
        // CORS classifies the combined value of repeated, case-insensitive
        // names. Keep this derived view separate from the outgoing field list.
        let mut combined = HashMap::<String, String>::new();
        for (name, value) in custom_headers {
            combined.entry(name.to_ascii_lowercase()).and_modify(|previous| {
                previous.push_str(", "); previous.push_str(value);
            }).or_insert_with(|| value.clone());
        }
        cors_unsafe_request_header_names(&combined)
    } else {
        Vec::new()
    };
    let needs_preflight = is_cross_origin
        && mode == "cors"
        && (!is_cors_safelisted_method(&req_method) || !unsafe_header_names.is_empty());

    if needs_preflight {
        observation.start_before_preflight();
        let parsed_url = url::Url::parse(&url)
            .map_err(|e| deno_error::JsErrorBox::generic(e.to_string()))?;
        let mut headers = HashMap::from([
            ("Origin".to_string(), page_origin.to_string()),
            ("Access-Control-Request-Method".to_string(), method.to_string()),
        ]);
        if !unsafe_header_names.is_empty() {
            headers.insert("Access-Control-Request-Headers".into(), unsafe_header_names.join(","));
        }
        if let Some(value) = referrer_policy.referrer(referrer, &parsed_url) {
            headers.insert("Referer".into(), value.to_string());
        }
        // Preflights are credential-free and may not follow redirects. Hand
        // off pre-send accounting before primp starts counting this request.
        let fields: Vec<_> = headers.into_iter().collect();
        let shared = state.borrow().borrow::<SharedState>().clone();
        let mut preflight = NetworkRequest::new(shared, None, &url, "OPTIONS",
            Some(request_header_capture(fields.clone())), 0, ResourceType::Other);
        preflight.initiator_request_id = Some(observation.id.clone());
        let preflight_result: Result<(), deno_error::JsErrorBox> = async {
        let mut response = tokio::time::timeout(fetch_timeout(), stealth_client.send_single_traced_fields(
            "OPTIONS", &parsed_url, &fields, &[], false, false, fetch_max_body_bytes(), fetch_timeout(), None, Some(&preflight.trace)))
            .await.map_err(|_| deno_error::JsErrorBox::generic("CORS preflight timed out"))?
            .map_err(|e| deno_error::JsErrorBox::generic(format!("CORS preflight failed: {}", e)))?;
        preflight.trace.response(&response, true);
        pause_response_hop(&state, &mut preflight, &mut response).await?;
        let preflight_status = response.status;
        let mut preflight_headers = http::header::HeaderMap::new();
        for (name, value) in response.headers {
            let name = http::header::HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| deno_error::JsErrorBox::generic("invalid CORS preflight header"))?;
            let value = http::header::HeaderValue::from_str(&value)
                .map_err(|_| deno_error::JsErrorBox::generic("invalid CORS preflight header"))?;
            preflight_headers.append(name, value);
        }

        let allowed_origin = preflight_headers
            .get("access-control-allow-origin")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        let allow_credentials = preflight_headers
            .get("access-control-allow-credentials")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if !cors_response_allows(credentials, &page_origin, allowed_origin, allow_credentials) {
            return Err(deno_error::JsErrorBox::generic(format!(
                "CORS preflight: Origin '{}' not allowed by Access-Control-Allow-Origin '{}'",
                page_origin, allowed_origin
            )));
        }
        if !(200..300).contains(&preflight_status) {
            return Err(deno_error::JsErrorBox::generic(format!(
                "CORS preflight returned HTTP {}",
                preflight_status
            )));
        }

        let allowed_methods = parse_cors_header_list(
            &preflight_headers,
            "access-control-allow-methods",
        )
        .ok_or_else(|| {
            deno_error::JsErrorBox::generic(
                "CORS preflight returned an invalid Access-Control-Allow-Methods value",
            )
        })?;
        let allowed_headers = parse_cors_header_list(
            &preflight_headers,
            "access-control-allow-headers",
        )
        .ok_or_else(|| {
            deno_error::JsErrorBox::generic(
                "CORS preflight returned an invalid Access-Control-Allow-Headers value",
            )
        })?;
        let credentialed = credentials == FetchCredentials::Include;
        if !preflight_allows_method(&req_method, &allowed_methods, credentialed) {
            return Err(deno_error::JsErrorBox::generic(format!(
                "CORS preflight did not allow method '{}'",
                req_method
            )));
        }
        if let Some(name) = unsafe_header_names
            .iter()
            .find(|name| !preflight_allows_header(name, &allowed_headers, credentialed))
        {
            return Err(deno_error::JsErrorBox::generic(format!(
                "CORS preflight did not allow request header '{}'",
                name
            )));
        }
            Ok(())
        }.await;
        preflight.finish(preflight_result.as_ref().err().map(ToString::to_string));
        preflight_result?;
    }

    Ok(())
}

struct PageInFlightGuard(Arc<std::sync::atomic::AtomicU32>);
impl Drop for PageInFlightGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Pause after an actual transport response has been read and its exact body
/// has been admitted to the page body store. This runs once per hop, including
/// CORS preflights and redirects, before redirect/CORS processing can hide the
/// response from the CDP client.
async fn pause_response_hop(
    state: &Rc<RefCell<OpState>>,
    observation: &mut NetworkRequest,
    response: &mut obscura_net::Response,
) -> Result<(), deno_error::JsErrorBox> {
    let shared = state.borrow().borrow::<SharedState>().clone();
    let response_bodies = shared.borrow().network_response_bodies.clone();
    let (tx, matches) = {
        let state = shared.borrow();
        let resource_type = if observation.initiator_request_id.is_some() { "Preflight" } else {
            match observation.resource_type {
                ResourceType::Xhr => "XHR",
                ResourceType::Fetch => "Fetch",
                ResourceType::Script => "Script",
                ResourceType::Document => "Document",
                ResourceType::Stylesheet => "Stylesheet",
                ResourceType::Image => "Image",
                ResourceType::Font => "Font",
                ResourceType::Other => "Other",
            }
        };
        (state.intercept_tx.clone(), state.intercept_response_patterns.iter().any(|pattern|
            pattern.resource_type.as_deref().is_none_or(|expected| expected == resource_type)
                && glob_match(&pattern.url_pattern, response.url.as_str())))
    };
    let Some(tx) = tx.filter(|_| matches) else { return Ok(()); };
    let page_in_flight = shared.borrow().page_in_flight.clone();
    page_in_flight.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let _page_in_flight_guard = PageInFlightGuard(page_in_flight);

    let exchange = observation.trace.last().expect("response trace exists before response-stage pause");
    let exchange_index = observation.trace.len().saturating_sub(1);
    let start = if exchange_index == 0 {
        observation.network_start.clone()
    } else {
        while observation.hop_starts.len() < exchange_index {
            let start = Arc::new(std::sync::atomic::AtomicU8::new(0));
            observation.hop_starts.push(start.clone());
            observation.hop_interceptions.push(None);
        }
        observation.hop_starts[exchange_index - 1].clone()
    };
    let id = shared.borrow().intercept_counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    let request_id = format!("intercept-{id}");
    let body_id = exchange.body_request_id.as_ref().ok_or_else(|| {
        let canonical_id = format!("{}-hop-{exchange_index}", observation.id);
        let error = response_bodies.lock().unwrap_or_else(|e| e.into_inner())
            .get(&canonical_id).and_then(Result::err)
            .map(|error| error.to_string())
            .unwrap_or_else(|| "response_body_capture_incomplete".to_string());
        deno_error::JsErrorBox::generic(error)
    })?;
    response_bodies.lock().unwrap_or_else(|e| e.into_inner())
        .alias(body_id, &request_id)
        .map_err(|error| deno_error::JsErrorBox::generic(error.to_string()))?;
    let redirected_request_id = observation.response_interception_id.replace(request_id.clone());
    let (resolver, resolution) = tokio::sync::oneshot::channel();
    tx.send(InterceptedRequest {
        stage: InterceptionStage::Response,
        document_generation: observation.document_generation,
        document_url: observation.document_url.clone(),
        redirect_response: observation.trace.previous(),
        redirected_request_id,
        network_id: observation.id.clone(), network_start: start,
        request_raw_headers: exchange.request_headers.clone(),
        request_body_size: exchange.request_body_size,
        request_id, url: exchange.url, method: exchange.method,
        headers: exchange.request_headers.as_ref().map(|headers| headers.text_headers()).unwrap_or_default(),
        resource_type: if observation.initiator_request_id.is_some() { "Preflight".into() } else { cdp_resource_type(observation.resource_type).into() },
        response_status_code: Some(response.status),
        response_headers: Some(response.headers.clone()),
        response_raw_headers: response.raw_headers.clone(),
        response_body_request_id: exchange.body_request_id,
        resolver,
    }).map_err(|_| deno_error::JsErrorBox::generic("Aborted: interception channel closed"))?;

    match resolution.await.map_err(|_| deno_error::JsErrorBox::generic("Aborted: interception resolver closed"))? {
        InterceptResolution::ContinueResponse { status, status_text, headers, raw_headers } => {
            if let Some(status) = status { response.status = status; }
            observation.set_response_status_text(exchange_index, status_text);
            if let Some(headers) = headers { response.headers = headers; }
            if let Some(raw_headers) = raw_headers { response.raw_headers = Some(raw_headers); }
        }
        InterceptResolution::Continue { url: None, method: None, headers: None, body: None } => {}
        InterceptResolution::Fail { reason } => return Err(deno_error::JsErrorBox::generic(reason)),
        fulfillment @ (InterceptResolution::Fulfill { .. } | InterceptResolution::FulfillWithHeaders { .. }) => {
            let (raw_headers, status_text) = match &fulfillment {
                InterceptResolution::FulfillWithHeaders { raw_headers, status_text, .. } => {
                    (Some(raw_headers.clone()), status_text.clone())
                }
                _ => (None, None),
            };
            let (InterceptResolution::Fulfill { status, headers, body, body_base64, body_supplied }
                | InterceptResolution::FulfillWithHeaders { status, headers, body, body_base64, body_supplied, .. }) = fulfillment else { unreachable!() };
            response.status = status;
            observation.set_response_status_text(exchange_index, status_text);
            response.headers = headers;
            response.raw_headers = raw_headers;
            if body_supplied {
                response.body = if body_base64.is_empty() { body.into_bytes() } else {
                    BASE64.decode(body_base64).map_err(|e| deno_error::JsErrorBox::generic(e.to_string()))?
                };
            }
        }
        _ => return Err(deno_error::JsErrorBox::generic("request overrides cannot resolve a response-stage pause")),
    }
    // Replacing status/headers/body must also replace the retained exchange so
    // Network events and body reads report exactly what the page receives.
    observation.trace.response(response, response.status != 0);
    Ok(())
}

async fn pause_redirect_hop(
    state: &Rc<RefCell<OpState>>, observation: &mut NetworkRequest,
    previous: obscura_net::observation::Exchange,
    url: &mut String, method: &mut String, headers: &mut Vec<(String, String)>, body: &mut Vec<u8>,
    allow_private_network: bool,
) -> Result<Option<obscura_net::Response>, deno_error::JsErrorBox> {
    let start = Arc::new(std::sync::atomic::AtomicU8::new(0));
    let redirected_request_id = observation.hop_interceptions.iter().rev().find_map(Clone::clone)
        .or_else(|| observation.interception_id.clone());
    observation.hop_starts.push(start.clone());
    observation.hop_interceptions.push(None);
    let shared = state.borrow().borrow::<SharedState>().clone();
    let intercept = {
        let state = shared.borrow();
        if state.intercept_enabled
            && request_patterns_match(&state.intercept_request_patterns, url, observation.resource_type)
        {
            state.intercept_tx.clone()
        } else { None }
    };
    let Some(tx) = intercept else { return Ok(None); };
    let page_in_flight = shared.borrow().page_in_flight.clone();
    page_in_flight.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let _page_in_flight_guard = PageInFlightGuard(page_in_flight);
    let id = shared.borrow().intercept_counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    let request_id = format!("intercept-{id}");
    *observation.hop_interceptions.last_mut().unwrap() = Some(request_id.clone());
    let (resolver, resolution) = tokio::sync::oneshot::channel();
    tx.send(InterceptedRequest {
        stage: InterceptionStage::Request,
        document_generation: observation.document_generation, document_url: observation.document_url.clone(),
        redirect_response: Some(previous), redirected_request_id,
        network_id: observation.id.clone(), network_start: start,
        request_raw_headers: Some(request_header_capture(headers.clone())), request_body_size: body.len(),
        request_id, url: url.clone(), method: method.clone(), headers: headers.iter().cloned().collect(),
        resource_type: cdp_resource_type(observation.resource_type).into(),
        response_status_code: None, response_headers: None, response_raw_headers: None,
        response_body_request_id: None, resolver,
    }).map_err(|_| deno_error::JsErrorBox::generic("Aborted: interception channel closed"))?;
    let resolution = resolution.await.map_err(|_| deno_error::JsErrorBox::generic("Aborted: interception resolver closed"))?;
    match resolution {
        InterceptResolution::Fail { reason } => Err(deno_error::JsErrorBox::generic(reason)),
        InterceptResolution::Continue { url: u, method: m, headers: h, body: b } => {
            if let Some(u) = u { *url = u; }
            if let Some(m) = m { *method = m; }
            if let Some(h) = h { *headers = h.into_iter().collect(); }
            if let Some(b) = b { *body = b; }
            observation.trace.update_request(url, method, request_header_capture(headers.clone()), body.len());
            let parsed = url::Url::parse(url).map_err(|e| deno_error::JsErrorBox::generic(e.to_string()))?;
            validate_fetch_url(&parsed, allow_private_network).map_err(deno_error::JsErrorBox::generic)?;
            Ok(None)
        }
        InterceptResolution::ContinueWithHeaders { url: u, method: m, headers: h, body: b } => {
            if let Some(u) = u { *url = u; }
            if let Some(m) = m { *method = m; }
            *headers = h;
            if let Some(b) = b { *body = b; }
            observation.trace.update_request(url, method, request_header_capture(headers.clone()), body.len());
            let parsed = url::Url::parse(url).map_err(|e| deno_error::JsErrorBox::generic(e.to_string()))?;
            validate_fetch_url(&parsed, allow_private_network).map_err(deno_error::JsErrorBox::generic)?;
            Ok(None)
        }
        InterceptResolution::ContinueResponse { .. } => {
            Err(deno_error::JsErrorBox::generic("continueResponse cannot resolve a request-stage pause"))
        }
        fulfillment => {
            let (raw_headers, status_text) = match &fulfillment {
                InterceptResolution::FulfillWithHeaders { raw_headers, status_text, .. } => {
                    (Some(raw_headers.clone()), status_text.clone())
                }
                _ => (None, None),
            };
            let (InterceptResolution::Fulfill { status, headers, body, body_base64, .. }
                | InterceptResolution::FulfillWithHeaders { status, headers, body, body_base64, .. }) = fulfillment else { unreachable!() };
            let bytes = if body_base64.is_empty() { body.into_bytes() } else {
                BASE64.decode(body_base64).map_err(|e| deno_error::JsErrorBox::generic(e.to_string()))?
            };
            observation.set_response_status_text(
                observation.trace.len().saturating_sub(1), status_text,
            );
            Ok(Some(obscura_net::Response {
                url: url::Url::parse(url).map_err(|e| deno_error::JsErrorBox::generic(e.to_string()))?,
                status, headers, body: bytes, raw_headers, request_raw_headers: None,
                redirected_from: Vec::new(), request_referrer: None,
            }))
        }
    }
}

/// Scripted fetch()/XHR over primp: mirrors op_fetch_url's redirect, SSRF,
/// and CORS semantics but sends every hop through the primp stealth client so
/// the request carries the Chrome TLS fingerprint and client hints. Cookie
/// handling lives inside StealthHttpClient::send_single, which shares the
/// context jar and records the same CDP/MCP network observations as every
/// other page request.
async fn stealth_fetch_all(
    state: Rc<RefCell<OpState>>,
    stealth: Arc<StealthHttpClient>,
    url: String,
    method: String,
    mut custom_headers: Vec<(String, String)>,
    body: Vec<u8>,
    page_origin: String,
    mode: String,
    credentials: FetchCredentials,
    destination: Option<String>,
    resource_type: ResourceType,
    callbacks: Option<Arc<CallbackRegistry>>,
    allow_private_network: bool,
    mut referrer: Option<url::Url>,
    mut referrer_policy: ReferrerPolicy,
    observation: &mut NetworkRequest,
) -> Result<String, deno_error::JsErrorBox> {
    let mut current_url = url.clone();
    let mut current_method = method;
    let mut current_body = body;
    let mut redirects_followed: usize = 0;
    let mut redirected_from = Vec::new();
    let mut crossed_origin = request_origin(&current_url)
        .map(|request_origin| request_origin != page_origin)
        .unwrap_or(false);

    let mut response = loop {
        let parsed_current = match url::Url::parse(&current_url) {
            Ok(u) => u,
            Err(error) => {
                return Ok(serde_json::json!({
                    "status": 0, "body": "", "url": current_url, "headers": {}, "error": error.to_string(),
                })
                .to_string());
            }
        };

        let current_is_cross_origin = parsed_current.origin().ascii_serialization() != page_origin;
        crossed_origin |= current_is_cross_origin;
        let mut req_headers: HashMap<String, String> = scripted_fetch_metadata(&page_origin, &current_url, &mode, resource_type)
            .into_iter().map(|(name, value)| (name.into(), value.into())).collect();
        if destination.as_deref() == Some("worker") {
            req_headers.insert("sec-fetch-dest".into(), "worker".into());
        }
        if let Some(origin) = fetch_origin_header(&current_method, &page_origin, &current_url, &mode, referrer_policy, destination.as_deref()) {
            req_headers.insert("origin".to_string(), origin.into());
        }
        if !custom_headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("accept"))
        {
            req_headers.insert("accept".to_string(), "*/*".to_string());
        }
        referrer = referrer_policy.referrer(referrer.as_ref(), &parsed_current);
        if let Some(value) = &referrer { req_headers.insert("referer".into(), value.to_string()); }
        let mut req_headers: Vec<(String, String)> = req_headers.into_iter().collect();
        req_headers.extend(custom_headers.iter().cloned());
        let credentials_allowed = credentials.allows(&page_origin, &current_url);
        observation.trace.request_body(&current_body);
        let mut r = tokio::time::timeout(fetch_timeout(), stealth
            .send_single_traced_fields(
                &current_method,
                &parsed_current,
                &req_headers,
                &current_body,
                credentials_allowed,
                credentials_allowed,
                fetch_max_body_bytes(),
                fetch_timeout(),
                // Preserve one request callback per logical fetch. The response
                // callback carries the final hop's independently captured headers.
                callbacks.as_deref().filter(|_| redirects_followed == 0)
                    .map(|callbacks| (callbacks, resource_type)),
                Some(&observation.trace),
            ))
            .await
            .map_err(|_| deno_error::JsErrorBox::generic("fetch timed out"))?
            .map_err(|e| deno_error::JsErrorBox::generic(e.to_string()))?;

        observation.trace.response(&r, r.status != 0);
        pause_response_hop(&state, observation, &mut r).await?;
        // A local network policy can return status 0 without HTTP response
        // headers. Do not report it as a server CORS rejection.
        if r.status == 0 {
            return Ok(serde_json::json!({
                "status": 0, "body": "", "url": current_url, "headers": {},
                "blocked": true, "error": "Blocked",
            }).to_string());
        }
        if !(300..400).contains(&r.status) {
            break r;
        }
        if current_is_cross_origin && mode == "cors" && !cors_response_allows(credentials, &page_origin,
            r.headers.get("access-control-allow-origin").map(String::as_str).unwrap_or(""),
            r.headers.get("access-control-allow-credentials").map(String::as_str).unwrap_or("")) {
            return Err(deno_error::JsErrorBox::generic(format!("CORS redirect response from {} did not allow origin {}", current_url, page_origin)));
        }
        let Some(location) = r.headers.get("location").cloned() else {
            break r;
        };
        let next_url = match parsed_current.join(&location) {
            Ok(u) => u,
            Err(error) => return Err(deno_error::JsErrorBox::generic(format!("Invalid redirect URL: {}", error))),
        };
        // Re-validate every redirect target against the SSRF policy, matching
        // op_fetch_url (GHSA-8v6v-g4rh-jmcm).
        if let Err(reason) = validate_fetch_url(&next_url, allow_private_network) {
            return Ok(serde_json::json!({
                "status": 0, "body": "", "url": next_url.to_string(), "headers": {},
                "blocked": true,
                "error": format!("Redirect to forbidden URL blocked: {}", reason),
            })
            .to_string());
        }
        redirects_followed += 1;
        if redirects_followed > FETCH_REDIRECT_LIMIT {
            return Ok(serde_json::json!({
                "status": 0, "body": "", "url": next_url.to_string(), "headers": {},
                "blocked": true,
                "error": format!("Too many redirects (>{})", FETCH_REDIRECT_LIMIT),
            })
            .to_string());
        }
        // Browser semantics: 301/302/303 downgrade to GET with no body.
        if r.status == 301 || r.status == 302 || r.status == 303 {
            current_method = "GET".to_string();
            current_body.clear();
        }
        redirected_from.push(parsed_current);
        if let Some(policy) = r.header("referrer-policy").and_then(ReferrerPolicy::from_header) {
            referrer_policy = policy;
        }
        current_url = next_url.to_string();
        let previous = observation.trace.last().expect("redirect response trace");
        observation.trace.begin(&current_url, &current_method, Some(request_header_capture(custom_headers.clone())), current_body.len());
        observation.trace.request_body(&current_body);
        if let Some(response) = pause_redirect_hop(&state, observation, previous, &mut current_url,
            &mut current_method, &mut custom_headers, &mut current_body, allow_private_network).await? {
            observation.trace.response(&response, response.status != 0);
            break response;
        }
        custom_headers.retain(|(key, _)| !key.eq_ignore_ascii_case("referer") && !key.eq_ignore_ascii_case("origin") && !key.to_ascii_lowercase().starts_with("sec-"));
        if mode == "same-origin" && request_origin(&current_url).is_some_and(|origin| origin != page_origin) {
            return Err(deno_error::JsErrorBox::generic("CORS: same-origin request redirected across origins"));
        }
        scripted_preflight(state.clone(), &stealth, &current_url, &current_method, &custom_headers,
            &page_origin, &mode, credentials, referrer.as_ref(), referrer_policy, observation).await?;
    };

    response.redirected_from = redirected_from;
    let status = response.status;
    let resp_headers = &response.headers;
    let resp_bytes = &response.body;
    let final_is_cross_origin = request_origin(&current_url)
        .map(|request_origin| request_origin != page_origin)
        .unwrap_or(false);
    if final_is_cross_origin && mode == "cors" {
        let allowed = resp_headers
            .get("access-control-allow-origin")
            .map(|s| s.as_str())
            .unwrap_or("");
        let allow_credentials = resp_headers
            .get("access-control-allow-credentials")
            .map(|s| s.as_str())
            .unwrap_or("");
        if !cors_response_allows(credentials, &page_origin, allowed, allow_credentials) {
            return Ok(serde_json::json!({
                "status": 0, "body": "", "url": url, "headers": {},
                "corsBlocked": true,
                "corsError": if credentials == FetchCredentials::Include {
                    format!(
                        "CORS error: credentialed request requires Access-Control-Allow-Origin '{}' and Access-Control-Allow-Credentials 'true'",
                        page_origin
                    )
                } else {
                    format!(
                        "CORS error: Origin '{}' not in Access-Control-Allow-Origin '{}'",
                        page_origin, allowed
                    )
                },
            })
            .to_string());
        }
    }

    let resp_body = String::from_utf8_lossy(&resp_bytes).to_string();
    let resp_body_base64 = BASE64.encode(&resp_bytes);
    if let Some(ref cbs) = callbacks {
        if cbs.has_response_callbacks().await {
            let info = RequestInfo {
                raw_headers: response.request_raw_headers.clone(),
                body: current_body.clone(),
                url: response.url.clone(),
                method: current_method.clone(),
                headers: response.request_raw_headers.as_ref().map(|h| h.text_headers()).unwrap_or_default(),
                resource_type,
            };
            cbs.fire_response(&info, &response).await;
        }
    }

    let response_request_id = observation.id.clone();

    Ok(serde_json::json!({
        "status": status,
        "body": resp_body,
        "bodyBase64": resp_body_base64,
        "requestId": response_request_id,
        "url": current_url,
        "redirected": redirects_followed > 0,
        "opaque": mode == "no-cors" && crossed_origin,
        "headers": resp_headers,
    })
    .to_string())
}

pub(crate) fn glob_match(pattern: &str, url: &str) -> bool {
    if pattern == "*" {
        return true;
    }

    let mut remainder = url;
    let mut first = true;
    for part in pattern.split('*') {
        if part.is_empty() {
            continue;
        }

        let Some(index) = remainder.find(part) else {
            return false;
        };

        if first && !pattern.starts_with('*') && index != 0 {
            return false;
        }

        remainder = &remainder[index + part.len()..];
        first = false;
    }

    pattern.ends_with('*') || remainder.is_empty()
}

#[cfg(test)]
mod tests {
    use super::{
        cors_response_allows, cors_unsafe_request_header_names, glob_match,
        is_cors_safelisted_content_type, is_cors_safelisted_request_header,
        parse_cors_header_list, preflight_allows_header, preflight_allows_method,
        scripted_fetch_metadata, validate_fetch_url, FetchCredentials, ObscuraState,
    };
    use crate::runtime::ObscuraJsRuntime;
    use obscura_dom::parse_html;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;

    #[cfg(feature = "render")]
    use super::{
        ensure_prepared_geometry, ensure_prepared_render, node_is_connected,
        queue_retained_style_mutation, retained_style_mutation,
        shadow_including_connected_nodes, MAX_PENDING_STYLE_MUTATIONS,
    };
    #[cfg(feature = "render")]
    use obscura_dom::ShadowRootMode;

    use super::{pbkdf2_derive, push_capped, PBKDF2_MAX_ITERATIONS, PBKDF2_MAX_OUTPUT_BYTES};
    use super::intercept_fulfill_response;
    use base64::{engine::general_purpose::STANDARD as FULFILL_BASE64, Engine as _};

    #[test]
    fn cors_request_header_safelist_checks_values() {
        assert!(is_cors_safelisted_content_type(
            "application/x-www-form-urlencoded;charset=UTF-8"
        ));
        assert!(is_cors_safelisted_content_type(
            "multipart/form-data; boundary=test"
        ));
        assert!(is_cors_safelisted_content_type("text/plain"));
        assert!(!is_cors_safelisted_content_type("application/json"));
        assert!(!is_cors_safelisted_content_type("text /plain"));

        assert!(is_cors_safelisted_request_header(
            "Accept-Language",
            "en-US, en;q=0.9"
        ));
        assert!(!is_cors_safelisted_request_header(
            "Accept-Language",
            "en_US"
        ));
        assert!(!is_cors_safelisted_request_header(
            "Accept",
            &"a".repeat(129)
        ));
        assert!(is_cors_safelisted_request_header("Range", "bytes=0-499"));
        assert!(is_cors_safelisted_request_header("Range", "bytes=500-"));
        assert!(!is_cors_safelisted_request_header("Range", "bytes=-500"));
        assert!(!is_cors_safelisted_request_header("Range", "bytes=500-499"));
        assert!(!is_cors_safelisted_request_header(
            "Range",
            "bytes=0-1,4-5"
        ));
    }

    #[test]
    fn scripted_fetch_uses_chrome_incremental_priority() {
        let headers = scripted_fetch_metadata(
            "https://example.com",
            "https://example.com/api",
            "cors",
            obscura_net::ResourceType::Fetch,
        );
        assert!(headers.contains(&("priority", "u=1, i")));
        assert!(headers.contains(&("sec-fetch-dest", "empty")));
        let script_headers = scripted_fetch_metadata(
            "https://example.com",
            "https://example.com/script.js",
            "no-cors",
            obscura_net::ResourceType::Script,
        );
        assert!(script_headers.contains(&("sec-fetch-dest", "script")));
        assert!(script_headers.contains(&("sec-fetch-mode", "no-cors")));
    }

    #[test]
    fn cors_unsafe_header_names_are_lowercase_sorted_and_only_include_unsafe_headers() {
        let headers = HashMap::from([
            ("Content-Type".to_string(), "application/json".to_string()),
            ("X-Trace".to_string(), "1".to_string()),
            ("Accept".to_string(), "text/html".to_string()),
            ("Range".to_string(), "bytes=0-99".to_string()),
        ]);
        assert_eq!(
            cors_unsafe_request_header_names(&headers),
            vec!["content-type", "x-trace"]
        );
    }

    #[test]
    fn cors_safelist_aggregate_cap_forces_preflight() {
        let mut headers = HashMap::new();
        for bits in 0..9u8 {
            let name = "accept"
                .bytes()
                .enumerate()
                .map(|(index, byte)| {
                    if bits & (1 << index) == 0 {
                        byte as char
                    } else {
                        (byte as char).to_ascii_uppercase()
                    }
                })
                .collect::<String>();
            headers.insert(name, "a".repeat(128));
        }
        assert_eq!(cors_unsafe_request_header_names(&headers), vec!["accept"]);
    }

    #[test]
    fn cors_preflight_permissions_follow_credentials_and_authorization_rules() {
        assert!(preflight_allows_method(
            &http::Method::POST,
            &[],
            true
        ));
        assert!(preflight_allows_method(
            &http::Method::DELETE,
            &["DELETE"],
            true
        ));
        assert!(!preflight_allows_method(
            &http::Method::DELETE,
            &["delete"],
            false
        ));
        assert!(preflight_allows_method(
            &http::Method::DELETE,
            &["*"],
            false
        ));
        assert!(!preflight_allows_method(
            &http::Method::DELETE,
            &["*"],
            true
        ));

        assert!(preflight_allows_header(
            "Authorization",
            &["authorization"],
            true
        ));
        assert!(!preflight_allows_header(
            "Authorization",
            &["*"],
            false
        ));
        assert!(preflight_allows_header("X-Trace", &["*"], false));
        assert!(!preflight_allows_header("X-Trace", &["*"], true));
    }

    #[test]
    fn cors_preflight_rejects_malformed_permission_lists() {
        let mut headers = http::header::HeaderMap::new();
        headers.append(
            "access-control-allow-methods",
            "GET, DELETE".parse().unwrap(),
        );
        headers.append(
            "access-control-allow-methods",
            "PATCH".parse().unwrap(),
        );
        assert_eq!(
            parse_cors_header_list(&headers, "access-control-allow-methods"),
            Some(vec!["GET", "DELETE", "PATCH"])
        );

        headers.append(
            "access-control-allow-methods",
            "@invalid".parse().unwrap(),
        );
        assert!(parse_cors_header_list(&headers, "access-control-allow-methods").is_none());
    }

    // #912 — a fulfilled binary body (non-UTF-8) must survive as exact bytes via
    // `bodyBase64`, not be silently corrupted by the lossy `body` text view.
    #[test]
    fn intercept_fulfill_carries_binary_body_as_base64() {
        let raw = [0x89u8, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0xFF, 0xFE, 0x00];
        let b64 = FULFILL_BASE64.encode(raw);
        let lossy = String::from_utf8_lossy(&raw).to_string();
        let result = intercept_fulfill_response(
            200,
            std::collections::HashMap::new(),
            &lossy,
            &b64,
            "https://example.test/",
        );
        let out_b64 = result["bodyBase64"]
            .as_str()
            .expect("fulfill result must carry bodyBase64");
        let decoded = FULFILL_BASE64
            .decode(out_b64)
            .expect("bodyBase64 must be valid base64");
        assert_eq!(decoded, raw, "the exact bytes must survive via bodyBase64");
    }

    // SEC-002 / #705 — fetched_urls (and the like) must not grow without bound.
    #[test]
    fn push_capped_bounds_the_list_and_keeps_the_newest() {
        let mut list = Vec::new();
        for i in 0..10 {
            push_capped(&mut list, format!("u{i}"), 4);
        }
        assert_eq!(list.len(), 4, "the list must be capped at max");
        assert_eq!(
            list,
            vec!["u6", "u7", "u8", "u9"],
            "the newest entries must be kept, oldest evicted",
        );
    }

    // SEC-006 / #580 — PBKDF2 parameters arrive straight from page JS. Without
    // caps, a huge iteration count pins the single-threaded runtime and a huge
    // output length forces an unbounded allocation. The derivation must reject
    // both above the fixed maximums, and still work for ordinary inputs.

    #[test]
    fn pbkdf2_rejects_excessive_iterations() {
        let err = pbkdf2_derive("SHA-256", b"pw", b"salt", PBKDF2_MAX_ITERATIONS + 1, 32)
            .expect_err("iteration count above the cap must be rejected");
        assert!(
            err.to_string().contains("iteration"),
            "error should name the iteration cap: {err}"
        );
    }

    #[test]
    fn pbkdf2_rejects_excessive_output_length() {
        let err = pbkdf2_derive("SHA-256", b"pw", b"salt", 1_000, PBKDF2_MAX_OUTPUT_BYTES + 1)
            .expect_err("output length above the cap must be rejected");
        assert!(
            err.to_string().contains("length"),
            "error should name the length cap: {err}"
        );
    }

    #[test]
    fn pbkdf2_derives_within_limits() {
        let dk = pbkdf2_derive("SHA-256", b"password", b"salt", 1_000, 32)
            .expect("ordinary parameters must derive successfully");
        assert_eq!(dk.len(), 32, "derived key must have the requested length");
    }

    // HKDF and the CSPRNG draw share the same DoS shape: both size an output
    // buffer straight from an untrusted u32 length. Each must reject a length
    // above its fixed maximum before allocating, and still work for ordinary
    // inputs. See #910.
    use super::{hkdf_derive, random_bytes, HKDF_MAX_OUTPUT_BYTES, RANDOM_BYTES_MAX};

    #[test]
    fn hkdf_rejects_excessive_output_length() {
        let err = hkdf_derive("SHA-256", b"ikm", b"salt", b"info", HKDF_MAX_OUTPUT_BYTES + 1)
            .expect_err("output length above the cap must be rejected");
        assert!(
            err.to_string().contains("exceeds"),
            "error should name the length cap: {err}"
        );
    }

    #[test]
    fn hkdf_derives_within_limits() {
        let okm = hkdf_derive("SHA-256", b"ikm", b"salt", b"info", 32)
            .expect("ordinary parameters must derive successfully");
        assert_eq!(okm.len(), 32, "derived key must have the requested length");
    }

    #[test]
    fn random_bytes_rejects_excessive_length() {
        let err = random_bytes(RANDOM_BYTES_MAX + 1)
            .expect_err("a draw above the cap must be rejected");
        assert!(
            err.to_string().contains("exceeds"),
            "error should name the length cap: {err}"
        );
    }

    #[test]
    fn random_bytes_within_limits() {
        let buf = random_bytes(32).expect("an ordinary draw must succeed");
        assert_eq!(buf.len(), 32, "draw must return the requested length");
    }


    // SEC-005 / #581 — op_fetch_url must not buffer an unbounded response body.
    // The primp fetch reader streams the body and refuses anything larger than the
    // cap, covering a server that just keeps sending with no Content-Length.

    async fn serve_body_once(body_len: usize, with_content_length: bool) -> std::net::SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut header = String::from("HTTP/1.1 200 OK\r\nConnection: close\r\n");
            if with_content_length {
                header.push_str(&format!("Content-Length: {body_len}\r\n"));
            }
            header.push_str("\r\n");
            let _ = sock.write_all(header.as_bytes()).await;
            let chunk = vec![b'a'; 64 * 1024];
            let mut sent = 0;
            while sent < body_len {
                let n = std::cmp::min(chunk.len(), body_len - sent);
                if sock.write_all(&chunk[..n]).await.is_err() {
                    break;
                }
                sent += n;
            }
            let _ = sock.shutdown().await;
        });
        addr
    }

    #[tokio::test]
    async fn read_body_capped_rejects_oversized_streamed_body() {
        // No Content-Length forces the streaming-cap branch (lying/chunked server).
        let addr = serve_body_once(4 * 1024 * 1024, false).await;
        let client = obscura_net::StealthHttpClient::with_proxy(
            std::sync::Arc::new(obscura_net::CookieJar::new()), None, true,
            &obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145),
        );
        let err = client.send_single_with_limit("GET", &url::Url::parse(&format!("http://{addr}/")).unwrap(),
            &Default::default(), &[], false, false, 1024 * 1024, std::time::Duration::from_secs(30))
            .await
            .expect_err("a body larger than the cap must be rejected");
        assert!(
            err.to_string().contains("exceeded"),
            "error should mention the cap: {err}"
        );
    }

    #[tokio::test]
    async fn read_body_capped_reads_body_within_cap() {
        let addr = serve_body_once(1024, true).await;
        let client = obscura_net::StealthHttpClient::with_proxy(
            std::sync::Arc::new(obscura_net::CookieJar::new()), None, true,
            &obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145),
        );
        let response = client.send_single_with_limit("GET", &url::Url::parse(&format!("http://{addr}/")).unwrap(),
            &Default::default(), &[], false, false, 1024 * 1024, std::time::Duration::from_secs(30))
            .await
            .expect("a small body must be read successfully");
        assert_eq!(response.body.len(), 1024, "should read the whole small body");
    }

    #[test]
    fn glob_match_handles_cdp_blocked_url_patterns() {
        assert!(glob_match(
            "*://*.google.com/maps/vt/*",
            "https://www.google.com/maps/vt/pb=!1m4!1m3",
        ));
        assert!(glob_match(
            "*://*.gstatic.com/*.woff2",
            "https://fonts.gstatic.com/s/inter/v18/font.woff2",
        ));
        assert!(glob_match(
            "https://example.com/assets/*",
            "https://example.com/assets/app.js",
        ));
        assert!(!glob_match(
            "https://example.com/assets/*",
            "https://cdn.example.com/assets/app.js",
        ));
        assert!(!glob_match(
            "*://*.gstatic.com/*.woff2",
            "https://fonts.gstatic.com/s/inter/v18/font.woff",
        ));
    }

    #[test]
    fn fetch_credentials_gate_cookie_send_and_storage_per_request_origin() {
        let page_origin = "https://www.example.com";
        let same_origin_url = "https://www.example.com/api";
        let explicit_default_port = "https://www.example.com:443/api";
        let cross_origin_url = "https://api.example.com/data";

        assert!(!FetchCredentials::Omit.allows(page_origin, same_origin_url));
        assert!(!FetchCredentials::Omit.allows(page_origin, cross_origin_url));

        assert!(FetchCredentials::SameOrigin.allows(page_origin, same_origin_url));
        assert!(FetchCredentials::SameOrigin.allows(page_origin, explicit_default_port));
        assert!(!FetchCredentials::SameOrigin.allows(page_origin, cross_origin_url));

        assert!(FetchCredentials::Include.allows(page_origin, same_origin_url));
        assert!(FetchCredentials::Include.allows(page_origin, cross_origin_url));
    }

    #[test]
    fn credentialed_cors_requires_exact_origin_and_allow_credentials() {
        let page_origin = "https://www.example.com";

        assert!(cors_response_allows(
            FetchCredentials::SameOrigin,
            page_origin,
            "*",
            "",
        ));
        assert!(!cors_response_allows(
            FetchCredentials::Include,
            page_origin,
            "*",
            "true",
        ));
        assert!(!cors_response_allows(
            FetchCredentials::Include,
            page_origin,
            page_origin,
            "",
        ));
        assert!(cors_response_allows(
            FetchCredentials::Include,
            page_origin,
            page_origin,
            "true",
        ));
    }

    #[test]
    fn fetch_url_validation_honors_per_context_private_network_opt_in() {
        let loopback = url::Url::parse("http://127.0.0.1:8080/resource").unwrap();
        assert!(validate_fetch_url(&loopback, true).is_ok());
    }

    // SEC-005 / #708 — fetch() must not accept file:// (deny-by-default, matching
    // Page.navigate / Target.createTarget). The transports can't fetch it, but
    // it should be rejected up front rather than short-circuiting the gate.
    #[test]
    fn fetch_url_validation_rejects_file_scheme() {
        let file = url::Url::parse("file:///etc/passwd").unwrap();
        // Rejected even with private-network access granted.
        let err = validate_fetch_url(&file, true)
            .expect_err("file:// must be rejected by the fetch scheme gate");
        assert!(
            err.to_lowercase().contains("scheme"),
            "error should name the forbidden scheme: {err}"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn posted_task_chains_complete_without_zero_delay_timer_floor() {
        let mut runtime = ObscuraJsRuntime::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));
        runtime.set_dom(parse_html("<html><body></body></html>"));
        runtime.set_url("http://example.com/posted-task-test");
        runtime.run_page_init();
        runtime
            .execute_script(
                "posted-task-throughput",
                r#"
                    globalThis.__postedTaskBench = {
                        message: 0,
                        postTask: 0,
                        yields: 0,
                        started: performance.now(),
                        finished: 0,
                    };
                    const markFinished = () => {
                        if (__postedTaskBench.message === 100 &&
                            __postedTaskBench.postTask === 100 &&
                            __postedTaskBench.yields === 100) {
                            __postedTaskBench.finished = performance.now();
                        }
                    };

                    const channel = new MessageChannel();
                    channel.port2.onmessage = () => {
                        __postedTaskBench.message++;
                        if (__postedTaskBench.message < 100) channel.port1.postMessage(null);
                        else markFinished();
                    };
                    channel.port1.postMessage(null);

                    const postNext = () => scheduler.postTask(() => {
                        __postedTaskBench.postTask++;
                        if (__postedTaskBench.postTask < 100) postNext();
                        else markFinished();
                    });
                    postNext();

                    scheduler.postTask(async () => {
                        while (__postedTaskBench.yields < 100) {
                            await scheduler.yield();
                            __postedTaskBench.yields++;
                        }
                        markFinished();
                    });
                "#,
            )
            .unwrap();

        runtime.run_event_loop_bounded(100).await.unwrap();
        let result = runtime
            .evaluate(
                r#"[
                    __postedTaskBench.message,
                    __postedTaskBench.postTask,
                    __postedTaskBench.yields,
                    __postedTaskBench.finished - __postedTaskBench.started,
                ]"#,
            )
            .unwrap();
        let values = result.as_array().unwrap();
        assert!(
            values[..3].iter().all(|value| value.as_f64() == Some(100.0)),
            "posted-task chains did not finish inside the 100ms pump: {result}",
        );
        assert!(
            values[3].as_f64().is_some_and(|elapsed| elapsed >= 0.0 && elapsed < 75.0),
            "300 chained posted-task deliveries retained timer-wheel latency: {result}",
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn shared_posted_task_queue_preserves_priority_fifo_and_microtasks() {
        let mut runtime = ObscuraJsRuntime::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));
        runtime.set_dom(parse_html("<html><body></body></html>"));
        runtime.set_url("http://example.com/posted-task-order");
        runtime.run_page_init();
        runtime
            .execute_script(
                "shared-posted-task-order",
                r#"
                    globalThis.__sharedPostedOrder = ["sync"];
                    const channel = new MessageChannel();
                    channel.port2.onmessage = event => {
                        __sharedPostedOrder.push("message-" + event.data);
                        Promise.resolve().then(() => {
                            __sharedPostedOrder.push("message-" + event.data + "-microtask");
                        });
                    };
                    channel.port1.postMessage(1);
                    scheduler.postTask(() => {
                        __sharedPostedOrder.push("visible");
                        Promise.resolve().then(() => __sharedPostedOrder.push("visible-microtask"));
                    });
                    channel.port1.postMessage(2);
                    scheduler.postTask(() => {
                        __sharedPostedOrder.push("background");
                    }, { priority: "background" });
                    scheduler.postTask(() => {
                        __sharedPostedOrder.push("blocking");
                        Promise.resolve().then(() => __sharedPostedOrder.push("blocking-microtask"));
                    }, { priority: "user-blocking" });
                    Promise.resolve().then(() => __sharedPostedOrder.push("initial-microtask"));
                "#,
            )
            .unwrap();

        runtime.run_event_loop_bounded(100).await.unwrap();
        assert_eq!(
            runtime.evaluate("__sharedPostedOrder").unwrap(),
            serde_json::json!([
                "sync",
                "initial-microtask",
                "blocking",
                "blocking-microtask",
                "message-1",
                "message-1-microtask",
                "visible",
                "visible-microtask",
                "message-2",
                "message-2-microtask",
                "background",
            ]),
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn bulk_posted_task_batch_completes() {
        let mut runtime = ObscuraJsRuntime::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));
        runtime.set_dom(parse_html("<html><body></body></html>"));
        runtime.set_url("http://example.com/posted-task-bulk");
        runtime.run_page_init();
        runtime
            .execute_script(
                "posted-task-bulk",
                r#"
                    globalThis.__bulkPosted = { count: 0 };
                    const tasks = [];
                    for (let i = 0; i < 4096; i++) {
                        tasks.push(scheduler.postTask(() => __bulkPosted.count++));
                    }
                    Promise.all(tasks);
                "#,
            )
            .unwrap();

        runtime.run_event_loop_bounded(500).await.unwrap();
        let result = runtime.evaluate("__bulkPosted").unwrap();
        assert_eq!(result["count"].as_f64(), Some(4096.0));
    }

    /// A network-op Promise reaction can schedule browser work while
    /// deno_core is dispatching an async-op result batch. Posted tasks must not
    /// recursively submit another async op through that borrowed driver.
    #[tokio::test(flavor = "current_thread")]
    async fn posted_task_from_async_op_resolution_avoids_driver_submission() {
        let mut runtime = ObscuraJsRuntime::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));
        runtime.set_dom(parse_html("<html><body></body></html>"));
        runtime.set_url("http://example.com/posted-task-from-async-op");
        runtime.run_page_init();
        runtime
            .execute_script(
                "posted-task-from-op-resolution",
                r#"
                    globalThis.__postedFromOp = 0;
                    Deno.core.ops.op_sleep(0).then(() => {
                        const rearm = () => scheduler.postTask(() => {
                            __postedFromOp++;
                            if (__postedFromOp < 250) rearm();
                        });
                        rearm();
                    });
                "#,
            )
            .unwrap();

        runtime.run_event_loop_bounded(300).await.unwrap();
        assert_eq!(
            runtime.evaluate("__postedFromOp").unwrap(),
            serde_json::json!(250.0),
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn posted_task_is_cancelled_when_its_document_is_replaced() {
        let mut runtime = ObscuraJsRuntime::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));
        runtime.set_dom(parse_html("<html><body data-document='old'></body></html>"));
        runtime.set_url("http://example.com/posted-task-old-document");
        runtime.run_page_init();
        runtime
            .execute_script(
                "posted-task-old-document",
                "const staleController = new AbortController();\
                 scheduler.postTask(\
                   () => document.body.setAttribute('data-stale-task', 'ran'),\
                   { signal: staleController.signal });",
            )
            .unwrap();

        runtime.set_dom(parse_html("<html><body data-document='new'></body></html>"));
        runtime
            .execute_script(
                "posted-task-new-document",
                "scheduler.postTask(() => document.body.setAttribute('data-fresh-task', 'ran'));",
            )
            .unwrap();
        runtime.run_event_loop_bounded(100).await.unwrap();

        assert_eq!(
            runtime
                .evaluate("document.body.getAttribute('data-stale-task')")
                .unwrap(),
            serde_json::Value::Null,
        );
        assert_eq!(
            runtime
                .evaluate("document.body.getAttribute('data-document')")
                .unwrap(),
            serde_json::json!("new"),
        );
        assert_eq!(
            runtime
                .evaluate("document.body.getAttribute('data-fresh-task')")
                .unwrap(),
            serde_json::json!("ran"),
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn delayed_posted_task_keeps_its_creation_document_generation() {
        let mut runtime = ObscuraJsRuntime::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));
        runtime.set_dom(parse_html("<html><body data-document='old'></body></html>"));
        runtime.set_url("http://example.com/delayed-posted-task-old-document");
        runtime.run_page_init();
        runtime
            .execute_script(
                "delayed-posted-task-old-document",
                "scheduler.postTask(\
                   () => document.body.setAttribute('data-delayed-stale-task', 'ran'),\
                   { delay: 1 });",
            )
            .unwrap();

        runtime.set_dom(parse_html("<html><body data-document='new'></body></html>"));
        runtime.run_event_loop_bounded(100).await.unwrap();

        assert_eq!(
            runtime
                .evaluate("document.body.getAttribute('data-delayed-stale-task')")
                .unwrap(),
            serde_json::Value::Null,
        );
    }

    #[test]
    fn posted_task_owner_contention_is_panic_safe() {
        let owner = Rc::new(RefCell::new(ObscuraState::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145))));
        let weak = Rc::downgrade(&owner);
        let generation = owner.borrow().document_generation;

        let held = owner.borrow_mut();
        assert_eq!(
            super::posted_task_owner_status(&weak),
            super::PostedTaskOwnerStatus::Busy,
        );
        drop(held);
        assert_eq!(
            super::posted_task_owner_status(&weak),
            super::PostedTaskOwnerStatus::Generation(generation),
        );
        drop(owner);
        assert_eq!(
            super::posted_task_owner_status(&weak),
            super::PostedTaskOwnerStatus::Gone,
        );
    }

    #[cfg(feature = "render")]
    #[test]
    fn connected_shadow_nodes_invalidate_without_entering_light_tree_retention() {
        let dom = parse_html(
            r#"<x-host id="host"></x-host><div id="source"><span id="shadow-child"></span></div>"#,
        );
        let host = dom.get_element_by_id("host").unwrap();
        let source = dom.get_element_by_id("source").unwrap();
        let child = dom.get_element_by_id("shadow-child").unwrap();
        let root = dom
            .attach_shadow_root(host, ShadowRootMode::Open)
            .unwrap();
        dom.append_child(root, child);

        assert!(node_is_connected(&dom, child));
        assert!(shadow_including_connected_nodes(&dom).contains(&child));
        assert!(
            retained_style_mutation(&dom, "set_attribute", &child.index().to_string(), "class\0changed")
                .is_none(),
            "shadow mutations require a full scoped cascade"
        );

        dom.append_child(source, host);
        assert!(node_is_connected(&dom, child));
        dom.remove(source);
        assert!(!node_is_connected(&dom, child));
        assert!(!shadow_including_connected_nodes(&dom).contains(&child));
    }

    #[cfg(feature = "render")]
    #[test]
    fn repeated_inline_style_writes_share_one_retained_dirty_marker_per_node() {
        let mut pending = Vec::new();
        let style_mutation = |raw| {
            obscura_render::RetainedStyleMutation::Attribute(
                obscura_render::AttributeStyleMutation {
                    node: obscura_dom::tree::NodeId::new(raw),
                    name: "style".to_string(),
                    old_value: None,
                    new_value: None,
                },
            )
        };

        // Motion/React commonly writes a connected element's serialized style
        // twice in one commit. The old queue reached its 256-record ceiling at
        // only 128 elements and discarded the complete PreparedRender.
        for raw in 1..=200 {
            assert!(queue_retained_style_mutation(
                &mut pending,
                style_mutation(raw),
            ));
            assert!(queue_retained_style_mutation(
                &mut pending,
                style_mutation(raw),
            ));
        }
        assert_eq!(pending.len(), 200);

        // The memory bound remains real: unique dirty nodes still consume one
        // slot, while an already-recorded node remains safe at the ceiling.
        for raw in 201..=MAX_PENDING_STYLE_MUTATIONS as u32 {
            assert!(queue_retained_style_mutation(
                &mut pending,
                style_mutation(raw),
            ));
        }
        assert_eq!(pending.len(), MAX_PENDING_STYLE_MUTATIONS);
        assert!(queue_retained_style_mutation(
            &mut pending,
            style_mutation(1),
        ));
        assert!(!queue_retained_style_mutation(
            &mut pending,
            style_mutation(MAX_PENDING_STYLE_MUTATIONS as u32 + 1),
        ));
        assert_eq!(pending.len(), MAX_PENDING_STYLE_MUTATIONS);
    }

    #[cfg(feature = "render")]
    #[test]
    fn repeated_selector_attribute_writes_keep_only_the_rendered_transition() {
        let node = obscura_dom::tree::NodeId::new(7);
        let mutation = |old: &str, new: &str| {
            obscura_render::RetainedStyleMutation::Attribute(
                obscura_render::AttributeStyleMutation {
                    node,
                    name: "class".to_string(),
                    old_value: Some(old.to_string()),
                    new_value: Some(new.to_string()),
                },
            )
        };
        let mut pending = Vec::new();
        assert!(queue_retained_style_mutation(
            &mut pending,
            mutation("before", "intermediate"),
        ));
        assert!(queue_retained_style_mutation(
            &mut pending,
            mutation("intermediate", "after"),
        ));
        assert_eq!(
            pending,
            vec![obscura_render::RetainedStyleMutation::Attribute(
                obscura_render::AttributeStyleMutation {
                    node,
                    name: "class".to_string(),
                    old_value: Some("before".to_string()),
                    new_value: Some("after".to_string()),
                }
            )]
        );
    }

    #[cfg(feature = "render")]
    #[test]
    fn repeated_animation_changes_share_one_retained_dirty_marker_per_node() {
        let mut pending = Vec::new();
        let first = obscura_dom::tree::NodeId::new(1);
        let second = obscura_dom::tree::NodeId::new(2);
        for _ in 0..300 {
            assert!(queue_retained_style_mutation(
                &mut pending,
                obscura_render::RetainedStyleMutation::Animation { node: first },
            ));
        }
        assert!(queue_retained_style_mutation(
            &mut pending,
            obscura_render::RetainedStyleMutation::Animation { node: second },
        ));
        assert_eq!(
            pending,
            vec![
                obscura_render::RetainedStyleMutation::Animation { node: first },
                obscura_render::RetainedStyleMutation::Animation { node: second },
            ]
        );
    }

    #[cfg(feature = "render")]
    #[test]
    fn repeated_resource_changes_share_one_retained_refresh_marker() {
        let mut pending = vec![obscura_render::RetainedStyleMutation::Animation {
            node: obscura_dom::tree::NodeId::new(1),
        }];
        for _ in 0..300 {
            assert!(queue_retained_style_mutation(
                &mut pending,
                obscura_render::RetainedStyleMutation::Resource,
            ));
        }
        assert_eq!(
            pending,
            vec![
                obscura_render::RetainedStyleMutation::Animation {
                    node: obscura_dom::tree::NodeId::new(1),
                },
                obscura_render::RetainedStyleMutation::Resource,
            ]
        );

        let mut full_style_batch = (1..=MAX_PENDING_STYLE_MUTATIONS)
            .map(|raw| obscura_render::RetainedStyleMutation::Animation {
                node: obscura_dom::tree::NodeId::new(raw as u32),
            })
            .collect::<Vec<_>>();
        assert!(queue_retained_style_mutation(
            &mut full_style_batch,
            obscura_render::RetainedStyleMutation::Resource,
        ));
        assert_eq!(full_style_batch.len(), MAX_PENDING_STYLE_MUTATIONS + 1);
        assert!(!queue_retained_style_mutation(
            &mut full_style_batch,
            obscura_render::RetainedStyleMutation::Animation {
                node: obscura_dom::tree::NodeId::new(5_000),
            },
        ));
    }

    #[cfg(feature = "render")]
    #[test]
    fn geometry_consumer_defers_paint_only_sample_until_exact_consumer() {
        let dom = parse_html(
            r#"<style>
                @keyframes fade { from { opacity:0 } to { opacity:1 } }
                #box { width:40px;height:20px;animation:fade 1000ms linear both }
            </style><div id="box"></div>"#,
        );
        let box_node = dom.get_element_by_id("box").unwrap();
        let mut state = ObscuraState::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));
        state.dom = Some(dom);
        state.animation_sample = obscura_render::AnimationSample::document(0.0);
        ensure_prepared_render(&mut state).expect("initial render");
        assert_eq!(
            state.prepared_render.as_ref().unwrap().layout().styles[&box_node].opacity,
            Some(0.0),
        );

        state.animation_sample = obscura_render::AnimationSample::document(500.0);
        let geometry = ensure_prepared_geometry(&mut state).expect("retained geometry");
        assert_eq!(geometry.animation_sample_time().milliseconds, 0.0);
        assert_eq!(geometry.document_rect(box_node).unwrap().width, 40.0);
        assert_eq!(geometry.layout().styles[&box_node].opacity, Some(0.0));

        let exact = ensure_prepared_render(&mut state).expect("exact sampled style");
        assert_eq!(exact.animation_sample_time().milliseconds, 500.0);
        let opacity = exact.layout().styles[&box_node].opacity.unwrap();
        assert!((opacity - 0.5).abs() < 0.01, "exact opacity was {opacity}");
        assert_eq!(exact.document_rect(box_node).unwrap().width, 40.0);
    }

    #[cfg(feature = "render")]
    #[test]
    fn geometry_consumer_materializes_geometry_animation_sample() {
        let dom = parse_html(
            r#"<style>
                @keyframes grow { from { width:20px } to { width:100px } }
                #box { height:20px;animation:grow 1000ms linear both }
            </style><div id="box"></div>"#,
        );
        let box_node = dom.get_element_by_id("box").unwrap();
        let mut state = ObscuraState::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));
        state.dom = Some(dom);
        state.animation_sample = obscura_render::AnimationSample::document(0.0);
        ensure_prepared_render(&mut state).expect("initial render");

        state.animation_sample = obscura_render::AnimationSample::document(500.0);
        let geometry = ensure_prepared_geometry(&mut state).expect("sampled geometry");
        assert_eq!(geometry.animation_sample_time().milliseconds, 500.0);
        let width = geometry.document_rect(box_node).unwrap().width;
        assert!((width - 60.0).abs() < 0.1, "sampled width was {width}");
    }
}

fn validate_fetch_url(url: &url::Url, allow_private_network: bool) -> Result<(), String> {
    let scheme = url.scheme();
    // file:// is denied by default here, matching Page.navigate /
    // Target.createTarget (which gate it behind --allow-file-access). The
    // transports cannot fetch file:// anyway, so a page never reaches the
    // filesystem through fetch()/XHR.
    if scheme != "http" && scheme != "https" {
        return Err(format!(
            "Forbidden URL scheme '{}' - only http and https are allowed",
            scheme
        ));
    }

    if allow_private_network || obscura_net::env_allows_private_network() {
        return Ok(());
    }

    if let Some(host) = url.host() {
        match host {
            url::Host::Ipv4(ip) => {
                if obscura_net::is_forbidden_ip(std::net::IpAddr::V4(ip)) {
                    return Err(format!(
                        "Access to private/internal IP address {} is not allowed",
                        ip
                    ));
                }
            }
            url::Host::Ipv6(ip) => {
                if obscura_net::is_forbidden_ip(std::net::IpAddr::V6(ip)) {
                    return Err(format!(
                        "Access to private/internal IPv6 address {} is not allowed",
                        ip
                    ));
                }
            }
            url::Host::Domain(domain) => {
                let lower_domain = domain.to_lowercase();
                if lower_domain == "localhost"
                    || lower_domain.ends_with(".localhost")
                    || lower_domain == "127.0.0.1"
                    || lower_domain == "::1"
                {
                    return Err(format!(
                        "Access to localhost domain '{}' is not allowed",
                        domain
                    ));
                }
            }
        }
    }

    Ok(())
}

#[op2]
#[string]
fn op_get_cookies(scope: &mut v8::HandleScope, state: &OpState) -> String {
    let gs = realm_state(scope, state);
    let gs = gs.borrow();
    let jar = match &gs.cookie_jar {
        Some(j) => j,
        None => return String::new(),
    };
    let url = match url::Url::parse(&gs.url) {
        Ok(u) => u,
        Err(_) => return String::new(),
    };
    jar.get_js_visible_cookies(&url)
}

#[op2(fast)]
fn op_set_cookie(scope: &mut v8::HandleScope, state: &OpState, #[string] cookie_str: &str) {
    let shared = realm_state(scope, state);
    let gs = shared.borrow();
    let jar = match &gs.cookie_jar {
        Some(j) => j,
        None => return,
    };
    let url = match url::Url::parse(&gs.url) {
        Ok(u) => u,
        Err(_) => return,
    };
    jar.set_cookie_from_js(cookie_str, &url);
    let trace = gs.runtime_events_enabled && gs.diagnostic_events_enabled;
    drop(gs);
    if !trace { return; }
    use sha2::Digest as _;
    let name = cookie_str.split(';').next().unwrap_or("").split('=').next().unwrap_or("").trim();
    let event = RuntimeCookieEvent {
        origin: url.origin().ascii_serialization(),
        name: name.to_string(),
        assignment_bytes: cookie_str.len(),
        assignment_sha256: format!("{:x}", sha2::Sha256::digest(cookie_str.as_bytes())),
        timestamp: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default().as_secs_f64() * 1_000.0,
    };
    let mut gs = shared.borrow_mut();
    if gs.pending_runtime_events.len() >= 1_024 { gs.pending_runtime_events.pop_front(); }
    gs.pending_runtime_events.push_back(RuntimeEvent::Cookie(event));
}

struct HistorySerializer<'s> {
    error: v8::Local<'s, v8::Function>,
}

impl v8::ValueSerializerImpl for HistorySerializer<'_> {
    fn throw_data_clone_error<'s>(
        &self,
        scope: &mut v8::HandleScope<'s>,
        message: v8::Local<'s, v8::String>,
    ) {
        let scope = &mut v8::TryCatch::new(scope);
        let undefined = v8::undefined(scope).into();
        self.error.call(scope, undefined, &[message.into()]);
        if scope.has_caught() || scope.has_terminated() {
            scope.rethrow();
        }
    }

    fn get_shared_array_buffer_id<'s>(
        &self,
        scope: &mut v8::HandleScope<'s>,
        _: v8::Local<'s, v8::SharedArrayBuffer>,
    ) -> Option<u32> {
        let message = v8::String::new(scope, "SharedArrayBuffer cannot be stored").unwrap();
        self.throw_data_clone_error(scope, message);
        None
    }

    fn get_wasm_module_transfer_id(
        &self,
        scope: &mut v8::HandleScope<'_>,
        _: v8::Local<v8::WasmModuleObject>,
    ) -> Option<u32> {
        let message = v8::String::new(scope, "WebAssembly.Module cannot be stored").unwrap();
        self.throw_data_clone_error(scope, message);
        None
    }
}

// SerializeForStorage needs a DataCloneError even on V8's shared-memory path.
// User getters run inside write_value; rethrow their original exceptions.
#[op2(reentrant)]
#[buffer]
fn op_history_serialize(
    scope: &mut v8::HandleScope,
    value: v8::Local<v8::Value>,
    error: v8::Local<v8::Function>,
) -> Vec<u8> {
    use v8::ValueSerializerHelper;
    let serializer = v8::ValueSerializer::new(scope, Box::new(HistorySerializer { error }));
    serializer.write_header();
    let scope = &mut v8::TryCatch::new(scope);
    let result = serializer.write_value(scope.get_current_context(), value);
    if scope.has_caught() || scope.has_terminated() {
        scope.rethrow();
        Vec::new()
    } else if result == Some(true) {
        serializer.release()
    } else {
        let message = v8::String::new(scope, "Value cannot be stored").unwrap();
        v8::ValueSerializerImpl::throw_data_clone_error(
            &HistorySerializer { error },
            scope,
            message,
        );
        Vec::new()
    }
}

// A frame that navigates itself must not move the top document. Recording the
// navigation against the calling realm keeps it inside that frame.
#[op2(fast)]
fn op_navigate(
    scope: &mut v8::HandleScope,
    state: &OpState,
    #[string] url: &str,
    #[string] method: &str,
    #[string] body: &str,
    #[string] behavior: &str,
) {
    if state.try_borrow::<crate::worker::WorkerEndpoint>().is_some() { return; }
    let gs = realm_state(scope, state);
    let mut gs = gs.borrow_mut();
    // Only queue the navigation — do NOT change the realm URL here. The URL is
    // updated on commit via `set_url` once the navigation is actually performed.
    // Moving it early let synchronous JS run between two navigations read and
    // write another origin's cookies through document.cookie, whose ops derive
    // the cookie domain from this URL (SOP bypass, #940).
    let source = gs
        .dom
        .as_ref()
        .and_then(DomTree::document_url)
        .and_then(|url| url::Url::parse(&url).ok());
    let mut request = ResourceRequest::navigation();
    request.referrer_policy = gs.referrer_policy;
    request.referrer = source.clone();
    request.initiator = source;
    gs.pending_navigation = Some(PendingNavigation {
        history: match behavior {
            "replace" => HistoryNavigation::Replace,
            "reload" => HistoryNavigation::Reload,
            _ => HistoryNavigation::Push,
        },
        url: url.to_string(),
        method: method.to_string(),
        body: body.to_string(),
        request,
    });
    gs.same_document_navigation = false;
}

pub(crate) fn frame_message_queue_entry_limit() -> usize {
    std::env::var("OBSCURA_FRAME_MESSAGE_QUEUE_ENTRIES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(4096)
}

pub(crate) fn frame_message_queue_byte_limit() -> usize {
    std::env::var("OBSCURA_FRAME_MESSAGE_QUEUE_BYTES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8 * 1024 * 1024)
}

// Queues one postMessage for another realm. Always on the page's state, never
// the caller's: the Page drains a single queue, and a message sent by a nested
// frame would otherwise sit in that frame's own state and never be looked at.
//
// The queue is capped. Script can post in a synchronous loop while the host
// only drains between event loop turns, and this buffer lives on the process
// heap rather than V8's, so an unbounded queue would let a page grow memory
// without bound in the one place the heap-limit guard cannot see. Over the cap
// the newest message is dropped, keeping the earlier traffic that a widget
// handshake actually depends on.
#[op2(fast)]
fn op_post_frame_message(
    state: &OpState,
    target_frame_id: u32,
    source_frame_id: u32,
    #[string] origin: &str,
    #[string] target_origin: &str,
    #[string] data_json: &str,
) {
    let gs = state.borrow::<SharedState>().clone();
    let mut gs = gs.borrow_mut();
    let over_entries = gs.pending_frame_messages.len() >= frame_message_queue_entry_limit();
    let over_bytes = gs
        .pending_frame_message_bytes
        .saturating_add(data_json.len())
        > frame_message_queue_byte_limit();
    if over_entries || over_bytes {
        tracing::warn!(
            "dropping a postMessage for frame {}: {} already queued, {} bytes",
            target_frame_id,
            gs.pending_frame_messages.len(),
            gs.pending_frame_message_bytes,
        );
        return;
    }
    gs.pending_frame_message_bytes = gs.pending_frame_message_bytes.saturating_add(data_json.len());
    gs.pending_frame_messages.push(PendingFrameMessage {
        target_frame_id,
        source_frame_id,
        origin: origin.to_string(),
        target_origin: target_origin.to_string(),
        data_json: data_json.to_string(),
    });
}

/// Resolves after `millis`, as the timer source for child frame realms.
///
/// deno_core's own timer queue is not usable from a frame: `op_timer_queue`
/// resolves per-context state that only a deno_core-created context carries,
/// and a snapshot-restored realm has none. This resolves an ordinary promise
/// instead, and V8 reports the frame as the microtask context, so the ops a
/// timer callback makes still find the frame's own document.
#[op2(async)]
async fn op_sleep(#[number] millis: u64) {
    tokio::time::sleep(std::time::Duration::from_millis(millis)).await;
}

const MAX_PENDING_FRAME_DOCUMENTS: usize = 64;
const MAX_PENDING_FRAME_BYTES: usize = 32 * 1024 * 1024;

// Hands a fetched frame document to the host and returns the id the frame will
// have. The realm itself is built later, by whoever owns the runtime. A zero
// id means the bounded native queue refused the document.
#[op2(fast)]
fn op_frame_document_ready(
    scope: &mut v8::HandleScope,
    state: &OpState,
    #[string] url: &str,
    #[string] html: &str,
    #[number] viewport_width: u64,
    #[number] viewport_height: u64,
) -> u32 {
    // Whoever called this is the new frame's parent, which is how a frame
    // nested two deep gets `parent` pointing at the frame above it rather than
    // at the page.
    let parent_frame_id = realm_state(scope, state).borrow().frame_id;
    let gs = state.borrow::<SharedState>().clone();
    let mut gs = gs.borrow_mut();
    let bytes = url.len().saturating_add(html.len());
    if gs.pending_frames.len() >= MAX_PENDING_FRAME_DOCUMENTS
        || gs.pending_frame_bytes.saturating_add(bytes) > MAX_PENDING_FRAME_BYTES
    {
        tracing::warn!(
            "dropping frame document: {} pending documents, {} bytes",
            gs.pending_frames.len(),
            gs.pending_frame_bytes,
        );
        return 0;
    }
    let Some(frame_id) = gs.frame_id_counter.checked_add(1) else {
        tracing::warn!("frame id space exhausted");
        return 0;
    };
    gs.frame_id_counter = frame_id;
    gs.pending_frame_bytes = gs.pending_frame_bytes.saturating_add(bytes);
    gs.pending_frames.push(PendingFrame {
        frame_id,
        url: url.to_string(),
        html: html.to_string(),
        viewport_width,
        viewport_height,
        parent_frame_id,
    });
    frame_id
}

/// Whether async host work can be scheduled without aborting the isolate.
///
/// Some low-level embedders intentionally execute a synchronous expression
/// without entering Tokio (for example, update scroll state and immediately
/// capture). deno_core's timer queue requires a reactor even to enqueue a
/// zero-delay timer, so the bootstrap uses this probe for its sync-only
/// compatibility path.
#[op2(fast)]
fn op_async_runtime_available() -> bool {
    tokio::runtime::Handle::try_current().is_ok()
}

/// Queue one browser posted-task delivery on deno_core's engine-local V8 task
/// spawner. It is safe to call from an async-op reaction and wakes the event
/// loop without Tokio's timer-wheel floor or another async-op registration.
#[op2]
fn op_posted_task(
    state: &OpState,
    frame_id: u32,
    #[global] callback: v8::Global<v8::Function>,
) -> f64 {
    let Some(owner) = posted_task_owner(state, frame_id) else {
        return INVALID_POSTED_TASK_GENERATION;
    };
    let Ok(owner_state) = owner.try_borrow() else {
        return INVALID_POSTED_TASK_GENERATION;
    };
    let document_generation = owner_state.document_generation;
    drop(owner_state);
    let owner = Rc::downgrade(&owner);
    let spawner = state.borrow::<deno_core::V8TaskSpawner>().clone();
    spawner.spawn(move |scope| {
        let current_generation = match posted_task_owner_status(&owner) {
            PostedTaskOwnerStatus::Gone | PostedTaskOwnerStatus::Busy => {
                INVALID_POSTED_TASK_GENERATION
            }
            PostedTaskOwnerStatus::Generation(generation) => generation as f64,
        };
        let scope = &mut v8::TryCatch::new(scope);
        let callback = v8::Local::new(scope, callback);
        let receiver = v8::undefined(scope).into();
        let current_generation = v8::Number::new(scope, current_generation);
        if callback.call(scope, receiver, &[current_generation.into()]).is_none() {
            let message = scope
                .exception()
                .map(|exception| exception.to_rust_string_lossy(scope))
                .unwrap_or_else(|| "execution terminated".to_string());
            tracing::warn!("posted-task delivery failed: {message}");
        }
    });
    document_generation as f64
}

const INVALID_POSTED_TASK_GENERATION: f64 = -1.0;

#[derive(Debug, PartialEq, Eq)]
enum PostedTaskOwnerStatus {
    Gone,
    Busy,
    Generation(u64),
}

fn posted_task_owner_status(
    owner: &std::rc::Weak<RefCell<ObscuraState>>,
) -> PostedTaskOwnerStatus {
    let Some(owner) = owner.upgrade() else {
        return PostedTaskOwnerStatus::Gone;
    };
    let Ok(owner) = owner.try_borrow() else {
        return PostedTaskOwnerStatus::Busy;
    };
    PostedTaskOwnerStatus::Generation(owner.document_generation)
}

#[op2(fast)]
fn op_posted_task_generation(state: &OpState, frame_id: u32) -> f64 {
    let Some(owner) = posted_task_owner(state, frame_id) else {
        return INVALID_POSTED_TASK_GENERATION;
    };
    let generation = owner
        .try_borrow()
        .map(|owner| owner.document_generation as f64)
        .unwrap_or(INVALID_POSTED_TASK_GENERATION);
    generation
}

fn posted_task_owner(state: &OpState, frame_id: u32) -> Option<SharedState> {
    if frame_id == 0 {
        return Some(state.borrow::<SharedState>().clone());
    }
    let registry = state.try_borrow::<Rc<RefCell<RealmStates>>>()?.clone();
    let owner = registry.try_borrow().ok()?.by_frame_id(frame_id);
    owner
}

// Records a binding call from page JS. The CDP layer drains this queue
// after every dispatch and emits one `Runtime.bindingCalled` event per
// entry, that's how puppeteer's `page.exposeFunction` callbacks fire.
#[op2(fast)]
fn op_binding_called(state: &OpState, #[string] name: &str, #[string] payload: &str) {
    let gs = state.borrow::<SharedState>().clone();
    let mut gs = gs.borrow_mut();
    gs.pending_binding_calls
        .push((name.to_string(), payload.to_string()));
}

/// Real WebCrypto `crypto.subtle.digest`. `algorithm` is the SubtleCrypto
/// algorithm name (`SHA-1` / `SHA-256` / `SHA-384` / `SHA-512`, plus the
/// FIPS 180-4 truncated variants `SHA-512/224` and `SHA-512/256`). The JS
/// shim validates the name; any other value is unreachable.
/// Returns the raw digest bytes so the JS shim can hand them back as an ArrayBuffer.
#[op2]
#[buffer]
fn op_subtle_digest(#[string] algorithm: &str, #[buffer] data: &[u8]) -> Vec<u8> {
    use sha1::Digest as _;
    let alg = algorithm.to_ascii_uppercase();
    match alg.as_str() {
        "SHA-1" => sha1::Sha1::digest(data).to_vec(),
        "SHA-256" => sha2::Sha256::digest(data).to_vec(),
        "SHA-384" => sha2::Sha384::digest(data).to_vec(),
        "SHA-512" => sha2::Sha512::digest(data).to_vec(),
        "SHA-512/224" => sha2::Sha512_224::digest(data).to_vec(),
        "SHA-512/256" => sha2::Sha512_256::digest(data).to_vec(),
        _ => vec![],
    }
}

// ---------------------------------------------------------------------------
// WebCrypto (crypto.subtle) secret-key primitives.
//
// These ops are stateless. The JS shim in bootstrap.js owns the CryptoKey
// objects and their raw key bytes; it hands the bytes plus normalized algorithm
// parameters to these ops for each operation. Only secret-key algorithms live
// here (HMAC, AES-GCM/CBC/CTR, PBKDF2, HKDF); public-key algorithms are rejected
// in the shim. A fallible op returns a JsErrorBox that the shim turns into the
// appropriate DOMException (OperationError for a bad tag or padding, etc.).
// ---------------------------------------------------------------------------

fn crypto_err(msg: impl std::fmt::Display) -> deno_error::JsErrorBox {
    deno_error::JsErrorBox::generic(msg.to_string())
}

/// HMAC sign. `hash` is a normalized SubtleCrypto hash name; any key length is
/// accepted (HMAC pads or hashes the key per RFC 2104). Returns the MAC bytes;
/// the shim does the constant-time-insensitive compare for `verify`.
#[op2]
#[buffer]
fn op_subtle_hmac(
    #[string] hash: &str,
    #[buffer] key: &[u8],
    #[buffer] data: &[u8],
) -> Result<Vec<u8>, deno_error::JsErrorBox> {
    use hmac::{Hmac, Mac};
    macro_rules! run {
        ($d:ty) => {{
            let mut mac = Hmac::<$d>::new_from_slice(key).map_err(crypto_err)?;
            mac.update(data);
            mac.finalize().into_bytes().to_vec()
        }};
    }
    Ok(match hash {
        "SHA-1" => run!(sha1::Sha1),
        "SHA-256" => run!(sha2::Sha256),
        "SHA-384" => run!(sha2::Sha384),
        "SHA-512" => run!(sha2::Sha512),
        _ => return Err(crypto_err("unsupported HMAC hash")),
    })
}

/// AES-GCM encrypt/decrypt. WebCrypto's ciphertext carries the auth tag
/// appended, which is exactly RustCrypto's combined form, so this maps 1:1.
/// Restricted to a 96-bit IV and 128-bit tag (the WebCrypto defaults and the
/// overwhelming majority of real usage); the shim rejects other tag lengths.
#[op2]
#[buffer]
fn op_subtle_aes_gcm(
    encrypt: bool,
    #[buffer] key: &[u8],
    #[buffer] iv: &[u8],
    #[buffer] aad: &[u8],
    #[buffer] data: &[u8],
) -> Result<Vec<u8>, deno_error::JsErrorBox> {
    use aes_gcm::aead::{Aead, KeyInit, Payload};
    use aes_gcm::aes::{Aes192, Aes256};
    use aes_gcm::{AesGcm, Nonce};
    type Aes192Gcm = AesGcm<Aes192, aes_gcm::aead::consts::U12>;
    type Aes256Gcm = AesGcm<Aes256, aes_gcm::aead::consts::U12>;

    if iv.len() != 12 {
        return Err(crypto_err("AES-GCM requires a 96-bit (12-byte) IV"));
    }
    let nonce = Nonce::from_slice(iv);
    macro_rules! run {
        ($ty:ty) => {{
            let cipher = <$ty>::new_from_slice(key).map_err(crypto_err)?;
            if encrypt {
                cipher
                    .encrypt(nonce, Payload { msg: data, aad })
                    .map_err(|_| crypto_err("AES-GCM encryption failed"))?
            } else {
                cipher
                    .decrypt(nonce, Payload { msg: data, aad })
                    .map_err(|_| {
                        crypto_err("AES-GCM decryption failed: authentication tag mismatch")
                    })?
            }
        }};
    }
    Ok(match key.len() {
        16 => run!(aes_gcm::Aes128Gcm),
        24 => run!(Aes192Gcm),
        32 => run!(Aes256Gcm),
        _ => return Err(crypto_err("AES-GCM key must be 128, 192, or 256 bits")),
    })
}

/// AES-CBC encrypt/decrypt with PKCS#7 padding (the only padding WebCrypto
/// AES-CBC uses) and a 16-byte IV.
#[op2]
#[buffer]
fn op_subtle_aes_cbc(
    encrypt: bool,
    #[buffer] key: &[u8],
    #[buffer] iv: &[u8],
    #[buffer] data: &[u8],
) -> Result<Vec<u8>, deno_error::JsErrorBox> {
    use cbc::cipher::block_padding::Pkcs7;
    use cbc::cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit};
    use cbc::{Decryptor, Encryptor};

    if iv.len() != 16 {
        return Err(crypto_err("AES-CBC requires a 16-byte IV"));
    }
    macro_rules! run {
        ($cipher:ty) => {{
            if encrypt {
                Encryptor::<$cipher>::new_from_slices(key, iv)
                    .map_err(crypto_err)?
                    .encrypt_padded_vec_mut::<Pkcs7>(data)
            } else {
                Decryptor::<$cipher>::new_from_slices(key, iv)
                    .map_err(crypto_err)?
                    .decrypt_padded_vec_mut::<Pkcs7>(data)
                    .map_err(|_| crypto_err("AES-CBC decryption failed: invalid padding"))?
            }
        }};
    }
    Ok(match key.len() {
        16 => run!(aes::Aes128),
        24 => run!(aes::Aes192),
        32 => run!(aes::Aes256),
        _ => return Err(crypto_err("AES-CBC key must be 128, 192, or 256 bits")),
    })
}

/// AES-CTR. Encrypt and decrypt are the same keystream XOR. `counter_length` is
/// the WebCrypto counter width in bits; it selects the RustCrypto CTR flavor so
/// only the low `counter_length` bits of the 16-byte block increment.
#[op2]
#[buffer]
fn op_subtle_aes_ctr(
    #[buffer] key: &[u8],
    #[buffer] counter: &[u8],
    counter_length: u32,
    #[buffer] data: &[u8],
) -> Result<Vec<u8>, deno_error::JsErrorBox> {
    use ctr::cipher::{KeyIvInit, StreamCipher};

    if counter.len() != 16 {
        return Err(crypto_err("AES-CTR requires a 16-byte counter block"));
    }
    let mut buf = data.to_vec();
    macro_rules! run {
        ($ty:ty) => {{
            <$ty>::new_from_slices(key, counter)
                .map_err(crypto_err)?
                .apply_keystream(&mut buf);
        }};
    }
    macro_rules! by_key {
        ($flavor:ident) => {
            match key.len() {
                16 => run!(ctr::$flavor<aes::Aes128>),
                24 => run!(ctr::$flavor<aes::Aes192>),
                32 => run!(ctr::$flavor<aes::Aes256>),
                _ => return Err(crypto_err("AES-CTR key must be 128, 192, or 256 bits")),
            }
        };
    }
    match counter_length {
        128 => by_key!(Ctr128BE),
        64 => by_key!(Ctr64BE),
        32 => by_key!(Ctr32BE),
        _ => {
            return Err(crypto_err(
                "AES-CTR supports counter lengths of 32, 64, or 128 bits",
            ))
        }
    }
    Ok(buf)
}

/// Generous upper bounds on PBKDF2 parameters. WebCrypto imposes no limit, but
/// page JS drives this op on the single-threaded runtime: an unbounded
/// iteration count pins the V8 isolate (blocking every other CDP command on the
/// connection) and a huge output length forces an unbounded `vec![0u8; length]`
/// allocation. Both caps sit far above any legitimate use — OWASP recommends
/// ~600k iterations and derived keys are tens of bytes.
const PBKDF2_MAX_ITERATIONS: u32 = 10_000_000;
const PBKDF2_MAX_OUTPUT_BYTES: u32 = 1024 * 1024;

/// PBKDF2 key derivation with DoS guards. Split out from the op so the bounds
/// are unit-testable without the `#[op2]` wrapper.
fn pbkdf2_derive(
    hash: &str,
    password: &[u8],
    salt: &[u8],
    iterations: u32,
    length: u32,
) -> Result<Vec<u8>, deno_error::JsErrorBox> {
    if iterations > PBKDF2_MAX_ITERATIONS {
        return Err(crypto_err(format!(
            "PBKDF2 iteration count {iterations} exceeds the supported maximum of {PBKDF2_MAX_ITERATIONS}"
        )));
    }
    if length > PBKDF2_MAX_OUTPUT_BYTES {
        return Err(crypto_err(format!(
            "PBKDF2 output length {length} bytes exceeds the supported maximum of {PBKDF2_MAX_OUTPUT_BYTES}"
        )));
    }
    use pbkdf2::pbkdf2_hmac;
    let mut dk = vec![0u8; length as usize];
    match hash {
        "SHA-1" => pbkdf2_hmac::<sha1::Sha1>(password, salt, iterations, &mut dk),
        "SHA-256" => pbkdf2_hmac::<sha2::Sha256>(password, salt, iterations, &mut dk),
        "SHA-384" => pbkdf2_hmac::<sha2::Sha384>(password, salt, iterations, &mut dk),
        "SHA-512" => pbkdf2_hmac::<sha2::Sha512>(password, salt, iterations, &mut dk),
        _ => return Err(crypto_err("unsupported PBKDF2 hash")),
    }
    Ok(dk)
}

/// PBKDF2 key derivation. `length` is the derived-bits output in bytes.
#[op2]
#[buffer]
fn op_subtle_pbkdf2(
    #[string] hash: &str,
    #[buffer] password: &[u8],
    #[buffer] salt: &[u8],
    iterations: u32,
    length: u32,
) -> Result<Vec<u8>, deno_error::JsErrorBox> {
    pbkdf2_derive(hash, password, salt, iterations, length)
}

/// Generous DoS backstop on HKDF output length. HKDF itself rejects output
/// above 255*HashLen, but only after the buffer is allocated, so an enormous
/// `length` forces a multi-GB `vec![0u8; length]` first. This bound sits far
/// above any legitimate derived key and mirrors `PBKDF2_MAX_OUTPUT_BYTES`.
const HKDF_MAX_OUTPUT_BYTES: u32 = 1024 * 1024;

/// HKDF derivation with a DoS guard. Split out from the op so the bound is
/// unit-testable without the `#[op2]` wrapper.
fn hkdf_derive(
    hash: &str,
    ikm: &[u8],
    salt: &[u8],
    info: &[u8],
    length: u32,
) -> Result<Vec<u8>, deno_error::JsErrorBox> {
    if length > HKDF_MAX_OUTPUT_BYTES {
        return Err(crypto_err(format!(
            "HKDF output length {length} bytes exceeds the supported maximum of {HKDF_MAX_OUTPUT_BYTES}"
        )));
    }
    use hkdf::Hkdf;
    let mut okm = vec![0u8; length as usize];
    macro_rules! run {
        ($d:ty) => {
            Hkdf::<$d>::new(Some(salt), ikm)
                .expand(info, &mut okm)
                .map_err(|_| crypto_err("HKDF: requested key length is too long"))?
        };
    }
    match hash {
        "SHA-1" => run!(sha1::Sha1),
        "SHA-256" => run!(sha2::Sha256),
        "SHA-384" => run!(sha2::Sha384),
        "SHA-512" => run!(sha2::Sha512),
        _ => return Err(crypto_err("unsupported HKDF hash")),
    }
    Ok(okm)
}

/// HKDF key derivation. `length` is the output length in bytes. An empty salt
/// behaves as RFC 5869 specifies (HMAC zero-pads it to the block size, which is
/// what browsers do).
#[op2]
#[buffer]
fn op_subtle_hkdf(
    #[string] hash: &str,
    #[buffer] ikm: &[u8],
    #[buffer] salt: &[u8],
    #[buffer] info: &[u8],
    length: u32,
) -> Result<Vec<u8>, deno_error::JsErrorBox> {
    hkdf_derive(hash, ikm, salt, info, length)
}

/// Generous DoS backstop on a single CSPRNG draw. `getRandomValues` already
/// enforces the WebCrypto 65536-byte limit in JS; this guards the native op
/// against other callers (notably HMAC `generateKey`, whose `length` is
/// attacker-controllable) forcing a multi-GB allocation plus CSPRNG read.
const RANDOM_BYTES_MAX: u32 = 1024 * 1024;

/// Draw `len` bytes from the OS CSPRNG, with a DoS guard. Split out from the op
/// so the bound is unit-testable without the `#[op2]` wrapper.
fn random_bytes(len: u32) -> Result<Vec<u8>, deno_error::JsErrorBox> {
    if len > RANDOM_BYTES_MAX {
        return Err(crypto_err(format!(
            "random byte request of {len} bytes exceeds the supported maximum of {RANDOM_BYTES_MAX}"
        )));
    }
    let mut buf = vec![0u8; len as usize];
    getrandom::getrandom(&mut buf).map_err(|e| crypto_err(format!("getrandom failed: {e}")))?;
    Ok(buf)
}

/// Fill `len` bytes from the OS CSPRNG. Backs `crypto.getRandomValues`,
/// `crypto.randomUUID`, and `generateKey`, replacing the old Math.random shim
/// (which was neither uniform across typed-array widths nor cryptographically
/// random, and was a fingerprinting tell).
#[op2]
#[buffer]
fn op_random_bytes(len: u32) -> Result<Vec<u8>, deno_error::JsErrorBox> {
    random_bytes(len)
}

/// Serialize a parsed URL into the WHATWG IDL component shape consumed by the
/// `URL` class in bootstrap.js. Getters read these fields directly so no op
/// call happens per property access.
fn url_components(u: &url::Url) -> serde_json::Value {
    let port = u.port().map(|p| p.to_string()).unwrap_or_default();
    let hostname = u.host_str().unwrap_or("").to_string();
    let host = if hostname.is_empty() {
        String::new()
    } else if port.is_empty() {
        hostname.clone()
    } else {
        format!("{hostname}:{port}")
    };
    // WHATWG search/hash getters return "" for a null OR empty component.
    let search = match u.query() {
        Some(q) if !q.is_empty() => format!("?{q}"),
        _ => String::new(),
    };
    let hash = match u.fragment() {
        Some(f) if !f.is_empty() => format!("#{f}"),
        _ => String::new(),
    };
    serde_json::json!({
        "ok": true,
        "href": u.as_str(),
        "protocol": format!("{}:", u.scheme()),
        "username": u.username(),
        "password": u.password().unwrap_or(""),
        "host": host,
        "hostname": hostname,
        "port": port,
        "pathname": u.path(),
        "search": search,
        "hash": hash,
        "origin": u.origin().ascii_serialization(),
    })
}

/// Parse `href` (optionally resolved against `base`) with the WHATWG-compliant
/// `url` crate. Returns the component JSON, or `{"ok":false}` when the input is
/// not a valid URL (the JS side turns that into a TypeError, per spec).
#[op2]
#[string]
fn op_url_parse(#[string] href: &str, #[string] base: &str) -> String {
    // The url crate can panic on a few pathological inputs (internal range
    // slicing); catch it so a bad URL never aborts the process.
    std::panic::catch_unwind(|| {
        let parsed = if base.is_empty() {
            url::Url::parse(href)
        } else {
            url::Url::parse(base).and_then(|b| b.join(href))
        };
        match parsed {
            Ok(u) => url_components(&u).to_string(),
            Err(_) => "{\"ok\":false}".to_string(),
        }
    })
    .unwrap_or_else(|_| "{\"ok\":false}".to_string())
}

/// Apply a WHATWG URL setter (`part` = href/protocol/username/password/host/
/// hostname/port/pathname/search/hash) to `href` and return the new components.
fn url_set_inner(href: &str, part: &str, value: &str) -> Option<serde_json::Value> {
    let mut u = url::Url::parse(href).ok()?;
    match part {
        "href" => {
            let nu = url::Url::parse(value).ok()?;
            return Some(url_components(&nu));
        }
        "protocol" => {
            let _ = u.set_scheme(value.trim_end_matches(':'));
        }
        "username" => {
            let _ = u.set_username(value);
        }
        "password" => {
            let _ = u.set_password(if value.is_empty() { None } else { Some(value) });
        }
        "host" => set_host_port(&mut u, value),
        "hostname" => {
            if !value.is_empty() {
                let _ = u.set_host(Some(value));
            }
        }
        "port" => {
            if value.is_empty() {
                let _ = u.set_port(None);
            } else if let Ok(p) = value.parse::<u16>() {
                let _ = u.set_port(Some(p));
            }
        }
        "pathname" => u.set_path(value),
        "search" => {
            let q = value.strip_prefix('?').unwrap_or(value);
            u.set_query(if q.is_empty() { None } else { Some(q) });
        }
        "hash" => {
            let f = value.strip_prefix('#').unwrap_or(value);
            u.set_fragment(if f.is_empty() { None } else { Some(f) });
        }
        _ => {}
    }
    Some(url_components(&u))
}

#[op2]
#[string]
fn op_url_set(#[string] href: &str, #[string] part: &str, #[string] value: &str) -> String {
    // Some url-crate setters panic on pathological inputs (the url-setters WPT
    // tests exercise these). Catch the unwind and treat it as a no-op setter,
    // returning the URL unchanged, which matches WHATWG "do nothing on invalid".
    match std::panic::catch_unwind(|| url_set_inner(href, part, value)) {
        Ok(Some(v)) => v.to_string(),
        _ => match url::Url::parse(href) {
            Ok(u) => url_components(&u).to_string(),
            Err(_) => "{\"ok\":false}".to_string(),
        },
    }
}

/// Best-effort `host` setter: split `host[:port]` (handling bracketed IPv6) and
/// apply hostname and port separately, since `url::Url::set_host` rejects a port.
fn set_host_port(u: &mut url::Url, value: &str) {
    // IPv6 literals are bracketed; never split inside the brackets.
    if value.starts_with('[') {
        if let Some(close) = value.find(']') {
            let host = &value[..=close];
            let rest = &value[close + 1..];
            if u.set_host(Some(host)).is_ok() {
                if let Some(p) = rest.strip_prefix(':') {
                    if let Ok(pn) = p.parse::<u16>() {
                        let _ = u.set_port(Some(pn));
                    }
                }
            }
            return;
        }
    }
    if let Some(idx) = value.rfind(':') {
        let (h, p) = (&value[..idx], &value[idx + 1..]);
        if p.is_empty() || p.chars().all(|c| c.is_ascii_digit()) {
            if u.set_host(Some(h)).is_ok() {
                if p.is_empty() {
                    let _ = u.set_port(None);
                } else if let Ok(pn) = p.parse::<u16>() {
                    let _ = u.set_port(Some(pn));
                }
            }
            return;
        }
    }
    let _ = u.set_host(Some(value));
}

/// Resolve `href` against optional `base` and return only the serialized
/// absolute URL (no component breakdown). Used by the hot `a.href`/`area.href`
/// getter, which only needs the resolved string, so it avoids building and
/// re-parsing the full component JSON. Returns "" when the input is invalid.
#[op2]
#[string]
fn op_url_resolve(#[string] href: &str, #[string] base: &str) -> String {
    std::panic::catch_unwind(|| {
        let parsed = if base.is_empty() {
            url::Url::parse(href)
        } else {
            url::Url::parse(base).and_then(|b| b.join(href))
        };
        parsed.map(|u| u.as_str().to_string()).unwrap_or_default()
    })
    .unwrap_or_default()
}

/// Canonicalize and validate a `document.domain` assignment.
///
/// Gecko's `Document::IsValidDomain` accepts the current effective host or a
/// dot-delimited suffix no shorter than its registrable domain.  The latter
/// check is important: a plain `ends_with` would let `foo.example.co.uk`
/// relax all the way to `co.uk`, and would incorrectly treat private suffixes
/// such as `github.io` as shared registrable domains.
///
/// An empty return value means SecurityError on the JS side.  The current host
/// is supplied by the Document rather than read from op state because repeated
/// assignments operate on the already-relaxed effective domain.
#[op2]
#[string]
fn op_document_domain_candidate(#[string] current: &str, #[string] input: &str) -> String {
    let canonical = match url::Host::parse(input) {
        Ok(host) => host.to_string().to_ascii_lowercase(),
        Err(_) => return String::new(),
    };
    let current = current.to_ascii_lowercase();

    // Gecko permits assigning the exact current host, including IP literals
    // and single-label hosts.  Neither can be relaxed to a parent.
    if canonical == current {
        return canonical;
    }
    if current.parse::<std::net::IpAddr>().is_ok()
        || canonical.parse::<std::net::IpAddr>().is_ok()
        || !current.ends_with(&format!(".{canonical}"))
    {
        return String::new();
    }

    // `domain_str` is the eTLD+1.  A candidate shorter than it is a public
    // suffix and must not become an effective domain.
    match psl::domain_str(&current) {
        Some(registrable) if canonical.len() >= registrable.len() => canonical,
        _ => String::new(),
    }
}

#[op2]
#[string]
fn op_add_import_map(
    state: &OpState,
    #[string] source: String,
    #[string] base_url: String,
) -> String {
    let shared = state.borrow::<SharedState>().clone();
    let import_map = shared.borrow().import_map.clone();
    let parsed = match ImportMap::parse(&source, &base_url) {
        Ok(map) => map,
        Err(error) => return error,
    };
    let result = match import_map.try_borrow_mut() {
        Ok(mut current) => {
            current.merge(parsed);
            String::new()
        }
        Err(_) => "Import map is already borrowed".to_string(),
    };
    result
}

/// Canonical (lowercased) WHATWG name for a TextDecoder label, or "" if the
/// label is unknown (the JS constructor turns "" into a RangeError).
#[op2]
#[string]
fn op_encoding_for_label(#[string] label: &str) -> String {
    obscura_net::label_name(label).unwrap_or_default()
}

/// Decode bytes with a legacy/explicit encoding via encoding_rs. Returns
/// {"ok":true,"v":<string>} or {"ok":false} (unknown label, or a fatal decode
/// error). The UTF-8 non-fatal common case is handled in JS without this op.
#[op2]
#[string]
fn op_text_decode(
    #[string] label: &str,
    #[buffer] bytes: &[u8],
    fatal: bool,
    ignore_bom: bool,
) -> String {
    match obscura_net::decode_with_label(label, bytes, fatal, ignore_bom) {
        Some(s) => serde_json::json!({ "ok": true, "v": s }).to_string(),
        None => "{\"ok\":false}".to_string(),
    }
}

/// Re-encode a URL query component using a non-UTF-8 document encoding override
/// (the WHATWG "encoding override"). `query` is the already-UTF-8-decoded query
/// string; `label` the target charset; `special` whether the URL has a special
/// scheme (adds `'` to the percent-encode set). Returns the encoded query, or
/// the input unchanged if the label is unknown. Only called by the JS anchor
/// path when the document is non-UTF-8, so the UTF-8 hot path never reaches it.
#[op2]
#[string]
fn op_url_encode_query(#[string] query: &str, #[string] label: &str, special: bool) -> String {
    obscura_net::url_encode_query(query, label, special).unwrap_or_else(|| query.to_string())
}

#[cfg(feature = "render")]
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct DynamicFontFaceInput {
    family: String,
    source: String,
    style: String,
    weight: String,
    unicode_range: String,
}

/// Replace the native snapshot of `document.fonts`. The JS implementation
/// remains the source of truth for set semantics; this narrow bridge only
/// supplies resource descriptors to the render preparation path.
#[cfg(feature = "render")]
#[op2(fast)]
fn op_set_dynamic_fonts(state: &OpState, #[string] registrations: &str) -> bool {
    let Ok(inputs) = serde_json::from_str::<Vec<DynamicFontFaceInput>>(registrations) else {
        return false;
    };
    // Keep the observable registry broad enough for generated font families
    // (large applications commonly register dozens of subset faces). The
    // renderer independently caps decoded resources after ASCII filtering and
    // URL deduplication. BufferSource faces arrive as data URLs, so cap their
    // aggregate descriptor payload as well as each entry.
    if inputs.len() > 256
        || inputs
            .iter()
            .try_fold(0usize, |total, face| total.checked_add(face.source.len()))
            .map_or(true, |total| total > 64 * 1024 * 1024)
        || inputs.iter().any(|face| {
            face.family.len() > 1024
                || face.source.len() > 12 * 1024 * 1024
                || face.style.len() > 256
                || face.weight.len() > 256
                || face.unicode_range.len() > 4096
        })
    {
        return false;
    }
    let fonts = inputs
        .into_iter()
        .map(|face| obscura_render::DynamicFontFace {
            family: face.family,
            source: face.source,
            style: face.style,
            weight: face.weight,
            unicode_range: face.unicode_range,
        })
        .collect::<Vec<_>>();
    let shared = state.borrow::<SharedState>().clone();
    let mut state = shared.borrow_mut();
    if state.dynamic_fonts != fonts {
        state.dynamic_fonts = fonts;
        invalidate_render_resource_geometry(&mut state);
    }
    true
}

/// Retain the JavaScript-owned Canvas2D pixel buffer without copying it. A
/// canvas resize supplies a new fixed backing store and atomically replaces
/// the previous surface for the same DOM node.
#[cfg(feature = "render")]
#[op2]
fn op_canvas_register_surface(
    state: &OpState,
    nid: u32,
    width: u32,
    height: u32,
    #[buffer] pixels: JsBuffer,
) -> bool {
    const MAX_CANVAS_DIMENSION: u32 = 32_767;
    const MAX_CANVAS_PIXELS: usize = 67_108_864;
    const MAX_CANVAS_SURFACE_BYTES: usize = 256 * 1024 * 1024;
    let Some(expected) = (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
    else {
        return false;
    };
    if width > MAX_CANVAS_DIMENSION
        || height > MAX_CANVAS_DIMENSION
        || expected / 4 > MAX_CANVAS_PIXELS
        || pixels.len() != expected
    {
        return false;
    }

    let shared = state.borrow::<SharedState>().clone();
    let mut state = shared.borrow_mut();
    let node = NodeId::new(nid);
    let is_canvas = state
        .dom
        .as_ref()
        .and_then(|dom| dom.get_node(node))
        .is_some_and(|node| {
            node.as_element()
                .is_some_and(|name| name.local.as_ref() == "canvas")
        });
    if !is_canvas {
        return false;
    }
    let replacing = state.canvas_surfaces.get(&node).map(|surface| surface.pixels.len());
    let retained_bytes = state
        .canvas_surfaces
        .values()
        .try_fold(0usize, |total, surface| total.checked_add(surface.pixels.len()))
        .and_then(|total| total.checked_sub(replacing.unwrap_or(0)))
        .and_then(|total| total.checked_add(expected));
    if retained_bytes.is_none_or(|bytes| bytes > MAX_CANVAS_SURFACE_BYTES) {
        return false;
    }
    state.canvas_surfaces.insert(
        node,
        CanvasBackingSurface {
            width,
            height,
            pixels,
        },
    );
    true
}

/// Report one coalesced Canvas2D paint at the JavaScript task boundary. Pixel
/// bytes are already live through the retained backing store, so damage wakes
/// screencast/readiness without throwing away otherwise-valid layout.
#[cfg(feature = "render")]
#[op2(fast)]
fn op_canvas_paint_damage(state: &OpState, nid: u32) -> bool {
    let shared = state.borrow::<SharedState>().clone();
    let mut state = shared.borrow_mut();
    let node = NodeId::new(nid);
    if !state.canvas_surfaces.contains_key(&node) {
        return false;
    }
    let connected = state
        .dom
        .as_ref()
        .is_some_and(|dom| node_is_connected(dom, node));
    if connected {
        state.activity_generation = state.activity_generation.wrapping_add(1);
    }
    connected
}

pub fn build_extension() -> Extension {
    let mut ops = vec![
        op_dom(),
        op_script_mark_started(),
        op_script_try_start(),
        op_shadow_attach(),
        op_shadow_root_info(),
        op_runtime_events_enabled(),
        op_console_msg(),
        op_fetch_url(),
        op_fetch_start(),
        op_fetch_abort(),
        op_fetch_cleanup(),
        op_get_cookies(),
        op_set_cookie(),
        op_navigate(),
        op_history_serialize(),
        op_frame_document_ready(),
        op_post_frame_message(),
        op_sleep(),
        op_async_runtime_available(),
        op_posted_task(),
        op_posted_task_generation(),
        op_binding_called(),
        op_subtle_digest(),
        op_subtle_hmac(),
        op_subtle_aes_gcm(),
        op_subtle_aes_cbc(),
        op_subtle_aes_ctr(),
        op_subtle_pbkdf2(),
        op_subtle_hkdf(),
        op_random_bytes(),
        op_url_parse(),
        op_url_set(),
        op_url_resolve(),
        op_document_domain_candidate(),
        op_add_import_map(),
        op_encoding_for_label(),
        op_text_decode(),
        op_url_encode_query(),
        crate::worker::op_worker_serialize(),
        crate::worker::op_worker_deserialize(),
        crate::worker::op_worker_create(),
        crate::worker::op_worker_run(),
        crate::worker::op_worker_next_event(),
        crate::worker::op_worker_close(),
        crate::worker::op_worker_post_to_worker(),
        crate::worker::op_worker_post_to_parent(),
        crate::worker::op_worker_terminate(),
        crate::worker::op_worker_load_script(),
        crate::worker::op_worker_run_script(),
    ];
    // Only registered when the render feature is compiled in. bootstrap.js
    // probes with typeof before calling, so the op's absence is a clean fallback.
    #[cfg(feature = "render")]
    {
        ops.push(op_begin_render_task());
        ops.push(op_set_dynamic_fonts());
        ops.push(op_canvas_register_surface());
        ops.push(op_canvas_paint_damage());
        ops.push(op_image_metadata());
        ops.push(op_load_image_metadata());
        ops.push(op_layout_geometry());
        ops.push(op_resize_observer_measurements());
        ops.push(op_intersection_observer_measurements());
        ops.push(op_computed_style());
        ops.push(op_css_supports());
        ops.push(op_layout_metrics());
        ops.push(op_element_scroll_metrics());
        ops.push(op_element_scroll_to());
        ops.push(op_scroll_offset());
        ops.push(op_scroll_to());
        ops.push(op_waapi_create());
        ops.push(op_waapi_control());
    }
    Extension {
        name: "obscura_dom",
        ops: std::borrow::Cow::Owned(ops),
        ..Default::default()
    }
}

#[cfg(feature = "render")]
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WaapiCreateInput {
    id: u64,
    node: u32,
    keyframes: Vec<WaapiKeyframeInput>,
    duration: f32,
    delay: f32,
    iterations: f32,
    #[serde(default)]
    iterations_infinite: bool,
    fill: String,
    direction: String,
    easing_bezier: Option<[f32; 4]>,
    linear_easing: Option<Vec<f32>>,
}

#[cfg(feature = "render")]
#[derive(Deserialize)]
struct WaapiKeyframeInput {
    offset: f32,
    opacity: Option<f32>,
    transform: Option<String>,
}

#[cfg(feature = "render")]
fn waapi_document_time_ms(state: &ObscuraState) -> f32 {
    state.animation_timeline_origin.elapsed().as_secs_f32() * 1000.0
}

#[cfg(feature = "render")]
fn invalidate_waapi_render(state: &mut ObscuraState, node: NodeId) {
    // Adding or controlling one effect changes the animation cascade only for
    // its target. Keep the previous style graph available to the
    // retained planner instead of turning every animation setup into a full
    // document cascade. The bounded mutation queue remains the safety valve
    // for genuinely broad animation bursts.
    if state.prepared_render.is_some()
        && !queue_retained_style_mutation(
            &mut state.pending_style_mutations,
            obscura_render::RetainedStyleMutation::WaapiAnimation { node },
        )
    {
        state.prepared_render = None;
        state.pending_style_mutations.clear();
    }
    state.resolved_scroll = None;
    state.activity_generation = state.activity_generation.wrapping_add(1);
}

#[cfg(feature = "render")]
#[op2(fast)]
fn op_waapi_create(state: &OpState, #[string] input: &str) -> bool {
    let Ok(input) = serde_json::from_str::<WaapiCreateInput>(input) else {
        return false;
    };
    if !input.duration.is_finite()
        || input.duration < 0.0
        || !input.delay.is_finite()
        || !input.iterations.is_finite()
        || input.iterations < 0.0
        || input.keyframes.is_empty()
    {
        return false;
    }
    let shared = state.borrow::<SharedState>().clone();
    let mut state = shared.borrow_mut();
    let node = NodeId::new(input.node);
    if state.dom.as_ref().and_then(|dom| dom.get_node(node)).is_none() {
        return false;
    }
    let start_time_ms = waapi_document_time_ms(&state);
    let fill_mode = match input.fill.as_str() {
        "forwards" => obscura_render::AnimationFillMode::Forwards,
        "backwards" => obscura_render::AnimationFillMode::Backwards,
        "both" => obscura_render::AnimationFillMode::Both,
        _ => obscura_render::AnimationFillMode::None,
    };
    let direction = match input.direction.as_str() {
        "reverse" => obscura_render::AnimationDirection::Reverse,
        "alternate" => obscura_render::AnimationDirection::Alternate,
        "alternate-reverse" => obscura_render::AnimationDirection::AlternateReverse,
        _ => obscura_render::AnimationDirection::Normal,
    };
    let iterations = if input.iterations_infinite {
        f32::INFINITY
    } else {
        input.iterations
    };
    state.animation_timeline.register_waapi(obscura_render::WaapiAnimation {
        id: input.id,
        node,
        keyframes: input.keyframes.into_iter().map(|frame| obscura_render::WaapiKeyframe {
            offset: frame.offset.clamp(0.0, 1.0),
            opacity: frame.opacity.map(|value| value.clamp(0.0, 1.0)),
            transform: frame.transform,
        }).collect(),
        timing: obscura_render::AnimationTiming {
            duration_ms: input.duration,
            delay_ms: input.delay,
            iteration_count: iterations,
            direction,
            fill_mode,
            play_state: obscura_render::AnimationPlayState::Running,
        },
        easing: input.easing_bezier,
        linear_easing: input.linear_easing,
        start_time_ms,
        hold_time_ms: None,
        play_state: obscura_render::WaapiPlayState::Running,
    });
    invalidate_waapi_render(&mut state, node);
    true
}

#[cfg(feature = "render")]
#[op2(fast)]
fn op_waapi_control(
    state: &OpState,
    id: f64,
    #[string] action: &str,
    value: f64,
) -> bool {
    if !id.is_finite() || id < 0.0 {
        return false;
    }
    let shared = state.borrow::<SharedState>().clone();
    let mut state = shared.borrow_mut();
    let id = id as u64;
    let Some(node) = state.animation_timeline.waapi_node(id) else {
        return false;
    };
    let document_time = waapi_document_time_ms(&state);
    let changed = match action {
        "cancel" => state.animation_timeline.cancel_waapi(id),
        "finish" => state.animation_timeline.finish_waapi(id),
        "pause" => state.animation_timeline.set_waapi_play_state(
            id,
            obscura_render::WaapiPlayState::Paused,
            document_time,
        ),
        "play" => state.animation_timeline.set_waapi_play_state(
            id,
            obscura_render::WaapiPlayState::Running,
            document_time,
        ),
        "currentTime" if value.is_finite() => state.animation_timeline.set_waapi_current_time(
            id,
            document_time,
            value as f32,
        ),
        _ => false,
    };
    if changed {
        invalidate_waapi_render(&mut state, node);
    }
    changed
}

// Not tied to `render`: the JS layer resolves every relative URL through here, in all build
// variants.
pub(crate) fn document_base_url(state: &ObscuraState) -> Option<String> {
    let document_url = state.dom.as_ref().and_then(DomTree::document_url)
        .unwrap_or_else(|| state.url.clone());
    let document_url = url::Url::parse(&document_url).ok()?;
    if let Some((href, fallback)) = state.dom.as_ref().and_then(DomTree::frozen_base) {
        let fallback = url::Url::parse(&fallback).ok()?;
        return Some(match fallback.join(&href) {
            Ok(base) if !matches!(base.scheme(), "data" | "javascript") => base.to_string(),
            _ => fallback.to_string(),
        });
    }
    Some(document_url.to_string())
}

/// Raw href of the first HTML base; URL consumers use document_base_url.
fn document_base_href(state: &ObscuraState) -> Option<String> {
    state.dom.as_ref()?.frozen_base().map(|(href, _)| href)
}

/// Native base changes, document replacement and fallback URL changes invalidate the cache.
pub struct BaseUrlCache {
    base_generation: u64,
    activity_generation: u64,
    document_generation: u64,
    url: String,
    resolved: Option<String>,
    raw_href: Option<String>,
}

/// Both base values behind a cache. Uncached, each one walks the tree and runs the selector
/// engine, which would make `a.href` an O(nodes) read.
fn base_values_memoized(state: &ObscuraState) -> (Option<String>, Option<String>) {
    let base_generation = state.dom.as_ref().map_or(0, DomTree::base_generation);
    if let Some(cached) = state.base_url_cache.borrow().as_ref() {
        if cached.base_generation == base_generation
            && cached.activity_generation == state.activity_generation
            && cached.document_generation == state.document_generation
            && cached.url == state.url
        {
            return (cached.resolved.clone(), cached.raw_href.clone());
        }
    }
    let resolved = document_base_url(state);
    let raw_href = document_base_href(state);
    *state.base_url_cache.borrow_mut() = Some(BaseUrlCache {
        base_generation,
        activity_generation: state.activity_generation,
        document_generation: state.document_generation,
        url: state.url.clone(),
        resolved: resolved.clone(),
        raw_href: raw_href.clone(),
    });
    (resolved, raw_href)
}

pub(crate) fn document_base_url_memoized(state: &ObscuraState) -> Option<String> {
    base_values_memoized(state).0
}

pub(crate) fn document_base_href_memoized(state: &ObscuraState) -> Option<String> {
    base_values_memoized(state).1
}

#[cfg(feature = "render")]
pub(crate) fn ensure_prepared_render(
    state: &mut ObscuraState,
) -> Option<&obscura_render::PreparedRender> {
    let base_url = document_base_url(state);
    let viewport = state.viewport;
    let render_media = state.render_media;
    let animation_sample = state.animation_sample;
    let incompatible = state.prepared_render.as_ref().is_some_and(|prepared| {
        prepared.viewport() != viewport
            || prepared.base_url() != base_url.as_deref()
    });
    let needs_rebuild = state.prepared_render.as_ref().map_or(true, |prepared| {
        incompatible || prepared.animation_sample() != animation_sample
    }) || !state.pending_style_mutations.is_empty();
    if needs_rebuild {
        if let Some(dom) = state.dom.as_ref() {
            state
                .animation_timeline
                .materialize_start_candidates(dom);
        }
        let previous = (!incompatible && render_media == obscura_render::CssMediaType::Screen)
            .then(|| state.prepared_render.take())
            .flatten();
        let mutations = std::mem::take(&mut state.pending_style_mutations);
        let prepared = {
            let dom = state.dom.as_ref()?;
            match previous {
                Some(previous) => obscura_render::prepare_dom_with_retained_styles_with_animation_state(
                    dom,
                    viewport,
                    base_url.as_deref(),
                    &mut state.render_resources,
                    &state.dynamic_fonts,
                    &mut state.stylesheet_cache,
                    previous,
                    &mutations,
                    animation_sample,
                    &mut state.animation_timeline,
                )
                .or_else(|| {
                    obscura_render::prepare_dom_with_dynamic_fonts_and_stylesheet_cache_with_animation_state(
                        dom,
                        viewport,
                        base_url.as_deref(),
                        &mut state.render_resources,
                        &state.dynamic_fonts,
                        &mut state.stylesheet_cache,
                        animation_sample,
                        &mut state.animation_timeline,
                    )
                })?,
                None => match render_media {
                    obscura_render::CssMediaType::Screen => obscura_render::prepare_dom_with_dynamic_fonts_and_stylesheet_cache_with_animation_state(
                        dom,
                        viewport,
                        base_url.as_deref(),
                        &mut state.render_resources,
                        &state.dynamic_fonts,
                        &mut state.stylesheet_cache,
                        animation_sample,
                        &mut state.animation_timeline,
                    )?,
                    obscura_render::CssMediaType::Print => obscura_render::prepare_dom_with_dynamic_fonts_and_stylesheet_cache_for_media_with_animation_state(
                        dom,
                        viewport,
                        base_url.as_deref(),
                        &mut state.render_resources,
                        &state.dynamic_fonts,
                        &mut state.stylesheet_cache,
                        render_media,
                        animation_sample,
                        &mut state.animation_timeline,
                    )?,
                },
            }
        };
        if animation_sample.mode == obscura_render::AnimationSampleMode::DocumentTime {
            state.animation_timeline.clear_start_candidates();
        }
        let connected = state
            .dom
            .as_ref()
            .map(shadow_including_connected_nodes);
        if let Some(connected) = connected {
            state
                .animation_timeline
                .retain_nodes(|node| connected.contains(&node));
        }
        state.prepared_render = Some(prepared);
        state.resolved_scroll = None;
    }
    state.prepared_render.as_ref()
}

/// Prepare enough state for a geometry-only CSSOM consumer. A forward sample
/// with only paint effects may read the retained layout without resampling its
/// styles. `animation_sample` on PreparedRender remains behind intentionally,
/// making a later paint or computed-style consumer take the exact path above.
#[cfg(feature = "render")]
fn ensure_prepared_geometry(
    state: &mut ObscuraState,
) -> Option<&obscura_render::PreparedRender> {
    let base_url = document_base_url(state);
    let reusable = state.pending_style_mutations.is_empty()
        && !state.animation_timeline.has_pending_start_candidates()
        && state.prepared_render.as_ref().is_some_and(|prepared| {
            prepared.viewport() == state.viewport
                && prepared.base_url() == base_url.as_deref()
                && (prepared.animation_sample() == state.animation_sample
                    || prepared.can_reuse_geometry_for_animation_sample(state.animation_sample))
        });
    if reusable {
        return state.prepared_render.as_ref();
    }
    ensure_prepared_render(state)
}

#[cfg(feature = "render")]
pub(crate) fn sample_live_document_animations(state: &mut ObscuraState) {
    if state.animation_sampled_task_generation == state.animation_task_generation {
        return;
    }
    state.animation_sampled_task_generation = state.animation_task_generation;
    let sample = obscura_render::AnimationSample::document(
        (state.animation_timeline_origin.elapsed().as_secs_f64() * 1_000.0)
            .min(f64::from(f32::MAX)) as f32,
    );
    if state.animation_sample == sample {
        return;
    }
    if sample.time.milliseconds > state.animation_sample.time.milliseconds
        && state.animation_sample.mode == obscura_render::AnimationSampleMode::DocumentTime
        && state.pending_style_mutations.is_empty()
        && state.prepared_render.as_mut().is_some_and(|prepared| {
            prepared.advance_inactive_animation_sample_time(sample.time)
        })
    {
        state.animation_sample = sample;
        return;
    }
    let forward_document_sample =
        sample.mode == obscura_render::AnimationSampleMode::DocumentTime
        && state.animation_sample.mode == obscura_render::AnimationSampleMode::DocumentTime
        && sample.time.milliseconds > state.animation_sample.time.milliseconds;
    state.animation_sample = sample;
    if !forward_document_sample {
        state.prepared_render = None;
        state.pending_style_mutations.clear();
    }
    state.resolved_scroll = None;
}

#[cfg(feature = "render")]
pub(crate) fn begin_animation_task(state: &mut ObscuraState) {
    state.animation_task_generation = state.animation_task_generation.wrapping_add(1);
}

#[cfg(feature = "render")]
#[op2(fast)]
fn op_begin_render_task(state: &OpState) {
    let shared = state.borrow::<SharedState>().clone();
    begin_animation_task(&mut shared.borrow_mut());
}

#[cfg(feature = "render")]
pub(crate) fn ensure_resolved_scroll(state: &mut ObscuraState) -> Option<()> {
    ensure_resolved_scroll_for_consumer(state, false)
}

#[cfg(feature = "render")]
fn ensure_resolved_scroll_for_geometry(state: &mut ObscuraState) -> Option<()> {
    ensure_resolved_scroll_for_consumer(state, true)
}

#[cfg(feature = "render")]
fn ensure_resolved_scroll_for_consumer(
    state: &mut ObscuraState,
    geometry_only: bool,
) -> Option<()> {
    if geometry_only {
        ensure_prepared_geometry(state)?;
    } else {
        ensure_prepared_render(state)?;
    }
    if state
        .resolved_scroll
        .as_ref()
        .is_some_and(|(generation, _)| *generation == state.scroll_generation)
    {
        return Some(());
    }

    let valid = state
        .prepared_render
        .as_ref()?
        .scroll_container_nodes()
        .collect::<HashSet<_>>();
    let snapshot = {
        let dom = state.dom.as_ref()?;
        state.prepared_render.as_ref()?.resolve_scroll_state(
            dom,
            state.scroll_offset,
            &state.element_scroll_offsets,
        )
    };
    state.scroll_offset = snapshot.root_offset();
    for node in valid {
        let offset = state
            .prepared_render
            .as_ref()?
            .element_scroll_metrics(node, &snapshot)
            .map(|metrics| metrics.offset)
            .unwrap_or((0.0, 0.0));
        if offset == (0.0, 0.0) {
            state.element_scroll_offsets.remove(&node);
        } else {
            state.element_scroll_offsets.insert(node, offset);
        }
    }
    state.resolved_scroll = Some((state.scroll_generation, snapshot));
    Some(())
}

#[cfg(feature = "render")]
fn image_metadata_json(
    current_src: String,
    density: f32,
    known: bool,
    dimensions: Option<(f32, f32)>,
) -> String {
    if !known {
        return serde_json::json!({
            "state": "pending",
            "currentSrc": current_src,
            "density": density,
        })
        .to_string();
    }
    match dimensions {
        Some((width, height)) => serde_json::json!({
            "state": "loaded",
            "ok": true,
            "currentSrc": current_src,
            "density": density,
            "width": width,
            "height": height,
        })
        .to_string(),
        None => serde_json::json!({
            "state": "error",
            "ok": false,
            "currentSrc": current_src,
            "density": density,
        })
        .to_string(),
    }
}

#[cfg(feature = "render")]
fn image_request_profile(dom: &DomTree, node_id: NodeId) -> ImageRequestProfile {
    match dom
        .get_node(node_id)
        .and_then(|node| node.get_attribute("crossorigin").map(str::to_owned))
        .map(|value| value.trim().to_ascii_lowercase())
        .as_deref()
    {
        Some("use-credentials") => ImageRequestProfile::CorsInclude,
        Some(_) => ImageRequestProfile::CorsSameOrigin,
        None => ImageRequestProfile::NoCorsInclude,
    }
}

#[cfg(feature = "render")]
fn profiled_cached_image_metadata(
    gs: &ObscuraState,
    node_id: NodeId,
) -> Option<(String, f32, bool, Option<(f32, f32)>)> {
    let dom = gs.dom.as_ref()?;
    let base_url = document_base_url(gs);
    gs.render_resources.cached_image_element_metadata(
        dom,
        node_id,
        gs.viewport,
        base_url.as_deref(),
    )
}

#[cfg(feature = "render")]
fn cached_image_metadata_for_node(gs: &ObscuraState, node_id: NodeId) -> String {
    match profiled_cached_image_metadata(gs, node_id) {
        Some((current_src, density, known, dimensions)) => {
            image_metadata_json(current_src, density, known, dimensions)
        }
        None => serde_json::json!({ "ok": false, "currentSrc": "" }).to_string(),
    }
}

/// Probe one ordinary `<img>` through the renderer's page-scoped resource
/// cache. This op is intentionally cache-only. Lifecycle getters call it
/// synchronously and must never open a socket or wait on network I/O.
#[cfg(feature = "render")]
#[op2]
#[string]
fn op_image_metadata(state: &OpState, nid: u32, _cached_only: bool) -> String {
    let shared = state.borrow::<SharedState>().clone();
    let gs = shared.borrow();
    let node_id = NodeId::new(nid);
    let is_image = gs.dom.as_ref().is_some_and(|dom| {
        dom.get_node(node_id).is_some_and(|node| {
            node.as_element()
                .is_some_and(|element| element.local.as_ref() == "img")
        })
    });
    if !is_image {
        return serde_json::json!({ "ok": false, "currentSrc": "" }).to_string();
    }
    cached_image_metadata_for_node(&gs, node_id)
}

/// Compatibility path for standalone render runtimes which deliberately
/// install an in-memory `RenderResourceLoader` but have no owning page
/// transport. Browser pages install their transport before page script runs;
/// their caches are cache-only (`fresh_render_resources`), so neither this
/// path nor layout/paint can open a synchronous request for them.
#[cfg(feature = "render")]
fn load_image_metadata_without_page_transport(gs: &mut ObscuraState, node_id: NodeId) -> String {
    let base_url = document_base_url(&gs);
    let viewport = gs.viewport;
    let previous_dimensions = gs.dom.as_ref().and_then(|dom| {
        gs.render_resources
            .cached_image_element_metadata(dom, node_id, viewport, base_url.as_deref())
            .and_then(|(_, _, known, dimensions)| known.then_some(dimensions).flatten())
    });
    let Some(dom) = gs.dom.as_ref() else {
        return serde_json::json!({ "ok": false, "currentSrc": "" }).to_string();
    };
    let Some((current_src, density, dimensions)) = gs.render_resources.image_element_metadata(
        dom,
        node_id,
        viewport,
        base_url.as_deref(),
    ) else {
        return serde_json::json!({
            "state": "error",
            "ok": false,
            "currentSrc": "",
        })
        .to_string();
    };
    if dimensions.is_some() && dimensions != previous_dimensions {
        invalidate_render_resource_geometry(gs);
    }
    image_metadata_json(current_src, density, true, dimensions)
}

#[cfg(feature = "render")]
fn finish_async_image_metadata(
    shared: &SharedState,
    node_id: NodeId,
    document_generation: u64,
    expected_url: &str,
    request_profile: ImageRequestProfile,
) -> String {
    let gs = shared.borrow();
    if gs.document_generation != document_generation {
        return serde_json::json!({ "state": "stale", "currentSrc": expected_url })
            .to_string();
    }
    let Some(dom) = gs.dom.as_ref() else {
        return serde_json::json!({ "state": "stale", "currentSrc": expected_url })
            .to_string();
    };
    if image_request_profile(dom, node_id) != request_profile {
        return serde_json::json!({ "state": "stale", "currentSrc": expected_url })
            .to_string();
    }
    let Some((current_src, density, known, dimensions)) =
        profiled_cached_image_metadata(&gs, node_id)
    else {
        return serde_json::json!({ "state": "stale", "currentSrc": expected_url })
            .to_string();
    };
    if current_src != expected_url {
        return serde_json::json!({ "state": "stale", "currentSrc": current_src }).to_string();
    }
    image_metadata_json(current_src, density, known, dimensions)
}

/// Load HTMLImageElement bytes through the owning page's async transport.
/// Network runs after every RefCell borrow is released, requests for the same
/// navigation/URL/profile share one fetch, and completion revalidates both the
/// document identity and responsive candidate before exposing lifecycle state.
#[cfg(feature = "render")]
#[op2(async)]
#[string]
async fn op_load_image_metadata(state: Rc<RefCell<OpState>>, nid: u32) -> String {
    let shared = {
        let state = state.borrow();
        state.borrow::<SharedState>().clone()
    };
    let node_id = NodeId::new(nid);
    let (
        document_generation,
        selected_url,
        request_profile,
        resource_request,
        callbacks,
        blocked,
    ) = {
        let gs = shared.borrow();
        let Some(dom) = gs.dom.as_ref() else {
            return serde_json::json!({ "state": "stale", "currentSrc": "" }).to_string();
        };
        let is_image = dom.get_node(node_id).is_some_and(|node| {
            node.as_element()
                .is_some_and(|element| element.local.as_ref() == "img")
        });
        if !is_image {
            return serde_json::json!({ "state": "stale", "currentSrc": "" }).to_string();
        }
        let profile = image_request_profile(dom, node_id);
        let Some((selected_url, _, known, _)) =
            profiled_cached_image_metadata(&gs, node_id)
        else {
            return serde_json::json!({ "state": "error", "ok": false, "currentSrc": "" })
                .to_string();
        };
        if known {
            return cached_image_metadata_for_node(&gs, node_id);
        }
        let initiator = url::Url::parse(&gs.url)
            .or_else(|_| url::Url::parse(&selected_url))
            .unwrap_or_else(|_| url::Url::parse("about:blank").unwrap());
        let mut request = ResourceRequest::subresource(ResourceType::Image, &initiator);
        request.referrer_policy = gs.referrer_policy;
        match profile {
            ImageRequestProfile::CorsInclude => {
                request.mode = RequestMode::Cors;
                request.credentials = RequestCredentials::Include;
            }
            ImageRequestProfile::CorsSameOrigin => {
                request.mode = RequestMode::Cors;
                request.credentials = RequestCredentials::SameOrigin;
            }
            ImageRequestProfile::NoCorsInclude => {}
        }
        let blocked = gs.blocked_urls.iter().any(|pattern| {
            pattern == "*" || selected_url.contains(pattern) || glob_match(pattern, &selected_url)
        });
        (
            gs.document_generation,
            selected_url,
            profile,
            request,
            gs.callbacks.clone(),
            blocked,
        )
    };

    if shared.borrow().render_resources.sync_loading_enabled() {
        return load_image_metadata_without_page_transport(&mut shared.borrow_mut(), node_id);
    }
    let stealth_client = shared.borrow_mut().ensure_persona_transport();

    // Different CORS/credential profiles do not share an in-flight response.
    let request_key = (document_generation, selected_url.clone(), request_profile);
    let follower = {
        let mut gs = shared.borrow_mut();
        if let Some(waiters) = gs.render_image_in_flight.get_mut(&request_key) {
            let (sender, receiver) = tokio::sync::oneshot::channel();
            waiters.push(sender);
            Some(receiver)
        } else {
            gs.render_image_in_flight.insert(request_key.clone(), Vec::new());
            None
        }
    };
    if let Some(receiver) = follower {
        let _ = receiver.await;
        return finish_async_image_metadata(
            &shared,
            node_id,
            document_generation,
            &selected_url,
            request_profile,
        );
    }

    let parsed_url = url::Url::parse(&selected_url).ok();
    let response = if blocked || parsed_url.is_none() {
        None
    } else {
        let parsed_url = parsed_url.as_ref().unwrap();
        stealth_client
            .fetch_resource_with_callbacks(parsed_url, resource_request, callbacks.as_deref())
            .await
            .ok()
    };
    let bytes = response.and_then(|response| {
        (200..300)
            .contains(&response.status)
            .then_some(response.body)
    });
    let waiters = {
        let mut gs = shared.borrow_mut();
        if gs.document_generation == document_generation {
            match bytes {
                Some(bytes) => {
                    if obscura_render::image_intrinsic_dimensions(&bytes).is_some() {
                        gs.render_resources.seed_image(
                            selected_url.clone(),
                            request_profile,
                            bytes,
                        );
                        // The leader owns the unknown-to-known cache
                        // transition. Followers only observe this result and
                        // must not invalidate the retained render again.
                        invalidate_render_resource_geometry(&mut gs);
                    } else {
                        gs.render_resources
                            .seed_image_missing(selected_url.clone(), request_profile);
                    }
                }
                None => {
                    gs.render_resources
                        .seed_image_missing(selected_url.clone(), request_profile);
                }
            }
        }
        gs.render_image_in_flight
            .remove(&request_key)
            .unwrap_or_default()
    };
    for waiter in waiters {
        let _ = waiter.send(());
    }
    finish_async_image_metadata(
        &shared,
        node_id,
        document_generation,
        &selected_url,
        request_profile,
    )
}

#[cfg(feature = "render")]
pub(crate) fn clamp_scroll_offset(state: &mut ObscuraState, requested: (f32, f32)) -> (f32, f32) {
    clamp_scroll_offset_for_consumer(state, requested, false)
}

#[cfg(feature = "render")]
fn clamp_scroll_offset_for_geometry(
    state: &mut ObscuraState,
    requested: (f32, f32),
) -> (f32, f32) {
    clamp_scroll_offset_for_consumer(state, requested, true)
}

#[cfg(feature = "render")]
fn clamp_scroll_offset_for_consumer(
    state: &mut ObscuraState,
    requested: (f32, f32),
    geometry_only: bool,
) -> (f32, f32) {
    let prepared = if geometry_only {
        ensure_prepared_geometry(state)
    } else {
        ensure_prepared_render(state)
    };
    let clamped = prepared
        .map(|prepared| prepared.clamp_scroll(requested))
        .unwrap_or((0.0, 0.0));
    if state.scroll_offset != clamped {
        state.scroll_offset = clamped;
        state.activity_generation = state.activity_generation.wrapping_add(1);
        state.scroll_generation = state.scroll_generation.wrapping_add(1);
        state.resolved_scroll = None;
    }
    state.scroll_offset
}

/// Real border-box geometry for an element from the obscura-render layout
/// cache. The cache is computed lazily on first read and cleared on navigation
/// (see `set_dom`). Coordinates are viewport-relative after the shared root
/// scroll offset, except for viewport-fixed subtrees. Returns JSON
/// `{"x","y","width","height","clientWidth","clientHeight","clientRects"}`
/// in CSS pixels, or an empty string when the node has no box. The client
/// dimensions are the unscaled padding box used by CSSOM View, `clientRects`
/// retains every inline continuation, and the top-level rect is their visual
/// viewport-relative bounding union. Feature-gated.
#[cfg(feature = "render")]
#[op2]
#[string]
fn op_layout_geometry(state: &OpState, #[string] nid_str: String) -> String {
    let shared = state.borrow::<SharedState>().clone();
    let nid: u32 = nid_str.parse().unwrap_or(0);
    let nid = obscura_dom::tree::NodeId::new(nid);
    let mut gs = shared.borrow_mut();
    sample_live_document_animations(&mut gs);
    if ensure_resolved_scroll_for_geometry(&mut gs).is_some() {
        let Some((_, scroll)) = gs.resolved_scroll.as_ref() else {
            return String::new();
        };
        let Some(prepared) = gs.prepared_render.as_ref() else {
            return String::new();
        };
        let Some(rect) = prepared.viewport_rect_with_scroll(nid, scroll) else {
            return String::new();
        };
        let Some((client_width, client_height)) = prepared.client_size(nid) else {
            return String::new();
        };
        let Some(client_rects) = prepared.viewport_client_rects_with_scroll(nid, scroll) else {
            return String::new();
        };
        let client_rects = client_rects
            .into_iter()
            .map(|rect| {
                serde_json::json!({
                    "x": rect.x,
                    "y": rect.y,
                    "width": rect.width,
                    "height": rect.height,
                })
            })
            .collect::<Vec<_>>();
        let viewport_fixed = prepared.viewport_fixed_nodes().contains(&nid);
        return serde_json::json!({
            "x": rect.x,
            "y": rect.y,
            "width": rect.width,
            "height": rect.height,
            "clientWidth": client_width,
            "clientHeight": client_height,
            "clientRects": client_rects,
            "viewportFixed": viewport_fixed,
        })
        .to_string();
    }
    String::new()
}

/// Measure every target in one ResizeObserver rendering opportunity.
///
/// ResizeObserver gathers all observations before it invokes any callback.
/// Crossing the JS/native boundary once per target defeated that batching:
/// each read sampled the document timeline and could rebuild the retained
/// cascade/layout independently.  Accept the complete target list, freeze the
/// animation sample once, prepare/resolve layout once, and return the small
/// computed-style subset needed to derive content/border/device-pixel boxes.
/// The result is index-aligned with the input and contains `null` for targets
/// which currently generate no box (detached, `display:none`, and stale ids).
#[cfg(feature = "render")]
#[op2]
#[string]
fn op_resize_observer_measurements(state: &OpState, #[string] nids_json: String) -> String {
    let nids = serde_json::from_str::<Vec<u32>>(&nids_json).unwrap_or_default();
    if nids.is_empty() {
        return "[]".to_string();
    }

    let shared = state.borrow::<SharedState>().clone();
    let mut gs = shared.borrow_mut();
    sample_live_document_animations(&mut gs);
    if ensure_resolved_scroll_for_geometry(&mut gs).is_none() {
        return serde_json::to_string(&vec![serde_json::Value::Null; nids.len()])
            .unwrap_or_else(|_| "[]".to_string());
    }
    let Some((_, scroll)) = gs.resolved_scroll.as_ref() else {
        return serde_json::to_string(&vec![serde_json::Value::Null; nids.len()])
            .unwrap_or_else(|_| "[]".to_string());
    };
    let Some(prepared) = gs.prepared_render.as_ref() else {
        return serde_json::to_string(&vec![serde_json::Value::Null; nids.len()])
            .unwrap_or_else(|_| "[]".to_string());
    };

    let style_value =
        |snapshot: &std::collections::HashMap<&'static str, String>, name: &'static str| {
            snapshot.get(name).cloned().unwrap_or_default()
        };
    let measurements = nids
        .into_iter()
        .map(|nid| {
            let nid = obscura_dom::tree::NodeId::new(nid);
            let rect = prepared.viewport_rect_with_scroll(nid, scroll)?;
            let (client_width, client_height) = prepared.client_size(nid)?;
            let snapshot = prepared.computed_style(nid)?;
            Some(serde_json::json!({
                "x": rect.x,
                "y": rect.y,
                "clientWidth": client_width,
                "clientHeight": client_height,
                "paddingTop": style_value(&snapshot, "padding-top"),
                "paddingRight": style_value(&snapshot, "padding-right"),
                "paddingBottom": style_value(&snapshot, "padding-bottom"),
                "paddingLeft": style_value(&snapshot, "padding-left"),
                "borderTopWidth": style_value(&snapshot, "border-top-width"),
                "borderRightWidth": style_value(&snapshot, "border-right-width"),
                "borderBottomWidth": style_value(&snapshot, "border-bottom-width"),
                "borderLeftWidth": style_value(&snapshot, "border-left-width"),
                "writingMode": style_value(&snapshot, "writing-mode"),
                "display": style_value(&snapshot, "display"),
            }))
        })
        .collect::<Vec<_>>();
    serde_json::to_string(&measurements).unwrap_or_else(|_| "[]".to_string())
}

/// Measure the complete IntersectionObserver clip graph in one rendering
/// opportunity. The JS side supplies the unique observed targets, element
/// roots, and intervening element ancestors. Sampling animations and preparing
/// layout once here avoids turning each target/ancestor box and style read into
/// a separate retained-layout rebuild.
///
/// Results are index-aligned with the input. A `null` entry means that the node
/// currently generates no layout box (for example, it is detached or hidden).
#[cfg(feature = "render")]
#[op2]
#[string]
fn op_intersection_observer_measurements(
    state: &OpState,
    #[string] nids_json: String,
) -> String {
    let nids = serde_json::from_str::<Vec<u32>>(&nids_json).unwrap_or_default();
    if nids.is_empty() {
        return "[]".to_string();
    }

    let shared = state.borrow::<SharedState>().clone();
    let mut gs = shared.borrow_mut();
    sample_live_document_animations(&mut gs);
    if ensure_resolved_scroll_for_geometry(&mut gs).is_none() {
        return serde_json::to_string(&vec![serde_json::Value::Null; nids.len()])
            .unwrap_or_else(|_| "[]".to_string());
    }
    let Some((_, scroll)) = gs.resolved_scroll.as_ref() else {
        return serde_json::to_string(&vec![serde_json::Value::Null; nids.len()])
            .unwrap_or_else(|_| "[]".to_string());
    };
    let Some(prepared) = gs.prepared_render.as_ref() else {
        return serde_json::to_string(&vec![serde_json::Value::Null; nids.len()])
            .unwrap_or_else(|_| "[]".to_string());
    };

    let style_value =
        |snapshot: &std::collections::HashMap<&'static str, String>, name: &'static str| {
            snapshot.get(name).cloned().unwrap_or_default()
        };
    let measurements = nids
        .into_iter()
        .map(|nid| {
            let nid = obscura_dom::tree::NodeId::new(nid);
            let rect = prepared.viewport_rect_with_scroll(nid, scroll)?;
            let (client_width, client_height) = prepared.client_size(nid)?;
            let snapshot = prepared.computed_style(nid)?;
            Some(serde_json::json!({
                "x": rect.x,
                "y": rect.y,
                "width": rect.width,
                "height": rect.height,
                "clientWidth": client_width,
                "clientHeight": client_height,
                "borderTopWidth": style_value(&snapshot, "border-top-width"),
                "borderLeftWidth": style_value(&snapshot, "border-left-width"),
                "overflowX": style_value(&snapshot, "overflow-x"),
                "overflowY": style_value(&snapshot, "overflow-y"),
            }))
        })
        .collect::<Vec<_>>();
    serde_json::to_string(&measurements).unwrap_or_else(|_| "[]".to_string())
}

/// One renderer-computed CSS snapshot for `getComputedStyle()`. Returning all
/// supported properties together keeps a single JS style object to one native
/// call and one use of the retained prepared layout.
#[cfg(feature = "render")]
#[op2]
#[string]
fn op_computed_style(state: &OpState, #[string] nid_str: String) -> String {
    let shared = state.borrow::<SharedState>().clone();
    let nid: u32 = nid_str.parse().unwrap_or(0);
    let nid = obscura_dom::tree::NodeId::new(nid);
    let mut gs = shared.borrow_mut();
    sample_live_document_animations(&mut gs);
    let Some(prepared) = ensure_prepared_render(&mut gs) else {
        return String::new();
    };
    let Some(snapshot) = prepared.computed_style(nid) else {
        return String::new();
    };
    let custom = prepared.computed_custom_properties(nid).unwrap_or_default();
    let mut object = serde_json::Map::with_capacity(snapshot.len() + custom.len());
    for (name, value) in snapshot {
        object.insert(name.to_string(), serde_json::Value::String(value));
    }
    for (name, value) in custom {
        object.insert(name, serde_json::Value::String(value));
    }
    serde_json::Value::Object(object).to_string()
}

/// Use the renderer's declaration parser as the single feature-query source
/// of truth. Keeping this bridge synchronous and state-free makes the common
/// two-argument `CSS.supports()` overload a single native call.
#[cfg(feature = "render")]
#[op2(fast)]
fn op_css_supports(#[string] name: &str, #[string] value: &str) -> bool {
    obscura_render::style::supports_declaration(name, value)
}

/// Root scrolling overflow in CSS pixels. The JS CSSOM probes this op only in
/// render builds; default scraping builds retain their deliberately unbounded
/// synthetic scrolling behavior.
#[cfg(feature = "render")]
#[op2]
#[string]
fn op_layout_metrics(state: &OpState) -> String {
    let shared = state.borrow::<SharedState>().clone();
    let mut gs = shared.borrow_mut();
    sample_live_document_animations(&mut gs);
    let viewport = gs.viewport;
    let content = ensure_prepared_geometry(&mut gs)
        .map(|prepared| prepared.content_size())
        .unwrap_or(viewport);
    format!(
        "{{\"scrollWidth\":{},\"scrollHeight\":{},\"clientWidth\":{},\"clientHeight\":{}}}",
        content.0, content.1, viewport.0, viewport.1
    )
}

#[cfg(feature = "render")]
#[op2]
#[string]
fn op_element_scroll_metrics(state: &OpState, #[string] nid_str: String) -> String {
    let shared = state.borrow::<SharedState>().clone();
    let nid = NodeId::new(nid_str.parse().unwrap_or(0));
    let mut gs = shared.borrow_mut();
    sample_live_document_animations(&mut gs);
    if ensure_resolved_scroll_for_geometry(&mut gs).is_none() {
        return String::new();
    }
    let Some((_, scroll)) = gs.resolved_scroll.as_ref() else {
        return String::new();
    };
    let Some(metrics) = gs
        .prepared_render
        .as_ref()
        .and_then(|prepared| prepared.element_scroll_metrics(nid, scroll))
    else {
        // The op exists in render builds, so an unboxed/detached node must not
        // fall through to bootstrap's synthetic non-render metrics.
        return r#"{"scrollWidth":0,"scrollHeight":0,"clientWidth":0,"clientHeight":0,"x":0,"y":0,"maxX":0,"maxY":0,"hasBox":false}"#.to_string();
    };
    serde_json::json!({
        "scrollWidth": metrics.content_size.0,
        "scrollHeight": metrics.content_size.1,
        "clientWidth": metrics.client_size.0,
        "clientHeight": metrics.client_size.1,
        "x": metrics.offset.0,
        "y": metrics.offset.1,
        "maxX": metrics.max_offset.0,
        "maxY": metrics.max_offset.1,
        "hasBox": true,
    })
    .to_string()
}

#[cfg(feature = "render")]
#[op2]
#[string]
fn op_element_scroll_to(state: &OpState, #[string] nid_str: String, x: f64, y: f64) -> String {
    let shared = state.borrow::<SharedState>().clone();
    let nid = NodeId::new(nid_str.parse().unwrap_or(0));
    let mut gs = shared.borrow_mut();
    sample_live_document_animations(&mut gs);
    if ensure_resolved_scroll_for_geometry(&mut gs).is_none() {
        return String::new();
    }
    let current = gs.resolved_scroll.as_ref().and_then(|(_, scroll)| {
        gs.prepared_render
            .as_ref()?
            .element_scroll_metrics(nid, scroll)
    });
    let Some(current) = current else {
        return String::new();
    };
    let clamp = |value: f64, max: f32| {
        if value.is_finite() {
            obscura_render::quantize_scroll_value(value as f32, 1.0).clamp(0.0, max)
        } else {
            0.0
        }
    };
    let requested = (
        clamp(x, current.max_offset.0),
        clamp(y, current.max_offset.1),
    );
    if requested != current.offset {
        if requested == (0.0, 0.0) {
            gs.element_scroll_offsets.remove(&nid);
        } else {
            gs.element_scroll_offsets.insert(nid, requested);
        }
        gs.activity_generation = gs.activity_generation.wrapping_add(1);
        gs.scroll_generation = gs.scroll_generation.wrapping_add(1);
        gs.script_scroll_generation = gs.script_scroll_generation.wrapping_add(1);
        gs.resolved_scroll = None;
        return format!("{{\"x\":{},\"y\":{}}}", requested.0, requested.1);
    }
    format!("{{\"x\":{},\"y\":{}}}", current.offset.0, current.offset.1)
}

#[cfg(feature = "render")]
#[op2]
#[string]
fn op_scroll_offset(state: &OpState) -> String {
    let shared = state.borrow::<SharedState>().clone();
    let mut gs = shared.borrow_mut();
    sample_live_document_animations(&mut gs);
    let requested = gs.scroll_offset;
    let (x, y) = clamp_scroll_offset_for_geometry(&mut gs, requested);
    format!("{{\"x\":{},\"y\":{}}}", x, y)
}

#[cfg(feature = "render")]
#[op2]
#[string]
fn op_scroll_to(state: &OpState, x: f64, y: f64) -> String {
    let shared = state.borrow::<SharedState>().clone();
    let mut gs = shared.borrow_mut();
    sample_live_document_animations(&mut gs);
    let previous = gs.scroll_offset;
    let (x, y) = clamp_scroll_offset_for_geometry(&mut gs, (x as f32, y as f32));
    if previous != (x, y) {
        gs.script_scroll_generation = gs.script_scroll_generation.wrapping_add(1);
    }
    format!("{{\"x\":{},\"y\":{}}}", x, y)
}
