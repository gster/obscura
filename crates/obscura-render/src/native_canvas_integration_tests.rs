use super::*;
use crate::{font::with_native_provider_for_test, native_font::fixture_provider};
use std::sync::atomic::Ordering;
const NOTO: &[u8] = include_bytes!("../../../vendor/cosmic-text/fonts/NotoSans-Regular.ttf");

#[test]
fn canvas_native_transient_failure_does_not_poison_identical_last_run() {
    let (provider, signals) = fixture_provider(NOTO.to_vec());
    signals.fail_next.store(true, Ordering::Relaxed);
    with_native_provider_for_test(Some(provider), || {
        let mut engine = CanvasTextEngine::new();
        let font = CanvasFont::parse("32px 'Noto Sans', serif").unwrap();
        engine.shape(&font, "WWWiii").unwrap();
        assert!(engine.last.is_none());
        assert_eq!(signals.lookup_calls.load(Ordering::Relaxed), 1);
        let native = engine.shape(&font, "WWWiii").unwrap();
        assert!(engine.last.is_some());
        assert!(native.glyphs.iter().all(|g| engine.native.is_native_face(g.layout.font_id)));
        assert_eq!(signals.lookup_calls.load(Ordering::Relaxed), 2);
        let cached = engine.shape(&font, "WWWiii").unwrap();
        assert!(Arc::ptr_eq(&native, &cached));
        assert_eq!(signals.lookup_calls.load(Ordering::Relaxed), 2);
    });
}

#[test]
fn canvas_native_emoji_append_preserves_real_native_ids_bytes_and_metrics() {
    let (provider, signals) = fixture_provider(NOTO.to_vec());
    with_native_provider_for_test(Some(provider), || {
        let mut engine = CanvasTextEngine::new();
        let font = CanvasFont::parse("32px 'Noto Sans', serif").unwrap();
        let before = engine.shape(&font, "WWWiii").unwrap();
        let id = before.glyphs[0].layout.font_id;
        let count = engine.font_system.db().faces().count();
        let original = engine.font_system.db().face(id).unwrap().source.clone();
        engine.shape(&font, "A🚀").unwrap();
        assert!(engine.emoji);
        assert_eq!(engine.font_system.db().faces().count(), count + 1);
        let after = engine.shape(&font, "WWWiii").unwrap();
        assert_eq!(before.width, after.width);
        assert!(after.glyphs.iter().all(|g| g.layout.font_id == id));
        match (&original, &engine.font_system.db().face(id).unwrap().source) {
            (cosmic_text::fontdb::Source::Binary(a), cosmic_text::fontdb::Source::Binary(b)) => assert!(Arc::ptr_eq(a, b)),
            _ => panic!("native bytes must remain the same shared source"),
        }
        assert_eq!(signals.lookup_calls.load(Ordering::Relaxed), 1);
    });
}

#[test]
fn canvas_generic_and_loaded_faces_do_zero_native_backend_work() {
    let (provider, signals) = fixture_provider(NOTO.to_vec());
    with_native_provider_for_test(Some(provider), || {
        let mut engine = CanvasTextEngine::new();
        assert_eq!(signals.lookup_calls.load(Ordering::Relaxed), 0);
        for css in ["32px serif", "32px monospace, 'Noto Sans'", "32px 'Liberation Sans', 'Noto Sans'"] {
            let font = CanvasFont::parse(css).unwrap();
            engine.shape(&font, "iiiWWW").unwrap();
        }
        assert_eq!(signals.lookup_calls.load(Ordering::Relaxed), 0);
    });
}


const PRESSURE_HEBREW: &[u8] = include_bytes!("../../../vendor/cosmic-text/fonts/NotoSansHebrew.ttf");
const PRESSURE_ARABIC: &[u8] = include_bytes!("../../../vendor/cosmic-text/fonts/NotoSansArabic.ttf");

