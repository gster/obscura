//! Original image identity and content outlive active layout/network authority.
//! Host capabilities are never minted from author functions or later State.
use super::*;
use obscura_net::websocket::session::Owner;
#[cfg(not(feature = "render"))]
type ImageRequestProfile = ();

pub(crate) struct OriginalDocumentStorage {
    owner: std::rc::Weak<RefCell<ObscuraState>>,
    pub(super) retired_dom: RefCell<Option<DomTree>>,
    images: Rc<RefCell<ImageRegistry>>,
}
// Final-record drops remove their exact native identity, even for a retired
// document. A borrowed registry uses a deferred queue, never a history scan.
type ImageKey = (u32, u64);
#[derive(Default)]
struct ImageRegistry {
    entries: HashMap<ImageKey, std::rc::Weak<RefCell<ImageRecord>>>,
    retired: Rc<RefCell<Vec<ImageKey>>>,
    #[cfg(test)]
    reclaimed: usize,
    #[cfg(test)]
    publication_visits: usize,
}
impl ImageRegistry {
    fn prune(&mut self) {
        let mut retired = self.retired.borrow_mut();
        for key in retired.drain(..) {
            if self.entries.get(&key).is_some_and(|record| record.strong_count() == 0) {
                self.entries.remove(&key);
                #[cfg(test)] { self.reclaimed += 1; }
            }
        }
        if retired.capacity() > 64 { retired.shrink_to(64); }
        // HashMap iteration visits capacity, not just len. Shrink only after
        // geometric depletion, so growing/live cohorts do not rehash per bind.
        if self.entries.capacity() > self.entries.len().saturating_mul(4).max(64) {
            self.entries.shrink_to(self.entries.len().saturating_mul(2).max(32));
        }
    }
}
impl OriginalDocumentStorage {
    #[cfg(test)]
    pub(crate) fn image_registry_stats(&self) -> (usize, usize, usize) {
        let images = self.images.borrow();
        (images.entries.len(), images.entries.capacity(), images.reclaimed)
    }
    fn live_state(self: &Rc<Self>) -> Option<SharedState> {
        let owner = self.owner.upgrade()?;
        let current = owner.try_borrow().ok()?.original_document.as_ref()
            .is_some_and(|document| Rc::ptr_eq(document, self));
        current.then_some(owner)
    }
}
// Storage is not permission. Canvas can share it, but cannot mint an image cap.
pub(crate) fn original_document(state: &SharedState) -> Rc<OriginalDocumentStorage> {
    let mut original = state.borrow_mut();
    original.original_document.get_or_insert_with(|| Rc::new(OriginalDocumentStorage {
        owner: Rc::downgrade(state), retired_dom: RefCell::new(None), images: Rc::new(RefCell::new(ImageRegistry::default())),
    })).clone()
}
pub(crate) fn retire_original_document(state: &mut ObscuraState) {
    state.canvas_document = None;
    if let Some(document) = state.original_document.take() {
        // The one owner transition shared by image and Canvas. Never clone a
        // tree, refresh a URL cache, call JS, or start layout/loading here.
        *document.retired_dom.borrow_mut() = state.dom.take();
    }
}
pub(crate) fn transfer_original_document(state: &mut ObscuraState) {
    if let Some(document) = state.original_document.take() {
        if let Some(dom) = state.dom.as_ref() {
            document.images.borrow_mut().prune();
            for image in document.images.borrow().entries.values().filter_map(std::rc::Weak::upgrade) {
                let mut image = image.borrow_mut();
                if dom.node_generation(image.node) == Some(image.generation) {
                    image.transferred_node = dom.get_node(image.node);
                }
            }
        }
    }
}

#[derive(Clone)]
pub(crate) struct ImageDocumentCapability {
    storage: Rc<OriginalDocumentStorage>,
    owner: Arc<Owner>,
}
impl deno_core::cppgc::GarbageCollected for ImageDocumentCapability {
    fn get_name(&self) -> &'static std::ffi::CStr { c"ImageDocumentCapability" }
}
impl ImageDocumentCapability {
    fn issue(state: &SharedState) -> Self {
        Self { storage: original_document(state), owner: state.borrow().websocket_owner.clone() }
    }
}
pub(crate) fn install(scope: &mut v8::HandleScope,
    initialize: v8::Local<v8::Function>, state: &SharedState,
) -> Option<()> {
    let owner = deno_core::cppgc::make_cppgc_object(scope, ImageDocumentCapability::issue(state));
    let receiver = v8::undefined(scope);
    initialize.call(scope, receiver.into(), &[owner.into()])?;
    Some(())
}

