//! Host-only accepted input accounting. No JS admission or producer operation.
//! This candidate qualifies discrete input for a stable top-level Window only.
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_OWNER: AtomicU64 = AtomicU64::new(1);
pub type OwnerId = u64;
pub type EntryId = u64;

#[derive(Clone, Default)]
pub struct InputHub(Arc<Mutex<State>>);
#[derive(Default)]
struct State {
    next: EntryId,
    closed: bool,
    owners: HashMap<OwnerId, (bool, bool)>, // live, navigation intent/transition
    entries: HashMap<EntryId, Option<OwnerId>>,
}
pub struct Transaction<'a>(MutexGuard<'a, State>);
pub struct InputTicket { hub: InputHub, id: EntryId, armed: bool }

impl InputHub {
    pub fn transaction(&self) -> Transaction<'_> {
        Transaction(self.0.lock().unwrap_or_else(|error| error.into_inner()))
    }
    pub fn ticket(&self, tx: &mut Transaction<'_>, owner: Option<OwnerId>) -> InputTicket {
        tx.0.next = tx.0.next.checked_add(1).expect("input sequence exhausted");
        let id = tx.0.next;
        if !tx.0.closed { tx.0.entries.insert(id, owner); }
        InputTicket { hub: self.clone(), id, armed: true }
    }
    pub fn close(&self) {
        let mut tx = self.transaction(); tx.0.closed = true; tx.0.entries.clear();
    }
    fn pending(&self, owner: OwnerId) -> bool {
        let tx = self.transaction();
        !tx.0.closed && tx.0.owners.get(&owner) == Some(&(true, false))
            && tx.0.entries.values().any(|value| *value == Some(owner))
    }
}
impl Transaction<'_> {
    pub fn promote(&mut self, id: EntryId, owner: OwnerId) {
        if self.0.owners.get(&owner).is_some_and(|(live, _)| *live) {
            if let Some(slot @ None) = self.0.entries.get_mut(&id) { *slot = Some(owner); }
        }
    }
    pub fn contains(&self, id: EntryId) -> bool { self.0.entries.contains_key(&id) }
    pub fn cancel(&mut self, id: EntryId) { self.0.entries.remove(&id); }
}
impl InputTicket {
    pub fn id(&self) -> EntryId { self.id }
    /// Only use while holding this ticket's hub transaction; Drop stays outside.
    pub fn cancel_locked(&mut self, tx: &mut Transaction<'_>) {
        tx.cancel(self.id); self.armed = false;
    }
    pub fn begin_dispatch(mut self, owner: OwnerId) -> bool {
        let result = {
            let mut tx = self.hub.transaction();
            let accepted = tx.0.entries.remove(&self.id) == Some(Some(owner));
            accepted && !tx.0.closed && tx.0.owners.get(&owner) == Some(&(true, false))
        };
        self.armed = false;
        result
    }
}
impl Drop for InputTicket {
    fn drop(&mut self) {
        if self.armed { self.hub.transaction().cancel(self.id); }
    }
}