fn pressure_provider() -> Arc<crate::native_font::Provider> {
    crate::native_font::fixture_cascade_provider(vec![NOTO.to_vec(), PRESSURE_HEBREW.to_vec(), PRESSURE_ARABIC.to_vec()], vec![2,1,0], 32).0
}
fn pressure_pixels(engine: &mut CanvasTextEngine, font: &CanvasFont, text: &str) -> Vec<u8> {
    let mut pixels=vec![0;320*80*4];
    engine.draw(font,text,TextPaint { reference: TextReference {align:"left",baseline:"alphabetic",rtl:false},
        x:8.0,y:52.0,color:[30,70,160,255],alpha:1.0,stroke_width:Some(1.0),max_width:Some(250.0) },&mut pixels,320,80).unwrap();
    assert!(pixels.chunks_exact(4).any(|pixel| pixel[3]!=0)); pixels
}

#[test]
fn canvas_native_pressure_cycles_real_files_with_metrics_pixels_and_new_ids() {
    with_native_provider_for_test(Some(pressure_provider()), || {
        let cases=[("Noto Sans","WWWiii"),("Noto Sans Hebrew","שלום"),("Noto Sans Arabic","سلام")];
        let mut expected=Vec::new();
        for (name,text) in cases {
            let font=CanvasFont::parse(&format!("32px '{name}',serif")).unwrap();
            let mut fresh=CanvasTextEngine::new();
            let run=fresh.shape(&font,text).unwrap();
            assert!(run.glyphs.iter().all(|glyph| glyph.layout.glyph_id!=0 && fresh.native.is_native_face(glyph.layout.font_id)));
            expected.push((font,run.width,run.ink,pressure_pixels(&mut fresh,&CanvasFont::parse(&format!("32px '{name}',serif")).unwrap(),text)));
        }
        let mut engine=CanvasTextEngine::new();engine.native.reduce_canvas_file_limit(1);
        let mut retired=Vec::new();
        for cycle in 0..32 {
            for offset in 0..3 {
                let index=if cycle%2==0 {offset}else{2-offset};
                let (font,width,ink,pixels)=&expected[index];let text=cases[index].1;
                let run=engine.shape(font,text).unwrap();
                assert_eq!(run.width,*width);assert_eq!(run.ink,*ink);
                assert!(run.glyphs.iter().all(|glyph| glyph.layout.glyph_id!=0 && engine.native.is_native_face(glyph.layout.font_id)));
                for id in &retired { assert!(engine.font_system.db().face(*id).is_none()); }
                retired=run.glyphs.iter().map(|glyph| glyph.layout.font_id).collect();
                assert_eq!(pressure_pixels(&mut engine,font,text),*pixels);
                let (files,bytes,faces)=engine.native.resident_counts();
                assert_eq!(files,1);assert!(bytes<=32<<20);assert!(faces<=256);
                // Permit the deliberate repeated edge between forward/reverse cycles.
                if offset==2 { retired.clear(); }
            }
        }
    });
}

#[test]
fn canvas_native_eviction_releases_private_font_caches_but_keeps_other_engine_owner() {
    with_native_provider_for_test(Some(pressure_provider()), || {
        let a=CanvasFont::parse("32px 'Noto Sans',serif").unwrap();
        let b=CanvasFont::parse("32px 'Noto Sans Hebrew',serif").unwrap();
        let mut keeper=CanvasTextEngine::new();let saved=keeper.shape(&a,"WWWiii").unwrap();
        let keeper_pixels=pressure_pixels(&mut keeper,&a,"WWWiii");
        let mut engine=CanvasTextEngine::new();engine.native.reduce_canvas_file_limit(1);
        let old=engine.shape(&a,"WWWiii").unwrap().glyphs[0].layout.font_id;
        let old_bytes=match &engine.font_system.db().face(old).unwrap().source {
            cosmic_text::fontdb::Source::Binary(bytes)=>Arc::downgrade(bytes),_=>panic!("real source required")
        };
        pressure_pixels(&mut engine,&a,"WWWiii");engine.shape(&b,"שלום").unwrap();
        assert!(engine.font_system.db().face(old).is_none());
        assert!(engine.font_system.get_font(old).is_none());
        assert!(old_bytes.upgrade().is_some(),"separate live consumer owns the actual bytes");
        assert_eq!(keeper.shape(&a,"WWWiii").unwrap().width,saved.width);
        assert_eq!(pressure_pixels(&mut keeper,&a,"WWWiii"),keeper_pixels);
    });
}

