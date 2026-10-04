//! Bounded local named-font resources shared by text consumers.
use std::{collections::VecDeque, path::PathBuf, sync::{Arc, Mutex, Weak, atomic::{AtomicUsize, Ordering}}};
use cosmic_text::fontdb;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(test)]
mod tests;
#[cfg(test)]
pub(crate) use tests::{collection as fixture_collection, fixture_provider, fixture_style_provider, fixture_style_provider_with_file_limit, fixture_cascade_provider, FixtureSignals};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Unavailable { Unsupported, TooLarge, Capacity, Busy, InvalidRequest, Io, InvalidFont }
#[derive(Debug, Clone, PartialEq, Eq)]
struct ResourceKey { path: PathBuf, device: u64, inode: u64, bytes: usize, modified: i128 }
#[derive(Clone)]
pub(crate) struct Located { key: ResourceKey, family: String, postscript: String }
#[derive(Default)]
pub(crate) struct Cascade { pub candidates: Vec<Located>, pub incomplete: bool }
pub(crate) struct FallbackRequest<'a> {
    pub family: &'a str, pub postscript: Option<&'a str>, pub weight: u16,
    pub italic: bool, pub size: f32, pub locale: &'a str,
}
trait Backend: Send + Sync {
    fn locate(&self, family: &str) -> Result<Option<Located>, Unavailable>;
    fn locate_styled(&self, family: &str, _weight: u16, _italic: bool) -> Result<Option<Located>, Unavailable> { self.locate(family) }
    /// Ordered, bounded resource descriptors. No font activation or download.
    fn cascade(&self, _request: &FallbackRequest<'_>, _limit: usize)
        -> Result<Cascade, Unavailable> { Ok(Cascade::default()) }
    fn read(&self, key: &ResourceKey) -> Result<Vec<u8>, Unavailable>;
}
#[derive(Clone, Copy)]
struct Limits { file_bytes: usize, live_bytes: usize, cache_entries: usize, live_files: usize }
impl Default for Limits {
    fn default() -> Self { Self { file_bytes: 16 << 20, live_bytes: 64 << 20, cache_entries: 128, live_files: 32 } }
}
struct Budget { bytes: AtomicUsize, files: AtomicUsize, limits: Limits }
struct Reservation { budget: Arc<Budget>, bytes: usize }
impl Reservation {
    fn acquire(budget: &Arc<Budget>, bytes: usize) -> Result<Self, Unavailable> {
        if budget.files.fetch_update(Ordering::AcqRel, Ordering::Acquire, |n|
            (n < budget.limits.live_files).then_some(n + 1)).is_err() { return Err(Unavailable::Capacity); }
        if budget.bytes.fetch_update(Ordering::AcqRel, Ordering::Acquire, |n|
            n.checked_add(bytes).filter(|sum| *sum <= budget.limits.live_bytes)).is_err() {
            budget.files.fetch_sub(1, Ordering::AcqRel); return Err(Unavailable::Capacity);
        }
        Ok(Self { budget: budget.clone(), bytes })
    }
}
impl Drop for Reservation {
    fn drop(&mut self) { self.budget.bytes.fetch_sub(self.bytes, Ordering::AcqRel); self.budget.files.fetch_sub(1, Ordering::AcqRel); }
}
/// The reservation follows every fontdb Source::Binary clone, not cache membership.
pub(crate) struct NativeBytes { data: Vec<u8>, _reservation: Reservation }
impl AsRef<[u8]> for NativeBytes { fn as_ref(&self) -> &[u8] { &self.data } }
#[derive(Debug)]
pub(crate) struct Face { pub index: u32, pub families: Vec<String>, pub postscript: String }
struct FileAsset { data: Arc<NativeBytes>, faces: Vec<Face> }
pub(crate) struct NativeAsset { file: Arc<FileAsset>, pub selected_index: u32, pub family_indices: Vec<u32> }
impl NativeAsset {
    pub fn bytes(&self) -> Arc<NativeBytes> { self.file.data.clone() }
    pub fn faces(&self) -> &[Face] { &self.file.faces }
}
pub(crate) enum Lookup { Found(Arc<NativeAsset>), Absent, Unavailable(Unavailable) }
/// Future engine-owned admitted resources. Retains IDs' resource lifetimes without caching IDs.
pub(crate) struct Admission { files: Vec<Arc<FileAsset>>, bytes: usize, byte_limit: usize, file_limit: usize }
impl Admission {
    pub fn new() -> Self { Self { files: Vec::new(), bytes: 0, byte_limit: 32 << 20, file_limit: 16 } }
    #[cfg(test)]
    pub(crate) fn reduce_file_limit(&mut self, limit: usize) { assert!((1..=16).contains(&limit)); self.file_limit = limit; }
    #[cfg(test)]
    pub(crate) fn resident_counts(&self) -> (usize, usize, usize) {
        (self.files.len(), self.bytes, self.files.iter().map(|file| file.faces.len()).sum())
    }
    pub fn can_fit_alone(&self, asset: &NativeAsset) -> bool {
        asset.bytes().as_ref().as_ref().len() <= self.byte_limit && asset.faces().len() <= 256
    }
    pub fn release(&mut self, bytes: &Arc<NativeBytes>) {
        if let Some(index) = self.files.iter().position(|file| Arc::ptr_eq(&file.data, bytes)) {
            let file = self.files.remove(index);
            self.bytes -= file.data.as_ref().as_ref().len();
        }
    }
    pub fn can_admit(&self, asset: &NativeAsset) -> Result<(), Unavailable> {
        if self.files.iter().any(|file| Arc::ptr_eq(file, &asset.file)) { return Ok(()); }
        let bytes = asset.file.data.as_ref().as_ref().len();
        if self.files.len() >= self.file_limit || bytes > self.byte_limit.saturating_sub(self.bytes)
            || self.files.iter().map(|file| file.faces.len()).sum::<usize>() + asset.file.faces.len() > 256 {
            return Err(Unavailable::Capacity);
        }
        Ok(())
    }
    pub fn admit(&mut self, asset: &NativeAsset) -> Result<(), Unavailable> {
        self.can_admit(asset)?;
        if !self.files.iter().any(|file| Arc::ptr_eq(file, &asset.file)) {
            self.bytes += asset.file.data.as_ref().as_ref().len(); self.files.push(asset.file.clone());
        }
        Ok(())
    }
}

#[derive(Clone)]
enum Cached { Found(Arc<NativeAsset>), Absent }
#[derive(Default)]
struct Cache { names: VecDeque<((String, u16, bool), Cached)>, files: Vec<(ResourceKey, Weak<FileAsset>)> }
/// Share one provider across consumers. No background tasks or process-wide initialization.
pub(crate) struct Provider { backend: Box<dyn Backend>, budget: Arc<Budget>, cache: Mutex<Cache>, in_flight: AtomicUsize }
struct Flight<'a>(&'a AtomicUsize);
impl Drop for Flight<'_> { fn drop(&mut self) { self.0.fetch_sub(1, Ordering::AcqRel); } }
fn family_key(name: &str) -> String { name.to_lowercase() }
// Cache only a successfully queried and parsed OS version. Query/format errors retry.
fn cached_platform_support(cache: &std::sync::OnceLock<bool>, query: impl FnOnce() -> Result<Vec<u8>, Unavailable>) -> Result<bool, Unavailable> {
    if let Some(supported) = cache.get() { return Ok(*supported); }
    let bytes = query()?;
    if bytes.len() > 64 { return Err(Unavailable::Io); }
    let version = bytes.strip_suffix(&[0]).ok_or(Unavailable::Io)?;
    let version = std::str::from_utf8(version).map_err(|_| Unavailable::Io)?;
    let mut parts = version.split('.');
    let major = parts.next().ok_or(Unavailable::Io)?.parse::<u32>().map_err(|_| Unavailable::Io)?;
    if parts.any(|part| part.parse::<u32>().is_err()) { return Err(Unavailable::Io); }
    let supported = major >= 13;
    let _ = cache.set(supported);
    Ok(*cache.get().unwrap_or(&supported))
}
impl Provider {
    pub fn new() -> Self {
        #[cfg(target_os = "macos")]
        let backend: Box<dyn Backend> = Box::new(macos::Mac);
        #[cfg(not(target_os = "macos"))]
        let backend: Box<dyn Backend> = Box::new(Unsupported);
        Self::with_backend(backend, Limits::default())
    }
    fn with_backend(backend: Box<dyn Backend>, limits: Limits) -> Self {
        Self { backend, budget: Arc::new(Budget { bytes: AtomicUsize::new(0), files: AtomicUsize::new(0), limits }), cache: Mutex::new(Cache::default()), in_flight: AtomicUsize::new(0) }
    }
    pub fn lookup(&self, family: &str) -> Lookup { self.lookup_styled(family, 400, false) }
    pub fn lookup_styled(&self, family: &str, weight: u16, italic: bool) -> Lookup {
        self.lookup_styled_reclaim(family, weight, italic, &mut || false)
    }
    pub(crate) fn lookup_styled_reclaim(&self, family: &str, weight: u16, italic: bool, reclaim: &mut dyn FnMut() -> bool) -> Lookup {
        if family.is_empty() { return Lookup::Absent; }
        if family.len() > 256 || family.contains('\0') { return Lookup::Unavailable(Unavailable::InvalidRequest); }
        // Input is one already-decoded Named token; generic CSS interpretation belongs to callers.
        let key = (family_key(family), weight, italic);
        {
            let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(i) = cache.names.iter().position(|(name, _)| name == &key) {
                let entry = cache.names.remove(i).unwrap();
                let result = match &entry.1 { Cached::Found(asset) => Lookup::Found(asset.clone()), Cached::Absent => Lookup::Absent };
                cache.names.push_back(entry); return result;
            }
        }
        if self.in_flight.fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| (n < 2).then_some(n + 1)).is_err() {
            return Lookup::Unavailable(Unavailable::Busy);
        }
        let _flight = Flight(&self.in_flight);
        match self.load(family, weight, italic, reclaim) {
            Ok(Some(asset)) => { self.remember(key, Cached::Found(asset.clone())); Lookup::Found(asset) }
            Ok(None) => { self.remember(key, Cached::Absent); Lookup::Absent }
            Err(error) => Lookup::Unavailable(error), // Transient/capacity failures are never negative entries.
        }
    }
    pub(crate) fn cascade(&self, request: &FallbackRequest<'_>, limit: usize) -> Result<Cascade, Unavailable> {
        if request.family.len() > 256 || request.family.contains('\0') || request.locale.len() > 128 || request.locale.contains('\0')
            || request.postscript.is_some_and(|name| name.len() > 256 || name.contains('\0'))
            || !request.size.is_finite() || !(0.0..=4096.0).contains(&request.size) { return Err(Unavailable::InvalidRequest); }
        if self.in_flight.fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| (n < 2).then_some(n + 1)).is_err() { return Err(Unavailable::Busy); }
        let _flight = Flight(&self.in_flight);
        self.backend.cascade(request, limit.min(128))
    }
    pub(crate) fn load_candidate(&self, located: Located, reclaim: &mut dyn FnMut() -> bool) -> Lookup {
        if self.in_flight.fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| (n < 2).then_some(n + 1)).is_err() { return Lookup::Unavailable(Unavailable::Busy); }
        let _flight = Flight(&self.in_flight);
        match self.load_located(located, reclaim) {
            Ok(asset) => Lookup::Found(asset), Err(error) => Lookup::Unavailable(error),
        }
    }
    fn remember(&self, key: (String, u16, bool), value: Cached) {
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        cache.names.retain(|(name, _)| name != &key);
        while cache.names.len() >= self.budget.limits.cache_entries && !cache.names.is_empty() { cache.names.pop_front(); }
        if self.budget.limits.cache_entries != 0 { cache.names.push_back((key, value)); }
        cache.files.retain(|(_, file)| file.strong_count() != 0);
    }
    fn load(&self, family: &str, weight: u16, italic: bool, reclaim: &mut dyn FnMut() -> bool) -> Result<Option<Arc<NativeAsset>>, Unavailable> {
        let Some(located) = self.backend.locate_styled(family, weight, italic)? else { return Ok(None); };
        if family_key(&located.family) != family_key(family) { return Ok(None); }
        self.load_located(located, reclaim).map(Some)
    }
    fn load_located(&self, located: Located, reclaim: &mut dyn FnMut() -> bool) -> Result<Arc<NativeAsset>, Unavailable> {
        let family = located.family.clone();
        if located.key.bytes == 0 { return Err(Unavailable::InvalidFont); }
        if located.key.bytes > self.budget.limits.file_bytes { return Err(Unavailable::TooLarge); }
        let existing = {
            let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            cache.files.retain(|(_, file)| file.strong_count() != 0);
            cache.files.iter().find(|(key, _)| key == &located.key).and_then(|(_, file)| file.upgrade())
        };
        let file = if let Some(file) = existing { file } else {
            let reservation = loop {
                match Reservation::acquire(&self.budget, located.key.bytes) {
                    Ok(reservation) => break reservation,
                    Err(error) => {
                        // Only evict cache references. Live consumer reservations cannot be reclaimed.
                        let cached = { self.cache.lock().unwrap_or_else(|e| e.into_inner()).names.pop_front() };
                        if cached.is_some() { drop(cached); continue; }
                        // No cache mutex, reservation or parser FontSystem borrow
                        // survives here. Keep this exact Located candidate while
                        // the consumer releases only its completed-call caches.
                        if !reclaim() { return Err(error); }
                    }
                }
            };
            let data = self.backend.read(&located.key)?;
            if data.len() != located.key.bytes { return Err(Unavailable::Io); }
            preflight(&data)?;
            let data = Arc::new(NativeBytes { data, _reservation: reservation });
            let faces = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let mut db = fontdb::Database::new();
                db.load_font_source(fontdb::Source::Binary(data.clone()));
                let faces = db.faces().map(|face| Face { index: face.index, families: face.families.iter().map(|(name, _)| name.clone()).collect(), postscript: face.post_script_name.clone() }).collect::<Vec<_>>();
                faces
            })).map_err(|_| Unavailable::InvalidFont)?;
            if faces.is_empty() { return Err(Unavailable::InvalidFont); }
            let parsed = Arc::new(FileAsset { data, faces });
            let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            cache.files.retain(|(_, file)| file.strong_count() != 0);
            // Concurrent misses may read twice, but published resources share one byte owner.
            if let Some(file) = cache.files.iter().find(|(key, _)| key == &located.key).and_then(|(_, file)| file.upgrade()) { file }
            else { cache.files.push((located.key, Arc::downgrade(&parsed))); parsed }
        };
        let matching: Vec<_> = file.faces.iter().filter(|face| face.families.iter().any(|name| family_key(name) == family_key(&family))).collect();
        let exact: Vec<_> = matching.iter().filter(|face| face.postscript == located.postscript).collect();
        if exact.len() != 1 { return Err(Unavailable::InvalidFont); }
        Ok(Arc::new(NativeAsset { selected_index: exact[0].index, family_indices: matching.iter().map(|face| face.index).collect(), file }))
    }
}
#[cfg(not(target_os = "macos"))]
struct Unsupported;
#[cfg(not(target_os = "macos"))]
impl Backend for Unsupported {
    fn locate(&self, _: &str) -> Result<Option<Located>, Unavailable> { Err(Unavailable::Unsupported) }
    fn read(&self, _: &ResourceKey) -> Result<Vec<u8>, Unavailable> { Err(Unavailable::Unsupported) }
}

