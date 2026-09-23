//! Request-scoped diagnostic capture. This is application/transport metadata,
//! never a wire capture. Incomplete bodies are not retained as complete bodies.
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use crate::{HeaderCapture, ObscuraNetError, ResourceType, Response};

/// Page-owned lifecycle sink for ordinary requests.  The transport calls
/// `started` after the exact prepared headers/body are known, but before the
/// first I/O await.  A rejected start therefore prevents the request from
/// reaching the network.  Terminal reporting is best-effort because the
/// transport side effect has already happened; sinks retain any failure as an
/// explicit accepted-prefix diagnostic instead of rewriting it as an HTTP
/// failure.
pub trait RequestLifecycleObserver: Send + Sync {
    fn started(
        &self,
        request_id: &str,
        resource_type: ResourceType,
        hop_index: usize,
        exchange: &Exchange,
    ) -> Result<(), ObscuraNetError>;

    fn terminal(
        &self,
        request_id: &str,
        resource_type: ResourceType,
        hop_index: usize,
        exchange: &Exchange,
        response_body: Option<&[u8]>,
        error: Option<&str>,
    );
}

#[derive(Debug, Clone)]
pub struct Exchange {
    pub url: String,
    pub method: String,
    pub request_headers: Option<HeaderCapture>,
    pub request_body_present: bool,
    pub request_body_size: usize,
    pub request_body_request_id: Option<String>,
    pub transport_request_body_present: bool,
    pub transport_request_body_size: usize,
    pub transport_request_body_request_id: Option<String>,
    pub response: Option<Response>,
    pub body_complete: bool,
    pub body_size: usize,
    pub body_request_id: Option<String>,
    pub body_capture_error: Option<String>,
    starting: bool,
    started: bool,
    terminal: bool,
}

#[derive(Clone)]
pub struct RequestTrace {
    exchanges: Arc<Mutex<Vec<Exchange>>>,
    bodies: Arc<Mutex<crate::response_body::ResponseBodyStore>>,
    request_bodies: Arc<Mutex<crate::request_body::RequestBodyStore>>,
    request_id: String,
    capture_response_bodies: bool,
    capture_redirect_response_bodies: bool,
    observer: Option<Arc<dyn RequestLifecycleObserver>>,
    observed_resource_type: Option<ResourceType>,
    network_activity_generation: Option<u64>,
    /// Serializes the terminal state transition with its observer callback.
    /// Teardown can call `fail` and know that, when it returns, no earlier
    /// terminal callback for this trace remains in flight.
    terminal_serial: Arc<Mutex<()>>,
    cancel_requested: Arc<AtomicBool>,
    cancel_reason: Arc<Mutex<Option<String>>>,
}

impl RequestTrace {
    fn response_is_binary(&self, response: &Response) -> bool {
        match self.observed_resource_type {
            Some(ResourceType::Image | ResourceType::Font) => true,
            Some(ResourceType::Document) => response.headers.iter()
                .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
                .map(|(_, value)| value.to_ascii_lowercase())
                .is_some_and(|content_type| {
                    !(content_type.starts_with("text/")
                        || content_type.contains("json")
                        || content_type.contains("javascript")
                        || content_type.contains("xml")
                        || content_type.contains("x-www-form-urlencoded"))
                }),
            _ => false,
        }
    }

    pub fn new(bodies: Arc<Mutex<crate::response_body::ResponseBodyStore>>,
        request_bodies: Arc<Mutex<crate::request_body::RequestBodyStore>>, request_id: String,
    ) -> Self {
        Self {
            exchanges: Arc::new(Mutex::new(Vec::new())), bodies, request_bodies,
            request_id, capture_response_bodies: true,
            capture_redirect_response_bodies: true,
            observer: None,
            observed_resource_type: None,
            network_activity_generation: None,
            terminal_serial: Arc::new(Mutex::new(())),
            cancel_requested: Arc::new(AtomicBool::new(false)),
            cancel_reason: Arc::new(Mutex::new(None)),
        }
    }

    pub fn observe(
        mut self,
        observer: Arc<dyn RequestLifecycleObserver>,
        resource_type: ResourceType,
    ) -> Self {
        self.observer = Some(observer);
        self.observed_resource_type = Some(resource_type);
        self
    }

    pub fn with_network_activity_generation(mut self, generation: Option<u64>) -> Self {
        self.network_activity_generation = generation;
        self
    }

    pub fn network_activity_generation(&self) -> Option<u64> {
        self.network_activity_generation
    }

