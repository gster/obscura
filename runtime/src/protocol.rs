use serde::Deserialize;
use serde_json::{json, Value};
use std::io::{self, BufRead, Read, Write};
use tokio::sync::{mpsc, watch};

pub const LIMIT: usize = 64 * 1024;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub id: u64,
    pub method: String,
    pub timeout_ms: u64,
    pub page_id: Option<String>,
    pub page_generation: Option<u64>,
    pub params: Value,
}

pub struct Input {
    pub actions: mpsc::Receiver<Request>,
    pub controls: mpsc::Receiver<Request>,
    pub closed: watch::Receiver<Option<&'static str>>,
}

pub fn input() -> Input {
    let (actions_tx, actions) = mpsc::channel(16);
    let (controls_tx, controls) = mpsc::channel(16);
    let (closed_tx, closed) = watch::channel(None);
    std::thread::spawn(move || {
        let stdin = io::stdin();
        let mut reader = stdin.lock();
        let mut previous = 0;
        loop {
            let mut line = Vec::new();
            let count = (&mut reader)
                .take((LIMIT + 1) as u64)
                .read_until(b'\n', &mut line);
            let reason = match count {
                Ok(0) => "PARENT_EOF",
                Ok(_) if line.len() > LIMIT || !line.ends_with(b"\n") => "LINE_LIMIT",
                Ok(_) => {
                    let request = serde_json::from_slice::<Request>(&line);
                    match request {
                        Ok(request)
                            if request.id > previous
                                && (1..=30000).contains(&request.timeout_ms) =>
                        {
                            previous = request.id;
                            let tx = if matches!(
                                request.method.as_str(),
                                "set_mode" | "begin_recheck" | "finish_recheck" | "close"
                            ) {
                                &controls_tx
                            } else {
                                &actions_tx
                            };
                            if tx.try_send(request).is_ok() {
                                continue;
                            }
                            "MAILBOX_FULL"
                        }
                        _ => "INVALID_REQUEST",
                    }
                }
                Err(_) => "INPUT_FAILED",
            };
            let _ = closed_tx.send(Some(reason));
            break;
        }
    });
    Input {
        actions,
        controls,
        closed,
    }
}

pub fn error(id: u64, code: &str, dispatch_state: &str) -> Value {
    json!({"id": id, "ok": false, "error": {"code": code, "message": code}, "dispatch_state": dispatch_state})
}

pub fn output(value: Value) -> io::Result<()> {
    let mut bytes = serde_json::to_vec(&value)?;
    if bytes.len() + 1 > LIMIT {
        return Err(io::Error::other("OUTPUT_LIMIT"));
    }
    bytes.push(b'\n');
    let mut out = io::stdout().lock();
    out.write_all(&bytes)?;
    out.flush()
}