// A genuine Arabic cmap witness must work cold, after Named import and after eviction.
#[test]
fn canvas_generic_missing_cmap_support_must_survive_named_cache_pressure() {
    with_native_provider_for_test(Some(pressure_provider()), || {
        let mut engine=CanvasTextEngine::new();engine.native.reduce_canvas_file_limit(1);
        let face=cosmic_text::ttf_parser::Face::parse(PRESSURE_ARABIC,0).unwrap();
        let witness=(0x600..=0x8ff).filter_map(char::from_u32).find(|ch|
            ch.is_alphabetic() && face.glyph_index(*ch).is_some() && engine.font_system.db().faces().all(|candidate|
                engine.font_system.db().with_face_data(candidate.id,|bytes,index|
                    cosmic_text::ttf_parser::Face::parse(bytes,index).unwrap().glyph_index(*ch).is_none()).unwrap()))
            .expect("fixture must contain an actual cmap entry absent from every base face");
        let text=witness.to_string();let generic=CanvasFont::parse("32px serif").unwrap();
        let cold = canvas_snapshot(&mut engine, &generic, &text);
        engine.shape(&CanvasFont::parse("32px 'Noto Sans Arabic'").unwrap(),&text).unwrap();
        assert_eq!(canvas_snapshot(&mut engine, &generic, &text), cold);
        let supported=engine.shape(&generic,&text).unwrap();
        assert!(!supported.glyphs.is_empty() && supported.glyphs.iter().any(|glyph|engine.native.is_native_face(glyph.layout.font_id))
            && supported.glyphs.iter().all(|glyph|glyph.layout.glyph_id!=0),"import must actually establish fallback support");
        engine.shape(&CanvasFont::parse("32px 'Noto Sans'").unwrap(),"WWWiii").unwrap();
        let after=engine.shape(&generic,&text).unwrap();
        assert!(!after.glyphs.is_empty() && after.glyphs.iter().all(|glyph|glyph.layout.glyph_id!=0),"eviction regressed actual generic glyph support");
        assert_eq!(after.width,supported.width);
        assert_eq!(canvas_snapshot(&mut engine, &generic, &text), cold);
        for reverse in [false, true] {
            for i in 0..32 {
                let name = if (i % 2 == 0) ^ reverse { "Noto Sans" } else { "Noto Sans Hebrew" };
                engine.shape(&CanvasFont::parse(&format!("32px '{name}'")).unwrap(), "abc").unwrap();
                assert_eq!(canvas_snapshot(&mut engine, &generic, &text), cold);
            }
        }
    });
}


#[test]
fn canvas_128th_real_candidate_survives_local_and_provider_capacity_without_relookup() {
    for provider_files in [32,1] {
        let (provider,calls)=crate::native_font::fixture_style_provider_with_file_limit(
            vec![NOTO.to_vec(),PRESSURE_HEBREW.to_vec()],provider_files);
        with_native_provider_for_test(Some(provider),||{
            let mut engine=CanvasTextEngine::new();engine.native.reduce_canvas_file_limit(1);
            engine.shape(&CanvasFont::parse("32px 'Noto Sans Hebrew'").unwrap(),"שלום").unwrap();
            let before=calls.lookup_calls.load(Ordering::Relaxed);
            let stack=(0..127).map(|n|format!("'Missing pressure {n}'"))
                .chain(["'Noto Sans'".into(),"serif".into()]).collect::<Vec<_>>().join(",");
            let font=CanvasFont::parse(&format!("32px {stack}")).unwrap();
            let run=engine.shape(&font,"WWWiii").unwrap();
            assert!(!run.glyphs.is_empty() && run.glyphs.iter().all(|glyph|engine.native.is_native_face(glyph.layout.font_id)));
            assert!(run.glyphs.iter().all(|glyph|engine.font_system.db().face(glyph.layout.font_id).unwrap().post_script_name=="NotoSans-Regular"));
            assert_eq!(calls.lookup_calls.load(Ordering::Relaxed)-before,128,"never replay missing prefixes or the found/located 128th candidate");
            assert_eq!(engine.native.resident_counts().0,1);
        });
    }
}

