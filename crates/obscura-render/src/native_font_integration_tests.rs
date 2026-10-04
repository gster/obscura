use super::*;
use crate::native_font::{fixture_collection, fixture_provider};
use cosmic_text::fontdb;

const NOTO: &[u8] = include_bytes!("../../../vendor/cosmic-text/fonts/NotoSans-Regular.ttf");
const HEBREW: &[u8] = include_bytes!("../../../vendor/cosmic-text/fonts/NotoSansHebrew.ttf");

fn binary(fs: &FontSystem, id: fontdb::ID) -> Arc<dyn AsRef<[u8]> + Send + Sync> {
    match &fs.db().face(id).unwrap().source {
        fontdb::Source::Binary(bytes) => bytes.clone(),
        _ => panic!("native assets must retain their original shared binary owner"),
    }
}

#[test]
fn native_selection_only_queries_unresolved_ordered_named_tokens() {
    let (provider, calls) = fixture_provider(NOTO.to_vec());
    let (mut fs, mut loaded) = create_font_system(&[], false);
    let mut native = NativeFonts::injected(provider, false);
    for stack in ["serif", "monospace, 'Noto Sans'", "'Liberation Sans', 'Noto Sans'", "-apple-system"] {
        let result = resolve_native_font(Some(stack), 400, false, &mut fs, &mut loaded, &mut native);
        assert!(result.font_id.is_some());
    }
    assert_eq!(calls.lookup_calls.load(Ordering::Relaxed), 0);
    let selected = resolve_native_font(Some("'Absent Requested Face', 'Noto Sans', serif"), 400, false, &mut fs, &mut loaded, &mut native);
    let id = selected.font_id.unwrap();
    assert_eq!(fs.db().face(id).unwrap().post_script_name, "NotoSans-Regular");
    assert!(native.is_native_face(id));
    assert_eq!(calls.lookup_calls.load(Ordering::Relaxed), 2);
    let count = fs.db().faces().count();
    let again = resolve_native_font(Some("'Noto Sans', serif"), 400, false, &mut fs, &mut loaded, &mut native);
    assert_eq!(again.font_id, Some(id));
    assert_eq!(fs.db().faces().count(), count);
    assert_eq!(calls.lookup_calls.load(Ordering::Relaxed), 2);
}

#[test]
fn native_resource_alias_wins_without_any_host_lookup() {
    let (provider, calls) = fixture_provider(NOTO.to_vec());
    let resource = WebFont { data: Arc::new(MONO_R.to_vec()), family: Some("Noto Sans".into()), weight: None, italic: None };
    let (mut fs, mut loaded) = create_font_system(&[resource], false);
    let expected = resolve_loaded_font(Some("'Noto Sans'"), 400, false, &loaded).font_id;
    let mut native = NativeFonts::injected(provider, false);
    let selected = resolve_native_font(Some("'Noto Sans'"), 400, false, &mut fs, &mut loaded, &mut native);
    assert_eq!(selected.font_id, expected);
    assert_eq!(fs.db().face(selected.font_id.unwrap()).unwrap().post_script_name, "LiberationMono");
    assert_eq!(calls.lookup_calls.load(Ordering::Relaxed), 0);
    assert!(!native.is_native_face(selected.font_id.unwrap()));
}

#[test]
fn native_ttc_multiple_families_preserve_indices_and_share_bytes_across_engines() {
    let (provider, _) = fixture_provider(fixture_collection(&[NOTO, HEBREW]));
    let (mut a, mut la) = create_font_system(&[], false);
    let (mut b, mut lb) = create_font_system(&[WebFont { data: Arc::new(MONO_R.to_vec()), family: Some("Page extra".into()), weight: None, italic: None }], false);
    let (mut na, mut nb) = (NativeFonts::injected(provider.clone(), false), NativeFonts::injected(provider, false));
    let count = a.db().faces().count();
    let aid = resolve_native_font(Some("'Noto Sans'"), 400, false, &mut a, &mut la, &mut na).font_id.unwrap();
    assert_eq!(a.db().faces().count(), count + 1);
    let hid = resolve_native_font(Some("'Noto Sans Hebrew'"), 400, false, &mut a, &mut la, &mut na).font_id.unwrap();
    assert_eq!(a.db().faces().count(), count + 2);
    assert_eq!(a.db().face(aid).unwrap().index, 0);
    assert_eq!(a.db().face(hid).unwrap().index, 1);
    let bid = resolve_native_font(Some("'Noto Sans Hebrew'"), 400, false, &mut b, &mut lb, &mut nb).font_id.unwrap();
    assert_eq!(b.db().face(bid).unwrap().index, 1);
    assert!(Arc::ptr_eq(&binary(&a, aid), &binary(&a, hid)));
    assert!(Arc::ptr_eq(&binary(&a, hid), &binary(&b, bid)));
    assert_eq!(na.files.len(), 1);
    let again = resolve_native_font(Some("'Noto Sans'"), 700, false, &mut a, &mut la, &mut na);
    assert_eq!(again.font_id, Some(aid));
    assert_eq!(a.db().faces().count(), count + 2);
}

