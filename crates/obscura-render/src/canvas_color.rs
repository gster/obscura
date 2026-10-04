//! Complete-token parsing for Canvas's sRGB paint colors.
//! Context-dependent and wide-gamut colors require a separate color pipeline.
use cssparser::{Parser, ParserInput, Token};

fn component(input: &mut Parser<'_, '_>) -> Option<(f32, bool)> {
    match input.next().ok()? {
        Token::Number { value, .. } if value.is_finite() => Some((*value, false)),
        Token::Percentage { unit_value, .. } if unit_value.is_finite() => Some((*unit_value, true)),
        _ => None,
    }
}

fn alpha(input: &mut Parser<'_, '_>) -> Option<f32> {
    Some(component(input)?.0.clamp(0.0, 1.0))
}

fn function(name: &str, input: &mut Parser<'_, '_>) -> Option<[f32; 4]> {
    let hsl = name.eq_ignore_ascii_case("hsl") || name.eq_ignore_ascii_case("hsla");
    if !hsl && !name.eq_ignore_ascii_case("rgb") && !name.eq_ignore_ascii_case("rgba") { return None; }
    let first = if hsl {
        match input.next().ok()? {
            Token::Number { value, .. } if value.is_finite() => (*value, false),
            Token::Dimension { value, unit, .. } if value.is_finite() => {
                let degrees = if unit.eq_ignore_ascii_case("deg") { *value }
                    else if unit.eq_ignore_ascii_case("grad") { *value * 0.9 }
                    else if unit.eq_ignore_ascii_case("rad") { value.to_degrees() }
                    else if unit.eq_ignore_ascii_case("turn") { *value * 360.0 }
                    else { return None; };
                if !degrees.is_finite() { return None; }
                (degrees, false)
            }
            _ => return None,
        }
    } else { component(input)? };
    let legacy = input.try_parse(|input| input.expect_comma()).is_ok();
    let second = component(input)?;
    if legacy { input.expect_comma().ok()?; }
    let third = component(input)?;
    let a = if legacy {
        if input.try_parse(|input| input.expect_comma()).is_ok() { alpha(input)? } else { 1.0 }
    } else if input.try_parse(|input| input.expect_delim('/')).is_ok() { alpha(input)? } else { 1.0 };
    input.expect_exhausted().ok()?;
    if hsl {
        if !second.1 || !third.1 { return None; }
        let h = first.0.rem_euclid(360.0) / 60.0;
        let s = second.0.clamp(0.0, 1.0);
        let l = third.0.clamp(0.0, 1.0);
        let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
        let x = c * (1.0 - (h.rem_euclid(2.0) - 1.0).abs());
        let m = l - c / 2.0;
        let (r, g, b) = match h as u8 {
            0 => (c, x, 0.0), 1 => (x, c, 0.0), 2 => (0.0, c, x),
            3 => (0.0, x, c), 4 => (x, 0.0, c), _ => (c, 0.0, x),
        };
        return Some([((r + m) * 255.0).round(), ((g + m) * 255.0).round(), ((b + m) * 255.0).round(), a * 255.0]);
    }
    if legacy && (first.1 != second.1 || first.1 != third.1) { return None; }
    let channel = |(value, percentage): (f32, bool)|
        (if percentage { value * 255.0 } else { value }).round().clamp(0.0, 255.0);
    Some([channel(first), channel(second), channel(third), a * 255.0])
}

pub fn parse(value: &str) -> Option<[f32; 4]> {
    if value.len() > 4096 { return None; }
    let mut source = ParserInput::new(value);
    let mut input = Parser::new(&mut source);
    let color = match input.next().ok()?.clone() {
        Token::Hash(value) | Token::IDHash(value) => {
            let (r, g, b, a) = cssparser::color::parse_hash_color(value.as_bytes()).ok()?;
            [r as f32, g as f32, b as f32, a * 255.0]
        }
        Token::Ident(value) if value.eq_ignore_ascii_case("transparent") => [0.0; 4],
        Token::Ident(value) => {
            let (r, g, b) = cssparser::color::parse_named_color(&value).ok()?;
            [r as f32, g as f32, b as f32, 255.0]
        }
        Token::Function(name) => {
            let result: Result<_, cssparser::ParseError<'_, ()>> = input.parse_nested_block(|input|
                function(&name, input).ok_or_else(|| input.new_custom_error(())));
            result.ok()?
        }
        _ => return None,
    };
    input.expect_exhausted().ok()?;
    Some(color)
}

pub fn serialize(rgba: [f32; 4]) -> String {
    if rgba[3].round() == 255.0 {
        return format!("#{:02x}{:02x}{:02x}", rgba[0] as u8, rgba[1] as u8, rgba[2] as u8);
    }
    let mut css = format!("rgba({}, {}, {}", rgba[0] as u8, rgba[1] as u8, rgba[2] as u8);
    cssparser::color::serialize_color_alpha(&mut css, Some(rgba[3].round() / 255.0), true).ok();
    css.push(')');
    css
}
