//! Strict private child protocol. Native identifiers never serialize to author JS.
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use crate::speech_provider::ProviderError;
pub(crate) const MAX_OUTPUT: usize = 4 * 1024 * 1024;
const MAX_STRING: usize = 16384;
const MAX_VOICES: usize = 4096;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct WebVoice {
    #[serde(rename = "voiceURI")]
    pub voice_uri: String,
    pub name: String,
    pub lang: String,
    #[serde(rename = "localService")]
    pub local_service: bool,
    #[serde(rename = "default")]
    pub is_default: bool,
}
#[derive(Debug)]
pub(crate) struct NativeVoice { pub native_identifier: String, pub web: WebVoice }
#[derive(Debug)]
pub(crate) struct Snapshot { pub voices: Arc<[NativeVoice]>, pub default_branch: String }
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawVoice { native_identifier: String, raw_name: String, language: String }
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MappedVoice {
    native_identifier: String, raw_name: String, language: String,
    native: bool, remote: bool, name_count: usize,
    localized_language: Option<String>, web: WebVoice,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DefaultSelection { branch: String, native_identifier: Option<String>, removed_equal_av_objects: usize }
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Payload {
    schema: u32, status: String, main_thread: bool, main_run_loop: bool,
    chromium_revision: String, mapping_scope: String, default_selection: DefaultSelection,
    locale_before: String, locale_after: String, locale_identifier_changed: bool,
    native_count: usize, record_count: usize, skipped_nil_name: usize,
    ordered_skipped_nil_name: usize, mapped_count: usize, string_bytes_charged: usize,
    synthesis: String, preferences: String, voices: Vec<RawVoice>, mapped_voices: Vec<MappedVoice>,
}
fn field(value: &str) -> bool { value.len() <= MAX_STRING }
pub(crate) fn decode(bytes: &[u8]) -> Result<Arc<Snapshot>, ProviderError> {
    if bytes.len() > MAX_OUTPUT { return Err(ProviderError::OutputLimit); }
    let p: Payload = serde_json::from_slice(bytes).map_err(|_| ProviderError::Protocol)?;
    decode_payload(p, "settled_native_only_not_extension_or_first_async_inventory", false)
}
fn decode_payload(p: Payload, scope: &str, pending: bool) -> Result<Arc<Snapshot>, ProviderError> {
    let fail = || ProviderError::Protocol;
    if p.schema != 2 || p.status != "native_mapping_observed" || !p.main_thread || !p.main_run_loop
        || p.chromium_revision != "792bf6722e73a45aa9e47c163b9901bdc17f3230"
        || p.mapping_scope != scope
        || p.synthesis != "not_invoked" || p.preferences != "read_only_chromium_default_chain"
        || p.native_count > MAX_VOICES || p.record_count != p.voices.len()
        || p.mapped_count != p.mapped_voices.len() || p.mapped_count > MAX_VOICES
        || p.record_count.checked_add(p.skipped_nil_name) != Some(p.native_count)
        || p.string_bytes_charged > MAX_OUTPUT || !field(&p.locale_before) || !field(&p.locale_after)
        || p.locale_identifier_changed != (p.locale_before != p.locale_after) { return Err(fail()); }
    if p.locale_identifier_changed { return Err(ProviderError::LocaleChanged); }
    let has_default = p.default_selection.native_identifier.is_some();
    if !(matches!(p.default_selection.branch.as_str(), "none" | "accessibility_voice_id" | "ns_speech_default_voice" | "av_system_language_region")
            || (pending && p.default_selection.branch == "pending"))
        || (pending && (p.default_selection.branch != "pending" || has_default))
        || matches!(p.default_selection.branch.as_str(), "none" | "pending") == has_default
        || p.default_selection.native_identifier.as_deref().is_some_and(|v| !field(v))
        || (!has_default && p.default_selection.removed_equal_av_objects != 0) { return Err(fail()); }
    let expected = p.native_count.checked_sub(p.default_selection.removed_equal_av_objects)
        .and_then(|v| v.checked_add(usize::from(has_default)))
        .and_then(|v| v.checked_sub(p.ordered_skipped_nil_name));
    if expected != Some(p.mapped_count) { return Err(fail()); }
    for row in &p.voices {
        if !field(&row.native_identifier) || !field(&row.raw_name) || !field(&row.language) { return Err(fail()); }
    }
    let originals: HashSet<_> = p.voices.iter().map(|row| (row.native_identifier.as_str(), row.raw_name.as_str(), row.language.as_str())).collect();
    let mut counts = HashMap::<&str, usize>::new();
    for row in &p.mapped_voices { *counts.entry(row.raw_name.as_str()).or_default() += 1; }
    let mut voices = Vec::with_capacity(p.mapped_count);
    for (index, row) in p.mapped_voices.iter().enumerate() {
        if !field(&row.native_identifier) || !field(&row.raw_name) || !field(&row.language)
            || row.localized_language.as_deref().is_some_and(|v| !field(v))
            || !field(&row.web.name) || !field(&row.web.voice_uri) || !field(&row.web.lang)
            || !row.native || row.remote || !row.web.local_service || row.web.is_default != (index == 0)
            || row.web.lang != row.language || row.web.voice_uri != row.web.name
            || counts.get(row.raw_name.as_str()).copied() != Some(row.name_count) { return Err(fail()); }
        let from_inventory = originals.contains(&(row.native_identifier.as_str(), row.raw_name.as_str(), row.language.as_str()));
        if !from_inventory && p.default_selection.native_identifier.as_deref() != Some(row.native_identifier.as_str()) {
            return Err(fail());
        }
        let expected_name = if row.name_count > 1 {
            format!("{} ({})", row.raw_name, row.localized_language.as_deref().unwrap_or("(null)"))
        } else {
            if row.localized_language.is_some() { return Err(fail()); }
            row.raw_name.clone()
        };
        if expected_name != row.web.name { return Err(fail()); }
        voices.push(NativeVoice { native_identifier: row.native_identifier.clone(), web: row.web.clone() });
    }
    Ok(Arc::new(Snapshot { voices: voices.into(), default_branch: p.default_selection.branch }))
}


#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResolvedSelection { branch: String, native_identifier: Option<String> }
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Transition { previous_identifier: Option<String>, object_equal: bool, identifier_equal: Option<bool> }
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum StreamFrame {
    Initial { schema: u32, revision: u32, default_state: String, query_pending: bool,
        queries_completed: u32, snapshot: Payload },
    DefaultChanged { schema: u32, revision: u32, default_state: String, query_pending: bool,
        queries_completed: u32, transition: Transition, snapshot: Payload },
    Terminal { schema: u32, next_revision: u32, queries_started: u32, queries_completed: u32,
        default_background_thread: bool, default_selection: ResolvedSelection },
}
#[derive(Clone, Debug)]
pub(crate) struct ResolvedDefault { pub branch: String, pub native_identifier: Option<String> }
pub(crate) enum StreamEvent { Snapshot(Arc<Snapshot>), Terminal(ResolvedDefault) }
#[derive(Default)]
pub(crate) struct StreamDecoder {
    next_revision: u32,
    last_identifier: Option<String>,
    last_queries_completed: u32,
    last_query_pending: bool,
    terminal: bool,
}
impl StreamDecoder {
    pub(crate) fn accept(&mut self, line: &[u8]) -> Result<StreamEvent, ProviderError> {
        if self.terminal || line.len() > MAX_OUTPUT { return Err(ProviderError::Protocol); }
        let frame: StreamFrame = serde_json::from_slice(line).map_err(|_| ProviderError::Protocol)?;
        match frame {
            StreamFrame::Initial { schema: 4, revision: 0, default_state, query_pending,
                queries_completed, snapshot } if self.next_revision == 0 => {
                let pending = match default_state.as_str() { "pending" => true, "resolved" => false, _ => return Err(ProviderError::Protocol) };
                if pending != (queries_completed == 0) || (pending && !query_pending) { return Err(ProviderError::Protocol); }
                let identifier = snapshot.default_selection.native_identifier.clone();
                let value = decode_payload(snapshot, "startup_current_default", pending)?;
                self.last_identifier = identifier; self.last_queries_completed = queries_completed;
                self.last_query_pending = query_pending; self.next_revision = 1;
                Ok(StreamEvent::Snapshot(value))
            }
            StreamFrame::DefaultChanged { schema: 4, revision, default_state, query_pending,
                queries_completed, transition, snapshot } if self.next_revision > 0 && revision == self.next_revision => {
                if !self.last_query_pending || default_state != "resolved" || queries_completed <= self.last_queries_completed
                    || transition.previous_identifier != self.last_identifier || transition.object_equal {
                    return Err(ProviderError::Protocol);
                }
                let identifier = snapshot.default_selection.native_identifier.clone();
                match (&self.last_identifier, &identifier) {
                    (None, None) => return Err(ProviderError::Protocol), // nil->nil is not a change
                    (Some(before), Some(after)) => {
                        if transition.identifier_equal != Some(before == after) || before == after { return Err(ProviderError::Protocol); }
                    }
                    _ => if transition.identifier_equal.is_some() { return Err(ProviderError::Protocol); },
                }
                let value = decode_payload(snapshot, "startup_current_default", false)?;
                self.last_identifier = identifier; self.last_queries_completed = queries_completed;
                self.last_query_pending = query_pending;
                self.next_revision = self.next_revision.checked_add(1).ok_or(ProviderError::Protocol)?;
                Ok(StreamEvent::Snapshot(value))
            }
            StreamFrame::Terminal { schema: 4, next_revision, queries_started, queries_completed,
                default_background_thread: true, default_selection }
                if self.next_revision > 0 && next_revision == self.next_revision => {
                let has_default = default_selection.native_identifier.is_some();
                if queries_started != queries_completed || queries_completed < 2
                    || (self.last_query_pending && queries_completed <= self.last_queries_completed)
                    || (!self.last_query_pending && queries_completed != self.last_queries_completed)
                    || default_selection.native_identifier != self.last_identifier
                    || !matches!(default_selection.branch.as_str(), "none" | "accessibility_voice_id" | "ns_speech_default_voice" | "av_system_language_region")
                    || (default_selection.branch == "none") == has_default
                    || default_selection.native_identifier.as_deref().is_some_and(|v| !field(v)) { return Err(ProviderError::Protocol); }
                self.terminal = true;
                Ok(StreamEvent::Terminal(ResolvedDefault { branch: default_selection.branch, native_identifier: default_selection.native_identifier }))
            }
            _ => Err(ProviderError::Protocol),
        }
    }
    pub(crate) fn finish(&self) -> Result<(), ProviderError> {
        if self.terminal { Ok(()) } else { Err(ProviderError::Protocol) }
    }
}
