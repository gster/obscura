//! Bounded outbound messages for one CDP connection.
//!
//! Capacity is reserved until the receiver drops the envelope, rather than
//! merely until dequeue. This keeps a slow websocket writer bounded even when
//! the processor has already handed messages to its send task.

use std::ops::Deref;
use std::sync::{Arc, Mutex};

use tokio::sync::{mpsc, watch, Notify};

pub const DEFAULT_MAX_MESSAGES: usize = 1024;
pub const DEFAULT_MAX_BYTES: usize = 128 * 1024 * 1024;
// Page.printToPDF permits a 64 MiB base64 result. Leave room for the CDP
// envelope while still rejecting a single response that can consume the
// entire connection budget. Oversized messages close the connection; callers
// should use an existing streaming CDP method where that domain provides one.
pub const DEFAULT_MAX_MESSAGE_BYTES: usize = 80 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseReason {
    Count,
    Bytes,
    MessageBytes,
    WriterIo,
    WriterTimeout,
    InboundCount,
    InboundBytes,
    InboundMessageBytes,
    InboundMessageType,
    PendingEventsCount,
    PendingEventsBytes,
    PendingEventBytes,
    PendingEventSerialization,
    ConnectionClosed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct State {
    messages: usize,
    bytes: usize,
    closed: Option<CloseReason>,
}

struct Inner {
    state: Mutex<State>,
    max_messages: usize,
    max_bytes: usize,
    max_message_bytes: usize,
    closed_tx: watch::Sender<bool>,
    capacity_changed: Notify,
}

pub struct OutboundSender {
    tx: mpsc::UnboundedSender<Envelope>,
    inner: Arc<Inner>,
}

pub struct OutboundReceiver {
    rx: mpsc::UnboundedReceiver<Envelope>,
    inner: Arc<Inner>,
}

pub struct Envelope {
    message: Option<String>,
    reservation: Option<Reservation>,
}

pub struct Reservation {
    inner: Arc<Inner>,
    bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendError<T>(pub T);

pub fn channel() -> (OutboundSender, OutboundReceiver, watch::Receiver<bool>) {
    channel_with_limits(DEFAULT_MAX_MESSAGES, DEFAULT_MAX_BYTES, DEFAULT_MAX_MESSAGE_BYTES)
}

pub fn channel_with_limits(
    max_messages: usize,
    max_bytes: usize,
    max_message_bytes: usize,
) -> (OutboundSender, OutboundReceiver, watch::Receiver<bool>) {
    let (tx, rx) = mpsc::unbounded_channel();
    let (closed_tx, closed_rx) = watch::channel(false);
    let inner = Arc::new(Inner {
        state: Mutex::new(State { messages: 0, bytes: 0, closed: None }),
        max_messages,
        max_bytes,
        max_message_bytes,
        closed_tx,
        capacity_changed: Notify::new(),
    });
    (
        OutboundSender { tx, inner: inner.clone() },
        OutboundReceiver { rx, inner },
        closed_rx,
    )
}

impl OutboundSender {
    pub fn send(&self, message: String) -> Result<(), SendError<String>> {
        let bytes = message.len();
        let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.closed.is_some() {
            return Err(SendError(message));
        }
        let reason = if bytes > self.inner.max_message_bytes {
            Some(CloseReason::MessageBytes)
        } else if state.messages >= self.inner.max_messages {
            Some(CloseReason::Count)
        } else if bytes > self.inner.max_bytes.saturating_sub(state.bytes) {
            Some(CloseReason::Bytes)
        } else {
            None
        };
        if let Some(reason) = reason {
            state.closed = Some(reason);
            drop(state);
            self.signal_closed();
            return Err(SendError(message));
        }
        state.messages += 1;
        state.bytes += bytes;
        drop(state);
        let envelope = Envelope {
            message: Some(message),
            reservation: Some(Reservation { inner: self.inner.clone(), bytes }),
        };
        if let Err(envelope) = self.tx.send(envelope) {
            let (message, reservation) = envelope.0.into_parts();
            self.close(CloseReason::ConnectionClosed);
            drop(reservation);
            return Err(SendError(message));
        }
        Ok(())
    }

    pub fn close(&self, reason: CloseReason) {
        self.inner.close(reason);
    }

    pub fn is_closed(&self) -> bool {
        self.inner.state.lock().unwrap_or_else(|e| e.into_inner()).closed.is_some()
    }

    pub fn close_reason(&self) -> Option<CloseReason> {
        self.inner.state.lock().unwrap_or_else(|e| e.into_inner()).closed
    }

    /// Wait until every accepted envelope has released its reservation. On the
    /// normal writer path that follows websocket send completion; writer
    /// failure or cancellation also releases reservations. Callers must impose
    /// their own deadline.
    pub async fn wait_empty(&self) {
        loop {
            let capacity_changed = self.inner.capacity_changed.notified();
            if self.inner.state.lock().unwrap_or_else(|e| e.into_inner()).messages == 0 {
                return;
            }
            capacity_changed.await;
        }
    }

    #[cfg(test)]
    pub(crate) fn usage(&self) -> (usize, usize) {
        let state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
        (state.messages, state.bytes)
    }

    fn signal_closed(&self) {
        let _ = self.inner.closed_tx.send(true);
    }
}

impl Inner {
    fn close(&self, reason: CloseReason) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.closed.is_some() { return; }
        state.closed = Some(reason);
        drop(state);
        let _ = self.closed_tx.send(true);
    }
}

impl Clone for OutboundSender {
    fn clone(&self) -> Self { Self { tx: self.tx.clone(), inner: self.inner.clone() } }
}

impl OutboundReceiver {
    pub async fn recv(&mut self) -> Option<Envelope> { self.rx.recv().await }

    #[cfg(test)]
    pub fn try_recv(&mut self) -> Result<Envelope, mpsc::error::TryRecvError> { self.rx.try_recv() }
}

impl Envelope {
    pub fn as_str(&self) -> &str { self.message.as_deref().expect("outbound envelope already taken") }

    pub fn into_parts(mut self) -> (String, Reservation) {
        (
            self.message.take().expect("outbound envelope already taken"),
            self.reservation.take().expect("outbound reservation already taken"),
        )
    }
}

impl Deref for Envelope {
    type Target = str;

    fn deref(&self) -> &Self::Target { self.as_str() }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
        state.messages = state.messages.saturating_sub(1);
        state.bytes = state.bytes.saturating_sub(self.bytes);
        drop(state);
        self.inner.capacity_changed.notify_one();
    }
}

