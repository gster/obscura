use serde_json::{json, Value};

use crate::dispatch::CdpContext;

/// Embed a string as a JS string literal (double-quoted, with backslash,
/// quotes, and control characters escaped) for interpolation into generated
/// KeyboardEvent scripts. A plain `replace('\'', ...)` misses newline / NUL /
/// U+2028-29, which terminate the literal and silently drop the event.
fn js_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
}

// Insert `text` at the caret, replacing any non-collapsed selection the way a
// real browser does when you type over selected text (for example after a
// triple-click select-all). selectionStart is null during ordinary typing, so
// the legacy append path is kept when no selection is tracked.
//
// The text is embedded as a JSON string literal rather than escaped by hand
// into single quotes. JSON string syntax is a subset of JavaScript's, so this
// covers the quote and the backslash of issue #433 and the control characters
// they left out: a newline inside a single-quoted literal is a syntax error,
// so the whole snippet was dropped and nothing was inserted. obscura-mcp
// already builds its typing snippet this way.
fn insert_text_js(text: &str) -> String {
    let literal = serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_string());
    format!(
        "(function() {{\
            var t = document.activeElement;\
            if (!t || (t.localName !== 'input' && t.localName !== 'textarea')) return;\
            var ins = {text};\
            var v = t.value || '';\
            var s = t.selectionStart, e = t.selectionEnd;\
            if (s == null) {{\
                globalThis.__obscura_setFieldValue(t, 'value', v + ins);\
            }} else {{\
                s = Math.max(0, Math.min(s, v.length));\
                e = (e == null) ? s : Math.max(0, Math.min(e, v.length));\
                var lo = Math.min(s, e), hi = Math.max(s, e);\
                globalThis.__obscura_setFieldValue(t, 'value', v.slice(0, lo) + ins + v.slice(hi));\
                var caret = lo + ins.length;\
                t.setSelectionRange(caret, caret);\
            }}\
            t.dispatchEvent(globalThis.__obscura_markTrusted(new Event('input', {{bubbles:true}})));\
        }})()",
        text = literal,
    )
}

// Backspace deletes the selected range when there is one, so the common
// "triple-click to select-all, then Backspace to clear" pattern works. With a
// collapsed caret it removes the character before the caret, and with no
// selection tracked it falls back to trimming the last character (legacy).
const BACKSPACE_JS: &str = "(function() {\
    var t = document.activeElement;\
    if (!t || (t.localName !== 'input' && t.localName !== 'textarea')) return;\
    var v = t.value || '';\
    var s = t.selectionStart, e = t.selectionEnd;\
    if (s == null) {\
        globalThis.__obscura_setFieldValue(t, 'value', v.slice(0, -1));\
    } else {\
        s = Math.max(0, Math.min(s, v.length));\
        e = (e == null) ? s : Math.max(0, Math.min(e, v.length));\
        if (s !== e) {\
            var lo = Math.min(s, e), hi = Math.max(s, e);\
            globalThis.__obscura_setFieldValue(t, 'value', v.slice(0, lo) + v.slice(hi));\
            t.setSelectionRange(lo, lo);\
        } else if (s > 0) {\
            globalThis.__obscura_setFieldValue(t, 'value', v.slice(0, s - 1) + v.slice(s));\
            t.setSelectionRange(s - 1, s - 1);\
        }\
    }\
    t.dispatchEvent(globalThis.__obscura_markTrusted(new Event('input', {bubbles:true})));\
})()";