#[test]
fn native_layout_freezes_complete_first_choice_and_seals_before_shaping() {
    let (provider, signals) = fixture_provider(fixture_collection(&[NOTO, HEBREW]));
    signals.fail_next.store(true, Ordering::Relaxed);
    let (mut fs, mut loaded) = create_font_system(&[], false);
    let mut native = NativeFonts::injected(provider, true);
    let stack = "'Noto Sans', 'Noto Sans Hebrew', serif";
    let first = resolve_native_font(Some(stack), 400, false, &mut fs, &mut loaded, &mut native);
    assert_eq!(fs.db().face(first.font_id.unwrap()).unwrap().index, 1);
    // A later style may add another real family, but a prior full request keeps its choice.
    let later = resolve_native_font(Some("'Noto Sans Hebrew'"), 700, false, &mut fs, &mut loaded, &mut native);
    assert_eq!(later.font_id, first.font_id);
    let prepared = resolve_prepared_font(Some(stack), 400, false, &loaded, &native);
    assert_eq!(prepared.font_id, first.font_id);
    native.seal();
    let calls = signals.lookup_calls.load(Ordering::Relaxed);
    resolve_native_font(Some("'Another absent face', serif"), 400, false, &mut fs, &mut loaded, &mut native);
    assert_eq!(signals.lookup_calls.load(Ordering::Relaxed), calls);
    assert_eq!(resolve_prepared_font(Some(stack), 400, false, &loaded, &native).font_id, first.font_id);
}

#[test]
fn native_layout_decision_capacity_stops_new_imports_without_eviction() {
    let (provider, signals) = fixture_provider(NOTO.to_vec());
    let (mut fs, mut loaded) = create_font_system(&[], false);
    let mut native = NativeFonts::injected(provider, true);
    for index in 0..1024 {
        let stack = format!("'One Absent Family', serif, 'Unvisited {index}'");
        resolve_native_font(Some(&stack), 400, false, &mut fs, &mut loaded, &mut native);
    }
    let first_stack = "'One Absent Family', serif, 'Unvisited 0'";
    let first = native.prepared(Some(first_stack), 400, false).unwrap();
    let result = resolve_native_font(Some("'Noto Sans', serif"), 400, false, &mut fs, &mut loaded, &mut native);
    assert!(!native.is_native_face(result.font_id.unwrap()));
    assert!(native.stopped);
    assert_eq!(signals.lookup_calls.load(Ordering::Relaxed), 1);
    assert_eq!(native.prepared(Some(first_stack), 400, false).unwrap().font_id, first.font_id);
    assert_eq!(native.decision_count, 1024);
}

#[test]
fn appended_fonts_refresh_monospace_classification_and_remain_normal_fallback_candidates() {
    let mut fs = FontSystem::new_with_locale_and_db("en-US".into(), fontdb::Database::new());
    let ids = fs.append_font_source(fontdb::Source::Binary(Arc::new(MONO_R)), Some(&[0]));
    assert_eq!(ids.len(), 1);
    assert!(fs.is_monospace(ids[0]));
    let original = ids[0];
    let sans = fs.append_font_source(fontdb::Source::Binary(Arc::new(NOTO)), Some(&[0]));
    assert!(fs.db().face(original).is_some());
    assert!(!fs.is_monospace(sans[0]));
    let matches = fs.get_font_matches(&cosmic_text::Attrs::new());
    assert_eq!(matches.len(), 2, "both loaded real resources must remain eligible for ordinary glyph fallback");
}

#[test]
fn native_generic_fastpath_does_not_consume_decision_capacity() {
    let (provider, signals) = fixture_provider(NOTO.to_vec());
    let (mut fs, mut loaded) = create_font_system(&[], false);
    let mut native = NativeFonts::injected(provider, true);
    for index in 0..2048 {
        let stack = format!("serif, 'Unvisited {index}'");
        resolve_native_font(Some(&stack), 400, false, &mut fs, &mut loaded, &mut native);
    }
    assert_eq!(native.decision_count, 0);
    assert_eq!(signals.lookup_calls.load(Ordering::Relaxed), 0);
    let selected = resolve_native_font(Some("'Noto Sans', serif"), 400, false, &mut fs, &mut loaded, &mut native);
    assert!(native.is_native_face(selected.font_id.unwrap()));
    assert_eq!(signals.lookup_calls.load(Ordering::Relaxed), 1);
}

