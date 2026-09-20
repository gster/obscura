use obscura_browser::{lifecycle::WaitUntil, BrowserContext, Page};
use obscura_net::{
    interceptor::{InterceptAction, RequestInterceptor},
    RequestInfo,
};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::PathBuf,
    sync::Arc,
};
use url::Url;

use crate::protocol::{self, Request};

#[path = "manual.rs"]
mod manual;
#[path = "automation.rs"]
mod automation;
#[path = "network.rs"]
pub mod network;

type Error = (&'static str, &'static str);
fn invalid(code: &'static str) -> Error {
    (code, "NOT_SENT")
}
fn params<T: serde::de::DeserializeOwned>(request: &Request) -> Result<T, Error> {
    serde_json::from_value(request.params.clone()).map_err(|_| invalid("INVALID_PARAMS"))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Init {
    protocol_version: String,
    runtime_sha256: String,
    initial_mode: String,
    persona: obscura_net::PersonaSpec,
    #[serde(default)]
    privacy_policy: PrivacyPolicy,
    allowed_origins: Vec<String>,
    #[serde(default)]
    proxy_url: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct PrivacyPolicy {
    #[serde(default)]
    tracker_blocking: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Navigate {
    url: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadText {
    selector: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Fill {
    selector: String,
    value: String,
}
#[derive(Default, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum ClickNavigation {
    #[default]
    None,
    Required,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Click {
    selector: String,
    #[serde(default)]
    navigation: ClickNavigation,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WaitText {
    selector: String,
    text: String,
    #[serde(default)]
    contains: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Mode {
    generation: u64,
    mode: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Recheck {
    generation: u64,
}

struct OriginGuard(Vec<String>);
#[async_trait::async_trait]
impl RequestInterceptor for OriginGuard {
    async fn intercept(&self, request: &RequestInfo) -> InterceptAction {
        if allowed(&request.url, &self.0) {
            InterceptAction::Continue
        } else {
            InterceptAction::Block
        }
    }
}

fn allowed(url: &Url, origins: &[String]) -> bool {
    if url.scheme() == "blob" {
        if let Ok(inner) = Url::parse(url.path()) {
            return matches!(inner.scheme(), "http" | "https")
                && inner.username().is_empty()
                && inner.password().is_none()
                && origins.contains(&inner.origin().ascii_serialization());
        }
        return false;
    }
    matches!(url.scheme(), "http" | "https")
        && url.username().is_empty()
        && url.password().is_none()
        && origins.contains(&url.origin().ascii_serialization())
}

struct OwnedPage {
    page: Page,
    generation: u64,
    url: String,
}

impl OwnedPage {
    fn navigation_url(&self) -> Option<String> {
        let js = self.page.js.as_ref()?;
        js.pending_navigation_url().or_else(|| {
            let url = js.document_url();
            (url != self.url).then_some(url)
        })
    }
}

pub struct BrowserRuntime {
    networks: HashMap<String, Arc<std::sync::Mutex<network::Network>>>,
    protocol_version: String,
    workspace: PathBuf,
    sha256: String,
    context: Option<Arc<BrowserContext>>,
    persona: Option<obscura_net::EffectivePersona>,
    origins: Vec<String>,
    pages: HashMap<String, OwnedPage>,
    page_counter: u64,
    capture_count: u32,
    capture_bytes: usize,
    mode: String,
    generation: u64,
    pub poisoned: bool,
    recheck_generation: Option<u64>,
    pub takeover: Option<crate::takeover::Channel>,
    takeover_attached: bool,
    manual: Option<crate::takeover::Session>,
    manual_generation: u64,
    manual_sessions: std::collections::HashSet<String>,
}

pub fn file_hash(path: &std::path::Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

impl BrowserRuntime {
    pub fn new(workspace: PathBuf) -> io::Result<Self> {
        let canonical = workspace.canonicalize()?;
        if workspace != canonical || !canonical.is_dir() {
            return Err(io::Error::other("INVALID_WORKSPACE"));
        }
        Ok(Self {
            networks: HashMap::new(),
            protocol_version: "1".into(),
            workspace,
            sha256: file_hash(&std::env::current_exe()?)?,
            context: None,
            persona: None,
            origins: vec![],
            pages: HashMap::new(),
            page_counter: 0,
            capture_count: 0,
            capture_bytes: 0,
            mode: "PAUSED".into(),
            generation: 0,
            recheck_generation: None,
            poisoned: false,
            takeover: None,
            takeover_attached: false,
            manual: None,
            manual_generation: 0,
            manual_sessions: std::collections::HashSet::new(),
        })
    }

    pub fn uses_automation(&self) -> bool { self.protocol_version == "2" }

    pub async fn handle(&mut self, request: &Request) -> Value {
        let mut response = match self.execute(request).await {
            Ok(result) => json!({"id": request.id, "ok": true, "result": result}),
            Err((code, dispatch)) => protocol::error(request.id, code, dispatch),
        };
        if self.protocol_version == "2" {
            if let Some(page)=request.page_id.as_ref().and_then(|id|self.pages.get(id)) {
                let target=if response["ok"]==true {&mut response["result"]} else {&mut response};
                target["page_generation"]=json!(page.generation);
                target["url"]=json!(page.url);
            }
        }
        response
    }

    async fn initialize(&mut self, request: &Request) -> Result<Value, Error> {
        if self.context.is_some() {
            return Err(invalid("ALREADY_INITIALIZED"));
        }
        let init: Init = params(request)?;
        if !matches!(init.protocol_version.as_str(), "1" | "2") || init.runtime_sha256 != self.sha256 {
            return Err(invalid("RUNTIME_VERSION_MISMATCH"));
        }
        if !matches!(init.initial_mode.as_str(), "RUNNING" | "PAUSED") {
            return Err(invalid("INVALID_MODE"));
        }
        let persona = init.persona.compile().map_err(|_| invalid("UNSUPPORTED_PERSONA"))?;
        if init.protocol_version == "1"
            && persona.profile() != obscura_net::StealthProfile::WindowsChrome145
        {
            return Err(invalid("UNSUPPORTED_PERSONA"));
        }
        obscura_net::activate_process_persona(&persona)
            .map_err(|_| invalid("UNSUPPORTED_PERSONA"))?;
        if init.allowed_origins.is_empty() || init.allowed_origins.len() > 16 {
            return Err(invalid("INVALID_ORIGINS"));
        }
        let mut loopbacks = 0;
        for origin in &init.allowed_origins {
            let url = Url::parse(origin).map_err(|_| invalid("INVALID_ORIGINS"))?;
            if !allowed(&url, &init.allowed_origins)
                || url.origin().ascii_serialization() != *origin
            {
                return Err(invalid("INVALID_ORIGINS"));
            }
            if matches!(url.host_str(), Some("127.0.0.1" | "[::1]")) {
                loopbacks += 1;
            }
        }
        // Private access is enabled only for an all-literal-loopback fixture list.
        if loopbacks > 0 && loopbacks != init.allowed_origins.len() {
            return Err(invalid("MIXED_PRIVATE_ORIGINS"));
        }
        if let Some(proxy) = &init.proxy_url {
            let url = Url::parse(proxy).map_err(|_| invalid("INVALID_PROXY"))?;
            if url.scheme() != "http" || url.host_str().is_none() || url.port_or_known_default().is_none()
                || !url.username().is_empty() || url.password().is_some() || url.path() != "/"
                || url.query().is_some() || url.fragment().is_some() {
                return Err(invalid("INVALID_PROXY"));
            }
        }
        let profile = persona.profile();
        let mut context = BrowserContext::with_options(
            "autopilot".into(),
            persona.clone(),
            obscura_browser::BrowserContextOptions {
                proxy_url: init.proxy_url,
                allow_private_network: loopbacks > 0,
                ..Default::default()
            },
        );
        let device_identity = context.device_identity();
        let client =
            Arc::get_mut(&mut context.http_client).ok_or(invalid("CONTEXT_ALREADY_SHARED"))?;
        client.block_trackers = init.privacy_policy.tracker_blocking;
        *client.interceptor.write().await =
            Some(std::sync::Arc::new(OriginGuard(init.allowed_origins.clone())));
        self.protocol_version = init.protocol_version;
        self.origins = init.allowed_origins;
        self.mode = init.initial_mode;
        self.persona = Some(persona.clone());
        self.context = Some(Arc::new(context));
        let mut result = json!({"ready": true, "protocol_version": self.protocol_version, "runtime_version": format!("br_{}", &self.sha256[..24]),
            "runtime_sha256": self.sha256, "persona": persona, "device_identity": device_identity,
            "font_bundle_sha256": env!("AUTOPILOT_FONT_BUNDLE_SHA256"), "mode": self.mode, "generation": self.generation,
            "supported_methods": ["init", "new_page", "navigate", "read_text", "read_value", "read_checked", "fill", "click", "wait", "capture", "set_mode", "begin_recheck", "finish_recheck", "attach_takeover", "close"]});
        if self.uses_automation() {
            result["browser_identity"] = json!({
                "profile": persona.profile().name(),
                "user_agent": persona.user_agent(),
                "transport_profile": match profile {
                    obscura_net::StealthProfile::MacChrome153 => "primp_chrome153_macos",
                    obscura_net::StealthProfile::MacChrome152 => "primp_chrome152_macos",
                    obscura_net::StealthProfile::WindowsChrome145 => "primp_chrome145_windows",
                },
                "chrome152_transport_verified": false,
            });
            result["supported_methods"].as_array_mut().unwrap().extend([json!("automation"), json!("network_body")]);
        }
        Ok(result)
    }

    fn page(&mut self, request: &Request) -> Result<&mut OwnedPage, Error> {
        let entry = self
            .pages
            .get_mut(request.page_id.as_deref().ok_or(invalid("MISSING_PAGE"))?)
            .ok_or(invalid("UNKNOWN_PAGE"))?;
        if Some(entry.generation) != request.page_generation {
            return Err(invalid("STALE_PAGE"));
        }
        if self.protocol_version == "1" && (entry.page.url_string() != entry.url
            || entry
                .page
                .js
                .as_ref()
                .is_some_and(|js| js.has_pending_navigation()))
        {
            return Err(invalid("UNEXPECTED_NAVIGATION"));
        }
        Ok(entry)
    }

    async fn execute(&mut self, request: &Request) -> Result<Value, Error> {
        if request.method == "close" {
            let _: Empty = params(request)?;
            self.revoke_takeover("BROWSER_CLOSED");
            self.takeover = None;
            self.pages.clear();
            self.context = None;
            return Ok(json!({"closed": true}));
        }
        if self.poisoned {
            return Err(invalid("SESSION_POISONED"));
        }
        if request.method == "init" {
            return self.initialize(request).await;
        }
        if self.context.is_none() {
            return Err(invalid("NOT_INITIALIZED"));
        }
        if request.method == "attach_takeover" {
            let _: Empty = params(request)?;
            if self.takeover_attached {
                return Err(invalid("TAKEOVER_ALREADY_ATTACHED"));
            }
            self.takeover_attached = true;
            self.takeover = Some(
                crate::takeover::Channel::connect(&self.workspace)
                    .await
                    .map_err(|_| invalid("TAKEOVER_ATTACH_FAILED"))?,
            );
            return Ok(json!({"attached":true,"capabilities":["control"]}));
        }
        if request.method == "begin_recheck" {
            let check: Recheck = params(request)?;
            if check.generation < self.generation {
                return Err(invalid("STALE_CONTROL"));
            }
            if check.generation == self.generation {
                if self.recheck_generation != Some(check.generation) || self.mode != "PAUSED" {
                    return Err(invalid("CONTROL_CONFLICT"));
                }
            } else {
                self.revoke_takeover("MANUAL_RECHECK");
                self.mode = "PAUSED".into();
                self.generation = check.generation;
                self.recheck_generation = Some(check.generation);
            }
            return Ok(json!({"mode": self.mode, "generation": self.generation}));
        }
        if request.method == "finish_recheck" {
            let check: Recheck = params(request)?;
            if self.recheck_generation != Some(check.generation)
                || self.generation != check.generation
            {
                return Err(invalid("RECHECK_REQUIRED"));
            }
            self.mode = "RUNNING".into();
            return Ok(json!({"mode": self.mode, "generation": self.generation}));
        }
        if request.method == "set_mode" {
            let desired: Mode = params(request)?;
            if !matches!(desired.mode.as_str(), "PAUSED" | "RUNNING") {
                return Err(invalid("INVALID_MODE"));
            }
            if desired.generation < self.generation {
                return Err(invalid("STALE_CONTROL"));
            }
            if desired.generation == self.generation && desired.mode != self.mode {
                return Err(invalid("CONTROL_CONFLICT"));
            }
            if desired.generation > self.generation {
                self.recheck_generation = None;
            }
            if self.manual.is_some() && desired.generation > self.generation {
                self.revoke_takeover("CONTROL_CHANGED");
                if desired.mode == "RUNNING" {
                    return Err(invalid("TAKEOVER_RECHECK_REQUIRED"));
                }
            }
            self.generation = desired.generation;
            self.mode = desired.mode;
            return Ok(json!({"mode": self.mode, "generation": self.generation}));
        }
        if self.mode == "PAUSED"
            && !(request.method == "automation" && request.params.get("operation").and_then(Value::as_str) == Some("close"))
            && matches!(
                request.method.as_str(),
                "new_page" | "navigate" | "fill" | "click" | "wait" | "automation"
            )
        {
            return Err(invalid("BROWSER_PAUSED"));
        }
        if self.protocol_version == "1" && (request.timeout_ms > 30000 || matches!(request.method.as_str(), "automation" | "network_body")) {
            return Err(invalid("PROTOCOL_VERSION_REQUIRED"));
        }
        match request.method.as_str() {
            "automation" => self.automation(request).await,
            "network_body" => self.network_body(request),
            "new_page" => {
                let _: Empty = params(request)?;
                if self.pages.len() >= 4 {
                    return Err(invalid("PAGE_LIMIT"));
                }
                self.page_counter += 1;
                let id = format!("p{}", self.page_counter);
                let mut page = Page::new(id.clone(), self.context.as_ref().unwrap().clone());
                let viewport = self.persona.as_ref().unwrap().viewport();
                page.set_viewport((viewport.width as f32, viewport.height as f32));
                page.add_preload_script(&self.persona.as_ref().unwrap().preload_script());
                self.pages.insert(
                    id.clone(),
                    OwnedPage {
                        page,
                        generation: 0,
                        url: "about:blank".into(),
                    },
                );
                if self.protocol_version == "2" { self.observe(&id); }
                Ok(json!({"page_id": id, "page_generation": 0}))
            }
            "navigate" => {
                let navigation: Navigate = params(request)?;
                let url = Url::parse(&navigation.url).map_err(|_| invalid("INVALID_URL"))?;
                if !allowed(&url, &self.origins) {
                    return Err(invalid("ORIGIN_NOT_ALLOWED"));
                }
                let wait = if self.protocol_version == "2" { WaitUntil::DomContentLoaded } else { WaitUntil::Load };
                let entry = self.page(request)?;
                entry.generation += 1;
                entry
                    .page
                    .navigate_with_wait(url.as_str(), wait)
                    .await
                    .map_err(|_| ("NAVIGATION_FAILED", "SENT"))?;
                entry.page.prepare_screenshot_resources(500).await;
                entry.url = entry.page.url_string();
                Ok(
                    json!({"page_id": request.page_id, "page_generation": entry.generation, "url": entry.url}),
                )
            }
            "read_text" => {
                let read: ReadText = params(request)?;
                if read.selector.is_empty() || read.selector.len() > 1024 {
                    return Err(invalid("INVALID_SELECTOR"));
                }
                let page = self.page(request)?;
                let text = page
                    .page
                    .with_dom(|dom| {
                        let node = dom
                            .query_selector(&read.selector)
                            .map_err(|_| invalid("INVALID_SELECTOR"))?
                            .ok_or(invalid("ELEMENT_NOT_FOUND"))?;
                        Ok::<_, Error>(dom.text_content(node))
                    })
                    .ok_or(invalid("NO_DOCUMENT"))??;
                if text.len() > 16384 {
                    return Err(invalid("TEXT_LIMIT"));
                }
                Ok(json!({"text": text}))
            }
            "read_value" => {
                let read: ReadText = params(request)?;
                let entry = self.page(request)?;
                let js = entry.page.js.as_ref().ok_or(invalid("NO_DOCUMENT"))?;
                let node = js.input_node(&read.selector).map_err(invalid)?;
                let value = js
                    .with_dom(|dom| dom.text_control(node))
                    .flatten()
                    .ok_or(invalid("INPUT_ELEMENT_UNSUPPORTED"))?
                    .value;
                if value.len() > 16384 {
                    return Err(invalid("INPUT_VALUE_LIMIT"));
                }
                Ok(json!({"value": value}))
            }
            "read_checked" => {
                let read: ReadText = params(request)?;
                let entry = self.page(request)?;
                let js = entry.page.js.as_ref().ok_or(invalid("NO_DOCUMENT"))?;
                let node = js.input_node(&read.selector).map_err(invalid)?;
                let checked = js
                    .with_dom(|dom| {
                        matches!(dom.input_type(node).as_deref(), Some("checkbox" | "radio"))
                            .then(|| dom.checked_state(node).map(|state| state.checked))
                            .flatten()
                    })
                    .flatten()
                    .ok_or(invalid("INPUT_ELEMENT_UNSUPPORTED"))?;
                Ok(json!({"checked": checked}))
            }
            "click" => {
                let click: Click = params(request)?;
                let origins = self.origins.clone();
                let entry = self.page(request)?;
                let result = entry
                    .page
                    .js
                    .as_mut()
                    .ok_or(invalid("NO_DOCUMENT"))?
                    .native_click(&click.selector)?;
                loop {
                    if let Some(navigation_url) = entry.navigation_url() {
                        if click.navigation == ClickNavigation::None {
                            return Err(("UNEXPECTED_NAVIGATION", "SENT"));
                        }
                        let url =
                            Url::parse(&navigation_url).map_err(|_| ("INVALID_URL", "SENT"))?;
                        if !allowed(&url, &origins) {
                            return Err(("ORIGIN_NOT_ALLOWED", "SENT"));
                        }
                        entry.generation += 1;
                        entry
                            .page
                            .process_pending_navigation()
                            .await
                            .map_err(|_| ("NAVIGATION_FAILED", "SENT"))?;
                        entry.url = entry.page.url_string();
                        let final_url =
                            Url::parse(&entry.url).map_err(|_| ("INVALID_URL", "SENT"))?;
                        if !allowed(&final_url, &origins) {
                            return Err(("ORIGIN_NOT_ALLOWED", "SENT"));
                        }
                        // Same-document commits can queue hashchange and a
                        // callback can navigate again. Observe those tasks
                        // under this RPC deadline without replaying the click.
                        entry.page.settle(20).await;
                        if entry.navigation_url().is_some() {
                            continue;
                        }
                        break;
                    }
                    entry.page.settle(20).await;
                    if click.navigation == ClickNavigation::None {
                        if entry.navigation_url().is_some() {
                            return Err(("UNEXPECTED_NAVIGATION", "SENT"));
                        }
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
                Ok(
                    json!({"page_id": request.page_id, "page_generation": entry.generation,
                    "default_prevented": result.default_prevented}),
                )
            }
            "fill" => {
                let fill: Fill = params(request)?;
                let entry = self.page(request)?;
                let result = entry
                    .page
                    .js
                    .as_mut()
                    .ok_or(invalid("NO_DOCUMENT"))?
                    .native_fill(&fill.selector, &fill.value)?;
                entry.page.settle(20).await;
                let js = entry.page.js.as_ref().ok_or(("NO_DOCUMENT", "SENT"))?;
                if entry.navigation_url().is_some() {
                    return Err(("UNEXPECTED_NAVIGATION", "SENT"));
                }
                if js
                    .input_node(&fill.selector)
                    .map_err(|code| (code, "SENT"))?
                    != result.node
                {
                    return Err(("INPUT_TARGET_CHANGED", "SENT"));
                }
                let current = js
                    .with_dom(|dom| dom.text_control(result.node))
                    .flatten()
                    .ok_or(("INPUT_TARGET_CHANGED", "SENT"))?;
                if current.value != result.value {
                    return Err(("INPUT_VALUE_CHANGED", "SENT"));
                }
                Ok(
                    json!({"changed": result.changed, "length": result.value.encode_utf16().count()}),
                )
            }
            "wait" => {
                let wait: WaitText = params(request)?;
                if wait.selector.is_empty() || wait.selector.len() > 1024 || wait.text.len() > 16384
                {
                    return Err(invalid("INVALID_PARAMS"));
                }
                let entry = self.page(request)?;
                loop {
                    let text = entry
                        .page
                        .with_dom(|dom| {
                            dom.query_selector(&wait.selector)
                                .map(|node| node.map(|node| dom.text_content(node)))
                                .map_err(|_| invalid("INVALID_SELECTOR"))
                        })
                        .ok_or(invalid("NO_DOCUMENT"))??;
                    if text.as_ref().is_some_and(|text| if wait.contains {
                        text.contains(&wait.text)
                    } else {
                        text == &wait.text
                    }) {
                        return Ok(json!({"text": text}));
                    }
                    entry.page.settle(20).await;
                    if entry.navigation_url().is_some() {
                        return Err(("UNEXPECTED_NAVIGATION", "SENT"));
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            }
            "capture" => {
                let _: Empty = params(request)?;
                if self.capture_count >= 16 {
                    return Err(invalid("CAPTURE_QUOTA"));
                }
                let entry = self.page(request)?;
                let png = entry
                    .page
                    .screenshot(entry.page.viewport)
                    .ok_or(invalid("CAPTURE_UNAVAILABLE"))?;
                if png.len() > 2 * 1024 * 1024 {
                    return Err(invalid("CAPTURE_LIMIT"));
                }
                if self.capture_bytes + png.len() > 16 * 1024 * 1024 {
                    return Err(invalid("CAPTURE_QUOTA"));
                }
                let directory = self.workspace.join("captures");
                if !directory.exists() {
                    fs::DirBuilder::new()
                        .mode(0o700)
                        .create(&directory)
                        .map_err(|_| invalid("CAPTURE_IO_FAILED"))?;
                }
                if directory.canonicalize().ok().as_ref() != Some(&directory) {
                    return Err(invalid("CAPTURE_PATH_INVALID"));
                }
                let relative = format!("captures/capture-{}.png", request.id);
                self.capture_count += 1;
                self.capture_bytes += png.len();
                let mut file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(self.workspace.join(&relative))
                    .map_err(|_| invalid("CAPTURE_IO_FAILED"))?;
                file.write_all(&png)
                    .and_then(|_| file.sync_all())
                    .map_err(|_| invalid("CAPTURE_IO_FAILED"))?;
                Ok(
                    json!({"path": relative, "sha256": format!("{:x}", Sha256::digest(&png)), "bytes": png.len(), "mime_type": "image/png"}),
                )
            }
            _ => Err(invalid("METHOD_UNAVAILABLE")),
        }
    }
}
