// Protocol fixtures derive from captured native scalars, not product constants.
use super::*;
const SELECTED: &[u8] = include_bytes!("testdata/speech_stream_pending_selected.jsonl");
const NONE: &[u8] = include_bytes!("testdata/speech_stream_pending_none.jsonl");
const RESOLVED: &[u8] = include_bytes!("testdata/speech_stream_resolved_selected.jsonl");
fn lines(bytes: &[u8]) -> Vec<&[u8]> { bytes.split(|b| *b == b'\n').filter(|line| !line.is_empty()).collect() }
fn frames(bytes: &[u8]) -> Vec<serde_json::Value> { lines(bytes).iter().map(|line| serde_json::from_slice(line).unwrap()).collect() }
#[test]
fn speech_stream_pending_is_not_resolved_none_and_real_default_is_retained() {
    for (bytes, count) in [(SELECTED, 2), (NONE, 1), (RESOLVED, 1)] {
        let mut decoder = speech_protocol::StreamDecoder::default();
        let mut snapshots = vec![];
        let mut resolved = None;
        for line in lines(bytes) {
            match decoder.accept(line).unwrap() {
                speech_protocol::StreamEvent::Snapshot(value) => snapshots.push(value),
                speech_protocol::StreamEvent::Terminal(value) => resolved = Some(value),
            }
        }
        decoder.finish().unwrap();
        assert_eq!(snapshots.len(), count);
        if bytes != RESOLVED { assert_eq!(snapshots[0].default_branch, "pending"); }
        let resolved = resolved.unwrap();
        // Expected branch comes from fixture metadata, not a host default assumption.
        let expected = frames(bytes).pop().unwrap();
        assert_eq!(resolved.branch, expected["default_selection"]["branch"].as_str().unwrap());
        assert_eq!(resolved.native_identifier.is_some(), bytes != NONE);
        for snapshot in snapshots { for (index, voice) in snapshot.voices.iter().enumerate() {
            assert_eq!(voice.web.is_default, index == 0);
        } }
        assert!(matches!(decoder.accept(b"{}"), Err(ProviderError::Protocol)));
    }
}
#[test]
fn speech_stream_rejects_reorder_missing_terminal_fake_resolution_and_extra_frames() {
    let source = frames(SELECTED);
    let mut bad = vec![];
    let mut v = source.clone(); v.swap(0, 1); bad.push(v);
    let mut v = source.clone(); v[0]["default_state"] = "resolved".into(); bad.push(v);
    let mut v = source.clone(); v[1]["revision"] = 0.into(); bad.push(v);
    let mut v = source.clone(); v[2]["queries_started"] = 3.into(); bad.push(v);
    let mut v = source.clone(); v[2]["default_background_thread"] = false.into(); bad.push(v);
    let mut v = source.clone(); v[2]["default_selection"]["native_identifier"] = "invented".into(); bad.push(v);
    let mut v = source.clone(); v[0]["unexpected"] = true.into(); bad.push(v);
    let mut v = source.clone(); v.remove(1); v[1]["next_revision"] = 1.into(); bad.push(v);
    for frames in bad {
        let mut decoder = speech_protocol::StreamDecoder::default();
        let result = frames.iter().try_for_each(|value| decoder.accept(&serde_json::to_vec(value).unwrap()).map(|_| ()));
        assert!(result.is_err());
    }
    let mut decoder = speech_protocol::StreamDecoder::default();
    decoder.accept(lines(SELECTED)[0]).unwrap();
    assert!(decoder.finish().is_err());
}
#[tokio::test(flavor = "current_thread")]
async fn speech_stream_publishes_initial_before_default_bytes_or_child_exit_exist() {
    let (provider, sender) = Provider::controlled();
    let mut subscriber = provider.subscribe().unwrap();
    let initial = lines(SELECTED)[0].to_vec();
    let rest = SELECTED[initial.len() + 1..].to_vec();
    let (mut writer, reader) = tokio::io::duplex(4096);
    let task = tokio::spawn(read_stream(reader, sender.clone()));
    use tokio::io::AsyncWriteExt;
    let write = async { writer.write_all(&initial).await.unwrap(); writer.write_all(b"\n").await.unwrap(); };
    let receive = async { tokio::time::timeout(Duration::from_secs(1), subscriber.next()).await.unwrap().unwrap() };
    let ((), first) = tokio::join!(write, receive);
    assert_eq!(first.revision, 0); assert!(!first.done);
    assert_eq!(first.snapshot.unwrap().default_branch, "pending");
    // Default bytes are not written until the first delivery was observed.
    let finish_write = async { writer.write_all(&rest).await.unwrap(); writer.shutdown().await.unwrap(); };
    let receive = async { subscriber.next().await.unwrap() };
    let ((), second) = tokio::join!(finish_write, receive);
    assert_eq!(second.revision, 1); assert!(!second.done);
    assert!(second.snapshot.is_some());
    task.await.unwrap().unwrap().finish().unwrap(); finish_state(&sender, Ok(()));
    let end = subscriber.next().await.unwrap(); assert!(end.done && end.snapshot.is_none());
}
#[tokio::test(flavor = "current_thread")]
async fn speech_stream_slow_and_concurrent_subscribers_keep_both_revisions_and_late_reads_latest() {
    let (provider, sender) = Provider::controlled();
    let mut a = provider.subscribe().unwrap(); let mut b = provider.subscribe().unwrap();
    read_stream(SELECTED, sender.clone()).await.unwrap().finish().unwrap(); finish_state(&sender, Ok(()));
    for subscription in [&mut a, &mut b] {
        assert_eq!(subscription.next().await.unwrap().revision, 0);
        let second = subscription.next().await.unwrap(); assert_eq!(second.revision, 1); assert!(second.done);
    }
    let late = provider.subscribe().unwrap().next().await.unwrap();
    assert_eq!(late.revision, 1); assert!(late.done);
    assert!(matches!(provider.subscribe_after(-2), Err(ProviderError::Protocol)));
}
#[tokio::test(flavor = "current_thread")]
async fn speech_stream_terminal_failure_keeps_initial_for_existing_and_new_subscribers() {
    let (provider, sender) = Provider::controlled();
    let mut a = provider.subscribe().unwrap();
    let initial = &SELECTED[..lines(SELECTED)[0].len()+1];
    assert!(matches!(read_stream(initial, sender.clone()).await.unwrap().finish(), Err(ProviderError::Protocol)));
    finish_state(&sender, Err(ProviderError::Deadline));
    let mut late = provider.subscribe().unwrap();
    for subscription in [&mut a, &mut late] {
        let initial = subscription.next().await.unwrap(); assert_eq!(initial.revision, 0); assert!(!initial.done);
        assert!(matches!(subscription.next().await, Err(ProviderError::Deadline)));
    }
}
#[tokio::test(flavor = "current_thread")]
async fn speech_stream_output_limit_is_cumulative_including_frame_delimiters() {
    let (provider, sender) = Provider::controlled();
    let mut bytes = SELECTED.to_vec();
    bytes.resize(MAX_OUTPUT + 1, b' ');
    assert!(matches!(read_stream(bytes.as_slice(), sender.clone()).await, Err(ProviderError::OutputLimit)));
    finish_state(&sender, Err(ProviderError::OutputLimit));
    let mut late = provider.subscribe().unwrap(); assert!(late.next().await.unwrap().snapshot.is_some());
    assert!(matches!(late.next().await, Err(ProviderError::OutputLimit)));
}

