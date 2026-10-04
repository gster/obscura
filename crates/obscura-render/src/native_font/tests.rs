use super::*;
use std::sync::atomic::AtomicBool;
struct Fixture {
    data: Arc<Vec<u8>>, locate_calls: AtomicUsize, read_calls: AtomicUsize,
    fail_once: AtomicBool, family: String, postscript: String,
}
impl Backend for Arc<Fixture> {
    fn locate(&self, family: &str) -> Result<Option<Located>, Unavailable> {
        self.locate_calls.fetch_add(1, Ordering::Relaxed);
        if self.fail_once.swap(false, Ordering::Relaxed) { return Err(Unavailable::Io); }
        if family_key(family) != family_key(&self.family) { return Ok(None); }
        Ok(Some(Located { family: self.family.clone(), postscript: self.postscript.clone(),
            key: ResourceKey { path: PathBuf::from("fixture"), device: 1, inode: 1, bytes: self.data.len(), modified: 1 } }))
    }
    fn read(&self, _: &ResourceKey) -> Result<Vec<u8>, Unavailable> {
        self.read_calls.fetch_add(1, Ordering::Relaxed); Ok(self.data.as_ref().clone())
    }
}
fn fixture(data: Vec<u8>, family: &str, postscript: &str) -> Arc<Fixture> {
    Arc::new(Fixture { data: Arc::new(data), locate_calls: AtomicUsize::new(0), read_calls: AtomicUsize::new(0), fail_once: AtomicBool::new(false), family: family.into(), postscript: postscript.into() })
}
fn mono() -> Arc<Fixture> { fixture(crate::font::MONO_R.to_vec(), "Liberation Mono", "LiberationMono") }
fn found(provider: &Provider, name: &str) -> Arc<NativeAsset> {
    match provider.lookup(name) { Lookup::Found(asset) => asset, _ => panic!("expected verified real fixture face") }
}
// Real SFNTs repacked into a TTC; table offsets remain file-relative, no synthetic names/glyphs.
pub(crate) fn collection(fonts: &[&[u8]]) -> Vec<u8> {
    let mut data = b"ttcf\0\x01\0\0".to_vec();
    data.extend_from_slice(&(fonts.len() as u32).to_be_bytes());
    data.resize(12 + fonts.len() * 4, 0);
    for (index, font) in fonts.iter().enumerate() {
        while data.len() % 4 != 0 { data.push(0); }
        let start = data.len();
        data[12 + index * 4..16 + index * 4].copy_from_slice(&(start as u32).to_be_bytes());
        data.extend_from_slice(font);
        let count = u16::from_be_bytes([font[4], font[5]]) as usize;
        for table in 0..count {
            let offset = 12 + table * 16 + 8;
            let original = u32::from_be_bytes(font[offset..offset + 4].try_into().unwrap());
            data[start + offset..start + offset + 4].copy_from_slice(&(original + start as u32).to_be_bytes());
        }
    }
    data
}
#[test]
fn verified_ttc_keeps_actual_nonzero_index_without_global_database_ids() {
    let fixture = fixture(collection(&[crate::font::SANS_R, crate::font::MONO_R]), "Liberation Mono", "LiberationMono");
    let provider = Provider::with_backend(Box::new(fixture), Limits::default());
    let asset = found(&provider, "Liberation Mono");
    assert_eq!(asset.selected_index, 1);
    assert_eq!(asset.family_indices, [1]);
    let mut a = fontdb::Database::new();
    let mut b = fontdb::Database::new();
    b.load_font_data(crate::font::SERIF_R.to_vec());
    for db in [&mut a, &mut b] {
        let ids = db.load_font_source(fontdb::Source::Binary(asset.bytes()));
        let face = db.face(ids[1]).unwrap();
        assert_eq!(face.index, 1);
        assert_eq!(face.post_script_name, "LiberationMono");
    }
}
#[test]
fn cache_hit_shares_actual_bytes_and_does_no_backend_work() {
    let backend = mono();
    let provider = Provider::with_backend(Box::new(backend.clone()), Limits::default());
    assert_eq!(backend.locate_calls.load(Ordering::Relaxed), 0);
    let a = found(&provider, "Liberation Mono");
    let b = found(&provider, "liberation mono");
    assert!(Arc::ptr_eq(&a.bytes(), &b.bytes()));
    assert_eq!(backend.locate_calls.load(Ordering::Relaxed), 1);
    assert_eq!(backend.read_calls.load(Ordering::Relaxed), 1);
}
#[test]
fn temporary_errors_retry_but_absence_cache_is_bounded() {
    let backend = mono(); backend.fail_once.store(true, Ordering::Relaxed);
    let provider = Provider::with_backend(Box::new(backend.clone()), Limits { cache_entries: 2, ..Limits::default() });
    assert!(matches!(provider.lookup("Liberation Mono"), Lookup::Unavailable(Unavailable::Io)));
    assert!(matches!(provider.lookup("Liberation Mono"), Lookup::Found(_)));
    for name in ["Missing1", "Missing2", "Missing3", "Missing3"] { assert!(matches!(provider.lookup(name), Lookup::Absent)); }
    assert_eq!(provider.cache.lock().unwrap().names.len(), 2);
    assert_eq!(backend.locate_calls.load(Ordering::Relaxed), 5);
}
#[test]
fn last_fontdb_source_retains_reservation_after_cache_eviction() {
    let backend = mono(); let size = backend.data.len();
    let provider = Provider::with_backend(Box::new(backend), Limits { cache_entries: 1, live_bytes: size, ..Limits::default() });
    let asset = found(&provider, "Liberation Mono");
    let mut db = fontdb::Database::new();
    db.load_font_source(fontdb::Source::Binary(asset.bytes()));
    drop(asset);
    assert!(matches!(provider.lookup("Missing"), Lookup::Absent));
    assert_eq!(provider.budget.bytes.load(Ordering::Acquire), size);
    assert!(matches!(provider.lookup("Liberation Mono"), Lookup::Unavailable(Unavailable::Capacity)));
    drop(db);
    assert_eq!(provider.budget.bytes.load(Ordering::Acquire), 0);
    assert!(matches!(provider.lookup("Liberation Mono"), Lookup::Found(_)));
}
#[test]
fn independent_engine_admission_counts_unique_assets_and_retains_shared_data() {
    let provider = Provider::with_backend(Box::new(mono()), Limits::default());
    let asset = found(&provider, "Liberation Mono");
    let bytes = asset.bytes().as_ref().as_ref().len();
    let mut a = Admission::new(); let mut b = Admission::new();
    a.byte_limit = bytes - 1;
    assert_eq!(a.admit(&asset), Err(Unavailable::Capacity));
    assert_eq!(a.bytes, 0);
    a.byte_limit = bytes;
    a.admit(&asset).unwrap(); a.admit(&asset).unwrap(); b.admit(&asset).unwrap();
    assert_eq!(a.files.len(), 1); assert_eq!(a.bytes, bytes);
    assert!(Arc::ptr_eq(&a.files[0].data, &b.files[0].data));
}
#[test]
fn mismatching_real_postscript_and_invalid_metadata_are_not_accepted() {
    let provider = Provider::with_backend(Box::new(fixture(crate::font::MONO_R.to_vec(), "Liberation Mono", "DifferentPostScript")), Limits::default());
    assert!(matches!(provider.lookup("Liberation Mono"), Lookup::Unavailable(Unavailable::InvalidFont)));
    assert_eq!(provider.budget.bytes.load(Ordering::Acquire), 0);
    let mut malformed = b"ttcf\0\x01\0\0\0\0\0\x01\xff\xff\xff\xff".to_vec();
    assert_eq!(preflight(&malformed), Err(Unavailable::InvalidFont));
    malformed[8..12].copy_from_slice(&257u32.to_be_bytes());
    assert_eq!(preflight(&malformed), Err(Unavailable::TooLarge));
}
#[test]
fn over_budget_asset_is_rejected_before_read_and_busy_is_not_cached() {
    let backend = mono();
    let provider = Provider::with_backend(Box::new(backend.clone()), Limits { file_bytes: 1, ..Limits::default() });
    assert!(matches!(provider.lookup("Liberation Mono"), Lookup::Unavailable(Unavailable::TooLarge)));
    assert_eq!(backend.read_calls.load(Ordering::Relaxed), 0);
    provider.in_flight.store(2, Ordering::Release);
    assert!(matches!(provider.lookup("Missing"), Lookup::Unavailable(Unavailable::Busy)));
    provider.in_flight.store(0, Ordering::Release);
    assert!(matches!(provider.lookup("Missing"), Lookup::Absent));
}
#[cfg(target_os = "macos")]
#[test]
#[ignore = "explicit host public-font lookup; not a hermetic test"]
fn actual_macos_protected_public_font_lookup() {
    let provider = Provider::new();
    for family in ["Menlo", "Helvetica Neue", "Arial"] {
        let asset = found(&provider, family);
        assert!(asset.faces().iter().any(|face| face.index == asset.selected_index && face.families.iter().any(|name| family_key(name) == family_key(family))));
    }
    assert!(matches!(provider.lookup("MissingNativeFont_0ac078a2_1842_4cfe_a744_f8409bc79ddb"), Lookup::Absent));
    for named in ["serif", "monospace", "ArialMT"] {
        match provider.lookup(named) {
            Lookup::Absent => (),
            Lookup::Found(asset) => assert!(asset.faces().iter().any(|face| face.index == asset.selected_index && face.families.iter().any(|family| family_key(family) == family_key(named)))),
            Lookup::Unavailable(reason) => panic!("host lookup unavailable: {reason:?}"),
        }
    }
}