impl Drop for OutboundReceiver {
    fn drop(&mut self) {
        self.inner.close(CloseReason::ConnectionClosed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(max_messages: usize, max_bytes: usize, max_message_bytes: usize) -> (OutboundSender, OutboundReceiver) {
        let (sender, receiver, _) = channel_with_limits(max_messages, max_bytes, max_message_bytes);
        (sender, receiver)
    }

    #[test]
    fn count_limit_is_sticky() {
        let (sender, mut receiver) = one(1, 100, 100);
        sender.send("a".into()).unwrap();
        assert!(sender.send("b".into()).is_err());
        assert_eq!(sender.close_reason(), Some(CloseReason::Count));
        assert!(sender.send("c".into()).is_err());
        drop(receiver.try_recv().unwrap());
        assert!(sender.send("d".into()).is_err());
    }

    #[test]
    fn bytes_limit_and_multibyte_use_utf8_bytes() {
        let (sender, _receiver) = one(10, 2, 10);
        sender.send("é".into()).unwrap();
        assert!(sender.send("a".into()).is_err());
        assert_eq!(sender.close_reason(), Some(CloseReason::Bytes));
    }

    #[test]
    fn single_message_limit_is_distinct() {
        let (sender, _receiver) = one(10, 100, 2);
        assert!(sender.send("abc".into()).is_err());
        assert_eq!(sender.close_reason(), Some(CloseReason::MessageBytes));
    }

    #[test]
    fn dequeue_does_not_release_reservation_until_envelope_drop() {
        let (sender, mut receiver) = one(1, 3, 3);
        sender.send("abc".into()).unwrap();
        let envelope = receiver.try_recv().unwrap();
        assert!(sender.send("x".into()).is_err());
        drop(envelope);
        let (sender, mut receiver) = one(1, 3, 3);
        sender.send("abc".into()).unwrap();
        let envelope = receiver.try_recv().unwrap();
        drop(envelope);
        assert!(sender.send("x".into()).is_ok());
    }

    #[test]
    fn fifo_and_closed_signal() {
        let (sender, mut receiver, mut closed) = channel_with_limits(3, 100, 100);
        sender.send("one".into()).unwrap();
        sender.send("two".into()).unwrap();
        let (one, one_reservation) = receiver.try_recv().unwrap().into_parts();
        assert_eq!(one, "one");
        drop(one_reservation);
        let (two, two_reservation) = receiver.try_recv().unwrap().into_parts();
        assert_eq!(two, "two");
        drop(two_reservation);
        assert!(!*closed.borrow());
        assert!(sender.send("x".repeat(101)).is_err());
        assert!(*closed.borrow_and_update());
        assert!(sender.is_closed());
    }

    #[test]
    fn receiver_drop_closes_sender() {
        let (sender, receiver) = one(2, 20, 20);
        drop(receiver);
        assert!(sender.send("x".into()).is_err());
        assert_eq!(sender.close_reason(), Some(CloseReason::ConnectionClosed));
    }

    #[test]
    fn moving_text_keeps_reservation_until_explicit_drop() {
        let (sender, mut receiver) = one(1, 3, 3);
        sender.send("abc".into()).unwrap();
        let (message, reservation) = receiver.try_recv().unwrap().into_parts();
        assert_eq!(message, "abc");
        assert_eq!(sender.usage(), (1, 3));
        assert!(sender.send("x".into()).is_err());
        drop(reservation);
        assert_eq!(sender.usage(), (0, 0));
    }

    #[tokio::test]
    async fn wait_empty_includes_dequeued_in_flight_envelope() {
        let (sender, mut receiver) = one(1, 3, 3);
        sender.send("abc".into()).unwrap();
        let (_, reservation) = receiver.try_recv().unwrap().into_parts();
        let waiting = sender.wait_empty();
        tokio::pin!(waiting);
        assert!(tokio::time::timeout(std::time::Duration::from_millis(10), &mut waiting).await.is_err());
        drop(reservation);
        tokio::time::timeout(std::time::Duration::from_secs(1), waiting)
            .await
            .expect("reservation drop should wake bounded flush");
    }
}