#[tokio::test(flavor = "current_thread")]
async fn speech_stream_none_terminal_retains_resolution_without_an_extra_snapshot() {
    let (provider, sender) = Provider::controlled();
    let mut subscribed = provider.subscribe().unwrap();
    read_stream(NONE, sender.clone()).await.unwrap().finish().unwrap(); finish_state(&sender, Ok(()));
    let initial = subscribed.next().await.unwrap(); assert_eq!(initial.revision, 0); assert!(initial.done);
    let state = provider.0.state.borrow();
    let InventoryState::Streaming(progress) = &*state else { panic!("expected stream cache"); };
    assert_eq!(progress.snapshots.len(), 1);
    let resolved = progress.resolved_default.as_ref().unwrap();
    assert_eq!(resolved.branch, "none"); assert!(resolved.native_identifier.is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn speech_stream_all_reupdate_changes_survive_coalescing_and_terminal_waits_for_quiescence() {
    let (provider, sender) = Provider::controlled();
    let mut first = provider.subscribe().unwrap(); let mut other = provider.subscribe().unwrap();
    let bytes: &[u8] = include_bytes!("testdata/speech_stream_three_changes.jsonl");
    read_stream(bytes, sender.clone()).await.unwrap().finish().unwrap(); finish_state(&sender, Ok(()));
    for subscriber in [&mut first, &mut other] {
        for revision in 0..4 {
            let delivery = subscriber.next().await.unwrap();
            assert_eq!(delivery.revision, revision); assert!(delivery.snapshot.is_some());
            assert_eq!(delivery.done, revision == 3);
        }
    }
    assert_eq!(provider.subscribe().unwrap().next().await.unwrap().revision, 3);
}
#[test]
fn speech_stream_nil_change_is_valid_but_same_identifier_is_not_a_changed_notification() {
    let mut source = frames(include_bytes!("testdata/speech_stream_three_changes.jsonl"));
    let mut decoder = speech_protocol::StreamDecoder::default();
    for frame in &source { decoder.accept(&serde_json::to_vec(frame).unwrap()).unwrap(); }
    decoder.finish().unwrap();
    source[2]["snapshot"] = source[1]["snapshot"].clone();
    source[2]["transition"]["identifier_equal"] = true.into();
    let mut decoder = speech_protocol::StreamDecoder::default();
    decoder.accept(&serde_json::to_vec(&source[0]).unwrap()).unwrap();
    decoder.accept(&serde_json::to_vec(&source[1]).unwrap()).unwrap();
    assert!(matches!(decoder.accept(&serde_json::to_vec(&source[2]).unwrap()), Err(ProviderError::Protocol)));
}

#[test]
fn speech_stream_rejects_premature_or_impossible_quiescent_terminal() {
    let mut bad = Vec::new();
    let mut values = frames(NONE);
    values[1]["queries_started"] = 1.into(); values[1]["queries_completed"] = 1.into();
    bad.push(("constructor requery missing", values));
    let mut values = frames(RESOLVED);
    values[0]["queries_completed"] = 2.into(); values[0]["query_pending"] = true.into();
    values[1]["queries_started"] = 2.into(); values[1]["queries_completed"] = 2.into();
    bad.push(("declared pending work never completed", values));
    let multi = include_bytes!("testdata/speech_stream_three_changes.jsonl");
    let mut values = frames(multi);
    let end = values.len() - 1;
    values[end]["queries_started"] = 4.into(); values[end]["queries_completed"] = 4.into();
    bad.push(("query invented after quiescence", values));
    let mut values = frames(multi); values[1]["query_pending"] = false.into();
    bad.push(("change after declared quiescence", values));
    let mut accepted = Vec::new();
    for (name, values) in bad {
        let mut decoder = speech_protocol::StreamDecoder::default();
        if values.iter().try_for_each(|value| decoder.accept(&serde_json::to_vec(value).unwrap()).map(|_| ())).is_ok() {
            accepted.push(name);
        }
    }
    assert!(accepted.is_empty(), "accepted impossible native completion: {accepted:?}");
}

#[tokio::test(flavor = "current_thread")]
async fn speech_stream_accounting_frontier_admits_equality_and_preserves_history_on_rejection() {
    // Seed only the accounting branch; this does not model actual 8MiB allocation
    // or claim this frontier is naturally reachable under the wire limit.
    let initial = lines(NONE)[0];
    let mut decoder = speech_protocol::StreamDecoder::default();
    let speech_protocol::StreamEvent::Snapshot(snapshot) = decoder.accept(initial).unwrap() else { panic!("snapshot required"); };
    let charge = snapshot_charge(&snapshot).unwrap();
    let bytes = [initial, b"\n"].concat();
    for extra in [0, 1] {
        let (provider, sender) = Provider::controlled();
        let prior = snapshot.clone();
        let seed = MAX_HISTORY_CHARGE - charge + extra;
        sender.send_replace(InventoryState::Streaming(Progress { snapshots: vec![prior.clone()], retained_charge: seed,
            terminal: None, resolved_default: None }));
        let result = read_stream(bytes.as_slice(), sender.clone()).await;
        if extra == 0 { assert!(result.is_ok()); }
        else { assert!(matches!(result, Err(ProviderError::HistoryLimit))); }
        {
            let state = provider.0.state.borrow();
            let InventoryState::Streaming(progress) = &*state else { panic!("history retained"); };
            assert!(Arc::ptr_eq(&progress.snapshots[0], &prior));
            assert_eq!(progress.snapshots.len(), if extra == 0 { 2 } else { 1 });
            assert_eq!(progress.retained_charge, if extra == 0 { MAX_HISTORY_CHARGE } else { seed });
        }
        if extra == 1 {
            finish_state(&sender, Err(ProviderError::HistoryLimit));
            let mut late = provider.subscribe().unwrap();
            assert!(Arc::ptr_eq(&late.next().await.unwrap().snapshot.unwrap(), &prior));
            assert!(matches!(late.next().await, Err(ProviderError::HistoryLimit)));
        }
    }
}
#[tokio::test(flavor = "current_thread")]
async fn speech_stream_multiple_revisions_survive_controlled_terminal_failure() {
    let (provider, sender) = Provider::controlled();
    let mut first = provider.subscribe().unwrap(); let mut second = provider.subscribe().unwrap();
    let bytes: &[u8] = include_bytes!("testdata/speech_stream_three_changes.jsonl");
    read_stream(bytes, sender.clone()).await.unwrap().finish().unwrap();
    // Controlled terminal injection, not an actual native child exit observation.
    finish_state(&sender, Err(ProviderError::NativeExit(Some(2))));
    for subscribed in [&mut first, &mut second] {
        for revision in 0..4 {
            let delivery = subscribed.next().await.unwrap();
            assert_eq!(delivery.revision, revision); assert!(delivery.snapshot.is_some()); assert!(!delivery.done);
        }
        assert!(matches!(subscribed.next().await, Err(ProviderError::NativeExit(Some(2)))));
    }
    let mut late = provider.subscribe().unwrap();
    assert_eq!(late.next().await.unwrap().revision, 3);
    assert!(matches!(late.next().await, Err(ProviderError::NativeExit(Some(2)))));
}
