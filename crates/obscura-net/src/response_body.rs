//! Byte-exact response retention, independent of the browser and JavaScript runtimes.
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug)]
pub struct ResponseBodyLimits {
    /// Bodies larger than this are spooled rather than copied into memory.
    pub memory_threshold: usize,
    /// Total raw bytes retained by canonical Page body slots.
    pub total_bytes: usize,
    /// Captured canonical response slots; aliases are free.
    pub entries: usize,
}

impl Default for ResponseBodyLimits {
    fn default() -> Self {
        fn limit(name: &str, default: usize) -> usize {
            std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
        }
        Self {
            memory_threshold: limit("OBSCURA_NETWORK_BODY_BUFFER_BYTES", 2 * 1024 * 1024),
            total_bytes: limit("OBSCURA_NETWORK_BODY_TOTAL_BYTES", 256 * 1024 * 1024),
            entries: limit("OBSCURA_NETWORK_BODY_BUFFER_ENTRIES", 16384),
        }
    }
}

#[derive(Clone, Debug, thiserror::Error)]
pub enum ResponseBodyError {
    #[error("response_body_budget_exhausted: {resource} limit {limit}, attempted {attempted}; clear response bodies to resume capture")]
    BudgetExceeded { resource: &'static str, limit: usize, attempted: usize },
    #[error("response_body_io_error: {0}")]
    Io(String),
    #[error("response_body_already_consumed")]
    Consumed,
    #[error("response_body_access_conflict: Fetch.getResponseBody and Fetch.takeResponseBodyAsStream are mutually exclusive")]
    AccessConflict,
}

impl From<std::io::Error> for ResponseBodyError {
    fn from(error: std::io::Error) -> Self { Self::Io(error.to_string()) }
}

enum Storage {
    Memory(Vec<u8>),
    File(tempfile::NamedTempFile),
}
type SharedStorage = Mutex<Storage>;

/// A raw response body. Clones share storage; no text/base64 copy is retained.
/// An IO stream clone keeps a spool alive independently of the Page store.
#[derive(Clone)]
pub struct ResponseBody {
    storage: Arc<SharedStorage>,
    len: usize,
}

impl std::fmt::Debug for ResponseBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResponseBody").field("len", &self.len).finish()
    }
}

impl ResponseBody {
    pub fn from_bytes(bytes: &[u8], memory_threshold: usize) -> Result<Self, ResponseBodyError> {
        let storage = if bytes.len() > memory_threshold {
            let mut file = tempfile::NamedTempFile::new()?;
            file.write_all(bytes)?;
            Storage::File(file)
        } else {
            Storage::Memory(bytes.to_vec())
        };
        Ok(Self { storage: Arc::new(Mutex::new(storage)), len: bytes.len() })
    }

    pub fn len(&self) -> usize { self.len }

    /// Convert only at the protocol boundary. Memory bodies are borrowed;
    /// spooled bodies need one temporary raw buffer for a whole-body reply.
    pub fn with_bytes<T>(&self, f: impl FnOnce(&[u8]) -> T) -> Result<T, ResponseBodyError> {
        let mut guard = self.storage.lock().unwrap_or_else(|e| e.into_inner());
        match &mut *guard {
            Storage::Memory(bytes) => Ok(f(bytes)),
            Storage::File(file) => {
                file.seek(SeekFrom::Start(0))?;
                let mut bytes = Vec::with_capacity(self.len);
                file.read_to_end(&mut bytes)?;
                Ok(f(&bytes))
            }
        }
    }

    /// Read a bounded chunk without loading a spooled body into memory.
    pub fn read(&self, offset: usize, size: usize) -> Result<Vec<u8>, ResponseBodyError> {
        let start = offset.min(self.len);
        let end = start.saturating_add(size).min(self.len);
        let mut guard = self.storage.lock().unwrap_or_else(|e| e.into_inner());
        match &mut *guard {
            Storage::Memory(bytes) => Ok(bytes[start..end].to_vec()),
            Storage::File(file) => {
                file.seek(SeekFrom::Start(start as u64))?;
                let mut bytes = vec![0; end - start];
                file.read_exact(&mut bytes)?;
                Ok(bytes)
            }
        }
    }
}

impl From<Vec<u8>> for ResponseBody {
    fn from(bytes: Vec<u8>) -> Self {
        Self { len: bytes.len(), storage: Arc::new(Mutex::new(Storage::Memory(bytes))) }
    }
}