#[test]
fn empty_named_token_and_invalid_requests_do_no_native_work() {
    let backend = mono();
    let provider = Provider::with_backend(Box::new(backend.clone()), Limits::default());
    assert!(matches!(provider.lookup(""), Lookup::Absent));
    assert!(matches!(provider.lookup("bad\0name"), Lookup::Unavailable(Unavailable::InvalidRequest)));
    assert!(matches!(provider.lookup(&"x".repeat(257)), Lookup::Unavailable(Unavailable::InvalidRequest)));
    assert_eq!(backend.locate_calls.load(Ordering::Relaxed), 0);
}

#[test]
fn platform_gate_retries_transient_failures_and_caches_only_parsed_versions() {
    let cache = std::sync::OnceLock::new();
    assert_eq!(cached_platform_support(&cache, || Err(Unavailable::Io)), Err(Unavailable::Io));
    assert!(cache.get().is_none());
    for bad in [vec![], b"13.0".to_vec(), vec![0xff, 0], b"bad\0".to_vec(), b"13.bad\0".to_vec(), vec![b'1'; 65]] {
        assert_eq!(cached_platform_support(&cache, || Ok(bad)), Err(Unavailable::Io));
        assert!(cache.get().is_none());
    }
    assert_eq!(cached_platform_support(&cache, || Ok(b"13.0.1\0".to_vec())), Ok(true));
    assert_eq!(cached_platform_support(&cache, || panic!("valid version must be cached")), Ok(true));
    let old = std::sync::OnceLock::new();
    assert_eq!(cached_platform_support(&old, || Ok(b"12.7\0".to_vec())), Ok(false));
    assert_eq!(cached_platform_support(&old, || panic!("confirmed old version must be cached")), Ok(false));
}

