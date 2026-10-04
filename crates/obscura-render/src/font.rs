//! Shared real font resources, selection, and variation raster cache.
use std::{collections::{hash_map::DefaultHasher, HashMap, VecDeque}, hash::{Hash, Hasher}, path::PathBuf, sync::{Arc, Mutex, OnceLock}};
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};
use cosmic_text::{CacheKey, CacheKeyFlags, Color, FontSystem, FontVariations, SwashImage, VariationTag};
use swash::scale::{image::Content as SwashContent, Render, ScaleContext, Source, StrikeWith};
use swash::zeno::{Angle, Format, Transform, Vector};
// Bundled resources back deterministic generic defaults. Named availability
// comes from actual loaded metadata, including configured and resource faces;
// a similar name never establishes that a font is installed.
pub(crate) static SANS_R: &[u8] = include_bytes!("../assets/liberation-sans.ttf");
pub(crate) static SANS_B: &[u8] = include_bytes!("../assets/liberation-sans-bold.ttf");
pub(crate) static SANS_O: &[u8] = include_bytes!("../assets/liberation-sans-oblique.ttf");
pub(crate) static SANS_BO: &[u8] = include_bytes!("../assets/liberation-sans-boldoblique.ttf");
pub(crate) static SERIF_R: &[u8] = include_bytes!("../assets/liberation-serif.ttf");
pub(crate) static SERIF_B: &[u8] = include_bytes!("../assets/liberation-serif-bold.ttf");
pub(crate) static SERIF_O: &[u8] = include_bytes!("../assets/liberation-serif-oblique.ttf");
pub(crate) static SERIF_BO: &[u8] = include_bytes!("../assets/liberation-serif-boldoblique.ttf");
pub(crate) static MONO_R: &[u8] = include_bytes!("../assets/liberation-mono.ttf");
pub(crate) static MONO_B: &[u8] = include_bytes!("../assets/liberation-mono-bold.ttf");
pub(crate) static MONO_O: &[u8] = include_bytes!("../assets/liberation-mono-oblique.ttf");
pub(crate) static MONO_BO: &[u8] = include_bytes!("../assets/liberation-mono-boldoblique.ttf");
pub(crate) static SYSTEM_R: &[u8] = include_bytes!("../assets/dejavu-sans.ttf");
pub(crate) static SYSTEM_B: &[u8] = include_bytes!("../assets/dejavu-sans-bold.ttf");
pub(crate) static EMOJI_R: &[u8] = include_bytes!("../assets/noto-color-emoji.ttf");
pub(crate) static CJK_R: &[u8] = include_bytes!("../../../fonts/NotoSansCJKsc-Regular.otf");
#[cfg(test)]
pub(crate) static FALLBACK: &[u8] = SYSTEM_R;

pub(crate) const FAMILY: &str = "Liberation Sans";
pub(crate) const SERIF_FAMILY: &str = "Liberation Serif";
pub(crate) const MONO_FAMILY: &str = "Liberation Mono";
pub(crate) const SYSTEM_FAMILY: &str = "DejaVu Sans";

/// Whether text contains a code point that can request emoji presentation.
/// Keep the color face out of ordinary render passes: its bitmap table is
/// large, and loading it for every page would spend RSS and startup time even
/// when no emoji can be shaped.
pub(crate) fn text_may_need_emoji_font(text: &str) -> bool {
    text.chars().any(|ch| {
        matches!(
            ch,
            '\u{00A9}' | '\u{00AE}' | '\u{203C}' | '\u{2049}' | '\u{2122}' | '\u{2139}'
                | '\u{2194}'..='\u{2199}' | '\u{21A9}'..='\u{21AA}'
                | '\u{231A}'..='\u{231B}' | '\u{2328}' | '\u{23CF}'
                | '\u{23E9}'..='\u{23F3}' | '\u{23F8}'..='\u{23FA}' | '\u{24C2}'
                | '\u{25AA}'..='\u{25AB}' | '\u{25B6}' | '\u{25C0}'
                | '\u{25FB}'..='\u{25FE}' | '\u{2600}'..='\u{2604}' | '\u{2611}'
                | '\u{2614}'..='\u{2615}' | '\u{2618}' | '\u{261D}' | '\u{2620}'
                | '\u{2622}'..='\u{2623}' | '\u{2626}' | '\u{262A}'
                | '\u{262E}'..='\u{262F}' | '\u{2638}'..='\u{263A}' | '\u{2640}'
                | '\u{2642}' | '\u{2648}'..='\u{2653}' | '\u{265F}'..='\u{2660}'
                | '\u{2663}' | '\u{2665}'..='\u{2666}' | '\u{2668}' | '\u{267B}'
                | '\u{267E}'..='\u{267F}' | '\u{2692}'..='\u{2697}' | '\u{2699}'
                | '\u{269B}'..='\u{269C}' | '\u{26A0}'..='\u{26A1}' | '\u{26A7}'
                | '\u{26AA}'..='\u{26AB}' | '\u{26B0}'..='\u{26B1}'
                | '\u{26BD}'..='\u{26BE}' | '\u{26C4}'..='\u{26C5}' | '\u{26C8}'
                | '\u{26CE}'..='\u{26CF}' | '\u{26D1}' | '\u{26D3}'..='\u{26D4}'
                | '\u{26E9}'..='\u{26EA}' | '\u{26F0}'..='\u{26F5}'
                | '\u{26F7}'..='\u{26FA}' | '\u{26FD}' | '\u{2702}' | '\u{2705}'
                | '\u{2708}'..='\u{270D}' | '\u{270F}' | '\u{2712}' | '\u{2714}'
                | '\u{2716}' | '\u{271D}' | '\u{2721}' | '\u{2728}'
                | '\u{2733}'..='\u{2734}' | '\u{2744}' | '\u{2747}' | '\u{274C}'
                | '\u{274E}' | '\u{2753}'..='\u{2755}' | '\u{2757}'
                | '\u{2763}'..='\u{2764}' | '\u{2795}'..='\u{2797}' | '\u{27A1}'
                | '\u{27B0}' | '\u{27BF}' | '\u{2934}'..='\u{2935}'
                | '\u{2B05}'..='\u{2B07}' | '\u{2B1B}'..='\u{2B1C}' | '\u{2B50}'
                | '\u{2B55}' | '\u{3030}' | '\u{303D}' | '\u{3297}' | '\u{3299}'
                | '\u{FE0F}' | '\u{1F000}'..='\u{1FAFF}'
        )
    })
}

