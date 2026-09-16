//! Bounded, page-owned network evidence. Payloads never travel in event lines.
use super::*;
use std::{collections::VecDeque, sync::Mutex};
const BODY_LIMIT: usize = 8 * 1024 * 1024;
const PAGE_LIMIT: usize = 32 * 1024 * 1024;
#[derive(Default)]
pub struct Network {
    sequence: u64,
    bytes: usize,
    bodies: HashMap<u64, Option<Vec<u8>>>,
    order: VecDeque<u64>,
    failed: bool,
}
impl Network {
    fn store(&mut self, body: &[u8]) -> u64 {
        self.sequence += 1;
        let id = self.sequence;
        while self.order.len() >= 256 || self.bytes + body.len().min(BODY_LIMIT) > PAGE_LIMIT {
            let Some(old) = self.order.pop_front() else {
                break;
            };
            if let Some(Some(b)) = self.bodies.remove(&old) {
                self.bytes -= b.len();
            }
        }
        let data = (body.len() <= BODY_LIMIT).then(|| body.to_vec());
        self.bytes += data.as_ref().map_or(0, Vec::len);
        self.bodies.insert(id, data);
        self.order.push_back(id);
        id
    }
    fn event(
        &mut self,
        page: &str,
        kind: &str,
        request: &RequestInfo,
        response: Option<&obscura_net::Response>,
    ) {
        let request_body = self.store(&request.body);
        let req = json!({"url":request.url,"method":request.method,"headers":request.headers,"body_id":request_body});
        let value = if let Some(response) = response {
            let body = self.store(&response.body);
            json!({"url":response.url,"status":response.status,"headers":response.headers,"redirected_from":response.redirected_from,"body_id":body,"request":req})
        } else {
            req
        };
        if protocol::output(json!({"event":kind,"page_id":page,"data":value})).is_err() {
            self.failed = true;
        }
    }
}
impl BrowserRuntime {
    pub(super) fn observe(&mut self, id: &str) {
        let network = Arc::new(Mutex::new(Network::default()));
        let entry = self.pages.get_mut(id).unwrap();
        let n = network.clone();
        let p = id.to_owned();
        entry.page.on_request(Arc::new(move |request| {
            if let Ok(mut n) = n.lock() {
                n.event(&p, "request", request, None);
            }
        }));
        let n = network.clone();
        let p = id.to_owned();
        entry.page.on_response(Arc::new(move |request, response| {
            if let Ok(mut n) = n.lock() {
                n.event(&p, "response", request, Some(response));
            }
        }));
        self.networks.insert(id.to_owned(), network);
    }
    pub fn network_snapshot(&self) -> HashMap<String, Arc<Mutex<Network>>> {
        self.networks.clone()
    }
    pub(super) fn network_body(&mut self, request: &Request) -> Result<Value, Error> {
        read_body(&self.networks, request)
    }
    pub async fn automation_tick(&mut self) -> Result<(), String> {
        if self.protocol_version != "2" || self.mode != "RUNNING" || self.poisoned {
            return Ok(());
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        for entry in self.pages.values_mut() {
            entry.page.advance_automation(deadline).await?;
        }
        Ok(())
    }
}

pub fn evidence_response(
    networks: &HashMap<String, Arc<Mutex<Network>>>,
    request: &Request,
) -> Value {
    match read_body(networks, request) {
        Ok(result) => json!({"id":request.id,"ok":true,"result":result}),
        Err((code, state)) => protocol::error(request.id, code, state),
    }
}
fn read_body(
    networks: &HashMap<String, Arc<Mutex<Network>>>,
    request: &Request,
) -> Result<Value, Error> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ReadBody {
        body_id: u64,
        offset: usize,
    }
    let args: ReadBody = params(request)?;
    let id = request.page_id.as_ref().ok_or(invalid("MISSING_PAGE"))?;
    let n = networks
        .get(id)
        .ok_or(invalid("UNKNOWN_PAGE"))?
        .lock()
        .map_err(|_| invalid("NETWORK_FAILED"))?;
    if n.failed {
        return Err(invalid("NETWORK_FAILED"));
    }
    let body = n
        .bodies
        .get(&args.body_id)
        .ok_or(invalid("BODY_RELEASED"))?
        .as_ref()
        .ok_or(invalid("BODY_LIMIT"))?;
    if args.offset > body.len() {
        return Err(invalid("INVALID_OFFSET"));
    }
    let end = args.offset.saturating_add(8192).min(body.len());
    Ok(json!({"bytes":body[args.offset..end],"eof":end==body.len()}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn body_cache_preserves_bytes_and_reports_limits() {
        let mut n = Network::default();
        let binary = vec![0, 255, 1, 128];
        let first = n.store(&binary);
        assert_eq!(n.bodies[&first].as_ref(), Some(&binary));
        let large = n.store(&vec![0; BODY_LIMIT + 1]);
        assert_eq!(n.bodies[&large], None);
        for _ in 0..256 {
            n.store(b"x");
        }
        assert!(!n.bodies.contains_key(&first));
        assert_eq!(n.order.len(), 256);
        assert!(n.bytes <= PAGE_LIMIT);
    }
}
