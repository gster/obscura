use std::collections::HashMap;

use base64::Engine as _;
use obscura_net::response_body::ResponseBody;
use serde_json::{json, Value};

use crate::dispatch::CdpContext;

// Default chunk size when the client does not pass `size`. Chrome uses a similar
// order of magnitude; keeping chunks bounded is the point of streaming (issue
// #360), so we never return the whole body in one IO.read.
const DEFAULT_CHUNK: usize = 1 << 20; // 1 MiB
const MAX_READ_CHUNK: usize = 4 << 20; // 4 MiB

fn io_stream_max_entries() -> usize {
    std::env::var("OBSCURA_IO_STREAM_MAX_ENTRIES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(32)
}

fn io_stream_max_bytes() -> usize {
    std::env::var("OBSCURA_IO_STREAM_MAX_BYTES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(256 * 1024 * 1024)
}

/// Context-owned raw response streams. Active handles are never evicted: new
/// streams fail explicitly when the context entry/byte budget is exhausted.
/// File-backed bodies stay on disk and are released on close/context drop.
pub struct IoStreamStore {
    streams: HashMap<String, (ResponseBody, usize)>,
    total_bytes: usize,
    counter: u64,
    max_entries: usize,
    max_bytes: usize,
}

/// Holds exclusive access until the body is taken from its Page. Dropping a
/// reservation without committing changes neither capacity nor handle state.
pub(crate) struct IoStreamReservation<'a> {
    store: &'a mut IoStreamStore,
    handle: String,
    next_counter: u64,
    total_bytes: usize,
}

impl IoStreamReservation<'_> {
    pub(crate) fn commit(self, body: ResponseBody) -> String {
        self.store.counter = self.next_counter;
        self.store.total_bytes = self.total_bytes;
        self.store.streams.insert(self.handle.clone(), (body, 0));
        self.handle
    }
}

impl Default for IoStreamStore {
    fn default() -> Self {
        Self::with_limits(io_stream_max_entries(), io_stream_max_bytes())
    }
}

impl IoStreamStore {
    pub(crate) fn with_limits(max_entries: usize, max_bytes: usize) -> Self {
        Self {
            streams: HashMap::new(),
            total_bytes: 0,
            counter: 0,
            max_entries,
            max_bytes,
        }
    }

