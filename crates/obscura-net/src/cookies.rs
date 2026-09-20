use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;
use url::Url;

const DEFAULT_SAME_SITE: &str = "Lax";
const COOKIE_STORE_VERSION: u32 = 1;

/// SameSite is case-insensitive per RFC 6265bis; normalize a present value to
/// title-case so stored cookies compare equal regardless of how they were sent.
/// Unrecognized values fall back to Lax per spec.
fn normalize_same_site(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "strict" => "Strict",
        "none" => "None",
        _ => "Lax",
    }
    .to_string()
}

/// The jar key for a domain. RFC 6265 4.1.2.3 ignores a leading dot, and hosts are
/// case-insensitive, so both spellings have to collapse before they reach the map.
/// Not intended for `domain_matches`, which compares without allocating on the
/// per-request path.
pub fn canonical_domain(domain: &str) -> String {
    domain.trim().trim_start_matches('.').to_lowercase()
}

pub struct CookieJar {
    /// domain -> (name, path) -> entry. RFC 6265 §5.3 identifies a cookie by
    /// (name, domain, path); the outer map scopes by domain and the inner key
    /// carries name+path so same-name cookies on different paths coexist
    /// instead of clobbering each other.
    cookies: RwLock<HashMap<String, HashMap<(String, String), CookieEntry>>>,
    /// Allocated while holding the cookies write lock, shared across domains.
    next_creation_order: AtomicU64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct CookieEntry {
    name: String,
    value: String,
    path: String,
    domain: String,
    /// Cookies set without a Domain attribute are host-only: sent to the exact
    /// origin host and never to subdomains.
    host_only: bool,
    secure: bool,
    http_only: bool,
    expires: Option<u64>,
    same_site: String,
    /// Zero means an older version 1 file did not record creation order.
    #[serde(default)]
    creation_order: u64,
}

type CookieKey = (String, String, String);

/// An opaque, lossless copy of the effective cookies in a jar. Unlike
/// `CookieInfo`, this retains internal matching state such as `host_only`.
#[derive(Debug, Clone)]
pub struct CookieSnapshot {
    entries: HashMap<CookieKey, CookieEntry>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct PersistedCookieStore {
    version: u32,
    cookies: Vec<CookieEntry>,
}

#[derive(serde::Deserialize)]
#[serde(untagged)]
enum PersistedCookieFile {
    Versioned(PersistedCookieStore),
    /// Before version 1, `cookies.json` was a bare CDP `CookieInfo` array. It
    /// did not record host-only state, so these cookies retain the historical
    /// domain-scoped import semantics.
    Legacy(Vec<CookieInfo>),
}

fn cookie_key(entry: &CookieEntry) -> CookieKey {
    (
        entry.domain.clone(),
        entry.name.clone(),
        entry.path.clone(),
    )
}

/// RFC 6265 section 5.4: longer paths first, then earlier creation time.
fn serialize_cookie_header(mut entries: Vec<&CookieEntry>) -> String {
    entries.sort_by(|a, b| {
        b.path.len().cmp(&a.path.len())
            .then_with(|| a.creation_order.cmp(&b.creation_order))
    });
    entries.into_iter()
        .map(|entry| format!("{}={}", entry.name, entry.value))
        .collect::<Vec<_>>()
        .join("; ")
}

fn unix_time_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn cookie_is_expired(entry: &CookieEntry, now: u64) -> bool {
    entry.expires.is_some_and(|expires| expires <= now)
}

fn parse_max_age(value: &str) -> Option<i64> {
    let value = value.trim();
    let digits = value.strip_prefix('-').unwrap_or(value);
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}

impl CookieJar {
    pub fn new() -> Self {
        CookieJar {
            cookies: RwLock::new(HashMap::new()),
            next_creation_order: AtomicU64::new(1),
        }
    }

    pub fn set_cookie(&self, set_cookie_str: &str, url: &Url) {
        let parts: Vec<&str> = set_cookie_str.splitn(2, ';').collect();
        let name_value = parts[0].trim();
        let (name, value) = match name_value.split_once('=') {
            Some((n, v)) => (n.trim().to_string(), v.trim().to_string()),
            None => return,
        };

        let request_host = url.host_str().unwrap_or("").to_lowercase();
        let mut domain_attr: Option<String> = None;
        let mut path = default_cookie_path(url.path());
        let mut secure = false;
        let mut http_only = false;
        let mut expires: Option<u64> = None;
        let mut max_age: Option<i64> = None;
        let mut same_site = "Lax".to_string();

        if parts.len() > 1 {
            for attr in parts[1].split(';') {
                let attr = attr.trim();
                if let Some((key, val)) = attr.split_once('=') {
                    match key.trim().to_lowercase().as_str() {
                        "domain" => {
                            domain_attr = Some(canonical_domain(val));
                        }
                        "path" => {
                            path = val.trim().to_string();
                        }
                        "expires" => {
                            if let Ok(ts) = parse_http_date(val.trim()) {
                                expires = Some(ts);
                            }
                        }
                        "max-age" => {
                            if let Some(secs) = parse_max_age(val) {
                                max_age = Some(secs);
                            }
                        }
                        "samesite" => {
                            same_site = normalize_same_site(val);
                        }
                        _ => {}
                    }
                } else {
                    match attr.to_lowercase().as_str() {
                        "secure" => secure = true,
                        "httponly" => http_only = true,
                        _ => {}
                    }
                }
            }
        }

        if let Some(secs) = max_age {
            expires = Some(if secs <= 0 {
                0
            } else {
                unix_time_secs().saturating_add(secs as u64)
            });
        }

        // Validate Domain against the response origin (RFC 6265): an unrelated
        // or public-suffix Domain is ignored so a response from attacker.test
        // cannot scope a cookie to victim.test (GHSA-f22c-8v6q-v6h6).
        let (domain, host_only) = match resolve_cookie_domain(&request_host, domain_attr.as_deref()) {
            Some(d) => d,
            None => return,
        };

        if let Some(exp) = expires {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            if exp <= now {
                let mut cookies = self.cookies.write().unwrap();
                if let Some(domain_cookies) = cookies.get_mut(&domain) {
                    domain_cookies.remove(&(name.clone(), path.clone()));
                }
                return;
            }
        }

        let entry = CookieEntry {
            name: name.clone(),
            value,
            path: path.clone(),
            domain: domain.clone(),
            host_only,
            secure,
            http_only,
            expires,
            same_site,
            creation_order: 0,
        };

        let mut cookies = self.cookies.write().unwrap();
        self.insert_entry(&mut cookies, entry);
    }

    pub fn get_cookie_header(&self, url: &Url) -> String {
        let host = url.host_str().unwrap_or("");
        let path = url.path();
        let is_secure = url.scheme() == "https";
        let cookies = self.cookies.read().unwrap();

        let mut matching: Vec<&CookieEntry> = Vec::new();

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        for (domain, domain_cookies) in cookies.iter() {
            if !domain_matches(host, domain) {
                continue;
            }
            for entry in domain_cookies.values() {
                if entry.host_only && !host.eq_ignore_ascii_case(domain) {
                    continue;
                }
                if let Some(exp) = entry.expires {
                    if exp <= now {
                        continue;
                    }
                }
                if entry.secure && !is_secure {
                    continue;
                }
                if !path_matches(path, &entry.path) {
                    continue;
                }
                matching.push(entry);
            }
        }

        serialize_cookie_header(matching)
    }

    pub fn get_all_cookies(&self) -> Vec<CookieInfo> {
        let cookies = self.cookies.read().unwrap();
        let now = unix_time_secs();
        let mut entries: Vec<_> = cookies.values()
            .flat_map(|domain_cookies| domain_cookies.values())
            .filter(|entry| !cookie_is_expired(entry, now))
            .collect();
        // CookieInfo has no creation metadata, so preserve it in array order
        // for callers that later import this projection through CDP.
        entries.sort_by_key(|entry| entry.creation_order);
        entries.into_iter().map(|entry| CookieInfo {
            name: entry.name.clone(),
            value: entry.value.clone(),
            domain: entry.domain.clone(),
            path: entry.path.clone(),
            secure: entry.secure,
            http_only: entry.http_only,
            same_site: entry.same_site.clone(),
            expires: entry.expires.map(|e| e as i64),
        }).collect()
    }

    /// Capture every stored entry without projecting it through the lossy CDP
    /// cookie representation. Expired entries remain in snapshots so elapsed
    /// time cannot be mistaken for an explicit deletion when computing a
    /// connection delta. Consumers that materialize state filter them.
    pub fn snapshot(&self) -> CookieSnapshot {
        let cookies = self.cookies.read().unwrap();
        let mut entries = HashMap::new();
        for domain_cookies in cookies.values() {
            for entry in domain_cookies.values() {
                entries.insert(cookie_key(entry), entry.clone());
            }
        }
        CookieSnapshot { entries }
    }

    /// Create an independent jar from a snapshot, preserving every field of
    /// each non-expired entry.
    pub fn from_snapshot(snapshot: &CookieSnapshot) -> Self {
        let jar = Self::new();
        jar.merge_entries(snapshot.entries.values().cloned());
        jar
    }

