use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::OnceLock;
use thiserror::Error;

use crate::StealthProfile;

pub const PERSONA_SCHEMA_VERSION: &str = "1";

#[derive(Clone, Debug, PartialEq, Eq)]
struct ProcessPersonaClaim {
    timezone: String,
    language: String,
}

static PROCESS_PERSONA: OnceLock<ProcessPersonaClaim> = OnceLock::new();

/// Freeze process-wide identity inputs before any V8 isolate or helper thread
/// is created. V8 reads timezone and ICU locale state from the process, so
/// accepting a second persona with different values would make JavaScript and
/// transport identity disagree.
pub fn activate_process_persona(persona: &EffectivePersona) -> Result<(), PersonaError> {
    let requested = ProcessPersonaClaim {
        timezone: persona.timezone().to_string(),
        language: persona.language().to_string(),
    };
    if let Some(active) = PROCESS_PERSONA.get() {
        if active == &requested {
            return Ok(());
        }
        return Err(PersonaError::Capability(format!(
            "process identity is already frozen to timezone {:?} and language {:?}; cannot activate timezone {:?} and language {:?}",
            active.timezone, active.language, requested.timezone, requested.language,
        )));
    }

    let active = PROCESS_PERSONA.get_or_init(|| {
        // SAFETY: every product entry point calls this before constructing V8
        // or its own worker threads. OnceLock serializes competing Obscura
        // startup calls and the timezone is never mutated after this point.
        unsafe {
            std::env::set_var("TZ", &requested.timezone);
        }
        requested.clone()
    });
    if active == &requested {
        Ok(())
    } else {
        Err(PersonaError::Capability(format!(
            "process identity is already frozen to timezone {:?} and language {:?}; cannot activate timezone {:?} and language {:?}",
            active.timezone, active.language, requested.timezone, requested.language,
        )))
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ViewportSpec {
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GeolocationSpec {
    pub latitude: f64,
    pub longitude: f64,
}

/// Versioned persona input. A supported profile selects the transport
/// capability. Built-in presets and external JSON are completed and validated
/// by the same compiler.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PersonaSpec {
    pub schema_version: String,
    pub persona_id: String,
    pub revision: String,
    #[serde(alias = "preset")]
    pub profile: String,
    #[serde(default)]
    pub viewport: Option<ViewportSpec>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub languages: Option<Vec<String>>,
    #[serde(default)]
    pub accept_language: Option<String>,
    #[serde(default)]
    pub timezone: Option<String>,
    #[serde(default)]
    pub do_not_track: Option<String>,
    #[serde(default)]
    pub hardware_concurrency: Option<u32>,
    #[serde(default)]
    pub device_memory: Option<f64>,
    #[serde(default)]
    pub screen_width: Option<u32>,
    #[serde(default)]
    pub screen_height: Option<u32>,
    #[serde(default)]
    pub screen_color_depth: Option<u32>,
    #[serde(default)]
    pub screen_avail_width: Option<u32>,
    #[serde(default)]
    pub screen_avail_height: Option<u32>,
    #[serde(default)]
    pub outer_width: Option<u32>,
    #[serde(default)]
    pub outer_height: Option<u32>,
    #[serde(default)]
    pub device_scale_factor: Option<f64>,
    #[serde(default)]
    pub battery_charging: Option<bool>,
    #[serde(default)]
    pub battery_level: Option<f64>,
    #[serde(default)]
    pub network_rtt: Option<u32>,
    #[serde(default)]
    pub storage_quota: Option<u64>,
    #[serde(default)]
    pub webgl_vendor: Option<String>,
    #[serde(default)]
    pub webgl_renderer: Option<String>,
    #[serde(default)]
    pub geolocation: Option<GeolocationSpec>,
}

impl PersonaSpec {
    pub fn preset(profile: StealthProfile) -> Self {
        Self {
            schema_version: PERSONA_SCHEMA_VERSION.to_string(),
            persona_id: profile.name().to_string(),
            revision: "builtin-1".to_string(),
            profile: profile.name().to_string(),
            viewport: None,
            language: None,
            languages: None,
            accept_language: None,
            timezone: None,
            do_not_track: None,
            hardware_concurrency: None,
            device_memory: None,
            screen_width: None,
            screen_height: None,
            screen_color_depth: None,
            screen_avail_width: None,
            screen_avail_height: None,
            outer_width: None,
            outer_height: None,
            device_scale_factor: None,
            battery_charging: None,
            battery_level: None,
            network_rtt: None,
            storage_quota: None,
            webgl_vendor: None,
            webgl_renderer: None,
            geolocation: None,
        }
    }

    pub fn from_json(json: &str) -> Result<Self, PersonaError> {
        serde_json::from_str(json).map_err(PersonaError::Json)
    }

    pub fn compile(self) -> Result<EffectivePersona, PersonaError> {
        if self.schema_version != PERSONA_SCHEMA_VERSION {
            return Err(PersonaError::Field {
                field: "schema_version",
                reason: format!("unsupported version {:?}", self.schema_version),
            });
        }
        validate_identifier("persona_id", &self.persona_id)?;
        validate_identifier("revision", &self.revision)?;
        let profile = StealthProfile::from_name(&self.profile).ok_or_else(|| {
            PersonaError::Capability(format!(
                "profile {:?} has no supported primp transport capability",
                self.profile
            ))
        })?;
        let macos = matches!(
            profile,
            StealthProfile::MacChrome152 | StealthProfile::MacChrome153
        );

        let mut languages = match (self.language.clone(), self.languages.clone()) {
            (None, None) if macos => vec!["en".to_string(), "zh-CN".to_string()],
            (None, None) => vec!["en-US".to_string(), "en".to_string()],
            (Some(language), None) => derive_languages(&language),
            (None, Some(languages)) | (Some(_), Some(languages)) => languages,
        };
        let language = self
            .language
            .or_else(|| languages.first().cloned())
            .ok_or_else(|| PersonaError::field("languages", "must not be empty"))?;
        if languages.first().map(String::as_str) != Some(language.as_str()) {
            return Err(PersonaError::field(
                "languages",
                "first entry must equal language",
            ));
        }
        if languages.len() > 8
            || languages.iter().any(|value| !valid_language_tag(value))
            || !valid_language_tag(&language)
        {
            return Err(PersonaError::field(
                "languages",
                "must contain 1-8 canonical tags from the supported BCP47 subset",
            ));
        }
        languages.shrink_to_fit();
        let accept_language = self
            .accept_language
            .unwrap_or_else(|| accept_language_from(&languages));
        validate_accept_language(&language, &languages, &accept_language)?;

        let viewport = self.viewport.unwrap_or(ViewportSpec {
            width: 1280,
            height: 720,
        });
        if !(320..=3840).contains(&viewport.width) {
            return Err(PersonaError::field(
                "viewport.width",
                "must be in 320..=3840",
            ));
        }
        if !(240..=2160).contains(&viewport.height) {
            return Err(PersonaError::field(
                "viewport.height",
                "must be in 240..=2160",
            ));
        }

        let timezone = self.timezone.unwrap_or_else(|| {
            if macos {
                "Asia/Shanghai"
            } else {
                "Europe/Berlin"
            }
            .to_string()
        });
        if timezone.is_empty()
            || timezone.len() > 64
            || !timezone
                .bytes()
                .all(|value| value.is_ascii_alphanumeric() || b"_+-/".contains(&value))
            || timezone.parse::<chrono_tz::Tz>().is_err()
        {
            return Err(PersonaError::field(
                "timezone",
                "invalid IANA timezone identifier",
            ));
        }
        if self
            .do_not_track
            .as_deref()
            .is_some_and(|value| !matches!(value, "0" | "1"))
        {
            return Err(PersonaError::field(
                "do_not_track",
                "must be \"0\", \"1\", or null",
            ));
        }

        let hardware_concurrency = self
            .hardware_concurrency
            .unwrap_or(if macos { 15 } else { 8 });
        if !(1..=256).contains(&hardware_concurrency) {
            return Err(PersonaError::field(
                "hardware_concurrency",
                "must be in 1..=256",
            ));
        }
        let device_memory = self.device_memory.unwrap_or(if macos { 32.0 } else { 8.0 });
        if !device_memory.is_finite() || !(0.25..=128.0).contains(&device_memory) {
            return Err(PersonaError::field(
                "device_memory",
                "must be finite and in 0.25..=128",
            ));
        }

        let screen_width = self.screen_width.unwrap_or(if macos { 2560 } else { 1920 });
        let screen_height = self
            .screen_height
            .unwrap_or(if macos { 1440 } else { 1080 });
        let screen_color_depth = self.screen_color_depth.unwrap_or(24);
        if !matches!(screen_color_depth, 24 | 30 | 32) {
            return Err(PersonaError::field("screen_color_depth", "must be 24, 30, or 32"));
        }
        if !(320..=16384).contains(&screen_width) || !(240..=16384).contains(&screen_height) {
            return Err(PersonaError::field(
                "screen",
                "dimensions are outside supported bounds",
            ));
        }
        let screen_avail_width = self.screen_avail_width.unwrap_or(screen_width);
        let screen_avail_height = self
            .screen_avail_height
            .unwrap_or_else(|| screen_height.saturating_sub(if macos { 120 } else { 40 }));
        let outer_width =
            self.outer_width
                .unwrap_or(if macos { viewport.width } else { screen_width });
        let outer_height = self.outer_height.unwrap_or(if macos {
            viewport.height
        } else {
            screen_avail_height
        });
        if !(320..=screen_width).contains(&screen_avail_width)
            || !(240..=screen_height).contains(&screen_avail_height)
            || !(320..=16384).contains(&outer_width)
            || !(240..=16384).contains(&outer_height)
        {
            return Err(PersonaError::field(
                "screen",
                "available dimensions must fit within the screen and outer dimensions within supported bounds",
            ));
        }

        let device_scale_factor = self
            .device_scale_factor
            .unwrap_or(if macos { 2.0 } else { 1.0 });
        if !device_scale_factor.is_finite() || !(0.5..=4.0).contains(&device_scale_factor) {
            return Err(PersonaError::field(
                "device_scale_factor",
                "must be finite and in 0.5..=4",
            ));
        }
        let battery_level = self.battery_level.unwrap_or(0.8);
        if !battery_level.is_finite() || !(0.0..=1.0).contains(&battery_level) {
            return Err(PersonaError::field(
                "battery_level",
                "must be finite and in 0..=1",
            ));
        }
        let network_rtt = self.network_rtt.unwrap_or(100);
        if !(1..=10_000).contains(&network_rtt) {
            return Err(PersonaError::field("network_rtt", "must be in 1..=10000"));
        }
        let storage_quota = self.storage_quota.unwrap_or(10_738_064_711);
        if !(1_000_000..=100_000_000_000).contains(&storage_quota) {
            return Err(PersonaError::field(
                "storage_quota",
                "outside supported bounds",
            ));
        }

        let webgl_vendor = self.webgl_vendor.unwrap_or_else(|| {
            if macos {
                "Google Inc. (Apple)"
            } else {
                "Google Inc. (NVIDIA)"
            }
            .to_string()
        });
        let webgl_renderer = self.webgl_renderer.unwrap_or_else(|| {
            if macos {
                "ANGLE (Apple, ANGLE Metal Renderer: Apple M5 Pro, Unspecified Version)"
            } else {
                "ANGLE (NVIDIA, NVIDIA GeForce RTX 3060 Direct3D11 vs_5_0 ps_5_0, D3D11)"
            }
            .to_string()
        });
        validate_text("webgl_vendor", &webgl_vendor, 256)?;
        validate_text("webgl_renderer", &webgl_renderer, 512)?;

        if let Some(location) = &self.geolocation {
            if !location.latitude.is_finite()
                || !location.longitude.is_finite()
                || !(-90.0..=90.0).contains(&location.latitude)
                || !(-180.0..=180.0).contains(&location.longitude)
            {
                return Err(PersonaError::field(
                    "geolocation",
                    "coordinates are outside valid bounds",
                ));
            }
        }

        let seed = stable_seed(&self.persona_id, &self.revision, profile.name());
        let mut effective = EffectivePersona {
            schema_version: PERSONA_SCHEMA_VERSION.to_string(),
            persona_id: self.persona_id,
            revision: self.revision,
            profile,
            user_agent: profile.user_agent().to_string(),
            full_version: profile.full_version().to_string(),
            platform: profile.platform().0.to_string(),
            ua_platform: profile.platform().1.to_string(),
            ua_platform_version: profile.platform().2.to_string(),
            architecture: if macos { "arm" } else { "x86" }.to_string(),
            viewport,
            language,
            languages,
            accept_language,
            timezone,
            do_not_track: self.do_not_track,
            seed,
            hardware_concurrency,
            device_memory,
            screen_width,
            screen_height,
            screen_color_depth,
            screen_avail_width,
            screen_avail_height,
            outer_width,
            outer_height,
            device_scale_factor,
            battery_charging: self.battery_charging.unwrap_or(true),
            battery_level,
            network_rtt,
            storage_quota,
            webgl_vendor,
            webgl_renderer,
            geolocation: self.geolocation,
            digest: String::new(),
        };
        effective.digest = effective.compute_digest();
        Ok(effective)
    }
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct EffectivePersona {
    schema_version: String,
    persona_id: String,
    revision: String,
    profile: StealthProfile,
    user_agent: String,
    full_version: String,
    platform: String,
    ua_platform: String,
    ua_platform_version: String,
    architecture: String,
    viewport: ViewportSpec,
    language: String,
    languages: Vec<String>,
    accept_language: String,
    timezone: String,
    do_not_track: Option<String>,
    seed: u32,
    hardware_concurrency: u32,
    device_memory: f64,
    screen_width: u32,
    screen_height: u32,
    screen_color_depth: u32,
    screen_avail_width: u32,
    screen_avail_height: u32,
    outer_width: u32,
    outer_height: u32,
    device_scale_factor: f64,
    battery_charging: bool,
    battery_level: f64,
    network_rtt: u32,
    storage_quota: u64,
    webgl_vendor: String,
    webgl_renderer: String,
    geolocation: Option<GeolocationSpec>,
    digest: String,
}

impl EffectivePersona {
    pub fn builtin(profile: StealthProfile) -> Self {
        PersonaSpec::preset(profile)
            .compile()
            .expect("built-in persona must remain valid")
    }

    pub fn schema_version(&self) -> &str {
        &self.schema_version
    }
    pub fn persona_id(&self) -> &str {
        &self.persona_id
    }
    pub fn revision(&self) -> &str {
        &self.revision
    }
    pub fn profile(&self) -> StealthProfile {
        self.profile
    }
    pub fn user_agent(&self) -> &str {
        &self.user_agent
    }
    pub fn full_version(&self) -> &str {
        &self.full_version
    }
    pub fn platform(&self) -> &str {
        &self.platform
    }
    pub fn ua_platform(&self) -> &str {
        &self.ua_platform
    }
    pub fn ua_platform_version(&self) -> &str {
        &self.ua_platform_version
    }
    pub fn architecture(&self) -> &str {
        &self.architecture
    }
    pub fn viewport(&self) -> &ViewportSpec {
        &self.viewport
    }
    pub fn language(&self) -> &str {
        &self.language
    }
    pub fn languages(&self) -> &[String] {
        &self.languages
    }
    pub fn accept_language(&self) -> &str {
        &self.accept_language
    }
    pub fn timezone(&self) -> &str {
        &self.timezone
    }
    pub fn do_not_track(&self) -> Option<&str> {
        self.do_not_track.as_deref()
    }
    pub fn seed(&self) -> u32 {
        self.seed
    }
    pub fn hardware_concurrency(&self) -> u32 {
        self.hardware_concurrency
    }
    pub fn device_memory(&self) -> f64 {
        self.device_memory
    }
    pub fn screen_width(&self) -> u32 {
        self.screen_width
    }
    pub fn screen_height(&self) -> u32 {
        self.screen_height
    }
    pub fn screen_color_depth(&self) -> u32 {
        self.screen_color_depth
    }
    pub fn screen_avail_width(&self) -> u32 {
        self.screen_avail_width
    }
    pub fn screen_avail_height(&self) -> u32 {
        self.screen_avail_height
    }
    pub fn outer_width(&self) -> u32 {
        self.outer_width
    }
    pub fn outer_height(&self) -> u32 {
        self.outer_height
    }
    pub fn device_scale_factor(&self) -> f64 {
        self.device_scale_factor
    }
    pub fn battery_charging(&self) -> bool {
        self.battery_charging
    }
    pub fn battery_level(&self) -> f64 {
        self.battery_level
    }
    pub fn network_rtt(&self) -> u32 {
        self.network_rtt
    }
    pub fn storage_quota(&self) -> u64 {
        self.storage_quota
    }
    pub fn webgl_vendor(&self) -> &str {
        &self.webgl_vendor
    }
    pub fn webgl_renderer(&self) -> &str {
        &self.webgl_renderer
    }
    pub fn geolocation(&self) -> Option<&GeolocationSpec> {
        self.geolocation.as_ref()
    }
    pub fn digest(&self) -> &str {
        &self.digest
    }

    pub fn to_spec(&self) -> PersonaSpec {
        PersonaSpec {
            schema_version: self.schema_version.clone(),
            persona_id: self.persona_id.clone(),
            revision: self.revision.clone(),
            profile: self.profile.name().to_string(),
            viewport: Some(self.viewport.clone()),
            language: Some(self.language.clone()),
            languages: Some(self.languages.clone()),
            accept_language: Some(self.accept_language.clone()),
            timezone: Some(self.timezone.clone()),
            do_not_track: self.do_not_track.clone(),
            hardware_concurrency: Some(self.hardware_concurrency),
            device_memory: Some(self.device_memory),
            screen_width: Some(self.screen_width),
            screen_height: Some(self.screen_height),
            screen_color_depth: Some(self.screen_color_depth),
            screen_avail_width: Some(self.screen_avail_width),
            screen_avail_height: Some(self.screen_avail_height),
            outer_width: Some(self.outer_width),
            outer_height: Some(self.outer_height),
            device_scale_factor: Some(self.device_scale_factor),
            battery_charging: Some(self.battery_charging),
            battery_level: Some(self.battery_level),
            network_rtt: Some(self.network_rtt),
            storage_quota: Some(self.storage_quota),
            webgl_vendor: Some(self.webgl_vendor.clone()),
            webgl_renderer: Some(self.webgl_renderer.clone()),
            geolocation: self.geolocation.clone(),
        }
    }

    pub fn preload_script(&self) -> String {
        format!(
            "globalThis.__obscura_battery_charging={};\
             globalThis.__obscura_battery_level={};\
             globalThis.__obscura_network_rtt={};\
             globalThis.__obscura_storage_quota={};\
             if(globalThis.screen){{globalThis.screen._availW={};globalThis.screen._availH={};}}\
             globalThis.outerWidth={};globalThis.outerHeight={};\
             globalThis.devicePixelRatio={};",
            self.battery_charging,
            self.battery_level,
            self.network_rtt,
            self.storage_quota,
            self.screen_avail_width,
            self.screen_avail_height,
            self.outer_width,
            self.outer_height,
            self.device_scale_factor,
        )
    }

    fn compute_digest(&self) -> String {
        let mut clone = self.clone();
        clone.digest.clear();
        let canonical = serde_json::to_vec(&clone).expect("effective persona serialization");
        format!("{:x}", Sha256::digest(canonical))
    }
}

#[derive(Debug, Error)]
pub enum PersonaError {
    #[error("invalid persona JSON: {0}")]
    Json(serde_json::Error),
    #[error("invalid persona field {field}: {reason}")]
    Field { field: &'static str, reason: String },
    #[error("unsupported persona capability: {0}")]
    Capability(String),
}

impl PersonaError {
    fn field(field: &'static str, reason: impl Into<String>) -> Self {
        Self::Field {
            field,
            reason: reason.into(),
        }
    }
}

fn validate_identifier(field: &'static str, value: &str) -> Result<(), PersonaError> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        return Err(PersonaError::field(
            field,
            "must be 1-64 ASCII letters, digits, '.', '_' or '-'",
        ));
    }
    Ok(())
}

fn validate_text(field: &'static str, value: &str, limit: usize) -> Result<(), PersonaError> {
    if value.is_empty() || value.len() > limit || value.chars().any(char::is_control) {
        return Err(PersonaError::field(
            field,
            format!("must be non-empty, control-free, and at most {limit} bytes"),
        ));
    }
    Ok(())
}

fn valid_language_tag(value: &str) -> bool {
    if value.is_empty() || value.len() > 32 || !value.is_ascii() {
        return false;
    }
    let subtags = value.split('-').collect::<Vec<_>>();
    let Some(language) = subtags.first().copied() else {
        return false;
    };
    if !(2..=8).contains(&language.len())
        || !language.bytes().all(|byte| byte.is_ascii_lowercase())
    {
        return false;
    }

    // Persona v1 intentionally supports a strict, canonical BCP47 subset:
    // language, optional Script, optional REGION, then ordinary variants.
    // Extlangs, extensions and private-use tags need explicit schema support
    // before they can be projected consistently through V8 and HTTP.
    let mut index = 1;
    if subtags.get(index).is_some_and(|subtag| {
        subtag.len() == 4
            && subtag.as_bytes()[0].is_ascii_uppercase()
            && subtag.as_bytes()[1..]
                .iter()
                .all(|byte| byte.is_ascii_lowercase())
    }) {
        index += 1;
    }
    if subtags.get(index).is_some_and(|subtag| {
        (subtag.len() == 2 && subtag.bytes().all(|byte| byte.is_ascii_uppercase()))
            || (subtag.len() == 3 && subtag.bytes().all(|byte| byte.is_ascii_digit()))
    }) {
        index += 1;
    }

    let mut variants = Vec::<&str>::new();
    for variant in &subtags[index..] {
        let valid_shape = (5..=8).contains(&variant.len())
            || (variant.len() == 4
                && variant.as_bytes()[0].is_ascii_digit());
        if !valid_shape
            || !variant
                .bytes()
                .all(|byte| byte.is_ascii_digit() || byte.is_ascii_lowercase())
            || variants.contains(variant)
        {
            return false;
        }
        variants.push(variant);
    }
    true
}

fn derive_languages(language: &str) -> Vec<String> {
    let mut result = vec![language.to_string()];
    if let Some((base, _)) = language.split_once('-') {
        result.push(base.to_string());
    }
    result
}

fn accept_language_from(languages: &[String]) -> String {
    let mut expanded = Vec::<String>::new();
    for language in languages {
        if !expanded.contains(language) {
            expanded.push(language.clone());
        }
        if let Some((base, _)) = language.split_once('-') {
            let base = base.to_string();
            if !expanded.contains(&base) {
                expanded.push(base);
            }
        }
    }
    expanded
        .into_iter()
        .enumerate()
        .map(|(index, language)| {
            if index == 0 {
                language
            } else {
                format!(
                    "{};q=0.{}",
                    language,
                    9usize.saturating_sub(index - 1).max(1)
                )
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn validate_accept_language(
    language: &str,
    languages: &[String],
    header: &str,
) -> Result<(), PersonaError> {
    if header.is_empty() || header.len() > 256 || header.contains(['\r', '\n']) {
        return Err(PersonaError::field(
            "accept_language",
            "invalid header value",
        ));
    }
    let mut tags = Vec::new();
    for (index, part) in header.split(',').enumerate() {
        let mut fields = part.trim().split(';');
        let tag = fields.next().unwrap_or("").trim();
        if !valid_language_tag(tag) {
            return Err(PersonaError::field(
                "accept_language",
                "contains an invalid language tag",
            ));
        }
        let parameters = fields.collect::<Vec<_>>();
        if index == 0 && !parameters.is_empty() {
            return Err(PersonaError::field(
                "accept_language",
                "first language must not have a quality parameter",
            ));
        }
        if parameters.len() > 1
            || parameters
                .first()
                .is_some_and(|parameter| !valid_accept_language_quality(parameter.trim()))
        {
            return Err(PersonaError::field(
                "accept_language",
                "contains an invalid quality parameter",
            ));
        }
        tags.push(tag);
    }
    if tags.first().copied() != Some(language) {
        return Err(PersonaError::field(
            "accept_language",
            "first tag must equal language",
        ));
    }
    let mut offset = 0;
    for language in languages {
        let Some(index) = tags[offset..].iter().position(|tag| *tag == language) else {
            return Err(PersonaError::field(
                "accept_language",
                "must contain navigator.languages in order",
            ));
        };
        offset += index + 1;
    }
    Ok(())
}

fn valid_accept_language_quality(parameter: &str) -> bool {
    let Some(value) = parameter.strip_prefix("q=") else {
        return false;
    };
    if value == "0" || value == "1" {
        return true;
    }
    if let Some(decimal) = value.strip_prefix("0.") {
        return !decimal.is_empty()
            && decimal.len() <= 3
            && decimal.bytes().all(|byte| byte.is_ascii_digit());
    }
    if let Some(decimal) = value.strip_prefix("1.") {
        return !decimal.is_empty()
            && decimal.len() <= 3
            && decimal.bytes().all(|byte| byte == b'0');
    }
    false
}

fn stable_seed(persona_id: &str, revision: &str, profile: &str) -> u32 {
    let mut hash = Sha256::new();
    hash.update(b"obscura-persona-v1\0");
    hash.update(persona_id.as_bytes());
    hash.update(b"\0");
    hash.update(revision.as_bytes());
    hash.update(b"\0");
    hash.update(profile.as_bytes());
    u32::from_be_bytes(hash.finalize()[..4].try_into().expect("sha256 prefix"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_are_complete_and_stable() {
        for profile in [
            StealthProfile::WindowsChrome145,
            StealthProfile::MacChrome152,
            StealthProfile::MacChrome153,
        ] {
            let first = EffectivePersona::builtin(profile);
            let second = EffectivePersona::builtin(profile);
            assert_eq!(first, second);
            assert_eq!(first.user_agent(), profile.user_agent());
            assert_eq!(first.profile(), profile);
            assert_eq!(first.digest().len(), 64);
            if profile == StealthProfile::WindowsChrome145 {
                assert_eq!(first.timezone(), "Europe/Berlin");
            }
        }
    }

    #[test]
    fn external_identity_uses_the_same_compiler() {
        let mut spec = PersonaSpec::preset(StealthProfile::MacChrome153);
        spec.persona_id = "customer_a".to_string();
        spec.revision = "2026-09-20".to_string();
        spec.language = Some("fr-CA".to_string());
        let effective = spec.compile().unwrap();
        assert_eq!(effective.persona_id(), "customer_a");
        assert_eq!(effective.languages(), ["fr-CA", "fr"]);
        assert_eq!(effective.accept_language(), "fr-CA,fr;q=0.9");
    }

    #[test]
    fn screen_color_depth_is_validated_and_round_trips() {
        let mut spec = PersonaSpec::preset(StealthProfile::MacChrome153);
        assert_eq!(spec.clone().compile().unwrap().screen_color_depth(), 24);
        spec.screen_color_depth = Some(30);
        let persona = spec.compile().unwrap();
        assert_eq!(persona.screen_color_depth(), 30);
        assert_eq!(persona.to_spec().screen_color_depth, Some(30));
        let mut invalid = PersonaSpec::preset(StealthProfile::MacChrome153);
        invalid.screen_color_depth = Some(16);
        assert!(matches!(invalid.compile(), Err(PersonaError::Field { field: "screen_color_depth", .. })));
        let mut window = PersonaSpec::preset(StealthProfile::MacChrome153);
        window.screen_width = Some(1365);
        window.screen_height = Some(768);
        window.outer_width = Some(1367);
        window.outer_height = Some(848);
        let window = window.compile().unwrap();
        assert_eq!((window.outer_width(), window.outer_height()), (1367, 848));
    }

    #[test]
    fn unsupported_profile_and_inconsistent_locale_fail_closed() {
        let mut unknown = PersonaSpec::preset(StealthProfile::MacChrome153);
        unknown.profile = "linux_chrome153".to_string();
        assert!(matches!(
            unknown.compile(),
            Err(PersonaError::Capability(_))
        ));

        let mut inconsistent = PersonaSpec::preset(StealthProfile::MacChrome153);
        inconsistent.language = Some("en-US".to_string());
        inconsistent.languages = Some(vec!["zh-CN".to_string()]);
        assert!(matches!(
            inconsistent.compile(),
            Err(PersonaError::Field { .. })
        ));

        for language in [
            "not a locale",
            "en_US",
            "e",
            "en--US",
            "en-US-US",
            "en-us",
            "en-US-u-ca-gregory",
            "x-private",
        ] {
            let mut invalid = PersonaSpec::preset(StealthProfile::WindowsChrome145);
            invalid.language = Some(language.to_string());
            assert!(
                matches!(invalid.compile(), Err(PersonaError::Field { .. })),
                "must reject {language:?}",
            );
        }

        for language in ["en", "en-US", "zh-Hant-TW", "de-CH-1901", "sl-rozaj"] {
            let mut valid = PersonaSpec::preset(StealthProfile::WindowsChrome145);
            valid.language = Some(language.to_string());
            assert!(valid.compile().is_ok(), "must accept {language:?}");
        }

        for header in [
            "en-US;q=0.9,en;q=0.8",
            "en-US,en;q=2",
            "en-US,en;level=1",
            "en-US,not a locale;q=0.9",
        ] {
            let mut invalid = PersonaSpec::preset(StealthProfile::WindowsChrome145);
            invalid.accept_language = Some(header.to_string());
            assert!(
                matches!(invalid.compile(), Err(PersonaError::Field { .. })),
                "must reject {header:?}",
            );
        }

        let mut invalid_timezone = PersonaSpec::preset(StealthProfile::MacChrome153);
        invalid_timezone.timezone = Some("Not/A_Real_Zone".to_string());
        assert!(matches!(
            invalid_timezone.compile(),
            Err(PersonaError::Field { .. })
        ));
    }

    #[test]
    fn process_identity_is_frozen_by_the_first_persona() {
        let first = EffectivePersona::builtin(StealthProfile::WindowsChrome145);
        activate_process_persona(&first).unwrap();
        activate_process_persona(&first).unwrap();

        let second = EffectivePersona::builtin(StealthProfile::MacChrome153);
        assert!(matches!(
            activate_process_persona(&second),
            Err(PersonaError::Capability(_))
        ));

        let mut different_locale = PersonaSpec::preset(StealthProfile::WindowsChrome145);
        different_locale.language = Some("fr-CA".to_string());
        let different_locale = different_locale.compile().unwrap();
        assert!(matches!(
            activate_process_persona(&different_locale),
            Err(PersonaError::Capability(_))
        ));
    }
}