/// Existing backed generic routing. Unimplemented generics remain valid CSS
/// and continue fallback; they do not imply an installed named family.
fn generic_font_family(name: &str) -> Option<&'static str> {
    match name.to_ascii_lowercase().as_str() {
        "serif" => Some(SERIF_FAMILY),
        "monospace" | "ui-monospace" => Some(MONO_FAMILY),
        "system-ui" | "ui-sans-serif" => Some(SYSTEM_FAMILY),
        // Preserve the existing explicit vendor compatibility policy.
        "sans-serif" | "-apple-system" => Some(FAMILY),
        _ => None,
    }
}

/// Only these exact bundled names are available to the legacy byte painter.
fn bundled_named_family(name: &str) -> Option<&'static str> {
    [FAMILY, SERIF_FAMILY, MONO_FAMILY, SYSTEM_FAMILY].into_iter()
        .find(|family| family.eq_ignore_ascii_case(name))
}

pub(crate) fn resolve_font_family(fam: Option<&str>) -> &'static str {
    use crate::style::{parse_font_family_list, CssFontFamily};
    let Some(source) = fam else { return FAMILY };
    if let Some(family) = generic_font_family(source.trim()) { return family; }
    if let Some(families) = parse_font_family_list(source) {
        for token in families {
            let family = match token {
                CssFontFamily::Named(name) => bundled_named_family(&name),
                CssFontFamily::Generic(name) => generic_font_family(&name),
            };
            if let Some(family) = family { return family; }
        }
    }
    FAMILY
}

#[derive(Clone)]
pub(crate) struct LoadedFamily {
    pub(crate) faces: Vec<LoadedFace>,
}

#[derive(Clone)]
pub(crate) struct LoadedFace {
    pub(crate) name: Arc<str>,
    pub(crate) font_id: Option<cosmic_text::fontdb::ID>,
    pub(crate) metrics: FaceMetrics,
    pub(crate) min_weight: u16,
    pub(crate) max_weight: u16,
    pub(crate) italic: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FaceMetrics {
    pub(crate) ascent: f32,
    pub(crate) descent: f32,
    pub(crate) line_gap: f32,
    pub(crate) units_per_em: f32,
}

#[derive(Clone)]
pub(crate) struct ResolvedFont {
    pub(crate) family: Arc<str>,
    pub(crate) font_id: Option<cosmic_text::fontdb::ID>,
    pub(crate) metrics: FaceMetrics,
    pub(crate) synthetic_italic: bool,
}

#[derive(Clone, Eq, PartialEq)]
pub(crate) struct WebFont {
    pub data: Arc<Vec<u8>>,
    pub family: Option<String>,
    pub weight: Option<(u16, u16)>,
    pub italic: Option<bool>,
}

pub(crate) type FontDatabase = (
    cosmic_text::fontdb::Database,
    HashMap<String, LoadedFamily>,
);
pub(crate) type FontDeclaration = (
    cosmic_text::fontdb::ID,
    Option<String>,
    Option<(u16, u16)>,
    Option<bool>,
);

pub(crate) const WEB_FONT_CACHE_ENTRIES: usize = 8;
pub(crate) const WEB_FONT_CACHE_BYTES: usize = 64 * 1024 * 1024;

pub(crate) struct CachedWebFontSet {
    pub(crate) signature: u64,
    pub(crate) load_emoji: bool,
    pub(crate) fonts: Vec<WebFont>,
    pub(crate) bytes: usize,
    pub(crate) database: FontDatabase,
}

pub(crate) static FONT_DIRECTORIES: OnceLock<Vec<PathBuf>> = OnceLock::new();
pub(crate) static BASE_FONT_DATABASE: OnceLock<FontDatabase> = OnceLock::new();
pub(crate) static EMOJI_FONT_DATABASE: OnceLock<FontDatabase> = OnceLock::new();
pub(crate) static WEB_FONT_DATABASES: OnceLock<Mutex<VecDeque<Arc<CachedWebFontSet>>>> = OnceLock::new();

#[cfg(test)]
pub(crate) static BASE_FONT_DATABASE_BUILDS: AtomicUsize = AtomicUsize::new(0);

/// Configure additional process-wide fonts before the first render.
///
/// Returns false when fonts have already been configured or initialized.
pub fn configure_font_directories(directories: Vec<PathBuf>) -> bool {
    if BASE_FONT_DATABASE.get().is_some() {
        return false;
    }
    FONT_DIRECTORIES.set(directories).is_ok()
}

pub(crate) fn load_font_directories(
    database: &mut cosmic_text::fontdb::Database,
    directories: &[PathBuf],
) -> Vec<cosmic_text::fontdb::ID> {
    let mut pending = directories.to_vec();
    let mut files = Vec::new();
    while let Some(directory) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }
            let path = entry.path();
            if file_type.is_dir() {
                pending.push(path);
            } else if file_type.is_file()
                && path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| {
                        matches!(
                            extension.to_ascii_lowercase().as_str(),
                            "ttf" | "ttc" | "otf" | "otc"
                        )
                    })
            {
                files.push(path);
            }
        }
    }

    files.sort_unstable();
    files.dedup();
    let mut ids = Vec::new();
    for path in files {
        let Ok(data) = std::fs::read(path) else {
            continue;
        };
        ids.extend(database.load_font_source(cosmic_text::fontdb::Source::Binary(Arc::new(data))));
    }
    ids
}

pub(crate) fn register_loaded_faces(
    database: &cosmic_text::fontdb::Database,
    declarations: Vec<FontDeclaration>,
) -> HashMap<String, LoadedFamily> {
    let mut loaded_families = HashMap::new();
    for (id, declared_family, declared_weight, declared_italic) in declarations {
        let Some(face) = database.face(id) else {
            continue;
        };
        let names = face.families.clone();
        let internal_name = names
            .first()
            .map(|(name, _)| Arc::<str>::from(name.as_str()))
            .unwrap_or_else(|| Arc::from(FAMILY));
        let shape_weight = face.weight.0;
        let metrics = font_metrics(database, id)
            .unwrap_or_else(|| bundled_face_metrics(internal_name.as_ref()));
        let italic = declared_italic
            .unwrap_or(!matches!(face.style, cosmic_text::fontdb::Style::Normal));
        let weight = declared_weight.unwrap_or((shape_weight, shape_weight));
        let declared_names: Vec<String> = declared_family
            .map(|name| vec![name])
            .unwrap_or_else(|| names.into_iter().map(|(name, _)| name).collect());
        for name in declared_names {
            let family = loaded_families
                .entry(family_key(&name))
                .or_insert_with(|| LoadedFamily { faces: Vec::new() });
            family.faces.push(LoadedFace {
                name: Arc::clone(&internal_name),
                font_id: Some(id),
                metrics,
                min_weight: weight.0,
                max_weight: weight.1,
                italic,
            });
        }
    }
    loaded_families
}

