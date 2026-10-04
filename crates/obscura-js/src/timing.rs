//! Bounded, document-owned timing entries; missing transport phases stay absent.
use serde::Serialize;
use obscura_net::timing::{now, RequestTiming};

const MAX_ENTRIES: usize = 1024;
const MAX_NAME_BYTES: usize = 4096;
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Entry {
    pub sequence: u64,
    pub name: String,
    pub entry_type: &'static str,
    pub initiator_type: &'static str,
    pub start_time: f64,
    pub duration: f64,
    pub fetch_start: f64,
    pub response_end: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_hop_protocol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decoded_body_size: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dom_interactive: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dom_content_loaded_event_start: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dom_content_loaded_event_end: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dom_complete: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub load_event_start: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub load_event_end: Option<f64>,
}
#[derive(Debug)]
pub(crate) struct Timeline {
    pub origin: f64,
    entries: Vec<Entry>,
    user_entries: Vec<Entry>,
    navigation: Option<Entry>,
    limit: usize,
    sequence: u64,
    dropped: u64,
    observer_types: u8,
    observer_records: Vec<(Entry, bool)>,
    observer_dropped: u64,
}
impl Default for Timeline {
    fn default() -> Self { Self { origin: now(), entries: Vec::new(), user_entries: Vec::new(), navigation: None,
        limit: 250, sequence: 0, dropped: 0, observer_types: 0,
        observer_records: Vec::new(), observer_dropped: 0 } }
}
impl Timeline {
    pub fn elapsed(&self) -> f64 { (now() - self.origin).max(0.0) }
    pub fn user(&mut self, name: &str, measure: bool, start: f64, duration: f64) -> Option<Entry> {
        if name.len() > MAX_NAME_BYTES || self.user_entries.len() >= MAX_ENTRIES
            || !start.is_finite() || start < 0.0 || !duration.is_finite() { return None; }
        self.sequence = self.sequence.saturating_add(1);
        let entry = Entry { sequence: self.sequence, name: name.into(),
            entry_type: if measure { "measure" } else { "mark" }, start_time: start, duration,
            ..Entry::default() };
        self.queue_observers(&entry, if measure { 8 } else { 4 }, true);
        self.user_entries.push(entry.clone());
        Some(entry)
    }
    pub fn clear_user(&mut self, measure: bool, name: Option<&str>) {
        let kind = if measure { "measure" } else { "mark" };
        self.user_entries.retain(|entry| entry.entry_type != kind || name.is_some_and(|name| name != entry.name));
    }
    fn entry(&self, facts: &RequestTiming, name: &str, kind: &'static str, details: bool) -> Option<Entry> {
        if name.len() > MAX_NAME_BYTES || facts.overflow || facts.start < self.origin { return None; }
        let end = facts.end.unwrap_or_else(now).max(facts.start);
        let mut clean_name = url::Url::parse(name).ok()?;
        let _ = clean_name.set_username(""); let _ = clean_name.set_password(None); clean_name.set_fragment(None);
        Some(Entry { sequence: 0, name: clean_name.to_string(), entry_type: "resource", initiator_type: kind,
            start_time: facts.start - self.origin, duration: end - facts.start,
            fetch_start: (if details { facts.fetch_start } else { facts.start }) - self.origin,
            response_end: end - self.origin,
            next_hop_protocol: if details { facts.protocol.map(str::to_owned) } else { Some(String::new()) },
            decoded_body_size: if details { facts.decoded_body_size } else { Some(0) },
            dom_interactive: None, dom_content_loaded_event_start: None,
            dom_content_loaded_event_end: None, dom_complete: None,
            load_event_start: None, load_event_end: None })
    }
    pub fn resource(&mut self, facts: &RequestTiming, name: &str, kind: &'static str, origin: &str, failed: bool) {
        let Some(mut entry) = self.entry(facts, name, kind, !failed && facts.allows_details(origin)) else { return; };
        self.sequence = self.sequence.saturating_add(1); entry.sequence = self.sequence;
        self.queue_observers(&entry, 1, self.entries.len() < self.limit);
        if self.entries.len() >= self.limit { self.dropped = self.dropped.saturating_add(1); return; }
        self.entries.push(entry);
    }
    pub fn navigation(&mut self, start: f64, facts: &RequestTiming, url: &str) {
        *self = Self { origin: start, ..Self::default() };
        let Some(mut entry) = self.entry(facts, url, "navigation", true) else { return; };
        entry.entry_type = "navigation"; entry.start_time = 0.0;
        entry.duration = 0.0;
        entry.dom_interactive = Some(0.0); entry.dom_content_loaded_event_start = Some(0.0);
        entry.dom_content_loaded_event_end = Some(0.0); entry.dom_complete = Some(0.0);
        entry.load_event_start = Some(0.0); entry.load_event_end = Some(0.0);
        self.navigation = Some(entry);
    }
    pub fn lifecycle(&mut self, phase: u8, end: bool) {
        let elapsed = self.elapsed();
        let Some(entry) = self.navigation.as_mut() else { return; };
        match (phase, end) {
            (1, false) => entry.dom_interactive = Some(elapsed),
            (2, false) => entry.dom_content_loaded_event_start = Some(elapsed),
            (2, true) => entry.dom_content_loaded_event_end = Some(elapsed),
            (3, false) => entry.dom_complete = Some(elapsed),
            (5, false) => entry.load_event_start = Some(elapsed),
            (3, true) => {
                entry.load_event_end = Some(elapsed); entry.duration = elapsed;
                self.sequence = self.sequence.saturating_add(1); entry.sequence = self.sequence;
            },
            _ => {},
        }
        if phase == 3 && end && self.observer_types & 2 != 0 {
            let entry = entry.clone();
            self.queue_observers(&entry, 2, true);
        }
    }
    // Independent of public retention. Work/allocations remain lazy when no
    // matching observers exist; overflow is counted rather than unbounded.
    fn queue_observers(&mut self, entry: &Entry, kind: u8, retained: bool) {
        if self.observer_types & kind == 0 { return; }
        if self.observer_records.len() < MAX_ENTRIES { self.observer_records.push((entry.clone(), retained)); }
        else { self.observer_dropped = self.observer_dropped.saturating_add(1); }
    }
    pub fn observe(&mut self, types: u8) {
        self.observer_types = types & 15;
        if self.observer_types == 0 { self.observer_records.clear(); }
    }
    pub fn take_observer_records(&mut self) -> serde_json::Value {
        let entries = std::mem::take(&mut self.observer_records);
        serde_json::json!({"origin":self.origin,"entries":entries,"dropped":self.observer_dropped})
    }
    pub fn set_limit(&mut self, limit: usize) -> bool {
        if limit > MAX_ENTRIES { return false; }
        self.limit = limit; true
    }
    pub fn clear_resources(&mut self) { self.entries.clear(); }
    pub fn navigation_snapshot(&self) -> serde_json::Value {
        serde_json::json!({"origin":self.origin,"entries":self.navigation.iter().collect::<Vec<_>>(),"dropped":self.dropped})
    }
    pub fn snapshot(&self) -> serde_json::Value {
        let mut entries: Vec<_> = self.navigation.iter().chain(self.entries.iter()).chain(self.user_entries.iter()).collect();
        entries.sort_by(|a, b| a.start_time.total_cmp(&b.start_time));
        serde_json::json!({"origin":self.origin,"entries": entries, "dropped": self.dropped})
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn facts(start: f64) -> RequestTiming {
        RequestTiming { start, fetch_start: start, end: Some(start + 5.0), protocol: Some("http/1.1"),
            decoded_body_size: Some(17), hops: vec![("https://a.test/a".into(), None)],
            complete: true, transport_attempted: true, overflow: false }
    }
    #[test]
    fn bounded_entries_tao_clear_and_document_epoch() {
        let mut t = Timeline::default(); let f = facts(t.origin + 1.0);
        t.set_limit(1); t.resource(&f,"https://a.test/a","fetch","https://b.test",false);
        t.resource(&f,"https://a.test/b","fetch","https://b.test",false);
        assert_eq!(t.entries.len(),1); assert_eq!(t.dropped,1);
        assert_eq!(t.entries[0].decoded_body_size,Some(0));
        assert_eq!(t.entries[0].next_hop_protocol.as_deref(),Some(""));
        assert_eq!(t.entries[0].duration,5.0);
        assert!(!t.snapshot()["entries"][0].as_object().unwrap().contains_key("connectStart"));
        t.clear_resources(); assert!(t.entries.is_empty());
        t.origin=f.start+100.0; t.resource(&f,"https://a.test/a","fetch","https://a.test",false);
        assert!(t.entries.is_empty()); assert!(!t.set_limit(1025));
    }
    #[test]
    fn navigation_uses_real_lifecycle_and_keeps_resource_origin() {
        let start=now(); let f=facts(start);
        let mut t=Timeline::default(); t.navigation(start,&f,"https://a.test/");
        assert_eq!(t.navigation.as_ref().unwrap().duration,0.0);
        t.lifecycle(1,false); t.lifecycle(2,false); t.lifecycle(2,true);
        t.lifecycle(3,false); t.lifecycle(5,false); t.lifecycle(3,true);
        let n=t.navigation.as_ref().unwrap();
        assert!(n.dom_content_loaded_event_end>=n.dom_content_loaded_event_start);
        assert!(n.load_event_end>=n.load_event_start); assert!(n.sequence>0);
    }
    #[test]
    fn redirects_are_visible_only_with_tao_and_failure_is_opaque() {
        let mut t=Timeline::default(); let mut f=facts(t.origin+1.0);
        f.fetch_start=f.start+2.0;
        t.resource(&f,"https://a.test/a","fetch","https://a.test",false);
        assert!((t.entries[0].fetch_start-3.0).abs()<1e-9);
        t.resource(&f,"https://a.test/a","fetch","https://b.test",false);
        assert!((t.entries[1].fetch_start-1.0).abs()<1e-9);
        f.end=None; f.complete=false;
        t.resource(&f,"https://a.test/a","fetch","https://a.test",true);
        assert!((t.entries[2].fetch_start-1.0).abs()<1e-9);
        assert_eq!(t.entries[2].next_hop_protocol.as_deref(),Some(""));
        assert_eq!(t.entries[2].decoded_body_size,Some(0));
    }

    #[test]
    fn observer_queue_survives_public_clear_zero_and_full_buffers() {
        let mut t = Timeline::default(); let f = facts(t.origin + 1.0);
        t.observe(1); t.set_limit(1);
        for path in ["a", "b", "c"] { t.resource(&f, &format!("https://a.test/{path}"), "fetch", "https://a.test", false); }
        assert_eq!(t.entries.len(), 1); assert_eq!(t.dropped, 2);
        t.clear_resources();
        let batch = t.take_observer_records();
        assert_eq!(batch["entries"].as_array().unwrap().len(), 3);
        assert!(t.take_observer_records()["entries"].as_array().unwrap().is_empty());
        t.set_limit(0); t.resource(&f, "https://a.test/zero", "fetch", "https://a.test", false);
        assert!(t.entries.is_empty()); assert_eq!(t.take_observer_records()["entries"].as_array().unwrap().len(), 1);
    }
    #[test]
    fn observer_queue_is_lazy_bounded_and_document_owned() {
        let mut t = Timeline::default(); let f = facts(t.origin + 1.0); t.set_limit(0);
        t.resource(&f, "https://a.test/no-observer", "fetch", "https://a.test", false);
        assert!(t.observer_records.is_empty()); assert_eq!(t.observer_records.capacity(), 0);
        t.observe(1);
        for _ in 0..MAX_ENTRIES + 3 { t.resource(&f, "https://a.test/full", "fetch", "https://a.test", false); }
        assert_eq!(t.observer_records.len(), MAX_ENTRIES); assert_eq!(t.observer_dropped, 3);
        t.observe(0); assert!(t.observer_records.is_empty());
        t.observe(2); t.resource(&f, "https://a.test/wrong-type", "fetch", "https://a.test", false);
        assert!(t.observer_records.is_empty());
        t.navigation(f.start, &f, "https://a.test/new");
        assert_eq!(t.observer_types, 0); assert!(t.observer_records.is_empty()); assert_eq!(t.observer_dropped, 0);
        t.observe(2); t.lifecycle(3, true);
        let batch = t.take_observer_records(); assert_eq!(batch["entries"][0][0]["entryType"], "navigation");
    }

}