#[derive(Clone, Debug, PartialEq)]
struct ImageRequest {
    url: String,
    density: f32,
    profile: ImageRequestProfile,
}
#[derive(Clone)]
struct ImageContent {
    request: ImageRequest,
    // Original content lifetime, not a pair of dimension snapshots. The bytes
    // are shared with render cache and can outlive its eviction/navigation.
    encoded_bytes: Arc<[u8]>,
    raw_size: (f32, f32),
}
struct ImageRecord {
    node: NodeId,
    generation: u64,
    request: Option<ImageRequest>,
    content: Option<ImageContent>,
    pending: bool,
    failed: bool,
    transferred_node: Option<obscura_dom::tree::Node>,
    registry: std::rc::Weak<RefCell<ImageRegistry>>,
    retired_registration: std::rc::Weak<RefCell<Vec<ImageKey>>>,
    #[cfg(test)]
    content_reads: usize,
    #[cfg(test)]
    selection_reads: usize,
}
impl Drop for ImageRecord {
    fn drop(&mut self) {
        if let Some(registry) = self.registry.upgrade() {
            if let Ok(mut registry) = registry.try_borrow_mut() {
                if registry.entries.remove(&(self.node.raw(), self.generation)).is_some() {
                    #[cfg(test)] { registry.reclaimed += 1; }
                }
                registry.prune();
                return;
            };
        }
        if let Some(retired) = self.retired_registration.upgrade() {
            retired.borrow_mut().push((self.node.raw(), self.generation));
        }
    }
}
#[derive(Clone)]
pub(crate) struct NativeImageHandle {
    document: Rc<OriginalDocumentStorage>,
    activity_owner: Arc<Owner>,
    record: Rc<RefCell<ImageRecord>>,
}
impl deno_core::cppgc::GarbageCollected for NativeImageHandle {
    fn get_name(&self) -> &'static std::ffi::CStr { c"NativeImageHandle" }
}
impl NativeImageHandle {
    // A coalesced image response belongs to the original document and Owner,
    // not to the continued existence of its first consumer's native arena slot.
    pub(super) fn active_document_state(&self) -> Option<SharedState> {
        let state = self.document.live_state()?;
        let original = state.try_borrow().ok()?;
        if !self.activity_owner.active() || !Arc::ptr_eq(&original.websocket_owner, &self.activity_owner) { return None; }
        drop(original);
        Some(state)
    }
    // Element operations and completion still require the original node generation.
    pub(super) fn active_state(&self) -> Option<SharedState> {
        let state = self.active_document_state()?;
        let original = state.try_borrow().ok()?;
        let record = self.record.try_borrow().ok()?;
        if !original.dom.as_ref().is_some_and(|dom| dom.node_generation(record.node) == Some(record.generation)) { return None; }
        drop(original);
        Some(state)
    }
    pub(super) fn node(&self) -> NodeId { self.record.borrow().node }
    #[cfg(feature = "render")]
    pub(super) fn begin_request(&self) {
        let mut record = self.record.borrow_mut();
        if record.pending && record.content.as_ref().map(|content| &content.request) != record.request.as_ref() {
            // Blink's DoUpdateFromElement replaces ImageResourceContent when
            // a new request is committed, even before its intrinsic size exists.
            record.content = None;
        }
    }
    fn with_dom<T>(&self, f: impl FnOnce(&DomTree, NodeId) -> T) -> Option<T> {
        let record = self.record.borrow();
        let (node, generation) = (record.node, record.generation);
        drop(record);
        if let Some(state) = self.document.live_state() {
            let state = state.borrow();
            let dom = state.dom.as_ref()?;
            (dom.node_generation(node) == Some(generation)).then(|| f(dom, node))
        } else {
            let retired = self.document.retired_dom.borrow();
            let dom = retired.as_ref()?;
            (dom.node_generation(node) == Some(generation)).then(|| f(dom, node))
        }
    }
    fn attribute(&self, name: &str) -> Option<String> {
        self.attribute_ns(None, name)
    }
    fn attribute_ns(&self, namespace: Option<&str>, name: &str) -> Option<String> {
        let read = |node: &obscura_dom::tree::Node| match namespace {
            Some(ns) => node.get_attribute_ns(ns, name).map(str::to_owned),
            None => node.get_attribute(name).map(str::to_owned),
        };
        self.with_dom(|dom, node| dom.with_node(node, read).flatten())
            .unwrap_or_else(|| self.record.borrow().transferred_node.as_ref().and_then(read))
    }
    fn update_attribute(&self, command: &str, namespace: Option<&str>, name: &str, value: &str)
        -> Result<(), deno_error::JsErrorBox> {
        let write = |node: &mut obscura_dom::tree::Node| {
            match (command, namespace) {
                ("set_attribute", Some(ns)) => node.set_attribute_ns(ns, name, value.to_owned()),
                ("set_attribute", None) => node.set_attribute(name, value.to_owned()),
                (_, Some(ns)) => node.remove_attribute_ns(ns, name),
                (_, None) => {
                    if let Some(attrs) = node.attrs_mut() { attrs.retain(|a| !a.qualified_name_eq(name)); }
                }
            }
        };
        if let Some(state) = self.document.live_state() {
            if self.with_dom(|_, _| ()).is_none() { return Err(deno_error::JsErrorBox::type_error("Image node unavailable")); }
            let (command, arg) = match (command, namespace) {
                ("set_attribute", Some(ns)) => ("set_attribute_ns", format!("{ns}\0{name}\0{value}")),
                ("remove_attribute", Some(ns)) => ("remove_attribute_ns", format!("{ns}\0{name}")),
                ("set_attribute", None) => ("set_attribute", format!("{name}\0{value}")),
                (_, None) => ("remove_attribute", name.to_owned()),
                _ => return Err(deno_error::JsErrorBox::type_error("Invalid image attribute operation")),
            };
            op_dom_inner(state, command.to_owned(), self.node().raw().to_string(), arg);
        } else if self.with_dom(|dom, node| dom.with_node_mut(node, write)).is_none() {
            let mut record = self.record.borrow_mut();
            let node = record.transferred_node.as_mut()
                .ok_or_else(|| deno_error::JsErrorBox::type_error("Image node unavailable"))?;
            write(node);
        }
        if namespace.is_none_or(str::is_empty) && matches!(name, "src" | "srcset" | "sizes" | "crossorigin") {
            // Inactive SelectSourceURL/UpdateFromElement return before changing
            // content. Active refresh decides from the selected picture/srcset
            // candidate; empty fallback src alone does not invalidate content.
            #[cfg(feature = "render")]
            if let Some(state) = self.active_state() { refresh_record(&state.borrow(), &self.record); }
        }
        Ok(())
    }

}

// Request/content identity is maintained while live, not reconstructed at
// retirement. Cache eviction does not erase previously acquired content.
#[cfg(feature = "render")]
fn refresh_record(state: &ObscuraState, record: &Rc<RefCell<ImageRecord>>) {
    let mut record = record.borrow_mut();
    let Some(dom) = state.dom.as_ref().filter(|dom| dom.node_generation(record.node) == Some(record.generation)) else { return; };
    // The common immutable data-src path borrows only relevant attributes.
    // get_node clones the complete Node (including a potentially huge src).
    if let Some(content) = record.content.as_ref().filter(|_| !record.pending) {
        if content.request.url.starts_with("data:") && content.request.density == 1.0 {
            let plain = dom.with_node(record.node, |node| {
                let profile = ImageRequestProfile::from_crossorigin_attribute(node.get_attribute("crossorigin"));
                let same = node.get_attribute("src").is_some_and(|src| src.trim() == content.request.url.as_str())
                    && node.get_attribute("srcset").is_none_or(|srcset| srcset.trim().is_empty())
                    && profile == content.request.profile;
                (same, node.parent)
            });
            if let Some((true, parent)) = plain {
                let picture = parent.is_some_and(|parent| dom.with_node(parent, |node|
                    node.as_element().is_some_and(|name| name.local.as_ref() == "picture")).unwrap_or(false));
                if !picture { return; }
            }
        }
    }
    #[cfg(test)] { record.selection_reads += 1; }
    let base_url = document_base_url(state);
    let Some((url, density, profile)) = state.render_resources.image_element_request(
        dom, record.node, state.viewport, base_url.as_deref()) else {
        record.request = None; record.content = None; record.pending = false; record.failed = false; return;
    };
    let request = ImageRequest { url, density, profile };
    let changed = record.request.as_ref() != Some(&request);
    record.request = Some(request.clone());
    // Responsive data selections still reselect for real source/density/base
    // changes, but an unchanged immutable body never needs another decode.
    if request.url.starts_with("data:") {
        if let Some(content) = record.content.as_mut().filter(|content|
            content.request.url == request.url && content.request.profile == request.profile) {
            content.request = request; record.pending = false; record.failed = false; return;
        }
        if !changed && record.failed { return; }
    }
    #[cfg(test)] { record.content_reads += 1; }
    match state.render_resources.cached_image_content_bytes(&request.url, request.profile) {
        None => {
            if let Some(content) = record.content.as_mut().filter(|content|
                content.request.url == request.url && content.request.profile == request.profile) {
                content.request = request; record.pending = false; record.failed = false;
            } else if changed || record.content.is_none() { record.pending = true; }
        }
        Some(None) => { record.content = None; record.pending = false; record.failed = true; }
        Some(Some(encoded_bytes)) => {
            if let Some(content) = record.content.as_mut().filter(|content|
                content.request.url == request.url && content.request.profile == request.profile
                    && Arc::ptr_eq(&content.encoded_bytes, &encoded_bytes)) {
                content.request = request; record.pending = false; record.failed = false; return;
            }
            record.pending = false;
            let dimensions = obscura_render::image_intrinsic_dimensions(&encoded_bytes);
            record.failed = dimensions.is_none();
            record.content = dimensions.map(|raw_size| ImageContent { request, encoded_bytes, raw_size });
        }
    }
}
#[cfg(feature = "render")]
pub(crate) fn attribute_changed(state: &ObscuraState, node: NodeId, name: &str) {
    if !matches!(name, "src" | "srcset" | "sizes" | "crossorigin") || !state.websocket_owner.active() { return; }
    let Some(document) = &state.original_document else { return; };
    let Some(generation) = state.dom.as_ref().and_then(|dom| dom.node_generation(node)) else { return; };
    let record = document.images.borrow().entries.get(&(node.raw(), generation)).and_then(std::rc::Weak::upgrade);
    if let Some(record) = record { refresh_record(state, &record); }
}

#[cfg(feature = "render")]
pub(crate) fn publish_cached_images(state: &ObscuraState) {
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
    if let Some(document) = &state.original_document {
        if !state.websocket_owner.active() { return; }
        {
            let mut images = document.images.borrow_mut();
            images.prune();
            #[cfg(test)] { images.publication_visits += images.entries.len(); }
        }
        for record in document.images.borrow().entries.values().filter_map(std::rc::Weak::upgrade) {
            refresh_record(state, &record);
        }
        document.images.borrow_mut().prune();
    }
    }));
    if outcome.is_err() { tracing::error!("image content publication failed"); }
}

