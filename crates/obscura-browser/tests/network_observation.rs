use obscura_browser::{NetworkEvent, NetworkEventPhase};

fn observation() -> NetworkEvent {
    NetworkEvent {
        document_generation: 7, document_url: "https://example.test/".into(),
        retired_document_url: Some("https://example.test/old".into()),
        initiator_request_id: None, pending: false, error: None,
        request_body_present: false, request_body_request_id: None, request_body_size: 0,
        transport_request_body_present: false, transport_request_body_request_id: None,
        transport_request_body_size: 0, request_started: true, redirect: false,
        response_body_request_id: Some("fetch-1-hop-0".into()),
        response_body_capture_error: None,
        request_id: "fetch-1".into(), url: "https://example.test/api".into(),
        method: "POST".into(), resource_type: "Fetch".into(), status: 200,
        status_text: "OK".into(), headers: Default::default(),
        response_headers: Default::default(), raw_headers: None,
        request_raw_headers: None, body_size: 4, timestamp: 123.5,
    }
}

#[test]
fn phase_classifies_lifecycle_not_prior_start_flag_or_http_status() {
    let mut event = observation();
    assert_eq!(event.phase(), NetworkEventPhase::Completed);
    event.status = 404;
    assert_eq!(event.phase(), NetworkEventPhase::Completed);
    event.pending = true;
    assert_eq!(event.phase(), NetworkEventPhase::Started);
    event.pending = false;
    event.redirect = true;
    assert_eq!(event.phase(), NetworkEventPhase::Redirect);
    event.redirect = false;
    event.error = Some("connection refused".into());
    assert_eq!(event.phase(), NetworkEventPhase::Failed);
}