pub(crate) fn base_font_database(load_emoji: bool) -> &'static FontDatabase {
    let base = BASE_FONT_DATABASE.get_or_init(|| {
        #[cfg(test)]
        BASE_FONT_DATABASE_BUILDS.fetch_add(1, Ordering::Relaxed);

        let mut database = cosmic_text::fontdb::Database::new();
        let mut declarations = Vec::new();
        for bytes in [
            SANS_R, SANS_B, SANS_O, SANS_BO, SERIF_R, SERIF_B, SERIF_O, SERIF_BO, MONO_R, MONO_B,
            MONO_O, MONO_BO, SYSTEM_R, SYSTEM_B, CJK_R,
        ] {
            for id in database
                .load_font_source(cosmic_text::fontdb::Source::Binary(Arc::new(bytes)))
            {
                declarations.push((id, None, None, None));
            }
        }
        declarations.extend(
            load_font_directories(
                &mut database,
                FONT_DIRECTORIES.get_or_init(Vec::new),
            )
            .into_iter()
            .map(|id| (id, None, None, None)),
        );
        let loaded_families = register_loaded_faces(&database, declarations);
        database.set_sans_serif_family(FAMILY);
        (database, loaded_families)
    });

    if !load_emoji {
        return base;
    }
    EMOJI_FONT_DATABASE.get_or_init(|| {
        let (mut database, _) = (*base).clone();
        let declarations = database
            .load_font_source(cosmic_text::fontdb::Source::Binary(Arc::new(EMOJI_R)))
            .into_iter()
            .map(|id| (id, None, None, None))
            .collect();
        let loaded_families = register_loaded_faces(&database, declarations);
        let mut all_loaded_families = base.1.clone();
        for (name, mut family) in loaded_families {
            all_loaded_families
                .entry(name)
                .or_insert_with(|| LoadedFamily { faces: Vec::new() })
                .faces
                .append(&mut family.faces);
        }
        (database, all_loaded_families)
    })
}

pub(crate) fn web_font_signature(fonts: &[WebFont], load_emoji: bool) -> u64 {
    let mut hasher = DefaultHasher::new();
    load_emoji.hash(&mut hasher);
    fonts.len().hash(&mut hasher);
    for font in fonts {
        font.family.hash(&mut hasher);
        font.weight.hash(&mut hasher);
        font.italic.hash(&mut hasher);
        let data = font.data.as_slice();
        data.len().hash(&mut hasher);
        data[..data.len().min(64)].hash(&mut hasher);
        if data.len() > 64 {
            data[data.len() - 64..].hash(&mut hasher);
        }
        if data.len() > 128 {
            let middle = data.len() / 2;
            data[middle - 32..middle + 32].hash(&mut hasher);
        }
    }
    hasher.finish()
}

pub(crate) fn cached_web_font_database(
    fonts: &[WebFont],
    load_emoji: bool,
) -> Option<Arc<CachedWebFontSet>> {
    let signature = web_font_signature(fonts, load_emoji);
    let candidates: Vec<_> = WEB_FONT_DATABASES
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .iter()
        .filter(|entry| entry.signature == signature && entry.load_emoji == load_emoji)
        .cloned()
        .collect();
    candidates.into_iter().find(|entry| entry.fonts == fonts)
}

pub(crate) fn cache_web_font_database(
    fonts: &[WebFont],
    load_emoji: bool,
    database: FontDatabase,
) -> FontDatabase {
    let bytes = fonts
        .iter()
        .fold(0usize, |total, font| total.saturating_add(font.data.len()));
    if bytes > WEB_FONT_CACHE_BYTES {
        return database;
    }

    let signature = web_font_signature(fonts, load_emoji);
    let mut cache = WEB_FONT_DATABASES
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(existing) = cache.iter().find(|entry| {
        entry.signature == signature && entry.load_emoji == load_emoji && entry.fonts == fonts
    }) {
        return existing.database.clone();
    }
    while cache.len() >= WEB_FONT_CACHE_ENTRIES
        || cache.iter().map(|entry| entry.bytes).sum::<usize>()
            > WEB_FONT_CACHE_BYTES.saturating_sub(bytes)
    {
        cache.pop_front();
    }
    cache.push_back(Arc::new(CachedWebFontSet {
        signature,
        load_emoji,
        fonts: fonts.to_vec(),
        bytes,
        database: database.clone(),
    }));
    database
}

pub(crate) fn family_key(name: &str) -> String { name.to_lowercase() }

fn resolve_font_stack(fam: Option<&str>, mut select: impl FnMut(&str, bool) -> Option<ResolvedFont>) -> ResolvedFont {
    use crate::style::{parse_font_family_list, CssFontFamily};
    if let Some(stack) = fam {
        if let Some(generic) = generic_font_family(stack.trim()) {
            if let Some(font) = select(generic, false) { return font; }
        } else if let Some(tokens) = parse_font_family_list(stack) {
            for token in tokens {
                let (name, named) = match token {
                    CssFontFamily::Named(name) => (name, true),
                    CssFontFamily::Generic(name) => {
                        let Some(generic) = generic_font_family(&name) else { continue; };
                        (generic.to_owned(), false)
                    }
                };
                if let Some(font) = select(&name, named) { return font; }
            }
        }
    }
    ResolvedFont { family: Arc::from(FAMILY), font_id: None, metrics: bundled_face_metrics(FAMILY), synthetic_italic: false }
}

pub(crate) fn resolve_loaded_font(fam: Option<&str>, weight: u16, italic: bool, loaded: &HashMap<String, LoadedFamily>) -> ResolvedFont {
    resolve_font_stack(fam, |name, _| loaded.get(&family_key(name)).and_then(|family| select_loaded_face(family, weight, italic)))
}

