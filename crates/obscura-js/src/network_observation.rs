//! Page-wide admission for scripted network observations.
//!
//! A Page and all of its Dedicated Workers share one reservation budget. Once
//! a batch is admitted its records may move through Worker, teardown, and Page
//! queues without being admitted again. The first capacity or serialization
//! failure is sticky: already admitted records stay intact, later producers
//! fail explicitly, and consumers receive the same terminal failure.

use std::io;
use std::ops::Index;
use std::sync::{Arc, Mutex};

use crate::ops::JsNetworkEvent;

pub const DEFAULT_MAX_EVENTS: usize = 4096;
pub const DEFAULT_MAX_BYTES: usize = 64 * 1024 * 1024;
pub const DEFAULT_MAX_EVENT_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum NetworkObservationFailureKind {
    Count,
    Bytes,
    EventBytes,
    Serialization,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkObservationFailure {
    pub kind: NetworkObservationFailureKind,
    pub message: String,
}

impl std::fmt::Display for NetworkObservationFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for NetworkObservationFailure {}

#[derive(Debug)]
struct Usage {
    events: usize,
    bytes: usize,
    failure: Option<NetworkObservationFailure>,
}

#[derive(Debug)]
struct Budget {
    usage: Mutex<Usage>,
    max_events: usize,
    max_bytes: usize,
    max_event_bytes: usize,
}

impl Budget {
    fn new(max_events: usize, max_bytes: usize, max_event_bytes: usize) -> Arc<Self> {
        Arc::new(Self {
            usage: Mutex::new(Usage { events: 0, bytes: 0, failure: None }),
            max_events,
            max_bytes,
            max_event_bytes,
        })
    }

    fn failure(&self) -> Option<NetworkObservationFailure> {
        self.usage.lock().unwrap_or_else(|error| error.into_inner()).failure.clone()
    }

    fn terminal_failure(&self) -> Option<NetworkObservationFailure> {
        let usage = self.usage.lock().unwrap_or_else(|error| error.into_inner());
        (usage.events == 0).then(|| usage.failure.clone()).flatten()
    }

    fn fail(&self, kind: NetworkObservationFailureKind) -> NetworkObservationFailure {
        let mut usage = self.usage.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(failure) = &usage.failure {
            return failure.clone();
        }
        let message = match kind {
            NetworkObservationFailureKind::Count => {
                format!("Network observation count exceeded {}", self.max_events)
            }
            NetworkObservationFailureKind::Bytes => {
                format!("Network observation bytes exceeded {}", self.max_bytes)
            }
            NetworkObservationFailureKind::EventBytes => {
                format!("Network observation event exceeded {} bytes", self.max_event_bytes)
            }
            NetworkObservationFailureKind::Serialization => {
                "Network observation serialization failed".to_string()
            }
        };
        let failure = NetworkObservationFailure { kind, message };
        usage.failure = Some(failure.clone());
        failure
    }

    fn reserve(self: &Arc<Self>, lengths: &[usize]) -> Result<Vec<Reservation>, NetworkObservationFailure> {
        let mut usage = self.usage.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(failure) = &usage.failure {
            return Err(failure.clone());
        }
        if lengths.iter().any(|length| *length > self.max_event_bytes) {
            drop(usage);
            return Err(self.fail(NetworkObservationFailureKind::EventBytes));
        }
        let Some(batch_bytes) = lengths.iter().try_fold(0usize, |total, length| total.checked_add(*length)) else {
            drop(usage);
            return Err(self.fail(NetworkObservationFailureKind::Bytes));
        };
        let Some(total_events) = usage.events.checked_add(lengths.len()) else {
            drop(usage);
            return Err(self.fail(NetworkObservationFailureKind::Count));
        };
        if total_events > self.max_events {
            drop(usage);
            return Err(self.fail(NetworkObservationFailureKind::Count));
        }
        if batch_bytes > self.max_bytes.saturating_sub(usage.bytes) {
            drop(usage);
            return Err(self.fail(NetworkObservationFailureKind::Bytes));
        }
        usage.events = total_events;
        usage.bytes += batch_bytes;
        drop(usage);
        Ok(lengths.iter().map(|bytes| Reservation { budget: self.clone(), bytes: *bytes }).collect())
    }
}

#[derive(Debug)]
struct Reservation {
    budget: Arc<Budget>,
    bytes: usize,
}

impl Drop for Reservation {
    fn drop(&mut self) {
        let mut usage = self.budget.usage.lock().unwrap_or_else(|error| error.into_inner());
        usage.events = usage.events.saturating_sub(1);
        usage.bytes = usage.bytes.saturating_sub(self.bytes);
    }
}

#[derive(Debug)]
pub(crate) struct NetworkObservationRecord {
    event: JsNetworkEvent,
    _reservation: Reservation,
}

impl NetworkObservationRecord {
    pub(crate) fn event(&self) -> &JsNetworkEvent { &self.event }
    fn into_event(self) -> JsNetworkEvent { self.event }
}

#[derive(Debug)]
pub struct NetworkObservationDrain {
    pub events: Vec<JsNetworkEvent>,
    pub failure: Option<NetworkObservationFailure>,
}

impl std::ops::Deref for NetworkObservationDrain {
    type Target = Vec<JsNetworkEvent>;

    fn deref(&self) -> &Self::Target { &self.events }
}

impl std::ops::DerefMut for NetworkObservationDrain {
    fn deref_mut(&mut self) -> &mut Self::Target { &mut self.events }
}

impl IntoIterator for NetworkObservationDrain {
    type Item = JsNetworkEvent;
    type IntoIter = std::vec::IntoIter<JsNetworkEvent>;

    fn into_iter(self) -> Self::IntoIter { self.events.into_iter() }
}

impl<'a> IntoIterator for &'a NetworkObservationDrain {
    type Item = &'a JsNetworkEvent;
    type IntoIter = std::slice::Iter<'a, JsNetworkEvent>;

    fn into_iter(self) -> Self::IntoIter { self.events.iter() }
}

impl<'a> IntoIterator for &'a mut NetworkObservationDrain {
    type Item = &'a mut JsNetworkEvent;
    type IntoIter = std::slice::IterMut<'a, JsNetworkEvent>;

    fn into_iter(self) -> Self::IntoIter { self.events.iter_mut() }
}

#[derive(Debug)]
pub struct NetworkObservationQueue {
    records: Vec<NetworkObservationRecord>,
    budget: Arc<Budget>,
}

impl Default for NetworkObservationQueue {
    fn default() -> Self {
        Self::with_limits(DEFAULT_MAX_EVENTS, DEFAULT_MAX_BYTES, DEFAULT_MAX_EVENT_BYTES)
    }
}

impl NetworkObservationQueue {
    pub fn with_limits(max_events: usize, max_bytes: usize, max_event_bytes: usize) -> Self {
        Self { records: Vec::new(), budget: Budget::new(max_events, max_bytes, max_event_bytes) }
    }

    pub fn sibling(&self) -> Self {
        Self { records: Vec::new(), budget: self.budget.clone() }
    }

    pub fn len(&self) -> usize { self.records.len() }
    pub fn is_empty(&self) -> bool { self.records.is_empty() }
    pub fn failure(&self) -> Option<NetworkObservationFailure> { self.budget.failure() }

    /// Return the sticky failure only after every previously admitted record
    /// has left all sibling queues. Consumers emit this after the accepted
    /// prefix; producers use `failure()` to fail fast immediately.
    pub fn terminal_failure(&self) -> Option<NetworkObservationFailure> {
        self.budget.terminal_failure()
    }

    pub fn try_push(&mut self, event: JsNetworkEvent) -> Result<(), NetworkObservationFailure> {
        self.try_extend(std::iter::once(event))
    }

    pub fn try_extend<I>(&mut self, events: I) -> Result<(), NetworkObservationFailure>
    where
        I: IntoIterator<Item = JsNetworkEvent>,
    {
        if let Some(failure) = self.failure() {
            return Err(failure);
        }
        let staged: Vec<JsNetworkEvent> = events.into_iter().collect();
        if staged.is_empty() {
            return Ok(());
        }
        let mut lengths = Vec::with_capacity(staged.len());
        for event in &staged {
            match serialized_len(event) {
                Ok(length) => lengths.push(length),
                Err(_) => return Err(self.budget.fail(NetworkObservationFailureKind::Serialization)),
            }
        }
        let reservations = self.budget.reserve(&lengths)?;
        self.records.extend(staged.into_iter().zip(reservations).map(
            |(event, reservation)| NetworkObservationRecord {
                event,
                _reservation: reservation,
            },
        ));
        Ok(())
    }

    pub(crate) fn take_records(&mut self) -> Vec<NetworkObservationRecord> {
        std::mem::take(&mut self.records)
    }

    pub(crate) fn append_records(&mut self, records: Vec<NetworkObservationRecord>) {
        debug_assert!(records.iter().all(|record| Arc::ptr_eq(&record._reservation.budget, &self.budget)));
        self.records.extend(records);
    }

    pub(crate) fn drain_records(&self, records: Vec<NetworkObservationRecord>) -> NetworkObservationDrain {
        let events = records.into_iter().map(NetworkObservationRecord::into_event).collect();
        NetworkObservationDrain {
            events,
            failure: self.terminal_failure(),
        }
    }

    pub fn drain(&mut self) -> NetworkObservationDrain {
        let records = self.take_records();
        self.drain_records(records)
    }

    #[cfg(test)]
    pub(crate) fn usage(&self) -> (usize, usize) {
        let usage = self.budget.usage.lock().unwrap_or_else(|error| error.into_inner());
        (usage.events, usage.bytes)
    }
}

impl Index<usize> for NetworkObservationQueue {
    type Output = JsNetworkEvent;

    fn index(&self, index: usize) -> &Self::Output {
        self.records[index].event()
    }
}

fn serialized_len(event: &JsNetworkEvent) -> Result<usize, serde_json::Error> {
    let mut writer = CountingWriter(0);
    serde_json::to_writer(&mut writer, event)?;
    Ok(writer.0)
}

struct CountingWriter(usize);

impl io::Write for CountingWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0 = self.0.checked_add(bytes.len())
            .ok_or_else(|| io::Error::new(io::ErrorKind::OutOfMemory, "serialized observation length overflow"))?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> { Ok(()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn event(request_id: &str) -> JsNetworkEvent {
        JsNetworkEvent {
            document_generation: 9,
            document_url: "https://example.test/document".into(),
            initiator_request_id: Some("preflight-parent".into()),
            pending: false,
            error: None,
            request_id: request_id.into(),
            url: format!("https://example.test/{request_id}"),
            method: "POST".into(),
            resource_type: obscura_net::ResourceType::Fetch,
            status: 200,
            status_text: "OK".into(),
            response_headers: HashMap::from([
                ("set-cookie".into(), "second=2".into()),
                ("authorization".into(), "Bearer full-value".into()),
            ]),
            raw_headers: Some(raw_headers("transportResponse")),
            request_raw_headers: Some(raw_headers("transportRequest")),
            request_body_size: 256,
            request_started: true,
            redirect: false,
            response_body_request_id: Some(request_id.into()),
            body_size: 256,
            timestamp: 123.5,
        }
    }

    fn raw_headers(stage: &'static str) -> obscura_net::HeaderCapture {
        obscura_net::HeaderCapture {
            capture_stage: stage,
            encoding: "base64",
            fields: vec![
                obscura_net::RawHeader { name: b"Cookie".to_vec(), value: b"first=1".to_vec() },
                obscura_net::RawHeader { name: b"Cookie".to_vec(), value: vec![0, 0xff, b'='] },
                obscura_net::RawHeader { name: b"Authorization".to_vec(), value: (0u8..=255).collect() },
            ],
        }
    }

    #[test]
    fn count_failure_keeps_the_atomic_accepted_prefix_and_stays_terminal() {
        let mut queue = NetworkObservationQueue::with_limits(2, usize::MAX, usize::MAX);
        queue.try_extend([event("one"), event("two")]).unwrap();

        let failure = queue.try_push(event("three")).unwrap_err();
        assert_eq!(failure.kind, NetworkObservationFailureKind::Count);
        assert_eq!(queue.len(), 2);

        let drained = queue.drain();
        assert_eq!(drained.events.iter().map(|event| event.request_id.as_str()).collect::<Vec<_>>(), ["one", "two"]);
        assert_eq!(drained.failure, Some(failure.clone()));
        assert_eq!(queue.try_push(event("later")).unwrap_err(), failure);
    }

    #[test]
    fn a_batch_is_admitted_all_or_none() {
        let mut queue = NetworkObservationQueue::with_limits(2, usize::MAX, usize::MAX);
        queue.try_push(event("accepted")).unwrap();
        let failure = queue.try_extend([event("rejected-a"), event("rejected-b")]).unwrap_err();

        assert_eq!(failure.kind, NetworkObservationFailureKind::Count);
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].request_id, "accepted");
    }

    #[test]
    fn byte_limits_use_full_serialized_records() {
        let sample = event("full-raw-fields");
        let length = serialized_len(&sample).unwrap();

        let mut exact = NetworkObservationQueue::with_limits(2, length, length);
        exact.try_push(sample).unwrap();
        let failure = exact.try_push(event("next")).unwrap_err();
        assert_eq!(failure.kind, NetworkObservationFailureKind::Bytes);

        let mut too_large = NetworkObservationQueue::with_limits(1, usize::MAX, length - 1);
        let failure = too_large.try_push(event("full-raw-fields")).unwrap_err();
        assert_eq!(failure.kind, NetworkObservationFailureKind::EventBytes);
    }

    #[test]
    fn moving_records_between_siblings_preserves_raw_bytes_and_reservations() {
        let mut source = NetworkObservationQueue::with_limits(2, usize::MAX, usize::MAX);
        let mut destination = source.sibling();
        source.try_push(event("raw")).unwrap();
        let usage = source.usage();

        destination.append_records(source.take_records());
        assert!(source.is_empty());
        assert_eq!(destination.usage(), usage);

        let drained = destination.drain();
        let captured = drained.events[0].request_raw_headers.as_ref().unwrap();
        assert_eq!(captured.fields.len(), 3);
        assert_eq!(captured.fields[1].value, vec![0, 0xff, b'=']);
        assert_eq!(captured.fields[2].value, (0u8..=255).collect::<Vec<_>>());
        assert_eq!(source.usage(), (0, 0));
    }

    #[test]
    fn terminal_failure_waits_for_records_in_every_sibling_queue() {
        let mut backlog = NetworkObservationQueue::with_limits(1, usize::MAX, usize::MAX);
        let mut producer = backlog.sibling();
        backlog.try_push(event("accepted-before-failure")).unwrap();
        let failure = producer.try_push(event("rejected")).unwrap_err();

        assert_eq!(producer.failure(), Some(failure.clone()));
        assert_eq!(producer.terminal_failure(), None);
        assert_eq!(producer.drain().failure, None);

        let accepted = backlog.drain();
        assert_eq!(accepted.events.len(), 1);
        assert_eq!(accepted.events[0].request_id, "accepted-before-failure");
        assert_eq!(accepted.failure, Some(failure));
    }

    #[test]
    fn state_drop_moves_the_last_accepted_records_to_teardown() {
        let teardown = Arc::new(Mutex::new(NetworkObservationQueue::with_limits(
            1, usize::MAX, usize::MAX,
        )));
        let mut state = crate::ops::ObscuraState::new(
            obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145),
        );
        state.js_network_events = teardown.lock().unwrap().sibling();
        state.network_teardown_events = teardown.clone();
        state.js_network_events.try_push(event("accepted-before-runtime-exit")).unwrap();
        let failure = {
            let mut producer = teardown.lock().unwrap().sibling();
            producer.try_push(event("rejected-before-runtime-exit")).unwrap_err()
        };

        drop(state);

        let drained = teardown.lock().unwrap().drain();
        assert_eq!(drained.events.len(), 1);
        assert_eq!(drained.events[0].request_id, "accepted-before-runtime-exit");
        assert_eq!(drained.failure, Some(failure));
    }
}
