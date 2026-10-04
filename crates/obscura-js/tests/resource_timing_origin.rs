use obscura_js::runtime::ObscuraJsRuntime;
use std::io::{Read, Write};
use std::time::{Duration, Instant};

// Serve actual HTTP responses through an explicit local proxy. Distinct
// request origins do not depend on DNS or external network access.
fn proxy(responses: Vec<(&'static str, &'static str)>) -> (String, std::thread::JoinHandle<Vec<String>>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let server = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut requests = Vec::new();
        for (headers, body) in responses {
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "timing fixture request did not arrive");
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("timing fixture accept failed: {error}"),
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            stream.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut request = Vec::new();
            while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                let mut bytes = [0; 4096];
                let count = stream.read(&mut bytes).unwrap();
                assert!(count > 0 && request.len() + count <= 16384);
                request.extend_from_slice(&bytes[..count]);
            }
            requests.push(String::from_utf8(request).unwrap().lines().next().unwrap().to_owned());
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n{}\r\n{}", body.len(), headers, body).unwrap();
        }
        requests
    });
    (address, server)
}

fn runtime(url: &str, proxy: &str) -> ObscuraJsRuntime {
    let persona = obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::MacChrome153);
    let mut runtime = ObscuraJsRuntime::with_base_url_and_proxy(url, Some(proxy.to_owned()), persona);
    runtime.set_dom(obscura_dom::parse_html("<html><head></head><body></body></html>"));
    runtime.set_url(url);
    runtime.run_page_init();
    runtime
}

#[tokio::test(flavor = "current_thread")]
async fn resource_timing_tao_uses_host_owner_origin_not_public_url_constructor() {
    let (address, server) = proxy(vec![
        ("", "same"),
        ("", "masked"),
        ("Timing-Allow-Origin: http://owner.test\r\n", "allowed"),
    ]);
    let mut runtime = runtime("http://owner.test/page", &address);
    let value = runtime.call_function_on_for_cdp(r#"async () => {
        await fetch('/same');
        const OriginalURL = URL;
        // Author code may replace a public constructor. This must not grant
        // timing access to a different origin inside the native fetch path.
        globalThis.URL = class extends OriginalURL {
            get origin() { return 'http://other.test'; }
        };
        try {
            await fetch('http://other.test/masked', {mode:'no-cors'});
            await fetch('http://other.test/allowed', {mode:'no-cors'});
        } finally { globalThis.URL = OriginalURL; }
        return performance.getEntriesByType('resource').map(entry =>
            [entry.name, entry.initiatorType, entry.nextHopProtocol, entry.decodedBodySize]);
    }"#, None, &[], true, true).await.unwrap().value.unwrap();
    assert_eq!(value, serde_json::json!([
        ["http://owner.test/same", "fetch", "http/1.1", 4],
        ["http://other.test/masked", "fetch", "", 0],
        ["http://other.test/allowed", "fetch", "http/1.1", 7],
    ]));
    assert_eq!(server.join().unwrap().len(), 3);
}

#[tokio::test(flavor = "current_thread")]
async fn resource_timing_opaque_owner_requires_tao_even_when_script_reports_tuple_origin() {
    let (address, server) = proxy(vec![
        ("", "masked"),
        ("Timing-Allow-Origin: null\r\n", "allowed"),
    ]);
    let mut runtime = runtime("about:blank", &address);
    let value = runtime.call_function_on_for_cdp(r#"async () => {
        const OriginalURL = URL;
        globalThis.URL = class extends OriginalURL {
            get origin() { return 'http://other.test'; }
        };
        try {
            await fetch('http://other.test/masked', {mode:'no-cors'});
            await fetch('http://other.test/allowed', {mode:'no-cors'});
        } finally { globalThis.URL = OriginalURL; }
        return performance.getEntriesByType('resource').map(entry =>
            [entry.nextHopProtocol, entry.decodedBodySize]);
    }"#, None, &[], true, true).await.unwrap().value.unwrap();
    assert_eq!(value, serde_json::json!([["", 0], ["http/1.1", 7]]));
    assert_eq!(server.join().unwrap().len(), 2);
}

#[tokio::test(flavor = "current_thread")]
async fn linked_stylesheets_do_not_publish_fake_fetch_resource_entries() {
    let (address, server) = proxy(vec![
        ("Content-Type: text/css\r\n", "body { color: rgb(12, 34, 56); }"),
        ("Content-Type: text/css\r\nTiming-Allow-Origin: http://owner.test\r\n", "body { background-color: rgb(65, 43, 21); }"),
        ("", "fetch-positive"),
    ]);
    let mut runtime = runtime("http://owner.test/page", &address);
    let value = runtime.call_function_on_for_cdp(r#"async () => {
        const loaded = [];
        for (const path of ['no-tao.css', 'with-tao.css']) {
            await new Promise((resolve, reject) => {
                const link = document.createElement('link');
                link.setAttribute('rel', 'stylesheet');
                link.setAttribute('href', 'http://other.test/' + path);
                link.addEventListener('load', () => { loaded.push(path); resolve(); }, {once:true});
                link.addEventListener('error', () => reject(new Error('stylesheet failed')), {once:true});
                document.head.appendChild(link);
            });
        }
        const afterStylesheets = performance.getEntriesByType('resource').length;
        await fetch('/positive');
        return {loaded, afterStylesheets, entries:performance.getEntriesByType('resource').map(entry =>
            [entry.name, entry.initiatorType, entry.decodedBodySize])};
    }"#, None, &[], true, true).await.unwrap().value.unwrap();
    assert_eq!(value, serde_json::json!({
        "loaded": ["no-tao.css", "with-tao.css"], "afterStylesheets": 0,
        "entries": [["http://owner.test/positive", "fetch", 14]],
    }));
    let requests = server.join().unwrap();
    assert!(requests[0].contains("http://other.test/no-tao.css"));
    assert!(requests[1].contains("http://other.test/with-tao.css"));
    assert!(requests[2].contains("http://owner.test/positive"));
}
