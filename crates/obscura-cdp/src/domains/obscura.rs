use base64::Engine as _;
use obscura_browser::{HistoryBodyKey, NetworkHistoryQuery, PageInstanceId};
use serde_json::{json, Map, Value};

use crate::dispatch::{CdpContext, CdpNetworkHistory};

const DEFAULT_PAGE_SIZE: usize = 100;
const MAX_PAGE_SIZE: usize = 1000;
const DEFAULT_BODY_CHUNK: usize = 64 * 1024;
const MAX_BODY_CHUNK: usize = 1024 * 1024;

fn params_object(
    method: &str,
    params: &Value,
    allowed: &[&str],
) -> Result<Map<String, Value>, String> {
    let object = if params.is_null() {
        Map::new()
    } else {
        params
            .as_object()
            .ok_or_else(|| format!("Obscura.{method} params must be an object"))?
            .clone()
    };
    if let Some(name) = object.keys().find(|name| !allowed.contains(&name.as_str())) {
        return Err(format!("Obscura.{method} does not support parameter {name}"));
    }
    Ok(object)
}

fn required_string<'a>(
    method: &str,
    params: &'a Map<String, Value>,
    name: &str,
) -> Result<&'a str, String> {
    params
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("Obscura.{method} requires string {name}"))
}

fn optional_usize(
    method: &str,
    params: &Map<String, Value>,
    name: &str,
    default: usize,
) -> Result<usize, String> {
    let Some(value) = params.get(name) else { return Ok(default); };
    let value = value
        .as_u64()
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| format!("Obscura.{method} parameter {name} must be a non-negative integer"))?;
    Ok(value)
}

fn history<'a>(
    method: &str,
    ctx: &'a CdpContext,
    history_id: &str,
) -> Result<&'a CdpNetworkHistory, String> {
    ctx.network_histories
        .get(history_id)
        .ok_or_else(|| format!("Obscura.{method} network history not found: {history_id}"))
}

fn pages_value(page: &obscura_browser::NetworkHistoryPage) -> Value {
    Value::Array(
        page.pages
            .iter()
            .map(|(page_instance_id, display_page_id)| {
                json!({
                    "pageInstanceId": page_instance_id,
                    "displayPageId": display_page_id,
                    "closed": page.closed_pages.contains(page_instance_id),
                })
            })
            .collect(),
    )
}

