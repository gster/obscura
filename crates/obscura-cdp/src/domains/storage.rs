use serde_json::{json, Value};

use crate::cookie_params::{parse_cdp_cookie, parse_delete_cookies_params};
use crate::dispatch::CdpContext;
use crate::domains::network::cookie_info_to_cdp_json;

fn cookie_jar_for(
    ctx: &CdpContext,
    params: &Value,
    session_id: &Option<String>,
) -> Result<std::sync::Arc<obscura_net::CookieJar>, String> {
    if params
        .get("browserContextId")
        .is_some_and(|value| !value.is_string())
    {
        return Err("Storage browserContextId must be a string".to_string());
    }
    match params.get("browserContextId").and_then(|value| value.as_str()) {
        Some(id) => ctx
            .browser_context(id)
            .map(|context| context.cookie_jar.clone())
            .ok_or_else(|| format!("Browser context not found: {}", id)),
        None => Ok(ctx
            .get_session_page(session_id)
            .map(|page| page.context.cookie_jar.clone())
            .unwrap_or_else(|| ctx.default_context.cookie_jar.clone())),
    }
}

pub async fn handle(
    method: &str,
    params: &Value,
    ctx: &mut CdpContext,
    session_id: &Option<String>,
) -> Result<Value, String> {
    match method {
        "enable" if params.is_null() || params.as_object().is_some_and(serde_json::Map::is_empty) => {
            Ok(json!({}))
        }
        "enable" => Err("Storage.enable supports only empty params".to_string()),
        "getCookies" => {
            let cookies = cookie_jar_for(ctx, params, session_id)?.get_all_cookies();
            let cdp_cookies: Vec<Value> = cookies.iter().map(cookie_info_to_cdp_json).collect();
            Ok(json!({ "cookies": cdp_cookies }))
        }
        "setCookies" => {
            let cookies = params
                .get("cookies")
                .and_then(Value::as_array)
                .ok_or("Storage.setCookies requires cookies array")?;
            let mut parsed = Vec::with_capacity(cookies.len());
            for (index, cookie) in cookies.iter().enumerate() {
                parsed.push(parse_cdp_cookie(cookie).ok_or_else(|| {
                    format!("Storage.setCookies invalid cookie at index {index}")
                })?);
            }
            cookie_jar_for(ctx, params, session_id)?.set_cookies_from_cdp(parsed);
            Ok(json!({}))
        }
        "clearCookies" => {
            cookie_jar_for(ctx, params, session_id)?.clear();
            Ok(json!({}))
        }
        "deleteCookies" => {
            let filter = parse_delete_cookies_params(params)
                .ok_or("Storage.deleteCookies requires a valid cookie filter")?;
            cookie_jar_for(ctx, params, session_id)?.delete_cookies_filtered(
                &filter.name,
                &filter.domain,
                filter.path.as_deref(),
            );
            Ok(json!({}))
        }
        _ => Err(format!("Unknown Storage method: {method}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use obscura_net::CookieInfo;

    fn sample_cookie(value: &str) -> CookieInfo {
        CookieInfo {
            name: "sid".to_string(),
            value: value.to_string(),
            domain: "example.com".to_string(),
            path: "/".to_string(),
            secure: false,
            http_only: false,
            same_site: String::new(),
            expires: None,
        }
    }

    #[tokio::test]
    async fn clear_cookies_is_scoped_to_browser_context() {
        let mut ctx = CdpContext::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));
        let browser_context_id = ctx.create_browser_context(None).unwrap();
        ctx.browser_context(&browser_context_id)
            .unwrap()
            .cookie_jar
            .set_cookies_from_cdp(vec![sample_cookie("isolated")]);
        ctx.default_context
            .cookie_jar
            .set_cookies_from_cdp(vec![sample_cookie("default")]);

        handle(
            "clearCookies",
            &json!({ "browserContextId": browser_context_id }),
            &mut ctx,
            &None,
        )
        .await
        .unwrap();

        assert!(ctx
            .browser_context(&browser_context_id)
            .unwrap()
            .cookie_jar
            .get_all_cookies()
            .is_empty());
        let default = ctx.default_context.cookie_jar.get_all_cookies();
        assert_eq!(default.len(), 1);
        assert_eq!(default[0].value, "default");
    }

    #[tokio::test]
    async fn clear_cookies_without_context_clears_default_context() {
        let mut ctx = CdpContext::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));
        ctx.default_context
            .cookie_jar
            .set_cookies_from_cdp(vec![sample_cookie("default")]);

        handle("clearCookies", &json!({}), &mut ctx, &None)
            .await
            .unwrap();

        assert!(ctx.default_context.cookie_jar.get_all_cookies().is_empty());
    }

    #[tokio::test]
    async fn unknown_storage_method_errors() {
        let error = handle("auditMethodDoesNotExist", &json!({}), &mut CdpContext::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145)), &None)
            .await
            .expect_err("unknown Storage methods must fail explicitly");
        assert!(error.contains("Unknown Storage method"));
    }

    #[tokio::test]
    async fn storage_mutations_reject_missing_or_malformed_required_params() {
        for (method, params) in [
            ("setCookies", json!({})),
            ("setCookies", json!({"cookies": [null]})),
            ("deleteCookies", json!({})),
            ("getCookies", json!({"browserContextId": 7})),
            ("enable", json!({"invented": true})),
        ] {
            assert!(
                handle(method, &params, &mut CdpContext::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145)), &None)
                    .await
                    .is_err(),
                "must reject {method} {params}"
            );
        }
    }
}
