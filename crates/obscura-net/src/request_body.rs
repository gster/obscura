//! Byte-exact request retention with Page-owned budgets.
//!
//! Request bodies have different lifetime and access semantics from response
//! bodies.  They are never consumed, and a logical Network request id is a
//! movable alias for the current redirect hop.  Canonical per-hop captures
//! remain readable for the Page lifetime.
use std::collections::{HashMap, HashSet};

use crate::response_body::ResponseBody;

#[derive(Clone, Copy, Debug)]
pub struct RequestBodyLimits {
    /// Bodies larger than this are spooled rather than copied into memory.
    pub memory_threshold: usize,
    /// Total retained unique raw bytes. Shared captures are charged once.
    pub total_bytes: usize,
    /// Canonical captures. Logical request aliases are free.
    pub entries: usize,
}

impl Default for RequestBodyLimits {
    fn default() -> Self {
        fn limit(name: &str, default: usize) -> usize {
            std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
        }
        Self {
            memory_threshold: limit("OBSCURA_NETWORK_REQUEST_BODY_BUFFER_BYTES", 2 * 1024 * 1024),
            total_bytes: limit("OBSCURA_NETWORK_REQUEST_BODY_TOTAL_BYTES", 256 * 1024 * 1024),
            entries: limit("OBSCURA_NETWORK_REQUEST_BODY_BUFFER_ENTRIES", 16384),
        }
    }
}

#[derive(Clone, Debug, thiserror::Error)]
pub enum RequestBodyError {
    #[error("request_body_budget_exhausted: {resource} limit {limit}, attempted {attempted}; clear request bodies to resume capture")]
    BudgetExceeded { resource: &'static str, limit: usize, attempted: usize },
    #[error("request_body_io_error: {0}")]
    Io(String),
    #[error("request_body_capture_missing: {0}")]
    Missing(String),
}

#[derive(Clone)]
struct Entry {
    body: ResponseBody,
}

/// A bounded, non-consuming request-body store. Budget or spool I/O failure is
/// sticky: the accepted prefix remains readable and no later capture is
/// admitted until `clear`.
pub struct RequestBodyStore {
    entries: HashMap<String, Entry>,
    aliases: HashMap<String, String>,
    absent_aliases: HashSet<String>,
    limits: RequestBodyLimits,
    total_bytes: usize,
    failure: Option<RequestBodyError>,
}

impl Default for RequestBodyStore {
    fn default() -> Self { Self::new(RequestBodyLimits::default()) }
}

impl RequestBodyStore {
    pub fn new(limits: RequestBodyLimits) -> Self {
        Self {
            entries: HashMap::new(), aliases: HashMap::new(), absent_aliases: HashSet::new(),
            limits, total_bytes: 0, failure: None,
        }
    }

    fn fail(&mut self, request_id: &str, error: RequestBodyError) -> RequestBodyError {
        tracing::warn!(request_id, reason = %error, "Request body capture stopped");
        self.failure = Some(error.clone());
        error
    }

    fn check_capture(&mut self, request_id: &str) -> Result<(), RequestBodyError> {
        if let Some(error) = &self.failure { return Err(error.clone()); }
        if self.entries.contains_key(request_id) {
            return Err(RequestBodyError::Missing(format!("duplicate capture id {request_id}")));
        }
        if self.entries.len() >= self.limits.entries {
            return Err(self.fail(request_id, RequestBodyError::BudgetExceeded {
                resource: "entries", limit: self.limits.entries,
                attempted: self.entries.len().saturating_add(1),
            }));
        }
        Ok(())
    }

    pub fn insert(&mut self, request_id: String, bytes: &[u8]) -> Result<(), RequestBodyError> {
        self.check_capture(&request_id)?;
        let attempted = self.total_bytes.checked_add(bytes.len()).unwrap_or(usize::MAX);
        if attempted > self.limits.total_bytes {
            return Err(self.fail(&request_id, RequestBodyError::BudgetExceeded {
                resource: "total_bytes", limit: self.limits.total_bytes, attempted,
            }));
        }
        let body = ResponseBody::from_bytes(bytes, self.limits.memory_threshold)
            .map_err(|error| self.fail(&request_id, RequestBodyError::Io(error.to_string())))?;
        self.total_bytes = attempted;
        self.entries.insert(request_id, Entry { body });
        Ok(())
    }

    /// Add a canonical capture which shares the exact spool owned by `from`.
    /// It consumes one entry but no additional byte budget.
    pub fn insert_shared(&mut self, from: &str, to: String) -> Result<(), RequestBodyError> {
        self.check_capture(&to)?;
        let body = self.entries.get(from).map(|entry| entry.body.clone())
            .ok_or_else(|| RequestBodyError::Missing(from.to_string()))?;
        self.entries.insert(to, Entry { body });
        Ok(())
    }