#[test]
fn canvas_hot_run_touches_each_distinct_physical_owner_once_independent_of_glyph_count() {
    with_native_provider_for_test(Some(pressure_provider()),||{
        let mut engine=CanvasTextEngine::new();
        let font=CanvasFont::parse("16px 'Noto Sans'").unwrap();
        let text="W".repeat(4096);let cold=engine.shape(&font,&text).unwrap();
        assert_eq!(cold.glyphs.len(),4096);assert_eq!(cold.native_owners.len(),1);
        let before=engine.native.owner_touch_count();
        for _ in 0..64 {
            let hot=engine.shape(&font,&text).unwrap();assert!(Arc::ptr_eq(&hot,&cold));
        }
        assert_eq!(engine.native.owner_touch_count()-before,64);
        assert_eq!(engine.native.resident_counts().0,1);
    });
}

// Physical identity is derived from the actual bytes, collection index and real
// metadata, independent of the ephemeral fontdb ID. Axis/glyph placement is exact.
#[derive(Debug, PartialEq)]
struct CanvasSnapshot {
    metrics: [f32; 7],
    glyphs: Vec<(u64, u32, String, u16, u16, bool, String, [f32; 5])>,
    pixels: Vec<u8>,
}
fn canvas_snapshot(engine: &mut CanvasTextEngine, font: &CanvasFont, text: &str) -> CanvasSnapshot {
    use std::hash::{Hash, Hasher};
    let run = engine.shape(font, text).unwrap();
    assert!(!run.glyphs.is_empty());
    let glyphs = run.glyphs.iter().map(|g| {
        assert_ne!(g.layout.glyph_id, 0, "actual shaped glyph required: {text:?}");
        let face = engine.font_system.db().face(g.layout.font_id).unwrap();
        let fingerprint = engine.font_system.db().with_face_data(g.layout.font_id, |bytes, _| {
            let mut hash = std::collections::hash_map::DefaultHasher::new(); bytes.hash(&mut hash); hash.finish()
        }).unwrap();
        (fingerprint, face.index, face.post_script_name.clone(), face.weight.0, g.layout.glyph_id,
            face.style == cosmic_text::fontdb::Style::Italic, format!("{:?}", g.variations),
            [g.layout.x, g.layout.y, g.layout.w, g.layout.x_offset, g.layout.y_offset])
    }).collect();
    let m = run.metrics(TextReference { align:"left",baseline:"alphabetic",rtl:false }).unwrap();
    CanvasSnapshot { metrics:[m.width,m.left,m.right,m.ascent,m.descent,m.font_ascent,m.font_descent],
        glyphs, pixels:pressure_pixels(engine,font,text) }
}