fn mouse_input(params: &Value) -> Result<obscura_browser::MouseInput, String> {
    use obscura_browser::{MouseInput, MouseInputPhase};
    let phase = match params.get("type").and_then(Value::as_str) {
        Some("mouseMoved") => MouseInputPhase::Move,
        Some("mousePressed") => MouseInputPhase::Down,
        Some("mouseReleased") => MouseInputPhase::Up,
        _ => return Err("Invalid Input.dispatchMouseEvent type".into()),
    };
    let coordinate = |name: &str| -> Result<f32, String> {
        let value = params.get(name).and_then(Value::as_f64)
            .ok_or_else(|| format!("Invalid mouse {name}: expected a finite number"))?;
        if !value.is_finite() || !(value as f32).is_finite() {
            return Err(format!("Invalid mouse {name}: expected a finite coordinate"));
        }
        Ok(value as f32)
    };
    let integer = |name: &str, default: u64, max: u64| -> Result<u64, String> {
        match params.get(name) {
            None => Ok(default),
            Some(value) => value.as_u64().filter(|value| *value <= max)
                .ok_or_else(|| format!("Invalid mouse {name}")),
        }
    };
    let button = match params.get("button") {
        None => -1,
        Some(value) => match value.as_str() {
            Some("none") => -1,
            Some("left") => 0,
            Some("middle") => 1,
            Some("right") => 2,
            Some("back") => 3,
            Some("forward") => 4,
            _ => return Err("Invalid mouse button".into()),
        },
    };
    // The qualified path models a mouse. Do not acknowledge pen orientation input
    // while silently replacing its identity and event metadata with a mouse.
    if let Some(value) = params.get("pointerType") {
        match value.as_str() {
            Some("mouse") => {}
            Some("pen") => return Err("UNSUPPORTED: pen pointer input".into()),
            _ => return Err("Invalid mouse pointerType".into()),
        }
    }
    for (name, min, max, integral) in [
        ("tangentialPressure", -1.0, 1.0, false),
        ("tiltX", -90.0, 90.0, true),
        ("tiltY", -90.0, 90.0, true),
        ("twist", 0.0, 359.0, true),
    ] {
        if let Some(value) = params.get(name) {
            let value = value.as_f64().filter(|value| {
                value.is_finite() && *value >= min && *value <= max
                    && (!integral || value.fract() == 0.0)
            }).ok_or_else(|| format!("Invalid mouse {name}"))?;
            if value != 0.0 {
                return Err(format!("UNSUPPORTED: nonzero mouse {name}"));
            }
        }
    }
    let force = match params.get("force") {
        None => 0.0,
        Some(value) => value.as_f64().filter(|value| {
            value.is_finite() && (0.0..=1.0).contains(value)
        }).ok_or_else(|| "Invalid mouse force".to_string())? as f32,
    };
    let default_buttons = if matches!(phase, MouseInputPhase::Down) {
        match button { 0 => 1, 1 => 4, 2 => 2, 3 => 8, 4 => 16, _ => 0 }
    } else { 0 };
    Ok(MouseInput {
        phase, x: coordinate("x")?, y: coordinate("y")?, button, force,
        buttons: integer("buttons", default_buttons, 31)? as u8,
        click_count: integer("clickCount", 0, i32::MAX as u64)? as u32,
        modifiers: integer("modifiers", 0, 15)? as u8,
    })
}

fn modifier_flags(modifiers: u64) -> (bool, bool, bool, bool) {
    // CDP Input.Modifier: Alt=1, Ctrl=2, Meta=4, Shift=8.
    (
        modifiers & 1 != 0,
        modifiers & 2 != 0,
        modifiers & 4 != 0,
        modifiers & 8 != 0,
    )
}