struct OwnerState { live: bool, paused: bool, hub: Option<InputHub> }
#[derive(Clone)]
pub struct InputOwner { id: OwnerId, state: Arc<Mutex<OwnerState>> }
impl Default for InputOwner {
    fn default() -> Self {
        Self { id: NEXT_OWNER.fetch_add(1, Ordering::Relaxed), state: Arc::new(Mutex::new(
            OwnerState { live: true, paused: false, hub: None })) }
    }
}
impl InputOwner {
    pub fn id(&self) -> OwnerId { self.id }
    /// A Window may be associated once with its connection, never rebound.
    pub fn bind(&self, hub: &InputHub) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if !state.live { return false; }
        if let Some(existing) = &state.hub { return Arc::ptr_eq(&existing.0, &hub.0); }
        {
            let mut tx = hub.transaction();
            if tx.0.closed { return false; }
            tx.0.owners.insert(self.id, (state.live, state.paused));
        }
        state.hub = Some(hub.clone()); true
    }
    pub fn navigation_pending(&self, paused: bool) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.paused = paused;
        if let Some(hub) = &state.hub {
            let mut tx = hub.transaction();
            if state.live && !tx.0.closed { tx.0.owners.insert(self.id, (true, paused)); }
            else { tx.0.owners.remove(&self.id); }
        }
    }
    pub fn retire(&self) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.live = false;
        if let Some(hub) = &state.hub {
            let mut tx = hub.transaction();
            tx.0.owners.remove(&self.id);
            tx.0.entries.retain(|_, owner| *owner != Some(self.id));
        }
    }
    pub fn pending(&self) -> bool {
        let state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.live && state.hub.as_ref().is_some_and(|hub| hub.pending(self.id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ticket_ends_before_handler_and_second_remains() {
        let hub = InputHub::default(); let owner = InputOwner::default(); owner.bind(&hub);
        let (first, second) = { let mut tx = hub.transaction();
            (hub.ticket(&mut tx, Some(owner.id())), hub.ticket(&mut tx, Some(owner.id()))) };
        assert!(owner.pending()); assert!(first.begin_dispatch(owner.id())); assert!(owner.pending());
        assert!(second.begin_dispatch(owner.id())); assert!(!owner.pending());
    }
    #[test]
    fn failed_publication_disarms_before_drop_and_owner_cannot_migrate() {
        let hub = InputHub::default(); let owner = InputOwner::default(); owner.bind(&hub);
        let mut tx = hub.transaction(); let mut ticket = hub.ticket(&mut tx, Some(owner.id()));
        ticket.cancel_locked(&mut tx); drop(tx); drop(ticket); assert!(!owner.pending());
        let ticket = { let mut tx = hub.transaction(); hub.ticket(&mut tx, Some(owner.id())) };
        owner.retire(); assert!(!owner.pending()); assert!(!ticket.begin_dispatch(owner.id()));
        assert!(!owner.bind(&InputHub::default()));
    }
    #[test]
    fn navigation_intent_is_not_owner_retirement() {
        let hub = InputHub::default(); let owner = InputOwner::default(); owner.bind(&hub);
        let ticket = { let mut tx = hub.transaction(); hub.ticket(&mut tx, Some(owner.id())) };
        owner.navigation_pending(true); assert!(!owner.pending());
        owner.navigation_pending(false); assert!(owner.pending());
        drop(ticket); assert!(!owner.pending());
    }
    #[test]
    fn retired_owner_churn_does_not_retain_history_or_revive_saved_handles() {
        let hub = InputHub::default(); let mut saved = Vec::new();
        for _ in 0..1024 {
            let owner = InputOwner::default(); assert!(owner.bind(&hub));
            let ticket = { let mut tx = hub.transaction(); hub.ticket(&mut tx, Some(owner.id())) };
            assert!(owner.pending()); owner.retire();
            assert!(!ticket.begin_dispatch(owner.id()));
            owner.navigation_pending(false);
            assert!(!owner.bind(&hub)); assert!(!owner.pending()); saved.push(owner);
        }
        let tx = hub.transaction(); assert!(tx.0.owners.is_empty()); assert!(tx.0.entries.is_empty());
        drop(tx); assert!(saved.iter().all(|owner| !owner.pending()));
    }

    #[test]
    fn query_cannot_observe_a_rolled_back_publication() {
        let hub = InputHub::default(); let owner = InputOwner::default(); assert!(owner.bind(&hub));
        let mut tx = hub.transaction(); let mut ticket = hub.ticket(&mut tx, Some(owner.id()));
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (result_tx, result_rx) = std::sync::mpsc::channel();
        let observer = std::thread::spawn(move || {
            started_tx.send(()).unwrap(); result_tx.send(owner.pending()).unwrap();
        });
        started_rx.recv().unwrap();
        assert!(matches!(result_rx.recv_timeout(std::time::Duration::from_millis(20)), Err(std::sync::mpsc::RecvTimeoutError::Timeout)));
        ticket.cancel_locked(&mut tx); drop(tx); drop(ticket);
        assert!(!result_rx.recv_timeout(std::time::Duration::from_secs(1)).unwrap()); observer.join().unwrap();
    }

}