/// Fetch-specific access mode for one canonical body slot. This does not affect
/// ordinary readers such as Network.getResponseBody: those always retain the
/// immutable canonical body. Aliases share this state so they cannot be used to
/// obtain a second Fetch stream for the same captured response.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FetchAccess { None, Get, Streamed }

enum Entry {
    Ready { body: ResponseBody, binary: bool, fetch_access: FetchAccess },
}

/// Page-owned bounded capture. A failed admission stops further capture until
/// clear, retaining one diagnostic instead of an unbounded map of failures.
/// While stopped, a missing ID returns that diagnostic, not an assertion that
/// the request never existed. Existing entries remain readable.
pub struct ResponseBodyStore {
    entries: HashMap<String, Arc<Mutex<Entry>>>,
    limits: ResponseBodyLimits,
    total_bytes: usize,
    entry_count: usize,
    failure: Option<ResponseBodyError>,
}

impl Default for ResponseBodyStore {
    fn default() -> Self { Self::new(ResponseBodyLimits::default()) }
}

impl ResponseBodyStore {
    pub fn new(limits: ResponseBodyLimits) -> Self {
        Self { entries: HashMap::new(), limits, total_bytes: 0, entry_count: 0, failure: None }
    }

    fn fail(&mut self, request_id: &str, error: ResponseBodyError) -> ResponseBodyError {
        tracing::warn!(request_id, reason = %error, "Response body capture stopped");
        self.failure = Some(error.clone());
        error
    }

    fn check_entry(&mut self, request_id: &str) -> Result<(), ResponseBodyError> {
        if let Some(error) = &self.failure { return Err(error.clone()); }
        if !self.entries.contains_key(request_id) && self.entry_count >= self.limits.entries {
            return Err(self.fail(request_id, ResponseBodyError::BudgetExceeded {
                resource: "entries", limit: self.limits.entries,
                attempted: self.entry_count.saturating_add(1),
            }));
        }
        Ok(())
    }

    pub fn insert(&mut self, request_id: String, bytes: &[u8], binary: bool) -> Result<(), ResponseBodyError> {
        self.check_entry(&request_id)?;
        let existing = self.entries.get(&request_id).cloned();
        let previous_len = existing.as_ref().map(|entry| {
            match &*entry.lock().unwrap_or_else(|e| e.into_inner()) {
                Entry::Ready { body, .. } => body.len(),
            }
        }).unwrap_or(0);
        let total = (self.total_bytes - previous_len).checked_add(bytes.len());
        if total.is_none_or(|total| total > self.limits.total_bytes) {
            return Err(self.fail(&request_id, ResponseBodyError::BudgetExceeded {
                resource: "total_bytes", limit: self.limits.total_bytes,
                attempted: total.unwrap_or(usize::MAX),
            }));
        }
        let body = ResponseBody::from_bytes(bytes, self.limits.memory_threshold)
            .map_err(|error| self.fail(&request_id, error))?;
        self.total_bytes = total.unwrap();
        let replacement = Entry::Ready { body, binary, fetch_access: FetchAccess::None };
        if let Some(entry) = existing {
            *entry.lock().unwrap_or_else(|e| e.into_inner()) = replacement;
        } else {
            self.entry_count += 1;
            self.entries.insert(request_id, Arc::new(Mutex::new(replacement)));
        }
        Ok(())
    }

    /// Includes canonical entries and aliases, but not a store-wide failure.
    pub fn contains(&self, request_id: &str) -> bool {
        self.entries.contains_key(request_id)
    }

    pub fn get(&self, request_id: &str) -> Option<Result<(ResponseBody, bool), ResponseBodyError>> {
        let Some(entry) = self.entries.get(request_id) else {
            return self.failure.clone().map(Err);
        };
        Some(match &*entry.lock().unwrap_or_else(|e| e.into_inner()) {
            Entry::Ready { body, binary, .. } => Ok((body.clone(), *binary)),
        })
    }

    /// Fetch whole-body reads and stream transfer are mutually exclusive.
    /// Repeated whole-body reads are allowed and share the retained storage.
    pub fn get_for_fetch(&mut self, request_id: &str) -> Option<Result<(ResponseBody, bool), ResponseBodyError>> {
        let Some(entry) = self.entries.get(request_id) else {
            return self.failure.clone().map(Err);
        };
        let mut entry = entry.lock().unwrap_or_else(|e| e.into_inner());
        Some(match &mut *entry {
            Entry::Ready { body, binary, fetch_access } => {
                match *fetch_access {
                    FetchAccess::None | FetchAccess::Get => {
                        *fetch_access = FetchAccess::Get;
                        Ok((body.clone(), *binary))
                    }
                    FetchAccess::Streamed => Err(ResponseBodyError::AccessConflict),
                }
            }
        })
    }

