//! Actual DOM consumer tests. Injection supplies unmodified real font resources;
//! these do not depend on an installed Mac family or rename fixture metadata.
use super::*;
use crate::font::with_native_provider_for_test;
use crate::native_font::{fixture_collection, fixture_provider};
use std::sync::atomic::Ordering;

const NOTO: &[u8] = include_bytes!("../../../vendor/cosmic-text/fonts/NotoSans-Regular.ttf");
const HEBREW: &[u8] = include_bytes!("../../../vendor/cosmic-text/fonts/NotoSansHebrew.ttf");
const ARABIC: &[u8] = include_bytes!("../../../vendor/cosmic-text/fonts/NotoSansArabic.ttf");

fn glyphs(engine: &TextEngine, item: usize) -> Vec<(cosmic_text::fontdb::ID, u16, f32)> {
    engine.items[item].buffer.layout_runs().flat_map(|run| run.glyphs.iter()
        .map(|glyph| (glyph.font_id, glyph.glyph_id, glyph.w))).collect()
}

#[test]
fn native_dom_only_rendered_text_consumers_request_fonts() {
    let (provider, signals) = fixture_provider(NOTO.to_vec());
    with_native_provider_for_test(Some(provider), || {
        let tree = obscura_dom::parse_html(r#"<!doctype html><style>
            .unused { font-family:'Noto Sans' } #hidden {display:none;font-family:'Noto Sans'}
            #box {font-family:'Noto Sans';width:10px;height:10px}
            </style><p style="font-family:sans-serif">Normal generic text</p>
            <div id=hidden><span>Hidden text</span></div><div id=box></div>
            <ul><li style="list-style:none;font-family:'Noto Sans'"></li></ul>"#);
        let _ = crate::dom::layout_dom(&tree, (600.0, 200.0));
        assert_eq!(signals.lookup_calls.load(Ordering::Relaxed), 0);
    });
}

#[test]
fn native_dom_pure_mixed_word_and_generated_text_share_physical_resource() {
    let html = r#"<!doctype html><style>
        body {font-family:'Noto Sans';font-size:28px;margin:0}
        #pseudo::before {content:'Native generated';font-family:'Noto Sans'}
        </style><div id=pure>MMMMiiiiWW</div>
        <div id=mixed>Mixed first<div>Block</div>Mixed last</div>
        <div id=words>Word <span style="display:inline-block;width:5px;height:5px"></span> tail</div>
        <div id=pseudo></div>"#;
    let (provider, signals) = fixture_provider(NOTO.to_vec());
    with_native_provider_for_test(Some(provider), || {
        let tree = obscura_dom::parse_html(html);
        let mut laid = crate::dom::layout_dom(&tree, (700.0, 400.0));
        let pure = tree.get_element_by_id("pure").unwrap();
        let mixed = tree.get_element_by_id("mixed").unwrap();
        let pseudo = tree.get_element_by_id("pseudo").unwrap();
        assert!(laid.ifc_items.contains_key(&pure));
        assert!(laid.run_ifc_items.get(&mixed).is_some_and(|items| !items.is_empty()));
        assert!(!laid.word_ifc_items.is_empty(), "atomic inline must exercise word shaping");
        assert!(laid.rects[&pseudo].height > 0.0);
        let selected = laid.text_engine.prepared_font(&laid.styles[&pure]).font_id.unwrap();
        assert!(laid.text_engine.native_fonts.is_native_face(selected));
        assert!(laid.text_engine.font_system.db().face(selected).unwrap().families.iter().any(|(name, _)| name == "Noto Sans"));
        for item in 0..laid.text_engine.items.len() {
            for (_, glyph, _) in glyphs(&laid.text_engine, item) { assert_ne!(glyph, 0); }
        }
        assert!(laid.text_engine.items.iter().flat_map(|item| item.buffer.layout_runs())
            .flat_map(|run| run.glyphs).any(|glyph| glyph.font_id == selected));
        let before = signals.lookup_calls.load(Ordering::Relaxed);
        let item = laid.ifc_items[&pure];
        let mut pixels = tiny_skia::Pixmap::new(700, 100).unwrap();
        laid.text_engine.paint_item(item, &mut pixels, (0.0, 0.0));
        assert!(pixels.data().chunks_exact(4).any(|pixel| pixel[3] != 0));
        assert_eq!(signals.lookup_calls.load(Ordering::Relaxed), before, "paint must never request a font");
    });
}

#[test]
fn native_dom_br_empty_inline_controls_and_marker_prepare_without_text_nodes() {
    let (provider, signals) = fixture_provider(NOTO.to_vec());
    with_native_provider_for_test(Some(provider), || {
        let tree = obscura_dom::parse_html(r#"<!doctype html><style>
            .native {font-family:'Noto Sans';font-size:31px;line-height:normal}
            </style><div><br id=br class=native></div>
            <p><span id=empty class=native></span>x</p>
            <input id=input class=native value=""><textarea id=area class=native></textarea>
            <select id=select class=native><option>Wide option</option></select>
            <ol><li id=marker class=native></li></ol>"#);
        let laid = crate::dom::layout_dom(&tree, (700.0, 500.0));
        assert!(signals.lookup_calls.load(Ordering::Relaxed) > 0);
        for name in ["br", "empty", "input", "area", "select", "marker"] {
            let id = tree.get_element_by_id(name).unwrap();
            let style = &laid.styles[&id];
            assert!(laid.text_engine.has_prepared_native_face(style), "{name}");
            let native_height = laid.text_engine.selected_line_height(style);
            assert!(native_height > 31.0, "real Noto normal metrics are not a guessed em");
            if matches!(name, "input" | "area" | "select") { assert!(laid.rects[&id].height >= native_height, "{name}"); }
        }
    });
}

#[test]
fn native_dom_missing_cmap_fallback_is_stable_across_later_named_consumer() {
    let (base, _) = crate::font::create_font_system(&[], false);
    let fallback = cosmic_text::ttf_parser::Face::parse(ARABIC, 0).unwrap();
    let missing = (0x600..0x900).filter_map(char::from_u32)
        .find(|ch| ch.is_alphabetic() && fallback.glyph_index(*ch).is_some()
            && base.db().faces().all(|face| !base.db().with_face_data(face.id, |bytes, index| {
                cosmic_text::ttf_parser::Face::parse(bytes, index).ok()
                    .is_some_and(|font| font.glyph_index(*ch).is_some())
            }).unwrap_or(false)))
        .expect("native fixture must add an alphabetic glyph absent from every actual base face");
    let (provider, signals) = fixture_provider(ARABIC.to_vec());
    with_native_provider_for_test(Some(provider), || {
        let tree = obscura_dom::parse_html(&format!("<!doctype html><style>p{{font-size:32px;font-family:sans-serif}}</style><p id=first>{missing}</p><p id=named style=\"font-family:'Noto Sans Arabic'\">{missing}</p><p id=last>{missing}</p>"));
        let laid = crate::dom::layout_dom(&tree, (600.0, 300.0));
        let first = laid.ifc_items[&tree.get_element_by_id("first").unwrap()];
        let last = laid.ifc_items[&tree.get_element_by_id("last").unwrap()];
        let named = laid.ifc_items[&tree.get_element_by_id("named").unwrap()];
        let before = glyphs(&laid.text_engine, first);
        assert!(!before.is_empty());
        assert!(before.iter().all(|(_, glyph, _)| *glyph != 0));
        assert!(before.iter().any(|(id, glyph, _)| laid.text_engine.native_fonts.is_native_face(*id) && *glyph != 0),
            "generic DOM text must actually fall back to the newly loaded native face");
        assert_eq!(before, glyphs(&laid.text_engine, last), "actual fallback IDs and advances must not depend on traversal order");
        assert!(glyphs(&laid.text_engine, named).iter().any(|(id, glyph, _)| laid.text_engine.native_fonts.is_native_face(*id) && *glyph != 0));
        assert!(signals.lookup_calls.load(Ordering::Relaxed) > 0);
    });
}

#[test]
fn native_dom_legacy_ttc_measure_and_paint_keep_face_index_and_existing_items() {
    let (provider, signals) = fixture_provider(fixture_collection(&[NOTO, HEBREW]));
    with_native_provider_for_test(Some(provider), || {
        let mut engine = TextEngine::new();
        let style = LayoutStyle { font_family: Some("'Noto Sans Hebrew'".into()), font_size: Some(32.0), color: Some([0,0,0,255]), ..Default::default() };
        engine.prepare_style(&style);
        engine.seal_native_preparation();
        let selected = engine.prepared_font(&style).font_id.unwrap();
        assert_eq!(engine.font_system.db().with_face_data(selected, |bytes, index| {
            assert!(cosmic_text::ttf_parser::Face::parse(bytes, index).unwrap().glyph_index('ש').is_some()); index
        }), Some(1));
        let text = "שלום";
        let canonical = TextEngine::legacy_native_style(&style, 32.0, 0.0);
        let existing = engine.push_generated_text(text, &canonical).unwrap();
        let expected = engine.measure(existing, None).0;
        let before_glyphs = glyphs(&engine, existing);
        let before_items = engine.len();
        let before_calls = signals.lookup_calls.load(Ordering::Relaxed);
        let width = engine.measure_prepared_native_text(text, &style, 32.0, 0.0).unwrap();
        assert!((expected - width).abs() < 0.001);
        let mut expected_pixels = tiny_skia::Pixmap::new(300, 100).unwrap();
        engine.finalize(existing, (2.0, 2.0), expected, None);
        engine.paint_item(existing, &mut expected_pixels, (0.0, 0.0));
        let mut actual_pixels = tiny_skia::Pixmap::new(300, 100).unwrap();
        assert!(engine.paint_prepared_native_text(text, &style, (2.0,2.0), [0,0,0,255], 32.0, 0.0, None, &mut actual_pixels, None, 1.0));
        assert_eq!(expected_pixels.data(), actual_pixels.data());
        assert!(actual_pixels.data().chunks_exact(4).any(|pixel| pixel[3] != 0));
        assert_eq!(engine.len(), before_items);
        assert_eq!(glyphs(&engine, existing), before_glyphs);
        assert_eq!(signals.lookup_calls.load(Ordering::Relaxed), before_calls);
    });
}

#[test]
fn native_dom_repeated_full_paint_never_imports_and_matches_resource_output() {
    let (provider, signals) = fixture_provider(NOTO.to_vec());
    with_native_provider_for_test(Some(provider), || {
        let tree = obscura_dom::parse_html("<!doctype html><body style=\"font-family:'Noto Sans';font-size:28px\"><p>Font pixels WWWiii</p><ol><li>Marker</li></ol><select style=\"font-family:'Noto Sans';font-size:28px\"><option>Option</option></select></body>");
        let mut resources = crate::paint::RenderResourceCache::default();
        let mut prepared = crate::paint::prepare_dom(&tree, (500.0,300.0), None, &mut resources).unwrap();
        let before = signals.lookup_calls.load(Ordering::Relaxed);
        assert!(before > 0);
        let first = crate::paint::paint_prepared(&tree, &mut prepared, &mut resources, (0.0,0.0)).unwrap();
        let second = crate::paint::paint_prepared(&tree, &mut prepared, &mut resources, (0.0,0.0)).unwrap();
        assert_eq!(first.data(), second.data());
        assert!(first.data().chunks_exact(4).any(|pixel| pixel[0] < 200));
        assert_eq!(signals.lookup_calls.load(Ordering::Relaxed), before);
        let disabled = with_native_provider_for_test(None, || crate::paint::paint_dom(&tree, (500.0,300.0), None).unwrap());
        assert_ne!(first.data(), disabled.data(), "real native bytes must reach DOM pixels, not merely a registry");
    });
}

#[cfg(target_os = "macos")]
#[test]
#[ignore = "explicit macOS Arial installed public resources; run old/new separately"]
fn native_dom_arial_physical_styles_survive_both_consumer_orders() {
    for first in ["", "<p style='font-weight:400'>First normal consumer</p>"] {
        let html = format!(r#"<!doctype html><style>
            body {{font:18px Arial,sans-serif}}
            p {{font-size:32px}}
            </style>{first}<h1 id=heading>Live control state</h1>
            <p id=regular style="font-weight:400;font-style:normal">Hamburgefontsiv WWW iii</p>
            <p id=bold style="font-weight:700;font-style:normal">Hamburgefontsiv WWW iii</p>
            <p id=italic style="font-weight:400;font-style:italic">Hamburgefontsiv WWW iii</p>
            <p id=bolditalic style="font-weight:700;font-style:italic">Hamburgefontsiv WWW iii</p>"#);
        let tree = obscura_dom::parse_html(&html);
        let laid = crate::dom::layout_dom(&tree, (900.0, 1000.0));
        for (name, weight, italic) in [("heading",700,false),("regular",400,false),
            ("bold",700,false),("italic",400,true),("bolditalic",700,true)] {
            let node = tree.get_element_by_id(name).unwrap();
            let style = &laid.styles[&node];
            assert_eq!(crate::style::used_font_weight(style), weight);
            assert_eq!(style.font_style_italic.unwrap_or(false), italic);
            let item = laid.ifc_items[&node];
            let glyphs = laid.text_engine.items[item].buffer.layout_runs()
                .flat_map(|run| run.glyphs.iter()).collect::<Vec<_>>();
            assert!(!glyphs.is_empty());
            for glyph in glyphs {
                assert_ne!(glyph.glyph_id, 0);
                let face = laid.text_engine.font_system.db().face(glyph.font_id).unwrap();
                assert!(face.families.iter().any(|(family, _)| family == "Arial"));
                assert_eq!(face.weight.0, weight, "{name} must use its physical face");
                assert_eq!(face.style != cosmic_text::Style::Normal, italic, "{name}");
            }
        }
    }
}

#[test]
fn prepared_dom_native_glyph_ids_and_owners_survive_canvas_eviction_and_exit() {
    let (provider, _) = crate::native_font::fixture_cascade_provider(vec![NOTO.to_vec(),HEBREW.to_vec(),ARABIC.to_vec()],vec![2,1],32);
    with_native_provider_for_test(Some(provider),||{
        let tree=obscura_dom::parse_html("<p id=p style=\"font:32px 'Noto Sans'\">Native WWWiii</p>");
        let mut resources=crate::RenderResourceCache::default();
        let mut prepared=crate::paint::prepare_dom(&tree,(400.0,120.0),None,&mut resources).unwrap();
        let node=tree.get_element_by_id("p").unwrap();
        let style=&prepared.layout().styles[&node];
        assert_eq!(style.font_size,Some(32.0));
        assert_eq!(style.font_family.as_deref(),Some("\"noto sans\""));
        let selected=prepared.layout().text_engine.prepared_font(style).font_id.expect("real Named selection");
        assert!(prepared.layout().text_engine.native_fonts.is_native_face(selected));
        let face=prepared.layout().text_engine.font_system.db().face(selected).unwrap();
        assert_eq!(face.post_script_name,"NotoSans-Regular");assert_eq!(face.index,0);
        let before:Vec<_>=(0..prepared.layout().text_engine.items.len())
            .map(|item|glyphs(&prepared.layout().text_engine,item)).collect();
        assert!(before.iter().flatten().any(|(_,glyph,_)|*glyph!=0));
        let used=before.iter().flatten().map(|(id,_,_)|*id).collect::<Vec<_>>();
        let owner=used.iter().find_map(|id|{
            let engine=&prepared.layout().text_engine;
            if !engine.native_fonts.is_native_face(*id) {return None;}
            match &engine.font_system.db().face(*id)?.source {
                cosmic_text::fontdb::Source::Binary(bytes)=>Some(Arc::downgrade(bytes)),_=>None
            }
        }).expect("PreparedRender must own an actual native byte source");
        let pixels=crate::paint::paint_prepared(&tree,&mut prepared,&mut resources,(0.0,0.0)).unwrap().data().to_vec();
        {
            let mut canvas=crate::text::CanvasTextEngine::new();canvas.reduce_native_file_limit_for_test(1);
            for i in 0..32 {
                let family=if i%2==0 {"Noto Sans Arabic"}else{"Noto Sans Hebrew"};
                let font=crate::text::CanvasFont::parse(&format!("32px '{family}'")).unwrap();
                canvas.measure(&font,"abc",crate::text::TextReference {align:"left",baseline:"alphabetic",rtl:false}).unwrap();
            }
        }
        let after:Vec<_>=(0..prepared.layout().text_engine.items.len())
            .map(|item|glyphs(&prepared.layout().text_engine,item)).collect();
        assert_eq!(before,after);assert!(owner.upgrade().is_some());
        assert_eq!(pixels,crate::paint::paint_prepared(&tree,&mut prepared,&mut resources,(0.0,0.0)).unwrap().data());
    });
}