pub async fn handle(
    method: &str,
    params: &Value,
    ctx: &CdpContext,
) -> Result<Value, String> {
    match method {
        "getNetworkHistories" => {
            params_object(method, params, &[])?;
            let mut histories = ctx.network_histories.values().collect::<Vec<_>>();
            histories.sort_by(|left, right| left.history.id().0.cmp(&right.history.id().0));
            let histories = histories.into_iter().map(|entry| {
                let page = entry.history.query(NetworkHistoryQuery {
                    after_sequence: 0,
                    limit: 1,
                    page_instance_id: None,
                });
                json!({
                    "historyId": page.history_id,
                    "browserContextId": entry.browser_context_id,
                    "live": entry.live,
                    "finalized": page.finalized,
                    "terminalFailure": page.terminal_failure,
                })
            }).collect::<Vec<_>>();
            Ok(json!({
                "schemaVersion": obscura_browser::network_history::NETWORK_HISTORY_SCHEMA_VERSION,
                "histories": histories,
                "recoveryFailures": ctx.network_history_recovery_failures,
            }))
        }
        "getNetworkHistory" => {
            let params = params_object(
                method,
                params,
                &["historyId", "afterSequence", "limit", "pageInstanceId"],
            )?;
            let history_id = required_string(method, &params, "historyId")?;
            let after_sequence = params.get("afterSequence").map_or(Ok(0), |value| {
                value.as_u64().ok_or_else(|| {
                    "Obscura.getNetworkHistory parameter afterSequence must be a non-negative integer"
                        .to_string()
                })
            })?;
            let limit = optional_usize(method, &params, "limit", DEFAULT_PAGE_SIZE)?;
            if limit == 0 || limit > MAX_PAGE_SIZE {
                return Err(format!(
                    "Obscura.getNetworkHistory limit must be between 1 and {MAX_PAGE_SIZE}"
                ));
            }
            let page_instance_id = params
                .get("pageInstanceId")
                .map(|value| {
                    value
                        .as_str()
                        .map(|value| PageInstanceId(value.to_string()))
                        .ok_or_else(|| {
                            "Obscura.getNetworkHistory parameter pageInstanceId must be a string"
                                .to_string()
                        })
                })
                .transpose()?;
            let entry = history(method, ctx, history_id)?;
            let mut page = entry.history.query(NetworkHistoryQuery {
                after_sequence,
                limit: limit + 1,
                page_instance_id,
            });
            let has_more = page.records.len() > limit;
            page.records.truncate(limit);
            let next_sequence = page
                .records
                .last()
                .map(|record| record.sequence)
                .unwrap_or(after_sequence);
            let records = serde_json::to_value(&page.records)
                .map_err(|error| format!("Obscura.getNetworkHistory serialization failed: {error}"))?;
            Ok(json!({
                "schemaVersion": obscura_browser::network_history::NETWORK_HISTORY_SCHEMA_VERSION,
                "historyId": page.history_id,
                "browserContextId": entry.browser_context_id,
                "live": entry.live,
                "records": records,
                "nextSequence": next_sequence,
                "hasMore": has_more,
                "pages": pages_value(&page),
                "finalized": page.finalized,
                "terminalFailure": page.terminal_failure,
            }))
        }
        "getNetworkBody" => {
            let params = params_object(method, params, &["historyId", "bodyKey", "offset", "length"])?;
            let history_id = required_string(method, &params, "historyId")?;
            let body_key = required_string(method, &params, "bodyKey")?;
            let offset = optional_usize(method, &params, "offset", 0)?;
            let length = optional_usize(method, &params, "length", DEFAULT_BODY_CHUNK)?;
            if length > MAX_BODY_CHUNK {
                return Err(format!(
                    "Obscura.getNetworkBody length must not exceed {MAX_BODY_CHUNK}"
                ));
            }
            let entry = history(method, ctx, history_id)?;
            let chunk = entry
                .history
                .read_body(&HistoryBodyKey(body_key.to_string()), offset, length)
                .map_err(|error| format!("Obscura.getNetworkBody failed: {error}"))?;
            Ok(json!({
                "historyId": history_id,
                "bodyKey": body_key,
                "data": base64::engine::general_purpose::STANDARD.encode(&chunk.bytes),
                "base64Encoded": true,
                "offset": chunk.offset,
                "totalSize": chunk.total_size,
                "eof": chunk.eof,
            }))
        }
        _ => Err(format!("Unknown Obscura method: {method}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use obscura_browser::{
        HistoryBodyCandidate, NetworkEvent, NetworkHistoryFailureKind, NetworkHistoryQuery,
    };
    use std::collections::HashMap;
    use std::sync::Arc;

    fn context() -> CdpContext {
        CdpContext::new(obscura_net::EffectivePersona::builtin(
            obscura_net::StealthProfile::WindowsChrome145,
        ))
    }

    fn event(request_id: &str, generation: u64, body_id: &str) -> NetworkEvent {
        NetworkEvent {
            document_generation: generation,
            document_url: format!("https://example.test/document-{generation}"),
            initiator_request_id: None,
            retired_document_url: None,
            pending: false,
            error: None,
            request_body_present: false,
            request_body_request_id: None,
            request_body_size: 0,
            transport_request_body_present: false,
            transport_request_body_request_id: None,
            transport_request_body_size: 0,
            request_started: false,
            redirect: false,
            response_body_request_id: Some(body_id.to_string()),
            response_body_capture_error: None,
            request_id: request_id.to_string(),
            url: format!("https://example.test/{generation}"),
            method: "GET".to_string(),
            resource_type: "Fetch".to_string(),
            status: 200,
            status_text: "OK".to_string(),
            headers: HashMap::new(),
            response_headers: Arc::new(HashMap::new()),
            raw_headers: None,
            request_raw_headers: None,
            body_size: 4,
            timestamp: generation as f64,
        }
    }

    #[tokio::test]
    async fn browser_history_survives_context_disposal_with_pagination_and_exact_body_chunks() {
        let mut ctx = context();
        let context_id = ctx.create_browser_context(None).unwrap();
        let history = ctx.browser_context(&context_id).unwrap().network_history();
        let first = history.register_page("page-one").unwrap();
        let second = history.register_page("page-two").unwrap();

        let first_sequence = first.append_event(
            &event("same-request", 1, "same-body"),
            None,
            None,
            Some(HistoryBodyCandidate::from_bytes("same-body", vec![0, 0xff, 1, 2])),
        ).unwrap();
        let second_sequence = first.append_event(
            &event("same-request", 2, "same-body"),
            None,
            None,
            Some(HistoryBodyCandidate::from_bytes("same-body", vec![3, 4, 5, 6])),
        ).unwrap();
        let third_sequence = second.append_event(
            &event("same-request", 1, "same-body"),
            None,
            None,
            Some(HistoryBodyCandidate::from_bytes("same-body", vec![7, 8, 9, 10])),
        ).unwrap();
        assert_eq!((first_sequence, second_sequence, third_sequence), (1, 2, 3));

        let terminal = history.fail(
            NetworkHistoryFailureKind::Producer,
            "producer stopped",
            Some(second.page_instance_id().clone()),
            Some("same-request".to_string()),
        );
        first.close().unwrap();
        second.close().unwrap();
        ctx.dispose_browser_context(&context_id).unwrap();

        let history_id = history.id().0;
        let listed = handle("getNetworkHistories", &json!({}), &ctx).await.unwrap();
        let listed_entry = listed["histories"].as_array().unwrap().iter()
            .find(|entry| entry["historyId"] == history_id).unwrap();
        assert_eq!(listed_entry["browserContextId"], context_id);
        assert_eq!(listed_entry["live"], false);
        assert_eq!(listed_entry["finalized"], true);

        let first_page = handle(
            "getNetworkHistory",
            &json!({"historyId": history_id, "limit": 2}),
            &ctx,
        ).await.unwrap();
        assert_eq!(first_page["records"].as_array().unwrap().len(), 2);
        assert_eq!(first_page["records"][0]["event"]["documentGeneration"], 1);
        assert_eq!(first_page["records"][1]["event"]["documentGeneration"], 2);
        assert_eq!(first_page["hasMore"], true);
        assert_eq!(first_page["nextSequence"], 2);
        assert_eq!(first_page["live"], false);
        assert_eq!(first_page["finalized"], true);
        assert_eq!(first_page["terminalFailure"]["message"], terminal.message);
        assert!(first_page["pages"].as_array().unwrap().iter()
            .all(|page| page["closed"] == true));

        let second_page = handle(
            "getNetworkHistory",
            &json!({"historyId": history_id, "afterSequence": 2, "limit": 2}),
            &ctx,
        ).await.unwrap();
        assert_eq!(second_page["records"].as_array().unwrap().len(), 1);
        assert_eq!(second_page["records"][0]["sequence"], 3);
        assert_eq!(second_page["hasMore"], false);
        let first_body_key = first_page["records"][0]["responseBody"]["key"]
            .as_str().unwrap();
        let second_page_body_key = second_page["records"][0]["responseBody"]["key"]
            .as_str().unwrap();
        assert_ne!(first_body_key, second_page_body_key);

        let body = handle(
            "getNetworkBody",
            &json!({
                "historyId": history_id,
                "bodyKey": first_body_key,
                "offset": 1,
                "length": 2,
            }),
            &ctx,
        ).await.unwrap();
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(body["data"].as_str().unwrap()).unwrap(),
            vec![0xff, 1],
        );
        assert_eq!(body["offset"], 1);
        assert_eq!(body["totalSize"], 4);
        assert_eq!(body["eof"], false);
    }

    #[test]
    fn connection_teardown_closes_pages_before_finalizing_the_default_history() {
        let (history, page_instance_id) = {
            let mut ctx = context();
            let page_id = ctx.create_page();
            let history = ctx.default_context.network_history();
            let page_instance_id = ctx.get_page(&page_id)
                .unwrap().network_history_page_instance_id().unwrap();
            ctx.finalize_network_histories();
            (history, page_instance_id)
        };
        let page = history.query(NetworkHistoryQuery {
            after_sequence: 0,
            limit: 1,
            page_instance_id: None,
        });
        assert!(page.finalized);
        assert!(page.closed_pages.contains(&page_instance_id));
    }

    #[tokio::test]
    async fn history_commands_reject_unbounded_or_unknown_inputs() {
        let ctx = context();
        for (method, params) in [
            ("getNetworkHistories", json!({"invented": true})),
            ("getNetworkHistory", json!({"historyId": "missing", "limit": 1001})),
            ("getNetworkBody", json!({"historyId": "missing", "bodyKey": "body", "length": MAX_BODY_CHUNK + 1})),
        ] {
            assert!(handle(method, &params, &ctx).await.is_err());
        }
    }

    #[tokio::test]
    async fn history_discovery_reports_corrupt_archives_without_hiding_live_history() {
        let root = std::env::temp_dir().join(format!(
            "obscura-cdp-history-recovery-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ));
        let corrupt = root.join("network-history/corrupt-archive");
        std::fs::create_dir_all(&corrupt).unwrap();
        std::fs::write(corrupt.join("manifest.json"), b"not-json").unwrap();
        let browser_context = Arc::new(obscura_browser::BrowserContext::with_options(
            "recovery-test".to_string(),
            obscura_net::EffectivePersona::builtin(
                obscura_net::StealthProfile::WindowsChrome145,
            ),
            obscura_browser::BrowserContextOptions {
                storage_dir: Some(root.clone()),
                ..Default::default()
            },
        ));
        let live_history_id = browser_context.network_history().id().0;
        let ctx = CdpContext::new_with_shared_context(browser_context);

        let listed = handle("getNetworkHistories", &json!({}), &ctx).await.unwrap();
        assert!(listed["histories"].as_array().unwrap().iter().any(|history| {
            history["historyId"] == live_history_id && history["live"] == true
        }));
        let failures = listed["recoveryFailures"].as_array().unwrap();
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0]["archivePath"], corrupt.display().to_string());
        assert_eq!(failures[0]["error"]["kind"], "recovery");
        assert!(failures[0]["error"]["message"].as_str().unwrap()
            .contains("manifest is invalid"));
        let _ = std::fs::remove_dir_all(root);
    }
}
