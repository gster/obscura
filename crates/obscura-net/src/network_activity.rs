use std::sync::{Arc, Mutex};

use tokio::sync::Notify;

/// A coherent view of page-scoped network activity for one document.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NetworkActivitySnapshot {
    pub generation: u64,
    pub active: u32,
    pub epoch: u64,
    pub last_above_zero_epoch: u64,
    pub last_above_two_epoch: u64,
    pub below_zero_since: Option<std::time::Instant>,
    pub below_two_since: Option<std::time::Instant>,
}

#[derive(Default)]
struct NetworkActivityState {
    snapshot: NetworkActivitySnapshot,
}

/// Page-scoped network activity shared by the page, its frames, and workers.
///
/// `begin_document` advances the document generation and clears its active
/// count. Guards retain the generation in which they began, so a late terminal
/// event from an old document cannot alter the current document's snapshot.
pub struct NetworkActivityTracker {
    state: Mutex<NetworkActivityState>,
    notify: Arc<Notify>,
}

impl Default for NetworkActivityTracker {
    fn default() -> Self {
        Self {
            state: Mutex::new(NetworkActivityState::default()),
            notify: Arc::new(Notify::new()),
        }
    }
}

impl NetworkActivityTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start a fresh document generation and return its identity.
    pub fn begin_document(&self) -> u64 {
        let now = std::time::Instant::now();
        let generation = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            let snapshot = &mut state.snapshot;
            snapshot.generation = snapshot.generation.saturating_add(1);
            snapshot.active = 0;
            snapshot.epoch = snapshot.epoch.saturating_add(1);
            snapshot.below_zero_since = Some(now);
            snapshot.below_two_since = Some(now);
            snapshot.generation
        };
        self.wake_waiters();
        generation
    }

    /// Begin one activity slot. Dropping the returned guard finishes it.
    pub fn begin(self: &Arc<Self>) -> NetworkActivityGuard {
        self.begin_inner(None)
    }

    /// Begin an activity owned by a specific document generation. A producer
    /// retained by an old runtime receives an inert guard after the page has
    /// committed a successor document, so a late start cannot pollute that
    /// document's quiet window.
    pub fn begin_for_generation(self: &Arc<Self>, generation: u64) -> NetworkActivityGuard {
        self.begin_inner(Some(generation))
    }

    fn begin_inner(self: &Arc<Self>, expected_generation: Option<u64>) -> NetworkActivityGuard {
        let generation = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            let snapshot = &mut state.snapshot;
            if expected_generation.is_some_and(|expected| expected != snapshot.generation) {
                return NetworkActivityGuard {
                    tracker: self.clone(),
                    generation: expected_generation.unwrap(),
                    finished: true,
                };
            }
            snapshot.active = snapshot.active.saturating_add(1);
            snapshot.epoch = snapshot.epoch.saturating_add(1);
            if snapshot.active > 0 {
                snapshot.last_above_zero_epoch = snapshot.epoch;
                snapshot.below_zero_since = None;
            }
            if snapshot.active > 2 {
                snapshot.last_above_two_epoch = snapshot.epoch;
                snapshot.below_two_since = None;
            }
            snapshot.generation
        };
        self.wake_waiters();
        NetworkActivityGuard {
            tracker: self.clone(),
            generation,
            finished: false,
        }
    }

    pub fn snapshot(&self) -> NetworkActivitySnapshot {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .snapshot
    }

    pub fn notify(&self) -> Arc<Notify> {
        self.notify.clone()
    }

    fn finish(&self, generation: u64) {
        let now = std::time::Instant::now();
        let changed = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            let snapshot = &mut state.snapshot;
            if snapshot.generation == generation {
                snapshot.active = snapshot.active.saturating_sub(1);
                snapshot.epoch = snapshot.epoch.saturating_add(1);
                if snapshot.active == 0 {
                    snapshot.below_zero_since = Some(now);
                }
                if snapshot.active == 2 {
                    snapshot.below_two_since = Some(now);
                }
                true
            } else {
                false
            }
        };
        if changed {
            self.wake_waiters();
        }
    }

    fn wake_waiters(&self) {
        // `notify_waiters` broadcasts to simultaneous Page/CDP observers but
        // deliberately stores no permit. Pair it with `notify_one` so an
        // activity transition in the small subscribe/recheck race is still
        // observed by the next waiter. The extra permit only causes a harmless
        // re-check when broadcast waiters were already present.
        self.notify.notify_waiters();
        self.notify.notify_one();
    }
}