#[test]
fn native_sealed_graph_keeps_generic_missing_glyph_runs_consistent() {
    use cosmic_text::{Attrs, Buffer, Family, Metrics, Shaping, Wrap};
    const ARABIC: &[u8] = include_bytes!("../../../vendor/cosmic-text/fonts/NotoSansArabic.ttf");
    let primary = cosmic_text::ttf_parser::Face::parse(SANS_R, 0).unwrap();
    let fallback = cosmic_text::ttf_parser::Face::parse(ARABIC, 0).unwrap();
    let missing = (0x600..0x900).filter_map(char::from_u32)
        .find(|ch| ch.is_alphabetic() && primary.glyph_index(*ch).is_none() && fallback.glyph_index(*ch).is_some()).expect("real Arabic fixture adds an actually missing character");
    let mut db = fontdb::Database::new();
    let ids = db.load_font_source(fontdb::Source::Binary(Arc::new(SANS_R)));
    let mut loaded = register_loaded_faces(&db, ids.into_iter().map(|id| (id, None, None, None)).collect());
    let mut fs = FontSystem::new_with_locale_and_db("en-US".into(), db);
    let (provider, _) = fixture_provider(ARABIC.to_vec());
    let mut native = NativeFonts::injected(provider, true);
    let generic = resolve_native_font(Some("sans-serif"), 400, false, &mut fs, &mut loaded, &mut native);
    let named = resolve_native_font(Some("'Noto Sans Arabic'"), 400, false, &mut fs, &mut loaded, &mut native);
    native.seal();
    let text = missing.to_string();
    let shape = |fs: &mut FontSystem, font: &ResolvedFont| {
        let attrs = Attrs::new().family(Family::Name(font.family.as_ref())).font_id(font.font_id.unwrap());
        let mut buffer = Buffer::new(fs, Metrics::new(32.0, 40.0));
        buffer.set_wrap(fs, Wrap::None); buffer.set_text(fs, &text, &attrs, Shaping::Advanced); buffer.shape_until_scroll(fs, false);
        buffer.layout_runs().map(|run| (run.line_w, run.glyphs.iter().map(|glyph| (glyph.font_id, glyph.glyph_id)).collect::<Vec<_>>())).collect::<Vec<_>>()
    };
    let before = shape(&mut fs, &generic);
    shape(&mut fs, &named);
    let after = shape(&mut fs, &generic);
    assert_eq!(before, after);
    assert!(before.iter().flat_map(|(_, glyphs)| glyphs).any(|(id, glyph)| native.is_native_face(*id) && *glyph != 0), "normal fallback must actually use the loaded native Arabic resource");
}

#[test]
fn native_canvas_lookup_work_is_bounded_per_request_without_false_absence() {
    for count in [129, 1000] {
        let (provider, signals) = fixture_provider(NOTO.to_vec());
        let (mut fs, mut loaded) = create_font_system(&[], false);
        let mut native = NativeFonts::injected(provider, false);
        let stack = (0..count).map(|n| format!("'Missing {n}'")).chain(["'Noto Sans'".into(), "serif".into()]).collect::<Vec<_>>().join(",");
        let fallback = resolve_native_font(Some(&stack), 400, false, &mut fs, &mut loaded, &mut native);
        assert_eq!(signals.lookup_calls.load(Ordering::Relaxed), 128);
        assert!(native.retry_needed());
        assert!(!native.is_native_face(fallback.font_id.unwrap()));
        assert!(!loaded.contains_key("noto sans"));
        let loaded_tail = stack.replace("'Noto Sans'", "'Liberation Mono'");
        let kept = resolve_native_font(Some(&loaded_tail), 400, false, &mut fs, &mut loaded, &mut native);
        assert_eq!(fs.db().face(kept.font_id.unwrap()).unwrap().post_script_name, "LiberationMono");
        assert!(native.retry_needed());
        assert_eq!(signals.lookup_calls.load(Ordering::Relaxed), 128);
        // An unqueried tail is not negatively cached. A later operation can resolve it.
        let actual = resolve_native_font(Some("'Noto Sans', serif"), 400, false, &mut fs, &mut loaded, &mut native);
        assert!(native.is_native_face(actual.font_id.unwrap()));
        assert!(!native.retry_needed());
        assert_eq!(signals.lookup_calls.load(Ordering::Relaxed), 129);
    }
}