/// Cross-consumer tests inject actual font bytes, deriving all names from that file.
pub(crate) struct FixtureSignals { pub lookup_calls: AtomicUsize, pub cascade_calls: AtomicUsize, pub fail_next: AtomicBool }
pub(crate) fn fixture_provider(data: Vec<u8>) -> (Arc<Provider>, Arc<FixtureSignals>) {
    struct BytesBackend { data: Vec<u8>, signals: Arc<FixtureSignals> }
    impl Backend for BytesBackend {
        fn locate(&self, family: &str) -> Result<Option<Located>, Unavailable> {
            self.signals.lookup_calls.fetch_add(1, Ordering::Relaxed);
            if self.signals.fail_next.swap(false, Ordering::Relaxed) { return Err(Unavailable::Io); }
            let mut db = fontdb::Database::new(); db.load_font_data(self.data.clone());
            let face = db.faces().find(|face| face.families.iter().any(|(name, _)| family_key(name) == family_key(family)));
            Ok(face.map(|face| Located { key: ResourceKey { path: PathBuf::from("injected-real-font"), device: 1, inode: 1, bytes: self.data.len(), modified: 1 },
                family: face.families.iter().find(|(name, _)| family_key(name) == family_key(family)).unwrap().0.clone(), postscript: face.post_script_name.clone() }))
        }
        fn read(&self, _: &ResourceKey) -> Result<Vec<u8>, Unavailable> { Ok(self.data.clone()) }
    }
    let signals = Arc::new(FixtureSignals { lookup_calls: AtomicUsize::new(0), cascade_calls: AtomicUsize::new(0), fail_next: AtomicBool::new(false) });
    (Arc::new(Provider::with_backend(Box::new(BytesBackend { data, signals: signals.clone() }), Limits::default())), signals)
}


