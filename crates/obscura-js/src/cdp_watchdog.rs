//! Shared per-command V8 watchdog for the CDP server.
//!
//! One long-lived watchdog thread bounds every in-flight V8 command with a
//! deadline, instead of spawning and joining a thread per command (which adds
//! ~240us per command on the hot dispatch path). `arm` and `disarm` are a mutex
//! plus a condvar notify, in the low microseconds.
//!
//! With the thread-per-connection server (issue #430) several connections can
//! have a command armed at the same time (one isolate per connection, each on
//! its own OS thread), so a single global slot would let one connection's arm
//! overwrite another's and leave that command unbounded. The watchdog therefore
//! tracks a set of armed slots keyed by a monotonic generation, fires whichever
//! have overrun, and terminates each through its thread-safe `IsolateHandle`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::runtime::IsolateHandle;

struct Slot {
    deadline: Instant,
    handle: IsolateHandle,
    fired: Arc<AtomicBool>,
}

struct Shared {
    // (armed slots keyed by generation, monotonic generation counter)
    state: Mutex<(HashMap<u64, Slot>, u64)>,
    cv: Condvar,
}

static SHARED: OnceLock<Arc<Shared>> = OnceLock::new();

fn shared() -> &'static Arc<Shared> {
    SHARED.get_or_init(|| {
        let s = Arc::new(Shared {
            state: Mutex::new((HashMap::new(), 0)),
            cv: Condvar::new(),
        });
        let worker = s.clone();
        std::thread::Builder::new()
            .name("cdp-watchdog".into())
            .spawn(move || watchdog_loop(worker))
            .expect("spawn cdp watchdog");
        s
    })
}

fn watchdog_loop(s: Arc<Shared>) {
    let mut guard = s.state.lock().unwrap();
    loop {
        let now = Instant::now();
        // Terminate every slot that has overrun its deadline. The dispatcher's
        // `disarm` will observe `fired` and clear the V8 termination flag before
        // that isolate runs its next command.
        let expired: Vec<u64> = guard
            .0
            .iter()
            .filter(|(_, slot)| slot.deadline <= now)
            .map(|(gen, _)| *gen)
            .collect();
        for gen in expired {
            if let Some(slot) = guard.0.remove(&gen) {
                slot.fired.store(true, Ordering::SeqCst);
                slot.handle.terminate_execution();
            }
        }
        // Sleep until the nearest remaining deadline, or until arm/disarm wakes
        // us. The worker holds the lock until it waits, so a notify cannot be
        // lost into the void.
        let next = guard
            .0
            .values()
            .map(|slot| slot.deadline.saturating_duration_since(now))
            .min();
        guard = match next {
            None => s.cv.wait(guard).unwrap(),
            Some(dur) => s.cv.wait_timeout(guard, dur).unwrap().0,
        };
    }
}

/// Handle to an armed command; pass to [`disarm`].
pub struct Armed {
    gen: Option<u64>,
    fired: Arc<AtomicBool>,
}

/// Arm the shared watchdog for the current command. If the isolate is still
/// executing `budget` later, it is terminated. O(1), no thread spawn. Safe to
/// call concurrently from several connections: each command gets its own slot.
pub fn arm(handle: IsolateHandle, budget: Duration) -> Armed {
    arm_until(handle, Instant::now() + budget)
}

/// Arm against the caller's existing deadline without adding elapsed setup time.
pub fn arm_until(handle: IsolateHandle, deadline: Instant) -> Armed {
    let s = shared();
    let mut guard = s.state.lock().unwrap();
    guard.1 += 1;
    let gen = guard.1;
    let fired = Arc::new(AtomicBool::new(false));
    guard.0.insert(
        gen,
        Slot {
            deadline,
            handle,
            fired: fired.clone(),
        },
    );
    s.cv.notify_one();
    Armed { gen: Some(gen), fired }
}

/// Disarm the command's watchdog. Returns true if it had already fired
/// (terminated the isolate), in which case the caller must clear the V8
/// termination flag before the next command runs.
pub fn disarm(mut armed: Armed) -> bool {
    armed.remove_slot();
    armed.fired.load(Ordering::SeqCst)
}

impl Armed {
    fn remove_slot(&mut self) {
        let Some(gen) = self.gen.take() else {
            return;
        };
        let s = shared();
        let mut guard = s.state.lock().unwrap();
        guard.0.remove(&gen);
        // Wake the worker so it recomputes its sleep if we removed the nearest slot.
        s.cv.notify_one();
    }
}

impl Drop for Armed {
    fn drop(&mut self) {
        // Dispatch futures can be cancelled and malformed methods can return
        // early. Never leave a detached slot which may terminate a reused or
        // already-dropped isolate at the old deadline.
        self.remove_slot();
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn dropping_armed_watchdog_removes_its_future_termination() {
        let mut runtime = crate::runtime::ObscuraJsRuntime::new(
            obscura_net::EffectivePersona::builtin(
                obscura_net::StealthProfile::WindowsChrome145,
            ),
        );
        let watchdog = super::arm(
            runtime.isolate_handle(),
            std::time::Duration::from_millis(50),
        );
        drop(watchdog);
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert!(
            runtime
                .execute_script("after-dropped-watchdog", "globalThis.ok = true")
                .is_ok(),
            "a dropped watchdog must not terminate later work"
        );
    }
}