#[op2(fast)]
pub(crate) fn op_image_same_document(#[cppgc] a: &ImageDocumentCapability,
    #[cppgc] b: &ImageDocumentCapability) -> bool {
    Rc::ptr_eq(&a.storage, &b.storage) && Arc::ptr_eq(&a.owner, &b.owner)
}

#[op2(fast)]
pub(crate) fn op_image_cache_node(#[cppgc] owner: &ImageDocumentCapability, nid: u32) -> bool {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let Some(state) = owner.storage.live_state() else { return false; };
        let state = state.borrow();
        owner.owner.active() && Arc::ptr_eq(&state.websocket_owner, &owner.owner)
            && state.dom.as_ref().is_some_and(|dom| dom.get_node(NodeId::new(nid)).is_some_and(|node|
                // Every HTML element now has an original-document popover
                // handle. A colliding cached Text/SVG wrapper must be evicted
                // as well; cache eviction does not reauthorize that old object.
                node.as_element().is_some_and(|name| name.ns.as_ref() == "http://www.w3.org/1999/xhtml")))
    })).unwrap_or(false)
}

#[op2]
#[cppgc]
pub(crate) fn op_image_bind_handle(#[cppgc] owner: &ImageDocumentCapability, nid: u32)
    -> Result<NativeImageHandle, deno_error::JsErrorBox> { bind_image(owner, nid) }

fn bind_image(owner: &ImageDocumentCapability, nid: u32)
    -> Result<NativeImageHandle, deno_error::JsErrorBox> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let state = owner.storage.live_state().ok_or_else(|| deno_error::JsErrorBox::type_error("Image document unavailable"))?;
        let state = state.borrow();
        if !owner.owner.active() || !Arc::ptr_eq(&state.websocket_owner, &owner.owner) {
            return Err(deno_error::JsErrorBox::type_error("Image document retired"));
        }
        let dom = state.dom.as_ref().ok_or_else(|| deno_error::JsErrorBox::type_error("Image document unavailable"))?;
        let node = NodeId::new(nid);
        let generation = dom.node_generation(node).ok_or_else(|| deno_error::JsErrorBox::type_error("Illegal invocation"))?;
        if !dom.get_node(node).is_some_and(|node| node.as_element().is_some_and(|name|
            name.local.as_ref() == "img" && name.ns.as_ref() == "http://www.w3.org/1999/xhtml")) {
            return Err(deno_error::JsErrorBox::type_error("Illegal invocation"));
        }
        let key = (nid, generation);
        owner.storage.images.borrow_mut().prune();
        let existing = owner.storage.images.borrow().entries.get(&key).and_then(std::rc::Weak::upgrade);
        let record = existing.unwrap_or_else(|| {
            let record = Rc::new(RefCell::new(ImageRecord { node, generation, request: None, content: None,
                pending: false, failed: false, transferred_node: None,
                registry: Rc::downgrade(&owner.storage.images),
                retired_registration: Rc::downgrade(&owner.storage.images.borrow().retired),
                #[cfg(test)] content_reads: 0,
                #[cfg(test)] selection_reads: 0 }));
            owner.storage.images.borrow_mut().entries.insert(key, Rc::downgrade(&record));
            record
        });
        #[cfg(feature = "render")]
        refresh_record(&state, &record);
        Ok(NativeImageHandle { document: owner.storage.clone(), activity_owner: owner.owner.clone(), record })
    })).unwrap_or_else(|_| Err(deno_error::JsErrorBox::generic("Image binding failed")))
}

