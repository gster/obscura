pub(crate) use obscura_browser::{lifecycle::WaitUntil, BrowserContext, Page};
pub(crate) use obscura_net::{
    interceptor::{InterceptAction, RequestInterceptor},
    CookieJar, ObscuraHttpClient, RequestInfo, Response, StealthHttpClient,
};
pub(crate) use serde_json::{json, Value};
pub(crate) use std::{collections::HashMap, io::Write, sync::Arc};
pub(crate) use url::Url;

struct HtmlFixture(&'static str);

#[async_trait::async_trait]
impl RequestInterceptor for HtmlFixture {
    async fn intercept(&self, request: &RequestInfo) -> InterceptAction {
        InterceptAction::Fulfill(Response {
            status: 200,
            url: request.url.clone(),
            headers: HashMap::from([("content-type".into(), "text/html".into())]),
            body: self.0.as_bytes().to_vec(),
            redirected_from: vec![],
            raw_headers: None,
            request_raw_headers: None,
            request_referrer: None,
        })
    }
}

pub(crate) async fn input_fixture(html: &'static str) -> Page {
    let mut context = BrowserContext::with_storage_and_network(
        "native-input-test".into(),
        obscura_net::EffectivePersona::builtin(
            obscura_net::StealthProfile::WindowsChrome145,
        ),
        None,
        None,
        true,
    );
    let client = Arc::get_mut(&mut context.http_client).unwrap();
    client.block_trackers = false;
    *client.interceptor.write().await = Some(Arc::new(HtmlFixture(html)));
    let mut page = Page::new("native-test".into(), Arc::new(context));
    page.set_viewport((640.0, 480.0));
    page.navigate("http://127.0.0.1/native-input-fixture")
        .await
        .unwrap();
    page
}

pub(crate) fn pixel(page: &Page, x: usize, y: usize) -> [u8; 4] {
    let png = page.screenshot((640.0, 480.0)).unwrap();
    let image = image::load_from_memory_with_format(&png, image::ImageFormat::Png).unwrap();
    assert_eq!(image.color(), image::ColorType::Rgba8);
    image.to_rgba8().get_pixel(x as u32, y as u32).0
}

pub(crate) fn fragment_landing(page: &mut Page, fragment: &str) {
    page.js
        .as_mut()
        .unwrap()
        .scroll_to_fragment(&format!(
            "http://127.0.0.1/native-input-fixture{fragment}"
        ))
        .unwrap();
}

pub(crate) fn history_eval(page: &mut Page, script: &str) -> serde_json::Value {
    let expression = format!("(() => {{try {{return {{ok:true,value:({script})}}}} catch(error) {{return {{ok:false,name:error.name,message:error.message,stack:error.stack}}}}}})()");
    let result = page.js.as_mut().unwrap().evaluate(&expression).unwrap();
    assert_eq!(result["ok"], json!(true), "{result}");
    result
        .get("value")
        .cloned()
        .unwrap_or(serde_json::Value::Null)
}
