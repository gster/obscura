//! Bounded inbound messages for one CDP connection.
//!
//! Reservations stay live while a message is queued, executing, or deferred by
//! a navigation. This bounds the complete `ServerMessage` ownership path rather
//! than merely the websocket reader's channel.

use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;

pub const DEFAULT_MAX_MESSAGES: usize = 1024;
pub const DEFAULT_MAX_BYTES: usize = 128 * 1024 * 1024;
pub const DEFAULT_MAX_MESSAGE_BYTES: usize = 64 * 1024 * 1024;
pub const DEFAULT_MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseReason {
    Count,
    Bytes,
    MessageBytes,
    ConnectionClosed,
}

pub trait MessageSize {
    fn message_bytes(&self) -> usize;
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
}

pub struct Sender<T> {
    tx: mpsc::UnboundedSender<Envelope<T>>,
    inner: Arc<Inner>,
}

pub struct Receiver<T> {
    rx: mpsc::UnboundedReceiver<Envelope<T>>,
    inner: Arc<Inner>,
}

pub struct Envelope<T> {
    message: Option<T>,
    _reservation: Option<Reservation>,
}

pub(crate) struct Reservation {
    inner: Arc<Inner>,
    bytes: usize,
}

pub struct SendError<T> {
    _message: T,
    pub reason: CloseReason,
}

impl<T> std::fmt::Debug for SendError<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SendError")
            .field("reason", &self.reason)
            .finish_non_exhaustive()
    }
}

pub fn channel<T: MessageSize>() -> (Sender<T>, Receiver<T>) {
    channel_with_limits(
        DEFAULT_MAX_MESSAGES,
        DEFAULT_MAX_BYTES,
        DEFAULT_MAX_MESSAGE_BYTES,
    )
}

pub fn channel_with_limits<T: MessageSize>(
    max_messages: usize,
    max_bytes: usize,
    max_message_bytes: usize,
) -> (Sender<T>, Receiver<T>) {
    let (tx, rx) = mpsc::unbounded_channel();
    let inner = Arc::new(Inner {
        state: Mutex::new(State {
            messages: 0,
            bytes: 0,
            closed: None,
        }),
        max_messages,
        max_bytes,
        max_message_bytes,
    });
    (
        Sender {
            tx,
            inner: inner.clone(),
        },
        Receiver { rx, inner },
    )
}

impl<T: MessageSize> Sender<T> {
    pub fn send(&self, message: T) -> Result<(), SendError<T>> {
        let bytes = message.message_bytes();
        let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(reason) = state.closed {
            return Err(SendError {
                _message: message,
                reason,
            });
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
            return Err(SendError {
                _message: message,
                reason,
            });
        }
        state.messages += 1;
        state.bytes += bytes;
        drop(state);

        let envelope = Envelope {
            message: Some(message),
            _reservation: Some(Reservation {
                inner: self.inner.clone(),
                bytes,
            }),
        };
        if let Err(envelope) = self.tx.send(envelope) {
            let message = envelope.0.into_parts().0;
            self.inner.close(CloseReason::ConnectionClosed);
            return Err(SendError {
                _message: message,
                reason: CloseReason::ConnectionClosed,
            });
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn usage(&self) -> (usize, usize) {
        let state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
        (state.messages, state.bytes)
    }
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
            inner: self.inner.clone(),
        }
    }
}

impl<T> Receiver<T> {
    pub async fn recv(&mut self) -> Option<Envelope<T>> {
        self.rx.recv().await
    }

    #[cfg(test)]
    pub fn try_recv(&mut self) -> Result<Envelope<T>, mpsc::error::TryRecvError> {
        self.rx.try_recv()
    }
}

impl<T> Envelope<T> {
    pub fn get(&self) -> &T {
        self.message
            .as_ref()
            .expect("inbound envelope already taken")
    }

    pub(crate) fn into_parts(mut self) -> (T, Reservation) {
        (
            self.message.take().expect("inbound envelope already taken"),
            self._reservation
                .take()
                .expect("inbound reservation already taken"),
        )
    }
}

impl Inner {
    fn close(&self, reason: CloseReason) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.closed.is_none() {
            state.closed = Some(reason);
        }
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
        state.messages = state.messages.saturating_sub(1);
        state.bytes = state.bytes.saturating_sub(self.bytes);
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        self.inner.close(CloseReason::ConnectionClosed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct SizedMessage(&'static str);

    impl MessageSize for SizedMessage {
        fn message_bytes(&self) -> usize {
            self.0.len()
        }
    }

    #[test]
    fn count_limit_is_sticky() {
        let (sender, mut receiver) = channel_with_limits(1, 100, 100);
        sender.send(SizedMessage("a")).unwrap();
        let error = sender.send(SizedMessage("b")).unwrap_err();
        assert_eq!(error.reason, CloseReason::Count);
        drop(receiver.try_recv().unwrap());
        assert_eq!(
            sender.send(SizedMessage("c")).unwrap_err().reason,
            CloseReason::Count
        );
    }

    #[test]
    fn byte_and_message_limits_are_distinct() {
        let (sender, _receiver) = channel_with_limits(4, 3, 2);
        sender.send(SizedMessage("ab")).unwrap();
        assert_eq!(
            sender.send(SizedMessage("cd")).unwrap_err().reason,
            CloseReason::Bytes
        );

        let (sender, _receiver) = channel_with_limits(4, 10, 2);
        assert_eq!(
            sender.send(SizedMessage("abc")).unwrap_err().reason,
            CloseReason::MessageBytes
        );
    }

    #[test]
    fn reservation_follows_dequeued_envelope_until_drop() {
        let (sender, mut receiver) = channel_with_limits(1, 3, 3);
        sender.send(SizedMessage("abc")).unwrap();
        let envelope = receiver.try_recv().unwrap();
        assert_eq!(sender.usage(), (1, 3));
        drop(envelope);
        assert_eq!(sender.usage(), (0, 0));
    }

    #[test]
    fn reservation_follows_message_until_processing_finishes() {
        let (sender, mut receiver) = channel_with_limits(1, 3, 3);
        sender.send(SizedMessage("abc")).unwrap();
        let (message, reservation) = receiver.try_recv().unwrap().into_parts();
        assert_eq!(message.0, "abc");
        assert_eq!(sender.usage(), (1, 3));
        drop(message);
        assert_eq!(sender.usage(), (1, 3));
        drop(reservation);
        assert_eq!(sender.usage(), (0, 0));
    }

    #[test]
    fn receiver_drop_is_explicit() {
        let (sender, receiver) = channel_with_limits(1, 3, 3);
        drop(receiver);
        assert_eq!(
            sender.send(SizedMessage("a")).unwrap_err().reason,
            CloseReason::ConnectionClosed
        );
    }
}
