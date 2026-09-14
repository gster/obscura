mod browser;
mod protocol;
#[cfg(test)]
mod referrer_tests;
mod takeover;

use browser::BrowserRuntime;
use std::{path::PathBuf, time::Duration};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 5 || args[1] != "--workspace" || args[3] != "--owner" {
        return Err("expected fixed --workspace and --owner arguments".into());
    }
    let mut runtime = BrowserRuntime::new(PathBuf::from(&args[2]))?;
    let mut input = protocol::input();
    loop {
        let takeover_deadline = runtime.takeover_deadline();
        let request = tokio::select! {
            biased;
            _ = input.closed.changed() => break,
            value = input.controls.recv() => value,
            _ = tokio::time::sleep_until(takeover_deadline) => {
                let deadline=runtime.manual_operation_deadline();
                let closed=runtime.takeover.as_ref().map(|c|c.closure());
                let result=tokio::select! {
                    biased;
                    _=input.closed.changed()=>break,
                    _=takeover::closed(closed)=>{
                        runtime.revoke_takeover("CONNECTION_CLOSED");runtime.takeover=None;continue;
                    },
                    result=tokio::time::timeout_at(deadline,runtime.manual_tick())=>result,
                };
                if result.is_err() {
                    runtime.poisoned=true;
                    runtime.revoke_takeover("FRAME_TIMEOUT");
                }
                continue;
            },
            value = takeover::receive(&mut runtime.takeover) => {
                let deadline=runtime.manual_operation_deadline();
                let closed=runtime.takeover.as_ref().map(|c|c.closure());
                let result=tokio::select! {
                    biased;
                    _=input.closed.changed()=>break,
                    _=takeover::closed(closed)=>{
                        runtime.revoke_takeover("CONNECTION_CLOSED");
                        runtime.takeover=None;
                        continue;
                    },
                    result=tokio::time::timeout_at(deadline,runtime.local_control(value))=>result,
                };
                if result.is_err() {runtime.poisoned=true;runtime.revoke_takeover("INPUT_TIMEOUT");}
                continue;
            },
            value = input.actions.recv() => value,
        };
        let Some(request) = request else {
            break;
        };
        let close = request.method == "close";
        let id = request.id;
        let response = tokio::select! {
            biased;
            _ = input.closed.changed() => break,
            result = tokio::time::timeout(Duration::from_millis(request.timeout_ms), runtime.handle(&request)) => {
                match result {
                    Ok(result) => result,
                    Err(_) => {
                        runtime.poisoned = true;
                        protocol::error(id, "BROWSER_TIMEOUT", "UNKNOWN")
                    }
                }
            }
        };
        protocol::output(response)?;
        if close {
            break;
        }
    }
    // Drop Page/V8 on the owning thread; the parent supervises a stuck shutdown.
    drop(runtime);
    Ok(())
}
