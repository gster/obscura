use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=js/bootstrap.js");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rustc-check-cfg=cfg(obscura_runtime_snapshot)");
    if std::env::var("TARGET").unwrap() != std::env::var("HOST").unwrap() {
        // A host snapshot cannot be deserialized by a different target V8.
        // Generate the same bootstrap snapshot on the target at first use.
        println!("cargo:rustc-cfg=obscura_runtime_snapshot");
        println!("cargo:warning=Obscura cross build uses a target-native runtime snapshot");
        return;
    }
    let snapshot_path = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("OBSCURA_SNAPSHOT.bin");
    generate_host_snapshot(include_str!("js/bootstrap.js"), &snapshot_path);
}

fn generate_host_snapshot(bootstrap_js: &str, snapshot_path: &PathBuf) {
    let bootstrap_js = bootstrap_js.to_string();
    let output = deno_core::snapshot::create_snapshot(
        deno_core::snapshot::CreateSnapshotOptions {
            cargo_manifest_dir: env!("CARGO_MANIFEST_DIR"),
            startup_snapshot: None,
            skip_op_registration: true,
            extensions: vec![],
            extension_transpiler: None,
            with_runtime_cb: Some(Box::new(move |runtime| {
                runtime
                    .execute_script("<obscura:bootstrap>", bootstrap_js.to_string())
                    .expect("bootstrap.js should not fail during snapshot creation");
            })),
        },
        None,
    )
    .expect("Failed to create V8 snapshot");

    std::fs::write(&snapshot_path, &*output.output).expect("Failed to write snapshot");
    println!(
        "cargo:rustc-env=OBSCURA_SNAPSHOT_PATH={}",
        snapshot_path.display()
    );

    for file in &output.files_loaded_during_snapshot {
        println!("cargo:rerun-if-changed={}", file.display());
    }
}