    /// Capture `bytes`, sharing `from` only when its raw contents are exactly
    /// equal. This keeps override divergence lossless without double-charging
    /// an unchanged transport body.
    pub fn insert_or_shared(&mut self, from: &str, to: String, bytes: &[u8]) -> Result<(), RequestBodyError> {
        let source = self.entries.get(from).map(|entry| entry.body.clone());
        let same = match source {
            Some(body) if body.len() == bytes.len() => body.with_bytes(|stored| stored == bytes)
                .map_err(|error| self.fail(&to, RequestBodyError::Io(error.to_string())))?,
            _ => false,
        };
        if same { self.insert_shared(from, to) } else { self.insert(to, bytes) }
    }

    /// Point a logical Network request id at a canonical current-hop capture.
    /// Aliases do not consume capture or byte budget.
    pub fn alias(&mut self, from: &str, to: &str) -> Result<(), RequestBodyError> {
        let canonical = if self.entries.contains_key(from) {
            Some(from.to_string())
        } else {
            self.aliases.get(from).filter(|id| self.entries.contains_key(*id)).cloned()
        };
        let Some(canonical) = canonical else {
            return self.failure.clone().map_or_else(
                || Err(RequestBodyError::Missing(from.to_string())), Err,
            );
        };
        self.absent_aliases.remove(to);
        self.aliases.insert(to.to_string(), canonical);
        Ok(())
    }

    /// Mark a logical request id as having no body on its current hop. This is
    /// distinct from an explicit empty capture, which has a real zero-byte id.
    pub fn clear_alias(&mut self, request_id: &str) {
        // Ordinary bodyless requests never need a tombstone: an unknown id
        // already reads as absent. Retain one only when a redirect clears a
        // previously body-bearing logical alias, so a later store-wide failure
        // cannot make that current hop look body-bearing. Each such tombstone
        // is backed by at least one budgeted canonical entry.
        if self.aliases.remove(request_id).is_some() {
            self.absent_aliases.insert(request_id.to_string());
        }
    }

    pub fn contains(&self, request_id: &str) -> bool {
        self.entries.contains_key(request_id)
            || self.aliases.get(request_id).is_some_and(|id| self.entries.contains_key(id))
    }

    pub fn get(&self, request_id: &str) -> Option<Result<ResponseBody, RequestBodyError>> {
        if self.absent_aliases.contains(request_id) { return None; }
        let id = self.aliases.get(request_id).map(String::as_str).unwrap_or(request_id);
        if let Some(entry) = self.entries.get(id) { return Some(Ok(entry.body.clone())); }
        self.failure.clone().map(Err)
    }

    pub fn failure(&self) -> Option<RequestBodyError> { self.failure.clone() }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.aliases.clear();
        self.absent_aliases.clear();
        self.total_bytes = 0;
        self.failure = None;
    }
}

impl Drop for RequestBodyStore {
    fn drop(&mut self) { self.clear(); }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits(total_bytes: usize, entries: usize) -> RequestBodyLimits {
        RequestBodyLimits { memory_threshold: 8, total_bytes, entries }
    }

    #[test]
    fn binary_spools_exactly_and_shared_capture_is_not_charged_twice() {
        let bytes: Vec<u8> = (0..=255).collect();
        let mut store = RequestBodyStore::new(limits(bytes.len(), 2));
        store.insert("hop-standard".into(), &bytes).unwrap();
        store.insert_shared("hop-standard", "hop-transport".into()).unwrap();
        for id in ["hop-standard", "hop-transport"] {
            assert_eq!(store.get(id).unwrap().unwrap().with_bytes(|body| body.to_vec()).unwrap(), bytes);
        }
    }

    #[test]
    fn absent_alias_and_explicit_empty_are_distinct() {
        let mut store = RequestBodyStore::new(limits(0, 1));
        store.insert("empty".into(), &[]).unwrap();
        store.alias("empty", "request").unwrap();
        assert_eq!(store.get("request").unwrap().unwrap().len(), 0);
        store.alias("request", "loader").unwrap();
        assert_eq!(store.get("loader").unwrap().unwrap().len(), 0);
        store.clear_alias("request");
        assert!(store.get("request").is_none());
        assert!(store.contains("empty"));
    }

    #[test]
    fn budget_failure_is_sticky_and_keeps_the_accepted_prefix() {
        let mut store = RequestBodyStore::new(limits(3, 3));
        store.insert("first".into(), b"abc").unwrap();
        assert!(store.insert("too-large".into(), b"d").is_err());
        assert!(store.insert("later".into(), &[]).is_err());
        assert_eq!(store.get("first").unwrap().unwrap().read(0, 3).unwrap(), b"abc");
        assert!(store.failure().is_some());
    }

    #[test]
    fn bodyless_requests_do_not_accumulate_unbudgeted_tombstones() {
        let mut store = RequestBodyStore::new(limits(0, 0));
        for index in 0..100_000 {
            store.clear_alias(&format!("get-{index}"));
        }
        assert!(store.aliases.is_empty());
        assert!(store.absent_aliases.is_empty());
    }
}
