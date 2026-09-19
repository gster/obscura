use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

struct RobotsServer {
    address: String,
    requests: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl RobotsServer {
    fn spawn() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind robots fixture");
        listener
            .set_nonblocking(true)
            .expect("set fixture nonblocking");
        let address = listener.local_addr().expect("fixture address").to_string();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let server_requests = Arc::clone(&requests);
        thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(15);
            while Instant::now() < deadline {
                match listener.accept() {
                    Ok((stream, _)) => serve(stream, &server_requests),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("accept fixture connection: {error}"),
                }
            }
        });
        Self { address, requests }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.address, path)
    }

    fn paths(&self) -> Vec<String> {
        self.requests()
            .iter()
            .map(|request| {
                std::str::from_utf8(request)
                    .expect("fixture request headers are valid HTTP text")
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or("/")
                    .to_string()
            })
            .collect()
    }

    fn requests(&self) -> Vec<Vec<u8>> {
        self.requests.lock().expect("fixture requests").clone()
    }
}

fn serve(mut stream: TcpStream, requests: &Arc<Mutex<Vec<Vec<u8>>>>) {
    stream
        .set_nonblocking(false)
        .expect("set fixture connection blocking");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set fixture read timeout");
    let mut raw_request = Vec::new();
    while !raw_request.windows(4).any(|window| window == b"\r\n\r\n") {
        let mut chunk = [0_u8; 1024];
        let count = stream.read(&mut chunk).expect("read fixture request");
        assert!(count > 0, "fixture connection closed before complete headers");
        raw_request.extend_from_slice(&chunk[..count]);
        assert!(raw_request.len() <= 64 * 1024, "fixture request headers exceed 64 KiB");
    }
    let request_text = std::str::from_utf8(&raw_request)
        .expect("fixture request headers are valid HTTP text");
    let first_line = request_text
        .lines()
        .next()
        .unwrap_or_default()
        .to_string();
    let path = first_line.split_whitespace().nth(1).unwrap_or("/");
    requests
        .lock()
        .expect("fixture requests")
        .push(raw_request);
    let (content_type, body) = if path == "/robots.txt" {
        ("text/plain", "User-agent: *\nDisallow: /private\n")
    } else {
        ("text/html", "<!doctype html><title>target reached</title><p>private target</p>")
    };
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(response.as_bytes())
        .expect("write fixture response");
}

fn obscura(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_obscura"))
        .args(args)
        .output()
        .expect("run obscura CLI")
}

#[test]
fn obey_robots_is_global_and_blocks_fetch_before_target_request() {
    let server = RobotsServer::spawn();
    let url = server.url("/private/page");
    let output = obscura(&[
        "fetch",
        "--obey-robots",
        "--allow-private-network",
        "--quiet",
        "--wait",
        "0",
        &url,
    ]);

    assert!(!output.status.success(), "disallowed fetch must fail");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("Blocked by robots.txt"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(server.paths(), vec!["/robots.txt"]);
}

#[test]
fn obey_robots_reaches_scrape_worker_and_blocks_target_request() {
    let server = RobotsServer::spawn();
    let url = server.url("/private/page");
    let output = obscura(&[
        "--obey-robots",
        "--allow-private-network",
        "scrape",
        "--quiet",
        "--timeout",
        "5",
        &url,
    ]);

    assert!(output.status.success(), "scrape reports per-URL errors as JSON");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Blocked by robots.txt"), "stdout: {stdout}");
    assert_eq!(server.paths(), vec!["/robots.txt"]);
}

#[test]
fn fetch_without_obey_robots_keeps_existing_navigation_behavior() {
    let server = RobotsServer::spawn();
    let url = server.url("/private/page");
    let output = obscura(&[
        "--allow-private-network",
        "fetch",
        "--quiet",
        "--wait",
        "0",
        &url,
    ]);

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(server.paths(), vec!["/private/page"]);
}

#[test]
fn obey_robots_fetches_an_allowed_target_after_loading_policy() {
    let server = RobotsServer::spawn();
    let url = server.url("/public/page");
    let output = obscura(&[
        "--obey-robots",
        "--allow-private-network",
        "fetch",
        "--quiet",
        "--wait",
        "0",
        &url,
    ]);

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(server.paths(), vec!["/robots.txt", "/public/page"]);
    let requests = server.requests();
    let user_agents: Vec<_> = requests
        .iter()
        .map(|request| {
            std::str::from_utf8(request)
                .expect("fixture request headers are valid HTTP text")
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("user-agent")
                        .then(|| value.trim().to_string())
                })
                .expect("complete raw request contains User-Agent")
        })
        .collect();
    assert_eq!(user_agents.len(), 2);
    assert_eq!(
        user_agents[0], user_agents[1],
        "robots.txt and navigation must use the same persona-owned User-Agent"
    );
}