/// Per-engine native IDs/admission. A DOM engine freezes complete choices for one pass.
/// Neither global provider cache nor retained CSS styles store database-local IDs.
pub(crate) struct NativeFonts {
    provider: NativeProvider,
    admission: crate::native_font::Admission,
    files: Vec<(Arc<crate::native_font::NativeBytes>, HashMap<u32, cosmic_text::fontdb::ID>)>,
    freeze: bool,
    stopped: bool,
    misses: HashMap<(String, u16, bool), bool>,
    styles: HashMap<(String, u16, bool), cosmic_text::fontdb::ID>,
    // Faces from a successfully resolved Named resource, including its real TTC
    // siblings. Incidental cascade imports never enter this provenance set.
    named_faces: std::collections::HashSet<cosmic_text::fontdb::ID>,
    decisions: HashMap<String, Vec<(u16, bool, ResolvedFont)>>,
    decision_count: usize,
    decision_bytes: usize,
    retry: bool,
    lookup_remaining: usize,
    evictions_remaining: usize,
    operation_pins: Vec<Arc<crate::native_font::NativeBytes>>,
    #[cfg(test)] fail_registration: bool,
    #[cfg(test)] owner_touches: usize,
}
#[derive(Clone)]
enum NativeProvider { Host, Disabled, #[cfg(test)] Injected(Arc<crate::native_font::Provider>) }
fn native_provider() -> NativeProvider {
    #[cfg(test)]
    if let Some(provider) = TEST_NATIVE_PROVIDER.with(|slot| slot.borrow().clone()) { return provider; }
    NativeProvider::Host
}
#[cfg(test)]
thread_local! { static TEST_NATIVE_PROVIDER: std::cell::RefCell<Option<NativeProvider>> = const { std::cell::RefCell::new(None) }; }
#[cfg(test)]
pub(crate) fn with_native_provider_for_test<R>(provider: Option<Arc<crate::native_font::Provider>>, work: impl FnOnce() -> R) -> R {
    struct Restore(Option<NativeProvider>);
    impl Drop for Restore { fn drop(&mut self) { TEST_NATIVE_PROVIDER.with(|slot| *slot.borrow_mut() = self.0.take()); } }
    let previous = TEST_NATIVE_PROVIDER.with(|slot| slot.replace(Some(provider.map_or(NativeProvider::Disabled, NativeProvider::Injected))));
    let _restore = Restore(previous);
    work()
}
impl NativeFonts {
    pub(crate) fn host() -> Self { Self::new(native_provider(), false) }
    pub(crate) fn for_layout() -> Self { Self::new(native_provider(), true) }
    pub(crate) fn disabled() -> Self { Self::new(NativeProvider::Disabled, false) }
    fn new(provider: NativeProvider, freeze: bool) -> Self {
        Self { provider, admission: crate::native_font::Admission::new(), files: Vec::new(), freeze, stopped: false,
            misses: HashMap::new(), styles: HashMap::new(), named_faces: std::collections::HashSet::new(), decisions: HashMap::new(), decision_count: 0, decision_bytes: 0, retry: false, lookup_remaining: 128, evictions_remaining: 0, operation_pins: Vec::new(), #[cfg(test)] fail_registration: false, #[cfg(test)] owner_touches: 0 }
    }
    #[cfg(test)]
    pub(crate) fn injected(provider: Arc<crate::native_font::Provider>, freeze: bool) -> Self { Self::new(NativeProvider::Injected(provider), freeze) }
    pub(crate) fn retry_needed(&self) -> bool { self.retry }
    #[cfg(test)]
    pub(crate) fn fail_next_registration(&mut self) { self.fail_registration = true; }
    #[cfg(test)]
    pub(crate) fn owner_touch_count(&self) -> usize { self.owner_touches }
    #[cfg(test)]
    pub(crate) fn reduce_canvas_file_limit(&mut self, limit: usize) { assert!(!self.freeze); self.admission.reduce_file_limit(limit); }
    #[cfg(test)]
    pub(crate) fn resident_counts(&self) -> (usize, usize, usize) { self.admission.resident_counts() }
    /// Called only at the Canvas pre-shape boundary after its prior caches drop.
    fn evict_idle_canvas_file(&mut self, fs: &mut FontSystem, loaded: &mut HashMap<String, LoadedFamily>, before_evict: &mut dyn FnMut() -> bool) -> bool {
        if self.freeze || self.evictions_remaining == 0 || self.files.is_empty() { return false; }
        let Some(victim) = self.files.iter().position(|(bytes, _)|
            !self.operation_pins.iter().any(|pin| Arc::ptr_eq(bytes, pin))) else { return false; };
        if !before_evict() { return false; }
        self.evictions_remaining -= 1;
        let (bytes, mapping) = self.files.remove(victim);
        let ids: Vec<_> = mapping.into_values().collect();
        fs.remove_font_faces(&ids);
        loaded.retain(|_, family| {
            family.faces.retain(|face| !face.font_id.is_some_and(|id| ids.contains(&id)));
            !family.faces.is_empty()
        });
        // Exact loaded styles still hit without provider I/O. Retry bookkeeping
        // for nearest-style requests may be relearned within the same bound.
        self.styles.clear();
        self.named_faces.retain(|id| !ids.contains(id));
        self.admission.release(&bytes);
        true
    }
    pub(crate) fn canvas_run_owners(&self, ids: impl Iterator<Item = cosmic_text::fontdb::ID>) -> Vec<std::sync::Weak<crate::native_font::NativeBytes>> {
        if self.files.is_empty() { return Vec::new(); }
        let ids: std::collections::HashSet<_> = ids.collect();
        self.files.iter().filter(|(_, mapping)| mapping.values().any(|id| ids.contains(id)))
            .map(|(bytes, _)| Arc::downgrade(bytes)).collect()
    }
    pub(crate) fn touch_canvas_owners(&mut self, owners: &[std::sync::Weak<crate::native_font::NativeBytes>]) {
        if self.freeze { return; }
        for owner in owners {
            #[cfg(test)] { self.owner_touches += 1; }
            if let Some(index) = self.files.iter().position(|(bytes, _)| std::ptr::eq(Arc::as_ptr(bytes), owner.as_ptr())) {
                let file = self.files.remove(index); self.files.push(file);
            }
        }
    }
    /// End actual-consumer preparation before shaping a DOM pass.
    pub(crate) fn seal(&mut self) { self.stopped = true; }
    pub(crate) fn is_native_face(&self, id: cosmic_text::fontdb::ID) -> bool { self.files.iter().any(|(_, ids)| ids.values().any(|stored| *stored == id)) }
    fn prepared(&self, fam: Option<&str>, weight: u16, italic: bool) -> Option<ResolvedFont> {
        self.decisions.get(fam.unwrap_or("")).and_then(|rows| rows.iter().find(|(w, i, _)| *w == weight && *i == italic)).map(|(_, _, font)| font.clone())
    }
    fn provider(provider: &NativeProvider) -> Option<Arc<crate::native_font::Provider>> {
        match provider {
            NativeProvider::Disabled => None,
            #[cfg(test)] NativeProvider::Injected(provider) => Some(provider.clone()),
            NativeProvider::Host => {
                #[cfg(target_os = "macos")]
                { static PROVIDER: OnceLock<Arc<crate::native_font::Provider>> = OnceLock::new();
                    Some(PROVIDER.get_or_init(|| Arc::new(crate::native_font::Provider::new())).clone()) }
                #[cfg(not(target_os = "macos"))]
                { None }
            }
        }
    }
    fn lookup(provider: &NativeProvider, name: &str, weight: u16, italic: bool, reclaim: &mut dyn FnMut() -> bool) -> crate::native_font::Lookup {
        Self::provider(provider).map_or(crate::native_font::Lookup::Absent,
            |provider| provider.lookup_styled_reclaim(name, weight, italic, reclaim))
    }
    pub(crate) fn pin_canvas_face(&mut self, id: cosmic_text::fontdb::ID) {
        if let Some((bytes, _)) = self.files.iter().find(|(_, mapping)| mapping.values().any(|face| *face == id)) {
            if !self.operation_pins.iter().any(|pin| Arc::ptr_eq(pin, bytes)) { self.operation_pins.push(bytes.clone()); }
        }
    }
    pub(crate) fn finish_canvas_operation(&mut self) { self.operation_pins.clear(); }
    pub(crate) fn canvas_cascade(&mut self, request: &crate::native_font::FallbackRequest<'_>) -> crate::native_font::Cascade {
        let Some(provider) = Self::provider(&self.provider) else { return Default::default(); };
        if self.lookup_remaining == 0 { self.retry = true; return Default::default(); }
        // Reserve the remaining logical candidate work once, including invalid descriptors.
        // This operation never restarts the prefix or raises the common 128 limit.
        let limit = self.lookup_remaining;
        self.lookup_remaining = 0;
        match provider.cascade(request, limit) {
            Ok(cascade) => cascade,
            Err(_) => { self.retry = true; Default::default() }
        }
    }
    pub(crate) fn incomplete_canvas_fallback(&mut self) { self.retry = true; }
    pub(crate) fn canvas_candidate(&mut self, candidate: crate::native_font::Located, missing: &str,
        fs: &mut FontSystem, loaded: &mut HashMap<String, LoadedFamily>, before_evict: &mut dyn FnMut() -> bool)
        -> Option<cosmic_text::fontdb::ID> {
        let provider = Self::provider(&self.provider)?;
        let asset = match provider.load_candidate(candidate, &mut || self.evict_idle_canvas_file(fs, loaded, before_evict)) {
            crate::native_font::Lookup::Found(asset) => asset,
            _ => { self.retry = true; return None; }
        };
        // This is only an inexpensive rejection filter. Coverage and cluster replacement
        // are decided by reshaping the entire text in Buffer, never by summing cmap widths.
        let bytes = asset.bytes();
        let face = cosmic_text::ttf_parser::Face::parse(bytes.as_ref().as_ref(), asset.selected_index).ok()?;
        if !missing.chars().any(|ch| face.glyph_index(ch).is_some()) { return None; }
        let name = asset.faces().iter().find(|face| face.index == asset.selected_index)?.families.first()?.clone();
        self.import_asset(&asset, &name, fs, loaded, before_evict)
    }
    /// Import is transactional: usable selected face first, then local ledger/aliases.
    fn import_asset(&mut self, asset: &crate::native_font::NativeAsset, name: &str, fs: &mut FontSystem,
        loaded: &mut HashMap<String, LoadedFamily>, before_evict: &mut dyn FnMut() -> bool) -> Option<cosmic_text::fontdb::ID> {
        let key = family_key(name);
        if loaded.get(&key).is_some_and(|family| !self.owns_family(family)) { return None; }
        while self.admission.can_admit(asset).is_err() {
            if !self.admission.can_fit_alone(asset) || !self.evict_idle_canvas_file(fs, loaded, before_evict) {
                self.retry = true; return None;
            }
        }
        let bytes = asset.bytes();
        let prior = self.files.iter().position(|(prior, _)| Arc::ptr_eq(prior, &bytes));
        let mut mapping = prior.map_or_else(HashMap::new, |i| self.files[i].1.clone());
        let indices: Vec<_> = asset.family_indices.iter().copied().filter(|index| !mapping.contains_key(index)).collect();
        let added = if indices.is_empty() { Vec::new() } else {
            fs.append_font_source(cosmic_text::fontdb::Source::Binary(bytes.clone()), Some(&indices))
        };
        for id in &added { if let Some(face) = fs.db().face(*id) { mapping.insert(face.index, *id); } }
        let selected = mapping.get(&asset.selected_index).copied().filter(|id| fs.get_font(*id).is_some());
        let declarations = asset.family_indices.iter().filter_map(|index| mapping.get(index).copied())
            .map(|id| (id, Some(name.to_owned()), None, None)).collect();
        let mut registered = register_loaded_faces(fs.db(), declarations).remove(&key);
        #[cfg(test)] if std::mem::take(&mut self.fail_registration) { registered = None; }
        if selected.is_none() || registered.is_none() {
            fs.remove_font_faces(&added); self.retry = true; return None;
        }
        // No operation can interleave between capacity check and this commit.
        if self.admission.admit(asset).is_err() {
            fs.remove_font_faces(&added); self.retry = true; return None;
        }
        if let Some(index) = prior { self.files[index].1 = mapping; }
        else { self.files.push((bytes, mapping)); }
        let mut family = registered.take().unwrap();
        if let Some(existing) = loaded.get_mut(&key) {
            family.faces.retain(|face| !existing.faces.iter().any(|prior| prior.font_id == face.font_id));
            existing.faces.append(&mut family.faces);
        } else { loaded.insert(key, family); }
        selected
    }
    fn selected_style(&self, name: &str, weight: u16, italic: bool, family: &LoadedFamily) -> Option<ResolvedFont> {
        let id = self.styles.get(&(family_key(name), weight, italic))?;
        let face = family.faces.iter().find(|face| face.font_id == Some(*id))?;
        Some(ResolvedFont { family: face.name.clone(), font_id: Some(*id), metrics: face.metrics,
            synthetic_italic: italic && !face.italic })
    }
    fn named_exact_style(&self, weight: u16, italic: bool, family: &LoadedFamily) -> Option<ResolvedFont> {
        // Reuse a unique exact static sibling already imported by a Named TTC
        // lookup. Ranged or ambiguous resources still need canonical OS selection.
        let mut faces = family.faces.iter().filter(|face| face.min_weight == weight && face.max_weight == weight
            && face.italic == italic && face.font_id.is_some_and(|id| self.named_faces.contains(&id)));
        let face = faces.next()?;
        if faces.next().is_some() { return None; }
        Some(ResolvedFont { family: face.name.clone(), font_id: face.font_id, metrics: face.metrics,
            synthetic_italic: false })
    }
    fn named_fallback(&self, weight: u16, italic: bool, family: &LoadedFamily) -> Option<ResolvedFont> {
        // A transient lookup failure may reuse a previously verified Named face,
        // but never adopts an incidental cascade alias or memoizes that failure.
        let confirmed = LoadedFamily { faces: family.faces.iter().filter(|face|
            face.font_id.is_some_and(|id| self.named_faces.contains(&id))).cloned().collect() };
        select_loaded_face(&confirmed, weight, italic)
    }
    fn owns_family(&self, family: &LoadedFamily) -> bool {
        !family.faces.is_empty() && family.faces.iter().all(|face| face.font_id.is_some_and(|id| self.is_native_face(id)))
    }
    fn ensure(&mut self, name: &str, weight: u16, italic: bool, fs: &mut FontSystem, loaded: &mut HashMap<String, LoadedFamily>, before_evict: &mut dyn FnMut() -> bool) {
        let key = family_key(name);
        let request = (key.clone(), weight, italic);
        if self.stopped || self.styles.contains_key(&request) { return; }
        if self.styles.len() >= 128 && !self.evict_idle_canvas_file(fs, loaded, before_evict) {
            self.retry = true; return;
        }
        if let Some(transient) = self.misses.get(&request) { self.retry |= *transient; return; }
        if self.freeze && self.misses.len() >= 128 { self.stopped = true; return; }
        if self.lookup_remaining == 0 { self.retry = true; return; }
        self.lookup_remaining -= 1;
        let provider = self.provider.clone();
        let result = Self::lookup(&provider, name, weight, italic,
            &mut || self.evict_idle_canvas_file(fs, loaded, before_evict));
        let asset = match result {
            crate::native_font::Lookup::Found(asset) => asset,
            crate::native_font::Lookup::Absent => { if self.freeze { self.misses.insert(request, false); } return; }
            crate::native_font::Lookup::Unavailable(reason) => {
                self.retry = true;
                let _ = reason;
                if self.freeze { self.misses.insert(request, true); } return;
            }
        };
        // Keep Found alive while the current token obtains local capacity.
        if let Some(id) = self.import_asset(&asset, name, fs, loaded, before_evict) {
            let bytes = asset.bytes();
            if let Some((_, mapping)) = self.files.iter().find(|(owner, _)| Arc::ptr_eq(owner, &bytes)) {
                self.named_faces.extend(asset.family_indices.iter().filter_map(|index| mapping.get(index).copied()));
            }
            self.styles.insert(request, id);
        } else { self.retry = true; if self.freeze { self.misses.insert(request, true); } }

    }
}

pub(crate) fn resolve_native_font(fam: Option<&str>, weight: u16, italic: bool, fs: &mut FontSystem, loaded: &mut HashMap<String, LoadedFamily>, native: &mut NativeFonts) -> ResolvedFont {
    resolve_native_font_with_reclaim(fam, weight, italic, fs, loaded, native, &mut || false)
}

/// Canvas supplies cache retirement at the pre-Buffer boundary. DOM opts out.
pub(crate) fn resolve_native_font_with_reclaim(fam: Option<&str>, weight: u16, italic: bool, fs: &mut FontSystem, loaded: &mut HashMap<String, LoadedFamily>, native: &mut NativeFonts, before_evict: &mut dyn FnMut() -> bool) -> ResolvedFont {
    native.retry = false;
    native.lookup_remaining = 128;
    native.evictions_remaining = 16;
    native.operation_pins.clear();
    if let Some(font) = native.prepared(fam, weight, italic) { return font; }
    let stack = fam.unwrap_or("");
    let new_bytes = if native.decisions.contains_key(stack) { 0 } else { stack.len() };
    let at_capacity = native.freeze && (native.decision_count >= 1024 || new_bytes > (256usize << 10).saturating_sub(native.decision_bytes));
    let mut needs_decision = false;
    let font = resolve_font_stack(fam, |name, named| {
        let key = family_key(name);
        if let Some(family) = loaded.get(&key) {
            let exact_style = family.faces.iter().any(|face| face.italic == italic && (face.min_weight..=face.max_weight).contains(&weight));
            if !named || !native.owns_family(family) || (native.freeze && exact_style) {
                return select_loaded_face(family, weight, italic);
            }
            if !native.freeze {
                if let Some(font) = native.selected_style(name, weight, italic, family) { return Some(font); }
                if let Some(font) = native.named_exact_style(weight, italic, family) {
                    if font.font_id.is_some_and(|id| fs.get_font(id).is_some()) { return Some(font); }
                }
            }
        }
        if named {
            needs_decision = true;
            if at_capacity { native.stopped = true; }
            native.ensure(name, weight, italic, fs, loaded, before_evict);
        }
        loaded.get(&key).and_then(|family| {
            if !native.freeze && native.owns_family(family) {
                native.selected_style(name, weight, italic, family)
                    .or_else(|| native.named_fallback(weight, italic, family))
            }
            else { select_loaded_face(family, weight, italic) }
        })
    });
    if native.freeze && needs_decision && !native.stopped {
        native.decisions.entry(stack.to_owned()).or_default().push((weight, italic, font.clone()));
        native.decision_count += 1; native.decision_bytes += new_bytes;
    }
    font
}

/// Read a prepared choice without host I/O. Overflow freezes native imports first.
pub(crate) fn resolve_prepared_font(fam: Option<&str>, weight: u16, italic: bool, loaded: &HashMap<String, LoadedFamily>, native: &NativeFonts) -> ResolvedFont {
    native.prepared(fam, weight, italic).unwrap_or_else(|| resolve_loaded_font(fam, weight, italic, loaded))
}

/// Emoji is appended so native/resource IDs and byte owners survive the first emoji run.
pub(crate) fn append_emoji_font(fs: &mut FontSystem, loaded: &mut HashMap<String, LoadedFamily>) {
    let ids = fs.append_font_source(cosmic_text::fontdb::Source::Binary(Arc::new(EMOJI_R)), None);
    for (name, mut family) in register_loaded_faces(fs.db(), ids.into_iter().map(|id| (id, None, None, None)).collect()) {
        loaded.entry(name).or_insert_with(|| LoadedFamily { faces: Vec::new() }).faces.append(&mut family.faces);
    }
}

pub(crate) fn select_loaded_face(
    family: &LoadedFamily,
    requested_weight: u16,
    requested_italic: bool,
) -> Option<ResolvedFont> {
    let exact_style: Vec<_> = family
        .faces
        .iter()
        .filter(|face| face.italic == requested_italic)
        .collect();
    let candidates: Vec<_> = if exact_style.is_empty() {
        family.faces.iter().collect()
    } else {
        exact_style
    };
    if let Some(face) = candidates
        .iter()
        .copied()
        .find(|face| (face.min_weight..=face.max_weight).contains(&requested_weight))
    {
        // The named-family matcher uses fontdb's default weight for this
        // resource, while a variable face commonly advertises `100 900` in
        // CSS. Preserve the descriptor-selected file and its database weight;
        // the authored coordinate enters the canonical axis tuple below.
        return Some(ResolvedFont {
            family: Arc::clone(&face.name),
            font_id: face.font_id,
            metrics: face.metrics,
            synthetic_italic: requested_italic && !face.italic,
        });
    }
    let available: Vec<_> = candidates.iter().map(|face| face.min_weight).collect();
    let matched = match_font_weight(requested_weight, &available);
    candidates
        .into_iter()
        .find(|face| face.min_weight == matched)
        .map(|face| ResolvedFont {
            family: Arc::clone(&face.name),
            font_id: face.font_id,
            metrics: face.metrics,
            synthetic_italic: requested_italic && !face.italic,
        })
}

/// CSS Fonts' asymmetric missing-weight search. In particular, 600 selects
/// 700 (not 400) when a family only provides regular and bold faces.
pub(crate) fn match_font_weight(requested: u16, available: &[u16]) -> u16 {
    if available.contains(&requested) {
        return requested;
    }
    let mut weights = available.to_vec();
    weights.sort_unstable();
    weights.dedup();
    if weights.is_empty() {
        return requested;
    }
    if (400..=500).contains(&requested) {
        weights
            .iter()
            .copied()
            .filter(|weight| *weight >= requested && *weight <= 500)
            .min()
            .or_else(|| {
                weights
                    .iter()
                    .copied()
                    .filter(|weight| *weight < requested)
                    .max()
            })
            .or_else(|| weights.iter().copied().filter(|weight| *weight > 500).min())
            .unwrap_or(requested)
    } else if requested < 400 {
        weights
            .iter()
            .copied()
            .filter(|weight| *weight <= requested)
            .max()
            .or_else(|| {
                weights
                    .iter()
                    .copied()
                    .filter(|weight| *weight > requested)
                    .min()
            })
            .unwrap_or(requested)
    } else {
        weights
            .iter()
            .copied()
            .filter(|weight| *weight >= requested)
            .min()
            .or_else(|| {
                weights
                    .iter()
                    .copied()
                    .filter(|weight| *weight < requested)
                    .max()
            })
            .unwrap_or(requested)
    }
}

/// Resolve `line-height: normal` from the selected face's horizontal header.
///
/// Chromium's FreeType-backed Linux path grid-fits the ascent, descent, and
/// line gap independently before adding them. Multiplying their sum by the
/// font size (or rounding the final line height) is observably different at
/// fractional and small sizes: Liberation Sans at 9.333px is 10px in
/// Chromium, not 11px. Keep these metrics beside the embedded faces so normal
/// line boxes follow the same device-pixel rhythm without consulting host
/// fonts.
pub(crate) fn bundled_face_metrics(family: &str) -> FaceMetrics {
    let (ascent, descent, line_gap) = match family {
        SERIF_FAMILY => (1825.0, 443.0, 87.0),
        MONO_FAMILY => (1705.0, 615.0, 0.0),
        SYSTEM_FAMILY => (1901.0, 483.0, 0.0),
        _ => (1854.0, 434.0, 67.0),
    };
    FaceMetrics {
        ascent,
        descent,
        line_gap,
        units_per_em: 2048.0,
    }
}


pub(crate) fn font_metrics(
    db: &cosmic_text::fontdb::Database,
    id: cosmic_text::fontdb::ID,
) -> Option<FaceMetrics> {
    db.with_face_data(id, |data, face_index| {
        let face = cosmic_text::ttf_parser::Face::parse(data, face_index).ok()?;
        Some(FaceMetrics {
            ascent: face.ascender().max(0) as f32,
            descent: -(face.descender().min(0) as f32),
            line_gap: face.line_gap().max(0) as f32,
            units_per_em: face.units_per_em() as f32,
        })
    })
    .flatten()
}


#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct VariableCacheKey {
    pub(crate) glyph: CacheKey,
    pub(crate) variations: Arc<FontVariations>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct VariationIntentKey {
    pub(crate) font_id: cosmic_text::fontdb::ID,
    pub(crate) weight_bits: Option<u32>,
    pub(crate) optical_size_bits: Option<u32>,
    pub(crate) italic: bool,
    pub(crate) explicit: Option<Arc<FontVariations>>,
}

/// Swash's ordinary cache key intentionally has no variation coordinates.
/// Keep variable instances in a separate cache so different axis tuples can
/// never share an outline and repeated paint remains O(1) after first raster.
pub(crate) struct VariableSwashCache {
    pub(crate) context: ScaleContext,
    pub(crate) images: HashMap<VariableCacheKey, Option<SwashImage>>,
    /// Canonical coordinates supported by the face actually selected for a
    /// glyph. This is deliberately separate from authored intent: unsupported
    /// axes and values that clamp to the same endpoint share raster entries.
    pub(crate) instances: HashMap<VariationIntentKey, Option<Arc<FontVariations>>>,
}

impl VariableSwashCache {
    pub(crate) fn new() -> Self {
        Self {
            context: ScaleContext::new(),
            images: HashMap::new(),
            instances: HashMap::new(),
        }
    }

