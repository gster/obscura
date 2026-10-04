// Build-time target helper only. No runtime compiler or native API invocation.
pub fn build_native_speech_helper() {
    for file in ["native/speech/native_mapping_main.mm", "native/speech/mapping_core.h", "native/speech/probe_host.h", "native/speech/stream_host.h", "native/speech/helper_lifecycle.h", "build_speech_helper.rs"] {
        println!("cargo:rerun-if-changed={file}");
    }
    println!("cargo:rerun-if-env-changed=MACOSX_DEPLOYMENT_TARGET");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") { return; }
    let host = std::env::var("HOST").expect("Cargo HOST");
    assert!(host.contains("apple-darwin"), "macOS Speech helper cross compilation requires a macOS host and SDK");
    let arch = match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("aarch64") => "arm64", Ok("x86_64") => "x86_64", _ => panic!("unsupported macOS Speech helper architecture"),
    };
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo OUT_DIR")).join("obscura-speech-helper");
    let deployment = std::env::var("MACOSX_DEPLOYMENT_TARGET").unwrap_or_else(|_| "13.0".into());
    let status = std::process::Command::new("xcrun").args(["--sdk", "macosx", "clang++", "-std=c++17", "-fobjc-arc", "-fblocks", "-arch", arch])
        .arg(format!("-mmacosx-version-min={deployment}"))
        .args(["native/speech/native_mapping_main.mm", "-framework", "Foundation", "-framework", "AVFAudio", "-framework", "AppKit", "-framework", "CoreFoundation", "-o"])
        .arg(out).status().expect("compile macOS Speech helper with installed SDK");
    assert!(status.success(), "macOS Speech helper compilation failed");
}