    /// Native navigation owns final-response storage because it must classify
    /// binary main resources. The trace still retains every redirect response
    /// body, hop metadata, and exact request body without duplicating the final
    /// response-body budget.
    pub fn new_request_only(bodies: Arc<Mutex<crate::response_body::ResponseBodyStore>>,
        request_bodies: Arc<Mutex<crate::request_body::RequestBodyStore>>, request_id: String,
    ) -> Self {
        let mut trace = Self::new(bodies, request_bodies, request_id);
        trace.capture_response_bodies = false;
        trace
    }

    pub fn begin(&self, url: &str, method: &str, headers: Option<HeaderCapture>, body: Option<&[u8]>)
        -> Result<(), crate::request_body::RequestBodyError>
    {
        let index = self.exchanges.lock().unwrap_or_else(|e| e.into_inner()).len();
        let body_id = body.map(|_| format!("{}-request-hop-{}-standard", self.request_id, index));
        let previous_transport_id = self.exchanges.lock().unwrap_or_else(|e| e.into_inner()).last()
            .and_then(|exchange| exchange.transport_request_body_request_id.clone()
                .or_else(|| exchange.request_body_request_id.clone()));
        {
            let mut store = self.request_bodies.lock().unwrap_or_else(|e| e.into_inner());
            if let (Some(bytes), Some(body_id)) = (body, body_id.as_ref()) {
                if let Some(previous_transport_id) = previous_transport_id.as_deref() {
                    store.insert_or_shared(previous_transport_id, body_id.clone(), bytes)?;
                } else {
                    store.insert(body_id.clone(), bytes)?;
                }
                store.alias(body_id, &self.request_id)?;
            } else {
                store.clear_alias(&self.request_id);
            }
        }
        self.exchanges.lock().unwrap_or_else(|e| e.into_inner()).push(Exchange {
            url: url.into(), method: method.into(), request_headers: headers,
            request_body_present: body.is_some(), request_body_size: body.map_or(0, <[u8]>::len),
            request_body_request_id: body_id,
            transport_request_body_present: false, transport_request_body_size: 0,
            transport_request_body_request_id: None,
            response: None, body_complete: false, body_size: 0, body_request_id: None,
            body_capture_error: None,
            starting: false, started: false, terminal: false,
        });
        Ok(())
    }

    /// Admit the current logical/cache/local hop before resolving it.  A real
    /// transport normally calls this from `prepared`; cache and local paths
    /// call it directly and intentionally have no transport header capture.
    pub fn start(&self) -> Result<(), ObscuraNetError> {
        // Shutdown uses the same lock for fail(). The observer callback, the
        // exchange.started commit, and the final cancellation check therefore
        // form one start-admission transaction.
        let _lifecycle = self.terminal_serial
            .lock().unwrap_or_else(|failure| failure.into_inner());
        let Some(observer) = &self.observer else { return Ok(()); };
        let Some(resource_type) = self.observed_resource_type else { return Ok(()); };
        if self.cancel_requested.load(Ordering::Acquire) {
            let reason = self.cancel_reason.lock().unwrap_or_else(|error| error.into_inner())
                .clone().unwrap_or_else(|| "request lifecycle is closing".to_string());
            return Err(ObscuraNetError::Blocked(reason));
        }
        let (index, exchange) = {
            let mut exchanges = self.exchanges.lock().unwrap_or_else(|e| e.into_inner());
            let Some(index) = exchanges.len().checked_sub(1) else { return Ok(()); };
            if exchanges[index].started { return Ok(()); }
            if exchanges[index].starting {
                return Err(ObscuraNetError::Blocked(
                    "network start admission already in progress".to_string(),
                ));
            }
            exchanges[index].starting = true;
            (index, exchanges[index].clone())
        };
        let result = observer.started(&self.request_id, resource_type, index, &exchange);
        if let Some(exchange) = self.exchanges.lock().unwrap_or_else(|e| e.into_inner()).get_mut(index) {
            exchange.starting = false;
            exchange.started = result.is_ok();
        }
        result?;
        if self.cancel_requested.load(Ordering::Acquire) {
            let reason = self.cancel_reason.lock().unwrap_or_else(|error| error.into_inner())
                .clone().unwrap_or_else(|| "request lifecycle is closing".to_string());
            return Err(ObscuraNetError::Blocked(reason));
        }
        Ok(())
    }