    pub(crate) fn effective_variations(
        &mut self,
        font_system: &mut FontSystem,
        font_id: cosmic_text::fontdb::ID,
        weight: Option<f32>,
        optical_size: Option<f32>,
        italic: bool,
        explicit: Option<Arc<FontVariations>>,
    ) -> Option<Arc<FontVariations>> {
        let key = VariationIntentKey {
            font_id,
            weight_bits: weight
                .filter(|value| value.is_finite())
                .map(|value| (value + 0.0).to_bits()),
            optical_size_bits: optical_size
                .filter(|value| value.is_finite())
                .map(|value| (value + 0.0).to_bits()),
            italic,
            explicit,
        };
        if let Some(cached) = self.instances.get(&key) {
            return cached.clone();
        }

        let resolved = font_system.get_font(font_id).and_then(|font| {
            let swash_font = font.as_swash();
            let has_ital = swash_font
                .variations()
                .any(|axis| axis.tag() == swash::tag_from_bytes(b"ital"));
            let mut variations = FontVariations::new();
            for axis in swash_font.variations() {
                let tag = axis.tag();
                let explicit_value = key.explicit.as_ref().and_then(|settings| {
                    settings
                        .iter()
                        .find(|setting| swash::tag_from_bytes(setting.tag.as_bytes()) == tag)
                        .map(|setting| setting.value.0)
                });
                let automatic_value = if tag == swash::tag_from_bytes(b"wght") {
                    key.weight_bits.map(f32::from_bits)
                } else if tag == swash::tag_from_bytes(b"opsz") {
                    key.optical_size_bits.map(f32::from_bits)
                } else if italic && tag == swash::tag_from_bytes(b"ital") {
                    Some(1.0)
                } else if italic && !has_ital && tag == swash::tag_from_bytes(b"slnt") {
                    Some(-14.0)
                } else {
                    None
                };
                let Some(value) = explicit_value.or(automatic_value) else {
                    continue;
                };
                if !value.is_finite() {
                    continue;
                }
                let value = value.clamp(axis.min_value(), axis.max_value()) + 0.0;
                variations.set(VariationTag::new(&tag.to_be_bytes()), value);
            }
            (!variations.is_empty()).then(|| Arc::new(variations))
        });
        self.instances.insert(key, resolved.clone());
        resolved
    }