    /// Mark this canonical slot as streamed and return an immutable storage
    /// clone. Ordinary reads remain available; only Fetch whole-body/stream
    /// access is mutually exclusive. Page clear/drop removes its references but
    /// an open IO stream keeps the shared backing alive.
    pub fn take_for_fetch(&mut self, request_id: &str) -> Option<Result<ResponseBody, ResponseBodyError>> {
        let Some(entry) = self.entries.get(request_id) else {
            return self.failure.clone().map(Err);
        };
        let mut entry = entry.lock().unwrap_or_else(|e| e.into_inner());
        Some(match &mut *entry {
            Entry::Ready { body, fetch_access, .. } => match *fetch_access {
                FetchAccess::None => {
                    *fetch_access = FetchAccess::Streamed;
                    Ok(body.clone())
                }
                FetchAccess::Get | FetchAccess::Streamed => Err(ResponseBodyError::AccessConflict),
            },
        })
    }

    /// Compatibility name for embedders that already use the backing-transfer
    /// API. The operation no longer consumes ordinary canonical readers.
    pub fn take(&mut self, request_id: &str) -> Option<Result<ResponseBody, ResponseBodyError>> {
        self.take_for_fetch(request_id)
    }

    pub fn fetch_access(&self, request_id: &str) -> Option<Result<FetchAccess, ResponseBodyError>> {
        let Some(entry) = self.entries.get(request_id) else {
            return self.failure.clone().map(Err);
        };
        Some(match &*entry.lock().unwrap_or_else(|e| e.into_inner()) {
            Entry::Ready { fetch_access, .. } => Ok(*fetch_access),
        })
    }

    pub fn is_fetch_streamed(&self, request_id: &str) -> Option<Result<bool, ResponseBodyError>> {
        self.fetch_access(request_id).map(|access| {
            access.map(|access| access == FetchAccess::Streamed)
        })
    }

    pub fn alias(&mut self, from: &str, to: &str) -> Result<(), ResponseBodyError> {
        if from == to || self.entries.contains_key(to) { return Ok(()); }
        let Some(entry) = self.entries.get(from).cloned() else {
            return self.failure.clone().map_or(Ok(()), Err);
        };
        self.entries.insert(to.to_string(), entry);
        Ok(())
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.total_bytes = 0;
        self.entry_count = 0;
        self.failure = None;
    }
}

impl Drop for ResponseBodyStore {
    fn drop(&mut self) { self.clear(); }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> ResponseBodyLimits {
        ResponseBodyLimits { memory_threshold: 2 * 1024 * 1024, total_bytes: 16 * 1024 * 1024, entries: 4 }
    }

    fn spool_path(body: &ResponseBody) -> std::path::PathBuf {
        match &*body.storage.lock().unwrap() {
            Storage::File(file) => file.path().to_owned(),
            Storage::Memory(_) => panic!("expected file-backed body"),
        }
    }

    #[test]
    fn response_body_large_text_and_binary_spool_exactly() {
        for (bytes, binary) in [(vec![b'x'; 2 * 1024 * 1024 + 31], false), ((0..2 * 1024 * 1024 + 31).map(|i| (i % 256) as u8).collect(), true)] {
            let mut store = ResponseBodyStore::new(limits());
            store.insert("request".into(), &bytes, binary).unwrap();
            let (body, actual_binary) = store.get("request").unwrap().unwrap();
            assert_eq!(actual_binary, binary);
            assert!(spool_path(&body).exists());
            assert_eq!(body.with_bytes(|b| b.to_vec()).unwrap(), bytes);
            assert_eq!(body.read(2 * 1024 * 1024 - 5, 99).unwrap(), bytes[2 * 1024 * 1024 - 5..]);
        }
    }

    #[test]
    fn response_body_small_memory_and_invalid_utf8_are_exact() {
        let mut store = ResponseBodyStore::new(limits());
        store.insert("text".into(), b"hello", false).unwrap();
        store.insert("legacy".into(), &[0xff, 0xe9, 0], false).unwrap();
        let (body, _) = store.get("text").unwrap().unwrap();
        assert!(matches!(*body.storage.lock().unwrap(), Storage::Memory(_)));
        assert_eq!(body.read(0, 99).unwrap(), b"hello");
        assert_eq!(store.get("legacy").unwrap().unwrap().0.read(0, 99).unwrap(), [0xff, 0xe9, 0]);
    }