#[test]
fn native_separate_style_files_load_bold_first_and_after_regular() {
    for order in [[(700, false), (400, false), (400, true), (700, true)], [(400, false), (700, false), (400, true), (700, true)]] {
        let (provider, calls) = crate::native_font::fixture_style_provider(vec![MONO_R.to_vec(), MONO_B.to_vec(), MONO_O.to_vec(), MONO_BO.to_vec()]);
        let mut fs = FontSystem::new_with_locale_and_db("en-US".into(), fontdb::Database::new());
        let mut loaded = HashMap::new();
        let mut native = NativeFonts::injected(provider, false);
        for (weight, italic) in order {
            let font = resolve_native_font(Some("'Liberation Mono'"), weight, italic, &mut fs, &mut loaded, &mut native);
            let face = fs.db().face(font.font_id.unwrap()).unwrap();
            assert_eq!(face.weight.0, weight);
            assert_eq!(face.style != fontdb::Style::Normal, italic);
            assert!(!font.synthetic_italic);
        }
        assert_eq!(calls.lookup_calls.load(Ordering::Relaxed), 4);
        assert_eq!(fs.db().faces().count(), 4);
        for (weight, italic) in order {
            resolve_native_font(Some("'Liberation Mono'"), weight, italic, &mut fs, &mut loaded, &mut native);
        }
        assert_eq!(calls.lookup_calls.load(Ordering::Relaxed), 4);
    }
}

#[test]
fn native_style_ttc_reuses_faces_and_explicit_resources_keep_priority() {
    let (provider, calls) = crate::native_font::fixture_style_provider(vec![fixture_collection(&[MONO_R, MONO_B, MONO_O])]);
    let mut fs = FontSystem::new_with_locale_and_db("en-US".into(), fontdb::Database::new());
    let mut loaded = HashMap::new();
    let mut native = NativeFonts::injected(provider.clone(), false);
    for (weight, italic, index) in [(400, false, 0), (700, false, 1), (400, true, 2)] {
        let id = resolve_native_font(Some("'Liberation Mono'"), weight, italic, &mut fs, &mut loaded, &mut native).font_id.unwrap();
        assert_eq!(fs.db().face(id).unwrap().index, index);
    }
    assert_eq!(calls.lookup_calls.load(Ordering::Relaxed), 1);
    assert_eq!(fs.db().faces().count(), 3);
    let (mut fs, mut loaded) = create_font_system(&[], false);
    let mut native = NativeFonts::injected(provider, false);
    for (weight, italic) in [(400, false), (700, false), (400, true)] {
        resolve_native_font(Some("'Liberation Mono'"), weight, italic, &mut fs, &mut loaded, &mut native);
    }
    assert_eq!(calls.lookup_calls.load(Ordering::Relaxed), 1, "bundled real family must never query host");
}

#[test]
fn native_style_failure_retries_and_completed_style_budget_is_bounded() {
    let (provider, calls) = crate::native_font::fixture_style_provider(vec![MONO_R.to_vec(), MONO_B.to_vec()]);
    let mut fs = FontSystem::new_with_locale_and_db("en-US".into(), fontdb::Database::new());
    let mut loaded = HashMap::new();
    let mut native = NativeFonts::injected(provider, false);
    resolve_native_font(Some("'Liberation Mono'"), 400, false, &mut fs, &mut loaded, &mut native);
    calls.fail_next.store(true, Ordering::Relaxed);
    let fallback = resolve_native_font(Some("'Liberation Mono'"), 700, false, &mut fs, &mut loaded, &mut native);
    assert_eq!(fs.db().face(fallback.font_id.unwrap()).unwrap().weight.0, 400);
    assert!(native.retry_needed());
    let bold = resolve_native_font(Some("'Liberation Mono'"), 700, false, &mut fs, &mut loaded, &mut native);
    assert_eq!(fs.db().face(bold.font_id.unwrap()).unwrap().weight.0, 700);
    for weight in 1..=126 {
        resolve_native_font(Some("'Liberation Mono'"), weight, false, &mut fs, &mut loaded, &mut native);
    }
    assert_eq!(native.styles.len(), 128);
    let before = calls.lookup_calls.load(Ordering::Relaxed);
    resolve_native_font(Some("'Liberation Mono'"), 399, true, &mut fs, &mut loaded, &mut native);
    assert!(native.retry_needed());
    assert_eq!(calls.lookup_calls.load(Ordering::Relaxed), before, "style memo exhaustion must not start another host lookup");
    let exact = resolve_native_font(Some("'Liberation Mono'"), 700, false, &mut fs, &mut loaded, &mut native);
    assert_eq!(fs.db().face(exact.font_id.unwrap()).unwrap().weight.0, 700);
}