// Bound work before fontdb allocates family strings across every TTC face.
fn preflight(data: &[u8]) -> Result<(), Unavailable> {
    fn n(data: &[u8], offset: usize, width: usize) -> Result<usize, Unavailable> {
        let bytes = data.get(offset..offset.checked_add(width).ok_or(Unavailable::InvalidFont)?).ok_or(Unavailable::InvalidFont)?;
        Ok(bytes.iter().fold(0usize, |n, byte| (n << 8) | usize::from(*byte)))
    }
    if data.len() > 16 << 20 { return Err(Unavailable::TooLarge); }
    let ttc = data.get(..4) == Some(b"ttcf");
    let count = if ttc { n(data, 8, 4)? } else { 1 };
    if count == 0 { return Err(Unavailable::InvalidFont); }
    if count > 256 { return Err(Unavailable::TooLarge); }
    let mut name_work = 0usize;
    let mut record_work = 0usize;
    for index in 0..count {
        let offset = if ttc { n(data, 12 + index * 4, 4)? } else { 0 };
        if data.len() < 12 || offset > data.len() - 12 { return Err(Unavailable::InvalidFont); }
        let tables = n(data, offset + 4, 2)?;
        if tables > 256 { return Err(Unavailable::TooLarge); }
        for table in 0..tables {
            let at = offset + 12 + table * 16;
            let tag = n(data, at, 4)?;
            let start = n(data, at + 8, 4)?;
            let size = n(data, at + 12, 4)?;
            let bytes = data.get(start..start.checked_add(size).ok_or(Unavailable::InvalidFont)?).ok_or(Unavailable::InvalidFont)?;
            if tag != 0x6e616d65 { continue; }
            let records = n(bytes, 2, 2)?;
            record_work += records;
            if record_work > 4096 { return Err(Unavailable::TooLarge); }
            for record in 0..records { name_work += n(bytes, 6 + record * 12 + 8, 2)?; }
            if name_work > 1024 * 1024 { return Err(Unavailable::TooLarge); }
        }
    }
    Ok(())
}

