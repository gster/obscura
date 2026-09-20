use std::io::{Read, Write};
use std::sync::Arc;
use std::time::{Duration, Instant};

use obscura_browser::{AutomationWait, AutomationWaitError, BrowserContext, Page};

fn page(name: &str) -> Page {
    let context = Arc::new(BrowserContext::with_storage_and_network(
        name.to_owned(),
        obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145),
        None,
        None,
        true,
    ));
    Page::new(format!("{name}-page"), context)
}

fn spawn_navigation_fixture(slow_delay: Duration) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();

    std::thread::spawn(move || {
        for incoming in listener.incoming() {
            let Ok(mut stream) = incoming else {
                continue;
            };
            let mut request = [0_u8; 2_048];
            let read = stream.read(&mut request).unwrap_or(0);
            let request = String::from_utf8_lossy(&request[..read]);
            let path = request.split_whitespace().nth(1).unwrap_or("/");
            let body = if path == "/slow" {
                std::thread::sleep(slow_delay);
                "<!doctype html><body><div id=landed>slow destination</div></body>"
            } else {
                r#"<!doctype html><body>
                    <script>setTimeout(() => location.href = '/slow', 10)</script>
                </body>"#
            };
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len(),
            );
        }
    });

    format!("http://{address}")
}

#[tokio::test(flavor = "current_thread")]
async fn waits_for_timer_created_selector_and_body_text() {
    let mut page = page("automation-wait-timer");
    page.navigate(
        r#"data:text/html,<body><script>
            setTimeout(() => {
                const node = document.createElement('div');
                node.id = 'ready';
                document.body.appendChild(node);
            }, 30);
            setTimeout(() => {
                document.body.appendChild(document.createTextNode('native timer text'));
            }, 120)
        </script></body>"#,
    )
    .await
    .unwrap();

    let deadline = Instant::now() + Duration::from_secs(2);
    let matched = page.wait_for_selector("#ready", deadline).await.unwrap();
    assert!(matches!(matched, AutomationWait::Matched(_)));
    assert_eq!(
        page.wait_for_text("native timer text", deadline).await.unwrap(),
        AutomationWait::Matched(()),
    );
}

#[tokio::test(flavor = "current_thread")]
async fn native_waits_ignore_poisoned_public_selector_queries() {
    let mut page = page("automation-wait-poison");
    page.navigate(
        "data:text/html,<body><div id=native-result>native document text</div></body>",
    )
    .await
    .unwrap();
    let poisoned = page.evaluate(
        r#"
        (() => {
            document.querySelector = () => null;
            return document.querySelector('#native-result') === null;
        })()
        "#,
    );
    assert_eq!(poisoned, serde_json::json!(true));

    let deadline = Instant::now() + Duration::from_secs(2);
    assert!(matches!(
        page.wait_for_selector("#native-result", deadline).await.unwrap(),
        AutomationWait::Matched(_),
    ));
    assert_eq!(
        page.wait_for_text("native document text", deadline).await.unwrap(),
        AutomationWait::Matched(()),
    );
}

