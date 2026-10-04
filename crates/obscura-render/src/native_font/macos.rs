//! Minimal CoreText seam. No CF object escapes this module or crosses a thread.
use super::*;
use std::{ffi::{c_char, c_int, c_void, OsString}, fs::{File, OpenOptions}, io::Read,
    os::{fd::AsRawFd, unix::{ffi::OsStringExt, fs::{MetadataExt, OpenOptionsExt}}}, path::Path, sync::OnceLock};
type CF = *const c_void;
#[repr(C)]
struct Callbacks { _opaque: [u8; 0] }
#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRelease(value: CF);
    fn CFGetTypeID(value: CF) -> usize;
    fn CFStringGetTypeID() -> usize;
    fn CFURLGetTypeID() -> usize;
    fn CFArrayGetTypeID() -> usize;
    fn CFArrayCreate(allocator: CF, values: *const CF, count: isize, callbacks: *const Callbacks) -> CF;
    fn CFArrayGetCount(array: CF) -> isize;
    fn CFArrayGetValueAtIndex(array: CF, index: isize) -> CF;
    static kCFTypeArrayCallBacks: Callbacks;
    fn CFStringCreateWithBytes(allocator: CF, bytes: *const u8, len: isize, encoding: u32, external: u8) -> CF;
    fn CFStringGetCString(value: CF, buffer: *mut c_char, size: isize, encoding: u32) -> u8;
    fn CFDictionaryCreate(allocator: CF, keys: *const CF, values: *const CF, count: isize, keys_callbacks: *const Callbacks, values_callbacks: *const Callbacks) -> CF;
    fn CFNumberCreate(allocator: CF, number_type: isize, value: *const c_void) -> CF;
    fn CFURLCopyScheme(url: CF) -> CF;
    fn CFURLGetFileSystemRepresentation(url: CF, resolve: u8, buffer: *mut u8, size: isize) -> u8;
    static kCFTypeDictionaryKeyCallBacks: Callbacks;
    static kCFTypeDictionaryValueCallBacks: Callbacks;
}
#[link(name = "CoreText", kind = "framework")]
extern "C" {
    static kCTFontFamilyNameAttribute: CF;
    static kCTFontNameAttribute: CF;
    static kCTFontURLAttribute: CF;
    static kCTFontTraitsAttribute: CF;
    static kCTFontSymbolicTrait: CF;
    fn CTFontDescriptorCreateWithAttributes(attributes: CF) -> CF;
    fn CTFontCreateWithFontDescriptorAndOptions(descriptor: CF, size: f64, matrix: CF, options: usize) -> CF;
    fn CTFontCopyFamilyName(font: CF) -> CF;
    fn CTFontDescriptorGetTypeID() -> usize;
    fn CTFontCopyDefaultCascadeListForLanguages(font: CF, languages: CF) -> CF;
    fn CTFontCopyPostScriptName(font: CF) -> CF;
    fn CTFontCopyAttribute(font: CF, attribute: CF) -> CF;
}
extern "C" {
    fn sysctlbyname(name: *const c_char, old: *mut c_void, len: *mut usize, new: *mut c_void, new_len: usize) -> c_int;
    fn fcntl(fd: c_int, command: c_int, ...) -> c_int;
}
struct Owned(CF);
impl Owned {
    fn new(raw: CF) -> Result<Self, Unavailable> { if raw.is_null() { Err(Unavailable::Io) } else { Ok(Self(raw)) } }
    fn string(&self) -> Result<String, Unavailable> {
        let mut bytes = [0u8; 1025];
        unsafe {
            if CFGetTypeID(self.0) != CFStringGetTypeID() || CFStringGetCString(self.0, bytes.as_mut_ptr().cast(), bytes.len() as isize, 0x08000100) == 0 { return Err(Unavailable::Io); }
        }
        let end = bytes.iter().position(|b| *b == 0).ok_or(Unavailable::Io)?;
        String::from_utf8(bytes[..end].to_vec()).map_err(|_| Unavailable::Io)
    }
}
impl Drop for Owned { fn drop(&mut self) { unsafe { CFRelease(self.0); } } }
fn supported() -> Result<bool, Unavailable> {
    static SUPPORTED: OnceLock<bool> = OnceLock::new();
    cached_platform_support(&SUPPORTED, || {
        let mut bytes = [0u8; 64];
        let mut len = bytes.len();
        let result = unsafe { sysctlbyname(b"kern.osproductversion\0".as_ptr().cast(), bytes.as_mut_ptr().cast(), &mut len, std::ptr::null_mut(), 0) };
        if result != 0 || len == 0 || len > bytes.len() { return Err(Unavailable::Io); }
        Ok(bytes[..len].to_vec())
    })
}
// Directory boundaries express public-resource access policy, never font-name mappings.
fn public(path: &Path) -> bool {
    path.starts_with("/System/Library/Fonts") || path.starts_with("/Library/Fonts")
}
fn open_public(path: &Path) -> Result<(File, ResourceKey), Unavailable> {
    if !public(path) || path.components().any(|c| c == std::path::Component::ParentDir) { return Err(Unavailable::Io); }
    let canonical = path.canonicalize().map_err(|_| Unavailable::Io)?;
    if !public(&canonical) { return Err(Unavailable::Io); }
    // macOS O_NOFOLLOW_ANY (sys/fcntl.h): no component may become a symlink on open.
    let file = OpenOptions::new().read(true).custom_flags(0x20000000).open(&canonical).map_err(|_| Unavailable::Io)?;
    let mut buffer = [0u8; 1024];
    if unsafe { fcntl(file.as_raw_fd(), 50, buffer.as_mut_ptr()) } != 0 { return Err(Unavailable::Io); } // F_GETPATH
    let end = buffer.iter().position(|b| *b == 0).ok_or(Unavailable::Io)?;
    let opened = PathBuf::from(OsString::from_vec(buffer[..end].to_vec()));
    if !public(&opened) { return Err(Unavailable::Io); }
    let meta = file.metadata().map_err(|_| Unavailable::Io)?;
    if !meta.is_file() { return Err(Unavailable::Io); }
    let bytes = usize::try_from(meta.len()).map_err(|_| Unavailable::Capacity)?;
    Ok((file, ResourceKey { path: opened, device: meta.dev(), inode: meta.ino(), bytes,
        modified: i128::from(meta.mtime()) * 1_000_000_000 + i128::from(meta.mtime_nsec()) }))
}
pub(super) struct Mac;
impl Backend for Mac {
    fn locate(&self, family: &str) -> Result<Option<Located>, Unavailable> { self.locate_styled(family, 400, false) }
    fn locate_styled(&self, family: &str, weight: u16, italic: bool) -> Result<Option<Located>, Unavailable> {
        if !supported()? { return Err(Unavailable::Unsupported); }
        unsafe {
            let name = Owned::new(CFStringCreateWithBytes(std::ptr::null(), family.as_ptr(), family.len() as isize, 0x08000100, 0))?;
            // CTFontTraits.h: italic bit 0, bold bit 1. Ask for actual style files,
            // never a transformed/synthetic font. This seam requests the two CSS
            // normal/bold classes; intermediate weights still use real loaded metadata.
            let symbolic: i32 = i32::from(italic) | if weight >= 600 { 2 } else { 0 };
            // CFNumber.h kCFNumberSInt32Type = 3.
            let number = Owned::new(CFNumberCreate(std::ptr::null(), 3, (&symbolic as *const i32).cast()))?;
            let traits = Owned::new(CFDictionaryCreate(std::ptr::null(), &kCTFontSymbolicTrait, &number.0, 1, &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks))?;
            let keys = [kCTFontFamilyNameAttribute, kCTFontTraitsAttribute];
            let values = [name.0, traits.0];
            let attributes = Owned::new(CFDictionaryCreate(std::ptr::null(), keys.as_ptr(), values.as_ptr(), 2, &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks))?;
            let descriptor = Owned::new(CTFontDescriptorCreateWithAttributes(attributes.0))?;
            // CTFontOptions: bit0 PreventAutoActivation, bit1 PreventAutoDownload (macOS13+).
            let font = Owned::new(CTFontCreateWithFontDescriptorAndOptions(descriptor.0, 12.0, std::ptr::null(), 3))?;
            let actual = Owned::new(CTFontCopyFamilyName(font.0))?.string()?;
            if family_key(&actual) != family_key(family) { return Ok(None); }
            located_font(font.0).map(Some)
        }
    }
    fn cascade(&self, request: &FallbackRequest<'_>, limit: usize) -> Result<Cascade, Unavailable> {
        if !supported()? { return Err(Unavailable::Unsupported); }
        if limit == 0 { return Ok(Cascade { candidates: Vec::new(), incomplete: true }); }
        let anchor = request.postscript.unwrap_or(request.family);
        let (weight, italic, locale) = (request.weight, request.italic, request.locale);
        unsafe {
            let name = Owned::new(CFStringCreateWithBytes(std::ptr::null(), anchor.as_ptr(), anchor.len() as isize, 0x08000100, 0))?;
            let symbolic: i32 = i32::from(italic) | if weight >= 600 { 2 } else { 0 };
            let number = Owned::new(CFNumberCreate(std::ptr::null(), 3, (&symbolic as *const i32).cast()))?;
            let traits = Owned::new(CFDictionaryCreate(std::ptr::null(), &kCTFontSymbolicTrait, &number.0, 1, &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks))?;
            let keys = [if request.postscript.is_some() { kCTFontNameAttribute } else { kCTFontFamilyNameAttribute }, kCTFontTraitsAttribute];
            let values = [name.0, traits.0];
            let attributes = Owned::new(CFDictionaryCreate(std::ptr::null(), keys.as_ptr(), values.as_ptr(), 2, &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks))?;
            let descriptor = Owned::new(CTFontDescriptorCreateWithAttributes(attributes.0))?;
            let font = Owned::new(CTFontCreateWithFontDescriptorAndOptions(descriptor.0, f64::from(request.size), std::ptr::null(), 3))?;
            if let Some(expected) = request.postscript {
                if Owned::new(CTFontCopyPostScriptName(font.0))?.string()? != expected { return Err(Unavailable::InvalidFont); }
            }
            // Deliberate engine locale, never NULL/system UI language.
            let language = Owned::new(CFStringCreateWithBytes(std::ptr::null(), locale.as_ptr(), locale.len() as isize, 0x08000100, 0))?;
            let languages = Owned::new(CFArrayCreate(std::ptr::null(), &language.0, 1, &kCFTypeArrayCallBacks))?;
            // Descriptor discovery does not request glyph substitution, activation or download.
            let descriptors = Owned::new(CTFontCopyDefaultCascadeListForLanguages(font.0, languages.0))?;
            if CFGetTypeID(descriptors.0) != CFArrayGetTypeID() { return Err(Unavailable::Io); }
            let count = usize::try_from(CFArrayGetCount(descriptors.0)).map_err(|_| Unavailable::Io)?;
            let mut candidates = Vec::new();
            // Anchor realization consumes one slot from the same remaining budget.
            let candidate_limit = limit.saturating_sub(1);
            let mut incomplete = count > candidate_limit;
            for index in 0..count.min(candidate_limit) {
                let descriptor = CFArrayGetValueAtIndex(descriptors.0, index as isize);
                if descriptor.is_null() || CFGetTypeID(descriptor) != CTFontDescriptorGetTypeID() { incomplete = true; continue; }
                // Every realization independently receives both restrictions. Do not replace
                // with CTFontCreateForString: it has no options argument or inheritance promise.
                let Ok(font) = Owned::new(CTFontCreateWithFontDescriptorAndOptions(descriptor, f64::from(request.size), std::ptr::null(), 3)) else { incomplete = true; continue; };
                let Ok(located) = located_font(font.0) else { incomplete = true; continue; };
                if !candidates.iter().any(|prior: &Located| prior.key == located.key && prior.postscript == located.postscript) {
                    candidates.push(located);
                }
            }
            Ok(Cascade { candidates, incomplete })
        }
    }
    fn read(&self, key: &ResourceKey) -> Result<Vec<u8>, Unavailable> {
        let (mut file, actual) = open_public(&key.path)?;
        if &actual != key { return Err(Unavailable::Io); }
        let mut bytes = vec![0; key.bytes];
        file.read_exact(&mut bytes).map_err(|_| Unavailable::Io)?;
        let mut extra = [0u8; 1];
        if file.read(&mut extra).map_err(|_| Unavailable::Io)? != 0 { return Err(Unavailable::Io); }
        let meta = file.metadata().map_err(|_| Unavailable::Io)?;
        if meta.len() != key.bytes as u64 || i128::from(meta.mtime()) * 1_000_000_000 + i128::from(meta.mtime_nsec()) != key.modified { return Err(Unavailable::Io); }
        Ok(bytes)
    }
}