/// Metrics of already admitted physical bytes, never another family lookup.
pub(crate) fn canvas_instance_metrics(parsed: &crate::canvas_font_geometry::ParsedCanvasFont<'_>,
    size: f32, axes: Option<&cosmic_text::FontVariations>)
    -> Result<crate::canvas_font_geometry::InstanceMetrics, Unavailable> {
    #[cfg(test)]
    if CANVAS_METRIC_FAILURE.with(|failure| failure.get()) { return Err(Unavailable::Io); }
    #[cfg(target_os = "macos")] { macos::canvas_instance_metrics(parsed, size, axes) }
    #[cfg(not(target_os = "macos"))] { let _ = (parsed, size, axes); Err(Unavailable::Unsupported) }
}

#[cfg(test)]
thread_local! {
    static CANVAS_METRIC_FAILURE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
/// Fail the metric adapter itself, independently of native family resolution.
#[cfg(test)]
pub(crate) fn with_canvas_metric_failure_for_test<T>(f: impl FnOnce() -> T) -> T {
    struct Reset(bool);
    impl Drop for Reset {
        fn drop(&mut self) { CANVAS_METRIC_FAILURE.with(|failure| failure.set(self.0)); }
    }
    let _reset = Reset(CANVAS_METRIC_FAILURE.with(|failure| failure.replace(true)));
    f()
}