#[tokio::test(flavor = "current_thread")]
async fn selector_wait_reports_invalid_selectors_and_probes_before_deadline() {
    let mut page = page("automation-wait-selector-errors");
    assert!(matches!(
        page.wait_for_selector("[", Instant::now() + Duration::from_secs(1)).await,
        Err(AutomationWaitError::InvalidSelector(_)),
    ));

    page.navigate("data:text/html,<body><div id=already></div></body>")
        .await
        .unwrap();
    assert!(matches!(
        page.wait_for_selector("#already", Instant::now() - Duration::from_millis(1)).await.unwrap(),
        AutomationWait::Matched(_),
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn timeout_leaves_the_page_usable() {
    let mut page = page("automation-wait-timeout");
    page.navigate("data:text/html,<body>still usable</body>")
        .await
        .unwrap();

    assert_eq!(
        page.wait_for_selector(
            "#missing",
            Instant::now() + Duration::from_millis(60),
        )
        .await
        .unwrap(),
        AutomationWait::TimedOut,
    );
    assert_eq!(page.evaluate("1 + 1"), serde_json::json!(2.0));
}

#[tokio::test(flavor = "current_thread")]
async fn pending_navigation_uses_the_remaining_wait_budget_instead_of_the_poll_cadence() {
    std::env::set_var("OBSCURA_ALLOW_PRIVATE_NETWORK", "1");
    let mut page = page("automation-wait-navigation");
    page.navigate(&spawn_navigation_fixture(Duration::from_millis(80)))
        .await
        .unwrap();

    assert!(matches!(
        page.wait_for_selector(
            "#landed",
            Instant::now() + Duration::from_secs(2),
        )
        .await
        .unwrap(),
        AutomationWait::Matched(_),
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn pending_navigation_deadline_error_is_typed_and_the_page_can_recover() {
    std::env::set_var("OBSCURA_ALLOW_PRIVATE_NETWORK", "1");
    let mut page = page("automation-wait-navigation-timeout");
    page.navigate(&spawn_navigation_fixture(Duration::from_millis(400)))
        .await
        .unwrap();

    assert!(matches!(
        page.wait_for_selector(
            "#landed",
            Instant::now() + Duration::from_millis(80),
        )
        .await,
        Err(AutomationWaitError::Navigation(_)),
    ));

    page.navigate("data:text/html,<body><div id=recovered></div></body>")
        .await
        .unwrap();
    assert!(matches!(
        page.wait_for_selector("#recovered", Instant::now()).await.unwrap(),
        AutomationWait::Matched(_),
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn queued_navigation_script_uses_the_wait_watchdog_and_recovers() {
    let mut page = page("automation-wait-navigation-script");
    page.navigate("data:text/html,<body>original</body>").await.unwrap();
    page.evaluate(r#"location.href = 'data:text/html,<body><script>const end=Date.now()+2500;while(Date.now()<end){}</script></body>'"#);

    let started = Instant::now();
    let outcome = page.wait_for_selector("#missing", started + Duration::from_millis(150)).await;
    assert!(matches!(outcome, Err(AutomationWaitError::Navigation(_))), "{outcome:?}");
    assert!(started.elapsed() < Duration::from_millis(1500), "navigation script escaped wait deadline");
    assert_eq!(page.evaluate("1 + 1"), serde_json::json!(2.0));
    page.navigate("data:text/html,<body><div id=recovered></div></body>").await.unwrap();
    assert!(matches!(page.wait_for_selector("#recovered", Instant::now()).await.unwrap(), AutomationWait::Matched(_)));
}

#[tokio::test(flavor = "current_thread")]
async fn native_text_wait_ignores_poisoned_public_text_getters() {
    let mut page = page("automation-wait-text-poison");
    page.navigate("data:text/html,<body>authentic native text</body>").await.unwrap();
    assert_eq!(page.evaluate(r#"(() => {
        Object.defineProperty(Element.prototype, 'innerText', {configurable:true, get() {return 'forged innerText';}});
        Object.defineProperty(Node.prototype, 'textContent', {configurable:true, get() {return 'forged textContent';}});
        return [document.body.innerText, document.body.textContent];
    })()"#), serde_json::json!(["forged innerText", "forged textContent"]));
    assert_eq!(page.wait_for_text("authentic native text", Instant::now()).await.unwrap(), AutomationWait::Matched(()));
    assert_eq!(page.wait_for_text("forged", Instant::now()).await.unwrap(), AutomationWait::TimedOut);
}

#[tokio::test(flavor = "current_thread")]
async fn dropping_a_wait_does_not_keep_advancing_the_page() {
    let mut page = page("automation-wait-cancel");
    page.navigate("data:text/html,<body></body>").await.unwrap();
    page.evaluate("(()=>{globalThis.waitTicks=0;setTimeout(()=>waitTicks++,100);return true})()");
    {
        let waiting = page.wait_for_selector("#missing", Instant::now() + Duration::from_secs(1));
        tokio::pin!(waiting);
        assert!(tokio::time::timeout(Duration::from_millis(25), &mut waiting).await.is_err());
    }
    tokio::time::sleep(Duration::from_millis(125)).await;
    assert_eq!(page.evaluate("waitTicks"), serde_json::json!(0.0));
    page.advance_automation(Instant::now() + Duration::from_millis(100)).await.unwrap();
    assert_eq!(page.evaluate("waitTicks"), serde_json::json!(1.0));
    page.advance_automation(Instant::now() + Duration::from_millis(100)).await.unwrap();
    assert_eq!(page.evaluate("waitTicks"), serde_json::json!(1.0));
}

#[tokio::test(flavor = "current_thread")]
async fn document_identity_detects_same_url_reload_and_document_open() {
    let mut page = page("automation-wait-document-identity");
    let url = "data:text/html,<body>identity</body>";
    page.navigate(url).await.unwrap();
    let original_url = page.url_string();
    let initial = page.document_identity();
    page.evaluate("location.reload()");
    assert!(page.process_pending_navigation().await.unwrap());
    let reloaded = page.document_identity();
    assert_ne!(initial, reloaded);
    assert_eq!(page.url_string(), original_url);
    page.evaluate("(()=>{document.open();document.write('<body>replacement</body>');document.close();return true})()");
    assert_ne!(reloaded, page.document_identity());
    assert_eq!(page.url_string(), original_url);
}