/// Real separate files or collections, selected through fontdb's real style metadata.
pub(crate) fn fixture_style_provider(files: Vec<Vec<u8>>) -> (Arc<Provider>, Arc<FixtureSignals>) {
    fixture_style_provider_with_file_limit(files, 32)
}

pub(crate) fn fixture_style_provider_with_file_limit(files: Vec<Vec<u8>>, live_files: usize) -> (Arc<Provider>, Arc<FixtureSignals>) {
    fixture_cascade_provider(files, Vec::new(), live_files)
}
pub(crate) fn fixture_cascade_provider(files: Vec<Vec<u8>>, cascade_order: Vec<usize>, live_files: usize) -> (Arc<Provider>, Arc<FixtureSignals>) {
    assert!((1..=32).contains(&live_files));
    struct Styled { files: Vec<Vec<u8>>, cascade_order: Vec<usize>, signals: Arc<FixtureSignals> }
    impl Backend for Styled {
        fn locate(&self, family: &str) -> Result<Option<Located>, Unavailable> { self.locate_styled(family, 400, false) }
        fn locate_styled(&self, family: &str, weight: u16, italic: bool) -> Result<Option<Located>, Unavailable> {
            self.signals.lookup_calls.fetch_add(1, Ordering::Relaxed);
            if self.signals.fail_next.swap(false, Ordering::Relaxed) { return Err(Unavailable::Io); }
            let mut db = fontdb::Database::new();
            let mut sources = Vec::new();
            for (file, bytes) in self.files.iter().enumerate() {
                sources.extend(db.load_font_source(fontdb::Source::Binary(Arc::new(bytes.clone()))).into_iter().map(|id| (id, file)));
            }
            // CSS/native family identity is case-insensitive; fontdb's Query is
            // exact-case. Resolve the spelling from real metadata before styling.
            let canonical = db.faces().flat_map(|face| face.families.iter())
                .find(|(name, _)| family_key(name) == family_key(family)).map(|(name, _)| name.clone());
            let Some(canonical) = canonical else { return Ok(None); };
            let Some(id) = db.query(&fontdb::Query { families: &[fontdb::Family::Name(&canonical)], weight: fontdb::Weight(weight),
                style: if italic { fontdb::Style::Italic } else { fontdb::Style::Normal }, ..Default::default() }) else { return Ok(None); };
            let file = sources.iter().find(|(face, _)| *face == id).unwrap().1;
            let face = db.face(id).unwrap();
            Ok(Some(Located { family: family.into(), postscript: face.post_script_name.clone(),
                key: ResourceKey { path: PathBuf::from(format!("style-fixture-{file}")), device: 1, inode: file as u64 + 1, bytes: self.files[file].len(), modified: 1 } }))
        }
        fn cascade(&self, _: &FallbackRequest<'_>, limit: usize) -> Result<Cascade, Unavailable> {
            self.signals.cascade_calls.fetch_add(1, Ordering::Relaxed);
            let mut result = Vec::new();
            for &file in self.cascade_order.iter().take(limit) {
                let mut db = fontdb::Database::new(); db.load_font_data(self.files[file].clone());
                let face = db.faces().next().expect("real fixture face");
                result.push(Located { family: face.families[0].0.clone(), postscript: face.post_script_name.clone(),
                    key: ResourceKey { path: PathBuf::from(format!("style-fixture-{file}")), device: 1, inode: file as u64 + 1, bytes: self.files[file].len(), modified: 1 } });
            }
            Ok(Cascade { candidates: result, incomplete: self.cascade_order.len() > limit })
        }
        fn read(&self, key: &ResourceKey) -> Result<Vec<u8>, Unavailable> { Ok(self.files[key.inode as usize - 1].clone()) }
    }
    let signals = Arc::new(FixtureSignals { lookup_calls: AtomicUsize::new(0), cascade_calls: AtomicUsize::new(0), fail_next: AtomicBool::new(false) });
    (Arc::new(Provider::with_backend(Box::new(Styled { files, cascade_order, signals: signals.clone() }), Limits { live_files, ..Limits::default() })), signals)
}