// The exact same public-file identity policy applies to cascade resources.
unsafe fn located_font(font: CF) -> Result<Located, Unavailable> {
            let actual = Owned::new(CTFontCopyFamilyName(font))?.string()?;
            let postscript = Owned::new(CTFontCopyPostScriptName(font))?.string()?;
            let url = Owned::new(CTFontCopyAttribute(font, kCTFontURLAttribute))?;
            if CFGetTypeID(url.0) != CFURLGetTypeID() || Owned::new(CFURLCopyScheme(url.0))?.string()? != "file" { return Err(Unavailable::Io); }
            let mut bytes = [0u8; 4096];
            if CFURLGetFileSystemRepresentation(url.0, 1, bytes.as_mut_ptr(), bytes.len() as isize) == 0 { return Err(Unavailable::Io); }
            let end = bytes.iter().position(|b| *b == 0).ok_or(Unavailable::Io)?;
            let path = PathBuf::from(OsString::from_vec(bytes[..end].to_vec()));
            let (_, key) = open_public(&path)?;
            Ok(Located { key, family: actual, postscript })
}

// The CFData allocator context retains the *existing* admitted byte owner until
// the final CoreText consumer releases it. No copied/hidden font blob, no CF
// object in the numeric cache, and no unsafe Send/Sync implementation.
#[repr(C)]
struct AllocatorContext {
    version: isize, info: *mut c_void,
    retain: Option<unsafe extern "C" fn(CF) -> CF>,
    release: Option<unsafe extern "C" fn(CF)>,
    description: Option<unsafe extern "C" fn(CF) -> CF>,
    allocate: Option<unsafe extern "C" fn(isize, usize, *mut c_void) -> *mut c_void>,
    reallocate: Option<unsafe extern "C" fn(*mut c_void, isize, usize, *mut c_void) -> *mut c_void>,
    deallocate: Option<unsafe extern "C" fn(*mut c_void, *mut c_void)>,
    preferred_size: Option<unsafe extern "C" fn(isize, usize, *mut c_void) -> isize>,
}
#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFAllocatorCreate(allocator: CF, context: *mut AllocatorContext) -> CF;
    fn CFDataCreateWithBytesNoCopy(allocator: CF, bytes: *const u8, len: isize, deallocator: CF) -> CF;
    fn CFDataGetLength(data: CF) -> isize;
    fn CFDataGetBytePtr(data: CF) -> *const u8;
    fn CFDataGetTypeID() -> usize;
    fn CFDictionaryGetTypeID() -> usize;
    fn CFDictionaryGetValue(dictionary: CF, key: CF) -> CF;
    fn CFNumberGetTypeID() -> usize;
    fn CFNumberGetValue(number: CF, number_type: isize, value: *mut c_void) -> u8;
}
#[link(name = "CoreText", kind = "framework")]
extern "C" {
    fn CTFontManagerCreateFontDescriptorsFromData(data: CF) -> CF;
    fn CTFontDescriptorCopyAttribute(descriptor: CF, attribute: CF) -> CF;
    fn CTFontDescriptorCreateCopyWithVariation(descriptor: CF, identifier: CF, value: f64) -> CF;
    fn CTFontGetAscent(font: CF) -> f64;
    fn CTFontGetDescent(font: CF) -> f64;
    fn CTFontGetUnitsPerEm(font: CF) -> u32;
    fn CTFontGetGlyphCount(font: CF) -> isize;
    fn CTFontCopyTable(font: CF, tag: u32, options: usize) -> CF;
    fn CTFontCopyVariation(font: CF) -> CF;
    fn CTFontCopyGraphicsFont(font: CF, attributes: *mut CF) -> CF;
}
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" { fn CGFontCopyTableForTag(font: CF, tag: u32) -> CF; }
type DataOwner = crate::canvas_font_geometry::FontBytes;
unsafe extern "C" fn retain_data(info: CF) -> CF {
    Arc::increment_strong_count(info.cast::<DataOwner>());
    info
}
unsafe extern "C" fn release_data(info: CF) { drop(Arc::from_raw(info.cast::<DataOwner>())); }
unsafe extern "C" fn deallocate_data(_: *mut c_void, _: *mut c_void) {}
fn font_data(bytes: &DataOwner) -> Result<Owned, Unavailable> {
    let owner = Arc::new(bytes.clone());
    let mut context = AllocatorContext { version: 0, info: Arc::as_ptr(&owner).cast_mut().cast(),
        retain: Some(retain_data), release: Some(release_data), description: None, allocate: None,
        reallocate: None, deallocate: Some(deallocate_data), preferred_size: None };
    unsafe {
        let allocator = Owned::new(CFAllocatorCreate(std::ptr::null(), &mut context))?;
        let raw = bytes.as_ref().as_ref();
        let length = isize::try_from(raw.len()).map_err(|_| Unavailable::TooLarge)?;
        Owned::new(CFDataCreateWithBytesNoCopy(std::ptr::null(), raw.as_ptr(), length, allocator.0))
    }
}
unsafe fn table(font: CF, tag: &[u8; 4]) -> Result<Option<Owned>, Unavailable> {
    let mut raw = CTFontCopyTable(font, u32::from_be_bytes(*tag), 0);
    if raw.is_null() {
        // Match pinned Skia's web-font table fallback on this exact CT instance.
        let graphics = Owned::new(CTFontCopyGraphicsFont(font, std::ptr::null_mut()))?;
        raw = CGFontCopyTableForTag(graphics.0, u32::from_be_bytes(*tag));
    }
    if raw.is_null() { return Ok(None); }
    let data = Owned::new(raw)?;
    if CFGetTypeID(raw) != CFDataGetTypeID() || !(0..=1_048_576).contains(&CFDataGetLength(raw)) {
        return Err(Unavailable::TooLarge);
    }
    Ok(Some(data))
}
unsafe fn data_slice(data: &Owned) -> &[u8] {
    let length = CFDataGetLength(data.0) as usize;
    if length == 0 { &[] } else { std::slice::from_raw_parts(CFDataGetBytePtr(data.0), length) }
}
fn postscript(face: &cosmic_text::ttf_parser::Face<'_>) -> Option<String> {
    face.names().into_iter().filter(|name| name.name_id == 6).find_map(|name| name.to_string())
}
/// Exact member query. Descriptor order is not a collection-index contract.
pub(super) fn canvas_instance_metrics(parsed: &crate::canvas_font_geometry::ParsedCanvasFont<'_>, size: f32,
    axes: Option<&cosmic_text::FontVariations>)
    -> Result<crate::canvas_font_geometry::InstanceMetrics, Unavailable> {
    use cosmic_text::ttf_parser::{Face, Tag};
    if !supported()? { return Err(Unavailable::Unsupported); }
    if !size.is_finite() || size <= 0.0 || size > 4096.0 { return Err(Unavailable::InvalidRequest); }
    let bytes = parsed.owner();
    let raw = bytes.as_ref().as_ref();
    let face = parsed.face();
    let expected = postscript(face).ok_or(Unavailable::InvalidFont)?;
    let members = cosmic_text::ttf_parser::fonts_in_collection(raw).unwrap_or(1);
    if members > 256 { return Err(Unavailable::TooLarge); }
    // Ambiguous PS names cannot establish which physical member CT realized.
    // The selected member was already parsed and has this exact PS name.
    // Every other member still participates in ambiguity rejection.
    if (0..members).filter(|i| *i != parsed.index()).filter_map(|i| Face::parse(raw, i).ok())
        .any(|member| postscript(&member).as_deref() == Some(expected.as_str())) {
        return Err(Unavailable::InvalidFont);
    }
    for tag in [b"head", b"maxp", b"name", b"hhea", b"OS/2"] {
        if face.raw_face().table(Tag::from_bytes(tag)).is_some_and(|data| data.len() > 1_048_576) {
            return Err(Unavailable::TooLarge);
        }
    }
    let variation_axes = face.variation_axes();
    if variation_axes.len() > 64 { return Err(Unavailable::TooLarge); }
    unsafe {
        let data = font_data(bytes)?;
        let descriptors = Owned::new(CTFontManagerCreateFontDescriptorsFromData(data.0))?;
        if CFGetTypeID(descriptors.0) != CFArrayGetTypeID() { return Err(Unavailable::InvalidFont); }
        let count = CFArrayGetCount(descriptors.0);
        if count <= 0 || count > 256 { return Err(Unavailable::TooLarge); }
        let mut matched: CF = std::ptr::null();
        for i in 0..count {
            let descriptor = CFArrayGetValueAtIndex(descriptors.0, i);
            if descriptor.is_null() || CFGetTypeID(descriptor) != CTFontDescriptorGetTypeID() { return Err(Unavailable::InvalidFont); }
            let name = Owned::new(CTFontDescriptorCopyAttribute(descriptor, kCTFontNameAttribute))?.string()?;
            if name == expected {
                if !matched.is_null() { return Err(Unavailable::InvalidFont); }
                matched = descriptor;
            }
        }
        if matched.is_null() { return Err(Unavailable::InvalidFont); }
        // Validate raw member identity at default coordinates before variation
        // instantiation. Table comparisons are bounded and objects immediately drop.
        let base = Owned::new(CTFontCreateWithFontDescriptorAndOptions(matched, f64::from(size), std::ptr::null(), 3))?;
        if Owned::new(CTFontCopyPostScriptName(base.0))?.string()? != expected
            || CTFontGetUnitsPerEm(base.0) != u32::from(face.units_per_em())
            || CTFontGetGlyphCount(base.0) != isize::try_from(face.number_of_glyphs()).unwrap_or(-1) {
            return Err(Unavailable::InvalidFont);
        }
        for tag in [b"head", b"maxp", b"name", b"hhea"] {
            let wanted = face.raw_face().table(Tag::from_bytes(tag)).ok_or(Unavailable::InvalidFont)?;
            if wanted.len() > 1_048_576 { return Err(Unavailable::TooLarge); }
            let actual = table(base.0, tag)?.ok_or(Unavailable::InvalidFont)?;
            if data_slice(&actual) != wanted { return Err(Unavailable::InvalidFont); }
        }
        let mut descriptor = None;
        // Set every supported axis, including defaults. CoreText must not infer
        // opsz from size while the shaped/raster instance uses another coordinate.
        for axis in variation_axes {
            let value = axes.and_then(|axes| axes.iter().find(|v| v.tag.as_bytes() == &axis.tag.to_bytes()))
                .map_or(axis.def_value, |v| v.value.0).clamp(axis.min_value, axis.max_value);
            let id = i64::from(axis.tag.0);
            let number = Owned::new(CFNumberCreate(std::ptr::null(), 4, (&id as *const i64).cast()))?;
            let prior = descriptor.as_ref().map_or(matched, |value: &Owned| value.0);
            descriptor = Some(Owned::new(CTFontDescriptorCreateCopyWithVariation(prior, number.0, f64::from(value)))?);
        }
        let varied;
        let font = if let Some(descriptor) = &descriptor {
            varied = Owned::new(CTFontCreateWithFontDescriptorAndOptions(descriptor.0, f64::from(size), std::ptr::null(), 3))?;
            varied.0
        } else { base.0 };
        if CTFontGetUnitsPerEm(font) != u32::from(face.units_per_em()) || CTFontGetGlyphCount(font) != face.number_of_glyphs() as isize {
            return Err(Unavailable::InvalidFont);
        }
        // CT's varied PostScript name may add an instance suffix. Physical table
        // identity is checked on the data-backed descriptor; verify actual axes.
        let variation = CTFontCopyVariation(font);
        let variation = if variation.is_null() { None } else { Some(Owned::new(variation)?) };
        if variation.as_ref().is_some_and(|v| CFGetTypeID(v.0) != CFDictionaryGetTypeID()) { return Err(Unavailable::InvalidFont); }
        for axis in face.variation_axes() {
            let wanted = axes.and_then(|axes| axes.iter().find(|v| v.tag.as_bytes() == &axis.tag.to_bytes()))
                .map_or(axis.def_value, |v| v.value.0).clamp(axis.min_value, axis.max_value);
            let id = i64::from(axis.tag.0);
            let number = Owned::new(CFNumberCreate(std::ptr::null(), 4, (&id as *const i64).cast()))?;
            let actual = variation.as_ref().map_or(std::ptr::null(), |v| CFDictionaryGetValue(v.0, number.0));
            let mut value = f64::from(axis.def_value);
            if !actual.is_null() && (CFGetTypeID(actual) != CFNumberGetTypeID()
                || CFNumberGetValue(actual, 6, (&mut value as *mut f64).cast()) == 0) { return Err(Unavailable::InvalidFont); }
            if value as f32 != wanted { return Err(Unavailable::InvalidFont); }
        }
        let ascent = CTFontGetAscent(font) as f32;
        let descent = CTFontGetDescent(font) as f32;
        if !ascent.is_finite() || !descent.is_finite() || ascent < 0.0 || descent < 0.0 || ascent + descent <= 0.0 {
            return Err(Unavailable::InvalidFont);
        }
        // Match SkTypeface's table route on the *instance*. Do not apply MVAR a
        // second time, or substitute default-face OS/2 for a varied CT table.
        let os2 = table(font, b"OS/2")?;
        let typo = crate::canvas_font_geometry::raw_typo(os2.as_ref().map(|data| data_slice(data)));
        let family = Owned::new(CTFontCopyFamilyName(font))?.string()?;
        Ok(crate::canvas_font_geometry::InstanceMetrics { ascent, descent, typo,
            mac_compatibility_family: matches!(family.as_str(), "Times" | "Helvetica" | "Courier"),
            #[cfg(test)] software_coordinates: None })
    }
}

#[cfg(test)]
mod metric_owner_tests {
    use super::*;
    #[test]
    fn canvas_metric_cfdata_owns_existing_bytes_until_its_final_release() {
        let bytes:DataOwner=Arc::new(crate::font::SANS_R);
        assert_eq!(Arc::strong_count(&bytes),1);
        let data=font_data(&bytes).unwrap();
        assert_eq!(Arc::strong_count(&bytes),2);
        let weak=Arc::downgrade(&bytes);drop(bytes);
        assert!(weak.upgrade().is_some());
        unsafe { assert_eq!(data_slice(&data),crate::font::SANS_R); }
        drop(data);assert!(weak.upgrade().is_none());
    }
}
