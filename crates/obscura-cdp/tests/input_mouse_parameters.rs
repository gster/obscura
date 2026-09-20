use obscura_cdp::dispatch::{dispatch, CdpContext};
use obscura_cdp::types::CdpRequest;
use serde_json::json;

#[tokio::test(flavor = "current_thread")]
async fn mouse_parameters_reject_missing_and_malformed_values() {
    let mut ctx = CdpContext::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));
    let page = ctx.create_page();
    ctx.sessions.insert("mouse-parameters".into(), page);
    let invalid = [
        json!({"x":0,"y":0}),
        json!({"type":"mouseMoved","y":0}),
        json!({"type":"mouseMoved","x":0}),
        json!({"type":"mouseMoved","x":"0","y":0}),
        json!({"type":"mouseMoved","x":0,"y":null}),
        json!({"type":"notMouse","x":0,"y":0}),
        json!({"type":"mousePressed","x":0,"y":0,"button":"primary"}),
        json!({"type":"mousePressed","x":0,"y":0,"buttons":-1}),
        json!({"type":"mousePressed","x":0,"y":0,"clickCount":1.5}),
        json!({"type":"mousePressed","x":0,"y":0,"modifiers":"shift"}),
        json!({"type":"mouseWheel","y":0,"deltaX":0,"deltaY":1}),
        json!({"type":"mouseWheel","x":0,"y":0,"deltaX":"1"}),
        json!({"type":"mouseWheel","x":0,"y":0,"deltaY":null}),
        json!({"type":"mouseWheel","x":0,"y":0,"deltaY":1e100}),
        json!({"type":"mouseWheel","x":0,"y":0,"buttons":32}),
        json!({"type":"mouseWheel","x":0,"y":0,"modifiers":16}),
        json!({"type":"mouseWheel","x":0,"y":0}),
        json!({"type":"mouseWheel","x":0,"y":0,"deltaX":0}),
        json!({"type":"mouseWheel","x":0,"y":0,"deltaY":0}),
    ];
    for (id, params) in invalid.into_iter().enumerate() {
        let response = dispatch(&CdpRequest {
            id:id as u64, method:"Input.dispatchMouseEvent".into(), params:params.clone(),
            session_id:Some("mouse-parameters".into()),
        }, &mut ctx).await;
        let error = response.error.expect("malformed mouse parameters must fail before dispatch");
        assert_eq!(error.code, -32602, "wrong parameter error for {params}: {error:?}");
    }
}

#[cfg(not(feature = "render"))]
#[tokio::test(flavor = "current_thread")]
async fn coordinate_mouse_explicitly_requires_render_geometry() {
    let mut ctx = CdpContext::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));
    let page = ctx.create_page();
    ctx.sessions.insert("mouse-no-render".into(), page);
    for phase in ["mouseMoved", "mouseWheel"] {
        let response = dispatch(&CdpRequest {
            id:1, method:"Input.dispatchMouseEvent".into(),
            params:json!({"type":phase,"x":0,"y":0,"deltaX":0,"deltaY":1}),
            session_id:Some("mouse-no-render".into()),
        }, &mut ctx).await;
        let error = response.error.expect("no-render coordinate input cannot silently succeed");
        assert_eq!(error.code, -32000);
        assert!(error.message.contains("UNSUPPORTED"), "expected explicit capability error: {error:?}");
    }
}