    /// Fence a start transaction without waiting for its observer callback.
    /// The render epoch calls this while holding its own shutdown mutex, then
    /// invokes `fail` after releasing that mutex to avoid lock inversion.
    pub fn request_cancel(&self, reason: &str) {
        *self.cancel_reason.lock().unwrap_or_else(|error| error.into_inner()) =
            Some(reason.to_string());
        self.cancel_requested.store(true, Ordering::Release);
    }

    pub fn last(&self) -> Option<Exchange> {
        self.exchanges.lock().unwrap_or_else(|e| e.into_inner()).last().cloned()
    }

    pub fn previous(&self) -> Option<Exchange> {
        let exchanges = self.exchanges.lock().unwrap_or_else(|e| e.into_inner());
        exchanges.len().checked_sub(2).and_then(|index| exchanges.get(index)).cloned()
    }

    pub fn len(&self) -> usize {
        self.exchanges.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    pub fn update_request(&self, url: &str, method: &str, headers: HeaderCapture, body: Option<&[u8]>)
        -> Result<(), crate::request_body::RequestBodyError>
    {
        let index = self.exchanges.lock().unwrap_or_else(|e| e.into_inner()).len().saturating_sub(1);
        let transport_id = body.map(|_| format!("{}-request-hop-{}-transport", self.request_id, index));
        let standard_id = self.exchanges.lock().unwrap_or_else(|e| e.into_inner()).last()
            .and_then(|exchange| exchange.request_body_request_id.clone());
        if let (Some(bytes), Some(transport_id)) = (body, transport_id.as_ref()) {
            let mut store = self.request_bodies.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(standard_id) = standard_id.as_deref() {
                store.insert_or_shared(standard_id, transport_id.clone(), bytes)?;
            } else {
                store.insert(transport_id.clone(), bytes)?;
            }
        }
        if let Some(exchange) = self.exchanges.lock().unwrap_or_else(|e| e.into_inner()).last_mut() {
            exchange.url = url.into(); exchange.method = method.into();
            exchange.request_headers = Some(headers);
            exchange.transport_request_body_present = body.is_some();
            exchange.transport_request_body_size = body.map_or(0, <[u8]>::len);
            exchange.transport_request_body_request_id = transport_id;
        }
        Ok(())
    }

    pub fn prepared(&self, headers: HeaderCapture, body: &[u8])
        -> Result<(), ObscuraNetError>
    {
        let (needs_capture, body_present) = self.exchanges.lock().unwrap_or_else(|e| e.into_inner()).last()
            .map(|exchange| (
                exchange.transport_request_body_request_id.is_none()
                    && !exchange.transport_request_body_present,
                exchange.request_body_present,
            )).unwrap_or((false, false));
        if needs_capture {
            let (url, method) = self.exchanges.lock().unwrap_or_else(|e| e.into_inner()).last()
                .map(|exchange| (exchange.url.clone(), exchange.method.clone())).unwrap_or_default();
            self.update_request(&url, &method, headers.clone(), body_present.then_some(body))
                .map_err(|error| ObscuraNetError::Network(error.to_string()))?;
        }
        if let Some(exchange) = self.exchanges.lock().unwrap_or_else(|e| e.into_inner()).last_mut() {
            exchange.request_headers = Some(headers);
        }
        self.start()
    }

    pub fn response(&self, response: &Response, body_complete: bool) {
        self.response_with_error(response, body_complete, None);
    }

    pub fn response_with_error(
        &self,
        response: &Response,
        body_complete: bool,
        terminal_error: Option<&str>,
    ) {
        let _terminal_serial = body_complete.then(|| {
            self.terminal_serial.lock().unwrap_or_else(|error| error.into_inner())
        });
        let terminal = {
            let mut exchanges = self.exchanges.lock().unwrap_or_else(|e| e.into_inner());
            let index = exchanges.len().saturating_sub(1);
            if let Some(exchange) = exchanges.last_mut() {
                // Spool now rather than retaining every redirect body in memory.
                // Header-only captures never register an empty successful body.
                let is_redirect = (300..400).contains(&response.status)
                    && response.raw_headers.as_ref().is_some_and(|headers| {
                        headers.fields.iter().any(|field| field.name.eq_ignore_ascii_case(b"location"))
                    });
                let body_stored = if body_complete
                    && (self.capture_response_bodies
                        || (self.capture_redirect_response_bodies && is_redirect))
                {
                    let body_id = format!("{}-hop-{}", self.request_id, index);
                    match self.bodies.lock().unwrap_or_else(|e| e.into_inner())
                        .insert(body_id.clone(), &response.body, self.response_is_binary(response))
                    {
                        Ok(()) => {
                            exchange.body_request_id = Some(body_id);
                            true
                        }
                        Err(error) => {
                            // Keep the attempted canonical id.  The Page store is
                            // sticky and returns its diagnostic for this missing
                            // entry, while history can still retain the raw bytes
                            // supplied to the terminal observer below.
                            exchange.body_request_id = Some(body_id);
                            exchange.body_capture_error = Some(error.to_string());
                            false
                        }
                    }
                } else { false };
                exchange.response = Some(Response {
                    url: response.url.clone(), status: response.status, headers: response.headers.clone(), body: Vec::new(),
                    raw_headers: response.raw_headers.clone(), request_raw_headers: response.request_raw_headers.clone(),
                    redirected_from: response.redirected_from.clone(), request_referrer: response.request_referrer.clone(),
                });
                exchange.body_size = response.body.len();
                exchange.body_complete = body_stored;
                if body_complete && !exchange.terminal {
                    exchange.terminal = true;
                    Some((index, exchange.clone()))
                } else { None }
            } else { None }
        };
        if let (Some(observer), Some(resource_type), Some((index, exchange))) =
            (&self.observer, self.observed_resource_type, terminal)
        {
            observer.terminal(
                &self.request_id,
                resource_type,
                index,
                &exchange,
                Some(&response.body),
                terminal_error,
            );
        }
    }

    pub fn fail(&self, error: &str) {
        let _terminal_serial = self.terminal_serial
            .lock().unwrap_or_else(|failure| failure.into_inner());
        let cancellation = self.cancel_reason
            .lock().unwrap_or_else(|failure| failure.into_inner()).clone();
        let error = cancellation.as_deref().unwrap_or(error);
        let terminal = {
            let mut exchanges = self.exchanges.lock().unwrap_or_else(|e| e.into_inner());
            let Some(index) = exchanges.len().checked_sub(1) else { return; };
            if !exchanges[index].started || exchanges[index].terminal { return; }
            exchanges[index].terminal = true;
            Some((index, exchanges[index].clone()))
        };
        if let (Some(observer), Some(resource_type), Some((index, exchange))) =
            (&self.observer, self.observed_resource_type, terminal)
        {
            observer.terminal(
                &self.request_id,
                resource_type,
                index,
                &exchange,
                None,
                Some(error),
            );
        }
    }

    pub fn response_if_missing(&self, response: &Response) {
        let captured = self.exchanges.lock().unwrap_or_else(|e| e.into_inner())
            .last().is_some_and(|exchange| exchange.body_complete);
        if !captured { self.response(response, response.status != 0); }
    }

    pub fn completed_since(&self, index: usize) -> Vec<Exchange> {
        let exchanges = self.exchanges.lock().unwrap_or_else(|e| e.into_inner());
        exchanges.iter().take(exchanges.len().saturating_sub(1)).skip(index).cloned().collect()
    }

    pub fn take(&self) -> Vec<Exchange> {
        std::mem::take(&mut *self.exchanges.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

impl Drop for RequestTrace {
    fn drop(&mut self) {
        if self.observer.is_some() && Arc::strong_count(&self.exchanges) == 1 {
            self.fail("Aborted");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request_body::{RequestBodyLimits, RequestBodyStore};
    use crate::response_body::{ResponseBodyLimits, ResponseBodyStore};

    fn request_bodies() -> Arc<Mutex<RequestBodyStore>> {
        Arc::new(Mutex::new(RequestBodyStore::new(RequestBodyLimits {
            memory_threshold: 8, total_bytes: 1024, entries: 16,
        })))
    }

    #[test]
    fn failed_body_admission_never_marks_an_exchange_complete() {
        let bodies = Arc::new(Mutex::new(ResponseBodyStore::new(ResponseBodyLimits {
            memory_threshold: 1,
            total_bytes: 1,
            entries: 1,
        })));
        let trace = RequestTrace::new(bodies.clone(), request_bodies(), "budget".into());
        trace.begin("https://example.test/body", "GET", None, None).unwrap();
        trace.response(&Response {
            url: url::Url::parse("https://example.test/body").unwrap(),
            status: 200,
            headers: Default::default(),
            body: b"too large".to_vec(),
            raw_headers: None,
            request_raw_headers: None,
            redirected_from: Vec::new(),
            request_referrer: None,
        }, true);

        let exchange = trace.last().unwrap();
        assert!(!exchange.body_complete);
        assert_eq!(exchange.body_request_id.as_deref(), Some("budget-hop-0"));
        let error = match bodies.lock().unwrap_or_else(|e| e.into_inner()).get("budget-hop-0") {
            Some(Err(error)) => error,
            _ => panic!("failed admission must retain the body-store diagnostic"),
        };
        assert!(error.to_string().contains("response_body_budget_exhausted"));
    }

    #[test]
    fn observed_binary_resource_keeps_valid_utf8_bytes_binary() {
        struct Observer;
        impl RequestLifecycleObserver for Observer {
            fn started(&self, _: &str, _: ResourceType, _: usize, _: &Exchange)
                -> Result<(), ObscuraNetError> { Ok(()) }
            fn terminal(&self, _: &str, _: ResourceType, _: usize, _: &Exchange,
                _: Option<&[u8]>, _: Option<&str>) {}
        }
        let bodies = Arc::new(Mutex::new(ResponseBodyStore::default()));
        let trace = RequestTrace::new(bodies.clone(), request_bodies(), "image".into())
            .observe(Arc::new(Observer), ResourceType::Image);
        trace.begin("https://example.test/image", "GET", None, None).unwrap();
        trace.start().unwrap();
        trace.response(&Response {
            url: url::Url::parse("https://example.test/image").unwrap(),
            status: 200,
            headers: std::collections::HashMap::from([
                ("content-type".into(), "image/svg+xml".into()),
            ]),
            body: b"valid utf8 image bytes".to_vec(),
            raw_headers: None,
            request_raw_headers: None,
            redirected_from: Vec::new(),
            request_referrer: None,
        }, true);
        let (_, binary) = bodies.lock().unwrap().get("image-hop-0").unwrap().unwrap();
        assert!(binary);

        let documents = Arc::new(Mutex::new(ResponseBodyStore::default()));
        let trace = RequestTrace::new(documents.clone(), request_bodies(), "document".into())
            .observe(Arc::new(Observer), ResourceType::Document);
        trace.begin("https://example.test/file.pdf", "GET", None, None).unwrap();
        trace.start().unwrap();
        trace.response(&Response {
            url: url::Url::parse("https://example.test/file.pdf").unwrap(),
            status: 200,
            headers: std::collections::HashMap::from([
                ("content-type".into(), "application/pdf".into()),
            ]),
            body: b"valid utf8 pdf bytes".to_vec(),
            raw_headers: None,
            request_raw_headers: None,
            redirected_from: Vec::new(),
            request_referrer: None,
        }, true);
        let (_, binary) = documents.lock().unwrap()
            .get("document-hop-0").unwrap().unwrap();
        assert!(binary);
    }

    #[test]
    fn standard_and_overridden_transport_bodies_are_distinct_and_exact() {
        let response_bodies = Arc::new(Mutex::new(ResponseBodyStore::default()));
        let request_bodies = request_bodies();
        let trace = RequestTrace::new(response_bodies, request_bodies.clone(), "override".into());
        let original: Vec<u8> = (0..=255).collect();
        trace.begin("https://example.test/", "POST", None, Some(&original)).unwrap();
        trace.update_request("https://example.test/", "POST", HeaderCapture {
            capture_stage: "test", encoding: "base64", fields: Vec::new(),
        }, Some(b"replacement")).unwrap();
        let exchange = trace.last().unwrap();
        let standard = exchange.request_body_request_id.unwrap();
        let transport = exchange.transport_request_body_request_id.unwrap();
        let store = request_bodies.lock().unwrap();
        assert_eq!(store.get(&standard).unwrap().unwrap().with_bytes(|body| body.to_vec()).unwrap(), original);
        assert_eq!(store.get(&transport).unwrap().unwrap().read(0, 64).unwrap(), b"replacement");
    }

    #[test]
    fn redirect_alias_distinguishes_302_absent_from_307_preserved() {
        let response_bodies = Arc::new(Mutex::new(ResponseBodyStore::default()));
        let request_bodies = request_bodies();
        let trace = RequestTrace::new(response_bodies, request_bodies.clone(), "redirect".into());
        trace.begin("https://example.test/start", "POST", None, Some(b"payload")).unwrap();
        trace.begin("https://example.test/after-302", "GET", None, None).unwrap();
        assert!(request_bodies.lock().unwrap().get("redirect").is_none());
        trace.begin("https://example.test/after-307", "POST", None, Some(b"payload")).unwrap();
        assert_eq!(request_bodies.lock().unwrap().get("redirect").unwrap().unwrap().read(0, 99).unwrap(), b"payload");
    }
}
