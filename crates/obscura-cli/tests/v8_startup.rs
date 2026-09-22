use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

fn worker() -> Command {
    Command::new(env!("CARGO_BIN_EXE_obscura-worker"))
}

struct LocalPage {
    address: SocketAddr,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl LocalPage {
    fn spawn() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind local fixture");
        listener.set_nonblocking(true).expect("make local fixture nonblocking");
        let address = listener.local_addr().expect("read local fixture address");
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let thread = std::thread::spawn(move || {
            while !thread_stop.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream
                            .set_nonblocking(false)
                            .expect("make fixture connection blocking");
                        stream
                            .set_read_timeout(Some(Duration::from_secs(2)))
                            .expect("set fixture read timeout");
                        let mut request = Vec::new();
                        while !request.windows(4).any(|part| part == b"\r\n\r\n") {
                            let mut chunk = [0_u8; 1024];
                            let count = stream.read(&mut chunk).expect("read fixture request");
                            if count == 0 {
                                break;
                            }
                            request.extend_from_slice(&chunk[..count]);
                            assert!(request.len() <= 64 * 1024, "fixture headers exceed 64 KiB");
                        }
                        let body = b"<!doctype html><title>V8 startup fixture</title>";
                        let head = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len(),
                        );
                        stream.write_all(head.as_bytes()).expect("write fixture response head");
                        stream.write_all(body).expect("write fixture response body");
                        break;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("accept fixture connection: {error}"),
                }
            }
        });
        Self { address, stop, thread: Some(thread) }
    }

    fn url(&self) -> String {
        format!("http://{}/", self.address)
    }
}

impl Drop for LocalPage {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = TcpStream::connect(self.address);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn output_with_timeout(command: &mut Command, limit: Duration) -> Output {
    let child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn command");
    wait_with_output_timeout(child, limit)
}

fn wait_with_output_timeout(mut child: Child, limit: Duration) -> Output {
    let deadline = Instant::now() + limit;
    loop {
        if child.try_wait().expect("poll command").is_some() {
            return child.wait_with_output().expect("collect command output");
        }
        if Instant::now() >= deadline {
            child.kill().expect("kill timed out command");
            let output = child.wait_with_output().expect("collect timed out command output");
            panic!(
                "command exceeded {limit:?}; stdout={} stderr={}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn worker_requires_nonempty_v8_flags_before_page_startup() {
    let missing = output_with_timeout(
        worker()
            .env_remove("OBSCURA_V8_FLAGS")
            .env_remove("OBSCURA_PERSONA_JSON"),
        Duration::from_secs(20),
    );
    assert_eq!(missing.status.code(), Some(2));
    assert!(missing.stdout.is_empty());
    assert_eq!(
        String::from_utf8(missing.stderr).unwrap(),
        "OBSCURA_V8_FLAGS is required\n",
    );

    let empty = output_with_timeout(
        worker()
            .env("OBSCURA_V8_FLAGS", "  \t ")
            .env_remove("OBSCURA_PERSONA_JSON"),
        Duration::from_secs(20),
    );
    assert_eq!(empty.status.code(), Some(2));
    assert!(empty.stdout.is_empty());
    assert_eq!(
        String::from_utf8(empty.stderr).unwrap(),
        "OBSCURA_V8_FLAGS must not be empty\n",
    );
}

#[test]
fn worker_accepts_parent_v8_flags_and_shuts_down_cleanly() {
    let persona = obscura_net::EffectivePersona::builtin(
        obscura_net::StealthProfile::WindowsChrome145,
    );
    let persona_json = serde_json::to_string(&persona.to_spec()).unwrap();
    let mut child = worker()
        .env("OBSCURA_V8_FLAGS", "--max-old-space-size=2048")
        .env("OBSCURA_PERSONA_JSON", persona_json)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn configured worker");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"{\"cmd\":\"shutdown\"}\n")
        .expect("send worker shutdown");
    let output = wait_with_output_timeout(child, Duration::from_secs(20));
    assert!(output.status.success(), "stderr={}", String::from_utf8_lossy(&output.stderr));
    assert!(output.stderr.is_empty());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap(),
        serde_json::json!({"ok": true, "result": "bye"}),
    );
}

#[test]
fn scrape_applies_parent_v8_flags_before_the_worker_page_starts() {
    let fixture = LocalPage::spawn();
    let output = output_with_timeout(
        Command::new(env!("CARGO_BIN_EXE_obscura")).args([
            "--persona",
            "windows_chrome145",
            "--allow-private-network",
            "--v8-flags=--expose-gc",
            "scrape",
            "--quiet",
            "--timeout",
            "5",
            "--eval",
            "typeof gc",
            &fixture.url(),
        ]),
        Duration::from_secs(20),
    );

    assert!(output.status.success(), "stderr={}", String::from_utf8_lossy(&output.stderr));
    assert!(output.stderr.is_empty());
    let stdout: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(stdout["total_urls"], 1);
    assert_eq!(stdout["results"][0]["eval"], "function");
    assert!(stdout["results"][0].get("error").is_none());
}
