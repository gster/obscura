use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

pub fn validate_font_bundle(repository: &Path, manifest_path: &Path) -> Result<(), String> {
    let manifest_bytes = fs::read(manifest_path)
        .map_err(|error| format!("cannot read {}: {error}", manifest_path.display()))?;
    let fonts: serde_json::Value = serde_json::from_slice(&manifest_bytes)
        .map_err(|error| format!("invalid {}: {error}", manifest_path.display()))?;
    let inline_path = repository.join("crates/obscura-render/src/inline.rs");
    let source = fs::read_to_string(&inline_path)
        .map_err(|error| format!("cannot read {}: {error}", inline_path.display()))?;

    if fonts["source"] != "crates/obscura-render/src/inline.rs" {
        return Err("font manifest source changed".into());
    }
    let files = fonts["files"]
        .as_array()
        .ok_or("font manifest files must be an array")?;
    if files.len() != 16 {
        return Err("review the embedded font set when changing its membership".into());
    }
    let declaration_count = source
        .lines()
        .filter(|line| line.starts_with("static ") && line.contains("include_bytes!"))
        .count();
    if declaration_count != files.len() {
        return Err("embedded font membership changed".into());
    }

    let mut names = BTreeSet::new();
    for entry in files {
        let name = entry["name"]
            .as_str()
            .ok_or("font entry name must be a string")?;
        if !names.insert(name) {
            return Err(format!("duplicate font declaration: {name}"));
        }
        let path = entry["path"]
            .as_str()
            .ok_or_else(|| format!("font path must be a string: {name}"))?;
        let (filename, include_path) = if let Some(filename) = path.strip_prefix("fonts/") {
            (filename, format!("../../../{path}"))
        } else {
            let filename = path
                .strip_prefix("crates/obscura-render/assets/")
                .ok_or_else(|| format!("font path is outside the embedded font roots: {path}"))?;
            (filename, format!("../assets/{filename}"))
        };
        if filename.contains('/') {
            return Err(format!("nested font path is not allowed: {path}"));
        }
        if !source.contains(&format!(
            "static {name}: &[u8] = include_bytes!(\"{include_path}\");"
        )) {
            return Err(format!("font declaration changed: {name}"));
        }

        let disk_path = repository.join(path);
        println!("cargo:rerun-if-changed={}", disk_path.display());
        let data = fs::read(&disk_path)
            .map_err(|error| format!("cannot read {}: {error}", disk_path.display()))?;
        let expected_bytes = entry["bytes"]
            .as_u64()
            .ok_or_else(|| format!("font byte count must be an integer: {path}"))?;
        if data.len() as u64 != expected_bytes {
            return Err(format!("font size changed: {path}"));
        }
        let expected_sha256 = entry["sha256"]
            .as_str()
            .ok_or_else(|| format!("font digest must be a string: {path}"))?;
        if format!("{:x}", Sha256::digest(data)) != expected_sha256 {
            return Err(format!("font digest changed: {path}"));
        }
    }
    Ok(())
}

fn main() {
    let crate_root = PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR must be set"),
    );
    let repository = crate_root.join("../..");
    let manifest = repository.join("persona-fonts.json");
    let inline = crate_root.join("src/inline.rs");
    println!("cargo:rerun-if-changed={}", manifest.display());
    println!("cargo:rerun-if-changed={}", inline.display());
    validate_font_bundle(&repository, &manifest).unwrap_or_else(|error| panic!("{error}"));
}