#[test]
fn incidental_cascade_exact_bold_cannot_override_named_provenance_on_transient_failure() {
    let (provider, calls) = crate::native_font::fixture_cascade_provider(vec![MONO_R.to_vec(),MONO_B.to_vec()],vec![1],32);
    let mut fs=FontSystem::new_with_locale_and_db("en-US".into(),fontdb::Database::new());
    let mut loaded=HashMap::new();let mut native=NativeFonts::injected(provider.clone(),false);
    let regular=resolve_native_font(Some("'Liberation Mono'"),400,false,&mut fs,&mut loaded,&mut native).font_id.unwrap();
    let cascade=provider.cascade(&crate::native_font::FallbackRequest {family:"Liberation Mono",postscript:Some("LiberationMono"),weight:400,italic:false,size:32.0,locale:"en-US"},1).unwrap();
    let incidental=native.canvas_candidate(cascade.candidates.into_iter().next().unwrap(),"W",&mut fs,&mut loaded,&mut||false).unwrap();
    assert_eq!(fs.db().face(incidental).unwrap().post_script_name,"LiberationMono-Bold");
    assert!(!native.named_faces.contains(&incidental));
    calls.fail_next.store(true,Ordering::Relaxed);
    let fallback=resolve_native_font(Some("'Liberation Mono'"),700,false,&mut fs,&mut loaded,&mut native);
    assert_eq!(fallback.font_id,Some(regular),"only the proven Named regular is a legitimate transient fallback");
    assert!(native.retry_needed());assert!(!native.styles.contains_key(&("liberation mono".into(),700,false)));
    assert_eq!(calls.lookup_calls.load(Ordering::Relaxed),2,"incidental exact metadata must not bypass canonical lookup");
    let bold=resolve_native_font(Some("'Liberation Mono'"),700,false,&mut fs,&mut loaded,&mut native);
    assert_eq!(bold.font_id,Some(incidental));assert!(!native.retry_needed());
    assert!(native.named_faces.contains(&incidental));
    assert_eq!(calls.lookup_calls.load(Ordering::Relaxed),3);
    assert_eq!(fs.db().face(bold.font_id.unwrap()).unwrap().weight.0,700);
}

#[test]
fn named_collection_provenance_reuses_exact_siblings_and_retires_with_real_owner() {
    let (provider,calls)=crate::native_font::fixture_style_provider(vec![fixture_collection(&[MONO_R,MONO_B,MONO_O]),NOTO.to_vec()]);
    let mut fs=FontSystem::new_with_locale_and_db("en-US".into(),fontdb::Database::new());
    let mut loaded=HashMap::new();let mut native=NativeFonts::injected(provider,false);native.reduce_canvas_file_limit(1);
    let regular=resolve_native_font(Some("'Liberation Mono'"),400,false,&mut fs,&mut loaded,&mut native).font_id.unwrap();
    let bold=resolve_native_font(Some("'Liberation Mono'"),700,false,&mut fs,&mut loaded,&mut native).font_id.unwrap();
    assert_eq!(calls.lookup_calls.load(Ordering::Relaxed),1);assert_eq!(fs.db().face(bold).unwrap().index,1);
    assert!(native.named_faces.contains(&regular)&&native.named_faces.contains(&bold));
    let noto=resolve_native_font_with_reclaim(Some("'Noto Sans'"),400,false,&mut fs,&mut loaded,&mut native,&mut||true).font_id.unwrap();
    assert!(native.named_faces.contains(&noto));assert!(!native.named_faces.contains(&regular)&&!native.named_faces.contains(&bold));
    assert!(fs.db().face(regular).is_none()&&fs.get_font(bold).is_none());
    let reloaded=resolve_native_font_with_reclaim(Some("'Liberation Mono'"),700,false,&mut fs,&mut loaded,&mut native,&mut||true).font_id.unwrap();
    assert_ne!(reloaded,bold);assert_eq!(fs.db().face(reloaded).unwrap().post_script_name,"LiberationMono-Bold");
    assert_eq!(fs.db().face(reloaded).unwrap().index,1);
}
