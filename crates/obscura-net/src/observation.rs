//! Request-scoped diagnostic capture. This is application/transport metadata,
//! never a wire capture. Incomplete bodies are not retained as complete bodies.
use std::sync::{Arc, Mutex};
use crate::{HeaderCapture, Response};

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
}

#[derive(Clone)]
pub struct RequestTrace {
    exchanges: Arc<Mutex<Vec<Exchange>>>,
    bodies: Arc<Mutex<crate::response_body::ResponseBodyStore>>,
    request_bodies: Arc<Mutex<crate::request_body::RequestBodyStore>>,
    request_id: String,
    capture_response_bodies: bool,
    capture_redirect_response_bodies: bool,
}

impl RequestTrace {
    pub fn new(bodies: Arc<Mutex<crate::response_body::ResponseBodyStore>>,
        request_bodies: Arc<Mutex<crate::request_body::RequestBodyStore>>, request_id: String,
    ) -> Self {
        Self {
            exchanges: Arc::new(Mutex::new(Vec::new())), bodies, request_bodies,
            request_id, capture_response_bodies: true,
            capture_redirect_response_bodies: true,
        }
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
        });
        Ok(())
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
        -> Result<(), crate::request_body::RequestBodyError>
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
            self.update_request(&url, &method, headers.clone(), body_present.then_some(body))?;
        }
        if let Some(exchange) = self.exchanges.lock().unwrap_or_else(|e| e.into_inner()).last_mut() {
            exchange.request_headers = Some(headers);
        }
        Ok(())
    }

    pub fn response(&self, response: &Response, body_complete: bool) {
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
                    .insert(body_id.clone(), &response.body, false)
                {
                    Ok(()) => {
                        exchange.body_request_id = Some(body_id);
                        true
                    }
                    Err(error) => {
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
        assert!(exchange.body_request_id.is_none());
        let error = match bodies.lock().unwrap_or_else(|e| e.into_inner()).get("budget-hop-0") {
            Some(Err(error)) => error,
            _ => panic!("failed admission must retain the body-store diagnostic"),
        };
        assert!(error.to_string().contains("response_body_budget_exhausted"));
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