#[test]
fn styled_cache_separates_positive_negative_and_transient_requests() {
    let (provider, calls) = fixture_style_provider(vec![crate::font::MONO_R.to_vec(), crate::font::MONO_B.to_vec(), crate::font::MONO_O.to_vec()]);
    let mut owners = Vec::new();
    for (weight, italic, ps) in [(400, false, "LiberationMono"), (700, false, "LiberationMono-Bold"), (400, true, "LiberationMono-Italic")] {
        let Lookup::Found(asset) = provider.lookup_styled("Liberation Mono", weight, italic) else { panic!("actual fixture style missing") };
        assert_eq!(asset.faces().iter().find(|face| face.index == asset.selected_index).unwrap().postscript, ps);
        owners.push(asset);
        assert!(matches!(provider.lookup_styled("Liberation Mono", weight, italic), Lookup::Found(_)));
    }
    assert_eq!(calls.lookup_calls.load(Ordering::Relaxed), 3);
    assert!(!Arc::ptr_eq(&owners[0].bytes(), &owners[1].bytes()));
    for (weight, italic) in [(400, false), (700, false), (700, false), (400, true)] {
        assert!(matches!(provider.lookup_styled("Absent fixture family", weight, italic), Lookup::Absent));
    }
    assert_eq!(calls.lookup_calls.load(Ordering::Relaxed), 6);
    calls.fail_next.store(true, Ordering::Relaxed);
    assert!(matches!(provider.lookup_styled("Liberation Mono", 500, true), Lookup::Unavailable(Unavailable::Io)));
    assert!(matches!(provider.lookup_styled("Liberation Mono", 500, true), Lookup::Found(_)));
    assert_eq!(calls.lookup_calls.load(Ordering::Relaxed), 8);
}

#[test]
fn styled_ttc_selection_shares_one_byte_owner_and_real_indices() {
    let (provider, _) = fixture_style_provider(vec![collection(&[crate::font::MONO_R, crate::font::MONO_B, crate::font::MONO_O])]);
    let mut assets = Vec::new();
    for (weight, italic, index) in [(400, false, 0), (700, false, 1), (400, true, 2)] {
        let Lookup::Found(asset) = provider.lookup_styled("Liberation Mono", weight, italic) else { panic!("TTC style missing") };
        assert_eq!(asset.selected_index, index); assets.push(asset);
    }
    assert!(Arc::ptr_eq(&assets[0].bytes(), &assets[1].bytes()));
    assert!(Arc::ptr_eq(&assets[0].bytes(), &assets[2].bytes()));
    assert_eq!(provider.budget.files.load(Ordering::Relaxed), 1);
}


