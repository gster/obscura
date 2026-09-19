use serde_json::{json, Value};

fn only_fields(method: &str, params: &Value, allowed: &[&str]) -> Result<(), String> {
    if params.is_null() {
        return Ok(());
    }
    let params = params
        .as_object()
        .ok_or_else(|| format!("Browser.{method} params must be an object"))?;
    if let Some(name) = params.keys().find(|name| !allowed.contains(&name.as_str())) {
        return Err(format!("Browser.{method} does not support parameter {name}"));
    }
    Ok(())
}

fn empty(method: &str, params: &Value) -> Result<(), String> {
    only_fields(method, params, &[])
}

fn optional_string(method: &str, params: &Value, name: &str) -> Result<(), String> {
    if params.get(name).is_some_and(|value| !value.is_string()) {
        return Err(format!("Browser.{method} parameter {name} must be a string"));
    }
    Ok(())
}

fn window_id(method: &str, params: &Value) -> Result<(), String> {
    if params.get("windowId").and_then(Value::as_i64).is_none() {
        return Err(format!("Browser.{method} requires integer windowId"));
    }
    Ok(())
}

fn validate_window_bounds(params: &Value) -> Result<(), String> {
    let method = "setWindowBounds";
    only_fields(method, params, &["windowId", "bounds"])?;
    window_id(method, params)?;
    if params.get("windowId").and_then(Value::as_i64) != Some(1) {
        return Err("Browser.setWindowBounds supports only windowId 1".to_string());
    }
    let bounds = params
        .get("bounds")
        .and_then(Value::as_object)
        .ok_or("Browser.setWindowBounds requires bounds object")?;
    if bounds.len() != 2
        || bounds.get("width").and_then(Value::as_u64).is_none()
        || bounds.get("height").and_then(Value::as_u64).is_none()
    {
        return Err(
            "Browser.setWindowBounds supports only positive integer bounds.width and bounds.height"
                .to_string(),
        );
    }
    if bounds["width"].as_u64() == Some(0) || bounds["height"].as_u64() == Some(0) {
        return Err(
            "Browser.setWindowBounds supports only positive integer bounds.width and bounds.height"
                .to_string(),
        );
    }
    Ok(())
}

fn validate_download_behavior(params: &Value) -> Result<(), String> {
    let method = "setDownloadBehavior";
    only_fields(
        method,
        params,
        &["behavior", "browserContextId", "downloadPath", "eventsEnabled"],
    )?;
    let behavior = params
        .get("behavior")
        .and_then(Value::as_str)
        .ok_or("Browser.setDownloadBehavior requires string behavior")?;
    if !matches!(behavior, "deny" | "allow" | "allowAndName" | "default") {
        return Err(format!("Browser.setDownloadBehavior unsupported behavior: {behavior}"));
    }
    if matches!(behavior, "allow" | "allowAndName")
        && params.get("downloadPath").and_then(Value::as_str).is_none()
    {
        return Err(
            "Browser.setDownloadBehavior requires string downloadPath for allow modes".to_string(),
        );
    }
    optional_string(method, params, "browserContextId")?;
    optional_string(method, params, "downloadPath")?;
    if params.get("eventsEnabled").is_some_and(|value| !value.is_boolean()) {
        return Err("Browser.setDownloadBehavior eventsEnabled must be boolean".to_string());
    }
    Ok(())
}

pub async fn handle(method: &str, params: &Value) -> Result<Value, String> {
    match method {
        "getVersion" => {
            empty(method, params)?;
            Ok(json!({
                "protocolVersion": "1.3",
                "product": "Chrome/145.0.0.0",
                "revision": "@0000000000000000000000000000000000000000",
                "userAgent": "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/145.0.0.0 Safari/537.36",
                "jsVersion": "14.5.0.0",
            }))
        }
        "close" => {
            empty(method, params)?;
            Ok(json!({}))
        }
        "getWindowForTarget" => {
            only_fields(method, params, &["targetId"])?;
            optional_string(method, params, "targetId")?;
            Ok(json!({
                "windowId": 1,
                "bounds": {
                    "left": 0,
                    "top": 0,
                    "width": 1280,
                    "height": 720,
                    "windowState": "normal",
                }
            }))
        }
        "setWindowBounds" => {
            validate_window_bounds(params)?;
            Ok(json!({}))
        }
        "setDownloadBehavior" => {
            validate_download_behavior(params)?;
            Ok(json!({}))
        }
        "getWindowBounds" => {
            only_fields(method, params, &["windowId"])?;
            window_id(method, params)?;
            Ok(json!({
                "bounds": { "left": 0, "top": 0, "width": 1280, "height": 720, "windowState": "normal" }
            }))
        }
        _ => Err(format!("Unknown Browser method: {}", method)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fixed_browser_methods_reject_ignored_parameters() {
        for (method, params) in [
            ("getVersion", json!({"invented": true})),
            ("getWindowBounds", json!({})),
            ("setDownloadBehavior", json!({"behavior": "allow"})),
        ] {
            let error = handle(method, &params)
                .await
                .expect_err("invalid params must not return placeholder success");
            assert!(error.starts_with("Browser."), "{method}: {error}");
        }
    }

    #[tokio::test]
    async fn unimplemented_browser_mutations_error_instead_of_acknowledging() {
        for method in ["grantPermissions", "resetPermissions"] {
            let error = handle(method, &json!({}))
                .await
                .expect_err("unimplemented Browser mutation must fail");
            assert!(error.contains("Unknown Browser method"), "{method}: {error}");
        }
    }

    #[tokio::test]
    async fn playwright_headless_window_bounds_shape_is_explicit() {
        handle(
            "setWindowBounds",
            &json!({"windowId": 1, "bounds": {"width": 1282, "height": 800}}),
        )
        .await
        .expect("the observed Playwright headless bounds should be accepted");

        for params in [
            json!({"windowId": 2, "bounds": {"width": 1282, "height": 800}}),
            json!({"windowId": 1, "bounds": {"width": 0, "height": 800}}),
            json!({"windowId": 1, "bounds": {"width": 1282, "height": 800, "left": 0}}),
        ] {
            handle("setWindowBounds", &params)
                .await
                .expect_err("unqualified window bounds must fail");
        }
    }

    #[tokio::test]
    async fn omitted_and_empty_params_are_equivalent_for_parameterless_methods() {
        for params in [Value::Null, json!({})] {
            handle("getVersion", &params)
                .await
                .expect("CDP permits omitted params for a parameterless command");
        }
    }

    #[tokio::test]
    async fn playwright_download_initializer_has_an_explicit_limited_shape() {
        handle(
            "setDownloadBehavior",
            &json!({
                "behavior": "allowAndName",
                "downloadPath": "/tmp/obscura-downloads",
                "eventsEnabled": true,
            }),
        )
        .await
        .expect("the documented Playwright initializer shape should be accepted");
    }
}