    /// Check before taking a Page body so admission failure does not consume it.
    pub fn ensure_capacity(&self, len: usize) -> Result<(), String> {
        let error = if self.streams.len() >= self.max_entries {
            Some(format!("io_stream_budget_exhausted: entries limit {}", self.max_entries))
        } else if self.total_bytes.checked_add(len).is_none_or(|total| total > self.max_bytes) {
            Some(format!("io_stream_budget_exhausted: adding {len} bytes to {} exceeds {}-byte per-context limit", self.total_bytes, self.max_bytes))
        } else if self.counter == u64::MAX {
            Some("IO stream handle space exhausted".to_string())
        } else { None };
        if let Some(error) = error {
            tracing::warn!(reason = %error, "Response body stream rejected");
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn reserve(&mut self, len: usize) -> Result<IoStreamReservation<'_>, String> {
        self.ensure_capacity(len)?;
        Ok(IoStreamReservation {
            handle: format!("stream-{}", self.counter),
            next_counter: self.counter + 1,
            total_bytes: self.total_bytes + len,
            store: self,
        })
    }

    pub fn insert(&mut self, bytes: impl Into<ResponseBody>) -> Result<String, String> {
        let bytes = bytes.into();
        Ok(self.reserve(bytes.len())?.commit(bytes))
    }

    #[cfg(test)]
    pub(crate) fn set_handle_counter(&mut self, counter: u64) {
        self.counter = counter;
    }

    /// Read up to `size` bytes from the stream, advancing its cursor. Returns
    /// the base64 chunk and whether EOF was reached, or None for an unknown or
    /// already-freed handle.
    pub fn read(
        &mut self,
        handle: &str,
        offset: Option<usize>,
        size: usize,
    ) -> Option<(String, bool)> {
        self.read_result(handle, offset, size)?.ok()
    }

    /// CDP retains I/O failures so a failed spool read is not an unknown handle.
    pub fn read_result(
        &mut self,
        handle: &str,
        offset: Option<usize>,
        size: usize,
    ) -> Option<Result<(String, bool), String>> {
        let (bytes, cursor) = self.streams.get_mut(handle)?;
        if let Some(offset) = offset {
            *cursor = offset.min(bytes.len());
        }
        let size = size.min(MAX_READ_CHUNK);
        let start = (*cursor).min(bytes.len());
        let end = start.saturating_add(size).min(bytes.len());
        let data = match bytes.read(start, end - start) {
            Ok(chunk) => base64::engine::general_purpose::STANDARD.encode(chunk),
            Err(error) => return Some(Err(error.to_string())),
        };
        *cursor = end;
        Some(Ok((data, end >= bytes.len())))
    }

    /// Free a stream's buffer (IO.close). A no-op for an unknown handle.
    pub fn remove(&mut self, handle: &str) {
        if let Some((b, _)) = self.streams.remove(handle) {
            self.total_bytes -= b.len();
        }
    }
}

/// CDP IO domain. Streams a response body handed out by
/// Fetch.takeResponseBodyAsStream: IO.read returns the next base64 chunk and
/// IO.close frees the buffer. Nothing here runs unless a client opened a stream.
pub async fn handle(method: &str, params: &Value, ctx: &mut CdpContext) -> Result<Value, String> {
    match method {
        "read" => {
            let handle = params
                .get("handle")
                .and_then(|v| v.as_str())
                .ok_or("IO.read requires handle")?;
            let size = params
                .get("size")
                .map(|value| {
                    value
                        .as_i64()
                        .filter(|size| *size >= 0)
                        .and_then(|size| usize::try_from(size).ok())
                        .ok_or("IO.read size must be a non-negative integer")
                })
                .transpose()?
                .unwrap_or(DEFAULT_CHUNK);
            let offset = params
                .get("offset")
                .map(|value| {
                    value
                        .as_i64()
                        .filter(|offset| *offset >= 0)
                        .and_then(|offset| usize::try_from(offset).ok())
                        .ok_or("IO.read offset must be a non-negative integer")
                })
                .transpose()?;

            let (data, eof) = ctx
                .io_streams
                .read_result(handle, offset, size)
                .ok_or_else(|| format!("IO.read: unknown handle {handle}"))??;

            Ok(json!({ "data": data, "eof": eof, "base64Encoded": true }))
        }
        "close" => {
            let handle = params
                .get("handle")
                .and_then(|v| v.as_str())
                .ok_or("IO.close requires handle")?;
            ctx.io_streams.remove(handle);
            Ok(json!({}))
        }
        _ => Err(format!("Unknown IO method: {}", method)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(s: &str) -> Vec<u8> {
        base64::engine::general_purpose::STANDARD.decode(s).unwrap()
    }

    #[test]
    fn response_body_unused_reservation_does_not_change_capacity_or_handle() {
        let mut store = IoStreamStore::with_limits(1, 4);
        drop(store.reserve(4).unwrap());
        assert_eq!(store.total_bytes, 0);
        assert_eq!(store.insert(vec![1; 4]).unwrap(), "stream-0");
    }

    #[test]
    fn reads_chunks_then_frees() {
        let mut store = IoStreamStore::with_limits(4, 1024);
        let h = store.insert(b"hello".to_vec()).unwrap();

        let (d1, eof1) = store.read(&h, None, 3).unwrap();
        assert_eq!(decode(&d1), b"hel");
        assert!(!eof1);

        let (d2, eof2) = store.read(&h, None, 3).unwrap();
        assert_eq!(decode(&d2), b"lo");
        assert!(eof2);

        store.remove(&h);
        assert!(store.read(&h, None, 3).is_none());
    }

    #[test]
    fn read_offset_seeks_and_zero_size_does_not_advance() {
        let mut store = IoStreamStore::with_limits(2, 1024);
        let handle = store.insert(b"abcdef".to_vec()).unwrap();

        let (empty, eof) = store.read(&handle, Some(1), 0).unwrap();
        assert_eq!(decode(&empty), b"");
        assert!(!eof);
        let (middle, eof) = store.read(&handle, None, 2).unwrap();
        assert_eq!(decode(&middle), b"bc");
        assert!(!eof);
        let (tail, eof) = store.read(&handle, Some(4), 10).unwrap();
        assert_eq!(decode(&tail), b"ef");
        assert!(eof);
    }

    #[tokio::test]
    async fn read_rejects_negative_or_non_integer_ranges() {
        let mut ctx = CdpContext::new();
        let handle_id = ctx.io_streams.insert(b"data".to_vec()).unwrap();
        for params in [
            json!({"handle": handle_id.clone(), "offset": -1}),
            json!({"handle": handle_id.clone(), "size": -1}),
            json!({"handle": handle_id, "offset": 1.5}),
        ] {
            assert!(handle("read", &params, &mut ctx).await.is_err(), "{params}");
        }
    }

    #[test]
    fn entry_budget_rejects_new_stream_and_keeps_active_handles() {
        let mut store = IoStreamStore::with_limits(1, 1024);
        let first = store.insert(vec![1]).unwrap();
        let error = store.insert(vec![2]).unwrap_err();
        assert!(error.contains("io_stream_budget_exhausted"));
        assert_eq!(decode(&store.read(&first, None, 10).unwrap().0), vec![1]);
        store.remove(&first);
        assert!(store.insert(vec![2]).is_ok());
    }

    #[test]
    fn byte_budget_rejects_new_stream_and_keeps_active_handles() {
        let mut store = IoStreamStore::with_limits(4, 10);
        let first = store.insert(vec![1u8; 8]).unwrap();
        assert!(store.insert(vec![2u8; 8]).unwrap_err().contains("io_stream_budget_exhausted"));
        assert!(store.insert(vec![2u8; 100]).is_err());
        assert_eq!(decode(&store.read(&first, None, 100).unwrap().0), vec![1; 8]);
    }

    #[test]
    fn requested_read_size_is_capped() {
        let mut store = IoStreamStore::with_limits(2, MAX_READ_CHUNK * 2);
        let handle = store.insert(vec![7u8; MAX_READ_CHUNK + 17]).unwrap();
        let (first, eof) = store.read_result(&handle, None, usize::MAX).unwrap().unwrap();
        assert_eq!(decode(&first).len(), MAX_READ_CHUNK);
        assert!(!eof);
        let (second, eof) = store.read_result(&handle, None, usize::MAX).unwrap().unwrap();
        assert_eq!(decode(&second).len(), 17);
        assert!(eof);
    }
}