fn nonnegative_attribute(value: &str) -> Option<u32> {
    let bytes = value.as_bytes(); let mut i = 0;
    while bytes.get(i).is_some_and(|v| matches!(v, b' ' | b'\t' | b'\n' | b'\r' | 0x0c)) { i += 1; }
    let negative = bytes.get(i) == Some(&b'-');
    if negative || bytes.get(i) == Some(&b'+') { i += 1; }
    let first = i; let mut number = 0u32;
    while let Some(digit) = bytes.get(i).filter(|v| v.is_ascii_digit()) {
        number = number.checked_mul(10)?.checked_add(u32::from(*digit - b'0'))?; i += 1;
    }
    (i != first && (!negative || number == 0)).then_some(number)
}
#[op2(fast)]
pub(crate) fn op_image_idl_dimension(#[cppgc] handle: &NativeImageHandle, height: bool)
    -> Result<u32, deno_error::JsErrorBox> {
    image_idl_dimension(handle, height)
}
// The op and native tests share this exact panic-safe implementation. op2 turns
// the annotated declaration into an OpDecl constructor, not a callable getter.
fn image_idl_dimension(handle: &NativeImageHandle, height: bool)
    -> Result<u32, deno_error::JsErrorBox> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        #[cfg(feature = "render")]
        if let Some(state) = handle.active_state() {
            let mut state = state.borrow_mut();
            refresh_record(&state, &handle.record);
            let node = handle.node();
            let connected = state.dom.as_ref().is_some_and(|dom| dom.is_connected(node)
                && dom.shadow_including_root(node) == Some(dom.document()));
            if connected {
                sample_live_document_animations(&mut state);
                ensure_prepared_geometry(&mut state);
                if let Some(size) = state.prepared_render.as_ref().zip(state.dom.as_ref())
                    .and_then(|(prepared, dom)| prepared.image_content_size(dom, node)) {
                    return Ok((if height { size.1 } else { size.0 }).round() as u32);
                }
            }
        }
        if let Some(value) = handle.attribute(if height { "height" } else { "width" }).as_deref().and_then(nonnegative_attribute) { return Ok(value); }
        let record = handle.record.borrow();
        let Some(content) = &record.content else { return Ok(0); };
        let value = if height { content.raw_size.1 } else { content.raw_size.0 };
        // Existing decoder sizing conventions retained. SVG density/fixed-point
        // rounding and Content-DPR still require the stated native oracle gates.
        Ok(value.round() as u32)
    })).unwrap_or_else(|_| Err(deno_error::JsErrorBox::generic("Image size unavailable")))
}
#[op2]
#[string]
pub(crate) fn op_image_attribute(#[cppgc] handle: &NativeImageHandle,
    #[string] command: &str, #[string] name: &str, #[string] value: &str,
) -> Result<String, deno_error::JsErrorBox> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let (command, namespace, name) = if let Some(operation) = command.strip_suffix("_ns") {
            let (ns, name) = name.split_once('\0').ok_or_else(|| deno_error::JsErrorBox::type_error("Invalid image namespace"))?;
            (operation, Some(ns), name)
        } else { (command, None, name) };
        if command == "attribute_names" {
            let names = |node: &obscura_dom::tree::Node| node.attrs().map(|attrs|
                attrs.iter().map(|attr| attr.qualified_name()).collect::<Vec<String>>()).unwrap_or_default();
            let values = handle.with_dom(|dom, node| dom.get_node(node).map(|node| names(&node)))
                .flatten().or_else(|| handle.record.borrow().transferred_node.as_ref().map(names)).unwrap_or_default();
            return Ok(serde_json::to_string(&values).unwrap());
        }
        if command == "get_attribute" { return Ok(serde_json::to_string(&handle.attribute_ns(namespace, name)).unwrap()); }
        if !matches!(command, "set_attribute" | "remove_attribute") { return Err(deno_error::JsErrorBox::type_error("Invalid image attribute operation")); }
        handle.update_attribute(command, namespace, name, value)?;
        Ok("true".into())
    })).unwrap_or_else(|_| Err(deno_error::JsErrorBox::generic("Image attribute unavailable")))
}
#[op2(fast)]
pub(crate) fn op_image_active(#[cppgc] handle: &NativeImageHandle) -> bool {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| handle.active_state().is_some())).unwrap_or(false)
}
#[op2]
#[string]
pub(crate) fn op_image_handle_metadata(#[cppgc] handle: &NativeImageHandle) -> Result<String, deno_error::JsErrorBox> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        #[cfg(feature = "render")]
        if let Some(state) = handle.active_state() { refresh_record(&state.borrow(), &handle.record); }
        let record = handle.record.borrow();
        let current_src = record.request.as_ref().map(|request| request.url.as_str()).unwrap_or("");
        if record.pending { return Ok(serde_json::json!({"state":"pending","currentSrc":current_src}).to_string()); }
        match &record.content {
            Some(content) => Ok(serde_json::json!({"state":"loaded","ok":true,"currentSrc":content.request.url,
                "density":content.request.density,"width":content.raw_size.0/content.request.density,
                "height":content.raw_size.1/content.request.density}).to_string()),
            None => Ok(serde_json::json!({"state":"error","ok":false,"currentSrc":current_src}).to_string()),
        }
    })).unwrap_or_else(|_| Err(deno_error::JsErrorBox::generic("Image metadata unavailable")))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn state() -> SharedState {
        let persona = obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145);
        let state = Rc::new(RefCell::new(ObscuraState::new(persona)));
        state.borrow_mut().dom = Some(obscura_dom::parse_html("<img id='i' width='17'>"));
        state
    }
    #[test]
    fn image_nonnegative_parser_matches_pinned_cases() {
        for (text, expected) in [("",None),(" \t+12px",Some(12)),("-0junk",Some(0)),("-1",None),
            ("0x10",Some(0)),("4294967295",Some(u32::MAX)),("4294967296",None),("\u{a0}7",None)] {
            assert_eq!(nonnegative_attribute(text), expected, "{text:?}");
        }
    }
    #[test]
    fn image_storage_survives_state_but_does_not_keep_state_or_retire_siblings() {
        let state = state();
        let weak = Rc::downgrade(&state);
        let cap = ImageDocumentCapability::issue(&state);
        let node = state.borrow().dom.as_ref().unwrap().get_element_by_id("i").unwrap();
        let first = bind_image(&cap, node.raw()).unwrap();
        let sibling = bind_image(&cap, node.raw()).unwrap();
        drop(first);
        assert!(cap.owner.active());
        assert!(sibling.active_state().is_some());
        // Parser/input lifetime changes do not change this document identity.
        state.borrow().input_document_epoch.set(19);
        assert!(sibling.active_state().is_some());
        retire_original_document(&mut state.borrow_mut());
        cap.owner.retire();
        assert!(sibling.active_state().is_none());
        assert_eq!(sibling.attribute("width").as_deref(),Some("17"));
        drop(state);
        assert!(weak.upgrade().is_none());
        sibling.update_attribute("set_attribute",None,"width","23").unwrap();
        assert_eq!(sibling.attribute("width").as_deref(),Some("23"));
    }
    #[cfg(feature = "render")]
    #[test]
    fn image_content_retains_encoded_bytes_until_active_request_commit() {
        let state=state();let cap=ImageDocumentCapability::issue(&state);
        let node=state.borrow().dom.as_ref().unwrap().get_element_by_id("i").unwrap();
        let handle=bind_image(&cap,node.raw()).unwrap();
        let body: Arc<[u8]>=Arc::from(&b"<svg xmlns='http://www.w3.org/2000/svg' width='24' height='12'/>"[..]);
        state.borrow_mut().render_resources.seed_image_shared("https://image.invalid/old.svg".into(),
            ImageRequestProfile::NoCorsInclude,body.clone());
        handle.update_attribute("set_attribute",None,"src","https://image.invalid/old.svg").unwrap();
        assert!(Arc::ptr_eq(&handle.record.borrow().content.as_ref().unwrap().encoded_bytes,&body));
        state.borrow_mut().render_resources=obscura_render::RenderResourceCache::default();
        publish_cached_images(&state.borrow());
        assert!(Arc::ptr_eq(&handle.record.borrow().content.as_ref().unwrap().encoded_bytes,&body));
        handle.update_attribute("set_attribute",None,"src","https://image.invalid/next.svg").unwrap();
        assert!(handle.record.borrow().pending);
        assert!(Arc::ptr_eq(&handle.record.borrow().content.as_ref().unwrap().encoded_bytes,&body));
        handle.begin_request();
        assert!(handle.record.borrow().content.is_none());
        retire_original_document(&mut state.borrow_mut());
        handle.update_attribute("set_attribute",None,"src","").unwrap();
        assert!(handle.record.borrow().content.is_none());
        assert!(handle.record.borrow().pending, "inactive edits do not commit an already pending request");
    }
    #[test]
    fn image_transfer_retains_original_node_without_aliasing_transferred_tree() {
        let state=state();let cap=ImageDocumentCapability::issue(&state);
        let node=state.borrow().dom.as_ref().unwrap().get_element_by_id("i").unwrap();
        let handle=bind_image(&cap,node.raw()).unwrap();
        transfer_original_document(&mut state.borrow_mut());
        let transferred=state.borrow_mut().dom.take().unwrap();
        assert!(handle.active_state().is_none());
        handle.update_attribute("set_attribute",None,"width","31").unwrap();
        assert_eq!(handle.attribute("width").as_deref(),Some("31"));
        assert_eq!(transferred.get_node(node).unwrap().get_attribute("width"),Some("17"));
        assert!(cap.owner.active());
    }
    #[test]
    fn image_same_storage_cannot_reactivate_capability_after_host_owner_reset() {
        let state=state();let cap=ImageDocumentCapability::issue(&state);
        let node=state.borrow().dom.as_ref().unwrap().get_element_by_id("i").unwrap();
        let handle=bind_image(&cap,node.raw()).unwrap();
        let replacement=Arc::new(Owner::new(cap.owner.budget.clone()));
        state.borrow_mut().websocket_owner=replacement;
        assert!(cap.owner.active());
        assert!(handle.active_state().is_none());
        assert!(bind_image(&cap,node.raw()).is_err());
    }
    #[cfg(feature = "render")]
    #[test]
    fn image_late_completion_is_rejected_after_original_storage_retirement() {
        let state=state();let cap=ImageDocumentCapability::issue(&state);
        let node=state.borrow().dom.as_ref().unwrap().get_element_by_id("i").unwrap();
        let handle=bind_image(&cap,node.raw()).unwrap();
        let generation=state.borrow().document_generation;
        retire_original_document(&mut state.borrow_mut());
        {
            let mut state=state.borrow_mut();
            state.dom=Some(obscura_dom::parse_html("<img id='i' width='99'>"));
            state.document_generation=state.document_generation.wrapping_add(1);
        }
        assert_eq!(finish_async_image_metadata(&handle,&state,node,generation,"https://image.invalid/old.svg",
            ImageRequestProfile::NoCorsInclude),"{\"state\":\"retired\"}");
        assert_eq!(handle.attribute("width").as_deref(),Some("17"));
        let state=state.borrow();let dom=state.dom.as_ref().unwrap();
        assert_eq!(dom.get_node(dom.get_element_by_id("i").unwrap()).unwrap().get_attribute("width"),Some("99"));
    }
    #[test]
    fn image_native_node_generation_rejects_recycled_slot() {
        let state=state();let cap=ImageDocumentCapability::issue(&state);
        let node=state.borrow().dom.as_ref().unwrap().get_element_by_id("i").unwrap();
        let handle=bind_image(&cap,node.raw()).unwrap();
        let saved=state.borrow().dom.as_ref().unwrap().get_node(node).unwrap().data;
        let state_ref=state.borrow();let dom=state_ref.dom.as_ref().unwrap();
        dom.remove(node);let replacement=dom.new_node(saved);assert_eq!(replacement,node);
        drop(state_ref);
        assert!(handle.active_state().is_none());
        assert!(handle.update_attribute("set_attribute",None,"width","91").is_err());
        assert_eq!(state.borrow().dom.as_ref().unwrap().get_node(replacement).unwrap().get_attribute("width"),Some("17"));
    }
    #[cfg(feature = "render")]
    #[test]
    fn image_completed_inactive_empty_source_preserves_content_without_pending_task() {
        let state=state();let cap=ImageDocumentCapability::issue(&state);
        let node=state.borrow().dom.as_ref().unwrap().get_element_by_id("i").unwrap();
        let handle=bind_image(&cap,node.raw()).unwrap();
        let body: Arc<[u8]>=Arc::from(&b"<svg xmlns='http://www.w3.org/2000/svg' width='24' height='12'/>"[..]);
        state.borrow_mut().render_resources.seed_image_shared("https://image.invalid/ready.svg".into(),
            ImageRequestProfile::NoCorsInclude,body.clone());
        handle.update_attribute("remove_attribute",None,"width","").unwrap();
        handle.update_attribute("set_attribute",None,"src","https://image.invalid/ready.svg").unwrap();
        assert!(!handle.record.borrow().pending);
        retire_original_document(&mut state.borrow_mut());
        for (command, value) in [("set_attribute", ""), ("set_attribute", "https://image.invalid/inactive.svg"), ("remove_attribute", "")] {
            handle.update_attribute(command,None,"src",value).unwrap();
            assert!(Arc::ptr_eq(&handle.record.borrow().content.as_ref().unwrap().encoded_bytes,&body));
            assert!(!handle.record.borrow().pending);
            assert_eq!(image_idl_dimension(&handle,false).unwrap(),24);
        }
    }
    #[cfg(feature = "render")]
    #[test]
    fn image_active_fallback_src_edits_preserve_selected_srcset_and_picture_after_eviction() {
        for html in ["<img id='i' src='/fallback.svg' srcset='https://image.invalid/selected.svg 2x' style='display:none'>",
            "<picture><source srcset='https://image.invalid/selected.svg 2x'><img id='i' src='/fallback.svg' style='display:none'></picture>"] {
            let state=state();state.borrow_mut().dom=Some(obscura_dom::parse_html(html));
            let cap=ImageDocumentCapability::issue(&state);
            let node=state.borrow().dom.as_ref().unwrap().get_element_by_id("i").unwrap();
            let body: Arc<[u8]>=Arc::from(&b"<svg xmlns='http://www.w3.org/2000/svg' width='24' height='12'/>"[..]);
            state.borrow_mut().render_resources.seed_image_shared("https://image.invalid/selected.svg".into(),
                ImageRequestProfile::NoCorsInclude,body.clone());
            let handle=bind_image(&cap,node.raw()).unwrap();
            state.borrow_mut().render_resources=obscura_render::RenderResourceCache::default();
            for namespace in [None,Some("")] {
                handle.update_attribute("set_attribute",namespace,"src","").unwrap();
                handle.update_attribute("remove_attribute",namespace,"src","").unwrap();
                let record=handle.record.borrow();let content=record.content.as_ref().unwrap();
                assert!(Arc::ptr_eq(&content.encoded_bytes,&body));
                assert_eq!(content.request.density,2.0);assert!(!record.pending);
            }
            assert_eq!(image_idl_dimension(&handle,false).unwrap(),24);
        }
    }
    #[cfg(feature = "render")]
    #[test]
    fn image_active_empty_selected_source_clears_content() {
        let state=state();let cap=ImageDocumentCapability::issue(&state);
        let node=state.borrow().dom.as_ref().unwrap().get_element_by_id("i").unwrap();
        state.borrow_mut().render_resources.seed_image("https://image.invalid/ready.svg".into(),
            ImageRequestProfile::NoCorsInclude,b"<svg xmlns='http://www.w3.org/2000/svg' width='24' height='12'/>".to_vec());
        let handle=bind_image(&cap,node.raw()).unwrap();
        handle.update_attribute("set_attribute",None,"src","https://image.invalid/ready.svg").unwrap();
        assert!(handle.record.borrow().content.is_some());
        handle.update_attribute("set_attribute",None,"src","").unwrap();
        assert!(handle.record.borrow().request.is_none());assert!(handle.record.borrow().content.is_none());
        assert!(!handle.record.borrow().pending);
    }
    #[cfg(feature = "render")]
    #[test]
    fn image_stable_large_data_source_reuses_content_and_avoids_selection_and_body_reads() {
        let state=state();let cap=ImageDocumentCapability::issue(&state);
        let node=state.borrow().dom.as_ref().unwrap().get_element_by_id("i").unwrap();
        let handle=bind_image(&cap,node.raw()).unwrap();
        handle.update_attribute("remove_attribute",None,"width","").unwrap();
        handle.update_attribute("set_attribute",None,"style","display:none").unwrap();
        let url=format!("data:image/svg+xml,<svg xmlns='http://www.w3.org/2000/svg' width='24' height='12'><!--{}--></svg>","x".repeat(65536));
        handle.update_attribute("set_attribute",None,"src",&url).unwrap();
        let (body,reads,selections)={let r=handle.record.borrow();
            (r.content.as_ref().unwrap().encoded_bytes.clone(),r.content_reads,r.selection_reads)};
        for _ in 0..256 {
            assert_eq!(image_idl_dimension(&handle,false).unwrap(),24);
            assert_eq!(image_idl_dimension(&handle,true).unwrap(),12);
        }
        {let r=handle.record.borrow();assert!(Arc::ptr_eq(&r.content.as_ref().unwrap().encoded_bytes,&body));
            assert_eq!(r.content_reads,reads);assert_eq!(r.selection_reads,selections);}
        let next="data:image/svg+xml,<svg xmlns='http://www.w3.org/2000/svg' width='31' height='7'/>";
        handle.update_attribute("set_attribute",None,"src",next).unwrap();
        assert_eq!(image_idl_dimension(&handle,false).unwrap(),31);
        assert!(!Arc::ptr_eq(&handle.record.borrow().content.as_ref().unwrap().encoded_bytes,&body));
        assert!(handle.record.borrow().content_reads>reads);
        // Adding responsive selection must bypass the plain-data fast path.
        handle.update_attribute("set_attribute",None,"srcset","https://image.invalid/new.svg 2x").unwrap();
        assert_eq!(handle.record.borrow().request.as_ref().unwrap().url,"https://image.invalid/new.svg");
        assert!(handle.record.borrow().pending);
    }
    #[cfg(feature = "render")]
    #[test]
    fn image_same_url_network_republication_replaces_arc_and_profile_is_not_ignored() {
        let state=state();let cap=ImageDocumentCapability::issue(&state);
        let node=state.borrow().dom.as_ref().unwrap().get_element_by_id("i").unwrap();
        let handle=bind_image(&cap,node.raw()).unwrap();
        let url="https://image.invalid/same.svg";
        let first: Arc<[u8]>=Arc::from(&b"<svg xmlns='http://www.w3.org/2000/svg' width='24' height='12'/>"[..]);
        let second: Arc<[u8]>=Arc::from(&b"<svg xmlns='http://www.w3.org/2000/svg' width='39' height='18'/>"[..]);
        state.borrow_mut().render_resources.seed_image_shared(url.into(),ImageRequestProfile::NoCorsInclude,first.clone());
        handle.update_attribute("set_attribute",None,"src",url).unwrap();
        state.borrow_mut().render_resources.seed_image_shared(url.into(),ImageRequestProfile::NoCorsInclude,second.clone());
        publish_cached_images(&state.borrow());
        assert!(Arc::ptr_eq(&handle.record.borrow().content.as_ref().unwrap().encoded_bytes,&second));
        assert_eq!(handle.record.borrow().content.as_ref().unwrap().raw_size,(39.0,18.0));
        handle.update_attribute("set_attribute",None,"crossorigin","anonymous").unwrap();
        assert_eq!(handle.record.borrow().request.as_ref().unwrap().profile,ImageRequestProfile::CorsSameOrigin);
        assert!(handle.record.borrow().pending);
        state.borrow_mut().render_resources.seed_image_shared(url.into(),ImageRequestProfile::CorsSameOrigin,first.clone());
        publish_cached_images(&state.borrow());
        assert!(Arc::ptr_eq(&handle.record.borrow().content.as_ref().unwrap().encoded_bytes,&first));
    }
    #[cfg(feature = "render")]
    #[test]
    fn image_registry_churn_reclaims_dead_keys_and_bounds_future_publication_work() {
        let state=state();let cap=ImageDocumentCapability::issue(&state);
        let original=state.borrow().dom.as_ref().unwrap().get_element_by_id("i").unwrap();
        let data=state.borrow().dom.as_ref().unwrap().get_node(original).unwrap().data;
        let saved=bind_image(&cap,original.raw()).unwrap();
        let sibling=bind_image(&cap,original.raw()).unwrap();drop(sibling);
        for _ in 0..3 {
            for _ in 0..2048 {
                let node=state.borrow().dom.as_ref().unwrap().new_node(data.clone());
                let handle=bind_image(&cap,node.raw()).unwrap();drop(handle);
                state.borrow().dom.as_ref().unwrap().remove(node);
            }
            let before=cap.storage.images.borrow().publication_visits;
            publish_cached_images(&state.borrow());
            let images=cap.storage.images.borrow();
            assert_eq!(images.entries.len(),1);assert!(images.entries.capacity()<=64);
            assert!(images.retired.borrow().is_empty());assert!(images.retired.borrow().capacity()<=64);
            assert_eq!(images.publication_visits-before,1);
        }
        // Also reclaim a large formerly-live cohort, including HashMap capacity.
        let mut cohort=Vec::new();
        for _ in 0..1024 {let node=state.borrow().dom.as_ref().unwrap().new_node(data.clone());
            cohort.push(bind_image(&cap,node.raw()).unwrap());}
        drop(cohort);publish_cached_images(&state.borrow());
        assert_eq!(cap.storage.images.borrow().entries.len(),1);
        assert!(cap.storage.images.borrow().entries.capacity()<=64);
        assert_eq!(cap.storage.images.borrow().reclaimed,3*2048+1024);
        assert_eq!(saved.attribute("width").as_deref(),Some("17"));
        assert!(saved.active_state().is_some());
    }
    #[cfg(feature = "render")]
    #[test]
    fn image_async_leader_rechecks_owner_before_seed_and_wakes_followers_on_discard() {
        let state=state();let cap=ImageDocumentCapability::issue(&state);
        let node=state.borrow().dom.as_ref().unwrap().get_element_by_id("i").unwrap();
        let leader=bind_image(&cap,node.raw()).unwrap();let follower=bind_image(&cap,node.raw()).unwrap();
        let generation=state.borrow().document_generation;
        let url="https://image.invalid/async.svg";
        let key=(generation,url.to_owned(),ImageRequestProfile::NoCorsInclude);
        let (tx,mut rx)=tokio::sync::oneshot::channel();
        state.borrow_mut().render_image_in_flight.insert(key.clone(),vec![tx]);
        state.borrow_mut().websocket_owner=Arc::new(Owner::new(cap.owner.budget.clone()));
        assert!(cap.owner.active()); // generation and old Owner active alone are insufficient
        publish_async_image_response(&leader,&state,&key,
            Some(b"<svg xmlns='http://www.w3.org/2000/svg' width='24' height='12'/>".to_vec()));
        assert!(rx.try_recv().is_ok());assert!(state.borrow().render_image_in_flight.is_empty());
        assert!(state.borrow().render_resources.cached_image_content_bytes(url,key.2).is_none());
        assert_eq!(finish_async_image_metadata(&follower,&state,node,generation,url,key.2),"{\"state\":\"retired\"}");
        // Positive control takes the same real publication function with a new original capability.
        let current=ImageDocumentCapability::issue(&state);let next=bind_image(&current,node.raw()).unwrap();
        let (tx,mut rx)=tokio::sync::oneshot::channel();state.borrow_mut().render_image_in_flight.insert(key.clone(),vec![tx]);
        publish_async_image_response(&next,&state,&key,
            Some(b"<svg xmlns='http://www.w3.org/2000/svg' width='39' height='18'/>".to_vec()));
        assert!(rx.try_recv().is_ok());assert!(state.borrow().render_resources.cached_image_content_bytes(url,key.2).unwrap().is_some());
    }

    #[cfg(feature = "render")]
    #[test]
    fn image_retained_data_profile_and_responsive_density_changes_are_observed() {
        let state=state();let cap=ImageDocumentCapability::issue(&state);
        let node=state.borrow().dom.as_ref().unwrap().get_element_by_id("i").unwrap();
        let handle=bind_image(&cap,node.raw()).unwrap();
        let data="data:image/svg+xml,<svg xmlns='http://www.w3.org/2000/svg' width='24' height='12'/>";
        handle.update_attribute("set_attribute",None,"src",data).unwrap();
        for attribute in [Some("use-credentials"),Some(" use-credentials "),Some("anonymous"),None] {
            match attribute {Some(value)=>handle.update_attribute("set_attribute",None,"crossorigin",value).unwrap(),
                None=>handle.update_attribute("remove_attribute",None,"crossorigin","").unwrap()};
            let expected=ImageRequestProfile::from_crossorigin_attribute(attribute);
            assert_eq!(handle.record.borrow().request.as_ref().unwrap().profile,expected);
            assert_eq!(handle.record.borrow().content.as_ref().unwrap().request.profile,expected);
            // The current existing parser trims whitespace, for both cache selection and fast path.
            assert_eq!(expected,match attribute {Some("anonymous")=>ImageRequestProfile::CorsSameOrigin,
                Some(_)=>ImageRequestProfile::CorsInclude,None=>ImageRequestProfile::NoCorsInclude});
        }
        let url="https://image.invalid/density.svg";
        let body: Arc<[u8]>=Arc::from(&b"<svg xmlns='http://www.w3.org/2000/svg' width='24' height='12'/>"[..]);
        state.borrow_mut().render_resources.seed_image_shared(url.into(),ImageRequestProfile::NoCorsInclude,body.clone());
        handle.update_attribute("set_attribute",None,"srcset",&format!("{url} 1x")).unwrap();
        state.borrow_mut().render_resources=obscura_render::RenderResourceCache::default();
        handle.update_attribute("set_attribute",None,"srcset",&format!("{url} 2x")).unwrap();
        let r=handle.record.borrow();let content=r.content.as_ref().unwrap();
        assert!(Arc::ptr_eq(&content.encoded_bytes,&body));assert_eq!(content.request.density,2.0);assert!(!r.pending);
    }
    #[test]
    fn image_registry_deferred_drop_drains_without_touching_live_sibling() {
        let state=state();let cap=ImageDocumentCapability::issue(&state);
        let node=state.borrow().dom.as_ref().unwrap().get_element_by_id("i").unwrap();
        let saved=bind_image(&cap,node.raw()).unwrap();
        let other=state.borrow().dom.as_ref().unwrap().new_node(state.borrow().dom.as_ref().unwrap().get_node(node).unwrap().data);
        let temporary=bind_image(&cap,other.raw()).unwrap();
        { let registry=cap.storage.images.borrow();drop(temporary);assert_eq!(registry.retired.borrow().len(),1); }
        let sibling=bind_image(&cap,node.raw()).unwrap();
        assert_eq!(cap.storage.images.borrow().entries.len(),1);
        assert!(Rc::ptr_eq(&saved.record,&sibling.record));
        retire_original_document(&mut state.borrow_mut());
        drop(saved);drop(sibling);
        assert_eq!(cap.storage.images.borrow().entries.len(),0,"retired final drop must not require another bind/publication");
    }

    #[cfg(feature = "render")]
    #[test]
    fn image_shared_response_survives_native_leader_recycle_for_distinct_live_follower() {
        let state=state();
        state.borrow_mut().dom=Some(obscura_dom::parse_html("<img id='leader' src='https://image.invalid/shared.svg'><img id='follower' src='https://image.invalid/shared.svg'>"));
        let cap=ImageDocumentCapability::issue(&state);
        let (a,b)={let state=state.borrow();let dom=state.dom.as_ref().unwrap();
            (dom.get_element_by_id("leader").unwrap(),dom.get_element_by_id("follower").unwrap())};
        assert_ne!(a,b);
        let leader=bind_image(&cap,a.raw()).unwrap();let follower=bind_image(&cap,b.raw()).unwrap();
        let generation=state.borrow().document_generation;
        let url="https://image.invalid/shared.svg";
        let key=(generation,url.to_owned(),ImageRequestProfile::NoCorsInclude);
        let (tx,mut rx)=tokio::sync::oneshot::channel();
        state.borrow_mut().render_image_in_flight.insert(key.clone(),vec![tx]);
        // This is the public native DomTree arena-removal boundary. Ordinary JS
        // removeChild/detach leaves the arena node alive and does not trigger it.
        {let state=state.borrow();let dom=state.dom.as_ref().unwrap();
            let data=dom.get_node(a).unwrap().data;
            let old_generation=dom.node_generation(a).unwrap();
            dom.remove(a);let replacement=dom.new_node(data);
            assert_eq!(replacement,a);assert_ne!(dom.node_generation(replacement),Some(old_generation));}
        assert!(leader.active_state().is_none());
        assert!(leader.active_document_state().is_some());
        assert!(follower.active_state().is_some());
        publish_async_image_response(&leader,&state,&key,
            Some(b"<svg xmlns='http://www.w3.org/2000/svg' width='24' height='12'/>".to_vec()));
        assert!(rx.try_recv().is_ok());assert!(state.borrow().render_image_in_flight.is_empty());
        let published=state.borrow().render_resources.cached_image_content_bytes(url,key.2).unwrap().unwrap();
        assert_eq!(finish_async_image_metadata(&leader,&state,a,generation,url,key.2),"{\"state\":\"retired\"}");
        let loaded:serde_json::Value=serde_json::from_str(&finish_async_image_metadata(&follower,&state,b,generation,url,key.2)).unwrap();
        assert_eq!(loaded["state"],"loaded");assert_eq!(loaded["ok"],true);
        assert_eq!(loaded["width"].as_f64(),Some(24.0));assert_eq!(loaded["height"].as_f64(),Some(12.0));
        assert_eq!(loaded["currentSrc"],url);
        // One shared response is seeded by the actual leader publication path;
        // follower completion observes that same immutable Arc without reseeding.
        let cached=state.borrow().render_resources.cached_image_content_bytes(url,key.2).unwrap().unwrap();
        assert!(Arc::ptr_eq(&published,&cached));
        assert!(Arc::ptr_eq(&published,&follower.record.borrow().content.as_ref().unwrap().encoded_bytes));
    }

    #[cfg(feature = "render")]
    #[test]
    fn image_retired_owner_keeps_dom_until_state_drop_without_async_publication() {
        let state=state();let weak=Rc::downgrade(&state);
        let cap=ImageDocumentCapability::issue(&state);
        let node=state.borrow().dom.as_ref().unwrap().get_element_by_id("i").unwrap();
        let handle=bind_image(&cap,node.raw()).unwrap();
        let generation=state.borrow().document_generation;
        let url="https://image.invalid/retired.svg";
        let key=(generation,url.to_owned(),ImageRequestProfile::NoCorsInclude);
        let (tx,mut rx)=tokio::sync::oneshot::channel();
        state.borrow_mut().render_image_in_flight.insert(key.clone(),vec![tx]);
        retire_document_referrer(&mut state.borrow_mut());
        assert!(state.borrow().dom.is_some());assert!(cap.storage.retired_dom.borrow().is_none());
        assert!(handle.active_document_state().is_none());assert!(handle.active_state().is_none());
        publish_async_image_response(&handle,&state,&key,
            Some(b"<svg xmlns='http://www.w3.org/2000/svg' width='24' height='12'/>".to_vec()));
        assert!(rx.try_recv().is_ok());assert!(state.borrow().render_image_in_flight.is_empty());
        assert!(state.borrow().render_resources.cached_image_content_bytes(url,key.2).is_none());
        assert_eq!(finish_async_image_metadata(&handle,&state,node,generation,url,key.2),"{\"state\":\"retired\"}");
        handle.update_attribute("set_attribute",None,"width","23").unwrap();
        assert_eq!(state.borrow().dom.as_ref().unwrap().with_node(node,|n|n.get_attribute("width").map(str::to_owned)).flatten().as_deref(),Some("23"));
        drop(state);
        assert!(weak.upgrade().is_none());assert!(cap.storage.retired_dom.borrow().is_some());
        assert_eq!(handle.attribute("width").as_deref(),Some("23"));
        handle.update_attribute("set_attribute",None,"width","31").unwrap();
        assert_eq!(image_idl_dimension(&handle,false).unwrap(),31);
    }

}