    /// Apply the changes between two snapshots without overwriting destination
    /// entries that were unchanged by the snapshot owner. Deletions and
    /// replacements remain explicit, matching the connection persistence merge
    /// contract.
    pub fn apply_snapshot_delta(
        &self,
        initial: &CookieSnapshot,
        current: &CookieSnapshot,
    ) {
        let now = unix_time_secs();
        let mut jar = self.cookies.write().unwrap();

        for (key, entry) in &initial.entries {
            if !current.entries.contains_key(key) {
                if let Some(domain_cookies) = jar.get_mut(&entry.domain) {
                    domain_cookies.remove(&(entry.name.clone(), entry.path.clone()));
                }
            }
        }

        let mut changes: Vec<_> = current.entries.iter()
            .filter(|(key, entry)| {
                !cookie_is_expired(entry, now) && initial.entries.get(*key) != Some(*entry)
            })
            .map(|(_, entry)| entry.clone())
            .collect();
        changes.sort_by_key(|entry| entry.creation_order);
        for entry in changes {
            // A changed creation order means the source deleted/expired and
            // recreated this key, rather than replacing its value in place.
            if initial.entries.get(&cookie_key(&entry))
                .is_some_and(|previous| previous.creation_order != entry.creation_order)
            {
                if let Some(domain_cookies) = jar.get_mut(&entry.domain) {
                    domain_cookies.remove(&(entry.name.clone(), entry.path.clone()));
                }
            }
            self.insert_entry(&mut jar, entry);
        }
    }

    fn merge_entries(&self, entries: impl IntoIterator<Item = CookieEntry>) {
        let now = unix_time_secs();
        let mut jar = self.cookies.write().unwrap();
        let mut entries: Vec<_> = entries.into_iter().collect();
        // Stable sorting gives old files without metadata their array order.
        entries.sort_by_key(|entry| entry.creation_order);
        for mut entry in entries {
            let domain = canonical_domain(&entry.domain);
            entry.domain = domain.clone();
            let key = (entry.name.clone(), entry.path.clone());
            if cookie_is_expired(&entry, now) {
                // Snapshots retain expired entries. Do not reuse their order
                // when a copied jar recreates one of these keys.
                self.next_creation_order.fetch_max(
                    entry.creation_order.saturating_add(1), Ordering::Relaxed,
                );
                if let Some(domain_cookies) = jar.get_mut(&domain) {
                    domain_cookies.remove(&key);
                }
                continue;
            }
            self.insert_entry(&mut jar, entry);
        }
    }

    /// A replacement keeps its original position. Imported creation orders are
    /// retained when possible; new entries from another jar follow destination
    /// entries, even when their independently allocated numbers overlap.
    fn insert_entry(
        &self,
        cookies: &mut HashMap<String, HashMap<(String, String), CookieEntry>>,
        mut entry: CookieEntry,
    ) {
        let key = (entry.name.clone(), entry.path.clone());
        let domain_cookies = cookies.entry(entry.domain.clone()).or_default();
        if let Some(previous) = domain_cookies.get(&key)
            .filter(|previous| !cookie_is_expired(previous, unix_time_secs()))
        {
            entry.creation_order = previous.creation_order;
        } else {
            let order = self.next_creation_order.load(Ordering::Relaxed)
                .max(entry.creation_order);
            entry.creation_order = order;
            self.next_creation_order.store(order.saturating_add(1), Ordering::Relaxed);
        }
        domain_cookies.insert(key, entry);
    }

    pub fn set_cookies_from_cdp(&self, cookies: Vec<CookieInfo>) {
        let mut jar = self.cookies.write().unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        for cookie in cookies {
            // RFC 6265 4.1.2.3: the leading dot is ignored. The Set-Cookie path already strips
            // it, but this code did not, which is why one cookie became two entries.
            let domain = canonical_domain(&cookie.domain);
            if cookie.expires.is_some_and(|expires| {
                expires == 0 || (expires > 0 && expires <= now)
            }) {
                if let Some(domain_cookies) = jar.get_mut(&domain) {
                    domain_cookies.retain(|_key, entry| {
                        entry.name != cookie.name || entry.path != cookie.path
                    });
                }
                continue;
            }
            let same_site = if cookie.same_site.is_empty() {
                DEFAULT_SAME_SITE.to_string()
            } else {
                normalize_same_site(&cookie.same_site)
            };
            let expires = cookie.expires.and_then(|e| if e > 0 { Some(e as u64) } else { None });
            let entry = CookieEntry {
                name: cookie.name.clone(),
                value: cookie.value,
                path: cookie.path.clone(),
                domain: domain.clone(),
                // CDP/persisted import is trusted; honor the explicit domain as
                // domain-scoped (matches the prior behavior).
                host_only: false,
                secure: cookie.secure,
                http_only: cookie.http_only,
                expires,
                same_site,
                creation_order: 0,
            };
            self.insert_entry(&mut jar, entry);
        }
    }

    pub fn get_js_visible_cookies(&self, url: &Url) -> String {
        let host = url.host_str().unwrap_or("");
        let path = url.path();
        let is_secure = url.scheme() == "https";
        let cookies = self.cookies.read().unwrap();

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let mut matching: Vec<&CookieEntry> = Vec::new();

        for (domain, domain_cookies) in cookies.iter() {
            if !domain_matches(host, domain) {
                continue;
            }
            for entry in domain_cookies.values() {
                if entry.host_only && !host.eq_ignore_ascii_case(domain) {
                    continue;
                }
                if entry.http_only {
                    continue;
                }
                if let Some(exp) = entry.expires {
                    if exp <= now {
                        continue;
                    }
                }
                if entry.secure && !is_secure {
                    continue;
                }
                if !path_matches(path, &entry.path) {
                    continue;
                }
                matching.push(entry);
            }
        }

        serialize_cookie_header(matching)
    }

    pub fn set_cookie_from_js(&self, cookie_str: &str, url: &Url) {
        let parts: Vec<&str> = cookie_str.splitn(2, ';').collect();
        let name_value = parts[0].trim();
        let (name, value) = match name_value.split_once('=') {
            Some((n, v)) => (n.trim().to_string(), v.trim().to_string()),
            None => return,
        };

        let request_host = url.host_str().unwrap_or("").to_lowercase();
        let mut domain_attr: Option<String> = None;
        let mut path = default_cookie_path(url.path());
        let mut secure = false;
        let mut expires: Option<u64> = None;
        let mut max_age: Option<i64> = None;
        let mut same_site = "Lax".to_string();

        if parts.len() > 1 {
            for attr in parts[1].split(';') {
                let attr = attr.trim();
                if let Some((key, val)) = attr.split_once('=') {
                    match key.trim().to_lowercase().as_str() {
                        "domain" => {
                            domain_attr = Some(canonical_domain(val));
                        }
                        "path" => {
                            path = val.trim().to_string();
                        }
                        "expires" => {
                            if let Ok(ts) = parse_http_date(val.trim()) {
                                expires = Some(ts);
                            }
                        }
                        "max-age" => {
                            if let Some(secs) = parse_max_age(val) {
                                max_age = Some(secs);
                            }
                        }
                        "samesite" => {
                            same_site = normalize_same_site(val);
                        }
                        _ => {}
                    }
                } else {
                    match attr.to_lowercase().as_str() {
                        "secure" => secure = true,
                        _ => {}
                    }
                }
            }
        }

        if let Some(secs) = max_age {
            expires = Some(if secs <= 0 {
                0
            } else {
                unix_time_secs().saturating_add(secs as u64)
            });
        }

        let (domain, host_only) = match resolve_cookie_domain(&request_host, domain_attr.as_deref()) {
            Some(d) => d,
            None => return,
        };

        if let Some(exp) = expires {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            if exp <= now {
                let mut cookies = self.cookies.write().unwrap();
                if let Some(domain_cookies) = cookies.get_mut(&domain) {
                    // RFC 6265 §5.3: a non-HTTP API (document.cookie) must not
                    // delete an existing HttpOnly cookie.
                    let key = (name.clone(), path.clone());
                    if domain_cookies.get(&key).is_some_and(|e| e.http_only) {
                        return;
                    }
                    domain_cookies.remove(&key);
                }
                return;
            }
        }

        let entry = CookieEntry {
            name: name.clone(),
            value,
            path: path.clone(),
            domain: domain.clone(),
            host_only,
            secure,
            http_only: false,
            expires,
            same_site,
            creation_order: 0,
        };

        let mut cookies = self.cookies.write().unwrap();
        let domain_cookies = cookies.entry(domain).or_default();
        // RFC 6265 §5.3: a non-HTTP API (document.cookie) must not overwrite an
        // existing HttpOnly cookie set by the server.
        if domain_cookies
            .get(&(name.clone(), path.clone()))
            .is_some_and(|e| e.http_only)
        {
            return;
        }
        self.insert_entry(&mut cookies, entry);
    }

    pub fn delete_cookie(&self, name: &str, domain: &str) {
        let mut cookies = self.cookies.write().unwrap();
        if domain.is_empty() {
            for domain_cookies in cookies.values_mut() {
                domain_cookies.retain(|_k, e| e.name != name);
            }
        } else if let Some(domain_cookies) = cookies.get_mut(canonical_domain(domain).as_str()) {
            domain_cookies.retain(|_k, e| e.name != name);
        }
    }

    pub fn delete_cookies_filtered(&self, name: &str, domain: &str, path: Option<&str>) {
        let mut cookies = self.cookies.write().unwrap();
        let matches_path = |entry_path: &str| match path {
            Some(p) => entry_path == p,
            None => true,
        };
        if domain.is_empty() {
            for domain_cookies in cookies.values_mut() {
                domain_cookies.retain(|_k, e| !(e.name == name && matches_path(&e.path)));
            }
        } else if let Some(domain_cookies) = cookies.get_mut(canonical_domain(domain).as_str()) {
            domain_cookies.retain(|_k, e| !(e.name == name && matches_path(&e.path)));
        }
    }

