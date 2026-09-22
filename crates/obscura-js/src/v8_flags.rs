use std::sync::Mutex;

use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum V8FlagsStatus {
    Noop,
    Applied,
    AlreadyConfigured,
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum V8FlagsError {
    #[error(
        "V8 flags are already configured as {active:?}; cannot configure conflicting flags {requested:?}"
    )]
    Conflict { active: String, requested: String },
    #[error(
        "cannot configure V8 flags {requested:?} after runtime construction has started"
    )]
    PlatformStarted { requested: String },
    #[error(
        "V8 flag application for {active:?} did not complete; refusing request {requested:?}"
    )]
    ApplicationIncomplete { active: String, requested: String },
    #[error("V8 flag process state is poisoned; refusing request {requested:?}")]
    StatePoisoned { requested: String },
}

#[derive(Debug)]
struct ProcessV8FlagsState {
    configured: Option<String>,
    applying: bool,
    platform_started: bool,
}

impl ProcessV8FlagsState {
    const fn new() -> Self {
        Self { configured: None, applying: false, platform_started: false }
    }

    fn claim(&mut self, requested: &str) -> Result<V8FlagsStatus, V8FlagsError> {
        if requested.is_empty() {
            return Ok(V8FlagsStatus::Noop);
        }
        if self.applying {
            return Err(V8FlagsError::ApplicationIncomplete {
                active: self.configured.clone().unwrap_or_default(),
                requested: requested.to_string(),
            });
        }
        if let Some(active) = self.configured.as_deref() {
            if active == requested {
                return Ok(V8FlagsStatus::AlreadyConfigured);
            }
            return Err(V8FlagsError::Conflict {
                active: active.to_string(), requested: requested.to_string(),
            });
        }
        if self.platform_started {
            return Err(V8FlagsError::PlatformStarted { requested: requested.to_string() });
        }
        self.configured = Some(requested.to_string());
        self.applying = true;
        Ok(V8FlagsStatus::Applied)
    }

    fn finish_application(&mut self) {
        debug_assert!(self.applying);
        self.applying = false;
    }
}

static STATE: Mutex<ProcessV8FlagsState> = Mutex::new(ProcessV8FlagsState::new());

/// Record that runtime construction has claimed the process-wide V8 platform.
///
/// This uses the same mutex as flag configuration. A concurrent flag request
/// therefore either completes before runtime construction starts or receives a
/// structured late-configuration error without calling V8.
pub(crate) fn mark_platform_started() {
    let mut state = STATE.lock().unwrap_or_else(|_| {
        panic!(
            "cannot start a V8 runtime after the process flag state was poisoned; \
             flag application may be incomplete"
        )
    });
    state.platform_started = true;
}

/// Configure process-wide V8 flags before the first runtime is constructed.
///
/// The first non-empty value is applied. Repeating that exact trimmed value is
/// idempotent. A conflicting value or a first value supplied after runtime
/// construction starts returns an error and never reaches V8. Empty and
/// whitespace-only values remain a no-op and do not claim the process setting.
pub fn try_set_v8_flags(flags: &str) -> Result<V8FlagsStatus, V8FlagsError> {
    let trimmed = flags.trim();
    let mut state = STATE.lock().map_err(|_| V8FlagsError::StatePoisoned {
        requested: trimmed.to_string(),
    })?;
    let status = state.claim(trimmed)?;
    if status == V8FlagsStatus::Applied {
        // Keep the process-state mutex held across the V8 call. Runtime
        // construction takes the same mutex in mark_platform_started(), so a
        // concurrent isolate cannot initialize between the check and apply.
        deno_core::v8::V8::set_flags_from_string(trimmed);
        state.finish_application();
    }
    Ok(status)
}

