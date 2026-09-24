//! Request-scoped diagnostic capture. This is application/transport metadata,
//! never a wire capture. Incomplete bodies are not retained as complete bodies.
use std::sync::{Arc, Mutex};
use crate::{HeaderCapture, Response};

#[derive(Debug, Clone)]
pub struct Exchange {
    pub url: String,
    pub method: String,
    pub request_started_at: f64,
    /// Headers and body have been prepared by primp; this does not prove wire send.
    pub request_prepared_at: Option<f64>,
    pub response_headers_at: Option<f64>,
    pub request_headers: Option<HeaderCapture>,
    pub request_body_size: usize,
    /// Small UTF-8 payloads only, for CDP postData diagnostics.
    pub request_post_data: Option<String>,
    pub response: Option<Response>,
    pub body_complete: bool,
    pub body_size: usize,
    pub body_request_id: Option<String>,
}

#[derive(Clone)]
pub struct RequestTrace {
    exchanges: Arc<Mutex<Vec<Exchange>>>,
    bodies: Arc<Mutex<crate::response_body::ResponseBodyStore>>,
    request_id: String,
}

impl RequestTrace {
    pub fn new(bodies: Arc<Mutex<crate::response_body::ResponseBodyStore>>, request_id: String) -> Self {
        Self { exchanges: Arc::new(Mutex::new(Vec::new())), bodies, request_id }
    }

    pub fn begin(&self, url: &str, method: &str, headers: Option<HeaderCapture>, body_size: usize) {
        self.exchanges.lock().unwrap_or_else(|e| e.into_inner()).push(Exchange {
            url: url.into(), method: method.into(),
            request_started_at: now_timestamp(), request_headers: headers,
            request_prepared_at: None, response_headers_at: None,
            request_body_size: body_size, request_post_data: None, response: None, body_complete: false, body_size: 0, body_request_id: None,
        });
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

    pub fn update_request(&self, url: &str, method: &str, headers: HeaderCapture, body_size: usize) {
        if let Some(exchange) = self.exchanges.lock().unwrap_or_else(|e| e.into_inner()).last_mut() {
            exchange.url = url.into(); exchange.method = method.into();
            exchange.request_headers = Some(headers); exchange.request_body_size = body_size;
            exchange.request_post_data = None;
        }
    }

    pub fn request_body(&self, body: &[u8]) {
        const MAX_POST_DATA: usize = 16 * 1024;
        if let Some(exchange) = self.exchanges.lock().unwrap_or_else(|e| e.into_inner()).last_mut() {
            exchange.request_body_size = body.len();
            exchange.request_post_data = if body.len() <= MAX_POST_DATA {
                std::str::from_utf8(body).ok().map(str::to_owned)
            } else { None };
        }
    }

    pub fn prepared(&self, headers: HeaderCapture) {
        if let Some(exchange) = self.exchanges.lock().unwrap_or_else(|e| e.into_inner()).last_mut() {
            exchange.request_headers = Some(headers);
            exchange.request_prepared_at.get_or_insert_with(now_timestamp);
        }
    }

    pub fn response(&self, response: &Response, body_complete: bool) {
        let mut exchanges = self.exchanges.lock().unwrap_or_else(|e| e.into_inner());
        let index = exchanges.len().saturating_sub(1);
        if let Some(exchange) = exchanges.last_mut() {
            exchange.response_headers_at.get_or_insert_with(now_timestamp);
            // Spool now rather than retaining every redirect body in memory.
            // Header-only captures never register an empty successful body.
            let body_stored = if body_complete {
                let body_id = format!("{}-hop-{}", self.request_id, index);
                match self.bodies.lock().unwrap_or_else(|e| e.into_inner())
                    .insert(body_id.clone(), &response.body, false)
                {
                    Ok(()) => {
                        exchange.body_request_id = Some(body_id);
                        true
                    }
                    Err(_) => false,
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

fn now_timestamp() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default().as_secs_f64()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::response_body::{ResponseBodyLimits, ResponseBodyStore};

    #[test]
    fn failed_body_admission_never_marks_an_exchange_complete() {
        let bodies = Arc::new(Mutex::new(ResponseBodyStore::new(ResponseBodyLimits {
            memory_threshold: 1,
            total_bytes: 1,
            entries: 1,
        })));
        let trace = RequestTrace::new(bodies.clone(), "budget".into());
        trace.begin("https://example.test/body", "GET", None, 0);
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
    fn request_post_data_is_text_only_and_bounded() {
        let bodies = Arc::new(Mutex::new(ResponseBodyStore::new(ResponseBodyLimits::default())));
        let trace = RequestTrace::new(bodies, "request".into());
        trace.begin("https://example.test/", "POST", None, 0);
        trace.request_body(b"route=BWI-MCO");
        assert_eq!(trace.last().unwrap().request_post_data.as_deref(), Some("route=BWI-MCO"));
        trace.request_body(b"\xff");
        assert!(trace.last().unwrap().request_post_data.is_none());
        trace.request_body(&vec![b'a'; 16 * 1024 + 1]);
        assert!(trace.last().unwrap().request_post_data.is_none());
        trace.update_request("https://example.test/next", "GET", HeaderCapture {
            capture_stage: "scriptRequest", encoding: "base64", fields: Vec::new(),
        }, 0);
        assert!(trace.last().unwrap().request_post_data.is_none());
    }
}
