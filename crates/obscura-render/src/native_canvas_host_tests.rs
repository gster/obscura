//! Existing-API host controls, runnable before native integration. No provider API dependency.
use super::*;
use crate::font::{create_font_system, WebFont};

fn compare_public_resource(family: &str, path: &str) {
    let bytes = std::fs::read(path).expect("explicit macOS public-font fixture is required");
    let resource = WebFont { data: Arc::new(bytes), family: None, weight: None, italic: None };
    let mut expected = CanvasTextEngine::new();
    (expected.font_system, expected.families) = create_font_system(&[resource], false);
    let mut actual = CanvasTextEngine::new();
    let requested = CanvasFont::parse(&format!("32px '{family}', serif")).unwrap();
    let fallback = CanvasFont::parse("32px serif").unwrap();
    let words = "iii WWW Mm0123";
    let reference = expected.shape(&requested, words).unwrap();
    let control = expected.shape(&fallback, words).unwrap();
    assert!((reference.width - control.width).abs() > 1.0, "host fixture must distinguish real named bytes from bundled serif fallback");
    let measured = actual.shape(&requested, words).unwrap();
    assert_eq!(measured.width, reference.width, "actual named family must use the same real resource advances");
    assert_eq!(measured.geometry.font_ascent, reference.geometry.font_ascent);
    assert_eq!(measured.geometry.font_descent, reference.geometry.font_descent);
}

#[test]
#[ignore = "explicit macOS installed public resource control; run old/new separately"]
fn canvas_host_menlo_matches_actual_public_font_resource() {
    compare_public_resource("Menlo", "/System/Library/Fonts/Menlo.ttc");
}

#[test]
#[ignore = "explicit macOS installed public resource control; run old/new separately"]
fn canvas_host_helvetica_neue_matches_actual_public_font_resource() {
    compare_public_resource("Helvetica Neue", "/System/Library/Fonts/HelveticaNeue.ttc");
}


#[test]
#[ignore = "explicit macOS Arial separate public files; run old/new separately"]
fn canvas_host_arial_styles_match_real_separate_public_resources() {
    let resources: Vec<_> = ["Arial.ttf", "Arial Bold.ttf", "Arial Italic.ttf", "Arial Bold Italic.ttf"].into_iter().map(|file| {
        WebFont { data: Arc::new(std::fs::read(format!("/System/Library/Fonts/Supplemental/{file}")).expect("public Arial fixture required")), family: None, weight: None, italic: None }
    }).collect();
    for order in [["700", "400", "italic 400", "italic 700"], ["400", "700", "italic 400", "italic 700"]] {
        let mut expected = CanvasTextEngine::new();
        (expected.font_system, expected.families) = create_font_system(&resources, false);
        let mut actual = CanvasTextEngine::new();
        for style in order {
            let requested = CanvasFont::parse(&format!("{style} 32px Arial, serif")).unwrap();
            let reference = expected.shape(&requested, "Hamburgefontsiv iii WWW 0123").unwrap();
            let measured = actual.shape(&requested, "Hamburgefontsiv iii WWW 0123").unwrap();
            assert_eq!(measured.width, reference.width, "real Arial {style} advances must match independent public resource");
            assert_eq!(measured.geometry.font_ascent, reference.geometry.font_ascent);
            assert_eq!(measured.geometry.font_descent, reference.geometry.font_descent);
        }
    }
}
