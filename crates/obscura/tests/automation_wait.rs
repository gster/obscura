use std::time::Duration;

use obscura::Browser;

#[tokio::test(flavor = "current_thread")]
async fn public_selector_wait_advances_tasks_and_ignores_query_selector_poisoning() {
    let browser = Browser::new(obscura::EffectivePersona::builtin(
        obscura::StealthProfile::WindowsChrome145,
    ))
    .unwrap();
    let mut page = browser.new_page().await.unwrap();
    page.goto("data:text/html,<body></body>").await.unwrap();
    page.evaluate(
        r#"(() => {
            setTimeout(() => {
                const node = document.createElement('div');
                node.id = 'ready';
                document.body.appendChild(node);
            }, 30);
        })()"#,
    );

    let timer_element = page
        .wait_for_selector("#ready", Duration::from_secs(2))
        .await
        .unwrap();
    assert_eq!(timer_element.attribute("id"), Some("ready".to_string()));
    drop(timer_element);

    page.evaluate(
        r#"(() => {
            const node = document.createElement('div');
            node.id = 'native-result';
            document.body.appendChild(node);
            document.querySelector = () => null;
            return document.querySelector('#native-result');
        })()"#,
    );
    let native_element = page
        .wait_for_selector("#native-result", Duration::from_millis(0))
        .await
        .unwrap();
    assert_eq!(native_element.attribute("id"), Some("native-result".to_string()));
}

#[tokio::test(flavor = "current_thread")]
async fn public_selector_wait_preserves_timeout_error() {
    let browser = Browser::new(obscura::EffectivePersona::builtin(
        obscura::StealthProfile::WindowsChrome145,
    ))
    .unwrap();
    let mut page = browser.new_page().await.unwrap();
    page.goto("data:text/html,<body></body>").await.unwrap();

    match page
        .wait_for_selector("#missing", Duration::from_millis(40))
        .await
    {
        Err(obscura::Error::Timeout(_)) => {}
        Err(error) => panic!("unexpected error: {error}"),
        Ok(_) => panic!("missing selector unexpectedly matched"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn public_selector_wait_preserves_queued_navigation_errors() {
    std::env::set_var("OBSCURA_ALLOW_PRIVATE_NETWORK", "1");
    let browser = Browser::new(obscura::EffectivePersona::builtin(
        obscura::StealthProfile::WindowsChrome145,
    ))
    .unwrap();
    let mut page = browser.new_page().await.unwrap();
    page.goto(
        "data:text/html,<body><script>setTimeout(()=>location.href='http://127.0.0.1:1/',10)</script></body>",
    )
    .await
    .unwrap();

    match page
        .wait_for_selector("#missing", Duration::from_secs(1))
        .await
    {
        Err(obscura::Error::Navigation(_)) => {}
        Err(error) => panic!("unexpected error: {error}"),
        Ok(_) => panic!("failed navigation unexpectedly matched"),
    }
}