#[test]
fn canvas_operation_order_uses_real_distinguishable_fallbacks_after_import_and_churn() {
    const DEJAVU: &[u8] = include_bytes!("../assets/dejavu-sans.ttf");
    let (provider, signals) = crate::native_font::fixture_cascade_provider(
        vec![PRESSURE_ARABIC.to_vec(),DEJAVU.to_vec(),NOTO.to_vec(),PRESSURE_HEBREW.to_vec()],vec![0,1],32);
    with_native_provider_for_test(Some(provider), || {
        // Remove only the test's bundled Arabic fallback resources, so two authentic
        // distinct resources can both compete through the injected configured cascade.
        fn engine() -> CanvasTextEngine {
            let mut e = CanvasTextEngine::new();
            let ids: Vec<_> = e.font_system.db().faces().filter(|f| f.families.iter().any(|(name,_)| name == "DejaVu Sans")).map(|f|f.id).collect();
            e.font_system.remove_font_faces(&ids); e.base_font_ids.retain(|id|!ids.contains(id));
            e.families.retain(|_,f| { f.faces.retain(|face|!face.font_id.is_some_and(|id|ids.contains(&id)));!f.faces.is_empty() });
            e.native.reduce_canvas_file_limit(2); e
        }
        let text="سلام سَلَام"; let generic=CanvasFont::parse("32px serif").unwrap();
        let mut e=engine();let cold=canvas_snapshot(&mut e,&generic,text);
        assert!(cold.glyphs.iter().any(|g|g.2.contains("NotoSansArabic")));
        let other=canvas_snapshot(&mut e,&CanvasFont::parse("32px 'DejaVu Sans'").unwrap(),text);
        assert!(other.glyphs.iter().any(|g|g.2.contains("DejaVuSans")));
        assert_ne!(other.pixels,cold.pixels,"second covering resource must be distinguishable");
        assert_eq!(canvas_snapshot(&mut e,&generic,text),cold);
        for reverse in [false,true] {
            for i in 0..32 {
                let name=if (i%2==0)^reverse {"Noto Sans"} else {"Noto Sans Hebrew"};
                e.shape(&CanvasFont::parse(&format!("32px '{name}'")).unwrap(),"abc").unwrap();
            }
            assert_eq!(canvas_snapshot(&mut e,&generic,text),cold);
        }
        let before=signals.cascade_calls.load(Ordering::Relaxed);
        let full=CanvasFont::parse("32px 'Noto Sans'").unwrap();
        e.shape(&full,"WWWiii").unwrap();e.shape(&full,"WWWiiii").unwrap();
        assert_eq!(signals.cascade_calls.load(Ordering::Relaxed),before,"fully covered primary never queries OS fallback");
    });
}

#[test]
fn canvas_failed_registration_leaves_no_ghost_ids_admission_or_owner() {
    with_native_provider_for_test(Some(pressure_provider()), || {
        let mut e=CanvasTextEngine::new();let count=e.font_system.db().faces().count();
        e.native.fail_next_registration();let font=CanvasFont::parse("32px 'Noto Sans',serif").unwrap();
        e.shape(&font,"WWWiii").unwrap();
        assert_eq!(e.native.resident_counts(),(0,0,0));
        assert_eq!(e.font_system.db().faces().count(),count);
        assert!(!e.families.contains_key("noto sans"));assert!(e.last.is_none());
        let run=e.shape(&font,"WWWiii").unwrap();
        assert!(run.glyphs.iter().all(|g|e.native.is_native_face(g.layout.font_id)));
        assert_eq!(e.native.resident_counts().0,1);
    });
}

#[test]
fn canvas_operation_pins_prevent_primary_eviction_during_fallback_capacity() {
    let (provider,_) = crate::native_font::fixture_cascade_provider(vec![NOTO.to_vec(),PRESSURE_ARABIC.to_vec()],vec![1],32);
    with_native_provider_for_test(Some(provider),||{
        let mut e=CanvasTextEngine::new();e.native.reduce_canvas_file_limit(1);
        let font=CanvasFont::parse("32px 'Noto Sans'").unwrap();
        let initial=e.shape(&font,"abc").unwrap().glyphs[0].layout.font_id;
        let run=e.shape(&font,"abc \u{620}").unwrap();
        assert!(e.font_system.db().face(initial).is_some());
        assert!(run.glyphs.iter().filter(|g|g.layout.start<3).all(|g|g.layout.font_id==initial));
        assert!(e.native.retry_needed());assert!(e.last.is_none());
        assert_eq!(e.native.resident_counts().0,1);
    });
}

