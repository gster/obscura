//! Bounds native Worker queues, including nested workers, outside V8's heap.
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

const MAX_BYTES: usize = 64 * 1024 * 1024;
const MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;
const MAX_MESSAGES: usize = 4096;
const MAX_WORKERS: usize = 32;

#[derive(Default)]
pub(crate) struct Resources(Mutex<Usage>);
#[derive(Default)]
struct Usage { bytes: usize, messages: usize, workers: usize }

impl Resources {
    pub(crate) fn worker(self: &Arc<Self>) -> Result<WorkerLease, &'static str> {
        let mut usage = self.0.lock().unwrap();
        if usage.workers >= MAX_WORKERS { return Err("Worker count limit exceeded"); }
        usage.workers += 1;
        Ok(WorkerLease(self.clone()))
    }
    #[cfg(test)]
    pub(crate) fn active_workers(&self) -> usize { self.0.lock().unwrap().workers }
}

pub(crate) struct WorkerLease(Arc<Resources>);
impl Drop for WorkerLease {
    fn drop(&mut self) { self.0.0.lock().unwrap().workers -= 1; }
}

struct Reservation { resources: Arc<Resources>, bytes: usize }
impl Drop for Reservation {
    fn drop(&mut self) {
        let mut usage = self.resources.0.lock().unwrap();
        usage.bytes -= self.bytes;
        usage.messages -= 1;
    }
}

pub(crate) trait Size { fn queued_bytes(&self) -> usize; }
pub(crate) struct Queued<T> { value: Option<T>, _reservation: Option<Reservation> }
impl<T> Queued<T> {
    pub(crate) fn into_inner(mut self) -> T { self.value.take().unwrap() }
}
pub(crate) struct Sender<T> { tx: mpsc::UnboundedSender<Queued<T>>, resources: Arc<Resources> }
pub(crate) type Receiver<T> = mpsc::UnboundedReceiver<Queued<T>>;
pub(crate) fn channel<T>(resources: Arc<Resources>) -> (Sender<T>, Receiver<T>) {
    let (tx, rx) = mpsc::unbounded_channel();
    (Sender { tx, resources }, rx)
}
impl<T: Size> Sender<T> {
    pub(crate) fn send(&self, value: T) -> Result<(), &'static str> {
        let bytes = value.queued_bytes();
        let mut usage = self.resources.0.lock().unwrap();
        if bytes > MAX_MESSAGE_BYTES || bytes > MAX_BYTES.saturating_sub(usage.bytes) || usage.messages >= MAX_MESSAGES {
            return Err("Worker message queue capacity exceeded");
        }
        usage.bytes += bytes;
        usage.messages += 1;
        drop(usage);
        let reservation = Reservation { resources: self.resources.clone(), bytes };
        self.tx.send(Queued { value: Some(value), _reservation: Some(reservation) })
            .map_err(|_| "Worker is no longer running")
    }
    // At most one terminal notification per worker, even when the data queue
    // is full. It carries no user payload and cannot recursively fill a queue.
    pub(crate) fn terminal(&self, value: T) {
        let _ = self.tx.send(Queued { value: Some(value), _reservation: None });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Payload(usize);
    impl Size for Payload { fn queued_bytes(&self) -> usize { self.0 } }

    #[test]
    fn worker_queues_share_capacity_and_release_on_receive_or_drop() {
        let resources = Arc::new(Resources::default());
        let (first, mut rx) = channel(resources.clone());
        let (second, other) = channel(resources.clone());
        for _ in 0..4 { first.send(Payload(MAX_MESSAGE_BYTES)).unwrap(); }
        assert!(second.send(Payload(1)).is_err());
        let _ = rx.try_recv().unwrap().into_inner();
        second.send(Payload(MAX_MESSAGE_BYTES)).unwrap();
        drop(other);
        assert!(second.send(Payload(1)).is_err());
        drop(rx);
        assert_eq!(resources.0.lock().unwrap().bytes, 0);
        assert_eq!(resources.0.lock().unwrap().messages, 0);
    }

    #[test]
    fn worker_queue_bounds_small_messages_and_worker_leases() {
        let resources = Arc::new(Resources::default());
        let (tx, rx) = channel(resources.clone());
        for _ in 0..MAX_MESSAGES { tx.send(Payload(0)).unwrap(); }
        assert!(tx.send(Payload(0)).is_err());
        drop(rx);
        let leases: Vec<_> = (0..MAX_WORKERS).map(|_| resources.worker().unwrap()).collect();
        assert!(resources.worker().is_err());
        drop(leases);
        assert_eq!(resources.active_workers(), 0);
    }
}
