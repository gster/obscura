use std::cell::RefCell;
use std::pin::Pin;
use std::rc::{Rc, Weak};
use std::sync::Arc;

use deno_core::error::ModuleLoaderError;
use deno_core::ModuleLoadResponse;
use deno_core::ModuleLoader;
use deno_core::ModuleSource;
use deno_core::ModuleSourceCode;
use deno_core::ModuleSpecifier;
use deno_core::RequestedModuleType;

use crate::import_map::ImportMap;
use crate::ops::ObscuraState;

/// Observable network activity for ES-module graphs.
///
/// deno_core keeps dynamic-import state inside its private module map. The
/// browser lifecycle still needs to distinguish a genuinely idle page from a
/// graph whose fetch future is being advanced in short event-loop slices. A
/// loader-owned counter provides that signal without treating unrelated
/// fetch/XHR analytics as render-blocking work.
#[derive(Debug, Default)]
pub(crate) struct ModuleLoadActivity {
    pending: std::sync::atomic::AtomicUsize,
    last_activity: std::sync::Mutex<Option<std::time::Instant>>,
}

impl ModuleLoadActivity {
    fn begin(self: &Arc<Self>) -> ModuleLoadGuard {
        self.pending
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        *self
            .last_activity
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(std::time::Instant::now());
        ModuleLoadGuard(self.clone())
    }

    pub(crate) fn is_pending_or_recent(&self, grace: std::time::Duration) -> bool {
        if self.pending.load(std::sync::atomic::Ordering::Relaxed) != 0 {
            return true;
        }
        self.last_activity
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_some_and(|last| last.elapsed() <= grace)
    }
}

struct ModuleLoadGuard(Arc<ModuleLoadActivity>);

impl Drop for ModuleLoadGuard {
    fn drop(&mut self) {
        let previous = self
            .0
            .pending
            .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
        debug_assert!(previous > 0, "module load activity counter underflow");
        *self
            .0
            .last_activity
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(std::time::Instant::now());
    }
}

pub struct ObscuraModuleLoader {
    pub base_url: String,
    /// Proxy URL threaded through to every dynamic ES-module fetch (#139).
    /// `None` keeps the pre-#139 direct-connection behaviour for callers
    /// that haven't been updated.
    pub proxy_url: Option<String>,
    /// The owning page's network context. Production runtimes always install
    /// this so every module in a graph uses the same cookie jar, configured
    /// identity, redirect/security policy, interception, and callbacks as the
    /// entry module. Directly-constructed standalone loaders remain supported.
    page_state: Option<Weak<RefCell<ObscuraState>>>,
    /// Directly-constructed loaders still use Obscura's network policy and
    /// connection pool, with an isolated cookie jar and default calibrated persona.
    standalone_client: Option<Arc<obscura_net::StealthHttpClient>>,
    import_map: Rc<RefCell<ImportMap>>,
    activity: Arc<ModuleLoadActivity>,
    /// Canonical and requested specifiers fetched into deno_core's module map.
    /// The runtime uses a cursor into this append-only list to associate a
    /// prepared root with the dependencies that its successful evaluation also
    /// evaluates.
    loaded_specifiers: Rc<RefCell<Vec<String>>>,
}

impl ObscuraModuleLoader {
    pub fn new(base_url: &str) -> Self {
        Self::with_proxy(base_url, None)
    }

    pub fn with_proxy(base_url: &str, proxy_url: Option<String>) -> Self {
        let import_map = Rc::new(RefCell::new(ImportMap::default()));
        Self::with_proxy_and_import_map(base_url, proxy_url, import_map)
    }

    fn with_proxy_and_import_map(
        base_url: &str,
        proxy_url: Option<String>,
        import_map: Rc<RefCell<ImportMap>>,
    ) -> Self {
        let cookie_jar = Arc::new(obscura_net::CookieJar::new());
        let policy = Arc::new(obscura_net::ObscuraHttpClient::with_options(
            cookie_jar.clone(), proxy_url.as_deref(),
        ));
        let standalone_client = Arc::new(obscura_net::StealthHttpClient::with_policy_profile_persona(
            cookie_jar, proxy_url.as_deref(), policy,
            obscura_net::StealthProfile::default(), "en-US,en;q=0.9", None,
        ));
        ObscuraModuleLoader {
            base_url: base_url.to_string(),
            proxy_url,
            page_state: None,
            standalone_client: Some(standalone_client),
            import_map,
            activity: Arc::new(ModuleLoadActivity::default()),
            loaded_specifiers: Rc::new(RefCell::new(Vec::new())),
        }
    }

