use super::*;
const CAPTURE: &str = include_str!("testdata/speech_native_protocol.json");
fn captured() -> serde_json::Value { serde_json::from_str(CAPTURE).unwrap() }
fn decode_value(value: serde_json::Value) -> Result<Arc<Snapshot>, ProviderError> {
    speech_protocol::decode(&serde_json::to_vec(&value).unwrap())
}
#[test]
fn speech_protocol_preserves_real_native_ids_but_web_serialization_has_only_five_fields() {
    let snapshot = speech_protocol::decode(CAPTURE.as_bytes()).unwrap();
    assert!(!snapshot.voices.is_empty());
    let rows = captured();
    assert_eq!(snapshot.voices.len(), rows["mapped_count"].as_u64().unwrap() as usize);
    for (voice, raw) in snapshot.voices.iter().zip(rows["mapped_voices"].as_array().unwrap()) {
        assert_eq!(voice.native_identifier, raw["native_identifier"].as_str().unwrap());
        let value = serde_json::to_value(&voice.web).unwrap();
        assert_eq!(value.as_object().unwrap().len(), 5);
        for key in ["voiceURI", "name", "lang", "localService", "default"] { assert!(value.get(key).is_some()); }
        assert!(value.get("native_identifier").is_none());
    }
}
#[test]
fn speech_protocol_rejects_wrong_types_mapping_flags_counts_and_extra_fields() {
    let mutations: Vec<Box<dyn Fn(&mut serde_json::Value)>> = vec![
        Box::new(|v| v["schema"] = 1.into()),
        Box::new(|v| v["main_thread"] = false.into()),
        Box::new(|v| v["mapped_count"] = 1.into()),
        Box::new(|v| v["mapped_voices"][0]["web"]["localService"] = "true".into()),
        Box::new(|v| v["mapped_voices"][0]["web"]["default"] = false.into()),
        Box::new(|v| v["mapped_voices"][0]["web"]["voiceURI"] = "wrong".into()),
        Box::new(|v| v["mapped_voices"][0]["native_identifier"] = "x".repeat(16385).into()),
        Box::new(|v| v["mapped_voices"][1]["native_identifier"] = "not-in-real-inventory".into()),
        Box::new(|v| v["invented"] = true.into()),
    ];
    for mutate in mutations { let mut v = captured(); mutate(&mut v); assert!(matches!(decode_value(v), Err(ProviderError::Protocol))); }
    let mut trailing = CAPTURE.as_bytes().to_vec(); trailing.extend_from_slice(b"{}");
    assert!(matches!(speech_protocol::decode(&trailing), Err(ProviderError::Protocol)));
    assert!(matches!(speech_protocol::decode(&vec![b' '; MAX_OUTPUT + 1]), Err(ProviderError::OutputLimit)));
}
#[test]
fn speech_protocol_ready_empty_is_valid_and_distinct_from_native_failure() {
    let mut v = captured();
    v["voices"] = serde_json::json!([]); v["mapped_voices"] = serde_json::json!([]);
    for key in ["native_count", "record_count", "mapped_count", "skipped_nil_name", "ordered_skipped_nil_name", "string_bytes_charged"] { v[key] = 0.into(); }
    v["default_selection"] = serde_json::json!({"branch":"none","native_identifier":null,"removed_equal_av_objects":0});
    assert!(decode_value(v).unwrap().voices.is_empty());
    assert!(matches!(speech_protocol::decode(br#"{"schema":2,"status":"objc_exception_or_invalid_inventory"}"#), Err(ProviderError::Protocol)));
}
#[test]
fn speech_protocol_rejects_locale_change_instead_of_publishing_a_mixed_snapshot() {
    let mut v = captured(); v["locale_after"] = "different".into(); v["locale_identifier_changed"] = true.into();
    assert!(matches!(decode_value(v), Err(ProviderError::LocaleChanged)));
}

#[cfg(unix)]
mod child_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    fn quote(path: &std::path::Path) -> String { format!("'{}'", path.to_string_lossy().replace('\'', "'\\''")) }
    fn helper(body: &str, budget: Duration) -> (tempfile::TempDir, Provider) {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("fixture.json"); std::fs::write(&file, include_str!("testdata/speech_stream_resolved_selected.jsonl")).unwrap();
        let script = directory.path().join("helper");
        let body = body.replace("@FIXTURE@", &quote(&file)).replace("@MARKER@", &quote(&directory.path().join("started")));
        std::fs::write(&script, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        let provider = Provider::start(Launch::External { executable: script, arguments: vec![] }, budget);
        (directory, provider)
    }
    async fn result(provider: &Provider) -> Result<Arc<Snapshot>, ProviderError> {
        let mut subscription = provider.subscribe().unwrap();
        tokio::time::timeout(Duration::from_secs(5), subscription.ready()).await.unwrap()
    }
    async fn stopped(mut stopped: watch::Receiver<bool>) {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop { if *stopped.borrow_and_update() { break; } stopped.changed().await.unwrap(); }
        }).await.unwrap();
    }
    #[tokio::test(flavor = "current_thread")]
    async fn speech_provider_coalesces_and_reuses_one_real_protocol_snapshot() {
        let (directory, provider) = helper("printf x >> @MARKER@\nexec /bin/cat @FIXTURE@", Duration::from_secs(2));
        let mut a = provider.subscribe().unwrap(); let mut b = provider.subscribe().unwrap();
        let (a, b) = tokio::join!(a.ready(), b.ready()); let a = a.unwrap(); let b = b.unwrap();
        assert!(Arc::ptr_eq(&a, &b));
        assert!(Arc::ptr_eq(&a, &result(&provider).await.unwrap()));
        assert_eq!(std::fs::read(directory.path().join("started")).unwrap(), b"x");
        stopped(provider.0.stopped.clone()).await;
    }
    #[tokio::test(flavor = "current_thread")]
    async fn speech_provider_stream_preserves_real_snapshot_but_nonzero_exit_is_terminal_failure() {
        for (body, code) in [("/bin/cat @FIXTURE@\nexit 7", 7), ("/usr/bin/head -n 1 @FIXTURE@\nexit 2", 2)] {
            let (_directory, provider) = helper(body, Duration::from_secs(2));
            stopped(provider.0.stopped.clone()).await;
            // Genuine publication survives both a complete stream with nonzero
            // exit and a native failure after initial but before terminal output.
            assert!(!result(&provider).await.unwrap().voices.is_empty());
            let mut next = provider.subscribe_after(0).unwrap();
            assert!(matches!(next.next().await, Err(ProviderError::NativeExit(Some(actual))) if actual == code));
        }
    }
    #[tokio::test(flavor = "current_thread")]
    async fn speech_provider_limits_stdout_and_stderr_and_reaps_each_owned_child() {
        for (body, expected) in [("exec /usr/bin/head -c 4194305 /dev/zero", ProviderError::OutputLimit), ("exec /usr/bin/yes >&2", ProviderError::StderrLimit)] {
            let (_directory, provider) = helper(body, Duration::from_secs(2));
            assert_eq!(result(&provider).await.unwrap_err(), expected);
            stopped(provider.0.stopped.clone()).await;
        }
    }
    #[tokio::test(flavor = "current_thread")]
    async fn speech_provider_deadline_runs_while_callers_current_thread_runtime_is_stalled() {
        let (_directory, provider) = helper("exec /bin/sleep 60", Duration::from_millis(100));
        // Intentional: prove cancellation does not depend on this caller's executor.
        std::thread::sleep(Duration::from_millis(300));
        assert!(matches!(result(&provider).await, Err(ProviderError::Deadline)));
        stopped(provider.0.stopped.clone()).await;
    }
    #[tokio::test(flavor = "current_thread")]
    async fn speech_provider_drop_cancels_only_its_owned_child() {
        struct Other(std::process::Child);
        impl Drop for Other { fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); } }
        let mut other = Other(std::process::Command::new("/bin/sleep").arg("60").spawn().unwrap());
        let (directory, provider) = helper("printf x > @MARKER@\nexec /bin/sleep 60", Duration::from_secs(8));
        tokio::time::timeout(Duration::from_secs(2), async {
            while !directory.path().join("started").exists() { tokio::time::sleep(Duration::from_millis(5)).await; }
        }).await.unwrap();
        let stopped_rx = provider.0.stopped.clone(); drop(provider); stopped(stopped_rx).await;
        assert!(other.0.try_wait().unwrap().is_none());
    }
    #[tokio::test(flavor = "current_thread")]
    async fn speech_provider_waiter_limit_has_no_unbounded_queue_and_returns_permits() {
        let (_directory, provider) = helper("exec /bin/sleep 60", Duration::from_secs(8));
        let mut requests: Vec<_> = (0..256).map(|_| provider.subscribe().unwrap()).collect();
        assert!(matches!(provider.subscribe(), Err(ProviderError::RequestLimit)));
        requests.pop(); assert!(provider.subscribe().is_ok());
        let stopped_rx = provider.0.stopped.clone(); drop(requests); drop(provider); stopped(stopped_rx).await;
    }
}

