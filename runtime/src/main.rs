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
    let mut next_automation_tick = tokio::time::Instant::now() + Duration::from_millis(20);
    loop {
        let takeover_deadline = runtime.takeover_deadline();
        let automation_active = runtime.uses_automation();
        let request = tokio::select! {
            biased;
            _ = input.closed.changed() => break,
            value = input.controls.recv() => value,
            value = input.evidence.recv() => {
                if let Some(request)=value {protocol::output(browser::network::evidence_response(&runtime.network_snapshot(),&request))?;}
                continue;
            },
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
            // Keep the deadline across read actions, and service it before the
            // next action once due. Recreating a sleep let frequent queries
            // indefinitely postpone timers and Worker message delivery.
            _ = tokio::time::sleep_until(next_automation_tick), if automation_active => {
                if !matches!(tokio::time::timeout(Duration::from_millis(30250), runtime.automation_tick()).await, Ok(Ok(()))) {
                    runtime.poisoned = true;
                }
                next_automation_tick = tokio::time::Instant::now() + Duration::from_millis(20);
                continue;
            },
            value = input.actions.recv() => value,
        };
        let Some(request) = request else {
            break;
        };
        let close = request.method == "close";
        let id = request.id;
        let networks=runtime.network_snapshot();
        let hard_timeout=request.timeout_ms + if runtime.uses_automation() {250} else {0};
        let mut timed_out=false;
        let response = {
            let operation=tokio::time::timeout(Duration::from_millis(hard_timeout),runtime.handle(&request));
            tokio::pin!(operation);
            loop {
                tokio::select! {
                    biased;
                    _ = input.closed.changed() => return Ok(()),
                    value=input.evidence.recv()=>{
                        if let Some(read)=value {protocol::output(browser::network::evidence_response(&networks,&read))?;}
                    },
                    result=&mut operation=>break match result {
                        Ok(result)=>result,
                        Err(_)=>{timed_out=true;protocol::error(id,"BROWSER_TIMEOUT","UNKNOWN")}
                    }
                }
            }
        };
        if timed_out {runtime.poisoned=true;}
        protocol::output(response)?;
        if close {
            break;
        }
    }
    // Drop Page/V8 on the owning thread; the parent supervises a stuck shutdown.
    drop(runtime);
    Ok(())
}