pub async fn handle(
    method: &str,
    params: &Value,
    ctx: &mut CdpContext,
    session_id: &Option<String>,
) -> Result<Value, String> {
    match method {
        "dispatchMouseEvent" => {
            let event_type = params.get("type").and_then(Value::as_str);
            if event_type != Some("mouseWheel") {
                let input = mouse_input(params)?;
                let page = ctx.get_session_page_mut(session_id)
                    .ok_or_else(|| "Input requires an attached page session".to_string())?;
                page.dispatch_mouse_input(input)?;
                let moved = page.process_pending_navigation().await.map_err(|e| e.to_string())?;
                let moved_frame = moved.then(|| (page.id.clone(), page.frame_id.clone(), page.url_string()));
                if let Some((page_id, frame_id, url)) = moved_frame {
                    let loader_id = ctx.current_loader_ids.get(&page_id).cloned()
                        .unwrap_or_else(|| format!("loader-blank-{page_id}"));
                    ctx.pending_events.push(crate::types::CdpEvent {
                        method: "Page.frameNavigated".into(),
                        params: json!({
                            "frame": crate::domains::page::frame_value(
                                &frame_id, None, &loader_id, &url, "text/html",
                            ),
                            "type": "Navigation",
                        }),
                        session_id: session_id.clone(),
                    });
                }
            } else {
                // Wheel remains on the existing implementation until its native
                // scrolling and cancellation path is qualified separately.
                let x = params.get("x").and_then(Value::as_f64).unwrap_or(0.0);
                let y = params.get("y").and_then(Value::as_f64).unwrap_or(0.0);
                let modifiers = params.get("modifiers").and_then(Value::as_u64).unwrap_or(0);
                let (alt_key, ctrl_key, meta_key, shift_key) = modifier_flags(modifiers);
                let delta_x = params.get("deltaX").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let delta_y = params.get("deltaY").and_then(|v| v.as_f64()).unwrap_or(0.0);
                if let Some(page) = ctx.get_session_page_mut(session_id) {
                    let code = format!(
                        "(function() {{\
                            var target = (document.elementFromPoint && document.elementFromPoint({x},{y})) || document.body || document.documentElement;\
                            if (!target) return;\
                            var wheel = globalThis.__obscura_markTrusted(new WheelEvent('wheel', {{bubbles:true,cancelable:true,view:globalThis,clientX:{x},clientY:{y},deltaX:{delta_x},deltaY:{delta_y},deltaMode:0,altKey:{alt_key},ctrlKey:{ctrl_key},metaKey:{meta_key},shiftKey:{shift_key}}}));\
                            if (!target.dispatchEvent(wheel)) return;\
                            var dx = {delta_x}, dy = {delta_y};\
                            var root = document.scrollingElement || document.documentElement || document.body;\
                            var scrollTarget = null;\
                            var el = target;\
                            while (el && el.nodeType === 1 && el !== root && el !== document.body && el !== document.documentElement) {{\
                                var maxX = Math.max(0, (el.scrollWidth || 0) - (el.clientWidth || 0));\
                                var maxY = Math.max(0, (el.scrollHeight || 0) - (el.clientHeight || 0));\
                                var style = null;\
                                try {{ style = getComputedStyle(el); }} catch (_e) {{}}\
                                var ox = style ? (style.overflowX || style.overflow || '') : '';\
                                var oy = style ? (style.overflowY || style.overflow || '') : '';\
                                var allowX = ox === 'auto' || ox === 'scroll' || ox === 'overlay';\
                                var allowY = oy === 'auto' || oy === 'scroll' || oy === 'overlay';\
                                var consumesX = allowX && ((dx > 0 && el.scrollLeft < maxX) || (dx < 0 && el.scrollLeft > 0));\
                                var consumesY = allowY && ((dy > 0 && el.scrollTop < maxY) || (dy < 0 && el.scrollTop > 0));\
                                if (consumesX || consumesY) {{ scrollTarget = el; break; }}\
                                el = el.parentElement;\
                            }}\
                            if (!scrollTarget) scrollTarget = root;\
                            if (scrollTarget === root && root && typeof root.scrollBy === 'function') {{\
                                var beforeX = root.scrollLeft, beforeY = root.scrollTop;\
                                root.scrollBy(dx, dy);\
                                if (root.scrollLeft !== beforeX || root.scrollTop !== beforeY) setTimeout(function() {{\
                                    try {{ document.dispatchEvent(new Event('scroll', {{bubbles:false}})); }} catch (_e) {{}}\
                                    try {{ globalThis.dispatchEvent(new Event('scroll', {{bubbles:false}})); }} catch (_e) {{}}\
                                }}, 0);\
                            }} else if (scrollTarget && typeof scrollTarget.scrollBy === 'function') scrollTarget.scrollBy(dx, dy);\
                        }})()",
                        x = x,
                        y = y,
                        delta_x = delta_x,
                        delta_y = delta_y,
                        alt_key = alt_key,
                        ctrl_key = ctrl_key,
                        meta_key = meta_key,
                        shift_key = shift_key,
                    );
                    page.evaluate(&code);
                }
            }

            Ok(json!({}))
        }
        // Chrome's Input.insertText: Playwright's fill() focuses the field in
        // page and then types the whole value through this one call (#577).
        "insertText" => {
            let text = params.get("text").and_then(|v| v.as_str()).unwrap_or("");
            if let Some(page) = ctx.get_session_page_mut(session_id) {
                page.evaluate(&insert_text_js(text));
            }
            Ok(json!({}))
        }
        "dispatchKeyEvent" => {
            let event_type = params.get("type").and_then(|v| v.as_str()).unwrap_or("");
            let key = params.get("key").and_then(|v| v.as_str()).unwrap_or("");
            let code = params.get("code").and_then(|v| v.as_str()).unwrap_or("");
            let text = params.get("text").and_then(|v| v.as_str()).unwrap_or("");

            if let Some(page) = ctx.get_session_page_mut(session_id) {
                match event_type {
                    "keyDown" | "rawKeyDown" => {
                        let js = format!(
                            "(function() {{\
                                var target = document.activeElement || document.body;\
                                var evt = globalThis.__obscura_markTrusted(new KeyboardEvent('keydown', {{bubbles:true,cancelable:true,key:{key},code:{code}}}));\
                                target.dispatchEvent(evt);\
                            }})()",
                            // Escape backslash BEFORE single-quote (as the text
                            // path below does) so a key like "\" — Chrome's
                            // backslash key — doesn't escape the closing quote
                            // and produce a syntax error that drops the event.
                            key = js_str(key),
                            code = js_str(code),
                        );
                        page.evaluate(&js);

                        if !text.is_empty() && text != "\r" && text != "\n" {
                            page.evaluate(&insert_text_js(text));
                        }

                        if key == "Enter" {
                            // In a textarea Enter inserts a newline; in input fields
                            // it submits the containing form. Real Chrome distinguishes
                            // these two and we should too: previously every Enter tried
                            // to submit the nearest form even from a textarea.
                            let js = "(function() {\
                                var target = document.activeElement;\
                                if (!target) return;\
                                target.dispatchEvent(globalThis.__obscura_markTrusted(new KeyboardEvent('keypress', {bubbles:true,key:'Enter',code:'Enter'})));\
                                if (target.localName === 'textarea') {\
                                    globalThis.__obscura_setFieldValue(target, 'value', (target.value || '') + '\\n');\
                                    target.dispatchEvent(globalThis.__obscura_markTrusted(new Event('input', {bubbles:true})));\
                                } else {\
                                    var form = target.form || (target.closest && target.closest('form'));\
                                    if (form) {{ try {{ if (typeof form.requestSubmit === 'function') {{ form.requestSubmit(); }} else {{ form.submit(); }} }} catch(e) {{}} }}\
                                }\
                            })()";
                            page.evaluate(js);
                        }

                        if key == "Backspace" {
                            page.evaluate(BACKSPACE_JS);
                        }
                    }
                    "keyUp" => {
                        let js = format!(
                            "(function() {{\
                                var target = document.activeElement || document.body;\
                                var evt = globalThis.__obscura_markTrusted(new KeyboardEvent('keyup', {{bubbles:true,key:{key},code:{code}}}));\
                                target.dispatchEvent(evt);\
                            }})()",
                            key = js_str(key),
                            code = js_str(code),
                        );
                        page.evaluate(&js);
                    }
                    "char" => {
                        if !text.is_empty() {
                            page.evaluate(&insert_text_js(text));
                            // Pump event loop so Angular change detection picks up the input
                            page.settle(50).await;
                        }
                    }
                    _ => {}
                }
            }

            Ok(json!({}))
        }
        "dispatchTouchEvent" => Ok(json!({})),
        "setIgnoreInputEvents" => Ok(json!({})),
        _ => Err(format!("Unknown Input method: {}", method)),
    }
}

#[cfg(test)]
mod tests {
    use super::js_str;

    // SEC-501 / #819 — key/code are embedded via js_str; it must escape control
    // characters (newline/CR/tab/NUL/U+2028-29), not just backslash and quote,
    // so a control char cannot terminate the literal and drop the event.
    #[test]
    fn js_str_escapes_control_characters() {
        let lit = js_str("a\nb\r\t'c\\d\"e");
        assert!(
            !lit.contains('\n') && !lit.contains('\r') && !lit.contains('\t'),
            "control characters must be escaped, not left raw: {lit:?}"
        );
        // The result must be a valid JS/JSON string literal that round-trips.
        let decoded: String =
            serde_json::from_str(&lit).expect("the literal must be valid JSON");
        assert_eq!(decoded, "a\nb\r\t'c\\d\"e");
    }
}