    #[test]
    fn response_body_clear_and_drop_remove_retained_spools() {
        let mut store = ResponseBodyStore::new(ResponseBodyLimits { memory_threshold: 0, ..limits() });
        store.insert("clear".into(), b"clear", false).unwrap();
        let clear_path = spool_path(&store.get("clear").unwrap().unwrap().0);
        assert!(clear_path.exists());
        store.clear();
        assert!(!clear_path.exists());
        assert!(store.get("clear").is_none());
        store.insert("drop".into(), b"drop", false).unwrap();
        let drop_path = spool_path(&store.get("drop").unwrap().unwrap().0);
        drop(store);
        assert!(!drop_path.exists());
    }

    #[test]
    fn response_body_fetch_stream_alias_is_once_but_canonical_reads_remain() {
        let mut store = ResponseBodyStore::new(ResponseBodyLimits { memory_threshold: 0, entries: 1, ..limits() });
        store.insert("request".into(), b"stream", false).unwrap();
        store.alias("request", "loader").unwrap();
        assert_eq!(store.total_bytes, 6);
        assert_eq!(store.entry_count, 1);
        let body = store.take_for_fetch("loader").unwrap().unwrap();
        let path = spool_path(&body);
        assert_eq!(store.fetch_access("request").unwrap().unwrap(), FetchAccess::Streamed);
        assert!(store.is_fetch_streamed("loader").unwrap().unwrap());
        assert!(matches!(store.take_for_fetch("request"), Some(Err(ResponseBodyError::AccessConflict))));
        assert!(matches!(store.get_for_fetch("loader"), Some(Err(ResponseBodyError::AccessConflict))));
        assert_eq!(store.get("request").unwrap().unwrap().0.read(0, 99).unwrap(), b"stream");
        assert_eq!(store.get("loader").unwrap().unwrap().0.read(0, 99).unwrap(), b"stream");
        store.clear();
        drop(store);
        assert!(path.exists());
        assert_eq!(body.read(0, 99).unwrap(), b"stream");
        drop(body);
        assert!(!path.exists());
    }

    #[test]
    fn response_body_duplicate_id_replaces_shared_entry_without_double_accounting() {
        let mut store = ResponseBodyStore::new(ResponseBodyLimits { entries: 1, total_bytes: 8, ..limits() });
        store.insert("request".into(), b"old", false).unwrap();
        store.alias("request", "loader").unwrap();
        store.insert("request".into(), b"longer", false).unwrap();
        assert_eq!(store.entry_count, 1);
        assert_eq!(store.total_bytes, 6);
        assert_eq!(store.get("loader").unwrap().unwrap().0.read(0, 99).unwrap(), b"longer");
        store.insert("loader".into(), b"new", false).unwrap();
        assert_eq!(store.entry_count, 1);
        assert_eq!(store.total_bytes, 3);
        assert_eq!(store.take_for_fetch("request").unwrap().unwrap().read(0, 99).unwrap(), b"new");
        assert_eq!(store.get("loader").unwrap().unwrap().0.read(0, 99).unwrap(), b"new");
        assert!(matches!(store.take_for_fetch("loader"), Some(Err(ResponseBodyError::AccessConflict))));
    }

    #[test]
    fn response_body_budget_failures_are_explicit_and_sticky_until_clear() {
        for configured in [ResponseBodyLimits { total_bytes: 4, ..limits() }, ResponseBodyLimits { entries: 1, ..limits() }] {
            let mut store = ResponseBodyStore::new(configured);
            assert!(store.get("unknown").is_none());
            store.insert("first".into(), b"1234", false).unwrap();
            let error = store.insert("rejected".into(), b"x", false).unwrap_err();
            assert!(error.to_string().contains("response_body_budget_exhausted"));
            assert!(matches!(store.get("rejected"), Some(Err(ResponseBodyError::BudgetExceeded { .. }))));
            assert!(matches!(store.take_for_fetch("rejected"), Some(Err(ResponseBodyError::BudgetExceeded { .. }))));
            assert_eq!(store.get("first").unwrap().unwrap().0.read(0, 10).unwrap(), b"1234");
            for index in 0..1000 { assert!(store.insert(index.to_string(), b"", false).is_err()); }
            assert_eq!(store.entries.len(), 1, "failure metadata must not grow with requests");
            store.clear();
            store.insert("new".into(), b"ok", false).unwrap();
        }
    }
}