#[cfg(target_os = "macos")]
#[tokio::test(flavor = "current_thread")]
#[ignore = "root explicitly qualifies the compiled embedded macOS helper"]
async fn speech_provider_embedded_native_protocol() {
    let mut request = Provider::shared().subscribe().unwrap();
    let began = std::time::Instant::now();
    let mut latest = None;
    tokio::time::timeout(Duration::from_secs(12), async {
        loop {
            let delivery = request.next().await.unwrap();
            if let Some(snapshot) = delivery.snapshot {
                eprintln!("NATIVE_SPEECH_REVISION={} RECORDS={} DEFAULT_BRANCH={} ELAPSED_MS={}",
                    delivery.revision, snapshot.voices.len(), snapshot.default_branch, began.elapsed().as_secs_f64()*1000.0);
                for (i, voice) in snapshot.voices.iter().enumerate() {
                    assert_eq!(voice.web.is_default, i == 0);
                    assert_eq!(voice.web.voice_uri, voice.web.name);
                    assert!(voice.web.local_service);
                }
                latest = Some(snapshot);
            }
            if delivery.done { break; }
        }
    }).await.unwrap();
    assert!(latest.is_some());
    // Ready(empty) is not rewritten or rejected here. Root must report the real
    // count and cannot call an empty capture a nonempty native inventory proof.
}

#[path = "speech_stream_tests.rs"]
mod streaming;