    pub(crate) fn with_pixels<F: FnMut(i32, i32, Color)>(
        &mut self,
        font_system: &mut FontSystem,
        cache_key: CacheKey,
        variations: Arc<FontVariations>,
        base: Color,
        mut f: F,
    ) {
        let render_variations = Arc::clone(&variations);
        let key = VariableCacheKey {
            glyph: cache_key,
            variations,
        };
        let image = self.images.entry(key).or_insert_with(|| {
            let font = font_system.get_font(cache_key.font_id)?;
            let settings = render_variations.iter().map(|variation| {
                (
                    swash::tag_from_bytes(variation.tag.as_bytes()),
                    variation.value.0,
                )
            });
            let mut scaler = self
                .context
                .builder(font.as_swash())
                .size(f32::from_bits(cache_key.font_size_bits))
                .hint(true)
                .variations(settings)
                .build();
            let offset = Vector::new(cache_key.x_bin.as_float(), cache_key.y_bin.as_float());
            Render::new(&[
                Source::ColorOutline(0),
                Source::ColorBitmap(StrikeWith::BestFit),
                Source::Outline,
            ])
            .format(Format::Alpha)
            .offset(offset)
            .transform(
                cache_key
                    .flags
                    .contains(CacheKeyFlags::FAKE_ITALIC)
                    .then(|| Transform::skew(Angle::from_degrees(14.0), Angle::from_degrees(0.0))),
            )
            .render(&mut scaler, cache_key.glyph_id)
        });
        let Some(image) = image else { return };
        let left = image.placement.left;
        let top = -image.placement.top;
        match image.content {
            SwashContent::Mask => {
                for (index, alpha) in image.data.iter().copied().enumerate() {
                    let x = index as i32 % image.placement.width as i32;
                    let y = index as i32 / image.placement.width as i32;
                    f(
                        left + x,
                        top + y,
                        Color(((alpha as u32) << 24) | base.0 & 0x00FF_FFFF),
                    );
                }
            }
            SwashContent::Color => {
                for (index, rgba) in image.data.chunks_exact(4).enumerate() {
                    let x = index as i32 % image.placement.width as i32;
                    let y = index as i32 / image.placement.width as i32;
                    f(
                        left + x,
                        top + y,
                        Color::rgba(rgba[0], rgba[1], rgba[2], rgba[3]),
                    );
                }
            }
            SwashContent::SubpixelMask => {}
        }
    }
}


pub(crate) fn create_font_system(fonts: &[WebFont], load_emoji: bool) -> (FontSystem, HashMap<String, LoadedFamily>) {
        let (db, loaded_families) = if fonts.is_empty() {
            (*base_font_database(load_emoji)).clone()
        } else if let Some(cached) = cached_web_font_database(fonts, load_emoji) {
            cached.database.clone()
        } else {
            let (mut db, mut loaded_families) = (*base_font_database(load_emoji)).clone();
            let mut declarations = Vec::new();
            for font in fonts {
                for id in db.load_font_source(cosmic_text::fontdb::Source::Binary(
                    font.data.clone(),
                )) {
                    declarations.push((id, font.family.clone(), font.weight, font.italic));
                }
            }
            for (name, mut family) in register_loaded_faces(&db, declarations) {
                loaded_families
                    .entry(name)
                    .or_insert_with(|| LoadedFamily { faces: Vec::new() })
                    .faces
                    .append(&mut family.faces);
            }
            cache_web_font_database(fonts, load_emoji, (db, loaded_families))
        };
        let font_system = FontSystem::new_with_locale_and_db("en-US".to_string(), db);
        (font_system, loaded_families)
}

#[cfg(test)]
mod ordered_family_regressions {
    use super::*;