#[cfg(feature="paint")]
#[test]
fn actual_prepared_render_repaints_identically_while_canvas_reclaims_shared_files() {
    let (provider,signals)=crate::native_font::fixture_cascade_provider(vec![NOTO.to_vec(),PRESSURE_HEBREW.to_vec(),PRESSURE_ARABIC.to_vec()],vec![2,1],32);
    with_native_provider_for_test(Some(provider),||{
        let tree=obscura_dom::parse_html("<p id=p style=\"font:32px 'Noto Sans'\">Native WWWiii</p>");
        let mut resources=crate::RenderResourceCache::default();
        let mut prepared=crate::paint::prepare_dom(&tree,(400.0,120.0),None,&mut resources).unwrap();
        assert!(signals.lookup_calls.load(Ordering::Relaxed)>0,"fixture must really request its Native Named resource");
        let id=tree.get_element_by_id("p").unwrap();
        let rect=prepared.layout().rects[&id];
        let before=crate::paint::paint_prepared(&tree,&mut prepared,&mut resources,(0.0,0.0)).unwrap().data().to_vec();
        let mut e=CanvasTextEngine::new();e.native.reduce_canvas_file_limit(1);
        for i in 0..32 { let name=if i%2==0 {"Noto Sans Arabic"} else {"Noto Sans Hebrew"};
            e.shape(&CanvasFont::parse(&format!("32px '{name}'")).unwrap(),"abc").unwrap(); }
        let after=crate::paint::paint_prepared(&tree,&mut prepared,&mut resources,(0.0,0.0)).unwrap().data().to_vec();
        assert_eq!(before,after);assert_eq!(prepared.layout().rects[&id],rect);
    });
}

#[cfg(target_os="macos")]
#[test]
#[ignore="explicit local public-font 17/32 physical-file pressure oracle"]
fn actual_macos_32_physical_fonts_are_order_independent_with_exact_metrics_glyphs_and_pixels() {
    use std::hash::{Hash,Hasher};
    let provider=Arc::new(crate::native_font::Provider::new());
    let names=["American Typewriter","Andale Mono","Apple Chancery","Arial","Arial Black","Arial Narrow","Arial Rounded MT Bold",
        "Baskerville","Big Caslon","Bradley Hand","Brush Script MT","Chalkboard","Chalkboard SE","Chalkduster",
        "Cochin","Comic Sans MS","Copperplate","Courier","Courier New","Didot","Futura","Geneva","Georgia",
        "Gill Sans","Helvetica","Helvetica Neue","Herculanum","Hoefler Text","Impact","Lucida Grande","Luminari",
        "Marker Felt","Menlo","Monaco","Noteworthy","Optima","Palatino","Papyrus","Phosphate","Rockwell",
        "Savoye LET","SignPainter","Skia","Snell Roundhand","Tahoma","Times","Times New Roman","Trattatello",
        "Trebuchet MS","Verdana","Zapfino"];
    let mut cases=Vec::new();let mut sources=Vec::new();let mut physical=std::collections::HashSet::new();
    'families: for name in names {
        for (weight,italic) in [(400,false),(700,false),(400,true)] {
            let crate::native_font::Lookup::Found(asset)=provider.lookup_styled(name,weight,italic) else {continue};
            let bytes=asset.bytes();
            let face=cosmic_text::ttf_parser::Face::parse(bytes.as_ref().as_ref(),asset.selected_index).unwrap();
            if !"WWWiii0123".chars().all(|c|face.glyph_index(c).is_some()) {continue;}
            let mut hash=std::collections::hash_map::DefaultHasher::new();bytes.as_ref().as_ref().hash(&mut hash);
            let fingerprint=hash.finish();
            if !physical.insert(fingerprint) {continue;}
            sources.push((fingerprint,asset.selected_index,asset.faces().iter().find(|f|f.index==asset.selected_index).unwrap().postscript.clone()));
            cases.push(CanvasFont{family:format!("'{name}'"),size:32.0,weight,italic});
            if cases.len()==32 {break 'families;}
        }
    }
    assert_eq!(cases.len(),32,"host must provide 32 admissible distinct real physical files; failure is not a skip");
    with_native_provider_for_test(Some(provider),||{
        let expected:Vec<_>=cases.iter().zip(&sources).map(|(font,source)| {
            let snapshot=canvas_snapshot(&mut CanvasTextEngine::new(),font,"WWWiii0123");
            assert!(snapshot.glyphs.iter().all(|g|g.0==source.0&&g.1==source.1&&g.2==source.2),"cold case must use the exact real selected resource");
            snapshot
        }).collect();
        for count in [17,32] {
            let mut e=CanvasTextEngine::new();
            for reverse in [false,true,false] {
                for offset in 0..count {
                    let i=if reverse {count-1-offset}else{offset};
                    assert_eq!(canvas_snapshot(&mut e,&cases[i],"WWWiii0123"),expected[i],"index {i}, reverse {reverse}");
                    let (files,bytes,faces)=e.native.resident_counts();
                    assert!(files<=16&&bytes<=32<<20&&faces<=256);
                }
                assert_eq!(canvas_snapshot(&mut e,&cases[0],"WWWiii0123"),expected[0],"repeat first after eviction");
            }
        }
    });
}