/// Ordinary DOM wrappers may be read between host DOM replacement and page
/// initialization. An old cap cannot mint a new document's popover authority.
#[op2(fast)]
pub(crate) fn op_popover_document_current(#[cppgc] owner: &ImageDocumentCapability) -> bool {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        owner.owner.active() && owner.storage.live_state().is_some()
    })).unwrap_or(false)
}

/// A popover operation names its original Document and arena generation.
/// The host-issued document cap is held only by trusted element factories.
pub(super) struct NativePopoverHandle {
    document: Rc<OriginalDocumentStorage>,
    activity_owner: Arc<Owner>,
    node: NodeId,
    generation: u64,
    open_interface: u32,
}
impl deno_core::cppgc::GarbageCollected for NativePopoverHandle {
    fn get_name(&self) -> &'static std::ffi::CStr { c"NativePopoverHandle" }
}
#[op2]
#[cppgc]
pub(crate) fn op_popover_bind_handle(#[cppgc] owner: &ImageDocumentCapability, nid: u32)
    -> Result<NativePopoverHandle, deno_error::JsErrorBox> {
    use deno_error::JsErrorBox;
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let state = owner.storage.live_state().ok_or_else(|| JsErrorBox::type_error("Document unavailable"))?;
        let state = state.try_borrow().map_err(|_| JsErrorBox::type_error("Document unavailable"))?;
        let dom = state.dom.as_ref().ok_or_else(|| JsErrorBox::type_error("Document unavailable"))?;
        let node = NodeId::new(nid);
        let generation = dom.node_generation(node).ok_or_else(|| JsErrorBox::type_error("Illegal invocation"))?;
        let element = dom.get_node(node).and_then(|node| node.as_element().cloned())
            .filter(|name| name.ns.as_ref() == "http://www.w3.org/1999/xhtml")
            .ok_or_else(|| JsErrorBox::type_error("Illegal invocation"))?;
        let open_interface = match element.local.as_ref() { "details" => 1, "dialog" => 2, _ => 0 };
        Ok(NativePopoverHandle { document: owner.storage.clone(), activity_owner: owner.owner.clone(), node, generation, open_interface })
    })).unwrap_or_else(|_| Err(JsErrorBox::generic("Popover binding failed")))
}
/// Immutable interface brand minted from the original native HTML node.
/// Actual reads/writes still validate the original arena generation and tag.
#[op2(fast)]
pub(crate) fn op_popover_open_interface(#[cppgc] handle: &NativePopoverHandle) -> u32 {
    handle.open_interface
}
// 0/1 are attribute values; 2 is an invalid/unavailable original node.
// The accessor semantic creates its own realm's TypeError from this status.
#[op2(fast)]
pub(crate) fn op_popover_open_value(#[cppgc] handle: &NativePopoverHandle, expected_interface: u32) -> u32 {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let required_tag = match expected_interface { 1 => "details", 2 => "dialog", _ => return 2 };
        if handle.open_interface != expected_interface { return 2; }
        let read = |dom: &DomTree| {
            if dom.node_generation(handle.node) != Some(handle.generation) { return 2; }
            dom.with_node(handle.node, |node| {
                let valid = node.as_element().is_some_and(|name|
                    name.ns.as_ref() == "http://www.w3.org/1999/xhtml" && name.local.as_ref() == required_tag);
                if valid { u32::from(node.get_attribute("open").is_some()) } else { 2 }
            }).unwrap_or(2)
        };
        if let Some(owner) = handle.document.live_state() {
            let Ok(state) = owner.try_borrow() else { return 2; };
            if let Some(dom) = state.dom.as_ref() { return read(dom); }
        }
        let Ok(retired) = handle.document.retired_dom.try_borrow() else { return 2; };
        retired.as_ref().map(read).unwrap_or(2)
    })).unwrap_or(2)
}
impl NativePopoverHandle {
    fn snapshot(&self, dom: &DomTree, active: bool) -> serde_json::Value {
        let node = (dom.node_generation(self.node) == Some(self.generation))
            .then(|| dom.get_node(self.node)).flatten();
        let kind = node.as_ref().and_then(|node| node.get_attribute("popover")).map(|value| {
            if value.is_empty() || value.eq_ignore_ascii_case("auto") { "auto" }
            else if value.eq_ignore_ascii_case("hint") { "hint" } else { "manual" }
        });
        serde_json::json!({"localName":node.as_ref().and_then(|node| node.as_element()).map(|name| name.local.as_ref()),
            "openAttributePresent":node.as_ref().is_some_and(|node| node.get_attribute("open").is_some()),
            "openAttribute":node.as_ref().and_then(|node| node.get_attribute("open")),"attribute":node.as_ref().and_then(|node| node.get_attribute("popover")),"type":kind,"connected":node.as_ref().is_some_and(|node| node.connected),
            "active":active,"open":node.is_some() && dom.popover_open(self.node),
            "onbeforetoggle":node.as_ref().and_then(|node| node.get_attribute("onbeforetoggle")),
            "ontoggle":node.as_ref().and_then(|node| node.get_attribute("ontoggle"))})
    }
}
#[op2]
#[string]
pub(crate) fn op_popover_state(#[cppgc] handle: &NativePopoverHandle) -> Result<String, deno_error::JsErrorBox> {
    use deno_error::JsErrorBox;
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if let Some(owner) = handle.document.live_state() {
            let state = owner.try_borrow().map_err(|_| JsErrorBox::type_error("Document unavailable"))?;
            if let Some(dom) = state.dom.as_ref() {
                let active = handle.activity_owner.active() && Arc::ptr_eq(&handle.activity_owner, &state.websocket_owner);
                return Ok(handle.snapshot(dom, active).to_string());
            }
        }
        let retired = handle.document.retired_dom.try_borrow().map_err(|_| JsErrorBox::type_error("Document unavailable"))?;
        Ok(retired.as_ref().map(|dom| handle.snapshot(dom, false))
            .unwrap_or_else(|| serde_json::json!({"type":null,"connected":false,"active":false,"open":false})).to_string())
    })).unwrap_or_else(|_| Err(JsErrorBox::generic("Popover state unavailable")))
}
#[op2(fast)]
pub(crate) fn op_popover_set(#[cppgc] handle: &NativePopoverHandle, open: bool) -> Result<bool, deno_error::JsErrorBox> {
    use deno_error::JsErrorBox;
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let Some(owner) = handle.document.live_state() else { return Ok(false); };
        let mut state = owner.try_borrow_mut().map_err(|_| JsErrorBox::type_error("Document unavailable"))?;
        if !handle.activity_owner.active() || !Arc::ptr_eq(&handle.activity_owner, &state.websocket_owner) { return Ok(false); }
        let changed = state.dom.as_ref().is_some_and(|dom|
            dom.node_generation(handle.node) == Some(handle.generation) && dom.set_popover_open(handle.node, open));
        if changed { invalidate_input_render(&mut state); }
        Ok(changed)
    })).unwrap_or_else(|_| Err(JsErrorBox::generic("Popover state unavailable")))
}