    #[test]
    fn bundled_named_policy_is_backed_by_real_face_metadata() {
        for (name, bytes) in [(FAMILY,SANS_R),(SERIF_FAMILY,SERIF_R),(MONO_FAMILY,MONO_R),(SYSTEM_FAMILY,SYSTEM_R)] {
            let mut database = cosmic_text::fontdb::Database::new();
            let ids = database.load_font_source(cosmic_text::fontdb::Source::Binary(Arc::new(bytes)));
            assert!(ids.into_iter().any(|id| database.face(id).unwrap().families.iter()
                .any(|(family, _)| family.eq_ignore_ascii_case(name))), "{name}");
        }
    }

    #[test]
    fn missing_named_families_do_not_guess_before_ordered_fallback() {
        let (_, loaded) = create_font_system(&[], false);
        for missing in ["MissingMonoFamilyQ12", "MissingSansFamilyQ12", "MissingTimesFamilyQ12", "MissingGaramondFamilyQ12"] {
            assert!(!loaded.contains_key(&missing.to_ascii_lowercase()));
            for generic in ["serif", "sans-serif", "monospace", "system-ui"] {
                let baseline = resolve_loaded_font(Some(generic), 400, false, &loaded);
                let stack = format!("\"{missing}\", {generic}");
                let actual = resolve_loaded_font(Some(&stack), 400, false, &loaded);
                assert_eq!(actual.font_id, baseline.font_id, "{stack}");
                assert_eq!(actual.family, baseline.family, "{stack}");
            }
        }
        let selected = resolve_loaded_font(Some("MissingMonoFamilyQ12, 'Liberation Serif', monospace"), 400, false, &loaded);
        assert_eq!(selected.family.as_ref(), SERIF_FAMILY);
    }

