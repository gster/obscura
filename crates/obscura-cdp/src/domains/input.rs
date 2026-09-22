use serde_json::{json, Value};

use crate::dispatch::CdpContext;

enum CoordinateInput {
    Mouse(obscura_browser::MouseInput),
    Wheel(obscura_browser::WheelInput),
}

fn coordinate_input(params: &Value) -> Result<CoordinateInput, String> {
    use obscura_browser::{MouseInput, MouseInputPhase};
    let phase = match params.get("type").and_then(Value::as_str) {
        Some("mouseMoved" | "mouseWheel") => MouseInputPhase::Move,
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
    let mouse = MouseInput {
        phase, x: coordinate("x")?, y: coordinate("y")?, button, force,
        buttons: integer("buttons", default_buttons, 31)? as u8,
        click_count: integer("clickCount", 0, i32::MAX as u64)? as u32,
        modifiers: integer("modifiers", 0, 15)? as u8,
    };
    if params["type"] == "mouseWheel" {
        if params.get("deltaX").is_none() || params.get("deltaY").is_none() {
            return Err("Invalid mouseWheel: deltaX and deltaY are required".into());
        }
        Ok(CoordinateInput::Wheel(obscura_browser::WheelInput {
            x: mouse.x, y: mouse.y, delta_x: coordinate("deltaX")?, delta_y: coordinate("deltaY")?,
            button: mouse.button, buttons: mouse.buttons, modifiers: mouse.modifiers,
        }))
    } else {
        Ok(CoordinateInput::Mouse(mouse))
    }
}

fn keyboard_input(params: &Value) -> Result<obscura_browser::KeyboardInput, String> {
    use obscura_browser::{KeyboardInput, KeyboardInputPhase};
    let phase = match params.get("type").and_then(Value::as_str) {
        Some("keyDown") => KeyboardInputPhase::KeyDown,
        Some("rawKeyDown") => KeyboardInputPhase::RawKeyDown,
        Some("char") => KeyboardInputPhase::Char,
        Some("keyUp") => KeyboardInputPhase::KeyUp,
        _ => return Err("Invalid keyboard type".into()),
    };
    let string = |name: &str| -> Result<String, String> {
        match params.get(name) {
            None => Ok(String::new()),
            Some(value) => value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("Invalid keyboard {name}: expected a string")),
        }
    };
    let boolean = |name: &str| -> Result<bool, String> {
        match params.get(name) {
            None => Ok(false),
            Some(value) => value
                .as_bool()
                .ok_or_else(|| format!("Invalid keyboard {name}: expected a boolean")),
        }
    };
    let integer = |name: &str, min: i64, max: i64| -> Result<i64, String> {
        match params.get(name) {
            None => Ok(0),
            Some(value) => value
                .as_i64()
                .filter(|value| (min..=max).contains(value))
                .ok_or_else(|| format!("Invalid keyboard {name}: expected an integer")),
        }
    };
    if let Some(value) = params.get("timestamp") {
        value
            .as_f64()
            .filter(|value| value.is_finite())
            .ok_or_else(|| "Invalid keyboard timestamp: expected a finite number".to_string())?;
    }
    let commands = match params.get("commands") {
        None => Vec::new(),
        Some(Value::Array(values)) => values
            .iter()
            .map(|value| {
                value.as_str().map(str::to_owned).ok_or_else(|| {
                    "Invalid keyboard commands: expected an array of strings".to_string()
                })
            })
            .collect::<Result<Vec<_>, _>>()?,
        Some(_) => {
            return Err("Invalid keyboard commands: expected an array of strings".into())
        }
    };
    Ok(KeyboardInput {
        phase,
        key: string("key")?,
        code: string("code")?,
        text: string("text")?,
        unmodified_text: string("unmodifiedText")?,
        windows_virtual_key_code: integer("windowsVirtualKeyCode", 0, i32::MAX as i64)? as i32,
        native_virtual_key_code: integer("nativeVirtualKeyCode", i32::MIN as i64, i32::MAX as i64)? as i32,
        modifiers: integer("modifiers", 0, 15)? as u32,
        auto_repeat: boolean("autoRepeat")?,
        location: integer("location", 0, 3)? as u32,
        is_keypad: boolean("isKeypad")?,
        is_system_key: boolean("isSystemKey")?,
        commands,
    })
}

async fn process_input_navigation(
    ctx: &mut CdpContext,
    session_id: &Option<String>,
) -> Result<(), String> {
    let moved_frame = {
        let page = ctx
            .get_session_page_mut(session_id)
            .ok_or_else(|| "Input requires an attached page session".to_string())?;
        let moved = page
            .process_pending_navigation()
            .await
            .map_err(|error| error.to_string())?;
        moved.then(|| (page.id.clone(), page.frame_id.clone(), page.url_string()))
    };
    if let Some((page_id, frame_id, url)) = moved_frame {
        let loader_id = ctx
            .current_loader_ids
            .get(&page_id)
            .cloned()
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
    Ok(())
}

pub async fn handle(
    method: &str,
    params: &Value,
    ctx: &mut CdpContext,
    session_id: &Option<String>,
) -> Result<Value, String> {
    match method {
        "dispatchMouseEvent" => {
            let input = coordinate_input(params)?;
            if ctx.mouse_and_key_input_ignored(session_id)? {
                return Ok(json!({}));
            }
            let page = ctx.get_session_page_mut(session_id)
                .ok_or_else(|| "Input requires an attached page session".to_string())?;
            match input {
                CoordinateInput::Mouse(input) => page.dispatch_mouse_input(input)?,
                CoordinateInput::Wheel(input) => page.dispatch_wheel_input(input)?,
            }
            process_input_navigation(ctx, session_id).await?;

            Ok(json!({}))
        }
        // Chrome's Input.insertText: Playwright's fill() focuses the field in
        // page and then types the whole value through this one call (#577).
        "insertText" => {
            let text = params
                .get("text")
                .and_then(Value::as_str)
                .ok_or_else(|| "Invalid insertText text: expected a string".to_string())?;
            let page = ctx
                .get_session_page_mut(session_id)
                .ok_or_else(|| "Input requires an attached page session".to_string())?;
            page.insert_text(text)?;
            process_input_navigation(ctx, session_id).await?;
            Ok(json!({}))
        }
        "dispatchKeyEvent" => {
            let input = keyboard_input(params)?;
            if ctx.mouse_and_key_input_ignored(session_id)? {
                return Ok(json!({}));
            }
            let page = ctx
                .get_session_page_mut(session_id)
                .ok_or_else(|| "Input requires an attached page session".to_string())?;
            page.dispatch_keyboard_input(input)?;
            process_input_navigation(ctx, session_id).await?;
            Ok(json!({}))
        }
        "dispatchTouchEvent" => Ok(json!({})),
        "setIgnoreInputEvents" => {
            let ignored = params
                .get("ignore")
                .and_then(Value::as_bool)
                .ok_or_else(|| {
                    "Invalid setIgnoreInputEvents ignore: expected a boolean".to_string()
                })?;
            ctx.set_input_events_ignored(session_id, ignored)?;
            Ok(json!({}))
        }
        _ => Err(format!("Unknown Input method: {}", method)),
    }
}
