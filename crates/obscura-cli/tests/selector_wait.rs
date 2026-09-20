use std::process::{Command, Output};

fn fetch(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_obscura"))
        .arg("fetch")
        .args(args)
        .env("OBSCURA_PERSONA", "windows_chrome145")
        .output()
        .expect("run obscura fetch")
}

#[test]
fn selector_wait_advances_after_the_fixed_settle_window() {
    let url = concat!(
        "data:text/html,<body><script>",
        "setTimeout(()=>{const n=document.createElement('div');",
        "n.id='ready';n.textContent='selector landed';document.body.appendChild(n)},1200)",
        "</script></body>"
    );
    let output = fetch(&[
        url,
        "--selector",
        "#ready",
        "--wait",
        "1",
        "--timeout",
        "5",
        "--dump",
        "text",
        "--quiet",
    ]);

    assert!(
        output.status.success(),
        "selector wait failed: {}",
        String::from_utf8_lossy(&output.stderr),
    );
    assert_eq!(
        String::from_utf8(output.stdout).expect("UTF-8 text dump").trim(),
        "selector landed",
    );
}

#[test]
fn missing_selector_preserves_warning_and_page_output() {
    let output = fetch(&[
        "data:text/html,<body>page remains available</body>",
        "--selector",
        "#missing",
        "--wait",
        "0",
        "--dump",
        "text",
        "--quiet",
    ]);

    assert!(output.status.success(), "missing selector should only warn");
    assert_eq!(
        String::from_utf8(output.stdout).expect("UTF-8 text dump").trim(),
        "page remains available",
    );
    assert_eq!(
        String::from_utf8(output.stderr).expect("UTF-8 warning").trim(),
        "Warning: selector '#missing' not found after 0s",
    );
}

#[test]
fn invalid_selector_is_a_command_error() {
    let output = fetch(&[
        "data:text/html,<body>page</body>",
        "--selector",
        "[",
        "--wait",
        "0",
        "--dump",
        "text",
        "--quiet",
    ]);

    assert!(!output.status.success(), "invalid selector was accepted");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("invalid selector"),
        "unexpected invalid-selector error: {}",
        String::from_utf8_lossy(&output.stderr),
    );
}
