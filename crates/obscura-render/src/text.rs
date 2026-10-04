//! Canvas single-line text backed by the renderer's real font selection.
//! This module does not prepare DOM layout or allocate a page-sized pixmap.
use std::{collections::HashMap, sync::Arc};
use cosmic_text::{Attrs, Buffer, CacheKey, CacheKeyFlags, Color, Family, FontSystem, FontVariations, LayoutGlyph, Metrics, Shaping, Style, SwashCache, Weight, Wrap};
use cssparser::{Parser, ParserInput, Token};
use swash::scale::{image::{Content, Image}, Render, ScaleContext, Source, StrikeWith};
use swash::zeno::{Angle, Format, Mask, Origin, Stroke, Transform, Vector};
use crate::font::{append_emoji_font, create_font_system, resolve_native_font_with_reclaim, NativeFonts, text_may_need_emoji_font, LoadedFamily, VariableSwashCache};

#[derive(Clone, Debug, PartialEq)]
pub struct CanvasFont {
    pub family: String,
    pub size: f32,
    pub weight: u16,
    pub italic: bool,
}

impl CanvasFont {
    /// Parse absolute-size CSS font shorthands without consulting page layout.
    /// Relative sizes need a computed-style environment and are not accepted here.
    pub fn parse(source: &str) -> Option<Self> {
        let mut input = ParserInput::new(source);
        let mut parser = Parser::new(&mut input);
        let mut weight = 400;
        let mut italic = false;
        let mut seen_weight = false;
        let mut seen_style = false;
        let size = loop {
            match parser.next().ok()?.clone() {
                Token::Ident(value) if value.eq_ignore_ascii_case("normal") => {}
                Token::Ident(value) if value.eq_ignore_ascii_case("italic") || value.eq_ignore_ascii_case("oblique") => {
                    if seen_style { return None; }
                    seen_style = true;
                    italic = true;
                }
                Token::Ident(value) if value.eq_ignore_ascii_case("bold") => {
                    if seen_weight { return None; }
                    seen_weight = true;
                    weight = 700;
                }
                Token::Number { value, .. } if (1.0..=1000.0).contains(&value) => {
                    if seen_weight { return None; }
                    seen_weight = true;
                    weight = value.round() as u16;
                }
                Token::Dimension { value, unit, .. } => {
                    let scale = match unit.to_ascii_lowercase().as_str() {
                        "px" => 1.0, "pt" => 96.0 / 72.0, "pc" => 16.0,
                        "in" => 96.0, "cm" => 96.0 / 2.54, "mm" => 96.0 / 25.4,
                        "q" => 96.0 / 101.6, _ => return None,
                    };
                    let size = value * scale;
                    if !size.is_finite() || size <= 0.0 { return None; }
                    break size;
                }
                _ => return None,
            }
        };
        // Canvas ignores authored line height, but it must still be syntactically valid.
        if parser.try_parse(|p| p.expect_delim('/')).is_ok() {
            match parser.next().ok()? {
                Token::Number { value, .. } | Token::Dimension { value, .. } if *value >= 0.0 => {}
                Token::Ident(value) if value.eq_ignore_ascii_case("normal") => {}
                _ => return None,
            }
        }
        let family_start = parser.position();
        while parser.next().is_ok() {}
        let families = crate::style::parse_font_family_list(parser.slice_from(family_start))?;
        let family = crate::style::serialize_font_family_list(&families);
        Some(Self { family, size, weight, italic })
    }

    pub fn css(&self) -> String {
        let family = crate::style::parse_font_family_list(&self.family)
            .map(|families| crate::style::serialize_font_family_list(&families))
            .unwrap_or_else(|| "sans-serif".to_owned());
        format!("{}{}{}px {}", if self.italic { "italic " } else { "" },
            if self.weight == 400 { String::new() } else { format!("{} ", self.weight) }, self.size, family)
    }

}

#[derive(Clone, Copy, Debug, Default)]
pub struct CanvasMetrics {
    pub width: f32,
    pub left: f32,
    pub right: f32,
    pub ascent: f32,
    pub descent: f32,
    pub font_ascent: f32,
    pub font_descent: f32,
}

#[derive(Clone, Copy)]
pub struct TextReference<'a> {
    pub align: &'a str,
    pub baseline: &'a str,
    pub rtl: bool,
}

pub struct TextPaint<'a> {
    pub reference: TextReference<'a>,
    pub x: f32,
    pub y: f32,
    pub color: [u8; 4],
    pub alpha: f32,
    pub stroke_width: Option<f32>,
    pub max_width: Option<f32>,
}

struct Glyph {
    layout: LayoutGlyph,
    variations: Option<Arc<FontVariations>>,
    raster_pixels: u64,
}

struct Run {
    width: f32,
    ink: Option<[f32; 4]>,
    geometry: crate::canvas_font_geometry::CanvasFontGeometry,
    glyphs: Vec<Glyph>,
    native_owners: Vec<std::sync::Weak<crate::native_font::NativeBytes>>,
}

