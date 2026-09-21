//! Bounded staging for CDP events produced during one processor turn.
//!
//! The byte budget is the exact UTF-8 length of each complete serialized CDP
//! event envelope. It is not a bound on `serde_json::Value` capacity or total
//! process RSS. Admission is atomic for batches and the first failure is sticky.

use std::io;
use std::ops::{Index, RangeFull};

use crate::types::CdpEvent;

pub const DEFAULT_MAX_EVENTS: usize = crate::outbound::DEFAULT_MAX_MESSAGES;
pub const DEFAULT_MAX_BYTES: usize = crate::outbound::DEFAULT_MAX_BYTES;
pub const DEFAULT_MAX_EVENT_BYTES: usize = crate::outbound::DEFAULT_MAX_MESSAGE_BYTES;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseReason {
    Count,
    Bytes,
    EventBytes,
    Serialization,
}

pub struct PendingEvents {
    events: Vec<CdpEvent>,
    event_bytes: Vec<usize>,
    bytes: usize,
    max_events: usize,
    max_bytes: usize,
    max_event_bytes: usize,
    closed: Option<CloseReason>,
    close_handle: Option<crate::outbound::OutboundSender>,
}

impl Default for PendingEvents {
    fn default() -> Self {
        Self::with_limits(
            DEFAULT_MAX_EVENTS,
            DEFAULT_MAX_BYTES,
            DEFAULT_MAX_EVENT_BYTES,
        )
    }
}

impl PendingEvents {
    pub fn with_limits(max_events: usize, max_bytes: usize, max_event_bytes: usize) -> Self {
        Self {
            events: Vec::new(),
            event_bytes: Vec::new(),
            bytes: 0,
            max_events,
            max_bytes,
            max_event_bytes,
            closed: None,
            close_handle: None,
        }
    }

    pub(crate) fn bind_close_handle(&mut self, close_handle: crate::outbound::OutboundSender) {
        if let Some(reason) = self.closed {
            close_handle.close(outbound_close_reason(reason));
        }
        self.close_handle = Some(close_handle);
    }

    pub fn push(&mut self, event: CdpEvent) {
        self.extend(std::iter::once(event));
    }

    pub fn extend<I>(&mut self, events: I)
    where
        I: IntoIterator<Item = CdpEvent>,
    {
        if self.closed.is_some() {
            return;
        }

        let staged: Vec<CdpEvent> = events.into_iter().collect();
        let Some(total_events) = self.events.len().checked_add(staged.len()) else {
            self.fail(CloseReason::Count);
            return;
        };
        if total_events > self.max_events {
            self.fail(CloseReason::Count);
            return;
        }

        let mut staged_bytes = Vec::with_capacity(staged.len());
        let mut batch_bytes = 0usize;
        for event in &staged {
            let Ok(bytes) = serialized_len(event) else {
                self.fail(CloseReason::Serialization);
                return;
            };
            if bytes > self.max_event_bytes {
                self.fail(CloseReason::EventBytes);
                return;
            }
            let Some(next) = batch_bytes.checked_add(bytes) else {
                self.fail(CloseReason::Bytes);
                return;
            };
            batch_bytes = next;
            staged_bytes.push(bytes);
        }
        if batch_bytes > self.max_bytes.saturating_sub(self.bytes) {
            self.fail(CloseReason::Bytes);
            return;
        }

        self.bytes += batch_bytes;
        self.events.extend(staged);
        self.event_bytes.extend(staged_bytes);
    }

    pub fn close_reason(&self) -> Option<CloseReason> {
        self.closed
    }

    fn fail(&mut self, reason: CloseReason) {
        if self.closed.is_some() {
            return;
        }
        self.closed = Some(reason);
        if let Some(close_handle) = &self.close_handle {
            close_handle.close(outbound_close_reason(reason));
        }
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    pub fn iter(&self) -> std::slice::Iter<'_, CdpEvent> {
        self.events.iter()
    }

    pub fn last(&self) -> Option<&CdpEvent> {
        self.events.last()
    }

    pub fn clear(&mut self) {
        self.events.clear();
        self.event_bytes.clear();
        self.bytes = 0;
    }

    pub fn truncate(&mut self, len: usize) {
        self.events.truncate(len);
        self.event_bytes.truncate(len);
        self.bytes = self.event_bytes.iter().copied().sum();
    }

    pub fn drain(&mut self, range: RangeFull) -> std::vec::Drain<'_, CdpEvent> {
        let _ = range;
        self.event_bytes.clear();
        self.bytes = 0;
        self.events.drain(..)
    }

    #[cfg(test)]
    pub(crate) fn usage(&self) -> (usize, usize) {
        (self.events.len(), self.bytes)
    }
}