    pub(crate) fn with_page_state(
        base_url: &str,
        proxy_url: Option<String>,
        page_state: &Rc<RefCell<ObscuraState>>,
        import_map: Rc<RefCell<ImportMap>>,
    ) -> Self {
        ObscuraModuleLoader {
            base_url: base_url.to_string(),
            proxy_url,
            page_state: Some(Rc::downgrade(page_state)),
            standalone_client: None,
            import_map,
            activity: Arc::new(ModuleLoadActivity::default()),
            loaded_specifiers: Rc::new(RefCell::new(Vec::new())),
        }
    }

    pub(crate) fn activity(&self) -> Arc<ModuleLoadActivity> {
        self.activity.clone()
    }

    pub(crate) fn loaded_specifiers(&self) -> Rc<RefCell<Vec<String>>> {
        self.loaded_specifiers.clone()
    }
}

fn io_err(msg: String) -> ModuleLoaderError {
    std::io::Error::new(std::io::ErrorKind::Other, msg).into()
}

impl ModuleLoader for ObscuraModuleLoader {
    fn resolve(
        &self,
        specifier: &str,
        referrer: &str,
        _kind: deno_core::ResolutionKind,
    ) -> Result<ModuleSpecifier, ModuleLoaderError> {
        // deno_core represents the root passed to load_side_es_module with a
        // synthetic "." referrer. A browser resolves <script type=module src>
        // as a resource URL before it starts a graph; the document import map
        // must not remap that root URL.
        if referrer == "." {
            return deno_core::resolve_import(specifier, &self.base_url)
                .map_err(|error| error.into());
        }

        let base = if referrer.is_empty()
            || referrer.starts_with('<')
            || referrer == "about:blank"
        {
            &self.base_url
        } else {
            referrer
        };

        let base = ModuleSpecifier::parse(base)
            .map_err(|e| io_err(format!("Invalid module referrer {}: {}", base, e)))?;
        self.import_map
            .try_borrow_mut()
            .map_err(|_| io_err("Import map is already borrowed".to_string()))?
            .resolve(specifier, &base)
            .map_err(io_err)
    }