impl Run {
    fn reference_offset(&self, reference: TextReference<'_>) -> Result<(f32, f32), &'static str> {
        let x = match reference.align {
            "center" => -self.width / 2.0,
            "right" => -self.width,
            "start" if reference.rtl => -self.width,
            "end" if !reference.rtl => -self.width,
            _ => 0.0,
        };
        let y = match reference.baseline {
            "alphabetic" => self.geometry.alphabetic.unwrap_or(0.0),
            "top" => self.geometry.em_ascent,
            "middle" => (self.geometry.em_ascent - self.geometry.em_descent) / 2.0,
            "bottom" => -self.geometry.em_descent,
            // These baselines need script-specific font baseline data.
            _ => return Err("Canvas hanging/ideographic text baselines are not implemented"),
        };
        Ok((x, y))
    }

    fn metrics(&self, reference: TextReference<'_>) -> Result<CanvasMetrics, &'static str> {
        let (x, y) = self.reference_offset(reference)?;
        let [left, top, right, bottom] = self.ink.unwrap_or([0.0; 4]);
        let result = CanvasMetrics { width: self.width,
            font_ascent: self.geometry.font_ascent - y, font_descent: self.geometry.font_descent + y,
            left: -(left + x), right: right + x, ascent: -(top + y), descent: bottom + y };
        Ok(result)
    }
}

#[derive(Clone, Eq, PartialEq, Hash)]
struct CanvasGlyphKey {
    glyph: CacheKey,
    variations: Option<Arc<FontVariations>>,
    scale_bits: u32,
    stroke_bits: Option<u32>,
}

/// Created only by the first Canvas measure/draw operation in a document.
/// One last run is sufficient to share ordinary measure-then-draw work and
/// keeps text retention bounded independently of the number of calls.
pub struct CanvasTextEngine {
    font_system: FontSystem,
    families: HashMap<String, LoadedFamily>,
    base_font_ids: Vec<cosmic_text::fontdb::ID>,
    #[cfg(test)] shaping_passes: usize,
    swash: SwashCache,
    variable: VariableSwashCache,
    geometry: crate::canvas_font_geometry::GeometryCache,
    emoji: bool,
    native: NativeFonts,
    last: Option<(CanvasFont, String, Arc<Run>)>,
    transformed: HashMap<CanvasGlyphKey, Option<Arc<Image>>>,
    scale_context: ScaleContext,
}

impl Default for CanvasTextEngine {
    fn default() -> Self { Self::new() }
}

impl CanvasTextEngine {
    pub fn new() -> Self {
        let (font_system, families) = create_font_system(&[], false);
        let base_font_ids = font_system.db().faces().map(|face| face.id).collect();
        Self { font_system, families, base_font_ids, #[cfg(test)] shaping_passes: 0, swash: SwashCache::new(), variable: VariableSwashCache::new(), geometry: Default::default(), emoji: false, native: NativeFonts::host(), last: None, transformed: HashMap::new(), scale_context: ScaleContext::new() }
    }

    #[cfg(test)]
    pub(crate) fn reduce_native_file_limit_for_test(&mut self, limit: usize) { self.native.reduce_canvas_file_limit(limit); }

    fn shape(&mut self, font: &CanvasFont, text: &str) -> Result<Arc<Run>, &'static str> {
        if text.len() > 65_536 || !font.size.is_finite() || font.size <= 0.0 || font.size > 4096.0 {
            return Err("Canvas text exceeds the native text work limit");
        }
        if let Some((prior_font, prior_text, run)) = &self.last {
            if prior_font == font && prior_text == text {
                self.native.touch_canvas_owners(&run.native_owners);
                return Ok(Arc::clone(run));
            }
        }
        self.trim_raster_cache();
        if !self.emoji && text_may_need_emoji_font(text) {
            append_emoji_font(&mut self.font_system, &mut self.families);
            self.emoji = true;
            self.base_font_ids = self.font_system.db().faces().filter(|face| !self.native.is_native_face(face.id)).map(|face| face.id).collect();
            self.swash = SwashCache::new();
            self.variable = VariableSwashCache::new();
            self.geometry.clear();
            self.transformed.clear();
        }
        let last = &mut self.last;
        let swash = &mut self.swash;
        let variable = &mut self.variable;
        let geometry = &mut self.geometry;
        let transformed = &mut self.transformed;
        let scale_context = &mut self.scale_context;
        let mut selected = resolve_native_font_with_reclaim(Some(&font.family), font.weight, font.italic,
            &mut self.font_system, &mut self.families, &mut self.native, &mut || {
                *last = None;
                *swash = SwashCache::new();
                *variable = VariableSwashCache::new();
                geometry.clear();
                transformed.clear();
                *scale_context = ScaleContext::new();
                true
            });
        // An all-unavailable Named stack deliberately returns the bundled
        // family with no ID. Realize that existing loaded family's CSS style
        // before shaping, so empty text and first-glyph fallback share the
        // same primary instance. This is local selection only: do not rerun
        // native resolution or clear its transient/capacity retry state.
        if selected.font_id.is_none() {
            selected = crate::font::resolve_loaded_font(Some(selected.family.as_ref()),
                font.weight, font.italic, &self.families);
        }
        let Some(primary_id) = selected.font_id else {
            self.native.finish_canvas_operation(); return Err("Canvas primary font unavailable");
        };
        let mut attrs = Attrs::new().family(Family::Name(selected.family.as_ref()))
            .weight(Weight(font.weight)).font_weight_axis(font.weight as f32)
            .font_optical_size(font.size).font_italic_axis(font.italic)
            .style(if font.italic { Style::Italic } else { Style::Normal });
        attrs = attrs.font_id(primary_id);
        if selected.synthetic_italic { attrs = attrs.cache_key_flags(CacheKeyFlags::FAKE_ITALIC); }
        let normalized: String = text.chars().map(|c| if matches!(c, '\t' | '\n' | '\r' | '\x0c') { ' ' } else { c }).collect();
        // All ID policy changes occur outside a live Buffer/FontFallbackIter. The
        // immutable base IDs retain existing platform fallback preference. The
        // native ordered tail comes only from this operation's real resolver.
        self.native.pin_canvas_face(primary_id);
        let mut ordered = Vec::new();
        self.font_system.set_operation_fallback(&self.base_font_ids, &ordered);
        let mut buffer = shape_canvas_buffer(&mut self.font_system, font.size, &normalized, &attrs);
        #[cfg(test)] { self.shaping_passes += 1; }
        let (mut missing_count, mut missing_text) = canvas_missing(&buffer);
        if missing_count != 0 {
            let locale = self.font_system.locale().to_owned();
            let postscript = selected.font_id.filter(|id| self.native.is_native_face(*id))
                .and_then(|id| self.font_system.db().face(id)).map(|face| face.post_script_name.as_str());
            let cascade = self.native.canvas_cascade(&crate::native_font::FallbackRequest {
                family: selected.family.as_ref(), postscript, weight:font.weight, italic:font.italic, size:font.size, locale:&locale,
            });
            let mut prepared_buffer = Some(buffer);
            for candidate in cascade.candidates {
                // No Buffer survives a possible provider/local capacity reclaim. A
                // rejected candidate does not require rebuilding that same Buffer.
                drop(prepared_buffer.take());
                let Self { font_system, families, native, last, swash, variable, geometry, transformed, scale_context, .. } = self;
                let id = native.canvas_candidate(candidate, &missing_text, font_system, families, &mut || {
                    *last = None; *swash = SwashCache::new(); *variable = VariableSwashCache::new(); geometry.clear();
                    transformed.clear(); *scale_context = ScaleContext::new(); true
                });
                let trial = id.filter(|id| Some(*id) != selected.font_id && !ordered.contains(id));
                let Some(id) = trial else { continue; };
                ordered.push(id);
                font_system.set_operation_fallback(&self.base_font_ids, &ordered);
                let trial_buffer = shape_canvas_buffer(font_system, font.size, &normalized, &attrs);
                #[cfg(test)] { self.shaping_passes += 1; }
                let (count, missing) = canvas_missing(&trial_buffer);
                if count < missing_count {
                    // Keep every contributing physical owner pinned until metrics and
                    // raster IDs have been finalized. No current-plan file is an LRU victim.
                    native.pin_canvas_face(id);
                    missing_count = count; missing_text = missing;
                    prepared_buffer = Some(trial_buffer);
                    if count == 0 { break; }
                } else {
                    ordered.pop();
                    drop(trial_buffer);
                    font_system.set_operation_fallback(&self.base_font_ids, &ordered);
                }
            }
            // Rebuild the final accepted plan once if the tail ended with rejected
            // candidates. This is independent of the number of cmap rejections.
            buffer = match prepared_buffer {
                Some(buffer) => buffer,
                None => {
                    self.font_system.set_operation_fallback(&self.base_font_ids, &ordered);
                    #[cfg(test)] { self.shaping_passes += 1; }
                    shape_canvas_buffer(&mut self.font_system, font.size, &normalized, &attrs)
                }
            };
            if missing_count != 0 && cascade.incomplete { self.native.incomplete_canvas_fallback(); }
        }
        // Primary instance is resolved independently of emitted glyphs: empty,
        // whitespace and first-glyph fallback all use this same physical face.
        let primary_axes = self.variable.effective_variations(&mut self.font_system, primary_id,
            Some(font.weight as f32), Some(font.size), font.italic, None);
        let geometry = match self.geometry.get(&self.font_system, primary_id, font.size, primary_axes) {
            Ok(geometry) => geometry,
            Err(error) => { drop(buffer); self.native.finish_canvas_operation(); return Err(error); }
        };
        let mut run = Run { width: 0.0, ink: None, geometry,
            glyphs: Vec::new(), native_owners: Vec::new() };
        for line in buffer.layout_runs() {
            run.width = run.width.max(line.line_w);
            for glyph in line.glyphs {
                let variations = glyph.font_is_variable.then(|| self.variable.effective_variations(
                    &mut self.font_system, glyph.font_id, glyph.font_weight_axis_opt,
                    glyph.font_optical_size_opt, glyph.font_italic_axis, None)).flatten();
                let bitmap_choice = self.font_system.get_font(glyph.font_id).and_then(|font| {
                    let font = font.as_swash();
                    let ppem = font.color_strikes().find_by_nearest_ppem(glyph.font_size as u16, glyph.glyph_id)?.ppem();
                    let apple = font.localized_strings().find_by_id(swash::StringId::Family, None)
                        .is_some_and(|name| name.chars().eq("Apple Color Emoji".chars()));
                    Some((ppem, apple))
                });
                let (bounds, raster_pixels) = self.font_system.db().with_face_data(glyph.font_id, |data, index| {
                    let mut face = cosmic_text::ttf_parser::Face::parse(data, index).ok()?;
                    if let Some(variations) = &variations {
                        for variation in variations.iter() {
                            face.set_variation(cosmic_text::ttf_parser::Tag::from_bytes(variation.tag.as_bytes()), variation.value.0);
                        }
                    }
                    let glyph_id = cosmic_text::ttf_parser::GlyphId(glyph.glyph_id);
                    let origin_x = glyph.x + glyph.font_size * glyph.x_offset;
                    let origin_y = glyph.y - glyph.font_size * glyph.y_offset;
                    if let Some(bounds) = face.glyph_bounding_box(glyph_id) {
                        let scale = glyph.font_size / face.units_per_em() as f32;
                        let mut left = bounds.x_min as f32 * scale;
                        let mut right = bounds.x_max as f32 * scale;
                        if glyph.cache_key_flags.contains(CacheKeyFlags::FAKE_ITALIC) {
                            let skew = 14.0f32.to_radians().tan();
                            left += (bounds.y_min as f32 * scale * skew).min(bounds.y_max as f32 * scale * skew);
                            right += (bounds.y_min as f32 * scale * skew).max(bounds.y_max as f32 * scale * skew);
                        }
                        let top = -bounds.y_max as f32 * scale;
                        let bottom = -bounds.y_min as f32 * scale;
                        let pixels = ((right - left).ceil() as u64 + 2).saturating_mul((bottom - top).ceil() as u64 + 2);
                        Some((Some([origin_x + left, origin_y + top, origin_x + right, origin_y + bottom]), pixels))
                    } else if let Some((bitmap, is_sbix, apple)) = bitmap_choice.and_then(|(ppem, apple)| {
                        if let Some(sbix) = face.tables().sbix {
                            let strike = sbix.strikes.into_iter().find(|strike| strike.pixels_per_em == ppem)?;
                            Some((strike.get(glyph_id)?, true, apple))
                        } else {
                            Some((face.tables().cbdt?.get(glyph_id, ppem)?, false, false))
                        }
                    }) {
                        // Read actual strike metadata only. measureText must never
                        // decode/rasterize a bitmap, especially at large font sizes.
                        if bitmap.pixels_per_em == 0 { return None; }
                        let scale = glyph.font_size / bitmap.pixels_per_em as f32;
                        let (mut width, mut height) = (u32::from(bitmap.width), u32::from(bitmap.height));
                        if bitmap.format == cosmic_text::ttf_parser::RasterImageFormat::PNG && bitmap.data.len() >= 24 {
                            width = u32::from_be_bytes(bitmap.data[16..20].try_into().ok()?);
                            height = u32::from_be_bytes(bitmap.data[20..24].try_into().ok()?);
                        }
                        let mut bitmap_top = bitmap.y as f32 + height as f32;
                        if is_sbix && apple && bitmap.y == 0 {
                            bitmap_top += (-100.0 * bitmap.pixels_per_em as f32 / face.units_per_em() as f32).round();
                        }
                        // Match Swash's integer strike placement after scaling.
                        let left = (bitmap.x as f32 * scale).trunc();
                        let top = -(bitmap_top * scale).trunc();
                        let right = left + (width as f32 * scale).trunc();
                        let bottom = top + (height as f32 * scale).trunc();
                        let source_pixels = u64::from(width).saturating_mul(u64::from(height));
                        let scaled_pixels = ((right - left).ceil() as u64 + 2).saturating_mul((bottom - top).ceil() as u64 + 2);
                        Some((Some([origin_x + left, origin_y + top, origin_x + right, origin_y + bottom]), source_pixels.saturating_add(scaled_pixels)))
                    } else {
                        Some((None, 0))
                    }
                }).flatten().unwrap_or((None, 0));
                if let Some(bounds) = bounds {
                    match &mut run.ink {
                        Some(ink) => { ink[0] = ink[0].min(bounds[0]); ink[1] = ink[1].min(bounds[1]); ink[2] = ink[2].max(bounds[2]); ink[3] = ink[3].max(bounds[3]); }
                        None => run.ink = Some(bounds),
                    }
                }
                run.glyphs.push(Glyph { layout: glyph.clone(), variations, raster_pixels });
            }
        }
        run.native_owners = self.native.canvas_run_owners(selected.font_id.into_iter()
            .chain(run.glyphs.iter().map(|glyph| glyph.layout.font_id)));
        self.native.touch_canvas_owners(&run.native_owners);
        drop(buffer);
        self.native.finish_canvas_operation();
        let run = Arc::new(run);
        self.last = if self.native.retry_needed() || (cfg!(target_os = "macos")
            && run.geometry.source == crate::canvas_font_geometry::MetricSource::Software) { None } else { Some((font.clone(), text.to_string(), Arc::clone(&run))) };
        Ok(run)
    }

    pub fn measure(&mut self, font: &CanvasFont, text: &str, reference: TextReference<'_>) -> Result<CanvasMetrics, &'static str> {
        self.shape(font, text)?.metrics(reference)
    }

    fn trim_raster_cache(&mut self) {
        let cached_bytes: usize = self.swash.image_cache.values().chain(self.variable.images.values())
            .filter_map(|image| image.as_ref()).map(|image| image.data.len()).sum();
        let cached_bytes = cached_bytes + self.transformed.values().filter_map(|image| image.as_ref()).map(|image| image.data.len()).sum::<usize>();
        if cached_bytes > 16 * 1024 * 1024 || self.swash.image_cache.len() + self.variable.images.len() + self.transformed.len() > 2048
            || self.variable.instances.len() > 2048 {
            self.swash = SwashCache::new();
            self.variable = VariableSwashCache::new();
            self.geometry.clear();
            self.transformed.clear();
        }
    }

    fn transformed_glyph(&mut self, glyph: &Glyph, key: CacheKey, scale_x: f32, stroke_width: Option<f32>) -> Result<Option<Arc<Image>>, &'static str> {
        let cache = CanvasGlyphKey { glyph: key, variations: glyph.variations.clone(), scale_bits: scale_x.to_bits(), stroke_bits: stroke_width.map(f32::to_bits) };
        if let Some(image) = self.transformed.get(&cache) { return Ok(image.clone()); }
        let font = self.font_system.get_font(key.font_id).ok_or("Canvas font unavailable")?;
        let mut builder = self.scale_context.builder(font.as_swash()).size(f32::from_bits(key.font_size_bits)).hint(true);
        if let Some(variations) = &glyph.variations {
            builder = builder.variations(variations.iter().map(|variation| (swash::tag_from_bytes(variation.tag.as_bytes()), variation.value.0)));
        }
        let mut scaler = builder.build();
        let offset = Vector::new(key.x_bin.as_float(), key.y_bin.as_float());
        let italic = key.flags.contains(CacheKeyFlags::FAKE_ITALIC);
        let mut outline = scaler.scale_outline(key.glyph_id).or_else(|| scaler.scale_color_outline(key.glyph_id));
        let image = if let Some(outline) = outline.as_mut() {
            if italic { outline.transform(&Transform::skew(Angle::from_degrees(14.0), Angle::from_degrees(0.0))); }
            let transform = Transform::scale(scale_x, 1.0);
            if let Some(line_width) = stroke_width {
                // Stroke the actual paths, before maxWidth scales the result.
                let mut stroke = Stroke::new(line_width);
                stroke.miter_limit = 10.0;
                let mut mask = Mask::new(outline.path());
                mask.style(stroke).transform(Some(transform)).format(Format::Alpha).origin(Origin::BottomLeft).render_offset(offset);
                let mut pixels = 0u64;
                mask.inspect(|_, width, height| pixels = u64::from(width) * u64::from(height));
                if pixels > 8_388_608 { return Err("Canvas glyph exceeds the raster work limit"); }
                let (data, placement) = mask.render();
                Some(Image { source: Source::Outline, content: Content::Mask, placement, data })
            } else {
                // Color outlines retain their palette; ordinary outlines use
                // the same native renderer as the uncompressed fast path.
                let bounds = outline.bounds();
                if f64::from(bounds.width().abs() + 2.0) * f64::from(bounds.height().abs() + 2.0) > 8_388_608.0 {
                    return Err("Canvas glyph exceeds the raster work limit");
                }
                let transform = if italic {
                    Transform::skew(Angle::from_degrees(14.0), Angle::from_degrees(0.0)).then_scale(scale_x, 1.0)
                } else { transform };
                Render::new(&[Source::ColorOutline(0), Source::Outline]).format(Format::Alpha)
                    .offset(offset).transform(Some(transform)).render(&mut scaler, key.glyph_id)
            }
        } else if stroke_width.is_some() {
            // Bitmap-only faces contain no path to stroke. Leave that glyph
            // unpainted, while still drawing any outline glyphs in the run.
            None
        } else {
            let image = Render::new(&[Source::ColorBitmap(StrikeWith::BestFit)])
                .render(&mut scaler, key.glyph_id);
            image.map(|image| compress_bitmap(image, scale_x, offset.x))
        };
        let image = image.map(Arc::new);
        // A single large glyph may be drawn within the work budget without
        // becoming a retained cache allocation larger than the cache budget.
        if image.as_ref().is_none_or(|image| image.data.len() <= 16 * 1024 * 1024) {
            self.transformed.insert(cache, image.clone());
        }
        Ok(image)
    }

    pub fn draw(&mut self, font: &CanvasFont, text: &str, paint: TextPaint<'_>, pixels: &mut [u8], width: u32, height: u32) -> Result<(), &'static str> {
        if u64::from(width) * u64::from(height) * 4 != pixels.len() as u64 { return Err("Invalid Canvas text surface"); }
        let run = self.shape(font, text)?;
        let (reference_x, reference_y) = run.reference_offset(paint.reference)?;
        let scale_x = paint.max_width.filter(|width| width.is_finite() && *width > 0.0)
            .filter(|width| *width < run.width).map_or(1.0, |width| width / run.width);
        if scale_x == 0.0 || paint.max_width.is_some_and(|width| !width.is_finite() || width <= 0.0) { return Ok(()); }
        if paint.stroke_width.is_some_and(|width| !width.is_finite() || width <= 0.0) { return Ok(()); }
        let stroke_extent = f64::from(paint.stroke_width.unwrap_or(0.0)) * 10.0;
        let raster_work: f64 = run.glyphs.iter().map(|glyph| {
            let size = f64::from(glyph.layout.font_size).ceil() + stroke_extent + 2.0;
            (size * size).max(glyph.raster_pixels as f64)
        }).sum();
        if raster_work > 8_388_608.0 { return Err("Canvas text exceeds the raster work limit"); }
        self.trim_raster_cache();
        let color = Color::rgba(paint.color[0], paint.color[1], paint.color[2], paint.color[3]);
        for glyph in &run.glyphs {
            let mut layout = glyph.layout.clone();
            layout.x *= scale_x;
            layout.x_offset *= scale_x;
            let physical = layout.physical((paint.x + reference_x * scale_x, paint.y + reference_y), 1.0);
            let image = if paint.stroke_width.is_some() || scale_x != 1.0 {
                Some(self.transformed_glyph(glyph, physical.cache_key, scale_x, paint.stroke_width)?)
            } else { None };
            let mut blend = |x: i32, y: i32, coverage: Color| {
                let px = i64::from(physical.x) + i64::from(x);
                let py = i64::from(physical.y) + i64::from(y);
                if px < 0 || py < 0 || px >= i64::from(width) || py >= i64::from(height) { return; }
                let alpha = coverage.a() as f32 / 255.0 * paint.color[3] as f32 / 255.0 * paint.alpha;
                if alpha <= 0.0 { return; }
                let index = (py as usize * width as usize + px as usize) * 4;
                let dst_alpha = pixels[index + 3] as f32 / 255.0;
                let out_alpha = alpha + dst_alpha * (1.0 - alpha);
                for (channel, source) in [coverage.r(), coverage.g(), coverage.b()].iter().enumerate() {
                    pixels[index + channel] = ((*source as f32 * alpha + pixels[index + channel] as f32 * dst_alpha * (1.0 - alpha)) / out_alpha).round() as u8;
                }
                pixels[index + 3] = (out_alpha * 255.0).round() as u8;
            };
            if let Some(image) = image {
                if let Some(image) = image { image_pixels(&image, color, &mut blend); }
            } else if let Some(variations) = &glyph.variations {
                self.variable.with_pixels(&mut self.font_system, physical.cache_key, Arc::clone(variations), color, &mut blend);
            } else {
                self.swash.with_pixels(&mut self.font_system, physical.cache_key, color, &mut blend);
            }
        }
        Ok(())
    }

}

fn image_pixels(image: &Image, color: Color, f: &mut impl FnMut(i32, i32, Color)) {
    let width = image.placement.width as usize;
    if width == 0 { return; }
    let channels = if image.content == Content::Color { 4 } else { 1 };
    for (index, pixel) in image.data.chunks_exact(channels).enumerate() {
        let sample = if channels == 4 { Color::rgba(pixel[0], pixel[1], pixel[2], pixel[3]) }
            else { Color::rgba(color.r(), color.g(), color.b(), pixel[0]) };
        f(image.placement.left + (index % width) as i32, -image.placement.top + (index / width) as i32, sample);
    }
}

/// Area-filter bitmap glyphs in premultiplied color space. Outline glyphs are
/// transformed before rasterization; only actual bitmap strikes take this path.
fn compress_bitmap(image: Image, scale_x: f32, offset_x: f32) -> Image {
    let source_width = image.placement.width as usize;
    let height = image.placement.height as usize;
    if source_width == 0 || height == 0 { return image; }
    let origin = image.placement.left as f32 * scale_x + offset_x;
    let left = origin.floor();
    let width = ((origin + source_width as f32 * scale_x).ceil() - left).max(0.0) as usize;
    let channels = if image.content == Content::Color { 4 } else { 1 };
    let mut output = vec![0; width * height * channels];
    for y in 0..height {
        for x in 0..width {
            let start = ((left + x as f32 - origin) / scale_x).max(0.0);
            let end = ((left + x as f32 + 1.0 - origin) / scale_x).min(source_width as f32);
            let mut alpha = 0.0;
            let mut color = [0.0; 3];
            for sx in (start.floor() as usize)..(end.ceil().max(0.0) as usize).min(source_width) {
                let weight = ((sx as f32 + 1.0).min(end) - (sx as f32).max(start)).max(0.0) * scale_x;
                let source = &image.data[(y * source_width + sx) * channels..];
                let a = source[channels - 1] as f32 / 255.0 * weight;
                alpha += a;
                if channels == 4 { for c in 0..3 { color[c] += source[c] as f32 * a; } }
            }
            let target = &mut output[(y * width + x) * channels..];
            if channels == 4 && alpha > 0.0 { for c in 0..3 { target[c] = (color[c] / alpha).round() as u8; } }
            target[channels - 1] = (alpha * 255.0).round() as u8;
        }
    }
    Image { placement: swash::zeno::Placement { left: left as i32, width: width as u32, ..image.placement }, data: output, ..image }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn reference() -> TextReference<'static> { TextReference { align: "start", baseline: "alphabetic", rtl: false } }
    #[test]
    fn canvas_uses_real_proportional_and_monospace_glyph_advances() {
        let mut engine = CanvasTextEngine::new();
        let serif = CanvasFont::parse("32px serif").unwrap();
        let mono = CanvasFont::parse("32px monospace").unwrap();
        let width = |engine: &mut CanvasTextEngine, font, text| engine.measure(font, text, reference()).unwrap().width;
        assert!(width(&mut engine, &serif, "WWW") > width(&mut engine, &serif, "iii") * 2.0);
        assert!((width(&mut engine, &mono, "WWW") - width(&mut engine, &mono, "iii")).abs() < 0.001);
        let missing = CanvasFont::parse("32px NoSuchAuditFont, serif").unwrap();
        assert_eq!(width(&mut engine, &serif, "iii"), width(&mut engine, &missing, "iii"));
    }
    #[test]
    fn canvas_spaces_have_advance_without_ink_and_keep_trailing_space() {
        let mut engine = CanvasTextEngine::new();
        let font = CanvasFont::parse("italic 32px serif").unwrap();
        let spaces = engine.measure(&font, "   ", reference()).unwrap();
        assert!(spaces.width > 0.0);
        assert_eq!((spaces.left, spaces.right, spaces.ascent, spaces.descent), (0.0, 0.0, 0.0, 0.0));
        let plain = engine.measure(&font, "Ag", reference()).unwrap();
        let trailing = engine.measure(&font, "Ag ", reference()).unwrap();
        assert!(trailing.width > plain.width);
        assert_eq!(plain.right, trailing.right);
        assert!(plain.ascent > 0.0 && plain.descent > 0.0);
    }
    #[test]
    fn canvas_text_raster_preserves_straight_alpha_and_measured_run() {
        let mut engine = CanvasTextEngine::new();
        let font = CanvasFont::parse("32px serif").unwrap();
        let measured = engine.measure(&font, "WWW", reference()).unwrap();
        let mut pixels = vec![0; 160 * 64 * 4];
        engine.draw(&font, "WWW", TextPaint { reference: reference(), x: 8.0, y: 44.0,
            color: [240, 30, 90, 128], alpha: 1.0, stroke_width: None, max_width: None }, &mut pixels, 160, 64).unwrap();
        let ink: Vec<_> = pixels.chunks_exact(4).enumerate().filter(|(_, p)| p[3] > 0).collect();
        assert!(!ink.is_empty());
        assert!(ink.iter().all(|(_, p)| p[0..3] == [240, 30, 90]));
        assert!(ink.iter().any(|(_, p)| p[3] >= 120));
        let right = ink.iter().map(|(i, _)| i % 160).max().unwrap() as f32;
        assert!((right - (8.0 + measured.right)).abs() < 2.0);
        assert_eq!(engine.measure(&font, "WWW", reference()).unwrap().width, measured.width);
    }
    #[test]
    fn canvas_strokes_real_outlines_and_compresses_fill_and_stroke() {
        let mut engine = CanvasTextEngine::new();
        let font = CanvasFont::parse("64px serif").unwrap();
        let paint = |stroke_width, max_width| TextPaint { reference: reference(), x: 10.0, y: 70.0,
            color: [40, 160, 200, 255], alpha: 1.0, stroke_width, max_width };
        let mut fill = vec![0; 260 * 100 * 4];
        let mut stroke = fill.clone();
        engine.draw(&font, "O", paint(None, None), &mut fill, 260, 100).unwrap();
        engine.draw(&font, "O", paint(Some(1.0), None), &mut stroke, 260, 100).unwrap();
        assert!(fill.chunks_exact(4).zip(stroke.chunks_exact(4)).any(|(f, s)| f[3] > 200 && s[3] == 0));
        assert!(fill.chunks_exact(4).zip(stroke.chunks_exact(4)).any(|(f, s)| f[3] == 0 && s[3] > 0));
        let count = |pixels: &[u8]| pixels.chunks_exact(4).filter(|p| p[3] > 0).count();
        let thin_ink = count(&stroke);
        stroke.fill(0);
        engine.draw(&font, "O", paint(Some(3.0), None), &mut stroke, 260, 100).unwrap();
        assert!(count(&stroke) > thin_ink);
        let advance = engine.measure(&font, "WWW", reference()).unwrap().width;
        for stroke_width in [None, Some(1.0)] {
            fill.fill(0); stroke.fill(0);
            engine.draw(&font, "WWW", paint(stroke_width, None), &mut fill, 260, 100).unwrap();
            engine.draw(&font, "WWW", paint(stroke_width, Some(advance / 2.0)), &mut stroke, 260, 100).unwrap();
            let extent = |pixels: &[u8]| {
                let xs: Vec<_> = pixels.chunks_exact(4).enumerate().filter(|(_, p)| p[3] > 0).map(|(i, _)| i % 260).collect();
                xs.iter().max().unwrap() - xs.iter().min().unwrap() + 1
            };
            assert!((extent(&stroke) as f32 - extent(&fill) as f32 / 2.0).abs() <= 3.0);
            assert_eq!(engine.measure(&font, "WWW", reference()).unwrap().width, advance);
        }
    }

    #[test]
    fn canvas_bitmap_measurement_does_not_rasterize_and_large_draw_is_budgeted() {
        let mut engine = CanvasTextEngine::new();
        let font = CanvasFont::parse("4096px sans-serif").unwrap();
        let text = "🚀".repeat(64);
        let measured = engine.measure(&font, &text, reference()).unwrap();
        assert!(measured.width > 0.0 && measured.ascent > 0.0);
        assert!(engine.swash.image_cache.is_empty());
        assert!(engine.variable.images.is_empty());
        assert!(engine.transformed.is_empty());
        let mut pixels = vec![0; 64 * 64 * 4];
        let error = engine.draw(&font, &text, TextPaint { reference: reference(), x: 0.0, y: 30.0,
            color: [0, 0, 0, 255], alpha: 1.0, stroke_width: None, max_width: None }, &mut pixels, 64, 64).unwrap_err();
        assert!(error.contains("work limit"));
        assert!(pixels.iter().all(|value| *value == 0));
        assert!(engine.swash.image_cache.is_empty());
    }

    #[test]
    fn canvas_bitmap_only_stroke_keeps_outline_neighbors_and_compressed_fill() {
        let mut engine = CanvasTextEngine::new();
        let font = CanvasFont::parse("32px sans-serif").unwrap();
        let width = engine.measure(&font, "🚀A", reference()).unwrap().width;
        let mut pixels = vec![0; 100 * 64 * 4];
        engine.draw(&font, "🚀A", TextPaint { reference: reference(), x: 2.0, y: 45.0,
            color: [20, 80, 180, 255], alpha: 1.0, stroke_width: Some(1.0), max_width: Some(width / 2.0) }, &mut pixels, 100, 64).unwrap();
        assert!(pixels.chunks_exact(4).any(|p| p[3] > 0));
        pixels.fill(0);
        engine.draw(&font, "🚀A", TextPaint { reference: reference(), x: 2.0, y: 45.0,
            color: [20, 80, 180, 255], alpha: 1.0, stroke_width: None, max_width: Some(width / 2.0) }, &mut pixels, 100, 64).unwrap();
        assert!(pixels.chunks_exact(4).any(|p| p[3] > 0 && p[0..3] != [20, 80, 180]));
    }

    #[test]
    fn canvas_font_parser_accepts_style_size_and_rejects_incomplete_values() {
        let font = CanvasFont::parse("italic bold 24pt \"Helvetica Neue\", serif").unwrap();
        assert_eq!(font.size, 32.0);
        assert_eq!(font.weight, 700);
        assert!(font.italic);
        assert!(CanvasFont::parse("32px").is_none());
        assert!(CanvasFont::parse("-1px serif").is_none());
        assert!(CanvasFont::parse("garbage 32px serif").is_none());
    }
}

#[cfg(test)]
mod font_serialization_regressions {
    use super::*;

    #[test]
    fn canvas_family_roundtrip_preserves_empty_names_commas_and_keyword_kind() {
        for family in ["\"\", serif", "\"serif\", monospace", "\"Absent, serif\", monospace",
            "\"A  B\", serif", "\"A\\\"B\", serif", "cursive, serif", "math, monospace"] {
            let source = format!("32px {family}");
            let parsed = CanvasFont::parse(&source).expect(&source);
            assert_eq!(CanvasFont::parse(&parsed.css()), Some(parsed));
        }
        let mut engine = CanvasTextEngine::new();
        let reference = TextReference { align: "start", baseline: "alphabetic", rtl: false };
        let empty = CanvasFont::parse("32px \"\", serif").unwrap();
        let serif = CanvasFont::parse("32px serif").unwrap();
        assert_eq!(engine.measure(&empty, "iiiWWW", reference).unwrap().width,
            engine.measure(&serif, "iiiWWW", reference).unwrap().width);
    }
}

#[cfg(test)]
mod canvas_font_trailing_syntax_regressions {
    use super::*;

    #[test]
    fn canvas_font_rejects_invalid_trailing_tokens_without_prefix_acceptance() {
        for value in ["32px serif)", "32px serif]", "32px serif; monospace", "32px serif {}"] {
            assert!(CanvasFont::parse(value).is_none(), "{value}");
        }
    }
}

#[cfg(test)]
#[path = "native_canvas_integration_tests.rs"]
mod native_canvas_integration_tests;

#[cfg(all(test, target_os = "macos"))]
#[path = "native_canvas_host_tests.rs"]
mod native_canvas_host_tests;

fn shape_canvas_buffer(fs: &mut FontSystem, size: f32, text: &str, attrs: &Attrs<'_>) -> Buffer {
    let mut buffer = Buffer::new(fs, Metrics::new(size, size));
    buffer.set_wrap(fs, Wrap::None);
    buffer.set_size(fs, None, None);
    buffer.set_text(fs, text, attrs, Shaping::Advanced);
    buffer.shape_until_scroll(fs, false);
    buffer
}

fn canvas_missing(buffer: &Buffer) -> (usize, String) {
    let mut count = 0;
    let mut ranges = Vec::new();
    for line in buffer.layout_runs() {
        for glyph in line.glyphs.iter().filter(|glyph| glyph.glyph_id == 0) {
            count += 1;
            ranges.push((line.line_i, line.text, glyph.start, glyph.end));
        }
    }
    // Several glyphs can share a whole combining cluster. Merge byte ranges
    // before copying text, so the prefilter retains each input byte at most once.
    ranges.sort_unstable_by_key(|&(line, _, start, end)| (line, start, end));
    let mut text = String::new();
    let mut index = 0;
    while index < ranges.len() {
        let (line, source, start, mut end) = ranges[index];
        index += 1;
        while index < ranges.len() && ranges[index].0 == line && ranges[index].2 <= end {
            end = end.max(ranges[index].3); index += 1;
        }
        if let Some(cluster) = source.get(start..end) { text.push_str(cluster); }
    }
    (count, text)
}

#[cfg(test)]
#[path = "canvas_geometry_tests.rs"]
mod canvas_geometry_tests;