/// Attribute reflection uses the same original Document as popover operations.
/// Retired wrappers may mutate their retained DOM, but never a replacement.
#[op2]
pub(crate) fn op_popover_attribute(#[cppgc] handle: &NativePopoverHandle, #[string] value: Option<String>)
    -> Result<bool, deno_error::JsErrorBox> {
    update_html_attribute(handle, "popover", None, value)
}

#[op2]
pub(crate) fn op_dialog_attribute(#[cppgc] handle: &NativePopoverHandle, #[string] value: Option<String>)
    -> Result<bool, deno_error::JsErrorBox> {
    update_html_attribute(handle, "open", Some("dialog"), value)
}

#[op2]
pub(crate) fn op_details_attribute(#[cppgc] handle: &NativePopoverHandle, #[string] value: Option<String>)
    -> Result<bool, deno_error::JsErrorBox> {
    update_html_attribute(handle, "open", Some("details"), value)
}

fn update_html_attribute(handle: &NativePopoverHandle, name: &str, required_tag: Option<&str>, value: Option<String>)
    -> Result<bool, deno_error::JsErrorBox> {
    use deno_error::JsErrorBox;
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if let Some(owner) = handle.document.live_state() {
            let previous = {
                let state = owner.try_borrow().map_err(|_| JsErrorBox::type_error("Document unavailable"))?;
                let dom = state.dom.as_ref().ok_or_else(|| JsErrorBox::type_error("Document unavailable"))?;
                if dom.node_generation(handle.node) != Some(handle.generation) {
                    return Err(JsErrorBox::type_error("Illegal invocation"));
                }
                dom.with_node(handle.node, |node| {
                    if required_tag.is_some_and(|tag| node.as_element().is_none_or(|element| element.local.as_ref() != tag)) {
                        return Err(JsErrorBox::type_error("Illegal invocation"));
                    }
                    Ok(node.get_attribute(name).map(str::to_owned))
                }).ok_or_else(|| JsErrorBox::type_error("Illegal invocation"))??
            };
            if previous == value { return Ok(value.is_some()); }
            let (command, arg) = match value {
                Some(value) => ("set_attribute", format!("{name}\0{value}")),
                None => ("remove_attribute", name.to_owned()),
            };
            op_dom_inner(owner, command.to_owned(), handle.node.raw().to_string(), arg);
            return Ok(true);
        }
        let retired = handle.document.retired_dom.try_borrow()
            .map_err(|_| JsErrorBox::type_error("Document unavailable"))?;
        let dom = retired.as_ref().ok_or_else(|| JsErrorBox::type_error("Document unavailable"))?;
        if dom.node_generation(handle.node) != Some(handle.generation) {
            return Err(JsErrorBox::type_error("Illegal invocation"));
        }
        dom.with_node_mut(handle.node, |node| {
            if required_tag.is_some_and(|tag| node.as_element().is_none_or(|element| element.local.as_ref() != tag)) {
                return Err(JsErrorBox::type_error("Illegal invocation"));
            }
            if node.get_attribute(name) == value.as_deref() { return Ok(value.is_some()); }
            if let Some(value) = value { node.set_attribute(name, value); }
            else if let Some(attrs) = node.attrs_mut() { attrs.retain(|attr| !attr.qualified_name_eq(name)); }
            Ok(true)
        }).ok_or_else(|| JsErrorBox::type_error("Illegal invocation"))?
    })).unwrap_or_else(|_| Err(JsErrorBox::generic("Popover attribute unavailable")))
}