/// Compatibility wrapper for embedders that used the historical infallible
/// API. New product entry points should call [`try_set_v8_flags`] and propagate
/// its structured error instead of treating an ignored configuration as
/// success.
pub fn set_v8_flags(flags: &str) {
    if let Err(error) = try_set_v8_flags(flags) {
        tracing::warn!("set_v8_flags ignored: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_machine_is_idempotent_and_rejects_conflicts() {
        let mut state = ProcessV8FlagsState::new();
        assert_eq!(state.claim(""), Ok(V8FlagsStatus::Noop));
        assert_eq!(state.claim("--first"), Ok(V8FlagsStatus::Applied));
        state.finish_application();
        assert_eq!(state.claim("--first"), Ok(V8FlagsStatus::AlreadyConfigured));
        assert_eq!(
            state.claim("--second"),
            Err(V8FlagsError::Conflict {
                active: "--first".to_string(), requested: "--second".to_string(),
            }),
        );
    }

    #[test]
    fn state_machine_rejects_first_configuration_after_runtime_claim() {
        let mut state = ProcessV8FlagsState::new();
        state.platform_started = true;
        assert_eq!(
            state.claim("--late"),
            Err(V8FlagsError::PlatformStarted { requested: "--late".to_string() }),
        );
    }

    #[test]
    fn concurrent_conflicting_claims_have_one_observable_winner() {
        let state = std::sync::Arc::new(Mutex::new(ProcessV8FlagsState::new()));
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
        let mut handles = Vec::new();
        for requested in ["--first", "--second"] {
            let state = state.clone();
            let barrier = barrier.clone();
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                let mut state = state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                let result = state.claim(requested);
                if result == Ok(V8FlagsStatus::Applied) {
                    state.finish_application();
                }
                (requested, result)
            }));
        }
        barrier.wait();
        let outcomes: Vec<_> = handles.into_iter().map(|handle| handle.join().unwrap()).collect();
        let active = state.lock().unwrap().configured.clone().unwrap();

        assert_eq!(
            outcomes.iter().filter(|(_, result)| result == &Ok(V8FlagsStatus::Applied)).count(),
            1,
        );
        for (requested, result) in outcomes {
            if requested == active {
                assert_eq!(result, Ok(V8FlagsStatus::Applied));
            } else {
                assert_eq!(
                    result,
                    Err(V8FlagsError::Conflict {
                        active: active.clone(), requested: requested.to_string(),
                    }),
                );
            }
        }
    }

    #[test]
    fn poisoned_application_is_never_reported_as_configured() {
        let state = std::sync::Arc::new(Mutex::new(ProcessV8FlagsState::new()));
        let thread_state = state.clone();
        let panic = std::thread::spawn(move || {
            let mut state = thread_state.lock().unwrap();
            assert_eq!(state.claim("--first"), Ok(V8FlagsStatus::Applied));
            panic!("simulated V8 flag application panic");
        })
        .join();
        assert!(panic.is_err());

        let result = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .claim("--first");
        assert_eq!(
            result,
            Err(V8FlagsError::ApplicationIncomplete {
                active: "--first".to_string(), requested: "--first".to_string(),
            }),
        );
    }

    #[test]
    fn poisoned_global_state_rejects_flags_and_runtime_start() {
        let panic = std::thread::spawn(|| {
            let _state = STATE.lock().unwrap();
            panic!("simulated process-state panic");
        })
        .join();
        assert!(panic.is_err());
        assert_eq!(
            try_set_v8_flags("--first"),
            Err(V8FlagsError::StatePoisoned { requested: "--first".to_string() }),
        );
        assert!(std::panic::catch_unwind(mark_platform_started).is_err());
    }

    #[test]
    fn empty_is_noop() {
        assert_eq!(try_set_v8_flags(""), Ok(V8FlagsStatus::Noop));
        assert_eq!(try_set_v8_flags("   "), Ok(V8FlagsStatus::Noop));
        assert_eq!(try_set_v8_flags("\t\n"), Ok(V8FlagsStatus::Noop));
    }

    // #853: calling V8's flag parser after platform initialization aborts the
    // process. The strict API must reject the call before reaching V8.
    #[test]
    fn late_call_after_a_runtime_exists_returns_error_without_aborting() {
        let _rt = crate::runtime::ObscuraJsRuntime::new(
            obscura_net::EffectivePersona::builtin(
                obscura_net::StealthProfile::WindowsChrome145,
            ),
        );
        assert_eq!(
            try_set_v8_flags("--max-old-space-size=32"),
            Err(V8FlagsError::PlatformStarted {
                requested: "--max-old-space-size=32".to_string(),
            }),
        );
    }

    #[test]
    fn identical_value_remains_valid_after_runtime_start() {
        assert_eq!(
            try_set_v8_flags(" --max-old-space-size=32 "),
            Ok(V8FlagsStatus::Applied),
        );
        let _rt = crate::runtime::ObscuraJsRuntime::new(
            obscura_net::EffectivePersona::builtin(
                obscura_net::StealthProfile::WindowsChrome145,
            ),
        );
        assert_eq!(
            try_set_v8_flags("--max-old-space-size=32"),
            Ok(V8FlagsStatus::AlreadyConfigured),
        );
    }

    #[test]
    fn global_flag_configuration_and_runtime_claim_are_serialized() {
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let flag_barrier = barrier.clone();
        let flag = std::thread::spawn(move || {
            flag_barrier.wait();
            try_set_v8_flags("--max-old-space-size=32")
        });
        barrier.wait();
        let _runtime = crate::runtime::ObscuraJsRuntime::new(
            obscura_net::EffectivePersona::builtin(
                obscura_net::StealthProfile::WindowsChrome145,
            ),
        );
        let flag_result = flag.join().unwrap();

        match flag_result {
            Ok(V8FlagsStatus::Applied) => assert_eq!(
                try_set_v8_flags("--max-old-space-size=32"),
                Ok(V8FlagsStatus::AlreadyConfigured),
            ),
            Err(V8FlagsError::PlatformStarted { requested }) => {
                assert_eq!(requested, "--max-old-space-size=32")
            }
            other => panic!("unexpected flag/runtime race result: {other:?}"),
        }
    }
}