    #[test]
    fn quoted_family_tokens_select_real_declared_bytes_without_losing_punctuation() {
        // Genuine font bytes with CSS declaration names, not fabricated metrics.
        for (name, css) in [
            ("serif", "\"serif\", monospace"),
            ("Fixture, serif", "\"Fixture, serif\", monospace"),
            ("Fixture  Spaced", "\"Fixture  Spaced\", monospace"),
            ("Fixture\"Quote", "\"Fixture\\\"Quote\", monospace"),
        ] {
            let font = WebFont { data: Arc::new(SANS_R.to_vec()), family: Some(name.into()), weight: Some((400,400)), italic: Some(false) };
            let (_, loaded) = create_font_system(&[font], false);
            let expected = select_loaded_face(loaded.get(&name.to_ascii_lowercase()).unwrap(), 400, false).unwrap();
            let selected = resolve_loaded_font(Some(css), 400, false, &loaded);
            assert_eq!(selected.font_id, expected.font_id, "{css}");
            assert_eq!(selected.family, expected.family, "{css}");
            if name == "serif" {
                let generic = resolve_loaded_font(Some("serif"), 400, false, &loaded);
                assert_eq!(generic.family.as_ref(), SERIF_FAMILY, "unquoted generic must not select the named declaration");
            }
        }
    }
}

#[cfg(test)]
#[path = "native_font_integration_tests.rs"]
mod native_font_integration_tests;
