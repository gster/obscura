use std::sync::Arc;
use std::time::Duration;

use obscura_browser::{BrowserContext, Page};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const FIRST_CHUNK: &str = "<!doctype html><html><body>two real document chunks";
const LAST_CHUNK: &str = r#"<script>
globalThis.timingEvents = [];
globalThis.timingFetchState = 'pending';
document.addEventListener('DOMContentLoaded', () => {
  timingEvents.push(['dom', performance.now()]);
});
addEventListener('load', () => {
  const n = performance.getEntriesByType('navigation')[0];
  timingEvents.push(['load', performance.now(), n.loadEventStart, n.loadEventEnd]);
});
fetch('/probe').then(response => response.text()).then(body => {
  globalThis.timingFetchBody = body;
  timingFetchState = 'done';
}, error => { timingFetchState = String(error); });
</script></body></html>"#;
const PROBE_BODY: &str = "actual resource body";

async fn request_path(stream: &mut tokio::net::TcpStream) -> String {
    let mut headers = Vec::new();
    while !headers.ends_with(b"\r\n\r\n") {
        assert!(headers.len() < 8192, "fixture request headers must stay bounded");
        headers.push(stream.read_u8().await.unwrap());
    }
    String::from_utf8(headers).unwrap().split_whitespace().nth(1).unwrap().to_owned()
}

#[tokio::test(flavor = "current_thread")]
async fn page_navigation_and_fetch_timing_follow_real_chunk_completion() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (first_sent, first_received) = tokio::sync::oneshot::channel();
    let (release_body, body_released) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut document, _) = listener.accept().await.unwrap();
        assert_eq!(request_path(&mut document).await, "/document");
        document.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n").await.unwrap();
        document.write_all(format!("{:x}\r\n{}\r\n", FIRST_CHUNK.len(), FIRST_CHUNK).as_bytes()).await.unwrap();
        let first_at = obscura_net::timing::now();
        first_sent.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(5), body_released).await.unwrap().unwrap();
        let release_at = obscura_net::timing::now();
        document.write_all(format!("{:x}\r\n{}\r\n0\r\n\r\n", LAST_CHUNK.len(), LAST_CHUNK).as_bytes()).await.unwrap();
        document.shutdown().await.unwrap();

        let (mut probe, _) = listener.accept().await.unwrap();
        assert_eq!(request_path(&mut probe).await, "/probe");
        probe.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", PROBE_BODY.len(), PROBE_BODY).as_bytes()).await.unwrap();
        probe.shutdown().await.unwrap();
        release_at - first_at
    });

    let context = Arc::new(BrowserContext::with_storage_and_network(
        "navigation-timing".into(),
        obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::MacChrome153),
        None, None, true,
    ));
    let mut page = Page::new("navigation-timing-page".into(), context);
    let document_url = format!("{base}/document");
    {
        let navigation = page.navigate(&document_url);
        tokio::pin!(navigation);
        tokio::select! {
            result = &mut navigation => panic!("navigation completed before the withheld body: {result:?}"),
            result = tokio::time::timeout(Duration::from_secs(5), first_received) => result.unwrap().unwrap(),
        }
        // A real incomplete HTTP body must keep navigation pending. The delay
        // creates an observable boundary, not a predicted browser duration.
        assert!(tokio::time::timeout(Duration::from_millis(40), &mut navigation).await.is_err());
        release_body.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(5), &mut navigation).await.unwrap().unwrap();
    }
    for _ in 0..30 {
        if page.evaluate("timingFetchState") == serde_json::json!("done") { break; }
        page.settle(50).await;
    }
    assert_eq!(page.evaluate("timingFetchState"), serde_json::json!("done"));
    let withheld_ms = tokio::time::timeout(Duration::from_secs(5), server).await.unwrap().unwrap();
    let result = page.evaluate(r#"(() => {
      const navigation = performance.getEntriesByType('navigation');
      const resources = performance.getEntriesByType('resource');
      const n = navigation[0], r = resources.find(entry => entry.name.endsWith('/probe'));
      return {
        navigationCount: navigation.length, resourceCount: resources.length,
        nav: n && n.toJSON(), resource: r && r.toJSON(), body: timingFetchBody,
        navigationBrand: n instanceof PerformanceNavigationTiming,
        resourceBrand: r instanceof PerformanceResourceTiming,
        readyState: document.readyState, events: timingEvents,
        timeOrigin: performance.timeOrigin, now: performance.now()
      };
    })()"#);
    assert_eq!(result["navigationCount"], 1);
    assert_eq!(result["resourceCount"], 1);
    assert_eq!(result["navigationBrand"], true);
    assert_eq!(result["resourceBrand"], true);
    assert_eq!(result["readyState"], "complete");
    assert_eq!(result["body"], PROBE_BODY);
    assert!(result["timeOrigin"].as_f64().unwrap() > 0.0);
    let nav = &result["nav"];
    let resource = &result["resource"];
    assert_eq!(nav["name"], document_url);
    assert_eq!(nav["startTime"], 0.0);
    assert_eq!(nav["decodedBodySize"], FIRST_CHUNK.len() + LAST_CHUNK.len());
    assert_eq!(resource["name"], format!("{base}/probe"));
    assert_eq!(resource["initiatorType"], "fetch");
    assert_eq!(resource["decodedBodySize"], PROBE_BODY.len());
    for entry in [nav, resource] {
        assert_eq!(entry["nextHopProtocol"], "http/1.1");
        let start = entry["startTime"].as_f64().unwrap();
        let fetch = entry["fetchStart"].as_f64().unwrap();
        let end = entry["responseEnd"].as_f64().unwrap();
        assert!(start.is_finite() && fetch.is_finite() && end.is_finite());
        assert!(start >= 0.0 && fetch >= start && end >= fetch);
        assert!(end <= result["now"].as_f64().unwrap());
    }
    assert!(nav["responseEnd"].as_f64().unwrap() - nav["fetchStart"].as_f64().unwrap() >= withheld_ms,
        "responseEnd must include the actual withheld document body interval: {result}");
    assert!(resource["startTime"].as_f64().unwrap() >= nav["responseEnd"].as_f64().unwrap());
    assert_eq!(result["events"][0][0], "dom");
    assert_eq!(result["events"][1][0], "load");
    assert_eq!(result["events"].as_array().unwrap().len(), 2);
    let load_start = nav["loadEventStart"].as_f64().unwrap();
    let load_end = nav["loadEventEnd"].as_f64().unwrap();
    assert!(nav["domContentLoadedEventEnd"].as_f64().unwrap() >= nav["domContentLoadedEventStart"].as_f64().unwrap());
    assert!(load_start >= nav["domContentLoadedEventEnd"].as_f64().unwrap());
    assert_eq!(result["events"][1][2], nav["loadEventStart"]);
    assert_eq!(result["events"][1][3], 0.0, "load end is not known inside the load callback");
    assert!(load_end >= result["events"][1][1].as_f64().unwrap());
    assert_eq!(nav["duration"], nav["loadEventEnd"]);
    // No DNS/TCP/TLS, socket reuse, or encoded-wire byte claims are inferred
    // from this HTTP/1.1 fixture or its decoded body length.
}
