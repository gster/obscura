use obscura_cdp::dispatch::{dispatch, CdpContext};
use obscura_cdp::types::{CdpRequest, CdpResponse};
use serde_json::{json, Value};

async fn set_user_agent(ctx: &mut CdpContext, id: u64, params: Value) -> CdpResponse {
    dispatch(
        &CdpRequest {
            id,
            method: "Network.setUserAgentOverride".to_string(),
            params,
            session_id: None,
        },
        ctx,
    )
    .await
}

#[tokio::test(flavor = "current_thread")]
async fn set_user_agent_override_is_unsupported_for_an_initialized_context() {
    let mut ctx = CdpContext::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));
    let calibrated = ctx.default_context.persona().user_agent().to_string();

    for (id, params) in [
        (1, json!({})),
        (2, json!({"userAgent": 42})),
        (3, json!({"userAgent": calibrated})),
        (4, json!({"userAgent": "Custom/1.0"})),
    ] {
        let response = set_user_agent(&mut ctx, id, params).await;
        let error = response.error.expect("override must return a CDP error");
        if id >= 3 {
            assert!(error.message.contains("immutable browser persona"));
        } else {
            assert!(error.message.contains("requires a string userAgent"));
        }
    }
}
