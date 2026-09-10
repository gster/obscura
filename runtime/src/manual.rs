//! Local manual sessions own image acknowledgement and input sequencing.
use super::{allowed, BrowserRuntime};
use crate::takeover::{Binding, Control, Frame, ManualReceipt, Operation, Session};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{os::unix::fs::PermissionsExt, time::Duration};
use tokio::time::Instant;
use url::Url;

impl BrowserRuntime {
    pub fn manual_operation_deadline(&self) -> Instant {
        let limit = Instant::now() + Duration::from_secs(2);
        self.manual
            .as_ref()
            .map(|s| s.deadline.min(s.lease_deadline).min(limit))
            .unwrap_or(limit)
    }
    pub fn takeover_deadline(&self) -> Instant {
        self.manual
            .as_ref()
            .map(|s| {
                if s.page_id.is_some() && s.pending_frame.is_none() {
                    s.deadline.min(s.lease_deadline).min(s.next_frame)
                } else {
                    s.deadline.min(s.lease_deadline)
                }
            })
            .unwrap_or_else(|| Instant::now() + Duration::from_secs(3600))
    }

    fn manual_send(&mut self, value: Value) {
        if !self.takeover.as_ref().is_some_and(|c| c.send(value)) {
            self.manual = None;
            self.mode = "PAUSED".into();
            self.takeover = None;
        }
    }

    pub fn revoke_takeover(&mut self, reason: &str) {
        if let Some(session) = self.manual.take() {
            self.mode = "PAUSED".into();
            self.manual_send(session.binding.reply("closed", reason));
        }
    }