impl<I> Index<I> for PendingEvents
where
    Vec<CdpEvent>: Index<I>,
{
    type Output = <Vec<CdpEvent> as Index<I>>::Output;

    fn index(&self, index: I) -> &Self::Output {
        &self.events[index]
    }
}

impl<'a> IntoIterator for &'a PendingEvents {
    type Item = &'a CdpEvent;
    type IntoIter = std::slice::Iter<'a, CdpEvent>;

    fn into_iter(self) -> Self::IntoIter {
        self.events.iter()
    }
}

#[derive(Default)]
struct CountingWriter {
    bytes: usize,
}

impl io::Write for CountingWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.bytes = self.bytes.checked_add(buf.len()).ok_or_else(|| {
            io::Error::new(io::ErrorKind::Other, "serialized event size overflow")
        })?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn serialized_len(event: &CdpEvent) -> Result<usize, serde_json::Error> {
    let mut writer = CountingWriter::default();
    serde_json::to_writer(&mut writer, event)?;
    Ok(writer.bytes)
}

fn outbound_close_reason(reason: CloseReason) -> crate::outbound::CloseReason {
    match reason {
        CloseReason::Count => crate::outbound::CloseReason::PendingEventsCount,
        CloseReason::Bytes => crate::outbound::CloseReason::PendingEventsBytes,
        CloseReason::EventBytes => crate::outbound::CloseReason::PendingEventBytes,
        CloseReason::Serialization => crate::outbound::CloseReason::PendingEventSerialization,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn event(value: &str) -> CdpEvent {
        CdpEvent::new("Runtime.consoleAPICalled", json!({"value": value}))
    }

    #[test]
    fn exact_serialized_bytes_are_accounted() {
        let item = event("é");
        let expected = serde_json::to_string(&item).unwrap().len();
        let mut pending = PendingEvents::with_limits(2, expected, expected);
        pending.push(item);
        assert_eq!(pending.usage(), (1, expected));
    }

    #[test]
    fn batch_admission_is_atomic_and_sticky() {
        let existing = event("existing");
        let existing_bytes = serde_json::to_string(&existing).unwrap().len();
        let one = event("one");
        let one_bytes = serde_json::to_string(&one).unwrap().len();
        let mut pending = PendingEvents::with_limits(3, existing_bytes + one_bytes * 2 - 1, 1000);
        pending.push(existing);
        pending.extend(vec![one, event("two")]);
        assert_eq!(pending.usage(), (1, existing_bytes));
        assert_eq!(pending[0].params["value"], "existing");
        assert_eq!(pending.close_reason(), Some(CloseReason::Bytes));
        pending.push(event("later"));
        assert_eq!(pending.len(), 1);
    }

    #[test]
    fn count_and_single_event_failures_are_distinct() {
        let mut pending = PendingEvents::with_limits(0, 1000, 1000);
        pending.push(event("x"));
        assert_eq!(pending.close_reason(), Some(CloseReason::Count));

        let mut pending = PendingEvents::with_limits(2, 1000, 1);
        pending.push(event("x"));
        assert_eq!(pending.close_reason(), Some(CloseReason::EventBytes));
    }

    #[test]
    fn truncate_and_drain_release_accounting() {
        let mut pending = PendingEvents::with_limits(3, 1000, 1000);
        pending.extend(vec![event("one"), event("two")]);
        let first_bytes = serde_json::to_string(&pending[0]).unwrap().len();
        pending.truncate(1);
        assert_eq!(pending.usage(), (1, first_bytes));
        let drained: Vec<_> = pending.drain(..).collect();
        assert_eq!(drained.len(), 1);
        assert_eq!(pending.usage(), (0, 0));
    }

    #[test]
    fn first_failure_closes_bound_outbound_and_clear_is_not_recovery() {
        let (outbound, _receiver, mut closed) = crate::outbound::channel();
        let mut pending = PendingEvents::with_limits(0, 1000, 1000);
        pending.bind_close_handle(outbound.clone());
        pending.push(event("complete raw value"));
        assert!(*closed.borrow_and_update());
        assert_eq!(
            outbound.close_reason(),
            Some(crate::outbound::CloseReason::PendingEventsCount)
        );
        pending.clear();
        pending.push(event("must not be admitted after failure"));
        assert!(pending.is_empty());
        assert_eq!(pending.close_reason(), Some(CloseReason::Count));
    }

    #[test]
    fn binding_after_failure_still_closes_connection() {
        let mut pending = PendingEvents::with_limits(0, 1000, 1000);
        pending.push(event("x"));
        let (outbound, _receiver, _) = crate::outbound::channel();
        pending.bind_close_handle(outbound.clone());
        assert_eq!(
            outbound.close_reason(),
            Some(crate::outbound::CloseReason::PendingEventsCount)
        );
    }
}