    fn load(
        &self,
        module_specifier: &ModuleSpecifier,
        maybe_referrer: Option<&ModuleSpecifier>,
        is_dyn_import: bool,
        _requested_module_type: RequestedModuleType,
    ) -> ModuleLoadResponse {
        let url = module_specifier.to_string();
        // Module-graph CORS and same-origin credentials are relative to the
        // owning document, not to the importing module. The importer remains
        // the HTTP referrer for a dependency; keeping these URLs distinct
        // prevents a cross-origin parent module from gaining CDN cookies when
        // it imports a sibling on that CDN.
        let document_url = ModuleSpecifier::parse(&self.base_url)
            .unwrap_or_else(|_| module_specifier.clone());
        let referrer = maybe_referrer
            .cloned()
            .unwrap_or_else(|| document_url.clone());
        // Capture the loader's proxy here so the async closure below owns a
        // plain Option<String> rather than borrowing &self across an `await`.
        let proxy_url = self.proxy_url.clone();
        let activity = self.activity.clone();
        let loaded_specifiers = self.loaded_specifiers.clone();
        loaded_specifiers.borrow_mut().push(url.clone());
        // Register before returning the future. The lifecycle can inspect the
        // runtime between deno_core accepting the load and first polling it.
        // Keeping the guard inside the future makes cancellation/navigation
        // decrement the count through Drop as well as success and failure.
        let activity_guard = is_dyn_import.then(|| activity.begin());
        let page_network = match self.page_state.as_ref() {
            Some(weak) => (|| {
                let state = weak
                    .upgrade()
                    .ok_or_else(|| "Module loader page state was dropped".to_string())?;
                let mut state = state
                    .try_borrow_mut()
                    .map_err(|_| "Module loader page state is already borrowed".to_string())?;
                let client = state.ensure_persona_transport();
                Ok((client, state.callbacks.clone(), state.referrer_policy))
            })(),
            None => self
                .standalone_client
                .clone()
                .map(|client| (client, None, obscura_net::ReferrerPolicy::default()))
                .ok_or_else(|| "No network context wired to module loader".to_string()),
        };

        ModuleLoadResponse::Async(Pin::from(Box::new(async move {
            // deno_core propagates `is_dyn_import` to every dependency edge in
            // the recursive graph, so this excludes parser-discovered/static
            // graphs without losing descendant fetches of a lazy graph.
            let _activity_guard = activity_guard;
            tracing::debug!(
                "Loading ES module: {} (proxy: {})",
                url,
                proxy_url.as_deref().unwrap_or("direct")
            );

            match page_network {
                Ok((client, callbacks, referrer_policy)) => {
                    let requested = ModuleSpecifier::parse(&url)
                        .map_err(|e| io_err(format!("Invalid module URL {}: {}", url, e)))?;
                    let mut request =
                        obscura_net::ResourceRequest::module_script(&document_url, &referrer);
                    request.referrer_policy = referrer_policy;
                    let resp = client
                        .fetch_resource_with_callbacks(&requested, request, callbacks.as_deref())
                        .await
                        .map_err(|e| io_err(format!("Failed to fetch module {}: {}", url, e)))?;
                    if !(200..=299).contains(&resp.status) {
                        return Err(io_err(format!(
                            "Module {} returned HTTP {}",
                            url, resp.status
                        )));
                    }
                    let found = ModuleSpecifier::parse(resp.url.as_str()).map_err(|e| {
                        io_err(format!("Invalid final module URL {}: {}", resp.url, e))
                    })?;
                    if found.as_str() != requested.as_str() {
                        loaded_specifiers
                            .borrow_mut()
                            .push(found.to_string());
                    }
                    let code = obscura_net::decode_non_html(&resp.body, resp.content_type());
                    Ok(ModuleSource::new_with_redirect(
                        deno_core::ModuleType::JavaScript,
                        ModuleSourceCode::String(code.into()),
                        &requested,
                        &found,
                        None,
                    ))
                }
                Err(error) => Err(io_err(error)),
            }
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    async fn load(loader: &ObscuraModuleLoader, url: &str) -> Result<ModuleSource, ModuleLoaderError> {
        let url = ModuleSpecifier::parse(url).unwrap();
        match loader.load(&url, None, false, RequestedModuleType::None) {
            ModuleLoadResponse::Sync(result) => result,
            ModuleLoadResponse::Async(future) => future.await,
        }
    }

    fn proxy(response: &'static [u8]) -> (String, std::thread::JoinHandle<Vec<u8>>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let thread = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
            let mut request = Vec::new();
            while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                let mut buffer = [0; 4096];
                let length = socket.read(&mut buffer).unwrap();
                assert!(length > 0);
                request.extend_from_slice(&buffer[..length]);
            }
            socket.write_all(response).unwrap();
            request
        });
        (format!("http://{address}"), thread)
    }

    #[tokio::test]
    async fn standalone_module_uses_persona_proxy_and_isolated_cookies() {
        let (proxy_url, server) = proxy(b"HTTP/1.1 200 OK\r\nContent-Type: application/javascript\r\nContent-Length: 17\r\nConnection: close\r\n\r\nexport default 7;");
        let loader = ObscuraModuleLoader::with_proxy("http://example.com/", Some(proxy_url));
        let client = loader.standalone_client.as_ref().unwrap();
        client.cookie_jar.set_cookie("session=raw-secret; Path=/", &ModuleSpecifier::parse("http://example.com/").unwrap());
        client.set_extra_headers(std::collections::HashMap::from([
            ("x-module-test".into(), "complete-header".into()),
        ])).await;
        let result = load(&loader, "http://example.com/entry.js").await.unwrap();
        let ModuleSourceCode::String(code) = result.code else { panic!("expected decoded module source") };
        assert_eq!(code.as_str(), "export default 7;");
        let request = String::from_utf8(server.join().unwrap()).unwrap();
        let lower = request.to_ascii_lowercase();
        assert!(request.starts_with("GET http://example.com/entry.js HTTP/1.1\r\n"), "{request}");
        for header in [
            format!("user-agent: {}\r\n", obscura_net::StealthProfile::default().user_agent().to_ascii_lowercase()),
            "sec-ch-ua-platform: \"windows\"\r\n".into(),
            "accept-language: en-us,en;q=0.9\r\n".into(),
            "sec-fetch-dest: script\r\n".into(),
            "cookie: session=raw-secret\r\n".into(),
            "x-module-test: complete-header\r\n".into(),
        ] {
            assert!(lower.contains(&header), "missing {header:?} in {request}");
        }
        let other = ObscuraModuleLoader::new("http://example.com/");
        assert!(other.standalone_client.as_ref().unwrap().cookie_jar
            .get_cookie_header(&ModuleSpecifier::parse("http://example.com/").unwrap()).is_empty());
    }

    #[tokio::test]
    async fn standalone_module_preserves_file_and_rejects_data_scheme() {
        let path = std::env::temp_dir().join(format!("obscura-module-{}.js", std::process::id()));
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&path).unwrap();
        file.write_all(b"export default 'local';").unwrap();
        let url = ModuleSpecifier::from_file_path(&path).unwrap();
        let loader = ObscuraModuleLoader::new(url.as_str());
        let result = load(&loader, url.as_str()).await;
        std::fs::remove_file(path).unwrap();
        let ModuleSourceCode::String(code) = result.unwrap().code else { panic!("expected decoded module source") };
        assert_eq!(code.as_str(), "export default 'local';");
        let error = load(&loader, "data:text/javascript,export%20default%201").await.unwrap_err();
        assert!(error.to_string().contains("Forbidden URL scheme"), "{error}");
    }

    #[tokio::test]
    async fn standalone_module_rejects_private_initial_and_redirect_targets() {
        let loader = ObscuraModuleLoader::new("http://127.0.0.1/");
        let error = load(&loader, "http://127.0.0.1/entry.js").await.unwrap_err();
        assert!(error.to_string().contains("private/internal IP address"), "{error}");
        let (proxy_url, server) = proxy(b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1/private.js\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        let loader = ObscuraModuleLoader::with_proxy("http://example.com/", Some(proxy_url));
        let error = load(&loader, "http://example.com/entry.js").await.unwrap_err();
        assert!(error.to_string().contains("private/internal IP address"), "{error}");
        assert!(!server.join().unwrap().is_empty());
    }

    struct FulfillModule;

    impl obscura_net::interceptor::RequestInterceptor for FulfillModule {
        fn intercept<'a, 'b, 'c>(
            &'a self, request: &'b obscura_net::RequestInfo,
        ) -> Pin<Box<dyn std::future::Future<Output = obscura_net::interceptor::InterceptAction> + Send + 'c>>
        where 'a: 'c, 'b: 'c, Self: 'c {
            Box::pin(async move {
                assert_eq!(request.resource_type, obscura_net::ResourceType::Script);
                obscura_net::interceptor::InterceptAction::Fulfill(obscura_net::Response {
                    status: 200, url: request.url.clone(), headers: Default::default(),
                    body: b"export default 'intercepted';".to_vec(),
                    raw_headers: None,
                    request_raw_headers: None,
                    redirected_from: Vec::new(), request_referrer: None,
                })
            })
        }
    }

    #[tokio::test]
    async fn page_module_keeps_bound_primp_interceptor_without_plain_client() {
        let policy = Arc::new(obscura_net::ObscuraHttpClient::new());
        *policy.interceptor.write().await = Some(Arc::new(FulfillModule));
        let client = Arc::new(obscura_net::StealthHttpClient::with_policy_profile_persona(
            policy.cookie_jar.clone(), None, policy,
            obscura_net::StealthProfile::MacChrome153, "en-US,en;q=0.9", None,
        ));
        let state = Rc::new(RefCell::new(ObscuraState::new()));
        state.borrow_mut().stealth_client = Some(client);
        let loader = ObscuraModuleLoader::with_page_state(
            "https://example.com/", None, &state, state.borrow().import_map.clone(),
        );
        let result = load(&loader, "https://example.com/entry.js").await.unwrap();
        let ModuleSourceCode::String(code) = result.code else { panic!("expected decoded module source") };
        assert_eq!(code.as_str(), "export default 'intercepted';");
    }
}
