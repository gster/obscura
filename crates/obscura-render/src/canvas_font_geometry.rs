//! Canvas geometry is a property of the selected primary physical instance.
//! It is independent of CSS line metrics, glyph fallback, text and alignment.
use std::{collections::VecDeque, sync::Arc};
use cosmic_text::{fontdb, FontSystem, FontVariations};
use cosmic_text::ttf_parser::{Face, Tag};

pub(crate) type FontBytes = Arc<dyn AsRef<[u8]> + Send + Sync>;

/// One default member parsed from this borrowed owner. No byte or platform
/// object is retained, and callers cannot pair a Face with unrelated bytes.
pub(crate) struct ParsedCanvasFont<'a> {
    owner: &'a FontBytes,
    index: u32,
    face: Face<'a>,
}
impl<'a> ParsedCanvasFont<'a> {
    pub(crate) fn parse(owner: &'a FontBytes, index: u32)
        -> Result<Self, cosmic_text::ttf_parser::FaceParsingError> {
        let face = Face::parse(owner.as_ref().as_ref(), index)?;
        Ok(Self { owner, index, face })
    }
    pub(crate) fn owner(&self) -> &FontBytes { self.owner }
    pub(crate) fn index(&self) -> u32 { self.index }
    pub(crate) fn face(&self) -> &Face<'a> { &self.face }

    // Consume the default input only when native metrics are unavailable.
    // Native validation therefore never sees software-normalized coordinates.
    fn software_metrics(mut self, size: f32, axes: Option<&FontVariations>) -> InstanceMetrics {
        if let Some(axes) = axes {
            for axis in axes.iter() {
                self.face.set_variation(Tag::from_bytes(axis.tag.as_bytes()), axis.value.0);
            }
        }
        let scale = size / f32::from(self.face.units_per_em());
        InstanceMetrics { ascent: f32::from(self.face.ascender()) * scale,
            descent: -f32::from(self.face.descender()) * scale,
            typo: raw_typo(self.face.raw_face().table(Tag::from_bytes(b"OS/2"))),
            mac_compatibility_family: false,
            #[cfg(test)] software_coordinates: Some(self.face.variation_coordinates().iter()
                .map(|coordinate| coordinate.get()).collect()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MetricSource {
    CoreText,
    /// A real varied ttf-parser instance, not qualified as a Chrome platform backend.
    Software,
    /// Chrome uses a different backend for this format (CFF2, avar2, COLR, CBDT).
    SoftwareOtherChromeBackend,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct CanvasFontGeometry {
    pub font_ascent: f32,
    pub font_descent: f32,
    pub em_ascent: f32,
    pub em_descent: f32,
    pub alphabetic: Option<f32>,
    pub has_unresolved_baselines: bool,
    pub source: MetricSource,
    pub uses_subpixel_tiny_metrics: bool,
}

/// Only scalar values survive the platform query. Neither this record nor the
/// cache owns a CTFont, a descriptor, or a second copy of admitted font bytes.
pub(crate) struct InstanceMetrics {
    pub ascent: f32,
    pub descent: f32,
    pub typo: Option<(f32, f32)>,
    pub mac_compatibility_family: bool,
    #[cfg(test)] pub software_coordinates: Option<Vec<i16>>,
}

#[derive(Clone, PartialEq, Eq)]
struct Key {
    id: fontdb::ID,
    size: u32,
    axes: Option<Arc<FontVariations>>,
}

#[derive(Default)]
pub(crate) struct GeometryCache {
    entries: VecDeque<(Key, CanvasFontGeometry)>,
    #[cfg(test)] pub queries: usize,
    #[cfg(test)] pub generation: usize,
    #[cfg(test)] pub software_coordinates: Option<Vec<i16>>,
}
impl GeometryCache {
    #[cfg(test)] pub fn len_for_test(&self) -> usize { self.entries.len() }
    pub fn clear(&mut self) {
        self.entries.clear();
        #[cfg(test)] { self.generation += 1; }
    }
    pub fn get(&mut self, fs: &FontSystem, id: fontdb::ID, size: f32,
        axes: Option<Arc<FontVariations>>) -> Result<CanvasFontGeometry, &'static str> {
        let key = Key { id, size: size.to_bits(), axes };
        if let Some(index) = self.entries.iter().position(|(candidate, _)| candidate == &key) {
            let entry = self.entries.remove(index).unwrap();
            let value = entry.1;
            self.entries.push_back(entry);
            return Ok(value);
        }
        let info = fs.db().face(id).ok_or("Canvas primary font unavailable")?;
        // Never reopen a path or copy an unbudgeted font into a hidden cache.
        let fontdb::Source::Binary(owner) = &info.source else {
            return Err("Canvas primary font has no retained byte owner");
        };
        let parsed = ParsedCanvasFont::parse(owner, info.index)
            .map_err(|_| "Invalid Canvas primary font")?;
        let other_backend = requires_other_chrome_backend(parsed.face());
        let has_unresolved_baselines = parsed.face().raw_face().table(Tag::from_bytes(b"BASE")).is_some();
        #[cfg(test)] { self.queries += 1; }
        let native = if other_backend { None } else {
            crate::native_font::canvas_instance_metrics(&parsed, size, key.axes.as_deref()).ok()
        };
        let source = if native.is_some() { MetricSource::CoreText }
            else if other_backend { MetricSource::SoftwareOtherChromeBackend } else { MetricSource::Software };
        let mut metrics = native.unwrap_or_else(|| parsed.software_metrics(size, key.axes.as_deref()));
        #[cfg(test)] { self.software_coordinates = metrics.software_coordinates.take(); }
        let tiny = metrics.ascent < 3.0 || metrics.ascent + metrics.descent < 2.0;
        // Chromium153 CanvasRenderingContext2DState::SetFont and SetFontInternal
        // both set SubpixelAscentDescent(true), overriding FontDescription's
        // false default. FontMetrics preserves fractions at these thresholds.
        if source == MetricSource::CoreText {
            (metrics.ascent, metrics.descent) = canvas_font_box(metrics.ascent, metrics.descent);
        }
        if source == MetricSource::CoreText && metrics.mac_compatibility_family {
            metrics.ascent += ((metrics.ascent + metrics.descent) * 0.15 + 0.5).floor();
        }
        let (em_ascent, em_descent) = metrics.typo.filter(|(a, _)| *a > 0.0)
            .and_then(|(a, d)| normalized_em(a, d, size))
            .or_else(|| normalized_em(metrics.ascent, metrics.descent, size))
            .ok_or("Canvas primary font has invalid vertical metrics")?;
        let value = CanvasFontGeometry { font_ascent: metrics.ascent, font_descent: metrics.descent,
            em_ascent, em_descent, alphabetic: None,
            has_unresolved_baselines,
            source, uses_subpixel_tiny_metrics: source == MetricSource::CoreText && tiny };
        // Platform failures remain retryable. A numeric success never pins an ID
        // across eviction: every owner retirement clears this cache before reuse.
        if source == MetricSource::CoreText || other_backend || !cfg!(target_os = "macos") {
            if self.entries.len() == 128 { self.entries.pop_front(); }
            self.entries.push_back((key, value));
        }
        Ok(value)
    }
}

pub(crate) fn raw_typo(table: Option<&[u8]>) -> Option<(f32, f32)> {
    let table = table?;
    let a = i16::from_be_bytes(table.get(68..70)?.try_into().ok()?);
    let d = i16::from_be_bytes(table.get(70..72)?.try_into().ok()?);
    Some((f32::from(a), -f32::from(d)))
}
fn requires_other_chrome_backend(face: &Face<'_>) -> bool {
    let table = |tag: &[u8; 4]| face.raw_face().table(Tag::from_bytes(tag));
    table(b"CFF2").is_some() || table(b"CBDT").is_some() || table(b"COLR").is_some()
        || table(b"avar").is_some_and(|data| data.get(..2) == Some(&[0, 2]))
}
fn canvas_font_box(ascent: f32, descent: f32) -> (f32, f32) {
    if ascent < 3.0 || ascent + descent < 2.0 { (ascent, descent) }
    else { (blink_round(ascent), blink_round(descent)) }
}
fn blink_round(value: f32) -> f32 { (f64::from(value) + 0.5).floor() as f32 }
fn round64(value: f32) -> f32 {
    // LayoutUnit::FromFloatRound: float32 multiplication and round-away, with
    // saturating signed fixed-point storage. Canvas size is separately bounded.
    ((value * 64.0).round() as i32) as f32 / 64.0
}
fn normalized_em(ascent: f32, descent: f32, size: f32) -> Option<(f32, f32)> {
    let height = ascent + descent;
    if !height.is_finite() || height <= 0.0 || !ascent.is_finite() || ascent < 0.0 || ascent > height { return None; }
    let a = round64(ascent * size / height);
    Some((a, round64(size) - a))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn canvas_true_subpixel_policy_preserves_tiny_fractions_only() {
        assert_eq!(canvas_font_box(2.75,0.625),(2.75,0.625));
        assert_eq!(canvas_font_box(2.999,0.625),(2.999,0.625));
        assert_eq!(canvas_font_box(3.0,0.625),(3.0,1.0));
        assert_eq!(canvas_font_box(9.5,2.5),(10.0,3.0));
    }
    #[test]
    fn normalized_em_validates_metadata_and_quantizes_size_first() {
        // Independent Liberation Sans OS/2 pair: 1491/-431, UPEM 2048.
        assert_eq!(normalized_em(1491.0, 431.0, 10.5), Some((8.140625, 2.359375)));
        assert_eq!(normalized_em(1.0, 1.0, 10.5078125), Some((5.25, 5.265625)));
        assert_eq!(round64(10.507811), 10.5);
        assert_eq!(round64(10.5078125), 10.515625);
        assert_eq!(round64(10.507814), 10.515625);
        for (a,d) in [(0.0,0.0),(-1.0,2.0),(3.0,-1.0),(f32::NAN,1.0),(1.0,f32::INFINITY)] {
            assert!(normalized_em(a,d,10.0).is_none());
        }
        assert_eq!(raw_typo(None), None);
        assert_eq!(raw_typo(Some(&[0;71])), None);
    }
}
