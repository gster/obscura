//! Monotonic request facts. No socket phase is inferred from a high-level await.
use std::sync::OnceLock;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

fn clock() -> &'static (Instant, f64) {
    static CLOCK: OnceLock<(Instant, f64)> = OnceLock::new();
    CLOCK.get_or_init(|| (Instant::now(), SystemTime::now().duration_since(UNIX_EPOCH)
        .unwrap_or_default().as_secs_f64() * 1000.0))
}
pub fn now() -> f64 { clock().0.elapsed().as_secs_f64() * 1000.0 }
pub fn epoch_at(monotonic: f64) -> f64 { (clock().1 + monotonic).floor() }

/// Additive API: public Response and Exchange remain source compatible.
/// `protocol` is the actual response HTTP version, not a persona prediction.
#[derive(Clone, Debug)]
pub struct RequestTiming {
    pub start: f64,
    pub fetch_start: f64,
    pub end: Option<f64>,
    pub protocol: Option<&'static str>,
    pub decoded_body_size: Option<usize>,
    pub hops: Vec<(String, Option<String>)>,
    pub complete: bool,
    pub transport_attempted: bool,
    pub overflow: bool,
}
impl Default for RequestTiming {
    fn default() -> Self { let start = now(); Self { start, fetch_start: start, end: None, protocol: None,
        decoded_body_size: None, hops: Vec::new(), complete: false, transport_attempted: false, overflow: false } }
}
impl RequestTiming {
    pub(crate) fn begin(&mut self, url: &str) {
        let started = now();
        if self.hops.is_empty() { self.start = started; }
        self.fetch_start = started;
        if self.hops.len() >= 32 || url.len() > 4096 { self.overflow = true; return; }
        self.hops.push((url.into(), None));
        self.protocol = None;
        self.end = None;
        self.decoded_body_size = None;
        self.complete = false;
    }
    pub(crate) fn response(&mut self, response: &crate::Response, body_complete: bool) {
        if body_complete {
            self.end.get_or_insert_with(now);
            self.decoded_body_size = Some(response.body.len());
            self.complete = true;
        }
        if let Some((url, tao)) = self.hops.last_mut() {
            if response.url.as_str().len() > 4096 { self.overflow = true; return; }
            *url = response.url.to_string();
            // The raw capture preserves repeated fields; the compatibility map
            // may collapse them. Bound before allocating, and fail closed on
            // invalid bytes rather than accepting a partial header projection.
            *tao = if let Some(raw) = &response.raw_headers {
                let mut combined = String::new();
                let mut valid = true;
                for field in &raw.fields {
                    if !field.name.eq_ignore_ascii_case(b"timing-allow-origin") { continue; }
                    let Ok(value) = std::str::from_utf8(&field.value) else { valid = false; break; };
                    if combined.len().saturating_add(value.len()).saturating_add(1) > 4096 {
                        valid = false; break;
                    }
                    if !combined.is_empty() { combined.push(','); }
                    combined.push_str(value);
                }
                (valid && !combined.is_empty()).then_some(combined)
            } else {
                response.headers.iter().find(|(name, _)| name.eq_ignore_ascii_case("timing-allow-origin"))
                    .and_then(|(_, value)| (value.len() <= 4096).then(|| value.clone()))
            };
        }
    }
    pub fn allows_details(&self, origin: &str) -> bool {
        if self.overflow || self.hops.is_empty() { return false; }
        let mut previous: Option<String> = None;
        let mut redirect_tainted = false;
        let mut crossed_origin = false;
        for (url, tao) in &self.hops {
            let Ok(url) = url::Url::parse(url) else { return false; };
            let current = url.origin().ascii_serialization();
            if previous.as_ref().is_some_and(|last| last != &current && last != origin) {
                redirect_tainted = true;
            }
            crossed_origin |= current != origin || current == "null";
            let serialized_origin = if redirect_tainted { "null" } else { origin };
            let allowed = !crossed_origin || tao.as_ref().is_some_and(|value| value.split(',').any(|token| {
                let token = token.trim(); token == "*" || token == serialized_origin
            }));
            if !allowed { return false; }
            previous = Some(current);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clock_monotonic_and_epoch_share_elapsed_time() {
        let a = now(); let b = now(); assert!(b >= a);
        assert!(((epoch_at(b) - epoch_at(a)) - (b - a)).abs() < 1.01);
    }
    #[test]
    fn timing_allow_origin_checks_each_hop_and_never_invents_a_connection() {
        let mut t = RequestTiming::default();
        t.begin("https://a.test/r"); assert!(t.allows_details("https://a.test"));
        t.begin("https://b.test/r"); assert!(!t.allows_details("https://a.test"));
        t.hops.last_mut().unwrap().1 = Some("https://a.test".into());
        assert!(t.allows_details("https://a.test"));
        assert!(t.protocol.is_none()); assert!(t.end.is_none());
        for _ in 0..40 { t.begin("https://a.test/r"); }
        assert!(t.overflow); assert!(t.hops.len() <= 32);
        assert!(!t.allows_details("https://a.test"));
    }
    #[test]
    fn final_hop_and_repeated_tao_are_real_bounded_facts() {
        let mut t = RequestTiming::default();
        t.begin("https://other.test/start");
        let first = t.start;
        t.begin("https://other.test/final");
        assert_eq!(t.start, first); assert!(t.fetch_start >= first);
        let mut response = crate::Response {
            url: url::Url::parse("https://other.test/final").unwrap(), status: 200,
            headers: std::collections::HashMap::new(), body: Vec::new(), redirected_from: Vec::new(),
            raw_headers: Some(crate::HeaderCapture {capture_stage:"transportResponse", encoding:"base64",
                fields: vec![crate::RawHeader {name:b"timing-allow-origin".to_vec(), value:b"https://wrong.test".to_vec()},
                    crate::RawHeader {name:b"Timing-Allow-Origin".to_vec(), value:b"https://a.test".to_vec()}]}),
            request_raw_headers: None, request_referrer: None,
        };
        t.hops[0].1=Some("*".into()); t.response(&response, true);
        assert!(t.allows_details("https://a.test"));
        response.raw_headers.as_mut().unwrap().fields.push(crate::RawHeader {
            name:b"timing-allow-origin".to_vec(), value:vec![b'x';4097]});
        t.response(&response, true); assert!(!t.allows_details("https://a.test"));
    }
    #[test]
    fn tao_fails_closed_across_redirect_taint_and_return_to_origin() {
        let mut t=RequestTiming::default();
        t.begin("https://a.test/start"); t.begin("https://b.test/middle");
        t.hops[1].1=Some("https://a.test".into());
        assert!(t.allows_details("https://a.test"));
        t.begin("https://a.test/final");
        assert!(!t.allows_details("https://a.test"));
        t.hops[2].1=Some("https://a.test".into());
        assert!(!t.allows_details("https://a.test"));
        t.hops[2].1=Some("null".into()); assert!(t.allows_details("https://a.test"));
        t.hops[1].1=None; assert!(!t.allows_details("https://a.test"));
    }

}