    fn manual_receipt_valid(&self, binding: &Binding, receipt: &ManualReceipt) -> bool {
        let path = self.workspace.join("effects.jsonl");
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            return false;
        };
        if !meta.is_file() || meta.len() > 1024 * 1024 || meta.permissions().mode() & 0o077 != 0 {
            return false;
        }
        let Ok(data) = std::fs::read(path) else {
            return false;
        };
        if data.last() != Some(&b'\n')
            || format!("{:x}", Sha256::digest(&data)) != receipt.journal_sha256
        {
            return false;
        }
        let Some(last) = data[..data.len() - 1].rsplit(|b| *b == b'\n').next() else {
            return false;
        };
        serde_json::from_slice::<Value>(last).ok()
            == Some(json!({"attempt_id":binding.attempt_id,
            "effect_seq":receipt.manual_effect_seq,"kind":"MANUAL_EFFECT_POSSIBLE",
            "action_id":format!("manual_{}",binding.generation),"generation":binding.generation}))
    }

    pub async fn local_control(&mut self, value: Option<Value>) {
        let original = value.clone();
        let Some(control) = value.and_then(|v| serde_json::from_value::<Control>(v).ok()) else {
            self.revoke_takeover("CONNECTION_CLOSED");
            self.takeover = None;
            return;
        };
        let binding = control.binding();
        if !self
            .takeover
            .as_ref()
            .is_some_and(|c| c.attempt_id == binding.attempt_id)
            || binding.session_id.is_empty()
            || binding.session_id.len() > 128
            || binding.generation == 0
        {
            self.revoke_takeover("BINDING_MISMATCH");
            self.takeover = None;
            return;
        }
        if let Control::Open {
            ttl_ms,
            manual_receipt,
            lease_ms,
            ..
        } = control
        {
            if self.poisoned
                || self.mode != "PAUSED"
                || binding.generation != self.generation
                || binding.generation <= self.manual_generation
                || self.recheck_generation.is_some()
                || self.manual.is_some()
                || self.manual_sessions.contains(&binding.session_id)
                || self.manual_sessions.len() >= 256
                || !(1..=300000).contains(&ttl_ms)
                || lease_ms.is_some_and(|ms| !(1..=300000).contains(&ms))
                || self.pages.values().any(|p| p.navigation_url().is_some())
            {
                self.revoke_takeover("OPEN_REJECTED");
                self.manual_send(binding.reply("closed", "OPEN_REJECTED"));
                return;
            }
            if manual_receipt
                .as_ref()
                .is_some_and(|r| !self.manual_receipt_valid(&binding, r))
            {
                self.manual_send(binding.reply("closed", "MANUAL_EFFECT_INVALID"));
                return;
            }
            let input_allowed = manual_receipt.is_some();
            self.manual_generation = binding.generation;
            self.manual_sessions.insert(binding.session_id.clone());
            self.manual = Some(Session {
                binding: binding.clone(),
                deadline: Instant::now() + Duration::from_millis(ttl_ms),
                lease_deadline: Instant::now()
                    + Duration::from_millis(lease_ms.unwrap_or(ttl_ms).min(ttl_ms)),
                next_frame: Instant::now(),
                page_id: None,
                frame_sequence: 0,
                pending_frame: None,
                displayed_frame: None,
                input_allowed,
                last_input: None,
            });
            let mut ready = binding.reply(
                "ready",
                if input_allowed {
                    "MANUAL_READY"
                } else {
                    "CONTROL_ONLY"
                },
            );
            ready["capabilities"] = if input_allowed {
                json!([
                    "control",
                    "view",
                    "pointer_click",
                    "text_insert",
                    "key_press"
                ])
            } else {
                json!(["control", "view"])
            };
            ready["pages"] = json!(self.pages.keys().collect::<Vec<_>>());
            self.manual_send(ready);
            return;
        }
        let same = self.manual.as_ref().is_some_and(|s| {
            s.binding.generation == binding.generation && s.binding.session_id == binding.session_id
        });
        if matches!(control, Control::Close { .. }) {
            if same {
                self.revoke_takeover("CLOSED");
            } else {
                self.manual_send(binding.reply("closed", "ALREADY_CLOSED"));
            }
            return;
        }
        if !same {
            self.manual_send(binding.reply("closed", "SESSION_NOT_OPEN"));
            return;
        }
        if self
            .manual
            .as_ref()
            .unwrap()
            .deadline
            .min(self.manual.as_ref().unwrap().lease_deadline)
            <= Instant::now()
        {
            self.revoke_takeover("EXPIRED");
            return;
        }
        match control {
            Control::Lease { lease_ms, .. } => {
                if !(1..=300000).contains(&lease_ms) {
                    self.revoke_takeover("LEASE_INVALID");
                    return;
                }
                let s = self.manual.as_mut().unwrap();
                s.lease_deadline = s
                    .deadline
                    .min(Instant::now() + Duration::from_millis(lease_ms));
            }
            Control::View { page_id, .. } => {
                if !self.pages.contains_key(&page_id)
                    || self.manual.as_ref().unwrap().pending_frame.is_some()
                {
                    self.revoke_takeover("VIEW_REJECTED");
                    return;
                }
                let s = self.manual.as_mut().unwrap();
                s.page_id = Some(page_id);
                s.displayed_frame = None;
                s.next_frame = Instant::now();
            }
            Control::FrameAck { frame_seq, .. } => {
                let s = self.manual.as_mut().unwrap();
                if s.pending_frame
                    .as_ref()
                    .is_some_and(|f| f.sequence == frame_seq)
                {
                    s.displayed_frame = s.pending_frame.take();
                } else if !s
                    .displayed_frame
                    .as_ref()
                    .is_some_and(|f| f.sequence == frame_seq)
                {
                    self.revoke_takeover("FRAME_ACK_INVALID");
                }
            }
            Control::Input {
                input_seq,
                page_id,
                navigation_generation,
                frame_seq,
                width,
                height,
                dpr,
                operation,
                ..
            } => {
                let original = original.unwrap();
                let s = self.manual.as_ref().unwrap();
                if let Some((seq, old, reply)) = &s.last_input {
                    if *seq == input_seq && *old == original {
                        self.manual_send(reply.clone());
                        return;
                    }
                }
                let expected = s.last_input.as_ref().map(|x| x.0 + 1).unwrap_or(1);
                if input_seq != expected {
                    self.revoke_takeover("INPUT_SEQUENCE_INVALID");
                    return;
                }
                let displayed = s.displayed_frame.clone();
                let valid = s.input_allowed
                    && displayed.as_ref().is_some_and(|f| {
                        f.sequence == frame_seq
                            && f.page_id == page_id
                            && f.navigation_generation == navigation_generation
                            && f.width == width
                            && f.height == height
                            && dpr == 1.0
                    });
                let result = if valid {
                    self.manual_input(displayed.as_ref().unwrap(), operation)
                        .await
                } else {
                    Err((
                        if s.input_allowed {
                            "STALE_FRAME"
                        } else {
                            "MANUAL_EFFECT_REQUIRED"
                        },
                        "NOT_SENT",
                    ))
                };
                let mut reply = binding.reply("input_ack", "APPLIED");
                reply["input_seq"] = json!(input_seq);
                match result {
                    Ok(prevented) => {
                        reply["state"] = json!("APPLIED");
                        reply["default_prevented"] = json!(prevented);
                    }
                    Err((reason, dispatch)) => {
                        reply["state"] = json!(if dispatch == "NOT_SENT" {
                            "REJECTED"
                        } else {
                            "UNKNOWN"
                        });
                        reply["reason"] = json!(reason);
                        reply["dispatch_state"] = json!(dispatch);
                    }
                }
                if let Some(s) = &mut self.manual {
                    s.last_input = Some((input_seq, original, reply.clone()));
                    if result.is_ok() || matches!(result, Err(("STALE_FRAME", _))) {
                        s.displayed_frame = None;
                    }
                }
                self.manual_send(reply);
                if result.is_err() && !matches!(result, Err(("STALE_FRAME", "NOT_SENT"))) {
                    self.revoke_takeover("INPUT_FAILED");
                }
            }
            _ => unreachable!(),
        }
    }

    pub async fn manual_tick(&mut self) {
        let Some(session) = &self.manual else {
            return;
        };
        if Instant::now() >= session.deadline.min(session.lease_deadline) {
            self.revoke_takeover("EXPIRED");
            return;
        }
        if session.pending_frame.is_some() || session.next_frame > Instant::now() {
            return;
        }
        let Some(page_id) = session.page_id.clone() else {
            return;
        };
        let binding = session.binding.clone();
        let entry = self.pages.get_mut(&page_id).unwrap();
        entry.page.settle(10).await;
        if entry.navigation_url().is_some() {
            self.revoke_takeover("UNEXPECTED_NAVIGATION");
            return;
        }
        let Some(png) = entry.page.screenshot(entry.page.viewport) else {
            self.revoke_takeover("CAPTURE_UNAVAILABLE");
            return;
        };
        let sha256 = format!("{:x}", Sha256::digest(&png));
        let text_identity = entry
            .page
            .js
            .as_ref()
            .and_then(|js| js.native_text_identity());
        let s = self.manual.as_mut().unwrap();
        s.next_frame = Instant::now() + Duration::from_millis(200);
        if s.displayed_frame.as_ref().is_some_and(|f| {
            f.sha256 == sha256
                && f.navigation_generation == entry.generation
                && f.text_identity == text_identity
        }) {
            return;
        }
        s.frame_sequence += 1;
        let viewport = &self.persona.as_ref().unwrap().viewport;
        let f = Frame {
            page_id: page_id.clone(),
            navigation_generation: entry.generation,
            sequence: s.frame_sequence,
            width: viewport.width,
            height: viewport.height,
            sha256,
            text_identity,
        };
        let metadata = json!({"attempt_id":binding.attempt_id,"generation":binding.generation,"session_id":binding.session_id,
            "page_id":f.page_id,"navigation_generation":f.navigation_generation,"frame_seq":f.sequence,
            "width":f.width,"height":f.height,"dpr":1,"mime_type":"image/png"});
        s.pending_frame = Some(f);
        if !self
            .takeover
            .as_ref()
            .is_some_and(|c| c.send_image(metadata, png))
        {
            self.revoke_takeover("FRAME_OUTPUT_FAILED");
            self.takeover = None;
        }
    }

    async fn manual_input(
        &mut self,
        frame: &Frame,
        operation: Operation,
    ) -> Result<bool, (&'static str, &'static str)> {
        let entry = self
            .pages
            .get_mut(&frame.page_id)
            .ok_or(("STALE_FRAME", "NOT_SENT"))?;
        if entry.generation != frame.navigation_generation || entry.navigation_url().is_some() {
            return Err(("STALE_FRAME", "NOT_SENT"));
        }
        let png = entry
            .page
            .screenshot(entry.page.viewport)
            .ok_or(("CAPTURE_UNAVAILABLE", "NOT_SENT"))?;
        if format!("{:x}", Sha256::digest(&png)) != frame.sha256 {
            return Err(("STALE_FRAME", "NOT_SENT"));
        }
        if let Operation::PointerClick { x, y } = &operation {
            if !x.is_finite()
                || !y.is_finite()
                || *x < 0.0
                || *y < 0.0
                || *x >= frame.width as f32
                || *y >= frame.height as f32
            {
                return Err(("INVALID_COORDINATES", "NOT_SENT"));
            }
        } else if entry
            .page
            .js
            .as_ref()
            .and_then(|js| js.native_text_identity())
            != frame.text_identity
        {
            return Err(("STALE_FRAME", "NOT_SENT"));
        }
        let remaining = self
            .manual
            .as_ref()
            .unwrap()
            .deadline
            .min(self.manual.as_ref().unwrap().lease_deadline)
            .saturating_duration_since(Instant::now())
            .min(Duration::from_millis(500));
        if remaining.is_zero() {
            return Err(("EXPIRED", "NOT_SENT"));
        }
        let js = entry.page.js.as_mut().ok_or(("NO_DOCUMENT", "NOT_SENT"))?;
        let watchdog = js.arm_watchdog(remaining);
        let result = match operation {
            Operation::PointerClick { x, y } => {
                js.native_pointer_click(x, y).map(|r| r.default_prevented)
            }
            Operation::TextInsert { text } => js.native_insert_text(&text),
            Operation::KeyPress { key } => js.native_edit_key(&key),
        };
        if js.disarm_watchdog(watchdog) {
            return Err(("INPUT_TIMEOUT", "UNKNOWN"));
        }
        let result = result?;
        entry.page.settle(20).await;
        if let Some(navigation) = entry.navigation_url() {
            let url = Url::parse(&navigation).map_err(|_| ("INVALID_URL", "SENT"))?;
            if !allowed(&url, &self.origins) {
                return Err(("ORIGIN_NOT_ALLOWED", "SENT"));
            }
            entry.generation += 1;
            entry
                .page
                .process_pending_navigation()
                .await
                .map_err(|_| ("NAVIGATION_FAILED", "SENT"))?;
            entry.url = entry.page.url_string();
            if !Url::parse(&entry.url)
                .ok()
                .is_some_and(|url| allowed(&url, &self.origins))
            {
                return Err(("ORIGIN_NOT_ALLOWED", "SENT"));
            }
        }
        Ok(result)
    }
}