    pub fn clear(&self) {
        self.cookies.write().unwrap().clear();
    }

    /// Serialize all non-expired cookies to a JSON file.
    /// Writes atomically via tempfile then rename.
    pub fn save_to_file(&self, path: &std::path::Path) -> Result<(), std::io::Error> {
        use std::io::Write;

        let snapshot = self.snapshot();
        let now = unix_time_secs();
        let mut store = PersistedCookieStore {
            version: COOKIE_STORE_VERSION,
            cookies: snapshot
                .entries
                .into_values()
                .filter(|entry| !cookie_is_expired(entry, now))
                .collect(),
        };
        store.cookies.sort_by_key(|entry| entry.creation_order);
        let json = serde_json::to_string_pretty(&store).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, e)
        })?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut tmp = tempfile::NamedTempFile::new_in(
            path.parent().unwrap_or(std::path::Path::new(".")),
        )?;
        tmp.write_all(json.as_bytes())?;
        tmp.persist(path).map_err(|e| e.error)?;
        Ok(())
    }

    /// Load cookies from a JSON file into the jar.
    /// Merges with existing cookies (does not clear).
    /// Returns the number of cookies loaded.
    pub fn load_from_file(&self, path: &std::path::Path) -> Result<usize, std::io::Error> {
        if !path.exists() {
            return Ok(0);
        }
        let data = std::fs::read_to_string(path)?;
        let file: PersistedCookieFile = serde_json::from_str(&data).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, e)
        })?;
        match file {
            PersistedCookieFile::Versioned(store) => {
                if store.version != COOKIE_STORE_VERSION {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("unsupported cookie store version {}", store.version),
                    ));
                }
                let count = store.cookies.len();
                self.merge_entries(store.cookies);
                Ok(count)
            }
            PersistedCookieFile::Legacy(cookies) => {
                let count = cookies.len();
                self.set_cookies_from_cdp(cookies);
                Ok(count)
            }
        }
    }
}

impl Default for CookieJar {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CookieInfo {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    pub secure: bool,
    #[serde(rename = "httpOnly")]
    pub http_only: bool,
    #[serde(default, rename = "sameSite")]
    pub same_site: String,
    #[serde(default)]
    pub expires: Option<i64>,
}

fn parse_http_date(s: &str) -> Result<u64, ()> {
    let months = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];

    let s = s.replace('-', " ");
    let parts: Vec<&str> = s.split_whitespace().collect();

    if parts.len() < 5 { return Err(()); }

    let day: u64 = parts[1].parse().map_err(|_| ())?;
    let month = months.iter().position(|m| parts[2].to_lowercase().starts_with(m))
        .ok_or(())? as u64 + 1;
    let year: u64 = parts[3].parse().map_err(|_| ())?;

    let time_parts: Vec<&str> = parts[4].split(':').collect();
    let hour: u64 = time_parts.first().and_then(|s| s.parse().ok()).unwrap_or(0);
    let minute: u64 = time_parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let second: u64 = time_parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);

    let mut days_total: u64 = 0;
    for y in 1970..year {
        days_total += if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) { 366 } else { 365 };
    }
    let days_in_month = [0, 31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let is_leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    for m in 1..month {
        days_total += days_in_month[m as usize] + if m == 2 && is_leap { 1 } else { 0 };
    }
    days_total += day - 1;

    Ok(days_total * 86400 + hour * 3600 + minute * 60 + second)
}

/// Resolve the storage domain and host-only flag for a cookie being set from
/// `origin_host` (RFC 6265 §5.2/§5.3). With no Domain attribute the cookie is
/// host-only: scoped to the exact origin host. A Domain attribute is honored
/// only when it domain-matches the origin (equal to it or a parent domain) and
/// is not an obvious public suffix; otherwise the attribute is ignored and the
/// cookie is stored host-only on the origin. This is what stops a response from
/// attacker.test planting a cookie scoped to victim.test.
///
/// Returns None only when the origin host itself is absent (the cookie cannot
/// be scoped and is dropped).
///
/// Note: a full public suffix list is not bundled, so multi-label public
/// suffixes (co.uk, github.io) are not rejected; the domain-match check still
/// blocks the reported cross-domain attack, and single-label suffixes (com,
/// local) are rejected.
fn resolve_cookie_domain(origin_host: &str, domain_attr: Option<&str>) -> Option<(String, bool)> {
    let origin = canonical_domain(origin_host);
    if origin.is_empty() {
        return None;
    }
    let dom = match domain_attr {
        None => return Some((origin, true)),
        Some(raw) => canonical_domain(raw),
    };
    if dom.is_empty() || dom == origin {
        return Some((origin, true));
    }
    if dom.contains('.') && origin.ends_with(&format!(".{dom}")) {
        Some((dom, false))
    } else {
        Some((origin, true))
    }
}

// RFC 6265 5.1.4 default-path: the path a cookie is scoped to when its
// Set-Cookie carries no Path attribute. It is the request URI's directory — the
// path up to but not including the right-most '/' — NOT the full request path.
// Using the full path scopes a session cookie to the exact URL that set it: a
// cookie set on `/app/login` would then not match `/app/dashboard`, silently
// logging the user out on the next navigation. Browsers store `/app` here.
pub fn default_cookie_path(request_path: &str) -> String {
    // "If the uri-path is empty or if the first character is not '/', output /."
    if !request_path.starts_with('/') {
        return "/".to_string();
    }
    match request_path.rfind('/') {
        // "If the uri-path contains no more than one '/', output /."
        Some(0) | None => "/".to_string(),
        // "Output the characters up to, but not including, the right-most '/'."
        Some(idx) => request_path[..idx].to_string(),
    }
}

// RFC 6265 5.1.4 path-match. A bare `starts_with` over-matches sibling paths
// that share a string prefix (a Path=/admin cookie leaking to /administrator),
// so a prefix match also requires the boundary to fall on a '/'.
fn path_matches(request_path: &str, cookie_path: &str) -> bool {
    if request_path == cookie_path {
        return true;
    }
    if !request_path.starts_with(cookie_path) {
        return false;
    }
    // Prefix match: exact when the cookie-path already ends in '/', otherwise
    // the next char of the request-path must be the '/' boundary.
    cookie_path.ends_with('/') || request_path.as_bytes().get(cookie_path.len()) == Some(&b'/')
}