#[test]
fn capacity_reclaim_keeps_located_candidate_and_runs_outside_provider_mutex() {
    let (provider,calls)=fixture_style_provider_with_file_limit(vec![crate::font::MONO_R.to_vec(),crate::font::SANS_R.to_vec()],1);
    let first=found(&provider,"Liberation Mono");
    let mut db=fontdb::Database::new();db.load_font_source(fontdb::Source::Binary(first.bytes()));
    drop(first);let mut owner=Some(db);let mut reclamations=0;
    let second=provider.lookup_styled_reclaim("Liberation Sans",400,false,&mut || {
        assert!(provider.cache.try_lock().is_ok(),"consumer callback must never hold provider cache mutex");
        assert_eq!(provider.in_flight.load(Ordering::Acquire),1,"one already-located load is being resumed");
        assert_eq!(provider.budget.files.load(Ordering::Acquire),1);
        drop(owner.take());
        assert_eq!(provider.budget.files.load(Ordering::Acquire),0,"last real source releases its reservation");
        assert_eq!(provider.budget.bytes.load(Ordering::Acquire),0);
        reclamations+=1;true
    });
    let Lookup::Found(second)=second else {panic!("capacity release must resume the same actual source")};
    assert_eq!(calls.lookup_calls.load(Ordering::Relaxed),2,"one locate per logical family, no candidate replay");
    assert_eq!(reclamations,1);assert_eq!(provider.budget.files.load(Ordering::Acquire),1);
    assert_eq!(second.faces()[second.selected_index as usize].postscript,"LiberationSans");
}

#[test]
fn operation_pins_and_external_source_keep_budget_until_actual_last_drop() {
    let (provider, _) = fixture_cascade_provider(vec![crate::font::MONO_R.to_vec(), crate::font::SANS_R.to_vec()], vec![1], 1);
    let first = found(&provider, "Liberation Mono");
    let mut owner = fontdb::Database::new();
    owner.load_font_source(fontdb::Source::Binary(first.bytes()));
    let weak = Arc::downgrade(&first.bytes());
    drop(first);
    provider.cache.lock().unwrap().names.clear();
    let candidate = provider.cascade(&FallbackRequest { family:"Liberation Mono",postscript:Some("LiberationMono"),weight:400,italic:false,size:32.0,locale:"en-US" }, 1).unwrap().candidates.pop().unwrap();
    assert!(matches!(provider.load_candidate(candidate.clone(), &mut || false), Lookup::Unavailable(Unavailable::Capacity)));
    assert_eq!(provider.budget.files.load(Ordering::Acquire), 1);
    drop(owner);
    assert!(weak.upgrade().is_none());
    assert_eq!(provider.budget.files.load(Ordering::Acquire), 0);
    assert_eq!(provider.budget.bytes.load(Ordering::Acquire), 0);
    let Lookup::Found(second) = provider.load_candidate(candidate, &mut || false) else { panic!("released capacity must be retryable") };
    assert_eq!(provider.budget.files.load(Ordering::Acquire), 1);
    drop(second);
    assert_eq!(provider.budget.files.load(Ordering::Acquire), 0);
    assert_eq!(provider.budget.bytes.load(Ordering::Acquire), 0);
}

#[test]
fn styled_fixture_obeys_real_case_insensitive_family_metadata_and_keeps_style() {
    let (provider, signals) = fixture_cascade_provider(vec![crate::font::MONO_R.to_vec(),crate::font::MONO_B.to_vec()],vec![1],32);
    for (name,weight,index,postscript) in [("liberation mono",400,0,"LiberationMono"),("LIBERATION MONO",700,0,"LiberationMono-Bold")] {
        let Lookup::Found(asset)=provider.lookup_styled(name,weight,false) else {panic!("real family case must not change availability")};
        let selected=asset.faces().iter().find(|face|face.index==asset.selected_index).unwrap();
        assert_eq!(asset.selected_index,index);assert_eq!(selected.postscript,postscript);
        assert!(selected.families.iter().any(|family|family=="Liberation Mono"),"real metadata must not be renamed");
    }
    assert_eq!(signals.lookup_calls.load(Ordering::Relaxed),2);
}