/// RAII completion token returned by `NetworkActivityTracker::begin`.
pub struct NetworkActivityGuard {
    tracker: Arc<NetworkActivityTracker>,
    generation: u64,
    finished: bool,
}

impl NetworkActivityGuard {
    pub fn finish(mut self) {
        self.finish_once();
    }

    fn finish_once(&mut self) {
        if !self.finished {
            self.finished = true;
            self.tracker.finish(self.generation);
        }
    }
}

impl Drop for NetworkActivityGuard {
    fn drop(&mut self) {
        self.finish_once();
    }
}

#[cfg(test)]
mod tests {
    use super::NetworkActivityTracker;
    use std::sync::Arc;

    #[test]
    fn tracks_zero_one_two_and_three_active_slots() {
        let tracker = Arc::new(NetworkActivityTracker::new());
        assert_eq!(tracker.snapshot().active, 0);
        let first = tracker.begin();
        assert_eq!(tracker.snapshot().active, 1);
        let second = tracker.begin();
        assert_eq!(tracker.snapshot().active, 2);
        let third = tracker.begin();
        let above_two = tracker.snapshot();
        assert_eq!(above_two.active, 3);
        assert!(above_two.last_above_two_epoch > 0);
        drop(third);
        assert_eq!(tracker.snapshot().active, 2);
        drop(second);
        assert_eq!(tracker.snapshot().active, 1);
        drop(first);
        assert_eq!(tracker.snapshot().active, 0);
    }

    #[test]
    fn fast_activity_is_visible_in_threshold_epochs_after_completion() {
        let tracker = Arc::new(NetworkActivityTracker::new());
        let before = tracker.snapshot();
        tracker.begin().finish();
        let after = tracker.snapshot();
        assert_eq!(after.active, 0);
        assert!(after.epoch > before.epoch);
        assert!(after.last_above_zero_epoch > before.last_above_zero_epoch);
    }

    #[test]
    fn old_generation_completion_does_not_change_current_snapshot() {
        let tracker = Arc::new(NetworkActivityTracker::new());
        let old = tracker.begin();
        let generation = tracker.begin_document();
        let before = tracker.snapshot();
        old.finish();
        assert_eq!(tracker.snapshot(), before);
        assert_eq!(before.generation, generation);
        assert_eq!(before.active, 0);
    }

    #[test]
    fn old_generation_start_is_inert_for_the_current_document() {
        let tracker = Arc::new(NetworkActivityTracker::new());
        let old_generation = tracker.begin_document();
        let current_generation = tracker.begin_document();
        let before = tracker.snapshot();

        let stale = tracker.begin_for_generation(old_generation);
        assert_eq!(tracker.snapshot(), before);
        stale.finish();
        assert_eq!(tracker.snapshot(), before);
        assert_eq!(before.generation, current_generation);
    }

    #[test]
    fn quiet_timestamps_begin_at_real_threshold_transitions() {
        let tracker = Arc::new(NetworkActivityTracker::new());
        tracker.begin_document();
        let initially_quiet = tracker.snapshot();
        assert!(initially_quiet.below_zero_since.is_some());
        assert!(initially_quiet.below_two_since.is_some());

        let first = tracker.begin();
        assert!(tracker.snapshot().below_zero_since.is_none());
        let second = tracker.begin();
        assert!(tracker.snapshot().below_two_since.is_some());
        let third = tracker.begin();
        assert!(tracker.snapshot().below_two_since.is_none());

        drop(third);
        let below_two = tracker.snapshot();
        assert!(below_two.below_two_since.is_some());
        assert!(below_two.below_zero_since.is_none());
        drop(second);
        assert!(tracker.snapshot().below_zero_since.is_none());
        drop(first);
        assert!(tracker.snapshot().below_zero_since.is_some());
    }

    #[tokio::test]
    async fn start_and_finish_both_notify_waiters() {
        let tracker = Arc::new(NetworkActivityTracker::new());
        let notify = tracker.notify();
        let started = notify.notified();
        tokio::pin!(started);
        started.as_mut().enable();
        let guard = tracker.begin();
        started.await;

        let finished = notify.notified();
        tokio::pin!(finished);
        finished.as_mut().enable();
        guard.finish();
        finished.await;
    }
}