#[test]
fn canvas_missing_combining_cluster_copies_each_real_input_byte_at_most_once() {
    let (mut fs, _) = create_font_system(&[], false);
    let mark=(0x1ab0..=0x1ace).filter_map(char::from_u32).find(|ch|fs.db().faces().all(|face|
        fs.db().with_face_data(face.id,|bytes,index|cosmic_text::ttf_parser::Face::parse(bytes,index).unwrap().glyph_index(*ch).is_none()).unwrap()))
        .expect("fixture needs a real combining mark absent from all base resources");
    let text=format!("a{}",mark.to_string().repeat(1024));
    let buffer=shape_canvas_buffer(&mut fs,32.0,&text,&Attrs::new().family(Family::Serif));
    let zeros:Vec<_>=buffer.layout_runs().flat_map(|line|line.glyphs.iter().filter(|g|g.glyph_id==0).map(|g|(g.start,g.end))).collect();
    assert!(zeros.len()>1,"must really shape multiple missing glyphs");
    assert!(zeros.windows(2).any(|pair|pair[0]==pair[1]&&pair[0].1-pair[0].0>4),"must reproduce a shared long shaping cluster");
    let (count,missing)=canvas_missing(&buffer);
    assert_eq!(count,zeros.len());assert!(!missing.is_empty());
    assert!(missing.len()<=text.len(),"cluster prefilter must remain bounded by actual input bytes");
}

#[test]
fn canvas_cmap_rejected_cascade_does_not_reshape_long_input_per_candidate() {
    let (provider,signals)=crate::native_font::fixture_cascade_provider(
        vec![NOTO.to_vec(),PRESSURE_HEBREW.to_vec(),PRESSURE_ARABIC.to_vec()],(0..128).map(|i|i%3).collect(),32);
    with_native_provider_for_test(Some(provider),||{
        let mut e=CanvasTextEngine::new();let font=CanvasFont::parse("32px serif").unwrap();
        let text=format!("{}\u{10fffd}","a".repeat(16384));
        let before=e.shaping_passes;
        let run=e.shape(&font,&text).unwrap();
        assert!(run.glyphs.iter().any(|g|g.layout.glyph_id==0),"real unsupported input must remain honestly missing");
        assert_eq!(signals.cascade_calls.load(Ordering::Relaxed),1);
        assert_eq!(e.shaping_passes-before,2,"one initial and one final shape, independent of 128 rejected real candidates");
    });
}
