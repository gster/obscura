use sha2::{Digest, Sha256};
use std::fs;

fn main() {
    let bytes = fs::read("../persona-fonts.json").unwrap();
    let fonts: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let source = fs::read_to_string("../crates/obscura-render/src/inline.rs").unwrap();
    assert_eq!(fonts["source"], "crates/obscura-render/src/inline.rs");
    let files = fonts["files"].as_array().unwrap();
    assert_eq!(
        files.len(),
        16,
        "review the embedded font set when changing its membership"
    );
    assert_eq!(
        source
            .lines()
            .filter(|line| line.starts_with("static ") && line.contains("include_bytes!"))
            .count(),
        files.len(),
        "embedded font membership changed"
    );
    let mut names = std::collections::BTreeSet::new();
    for entry in files {
        let name = entry["name"].as_str().unwrap();
        assert!(names.insert(name), "duplicate font declaration");
        let path = entry["path"].as_str().unwrap();
        let (filename, disk_path, include_path) =
            if let Some(filename) = path.strip_prefix("fonts/") {
                (filename, format!("../{path}"), format!("../../../{path}"))
            } else {
                let filename = path.strip_prefix("crates/obscura-render/assets/").unwrap();
                (
                    filename,
                    format!("../{path}"),
                    format!("../assets/{filename}"),
                )
            };
        assert!(!filename.contains('/'));
        assert!(
            source.contains(&format!(
                "static {name}: &[u8] = include_bytes!(\"{include_path}\");"
            )),
            "font declaration changed: {name}"
        );
        let data = fs::read(disk_path).unwrap();
        assert_eq!(
            data.len() as u64,
            entry["bytes"].as_u64().unwrap(),
            "font size changed: {path}"
        );
        assert_eq!(
            format!("{:x}", Sha256::digest(data)),
            entry["sha256"].as_str().unwrap(),
            "font bytes changed: {path}"
        );
    }
    println!(
        "cargo:rustc-env=AUTOPILOT_FONT_BUNDLE_SHA256={:x}",
        Sha256::digest(bytes)
    );
    for path in ["../persona-fonts.json", "../fonts", "../crates", "../vendor"] {
        println!("cargo:rerun-if-changed={path}");
    }
}