fn domain_matches(host: &str, domain: &str) -> bool {
    // Avoid allocations on the hot path. Cookie lookup runs per fetch
    // (every subresource on a page) and walks every domain in the jar.
    // Previously this allocated 2 lowercase Strings + a "." prefix
    // per (host, domain) pair.
    let domain = domain.trim_start_matches('.');
    if host.len() < domain.len() {
        return false;
    }
    // Exact match (case-insensitive)
    if host.eq_ignore_ascii_case(domain) {
        return true;
    }
    // Suffix match with a '.' boundary: host = "sub.example.com",
    // domain = "example.com". The byte before the suffix in host
    // must be '.'.
    let prefix_len = host.len() - domain.len();
    if prefix_len < 1 { return false; }
    if !host.is_char_boundary(prefix_len) { return false; }
    if host.as_bytes()[prefix_len - 1] != b'.' { return false; }
    host[prefix_len..].eq_ignore_ascii_case(domain)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_set_and_get_cookie() {
        let jar = CookieJar::new();
        let url = Url::parse("https://example.com/path").unwrap();
        jar.set_cookie("session=abc123; Path=/; Secure; HttpOnly", &url);

        let header = jar.get_cookie_header(&url);
        assert!(header.contains("session=abc123"));
    }

    // RFC 6265 §5.3: document.cookie (a non-HTTP API) must not overwrite or
    // delete a server-set HttpOnly cookie. See #915.
    #[test]
    fn js_cannot_overwrite_httponly_cookie() {
        let jar = CookieJar::new();
        let url = Url::parse("https://example.com/").unwrap();
        jar.set_cookie("session=server_secret; Path=/; HttpOnly", &url);

        jar.set_cookie_from_js("session=attacker_value", &url);

        assert!(
            jar.get_cookie_header(&url).contains("session=server_secret"),
            "JS must not overwrite an HttpOnly cookie"
        );
        assert!(
            !jar.get_cookie_header(&url).contains("attacker_value"),
            "the attacker value must not be stored"
        );
    }

    #[test]
    fn js_cannot_delete_httponly_cookie() {
        let jar = CookieJar::new();
        let url = Url::parse("https://example.com/").unwrap();
        jar.set_cookie("session=server_secret; Path=/; HttpOnly", &url);

        jar.set_cookie_from_js("session=; Max-Age=0", &url);

        assert!(
            jar.get_cookie_header(&url).contains("session=server_secret"),
            "JS must not delete an HttpOnly cookie"
        );
    }

    #[test]
    fn js_can_still_overwrite_non_httponly_cookie() {
        let jar = CookieJar::new();
        let url = Url::parse("https://example.com/").unwrap();
        jar.set_cookie("pref=light; Path=/", &url);

        jar.set_cookie_from_js("pref=dark", &url);

        assert!(
            jar.get_cookie_header(&url).contains("pref=dark"),
            "JS must remain able to overwrite a non-HttpOnly cookie"
        );
    }

    #[test]
    fn test_cookie_domain_matching() {
        let jar = CookieJar::new();
        let url = Url::parse("https://www.example.com/").unwrap();
        jar.set_cookie("token=xyz; Domain=example.com", &url);

        let header = jar.get_cookie_header(&url);
        assert!(header.contains("token=xyz"));

        let sub_url = Url::parse("https://api.example.com/").unwrap();
        let header2 = jar.get_cookie_header(&sub_url);
        assert!(header2.contains("token=xyz"));

        let other_url = Url::parse("https://other.com/").unwrap();
        let header3 = jar.get_cookie_header(&other_url);
        assert!(header3.is_empty());
    }

    #[test]
    fn test_cdp_cookie_with_leading_dot_domain_matches_requests() {
        let jar = CookieJar::new();
        jar.set_cookies_from_cdp(vec![CookieInfo {
            name: "token".to_string(),
            value: "xyz".to_string(),
            domain: ".example.com".to_string(),
            path: "/".to_string(),
            secure: false,
            http_only: false,
            same_site: String::new(),
            expires: None,
        }]);

        let apex_url = Url::parse("https://example.com/").unwrap();
        let apex_header = jar.get_cookie_header(&apex_url);
        assert!(apex_header.contains("token=xyz"));

        let subdomain_url = Url::parse("https://api.example.com/").unwrap();
        let subdomain_header = jar.get_cookie_header(&subdomain_url);
        assert!(subdomain_header.contains("token=xyz"));

        let other_url = Url::parse("https://other.com/").unwrap();
        let other_header = jar.get_cookie_header(&other_url);
        assert!(other_header.is_empty());
    }

    #[test]
    fn test_secure_cookie_not_sent_over_http() {
        let jar = CookieJar::new();
        let https_url = Url::parse("https://example.com/").unwrap();
        jar.set_cookie("secure_token=secret; Secure", &https_url);

        let http_url = Url::parse("http://example.com/").unwrap();
        let header = jar.get_cookie_header(&http_url);
        assert!(header.is_empty());
    }

    #[test]
    fn test_max_age_zero_deletes_cookie() {
        let jar = CookieJar::new();
        let url = Url::parse("https://example.com/").unwrap();
        jar.set_cookie("session=abc", &url);
        assert!(jar.get_cookie_header(&url).contains("session=abc"));

        jar.set_cookie("session=abc; Max-Age=0", &url);
        assert!(jar.get_cookie_header(&url).is_empty());
    }

    #[test]
    fn test_same_name_cookies_with_different_paths_coexist() {
        // RFC 6265 §5.3: a cookie is identified by (name, domain, path). Two
        // cookies that share a name but differ in path are distinct and must
        // both be retained — storing by name alone clobbers the first.
        let jar = CookieJar::new();
        let set_url = Url::parse("https://example.com/").unwrap();
        jar.set_cookie("id=1; Path=/a", &set_url);
        jar.set_cookie("id=2; Path=/b", &set_url);

        let header_a = jar.get_cookie_header(&Url::parse("https://example.com/a/page").unwrap());
        let header_b = jar.get_cookie_header(&Url::parse("https://example.com/b/page").unwrap());
        assert!(header_a.contains("id=1"), "/a must see the Path=/a cookie, got: {header_a:?}");
        assert!(header_b.contains("id=2"), "/b must see the Path=/b cookie, got: {header_b:?}");
        assert!(!header_a.contains("id=2"), "Path=/b cookie leaked to /a: {header_a:?}");
        assert!(!header_b.contains("id=1"), "Path=/a cookie leaked to /b: {header_b:?}");
    }

    #[test]
    fn cookie_headers_sort_by_path_then_creation_across_domains() {
        let jar = CookieJar::new();
        let url = Url::parse("https://www.example.com/account/detail/page").unwrap();
        jar.set_cookie("session=root; Path=/", &url);
        jar.set_cookie("first=old; Domain=example.com; Path=/account", &url);
        jar.set_cookie("session=scoped; Path=/account", &url);
        jar.set_cookie("deep=value; Path=/account/detail", &url);
        jar.set_cookie("private=value; Path=/account; HttpOnly", &url);
        jar.set_cookie("first=updated; Domain=example.com; Path=/account", &url);

        assert_eq!(jar.get_cookie_header(&url),
            "deep=value; first=updated; session=scoped; private=value; session=root");
        assert_eq!(jar.get_js_visible_cookies(&url),
            "deep=value; first=updated; session=scoped; session=root");
    }

    #[test]
    fn replacements_keep_creation_order_across_http_js_and_cdp() {
        let jar = CookieJar::new();
        let url = Url::parse("https://example.com/account/page").unwrap();
        jar.set_cookie("first=server; Path=/account", &url);
        jar.set_cookie("second=server; Path=/account", &url);
        jar.set_cookie_from_js("first=script; Path=/account", &url);
        assert_eq!(jar.get_cookie_header(&url), "first=script; second=server");
        let mut imported = jar.get_all_cookies().into_iter()
            .find(|cookie| cookie.name == "first").unwrap();
        imported.value = "cdp".to_string();
        imported.domain = ".EXAMPLE.com".to_string();
        jar.set_cookies_from_cdp(vec![imported]);
        assert_eq!(jar.get_cookie_header(&url), "first=cdp; second=server");
        jar.set_cookie("first=updated; Path=/account", &url);
        assert_eq!(jar.get_cookie_header(&url), "first=updated; second=server");
    }

    #[test]
    fn cdp_projection_reimport_preserves_creation_order() {
        let source = CookieJar::new();
        let url = Url::parse("https://www.example.com/account/page").unwrap();
        source.set_cookie("root=value; Path=/", &url);
        source.set_cookie("first=one; Domain=example.com; Path=/account", &url);
        source.set_cookie("second=two; Path=/account", &url);
        source.set_cookie("first=updated; Domain=example.com; Path=/account", &url);
        let projected = source.get_all_cookies();
        assert_eq!(projected.iter().map(|cookie| cookie.name.as_str()).collect::<Vec<_>>(),
            vec!["root", "first", "second"]);
        let restored = CookieJar::new();
        restored.set_cookies_from_cdp(projected);
        assert_eq!(restored.get_cookie_header(&url), "first=updated; second=two; root=value");
    }

    #[test]
    fn deleted_and_expired_cookies_get_fresh_creation_order() {
        let jar = CookieJar::new();
        let url = Url::parse("https://example.com/").unwrap();
        jar.set_cookie("first=one; Path=/", &url);
        jar.set_cookie("second=two; Path=/", &url);
        jar.set_cookie("first=gone; Path=/; Max-Age=0", &url);
        jar.set_cookie("first=three; Path=/", &url);
        assert_eq!(jar.get_cookie_header(&url), "second=two; first=three");

        // Mark the stored entry expired without depending on wall-clock sleeps.
        jar.cookies.write().unwrap().get_mut("example.com").unwrap()
            .get_mut(&("second".to_string(), "/".to_string())).unwrap().expires = Some(0);
        jar.set_cookie_from_js("second=four; Path=/", &url);
        assert_eq!(jar.get_cookie_header(&url), "first=three; second=four");
    }

    #[test]
    fn test_same_name_same_path_cookie_is_replaced() {
        // Same (name, path): the newer value replaces the older one — still
        // one entry, not two.
        let jar = CookieJar::new();
        let url = Url::parse("https://example.com/a/x").unwrap();
        jar.set_cookie("id=1; Path=/a", &url);
        jar.set_cookie("id=2; Path=/a", &url);
        let header = jar.get_cookie_header(&url);
        assert!(header.contains("id=2"), "newer value must win: {header:?}");
        assert!(!header.contains("id=1"), "old value must be replaced: {header:?}");
    }

    #[test]
    fn test_max_age_zero_deletes_only_matching_path() {
        // A Max-Age=0 Set-Cookie deletes the (name, path) it targets, leaving a
        // same-name cookie on a different path intact.
        let jar = CookieJar::new();
        let set_url = Url::parse("https://example.com/").unwrap();
        jar.set_cookie("id=1; Path=/a", &set_url);
        jar.set_cookie("id=2; Path=/b", &set_url);
        jar.set_cookie("id=x; Path=/a; Max-Age=0", &set_url);

        let header_a = jar.get_cookie_header(&Url::parse("https://example.com/a/page").unwrap());
        let header_b = jar.get_cookie_header(&Url::parse("https://example.com/b/page").unwrap());
        assert!(header_a.is_empty(), "Path=/a cookie should be deleted: {header_a:?}");
        assert!(header_b.contains("id=2"), "Path=/b cookie must survive: {header_b:?}");
    }

    #[test]
    fn test_max_age_sets_expiry() {
        let jar = CookieJar::new();
        let url = Url::parse("https://example.com/").unwrap();
        jar.set_cookie("token=xyz; Max-Age=3600", &url);
        assert!(jar.get_cookie_header(&url).contains("token=xyz"));
    }

    #[test]
    fn max_age_precedes_expires_independent_of_attribute_order() {
        let url = Url::parse("https://example.com/").unwrap();
        let setters: [fn(&CookieJar, &str, &Url); 2] = [
            CookieJar::set_cookie,
            CookieJar::set_cookie_from_js,
        ];
        let past = "Expires=Thu, 01 Jan 2020 00:00:00 GMT";
        let future = "Expires=Thu, 01 Jan 2099 00:00:00 GMT";

        for set in setters {
            for attributes in [
                format!("Max-Age=3600; {past}"),
                format!("{past}; Max-Age=3600"),
            ] {
                let jar = CookieJar::new();
                set(&jar, &format!("kept=value; {attributes}"), &url);
                assert!(
                    jar.get_cookie_header(&url).contains("kept=value"),
                    "valid Max-Age must override Expires for {attributes:?}"
                );
            }

            let jar = CookieJar::new();
            let before = unix_time_secs();
            set(
                &jar,
                &format!("kept=value; Max-Age=3600; Max-Age=+7200; {past}"),
                &url,
            );
            let after = unix_time_secs();
            let expires = jar.get_all_cookies()[0].expires.unwrap() as u64;
            assert!(
                (before + 3600..=after + 3600).contains(&expires),
                "an invalid later Max-Age must not replace the last valid value: {expires}"
            );

            let jar = CookieJar::new();
            set(&jar, &format!("gone=value; {past}; Max-Age=invalid"), &url);
            assert!(
                !jar.get_cookie_header(&url).contains("gone="),
                "invalid Max-Age must fall back to Expires"
            );

            for attributes in [
                format!("Max-Age=0; {future}"),
                format!("{future}; Max-Age=-1"),
            ] {
                let jar = CookieJar::new();
                set(&jar, "delete=current; Path=/", &url);
                set(&jar, &format!("delete=gone; Path=/; {attributes}"), &url);
                assert!(
                    !jar.get_cookie_header(&url).contains("delete="),
                    "non-positive Max-Age must delete for {attributes:?}"
                );
            }
        }
    }

    #[test]
    fn test_expired_cookie_not_sent() {
        let jar = CookieJar::new();
        let url = Url::parse("https://example.com/").unwrap();
        jar.set_cookie("old=current", &url);
        jar.set_cookie("old=gone; Expires=Thu, 01 Jan 2020 00:00:00 GMT", &url);
        assert!(jar.get_cookie_header(&url).is_empty());
        assert!(jar.get_all_cookies().is_empty());
    }

    #[test]
    fn test_expired_js_cookie_deletes_existing_cookie() {
        let jar = CookieJar::new();
        let url = Url::parse("https://example.com/").unwrap();
        jar.set_cookie_from_js("old=current", &url);
        jar.set_cookie_from_js(
            "old=gone; Expires=Thu, 01 Jan 2020 00:00:00 GMT",
            &url,
        );
        assert!(jar.get_all_cookies().is_empty());
    }

    #[test]
    fn test_samesite_parsed() {
        let jar = CookieJar::new();
        let url = Url::parse("https://example.com/").unwrap();
        jar.set_cookie("strict_cookie=val; SameSite=Strict", &url);
        assert!(jar.get_cookie_header(&url).contains("strict_cookie=val"));
    }

    #[test]
    fn test_clear_cookies() {
        let jar = CookieJar::new();
        let url = Url::parse("https://example.com/").unwrap();
        jar.set_cookie("a=1", &url);
        assert!(!jar.get_cookie_header(&url).is_empty());

        jar.clear();
        assert!(jar.get_cookie_header(&url).is_empty());
    }

    #[test]
    fn test_set_cookies_from_cdp_preserves_same_site_and_expires() {
        let jar = CookieJar::new();
        let future_expiry = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
            + 3600;
        jar.set_cookies_from_cdp(vec![CookieInfo {
            name: "sid".to_string(),
            value: "abc".to_string(),
            domain: "example.com".to_string(),
            path: "/".to_string(),
            secure: true,
            http_only: true,
            same_site: "Strict".to_string(),
            expires: Some(future_expiry),
        }]);

        let cookies = jar.get_all_cookies();
        assert_eq!(cookies.len(), 1);
        assert_eq!(cookies[0].same_site, "Strict");
        assert_eq!(cookies[0].expires, Some(future_expiry));
    }

    #[test]
    fn test_set_cookies_from_cdp_session_when_expires_none() {
        let jar = CookieJar::new();
        jar.set_cookies_from_cdp(vec![CookieInfo {
            name: "n".to_string(),
            value: "v".to_string(),
            domain: "example.com".to_string(),
            path: "/".to_string(),
            secure: false,
            http_only: false,
            same_site: String::new(),
            expires: None,
        }]);
        let cookies = jar.get_all_cookies();
        assert_eq!(cookies[0].expires, None);
        assert_eq!(cookies[0].same_site, DEFAULT_SAME_SITE);
    }

    #[test]
    fn test_set_cookies_from_cdp_minus_one_is_a_session_cookie() {
        let jar = CookieJar::new();
        jar.set_cookies_from_cdp(vec![CookieInfo {
            name: "n".to_string(),
            value: "v".to_string(),
            domain: "example.com".to_string(),
            path: "/".to_string(),
            secure: false,
            http_only: false,
            same_site: String::new(),
            expires: Some(-1),
        }]);
        let cookies = jar.get_all_cookies();
        assert_eq!(cookies.len(), 1);
        assert_eq!(cookies[0].expires, None);
    }

    fn cdp(name: &str, value: &str, domain: &str, path: &str) -> CookieInfo {
        CookieInfo {
            name: name.to_string(),
            value: value.to_string(),
            domain: domain.to_string(),
            path: path.to_string(),
            secure: false,
            http_only: false,
            same_site: String::new(),
            expires: None,
        }
    }

    fn marker(jar: &CookieJar) -> Vec<CookieInfo> {
        jar.get_all_cookies().into_iter().filter(|c| c.name == "marker").collect()
    }

    #[test]
    fn test_cdp_domain_leading_dot_is_not_a_separate_cookie() {
        // The two entrances disagreed: CDP retained the dot, while Set-Cookie stripped it.
        let jar = CookieJar::new();
        let url = Url::parse("https://app.example.com/").unwrap();

        jar.set_cookies_from_cdp(vec![cdp("marker", "stale", ".example.com", "/")]);
        jar.set_cookie("marker=fresh; Domain=example.com; Path=/", &url);

        assert_eq!(marker(&jar).len(), 1, "one cookie, one entry, got: {:?}", marker(&jar));
        let header = jar.get_cookie_header(&url);
        assert_eq!(header.matches("marker=").count(), 1, "header: {header:?}");
        assert!(header.contains("marker=fresh"), "newer value must win: {header:?}");
    }

    #[test]
    fn test_cdp_stored_domain_field_carries_no_dot() {
        // The dotted spelling comes LAST. That is why an implementation which only
        // canonicalizes the map key and leaves `entry.domain` raw also fails here.
        let jar = CookieJar::new();
        jar.set_cookies_from_cdp(vec![cdp("marker", "one", "example.com", "/")]);
        jar.set_cookies_from_cdp(vec![cdp("marker", "two", ".EXAMPLE.com", "/")]);

        let all = marker(&jar);
        assert_eq!(all.len(), 1, "both spellings collapse, got: {all:?}");
        assert_eq!(all[0].domain, "example.com", "entry.domain is canonical, got: {:?}", all[0].domain);
        assert_eq!(all[0].value, "two", "newer value must win");
    }

    #[test]
    fn test_cdp_domain_leading_dot_repairs_a_persisted_store() {
        // load_from_file() imports through here, so an old store must not resurrect the
        // duplicate.
        let jar = CookieJar::new();
        jar.set_cookies_from_cdp(vec![
            cdp("marker", "one", ".example.com", "/"),
            cdp("marker", "two", "example.com", "/"),
        ]);
        assert_eq!(marker(&jar).len(), 1, "got: {:?}", marker(&jar));
    }

    #[test]
    fn test_cdp_zero_expiry_deletes_across_domain_spellings() {
        // The insert canonicalizes the key, so every delete path has to canonicalize its
        // lookup too. Otherwise the cookie becomes unreachable and keeps going out.
        for (stored, deleted) in [
            ("Example.COM", "Example.COM"),
            (".example.com", "example.com"),
            ("example.com", ".EXAMPLE.com"),
        ] {
            let jar = CookieJar::new();
            jar.set_cookies_from_cdp(vec![cdp("marker", "current", stored, "/")]);
            let mut gone = cdp("marker", "", deleted, "/");
            gone.expires = Some(0);
            jar.set_cookies_from_cdp(vec![gone]);
            assert!(marker(&jar).is_empty(), "stored {stored:?}, deleted {deleted:?}: {:?}", marker(&jar));
        }
    }

    #[test]
    fn test_delete_cookie_canonicalizes_its_lookup() {
        for spelling in ["Example.COM", ".example.com", "example.com"] {
            let jar = CookieJar::new();
            jar.set_cookies_from_cdp(vec![cdp("marker", "current", "Example.COM", "/")]);
            jar.delete_cookie("marker", spelling);
            assert!(marker(&jar).is_empty(), "delete_cookie({spelling:?}): {:?}", marker(&jar));

            let jar = CookieJar::new();
            jar.set_cookies_from_cdp(vec![cdp("marker", "current", "Example.COM", "/")]);
            jar.delete_cookies_filtered("marker", spelling, Some("/"));
            assert!(marker(&jar).is_empty(), "delete_cookies_filtered({spelling:?}): {:?}", marker(&jar));
        }
    }

    #[test]
    fn test_set_cookies_from_cdp_zero_expiry_deletes_matching_cookie() {
        let jar = CookieJar::new();
        jar.set_cookies_from_cdp(vec![CookieInfo {
            name: "sid".to_string(),
            value: "current".to_string(),
            domain: ".example.com".to_string(),
            path: "/account".to_string(),
            secure: false,
            http_only: false,
            same_site: String::new(),
            expires: None,
        }]);
        jar.set_cookies_from_cdp(vec![CookieInfo {
            name: "sid".to_string(),
            value: String::new(),
            domain: "example.com".to_string(),
            path: "/account".to_string(),
            secure: false,
            http_only: false,
            same_site: String::new(),
            expires: Some(0),
        }]);
        assert!(jar.get_all_cookies().is_empty());
    }

    #[test]
    fn test_delete_cookies_filtered_path_mismatch_preserves_cookie() {
        let jar = CookieJar::new();
        jar.set_cookies_from_cdp(vec![CookieInfo {
            name: "sid".to_string(),
            value: "v".to_string(),
            domain: "example.com".to_string(),
            path: "/admin".to_string(),
            secure: false,
            http_only: false,
            same_site: String::new(),
            expires: None,
        }]);
        jar.delete_cookies_filtered("sid", "example.com", Some("/other"));
        assert_eq!(jar.get_all_cookies().len(), 1);

        jar.delete_cookies_filtered("sid", "example.com", Some("/admin"));
        assert!(jar.get_all_cookies().is_empty());
    }

    #[test]
    fn test_delete_cookies_filtered_no_path_deletes_regardless() {
        let jar = CookieJar::new();
        jar.set_cookies_from_cdp(vec![CookieInfo {
            name: "sid".to_string(),
            value: "v".to_string(),
            domain: "example.com".to_string(),
            path: "/admin".to_string(),
            secure: false,
            http_only: false,
            same_site: String::new(),
            expires: None,
        }]);
        jar.delete_cookies_filtered("sid", "example.com", None);
        assert!(jar.get_all_cookies().is_empty());
    }

    #[test]
    fn test_set_cookies_from_cdp_expired_does_not_persist() {
        let jar = CookieJar::new();
        jar.set_cookies_from_cdp(vec![CookieInfo {
            name: "old".to_string(),
            value: "current".to_string(),
            domain: "example.com".to_string(),
            path: "/".to_string(),
            secure: false,
            http_only: false,
            same_site: String::new(),
            expires: None,
        }]);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        jar.set_cookies_from_cdp(vec![CookieInfo {
            name: "old".to_string(),
            value: "v".to_string(),
            domain: "example.com".to_string(),
            path: "/".to_string(),
            secure: false,
            http_only: false,
            same_site: String::new(),
            expires: Some(now - 1),
        }]);
        assert!(jar.get_all_cookies().is_empty());
    }
    #[test]
    fn test_save_load_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("cookies.json");

        let jar = CookieJar::new();
        let url = Url::parse("https://example.com/").unwrap();
        jar.set_cookie("session=abc123; Domain=example.com; Path=/", &url);
        jar.set_cookie("token=xyz; Secure; HttpOnly", &url);

        jar.save_to_file(&path).unwrap();
        assert!(path.exists());

        let jar2 = CookieJar::new();
        let count = jar2.load_from_file(&path).unwrap();
        assert_eq!(count, 2);

        let header = jar2.get_cookie_header(&url);
        assert!(header.contains("session=abc123"));
        assert!(header.contains("token=xyz"));
    }

    #[test]
    fn versioned_save_load_is_lossless_for_http_and_document_cookies() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("cookies.json");
        let origin = Url::parse("https://www.example.com/account/login").unwrap();
        let jar = CookieJar::new();

        jar.set_cookie(
            "server_host=opaque==value; Path=/; Secure; HttpOnly; SameSite=Strict; Max-Age=3600",
            &origin,
        );
        jar.set_cookie(
            "server_domain=domain-value; Domain=example.com; Path=/; Secure; HttpOnly; SameSite=None; Max-Age=3600",
            &origin,
        );
        jar.set_cookie_from_js(
            "js_host=host-value; Path=/account; Secure; SameSite=Lax; Max-Age=3600",
            &origin,
        );
        jar.set_cookie_from_js(
            "js_domain=domain-value; Domain=example.com; Path=/; Secure; SameSite=Strict; Max-Age=3600",
            &origin,
        );
        jar.set_cookie("same=root; Domain=example.com; Path=/", &origin);
        jar.set_cookie("same=account; Domain=example.com; Path=/account", &origin);

        let expected = jar.snapshot();
        jar.save_to_file(&path).unwrap();

        let json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(json["version"], COOKIE_STORE_VERSION);
        assert!(json["cookies"].as_array().unwrap().iter().any(|cookie| {
            cookie["name"] == "server_host"
                && cookie["value"] == "opaque==value"
                && cookie["host_only"] == true
                && cookie["http_only"] == true
                && cookie["secure"] == true
                && cookie["same_site"] == "Strict"
        }));

        let restored = CookieJar::new();
        assert_eq!(restored.load_from_file(&path).unwrap(), 6);
        assert_eq!(restored.snapshot().entries, expected.entries);

        let same_origin = Url::parse("https://www.example.com/account/page").unwrap();
        let host_child = Url::parse("https://sub.www.example.com/account/page").unwrap();
        let domain_sibling = Url::parse("https://api.example.com/account/page").unwrap();
        let root = Url::parse("https://example.com/").unwrap();
        let account = Url::parse("https://example.com/account/page").unwrap();

        let same_origin_header = restored.get_cookie_header(&same_origin);
        assert!(same_origin_header.contains("server_host=opaque==value"));
        assert!(same_origin_header.contains("js_host=host-value"));
        let child_header = restored.get_cookie_header(&host_child);
        assert!(!child_header.contains("server_host=opaque==value"));
        assert!(!child_header.contains("js_host=host-value"));
        let sibling_header = restored.get_cookie_header(&domain_sibling);
        assert!(sibling_header.contains("server_domain=domain-value"));
        assert!(sibling_header.contains("js_domain=domain-value"));

        let visible = restored.get_js_visible_cookies(&same_origin);
        assert!(!visible.contains("server_host="));
        assert!(!visible.contains("server_domain="));
        assert!(visible.contains("js_host=host-value"));
        assert_eq!(restored.get_cookie_header(&root).matches("same=").count(), 1);
        assert_eq!(restored.get_cookie_header(&account).matches("same=").count(), 2);
    }

    #[test]
    fn saved_creation_order_survives_reordered_file_and_future_updates() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("cookies.json");
        let url = Url::parse("https://example.com/account/page").unwrap();
        let source = CookieJar::new();
        source.set_cookie("deleted=value; Path=/", &url);
        source.set_cookie("root=value; Path=/", &url);
        source.set_cookie("first=old; Path=/account", &url);
        source.set_cookie("second=value; Path=/account", &url);
        source.set_cookie("first=opaque==updated; Path=/account", &url);
        source.delete_cookie("deleted", "example.com");
        source.save_to_file(&path).unwrap();

        let mut json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let entries = json["cookies"].as_array_mut().unwrap();
        assert_eq!(entries.iter().map(|entry| entry["creation_order"].as_u64().unwrap())
            .collect::<Vec<_>>(), vec![2, 3, 4]);
        entries.reverse();
        std::fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();

        let restored = CookieJar::new();
        restored.load_from_file(&path).unwrap();
        assert_eq!(restored.snapshot().entries, source.snapshot().entries);
        assert_eq!(restored.get_cookie_header(&url),
            "first=opaque==updated; second=value; root=value");
        restored.set_cookie("third=value; Path=/account", &url);
        restored.set_cookie("first=again; Path=/account", &url);
        assert_eq!(restored.get_cookie_header(&url),
            "first=again; second=value; third=value; root=value");
    }

    #[test]
    fn older_cookie_files_use_array_order_for_missing_creation_metadata() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("cookies.json");
        let url = Url::parse("https://example.com/").unwrap();
        let source = CookieJar::new();
        source.set_cookie("first=opaque==one; Path=/", &url);
        source.set_cookie("second=two; Path=/", &url);
        source.save_to_file(&path).unwrap();
        let mut versioned: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        for entry in versioned["cookies"].as_array_mut().unwrap() {
            entry.as_object_mut().unwrap().remove("creation_order");
        }
        let legacy = serde_json::json!([
            {"name": "first", "value": "opaque==one", "domain": "example.com",
             "path": "/", "secure": false, "httpOnly": false},
            {"name": "second", "value": "two", "domain": "example.com",
             "path": "/", "secure": false, "httpOnly": false}
        ]);
        for file in [versioned, legacy] {
            std::fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();
            let restored = CookieJar::new();
            assert_eq!(restored.load_from_file(&path).unwrap(), 2);
            restored.set_cookie("first=updated; Path=/", &url);
            restored.set_cookie("third=three; Path=/", &url);
            assert_eq!(restored.get_cookie_header(&url),
                "first=updated; second=two; third=three");
        }
    }

    #[test]
    fn file_merge_preserves_destination_positions_and_source_relative_order() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("cookies.json");
        let url = Url::parse("https://example.com/").unwrap();
        let source = CookieJar::new();
        source.set_cookie("new_first=one; Path=/", &url);
        source.set_cookie("existing=updated; Path=/", &url);
        source.set_cookie("new_second=two; Path=/", &url);
        source.save_to_file(&path).unwrap();
        let destination = CookieJar::new();
        destination.set_cookie("existing=old; Path=/", &url);
        destination.set_cookie("concurrent=value; Path=/", &url);
        destination.load_from_file(&path).unwrap();
        assert_eq!(destination.get_cookie_header(&url),
            "existing=updated; concurrent=value; new_first=one; new_second=two");
    }

    #[test]
    fn legacy_cookie_array_keeps_domain_scoped_import_semantics() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("cookies.json");
        let future = unix_time_secs() as i64 + 3600;
        let legacy = serde_json::json!([{
            "name": "legacy",
            "value": "opaque==value",
            "domain": "example.com",
            "path": "/account",
            "secure": true,
            "httpOnly": true,
            "sameSite": "None",
            "expires": future
        }]);
        std::fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();

        let jar = CookieJar::new();
        assert_eq!(jar.load_from_file(&path).unwrap(), 1);
        let snapshot = jar.snapshot();
        let entry = snapshot.entries.values().next().unwrap();
        assert!(!entry.host_only, "legacy files never encoded host-only state");
        assert_eq!(entry.value, "opaque==value");
        assert_eq!(entry.path, "/account");
        assert!(entry.secure);
        assert!(entry.http_only);
        assert_eq!(entry.same_site, "None");
        assert_eq!(entry.expires, Some(future as u64));

        let child = Url::parse("https://sub.example.com/account/page").unwrap();
        assert!(jar.get_cookie_header(&child).contains("legacy=opaque==value"));
        assert!(!jar.get_js_visible_cookies(&child).contains("legacy="));
        let insecure = Url::parse("http://sub.example.com/account/page").unwrap();
        assert!(!jar.get_cookie_header(&insecure).contains("legacy="));
    }

    #[test]
    fn versioned_load_rejects_unknown_versions_without_mutating_the_jar() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("cookies.json");
        std::fs::write(&path, r#"{"version":2,"cookies":[]}"#).unwrap();
        let jar = CookieJar::new();
        let url = Url::parse("https://example.com/").unwrap();
        jar.set_cookie("kept=value; Path=/", &url);

        let error = jar.load_from_file(&path).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(jar.get_cookie_header(&url).contains("kept=value"));
    }

    #[test]
    fn versioned_load_requires_host_only_without_mutating_the_jar() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("cookies.json");
        std::fs::write(
            &path,
            r#"{
                "version": 1,
                "cookies": [{
                    "name": "ambiguous",
                    "value": "value",
                    "path": "/",
                    "domain": "example.com",
                    "secure": false,
                    "http_only": false,
                    "expires": null,
                    "same_site": "Lax"
                }]
            }"#,
        )
        .unwrap();
        let jar = CookieJar::new();
        let url = Url::parse("https://example.com/").unwrap();
        jar.set_cookie("kept=value; Path=/", &url);

        let error = jar.load_from_file(&path).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(jar.get_cookie_header(&url).contains("kept=value"));
        assert!(!jar.get_cookie_header(&url).contains("ambiguous="));
    }

    #[test]
    fn versioned_load_does_not_restore_expired_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("cookies.json");
        let store = PersistedCookieStore {
            version: COOKIE_STORE_VERSION,
            cookies: vec![CookieEntry {
                name: "gone".to_string(),
                value: "stale".to_string(),
                path: "/".to_string(),
                domain: "example.com".to_string(),
                host_only: true,
                secure: false,
                http_only: false,
                expires: Some(0),
                same_site: "Lax".to_string(),
                creation_order: 0,
            }],
        };
        std::fs::write(&path, serde_json::to_vec(&store).unwrap()).unwrap();
        let jar = CookieJar::new();
        let url = Url::parse("https://example.com/").unwrap();
        jar.set_cookie("gone=current; Path=/", &url);

        assert_eq!(jar.load_from_file(&path).unwrap(), 1);
        assert!(!jar.get_cookie_header(&url).contains("gone="));
    }

    #[test]
    fn snapshot_clone_preserves_scope_and_is_independent() {
        let origin = Url::parse("https://www.example.com/").unwrap();
        let child = Url::parse("https://sub.www.example.com/").unwrap();
        let source = CookieJar::new();
        source.set_cookie("host=source; Path=/", &origin);

        let snapshot = source.snapshot();
        let copied = CookieJar::from_snapshot(&snapshot.clone());
        assert!(copied.get_cookie_header(&origin).contains("host=source"));
        assert!(!copied.get_cookie_header(&child).contains("host=source"));

        copied.set_cookie("host=copied; Path=/", &origin);
        assert!(copied.get_cookie_header(&origin).contains("host=copied"));
        assert!(source.get_cookie_header(&origin).contains("host=source"));
    }

    #[test]
    fn snapshot_delta_orders_new_entries_after_destination_and_keeps_replacements() {
        let url = Url::parse("https://example.com/").unwrap();
        let destination = CookieJar::new();
        destination.set_cookie("discard=value; Path=/", &url);
        destination.set_cookie("existing=old; Path=/", &url);
        destination.delete_cookie("discard", "example.com");
        let initial = destination.snapshot();
        let connection = CookieJar::from_snapshot(&initial);
        assert_eq!(connection.snapshot().entries, initial.entries);
        destination.set_cookie("concurrent=value; Path=/", &url);
        connection.set_cookie("new_first=one; Path=/", &url);
        connection.set_cookie("existing=updated; Path=/", &url);
        connection.set_cookie("new_second=two; Path=/", &url);
        destination.apply_snapshot_delta(&initial, &connection.snapshot());
        assert_eq!(destination.get_cookie_header(&url),
            "existing=updated; concurrent=value; new_first=one; new_second=two");
    }

    #[test]
    fn snapshot_clone_does_not_reuse_expired_creation_order() {
        let source = CookieJar::new();
        let url = Url::parse("https://example.com/").unwrap();
        source.set_cookie("first=one; Path=/", &url);
        source.set_cookie("expired=two; Path=/", &url);
        source.cookies.write().unwrap().get_mut("example.com").unwrap()
            .get_mut(&("expired".to_string(), "/".to_string())).unwrap().expires = Some(0);
        let initial = source.snapshot();
        let copy = CookieJar::from_snapshot(&initial);
        copy.set_cookie("expired=two; Path=/", &url);
        let key = ("example.com".to_string(), "expired".to_string(), "/".to_string());
        assert!(copy.snapshot().entries[&key].creation_order > initial.entries[&key].creation_order);
    }

    #[test]
    fn snapshot_delta_recognizes_delete_and_recreate_of_same_key() {
        let url = Url::parse("https://example.com/").unwrap();
        let destination = CookieJar::new();
        destination.set_cookie("first=one; Path=/", &url);
        destination.set_cookie("second=two; Path=/", &url);
        let initial = destination.snapshot();
        let connection = CookieJar::from_snapshot(&initial);
        connection.delete_cookie("first", "example.com");
        connection.set_cookie("third=three; Path=/", &url);
        connection.set_cookie("first=one; Path=/", &url);
        destination.apply_snapshot_delta(&initial, &connection.snapshot());
        assert_eq!(destination.get_cookie_header(&url), "second=two; third=three; first=one");
    }

    #[test]
    fn snapshot_delta_compares_host_only_and_preserves_unchanged_concurrent_values() {
        let apex = Url::parse("https://example.com/").unwrap();
        let setter = Url::parse("https://www.example.com/").unwrap();
        let child = Url::parse("https://sub.example.com/").unwrap();

        let connection = CookieJar::new();
        connection.set_cookie("scope=one; Path=/", &apex);
        connection.set_cookie("unchanged=old; Path=/", &apex);
        let initial = connection.snapshot();
        connection.set_cookie("scope=one; Domain=example.com; Path=/", &setter);

        let destination = CookieJar::from_snapshot(&initial);
        destination.set_cookie("unchanged=concurrent; Path=/", &apex);
        destination.apply_snapshot_delta(&initial, &connection.snapshot());

        assert!(
            destination.get_cookie_header(&child).contains("scope=one"),
            "a host-only to domain-scoped change must count as a replacement"
        );
        assert!(
            destination.get_cookie_header(&apex).contains("unchanged=concurrent"),
            "an unchanged snapshot entry must not overwrite a concurrent value"
        );
    }

    #[test]
    fn snapshot_delta_does_not_treat_natural_expiry_as_an_explicit_delete() {
        let url = Url::parse("https://example.com/").unwrap();
        let connection = CookieJar::new();
        let expired = CookieEntry {
            name: "sid".to_string(),
            value: "old".to_string(),
            path: "/".to_string(),
            domain: "example.com".to_string(),
            host_only: true,
            secure: false,
            http_only: false,
            expires: Some(0),
            same_site: "Lax".to_string(),
            creation_order: 0,
        };
        connection
            .cookies
            .write()
            .unwrap()
            .entry(expired.domain.clone())
            .or_default()
            .insert((expired.name.clone(), expired.path.clone()), expired);

        let initial = connection.snapshot();
        let current = connection.snapshot();
        assert_eq!(initial.entries.len(), 1, "snapshots retain stored expired entries");
        assert!(
            CookieJar::from_snapshot(&initial).get_all_cookies().is_empty(),
            "materializing a snapshot must still filter expired entries"
        );

        let destination = CookieJar::new();
        destination.set_cookie("sid=refreshed; Path=/; Max-Age=3600", &url);
        destination.apply_snapshot_delta(&initial, &current);
        assert!(
            destination.get_cookie_header(&url).contains("sid=refreshed"),
            "an unchanged entry that naturally expired must not delete a concurrent refresh"
        );

        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("cookies.json");
        connection.save_to_file(&path).unwrap();
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert!(saved["cookies"].as_array().unwrap().is_empty());
    }

    #[test]
    fn snapshot_delta_keeps_explicit_deletes_and_ignores_expired_changes() {
        let url = Url::parse("https://example.com/").unwrap();

        let deleted_connection = CookieJar::new();
        deleted_connection.set_cookie("sid=old; Path=/; Max-Age=3600", &url);
        let deleted_initial = deleted_connection.snapshot();
        deleted_connection.set_cookie("sid=gone; Path=/; Max-Age=0", &url);
        let destination = CookieJar::new();
        destination.set_cookie("sid=refreshed; Path=/; Max-Age=3600", &url);
        destination.apply_snapshot_delta(&deleted_initial, &deleted_connection.snapshot());
        assert!(
            !destination.get_cookie_header(&url).contains("sid="),
            "a setter-driven removal remains an explicit delete"
        );

        let changed_connection = CookieJar::new();
        changed_connection.set_cookie("sid=old; Path=/; Max-Age=3600", &url);
        let changed_initial = changed_connection.snapshot();
        let mut expired_change = changed_initial.entries.values().next().unwrap().clone();
        expired_change.value = "expired-change".to_string();
        expired_change.expires = Some(0);
        changed_connection
            .cookies
            .write()
            .unwrap()
            .entry(expired_change.domain.clone())
            .or_default()
            .insert(
                (expired_change.name.clone(), expired_change.path.clone()),
                expired_change,
            );

        let destination = CookieJar::new();
        destination.set_cookie("sid=refreshed; Path=/; Max-Age=3600", &url);
        destination.apply_snapshot_delta(&changed_initial, &changed_connection.snapshot());
        assert!(
            destination.get_cookie_header(&url).contains("sid=refreshed"),
            "an expired changed entry must not erase or overwrite a concurrent refresh"
        );
    }

    #[test]
    fn test_load_nonexistent_file_returns_zero() {
        let jar = CookieJar::new();
        let count = jar
            .load_from_file(std::path::Path::new("/nonexistent/cookies.json"))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_domain_matches_subdomain_without_leading_dot() {
        let jar = CookieJar::new();
        jar.set_cookies_from_cdp(vec![CookieInfo {
            name: "session".to_string(),
            value: "abc".to_string(),
            domain: "xiaohongshu.com".to_string(),
            path: "/".to_string(),
            secure: false,
            http_only: true,
            same_site: String::new(),
            expires: None,
        }]);
        let url = Url::parse("https://www.xiaohongshu.com/explore").unwrap();
        let header = jar.get_cookie_header(&url);
        assert!(header.contains("session=abc"), "Cookie header was: '{}'", header);
    }

    #[test]
    fn test_cookie_from_file_load_then_send_in_request() {
        // Simulate what happens: load cookies from file → navigate → cookie should be in request
        use std::io::Write;
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("cookies.json");
        
        // Write cookies like we exported from Chrome
        let cookies = serde_json::json!([
            {"name": "a1", "value": "testval", "domain": "xiaohongshu.com", "path": "/", "secure": false, "httpOnly": false},
            {"name": "web_session", "value": "sess123", "domain": "xiaohongshu.com", "path": "/", "secure": false, "httpOnly": true},
        ]);
        std::fs::write(&path, serde_json::to_string(&cookies).unwrap()).unwrap();
        
        let jar = CookieJar::new();
        let count = jar.load_from_file(&path).unwrap();
        assert_eq!(count, 2, "Should load 2 cookies");
        
        let url = Url::parse("https://www.xiaohongshu.com/explore").unwrap();
        let header = jar.get_cookie_header(&url);
        assert!(header.contains("a1=testval"), "Missing a1 in: '{}'", header);
        assert!(header.contains("web_session=sess123"), "Missing web_session in: '{}'", header);
    }

    #[test]
    fn attacker_response_cannot_set_unrelated_victim_domain_cookie() {
        // GHSA-f22c-8v6q-v6h6: a response from attacker.test must not be able to
        // plant a cookie scoped to victim.test.
        let jar = CookieJar::new();
        let attacker = Url::parse("http://attacker.test/").unwrap();
        jar.set_cookie("sid=attacker; Domain=victim.test; Path=/", &attacker);

        let victim = Url::parse("http://victim.test/account").unwrap();
        assert!(
            !jar.get_cookie_header(&victim).contains("sid=attacker"),
            "cross-domain cookie leaked to victim: {}",
            jar.get_cookie_header(&victim)
        );
        // The cookie is stored host-only on the attacker origin instead.
        assert!(jar.get_cookie_header(&attacker).contains("sid=attacker"));
    }

    #[test]
    fn document_cookie_cannot_set_unrelated_victim_domain_cookie() {
        let jar = CookieJar::new();
        let attacker = Url::parse("http://attacker.test/").unwrap();
        jar.set_cookie_from_js("js_sid=attacker; Domain=victim.test; Path=/", &attacker);

        let victim = Url::parse("http://victim.test/account").unwrap();
        assert!(
            !jar.get_cookie_header(&victim).contains("js_sid=attacker"),
            "cross-domain JS cookie leaked to victim: {}",
            jar.get_cookie_header(&victim)
        );
    }

    #[test]
    fn public_suffix_domain_attribute_is_ignored() {
        let jar = CookieJar::new();
        let url = Url::parse("http://www.example.com/").unwrap();
        jar.set_cookie("bad=1; Domain=com; Path=/", &url);
        // "com" is a public suffix; the cookie must not be scoped to it.
        let other = Url::parse("http://other.com/").unwrap();
        assert!(!jar.get_cookie_header(&other).contains("bad=1"));
    }

    #[test]
    fn host_only_cookie_not_sent_to_subdomain() {
        let jar = CookieJar::new();
        let www = Url::parse("http://www.example.com/").unwrap();
        jar.set_cookie("hostonly=1; Path=/", &www); // no Domain attribute -> host-only

        assert!(jar.get_cookie_header(&www).contains("hostonly=1"));
        let sub = Url::parse("http://sub.www.example.com/").unwrap();
        assert!(
            !jar.get_cookie_header(&sub).contains("hostonly=1"),
            "host-only cookie leaked to subdomain: {}",
            jar.get_cookie_header(&sub)
        );
    }

    #[test]
    fn valid_subdomain_can_set_parent_domain_cookie() {
        // A subdomain setting Domain=<parent> (a legitimate parent) still works.
        let jar = CookieJar::new();
        let www = Url::parse("http://www.example.com/").unwrap();
        jar.set_cookie("token=1; Domain=example.com; Path=/", &www);

        let apex = Url::parse("http://example.com/").unwrap();
        assert!(jar.get_cookie_header(&apex).contains("token=1"));
        let api = Url::parse("http://api.example.com/").unwrap();
        assert!(jar.get_cookie_header(&api).contains("token=1"));
    }

    #[test]
    fn cookie_path_requires_slash_boundary() {
        // RFC 6265 5.1.4: a Path=/admin cookie must NOT be sent to a sibling
        // path like /administrator that merely shares the string prefix. It is
        // sent to /admin, /admin/, and /admin/x.
        let jar = CookieJar::new();
        let admin = Url::parse("https://example.com/admin").unwrap();
        jar.set_cookie("sess=1; Path=/admin", &admin);

        let sibling = Url::parse("https://example.com/administrator").unwrap();
        assert!(
            !jar.get_cookie_header(&sibling).contains("sess=1"),
            "cookie leaked to sibling path /administrator: {}",
            jar.get_cookie_header(&sibling)
        );

        assert!(jar.get_cookie_header(&admin).contains("sess=1"));
        let exact_slash = Url::parse("https://example.com/admin/").unwrap();
        assert!(jar.get_cookie_header(&exact_slash).contains("sess=1"));
        let sub = Url::parse("https://example.com/admin/panel").unwrap();
        assert!(jar.get_cookie_header(&sub).contains("sess=1"));
    }

    #[test]
    fn default_cookie_path_is_request_directory() {
        // RFC 6265 5.1.4 default-path: up to (not including) the right-most '/'.
        assert_eq!(default_cookie_path("/app/login"), "/app");
        assert_eq!(default_cookie_path("/app/"), "/app");
        assert_eq!(default_cookie_path("/a/b/c"), "/a/b");
        // No more than one '/', empty, or non-absolute -> "/".
        assert_eq!(default_cookie_path("/foo"), "/");
        assert_eq!(default_cookie_path("/"), "/");
        assert_eq!(default_cookie_path(""), "/");
        assert_eq!(default_cookie_path("relative"), "/");
    }

    #[test]
    fn cookie_without_path_defaults_to_directory_not_full_path() {
        // A Set-Cookie with no Path attribute on /app/login must scope to /app
        // (RFC 6265 5.1.4), so the session survives navigation to /app/dashboard.
        // Before this fix it was scoped to the full path /app/login and vanished
        // on the next page, appearing as a silent logout.
        let jar = CookieJar::new();
        let login = Url::parse("https://example.com/app/login").unwrap();
        jar.set_cookie("sid=abc", &login);

        let dashboard = Url::parse("https://example.com/app/dashboard").unwrap();
        assert!(
            jar.get_cookie_header(&dashboard).contains("sid=abc"),
            "session cookie was not sent to a sibling path under the same directory: {}",
            jar.get_cookie_header(&dashboard)
        );
        // Still sent at the directory root and the original path.
        let app_root = Url::parse("https://example.com/app/").unwrap();
        assert!(jar.get_cookie_header(&app_root).contains("sid=abc"));
        assert!(jar.get_cookie_header(&login).contains("sid=abc"));

        // But not to an unrelated top-level path outside the directory.
        let other = Url::parse("https://example.com/other").unwrap();
        assert!(
            !jar.get_cookie_header(&other).contains("sid=abc"),
            "cookie leaked outside its default-path directory"
        );
    }

    #[test]
    fn js_cookie_without_path_also_defaults_to_directory() {
        // document.cookie set on /shop/cart with no path must reach /shop/checkout.
        let jar = CookieJar::new();
        let cart = Url::parse("https://example.com/shop/cart").unwrap();
        jar.set_cookie_from_js("cart=xyz", &cart);
        let checkout = Url::parse("https://example.com/shop/checkout").unwrap();
        assert!(
            jar.get_js_visible_cookies(&checkout).contains("cart=xyz"),
            "JS cookie not visible at sibling path: {}",
            jar.get_js_visible_cookies(&checkout)
        );
    }
}
