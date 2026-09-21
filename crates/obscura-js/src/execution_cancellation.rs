//! Sticky cancellation for all V8 execution owned by one browser connection.
//!
//! A connection may own several page and worker isolates. Each runtime attaches
//! one slot and marks it active only while V8 is actually entered. Disconnect
//! cancellation is sticky: active isolates are terminated immediately, and an
//! isolate which races into V8 after the disconnect observes the closed flag
//! and terminates itself before running page code.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};

use crate::runtime::IsolateHandle;

struct ExecutionSlot {
    handle: IsolateHandle,
    active: AtomicUsize,
}

struct Registry {
    closed: AtomicBool,
    slots: Mutex<Vec<Weak<ExecutionSlot>>>,
}

/// Cancellation source shared by every V8 runtime owned by one connection.
#[derive(Clone)]
pub struct ExecutionCancellation {
    registry: Arc<Registry>,
}

impl Default for ExecutionCancellation {
    fn default() -> Self {
        Self {
            registry: Arc::new(Registry {
                closed: AtomicBool::new(false),
                slots: Mutex::new(Vec::new()),
            }),
        }
    }
}

impl ExecutionCancellation {
    /// Permanently cancel this connection and interrupt every isolate currently
    /// executing page or worker JavaScript.
    pub fn cancel(&self) {
        self.registry.closed.store(true, Ordering::SeqCst);
        let mut slots = self
            .registry
            .slots
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        slots.retain(|slot| {
            let Some(slot) = slot.upgrade() else {
                return false;
            };
            if slot.active.load(Ordering::SeqCst) != 0 {
                slot.handle.terminate_execution();
            }
            true
        });
    }

    pub fn is_cancelled(&self) -> bool {
        self.registry.closed.load(Ordering::SeqCst)
    }

    pub(crate) fn attach(&self, handle: IsolateHandle) -> ExecutionTracker {
        let slot = Arc::new(ExecutionSlot {
            handle,
            active: AtomicUsize::new(0),
        });
        let mut slots = self
            .registry
            .slots
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        slots.retain(|existing| existing.strong_count() != 0);
        slots.push(Arc::downgrade(&slot));
        drop(slots);
        ExecutionTracker {
            cancellation: self.clone(),
            slot,
        }
    }

    #[cfg(test)]
    fn slot_count(&self) -> usize {
        self.registry
            .slots
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len()
    }
}

#[derive(Clone)]
pub(crate) struct ExecutionTracker {
    cancellation: ExecutionCancellation,
    slot: Arc<ExecutionSlot>,
}

impl ExecutionTracker {
    pub(crate) fn enter(&self) -> ExecutionRegistration {
        self.slot.active.fetch_add(1, Ordering::SeqCst);
        // Recheck after publishing active. If cancel scanned this slot before
        // the increment, its preceding closed store is visible here.
        if self.cancellation.is_cancelled() {
            self.slot.handle.terminate_execution();
        }
        ExecutionRegistration {
            slot: self.slot.clone(),
        }
    }

    pub(crate) fn clear_termination(&self) {
        self.slot.handle.cancel_terminate_execution();
        // A deadline watchdog may race connection teardown. Clearing its
        // termination must never revive execution after the connection closed.
        if self.cancellation.is_cancelled() {
            self.slot.handle.terminate_execution();
        }
    }

}

pub(crate) struct ExecutionRegistration {
    slot: Arc<ExecutionSlot>,
}

impl Drop for ExecutionRegistration {
    fn drop(&mut self) {
        self.slot.active.fetch_sub(1, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::ExecutionCancellation;

    fn persona() -> obscura_net::EffectivePersona {
        obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145)
    }

    #[test]
    fn cancellation_interrupts_v8_and_remains_sticky_after_clear() {
        let mut runtime = crate::runtime::ObscuraJsRuntime::new(persona());
        let cancellation = ExecutionCancellation::default();
        runtime.set_execution_cancellation(Some(cancellation.clone()));

        let trigger = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(50));
            cancellation.cancel();
        });
        let started = std::time::Instant::now();
        let first = runtime.execute_script("disconnect-loop", "while (true) {}");
        trigger.join().unwrap();
        assert!(first.is_err(), "disconnect must terminate active V8");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "disconnect cancellation took {:?}",
            started.elapsed()
        );

        // Internal deadline recovery is allowed to clear its own termination,
        // but the closed connection must immediately assert it again.
        runtime.cancel_termination();
        let second = runtime.execute_script("after-disconnect", "globalThis.ran = true");
        assert!(
            second.is_err(),
            "sticky cancellation must reject later V8 entry"
        );
    }

    #[test]
    fn repeated_runtime_attachment_reclaims_dead_slots() {
        let runtime = crate::runtime::ObscuraJsRuntime::new(persona());
        let cancellation = ExecutionCancellation::default();
        let handle = runtime.isolate_handle();
        for _ in 0..1_000 {
            drop(cancellation.attach(handle.clone()));
        }
        assert_eq!(
            cancellation.slot_count(),
            1,
            "only the final dead slot may remain until the next attach or cancel"
        );
    }
}
