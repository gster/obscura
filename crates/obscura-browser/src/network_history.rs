//! Context-owned, append-only Network observation history.
//!
//! Live CDP delivery and Page state are intentionally not the authority here.
//! A browser context owns one history, every page receives a never-reused page
//! instance id, and accepted records remain queryable after navigation and Page
//! close. Persistent histories use a checksummed, length-framed journal. An
//! append is accepted only after the complete frame has been written and
//! `sync_data` has succeeded.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::page::NetworkEvent;

pub const NETWORK_HISTORY_SCHEMA_VERSION: u32 = 1;
const JOURNAL_MAGIC: &[u8; 8] = b"OBNHJ001";
const JOURNAL_HEADER_BYTES: usize = JOURNAL_MAGIC.len() + 8 + 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NetworkHistoryLimits {
    pub records: usize,
    pub metadata_bytes: usize,
    pub single_record_bytes: usize,
    pub body_bytes: usize,
    pub body_entries: usize,
    pub disk_bytes: usize,
}

impl Default for NetworkHistoryLimits {
    fn default() -> Self {
        fn limit(name: &str, default: usize) -> usize {
            std::env::var(name)
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(default)
        }
        Self {
            records: limit("OBSCURA_NETWORK_HISTORY_RECORDS", 4096),
            metadata_bytes: limit(
                "OBSCURA_NETWORK_HISTORY_METADATA_BYTES",
                64 * 1024 * 1024,
            ),
            single_record_bytes: limit(
                "OBSCURA_NETWORK_HISTORY_SINGLE_RECORD_BYTES",
                16 * 1024 * 1024,
            ),
            body_bytes: limit(
                "OBSCURA_NETWORK_HISTORY_BODY_BYTES",
                512 * 1024 * 1024,
            ),
            body_entries: limit("OBSCURA_NETWORK_HISTORY_BODY_ENTRIES", 32768),
            disk_bytes: limit(
                "OBSCURA_NETWORK_HISTORY_DISK_BYTES",
                640 * 1024 * 1024,
            ),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NetworkHistoryId(pub String);

#[derive(Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PageInstanceId(pub String);

#[derive(Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HistoryBodyKey(pub String);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HistoryBodyKind {
    Request,
    TransportRequest,
    Response,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnedRawHeader {
    #[serde(rename = "nameBase64", with = "base64_bytes")]
    pub name: Vec<u8>,
    #[serde(rename = "valueBase64", with = "base64_bytes")]
    pub value: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnedHeaderCapture {
    pub capture_stage: String,
    pub encoding: String,
    pub fields: Vec<OwnedRawHeader>,
}

impl From<&obscura_net::HeaderCapture> for OwnedHeaderCapture {
    fn from(value: &obscura_net::HeaderCapture) -> Self {
        Self {
            capture_stage: value.capture_stage.to_string(),
            encoding: value.encoding.to_string(),
            fields: value
                .fields
                .iter()
                .map(|field| OwnedRawHeader {
                    name: field.name.clone(),
                    value: field.value.clone(),
                })
                .collect(),
        }
    }
}

/// Owned, versioned history representation. Compatibility header maps are
/// retained, but raw captures remain the authoritative byte-exact fields.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnedNetworkEvent {
    pub document_generation: u64,
    pub document_url: String,
    pub initiator_request_id: Option<String>,
    pub retired_document_url: Option<String>,
    pub pending: bool,
    pub error: Option<String>,
    pub request_body_present: bool,
    pub request_body_request_id: Option<String>,
    pub request_body_size: usize,
    pub transport_request_body_present: bool,
    pub transport_request_body_request_id: Option<String>,
    pub transport_request_body_size: usize,
    pub request_started: bool,
    pub redirect: bool,
    pub response_body_request_id: Option<String>,
    #[serde(default)]
    pub response_body_capture_error: Option<String>,
    pub request_id: String,
    pub url: String,
    pub method: String,
    pub resource_type: String,
    pub status: u16,
    pub status_text: String,
    pub headers: HashMap<String, String>,
    pub response_headers: HashMap<String, String>,
    pub raw_headers: Option<OwnedHeaderCapture>,
    pub request_raw_headers: Option<OwnedHeaderCapture>,
    pub body_size: usize,
    pub timestamp: f64,
}

impl From<&NetworkEvent> for OwnedNetworkEvent {
    fn from(value: &NetworkEvent) -> Self {
        Self {
            document_generation: value.document_generation,
            document_url: value.document_url.clone(),
            initiator_request_id: value.initiator_request_id.clone(),
            retired_document_url: value.retired_document_url.clone(),
            pending: value.pending,
            error: value.error.clone(),
            request_body_present: value.request_body_present,
            request_body_request_id: value.request_body_request_id.clone(),
            request_body_size: value.request_body_size,
            transport_request_body_present: value.transport_request_body_present,
            transport_request_body_request_id: value
                .transport_request_body_request_id
                .clone(),
            transport_request_body_size: value.transport_request_body_size,
            request_started: value.request_started,
            redirect: value.redirect,
            response_body_request_id: value.response_body_request_id.clone(),
            response_body_capture_error: value.response_body_capture_error.clone(),
            request_id: value.request_id.clone(),
            url: value.url.clone(),
            method: value.method.clone(),
            resource_type: value.resource_type.clone(),
            status: value.status,
            status_text: value.status_text.clone(),
            headers: value.headers.clone(),
            response_headers: (*value.response_headers).clone(),
            raw_headers: value.raw_headers.as_ref().map(Into::into),
            request_raw_headers: value.request_raw_headers.as_ref().map(Into::into),
            body_size: value.body_size,
            timestamp: value.timestamp,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryBodyRef {
    pub key: HistoryBodyKey,
    pub local_id: String,
    pub kind: HistoryBodyKind,
    pub version: u64,
    pub size: usize,
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkHistoryRecord {
    pub schema_version: u32,
    pub sequence: u64,
    pub history_id: NetworkHistoryId,
    pub page_instance_id: PageInstanceId,
    pub display_page_id: String,
    pub event: OwnedNetworkEvent,
    pub request_body: Option<HistoryBodyRef>,
    pub transport_request_body: Option<HistoryBodyRef>,
    pub response_body: Option<HistoryBodyRef>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NetworkHistoryFailureKind {
    Count,
    MetadataBytes,
    RecordBytes,
    BodyBytes,
    BodyEntries,
    DiskBytes,
    BodyMissing,
    Serialization,
    Io,
    Recovery,
    Producer,
    Closed,
    ReadOnly,
    NotFound,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, thiserror::Error)]
#[error("{message}")]
#[serde(rename_all = "camelCase")]
pub struct NetworkHistoryError {
    pub kind: NetworkHistoryFailureKind,
    pub message: String,
    pub resource: Option<String>,
    pub limit: Option<usize>,
    pub attempted: Option<usize>,
    pub last_accepted_sequence: u64,
    pub page_instance_id: Option<PageInstanceId>,
    pub request_id: Option<String>,
}

impl NetworkHistoryError {
    fn transient(kind: NetworkHistoryFailureKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            resource: None,
            limit: None,
            attempted: None,
            last_accepted_sequence: 0,
            page_instance_id: None,
            request_id: None,
        }
    }
}

#[derive(Clone)]
enum HistoryBodySource {
    Bytes(Vec<u8>),
    ResponseBody(obscura_net::response_body::ResponseBody),
}

/// A body to commit with one history record. The source is read only during
/// admission; callers may pass an existing spooled ResponseBody without doing
/// their own serialization.
#[derive(Clone)]
pub struct HistoryBodyCandidate {
    pub local_id: String,
    source: HistoryBodySource,
}

impl HistoryBodyCandidate {
    pub fn from_bytes(local_id: impl Into<String>, bytes: impl Into<Vec<u8>>) -> Self {
        Self { local_id: local_id.into(), source: HistoryBodySource::Bytes(bytes.into()) }
    }

    pub fn from_response_body(
        local_id: impl Into<String>,
        body: obscura_net::response_body::ResponseBody,
    ) -> Self {
        Self { local_id: local_id.into(), source: HistoryBodySource::ResponseBody(body) }
    }

    fn exact_bytes(&self) -> Result<Vec<u8>, NetworkHistoryError> {
        match &self.source {
            HistoryBodySource::Bytes(bytes) => Ok(bytes.clone()),
            HistoryBodySource::ResponseBody(body) => body
                .with_bytes(|bytes| bytes.to_vec())
                .map_err(|error| NetworkHistoryError::transient(
                    NetworkHistoryFailureKind::Io,
                    format!("network history body read failed: {error}"),
                )),
        }
    }
}

pub struct NetworkHistoryCandidate {
    pub event: OwnedNetworkEvent,
    pub request_body: Option<HistoryBodyCandidate>,
    pub transport_request_body: Option<HistoryBodyCandidate>,
    pub response_body: Option<HistoryBodyCandidate>,
}

impl NetworkHistoryCandidate {
    pub fn new(event: OwnedNetworkEvent) -> Self {
        Self {
            event,
            request_body: None,
            transport_request_body: None,
            response_body: None,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct NetworkHistoryQuery {
    pub after_sequence: u64,
    pub limit: usize,
    pub page_instance_id: Option<PageInstanceId>,
}

#[derive(Clone, Debug)]
pub struct NetworkHistoryPage {
    pub history_id: NetworkHistoryId,
    pub records: Vec<NetworkHistoryRecord>,
    pub next_sequence: u64,
    pub terminal_failure: Option<NetworkHistoryError>,
    pub finalized: bool,
    pub pages: BTreeMap<PageInstanceId, String>,
    pub closed_pages: BTreeSet<PageInstanceId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoryBodyChunk {
    pub bytes: Vec<u8>,
    pub offset: usize,
    pub total_size: usize,
    pub eof: bool,
}

#[derive(Clone)]
pub struct NetworkHistory {
    inner: Arc<Mutex<Inner>>,
}

#[derive(Clone)]
pub struct PageHistoryWriter {
    history: NetworkHistory,
    page_instance_id: PageInstanceId,
    display_page_id: String,
}

struct Inner {
    history_id: NetworkHistoryId,
    context_id: String,
    limits: NetworkHistoryLimits,
    next_sequence: u64,
    next_page_instance: u64,
    records: Vec<NetworkHistoryRecord>,
    pages: BTreeMap<PageInstanceId, String>,
    closed_pages: BTreeSet<PageInstanceId>,
    body_versions: HashMap<(PageInstanceId, HistoryBodyKind, String), u64>,
    body_index: HashMap<HistoryBodyKey, String>,
    blobs: HashMap<String, Arc<Vec<u8>>>,
    metadata_bytes: usize,
    body_bytes: usize,
    body_entries: usize,
    disk_bytes: usize,
    terminal: Option<NetworkHistoryError>,
    finalized: bool,
    read_only: bool,
    directory: Option<PathBuf>,
    journal: Option<File>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    schema_version: u32,
    history_id: NetworkHistoryId,
    context_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JournalFrame {
    schema_version: u32,
    history_id: NetworkHistoryId,
    page_registrations: Vec<PageRegistration>,
    records: Vec<NetworkHistoryRecord>,
    blobs: Vec<PersistedBlob>,
    page_closures: Vec<PageInstanceId>,
    context_closed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    terminal_failure: Option<NetworkHistoryError>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageRegistration {
    page_instance_id: PageInstanceId,
    display_page_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PersistedBlob {
    sha256: String,
    #[serde(with = "base64_bytes")]
    bytes: Vec<u8>,
}

impl NetworkHistory {
    pub fn ephemeral(context_id: impl Into<String>) -> Self {
        Self::new_inner(context_id.into(), NetworkHistoryLimits::default(), None, false)
    }

    pub fn with_limits(context_id: impl Into<String>, limits: NetworkHistoryLimits) -> Self {
        Self::new_inner(context_id.into(), limits, None, false)
    }

    pub fn persistent(
        context_id: impl Into<String>,
        storage_dir: &Path,
    ) -> Result<Self, NetworkHistoryError> {
        Self::persistent_with_limits(context_id, storage_dir, NetworkHistoryLimits::default())
    }

    pub fn persistent_with_limits(
        context_id: impl Into<String>,
        storage_dir: &Path,
        limits: NetworkHistoryLimits,
    ) -> Result<Self, NetworkHistoryError> {
        let context_id = context_id.into();
        let history_id = NetworkHistoryId(uuid::Uuid::new_v4().to_string());
        let manifest = Manifest {
            schema_version: NETWORK_HISTORY_SCHEMA_VERSION,
            history_id: history_id.clone(),
            context_id: context_id.clone(),
        };
        let manifest_bytes = serde_json::to_vec(&manifest).map_err(|error| {
            NetworkHistoryError::transient(
                NetworkHistoryFailureKind::Serialization,
                format!("network history manifest serialization failed: {error}"),
            )
        })?;
        if manifest_bytes.len() > limits.disk_bytes {
            return Err(NetworkHistoryError {
                kind: NetworkHistoryFailureKind::DiskBytes,
                message: format!(
                    "network history diskBytes limit {}, attempted {}",
                    limits.disk_bytes,
                    manifest_bytes.len()
                ),
                resource: Some("diskBytes".to_string()),
                limit: Some(limits.disk_bytes),
                attempted: Some(manifest_bytes.len()),
                last_accepted_sequence: 0,
                page_instance_id: None,
                request_id: None,
            });
        }
        let root = storage_dir.join("network-history");
        secure_create_dir(&root)?;
        let directory = root.join(&history_id.0);
        secure_create_dir(&directory)?;
        sync_directory(&root)?;
        let manifest_path = directory.join("manifest.json");
        let mut manifest_file = secure_create_file(&manifest_path)?;
        manifest_file.write_all(&manifest_bytes).map_err(io_error)?;
        manifest_file.sync_data().map_err(io_error)?;
        let journal_path = directory.join("journal.log");
        let journal = secure_create_file(&journal_path)?;
        journal.sync_data().map_err(io_error)?;
        sync_directory(&directory)?;

        let history = Self::new_inner(context_id, limits, Some((directory, journal)), false);
        {
            let mut inner = history.inner.lock().unwrap_or_else(|error| error.into_inner());
            inner.history_id = history_id;
            inner.disk_bytes = manifest_bytes.len();
        }
        Ok(history)
    }

    /// Construct an in-memory terminal history when persistence setup failed.
    /// The BrowserContext remains usable, but observation producers receive the
    /// same explicit sticky failure rather than silently falling back.
    pub(crate) fn failed(
        context_id: impl Into<String>,
        error: NetworkHistoryError,
    ) -> Self {
        let history = Self::ephemeral(context_id);
        history.inner.lock().unwrap_or_else(|failure| failure.into_inner()).terminal = Some(error);
        history
    }

    fn new_inner(
        context_id: String,
        limits: NetworkHistoryLimits,
        storage: Option<(PathBuf, File)>,
        read_only: bool,
    ) -> Self {
        let history_id = NetworkHistoryId(uuid::Uuid::new_v4().to_string());
        let (directory, journal) = match storage {
            Some((directory, journal)) => (Some(directory), Some(journal)),
            None => (None, None),
        };
        Self {
            inner: Arc::new(Mutex::new(Inner {
                history_id,
                context_id,
                limits,
                next_sequence: 0,
                next_page_instance: 0,
                records: Vec::new(),
                pages: BTreeMap::new(),
                closed_pages: BTreeSet::new(),
                body_versions: HashMap::new(),
                body_index: HashMap::new(),
                blobs: HashMap::new(),
                metadata_bytes: 0,
                body_bytes: 0,
                body_entries: 0,
                disk_bytes: 0,
                terminal: None,
                finalized: false,
                read_only,
                directory,
                journal,
            })),
        }
    }

    pub fn id(&self) -> NetworkHistoryId {
        self.inner.lock().unwrap_or_else(|error| error.into_inner()).history_id.clone()
    }

    pub fn context_id(&self) -> String {
        self.inner.lock().unwrap_or_else(|error| error.into_inner()).context_id.clone()
    }

    pub fn storage_path(&self) -> Option<PathBuf> {
        self.inner.lock().unwrap_or_else(|error| error.into_inner()).directory.clone()
    }

    pub fn register_page(
        &self,
        display_page_id: impl Into<String>,
    ) -> Result<PageHistoryWriter, NetworkHistoryError> {
        let display_page_id = display_page_id.into();
        let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        ensure_open(&inner)?;
        if let Some(failure) = &inner.terminal {
            return Err(failure.clone());
        }
        let attempted_pages = inner.pages.len().checked_add(1).unwrap_or(usize::MAX);
        if attempted_pages > inner.limits.records {
            let limit = inner.limits.records;
            return Err(fail_locked(
                &mut inner,
                NetworkHistoryFailureKind::Count,
                "pageInstances",
                limit,
                attempted_pages,
                None,
                None,
            ));
        }
        let number = inner.next_page_instance.checked_add(1).ok_or_else(|| {
            fail_locked(
                &mut inner,
                NetworkHistoryFailureKind::Count,
                "pageInstances",
                usize::MAX,
                usize::MAX,
                None,
                None,
            )
        })?;
        let page_instance_id = PageInstanceId(format!("{}-page-{number}", inner.history_id.0));
        let frame = JournalFrame {
            schema_version: NETWORK_HISTORY_SCHEMA_VERSION,
            history_id: inner.history_id.clone(),
            page_registrations: vec![PageRegistration {
                page_instance_id: page_instance_id.clone(),
                display_page_id: display_page_id.clone(),
            }],
            records: Vec::new(),
            blobs: Vec::new(),
            page_closures: Vec::new(),
            context_closed: false,
            terminal_failure: None,
        };
        persist_control_frame(&mut inner, &frame)?;
        inner.next_page_instance = number;
        inner.pages.insert(page_instance_id.clone(), display_page_id.clone());
        drop(inner);
        Ok(PageHistoryWriter {
            history: self.clone(),
            page_instance_id,
            display_page_id,
        })
    }

    pub fn query(&self, query: NetworkHistoryQuery) -> NetworkHistoryPage {
        let inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        let limit = if query.limit == 0 { 100 } else { query.limit };
        let records = inner
            .records
            .iter()
            .filter(|record| {
                record.sequence > query.after_sequence
                    && query.page_instance_id.as_ref().is_none_or(|page| {
                        &record.page_instance_id == page
                    })
            })
            .take(limit)
            .cloned()
            .collect::<Vec<_>>();
        let next_sequence = records
            .last()
            .map(|record| record.sequence)
            .unwrap_or(query.after_sequence);
        NetworkHistoryPage {
            history_id: inner.history_id.clone(),
            records,
            next_sequence,
            terminal_failure: inner.terminal.clone(),
            finalized: inner.finalized,
            pages: inner.pages.clone(),
            closed_pages: inner.closed_pages.clone(),
        }
    }

    pub fn read_body(
        &self,
        key: &HistoryBodyKey,
        offset: usize,
        length: usize,
    ) -> Result<HistoryBodyChunk, NetworkHistoryError> {
        let inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        let sha = inner.body_index.get(key).ok_or_else(|| {
            NetworkHistoryError::transient(
                NetworkHistoryFailureKind::NotFound,
                format!("network history body not found: {}", key.0),
            )
        })?;
        let bytes = inner.blobs.get(sha).ok_or_else(|| {
            NetworkHistoryError::transient(
                NetworkHistoryFailureKind::Recovery,
                format!("network history blob missing for body {}", key.0),
            )
        })?;
        let start = offset.min(bytes.len());
        let end = start.saturating_add(length).min(bytes.len());
        Ok(HistoryBodyChunk {
            bytes: bytes[start..end].to_vec(),
            offset: start,
            total_size: bytes.len(),
            eof: end == bytes.len(),
        })
    }

    pub fn terminal_failure(&self) -> Option<NetworkHistoryError> {
        self.inner.lock().unwrap_or_else(|error| error.into_inner()).terminal.clone()
    }

    /// Stop this context's observation history after a producer or teardown
    /// failure which happened outside journal admission. The first failure is
    /// sticky and the already committed prefix remains authoritative.
    pub fn fail(
        &self,
        kind: NetworkHistoryFailureKind,
        message: impl Into<String>,
        page_instance_id: Option<PageInstanceId>,
        request_id: Option<String>,
    ) -> NetworkHistoryError {
        let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        fail_message_locked(&mut inner, kind, message, page_instance_id, request_id)
    }

    pub fn finalize(&self) -> Result<(), NetworkHistoryError> {
        let mut inner = self.inner.lock().unwrap_or_else(|error| error.into_inner());
        if inner.finalized {
            return Ok(());
        }
        if inner.read_only {
            return Err(read_only_error(inner.next_sequence));
        }
        let frame = JournalFrame {
            schema_version: NETWORK_HISTORY_SCHEMA_VERSION,
            history_id: inner.history_id.clone(),
            page_registrations: Vec::new(),
            records: Vec::new(),
            blobs: Vec::new(),
            page_closures: Vec::new(),
            context_closed: true,
            terminal_failure: inner.terminal.clone(),
        };
        persist_control_frame(&mut inner, &frame)?;
        inner.finalized = true;
        Ok(())
    }

    pub fn recover_archive(
        directory: &Path,
        limits: NetworkHistoryLimits,
    ) -> Result<Self, NetworkHistoryError> {
        let manifest_path = directory.join("manifest.json");
        let manifest_len = fs::metadata(&manifest_path).map_err(io_error)?.len();
        if manifest_len > limits.disk_bytes as u64 {
            return Err(recovery_limit_error(
                NetworkHistoryFailureKind::DiskBytes,
                "diskBytes",
                limits.disk_bytes,
                usize::try_from(manifest_len).unwrap_or(usize::MAX),
                0,
            ));
        }
        let manifest_bytes = fs::read(manifest_path).map_err(io_error)?;
        let manifest: Manifest = serde_json::from_slice(&manifest_bytes).map_err(|error| {
            NetworkHistoryError::transient(
                NetworkHistoryFailureKind::Recovery,
                format!("network history manifest is invalid: {error}"),
            )
        })?;
        if manifest.schema_version != NETWORK_HISTORY_SCHEMA_VERSION {
            return Err(NetworkHistoryError::transient(
                NetworkHistoryFailureKind::Recovery,
                format!(
                    "unsupported network history schema {}",
                    manifest.schema_version
                ),
            ));
        }
        let history = Self::new_inner(manifest.context_id, limits, None, true);
        {
            let mut inner = history.inner.lock().unwrap_or_else(|error| error.into_inner());
            inner.history_id = manifest.history_id;
            inner.directory = Some(directory.to_path_buf());
            inner.disk_bytes = manifest_bytes.len();
        }
        let mut journal = File::open(directory.join("journal.log")).map_err(io_error)?;
        let max_frame_bytes = limits.disk_bytes;
        loop {
            match read_frame(&mut journal, max_frame_bytes) {
                Ok(Some((frame, frame_bytes))) => {
                    let mut inner = history.inner.lock().unwrap_or_else(|error| error.into_inner());
                    if let Err(error) = apply_recovered_frame(&mut inner, frame, frame_bytes) {
                        inner.terminal.get_or_insert(error);
                        break;
                    }
                }
                Ok(None) => {
                    let mut inner = history.inner.lock().unwrap_or_else(|error| error.into_inner());
                    if !inner.finalized && inner.terminal.is_none() {
                        let last = inner.next_sequence;
                        inner.terminal = Some(NetworkHistoryError {
                            kind: NetworkHistoryFailureKind::Recovery,
                            message: "network history archive ended without a clean context-close frame".to_string(),
                            resource: Some("journal".to_string()),
                            limit: None,
                            attempted: None,
                            last_accepted_sequence: last,
                            page_instance_id: None,
                            request_id: None,
                        });
                    }
                    break;
                }
                Err(error) => {
                    let mut inner = history.inner.lock().unwrap_or_else(|failure| failure.into_inner());
                    let last = inner.next_sequence;
                    inner.terminal.get_or_insert(NetworkHistoryError {
                        kind: NetworkHistoryFailureKind::Recovery,
                        message: error,
                        resource: Some("journal".to_string()),
                        limit: None,
                        attempted: None,
                        last_accepted_sequence: last,
                        page_instance_id: None,
                        request_id: None,
                    });
                    break;
                }
            }
        }
        Ok(history)
    }

    pub fn recover_archives(
        storage_dir: &Path,
        limits: NetworkHistoryLimits,
    ) -> Result<Vec<Self>, NetworkHistoryError> {
        let root = storage_dir.join("network-history");
        if !root.exists() {
            return Ok(Vec::new());
        }
        let mut directories = fs::read_dir(root)
            .map_err(io_error)?
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false))
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        directories.sort();
        directories
            .into_iter()
            .map(|directory| Self::recover_archive(&directory, limits))
            .collect()
    }
}

impl PageHistoryWriter {
    pub fn page_instance_id(&self) -> &PageInstanceId {
        &self.page_instance_id
    }

    pub fn display_page_id(&self) -> &str {
        &self.display_page_id
    }

    pub fn append_event(
        &self,
        event: &NetworkEvent,
        request_body: Option<HistoryBodyCandidate>,
        transport_request_body: Option<HistoryBodyCandidate>,
        response_body: Option<HistoryBodyCandidate>,
    ) -> Result<u64, NetworkHistoryError> {
        let mut candidate = NetworkHistoryCandidate::new(event.into());
        candidate.request_body = request_body;
        candidate.transport_request_body = transport_request_body;
        candidate.response_body = response_body;
        let mut records = self.append_owned_batch(vec![candidate])?;
        Ok(records.remove(0).sequence)
    }

    pub fn append_owned_batch(
        &self,
        candidates: Vec<NetworkHistoryCandidate>,
    ) -> Result<Vec<NetworkHistoryRecord>, NetworkHistoryError> {
        if candidates.is_empty() {
            return Ok(Vec::new());
        }
        let mut inner = self.history.inner.lock().unwrap_or_else(|error| error.into_inner());
        ensure_open(&inner)?;
        if let Some(failure) = &inner.terminal {
            return Err(failure.clone());
        }
        if inner.closed_pages.contains(&self.page_instance_id) {
            return Err(closed_error(
                inner.next_sequence,
                format!("page {} is closed", self.page_instance_id.0),
            ));
        }
        if !inner.pages.contains_key(&self.page_instance_id) {
            return Err(NetworkHistoryError::transient(
                NetworkHistoryFailureKind::NotFound,
                format!("page {} is not registered", self.page_instance_id.0),
            ));
        }

        let mut staged_versions = inner.body_versions.clone();
        let mut staged_blobs = BTreeMap::<String, Vec<u8>>::new();
        let mut new_blob_bytes = 0usize;
        let mut body_entries = 0usize;
        let mut records = Vec::with_capacity(candidates.len());

        for (index, candidate) in candidates.into_iter().enumerate() {
            validate_body_candidate(
                &mut inner,
                &self.page_instance_id,
                &candidate.event.request_id,
                candidate.event.request_body_present,
                candidate.event.request_body_request_id.as_deref(),
                candidate.request_body.as_ref(),
                "request",
            )?;
            validate_body_candidate(
                &mut inner,
                &self.page_instance_id,
                &candidate.event.request_id,
                candidate.event.transport_request_body_present,
                candidate.event.transport_request_body_request_id.as_deref(),
                candidate.transport_request_body.as_ref(),
                "transport request",
            )?;
            validate_body_candidate(
                &mut inner,
                &self.page_instance_id,
                &candidate.event.request_id,
                candidate.event.response_body_request_id.is_some(),
                candidate.event.response_body_request_id.as_deref(),
                candidate.response_body.as_ref(),
                "response",
            )?;

            let request_body = stage_body(
                &mut inner,
                &mut staged_versions,
                &mut staged_blobs,
                &mut new_blob_bytes,
                &mut body_entries,
                &self.page_instance_id,
                HistoryBodyKind::Request,
                candidate.request_body,
            )?;
            let transport_request_body = stage_body(
                &mut inner,
                &mut staged_versions,
                &mut staged_blobs,
                &mut new_blob_bytes,
                &mut body_entries,
                &self.page_instance_id,
                HistoryBodyKind::TransportRequest,
                candidate.transport_request_body,
            )?;
            let response_body = stage_body(
                &mut inner,
                &mut staged_versions,
                &mut staged_blobs,
                &mut new_blob_bytes,
                &mut body_entries,
                &self.page_instance_id,
                HistoryBodyKind::Response,
                candidate.response_body,
            )?;
            for (label, declared, captured) in [
                ("request", candidate.event.request_body_size, request_body.as_ref()),
                (
                    "transport request",
                    candidate.event.transport_request_body_size,
                    transport_request_body.as_ref(),
                ),
                ("response", candidate.event.body_size, response_body.as_ref()),
            ] {
                if captured.is_some_and(|body| body.size != declared) {
                    return Err(fail_message_locked(
                        &mut inner,
                        NetworkHistoryFailureKind::BodyMissing,
                        format!(
                            "network history {label} body size does not match its exact capture"
                        ),
                        Some(self.page_instance_id.clone()),
                        Some(candidate.event.request_id.clone()),
                    ));
                }
            }
            let sequence = inner
                .next_sequence
                .checked_add(index as u64 + 1)
                .ok_or_else(|| {
                    fail_locked(
                        &mut inner,
                        NetworkHistoryFailureKind::Count,
                        "sequence",
                        usize::MAX,
                        usize::MAX,
                        Some(self.page_instance_id.clone()),
                        Some(candidate.event.request_id.clone()),
                    )
                })?;
            records.push(NetworkHistoryRecord {
                schema_version: NETWORK_HISTORY_SCHEMA_VERSION,
                sequence,
                history_id: inner.history_id.clone(),
                page_instance_id: self.page_instance_id.clone(),
                display_page_id: self.display_page_id.clone(),
                event: candidate.event,
                request_body,
                transport_request_body,
                response_body,
            });
        }

        let lengths = records
            .iter()
            .map(|record| serde_json::to_vec(record).map(|bytes| bytes.len()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| {
                fail_message_locked(
                    &mut inner,
                    NetworkHistoryFailureKind::Serialization,
                    format!("network history record serialization failed: {error}"),
                    Some(self.page_instance_id.clone()),
                    None,
                )
            })?;
        admit_records(
            &mut inner,
            &records,
            &lengths,
            body_entries,
            new_blob_bytes,
            &self.page_instance_id,
        )?;

        let blobs = staged_blobs
            .iter()
            .filter(|(sha, _)| !inner.blobs.contains_key(*sha))
            .map(|(sha, bytes)| PersistedBlob { sha256: sha.clone(), bytes: bytes.clone() })
            .collect::<Vec<_>>();
        let frame = JournalFrame {
            schema_version: NETWORK_HISTORY_SCHEMA_VERSION,
            history_id: inner.history_id.clone(),
            page_registrations: Vec::new(),
            records: records.clone(),
            blobs,
            page_closures: Vec::new(),
            context_closed: false,
            terminal_failure: None,
        };
        let payload = serialize_frame(&mut inner, &frame, Some(self.page_instance_id.clone()))?;
        admit_disk(&mut inner, payload.len() + JOURNAL_HEADER_BYTES, Some(self.page_instance_id.clone()))?;
        persist_payload(&mut inner, &payload, Some(self.page_instance_id.clone()))?;

        inner.next_sequence = records.last().map_or(inner.next_sequence, |record| record.sequence);
        inner.metadata_bytes += lengths.iter().sum::<usize>();
        inner.body_bytes += new_blob_bytes;
        inner.body_entries += body_entries;
        inner.body_versions = staged_versions;
        for (sha, bytes) in staged_blobs {
            inner.blobs.entry(sha).or_insert_with(|| Arc::new(bytes));
        }
        for record in &records {
            for body in [
                record.request_body.as_ref(),
                record.transport_request_body.as_ref(),
                record.response_body.as_ref(),
            ]
            .into_iter()
            .flatten()
            {
                inner.body_index.insert(body.key.clone(), body.sha256.clone());
            }
        }
        inner.records.extend(records.iter().cloned());
        Ok(records)
    }

    pub fn close(&self) -> Result<(), NetworkHistoryError> {
        let mut inner = self.history.inner.lock().unwrap_or_else(|error| error.into_inner());
        if inner.closed_pages.contains(&self.page_instance_id) {
            return Ok(());
        }
        if inner.read_only {
            return Err(read_only_error(inner.next_sequence));
        }
        if inner.finalized {
            return Err(closed_error(inner.next_sequence, "network history is finalized"));
        }
        let frame = JournalFrame {
            schema_version: NETWORK_HISTORY_SCHEMA_VERSION,
            history_id: inner.history_id.clone(),
            page_registrations: Vec::new(),
            records: Vec::new(),
            blobs: Vec::new(),
            page_closures: vec![self.page_instance_id.clone()],
            context_closed: false,
            terminal_failure: None,
        };
        persist_control_frame(&mut inner, &frame)?;
        inner.closed_pages.insert(self.page_instance_id.clone());
        Ok(())
    }
}

fn validate_body_candidate(
    inner: &mut Inner,
    page: &PageInstanceId,
    request_id: &str,
    present: bool,
    expected_id: Option<&str>,
    candidate: Option<&HistoryBodyCandidate>,
    label: &str,
) -> Result<(), NetworkHistoryError> {
    let matches = match (present, expected_id, candidate) {
        (false, None, None) => true,
        (true, Some(expected), Some(candidate)) => expected == candidate.local_id,
        _ => false,
    };
    if matches {
        return Ok(());
    }
    Err(fail_message_locked(
        inner,
        NetworkHistoryFailureKind::BodyMissing,
        format!(
            "network history {label} body presence/id does not match its exact body candidate"
        ),
        Some(page.clone()),
        Some(request_id.to_string()),
    ))
}

#[allow(clippy::too_many_arguments)]
fn stage_body(
    inner: &mut Inner,
    versions: &mut HashMap<(PageInstanceId, HistoryBodyKind, String), u64>,
    blobs: &mut BTreeMap<String, Vec<u8>>,
    new_blob_bytes: &mut usize,
    body_entries: &mut usize,
    page: &PageInstanceId,
    kind: HistoryBodyKind,
    candidate: Option<HistoryBodyCandidate>,
) -> Result<Option<HistoryBodyRef>, NetworkHistoryError> {
    let Some(candidate) = candidate else { return Ok(None); };
    let bytes = candidate.exact_bytes().map_err(|error| {
        fail_message_locked(
            inner,
            error.kind,
            error.message,
            Some(page.clone()),
            Some(candidate.local_id.clone()),
        )
    })?;
    let sha256 = sha256_hex(&bytes);
    if let Some(existing) = inner.blobs.get(&sha256) {
        if existing.as_slice() != bytes.as_slice() {
            return Err(fail_message_locked(
                inner,
                NetworkHistoryFailureKind::Serialization,
                "network history body hash collision",
                Some(page.clone()),
                Some(candidate.local_id),
            ));
        }
    } else if let Some(existing) = blobs.get(&sha256) {
        if existing != &bytes {
            return Err(fail_message_locked(
                inner,
                NetworkHistoryFailureKind::Serialization,
                "network history staged body hash collision",
                Some(page.clone()),
                Some(candidate.local_id),
            ));
        }
    } else {
        let body_bytes_limit = inner.limits.body_bytes;
        *new_blob_bytes = new_blob_bytes.checked_add(bytes.len()).ok_or_else(|| {
            fail_locked(
                inner,
                NetworkHistoryFailureKind::BodyBytes,
                "bodyBytes",
                body_bytes_limit,
                usize::MAX,
                Some(page.clone()),
                Some(candidate.local_id.clone()),
            )
        })?;
        blobs.insert(sha256.clone(), bytes.clone());
    }
    let body_entries_limit = inner.limits.body_entries;
    *body_entries = body_entries.checked_add(1).ok_or_else(|| {
        fail_locked(
            inner,
            NetworkHistoryFailureKind::BodyEntries,
            "bodyEntries",
            body_entries_limit,
            usize::MAX,
            Some(page.clone()),
            Some(candidate.local_id.clone()),
        )
    })?;
    let version_key = (page.clone(), kind, candidate.local_id.clone());
    let version = versions.get(&version_key).copied().unwrap_or(0).checked_add(1)
        .ok_or_else(|| fail_message_locked(
            inner,
            NetworkHistoryFailureKind::Count,
            "network history body version overflow",
            Some(page.clone()),
            Some(candidate.local_id.clone()),
        ))?;
    versions.insert(version_key, version);
    let kind_name = match kind {
        HistoryBodyKind::Request => "request",
        HistoryBodyKind::TransportRequest => "transport",
        HistoryBodyKind::Response => "response",
    };
    let key = HistoryBodyKey(format!(
        "{}/{}/{}/{}/{}",
        inner.history_id.0, page.0, kind_name, candidate.local_id, version
    ));
    Ok(Some(HistoryBodyRef {
        key,
        local_id: candidate.local_id,
        kind,
        version,
        size: bytes.len(),
        sha256,
    }))
}

fn admit_records(
    inner: &mut Inner,
    records: &[NetworkHistoryRecord],
    lengths: &[usize],
    body_entries: usize,
    body_bytes: usize,
    page: &PageInstanceId,
) -> Result<(), NetworkHistoryError> {
    let single_record_limit = inner.limits.single_record_bytes;
    if let Some(length) = lengths.iter().copied().find(|length| *length > single_record_limit) {
        return Err(fail_locked(
            inner,
            NetworkHistoryFailureKind::RecordBytes,
            "singleRecordBytes",
            single_record_limit,
            length,
            Some(page.clone()),
            records.first().map(|record| record.event.request_id.clone()),
        ));
    }
    let attempted_records = inner.records.len().checked_add(records.len()).unwrap_or(usize::MAX);
    let record_limit = inner.limits.records;
    if attempted_records > record_limit {
        return Err(fail_locked(
            inner,
            NetworkHistoryFailureKind::Count,
            "records",
            record_limit,
            attempted_records,
            Some(page.clone()),
            records.first().map(|record| record.event.request_id.clone()),
        ));
    }
    let batch_metadata = lengths.iter().try_fold(0usize, |sum, length| sum.checked_add(*length))
        .unwrap_or(usize::MAX);
    let attempted_metadata = inner.metadata_bytes.checked_add(batch_metadata).unwrap_or(usize::MAX);
    let metadata_limit = inner.limits.metadata_bytes;
    if attempted_metadata > metadata_limit {
        return Err(fail_locked(
            inner,
            NetworkHistoryFailureKind::MetadataBytes,
            "metadataBytes",
            metadata_limit,
            attempted_metadata,
            Some(page.clone()),
            records.first().map(|record| record.event.request_id.clone()),
        ));
    }
    let attempted_body_entries = inner.body_entries.checked_add(body_entries).unwrap_or(usize::MAX);
    let body_entries_limit = inner.limits.body_entries;
    if attempted_body_entries > body_entries_limit {
        return Err(fail_locked(
            inner,
            NetworkHistoryFailureKind::BodyEntries,
            "bodyEntries",
            body_entries_limit,
            attempted_body_entries,
            Some(page.clone()),
            records.first().map(|record| record.event.request_id.clone()),
        ));
    }
    let attempted_body_bytes = inner.body_bytes.checked_add(body_bytes).unwrap_or(usize::MAX);
    let body_bytes_limit = inner.limits.body_bytes;
    if attempted_body_bytes > body_bytes_limit {
        return Err(fail_locked(
            inner,
            NetworkHistoryFailureKind::BodyBytes,
            "bodyBytes",
            body_bytes_limit,
            attempted_body_bytes,
            Some(page.clone()),
            records.first().map(|record| record.event.request_id.clone()),
        ));
    }
    Ok(())
}

fn ensure_open(inner: &Inner) -> Result<(), NetworkHistoryError> {
    if inner.read_only {
        return Err(read_only_error(inner.next_sequence));
    }
    if inner.finalized {
        return Err(closed_error(inner.next_sequence, "network history is finalized"));
    }
    Ok(())
}

fn persist_control_frame(inner: &mut Inner, frame: &JournalFrame) -> Result<(), NetworkHistoryError> {
    let payload = serialize_frame(inner, frame, None)?;
    admit_disk(inner, payload.len() + JOURNAL_HEADER_BYTES, None)?;
    persist_payload(inner, &payload, None)
}

fn serialize_frame(
    inner: &mut Inner,
    frame: &JournalFrame,
    page: Option<PageInstanceId>,
) -> Result<Vec<u8>, NetworkHistoryError> {
    serde_json::to_vec(frame).map_err(|error| {
        fail_message_locked(
            inner,
            NetworkHistoryFailureKind::Serialization,
            format!("network history journal serialization failed: {error}"),
            page,
            None,
        )
    })
}

fn admit_disk(
    inner: &mut Inner,
    frame_bytes: usize,
    page: Option<PageInstanceId>,
) -> Result<(), NetworkHistoryError> {
    if inner.journal.is_none() {
        return Ok(());
    }
    let attempted = inner.disk_bytes.checked_add(frame_bytes).unwrap_or(usize::MAX);
    let disk_limit = inner.limits.disk_bytes;
    if attempted > disk_limit {
        return Err(fail_locked(
            inner,
            NetworkHistoryFailureKind::DiskBytes,
            "diskBytes",
            disk_limit,
            attempted,
            page,
            None,
        ));
    }
    Ok(())
}

fn persist_payload(
    inner: &mut Inner,
    payload: &[u8],
    page: Option<PageInstanceId>,
) -> Result<(), NetworkHistoryError> {
    let Some(journal) = inner.journal.as_mut() else { return Ok(()); };
    let mut frame = Vec::with_capacity(JOURNAL_HEADER_BYTES + payload.len());
    frame.extend_from_slice(JOURNAL_MAGIC);
    frame.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    frame.extend_from_slice(&crc32(payload).to_le_bytes());
    frame.extend_from_slice(payload);
    let result = journal.write_all(&frame).and_then(|_| journal.sync_data());
    if let Err(error) = result {
        return Err(fail_message_locked(
            inner,
            NetworkHistoryFailureKind::Io,
            format!("network history journal write failed: {error}"),
            page,
            None,
        ));
    }
    inner.disk_bytes += frame.len();
    Ok(())
}

fn fail_locked(
    inner: &mut Inner,
    kind: NetworkHistoryFailureKind,
    resource: &str,
    limit: usize,
    attempted: usize,
    page: Option<PageInstanceId>,
    request_id: Option<String>,
) -> NetworkHistoryError {
    if let Some(failure) = &inner.terminal {
        return failure.clone();
    }
    let failure = NetworkHistoryError {
        kind,
        message: format!("network history {resource} limit {limit}, attempted {attempted}"),
        resource: Some(resource.to_string()),
        limit: Some(limit),
        attempted: Some(attempted),
        last_accepted_sequence: inner.next_sequence,
        page_instance_id: page,
        request_id,
    };
    inner.terminal = Some(failure.clone());
    failure
}

fn fail_message_locked(
    inner: &mut Inner,
    kind: NetworkHistoryFailureKind,
    message: impl Into<String>,
    page: Option<PageInstanceId>,
    request_id: Option<String>,
) -> NetworkHistoryError {
    if let Some(failure) = &inner.terminal {
        return failure.clone();
    }
    let failure = NetworkHistoryError {
        kind,
        message: message.into(),
        resource: None,
        limit: None,
        attempted: None,
        last_accepted_sequence: inner.next_sequence,
        page_instance_id: page,
        request_id,
    };
    inner.terminal = Some(failure.clone());
    failure
}

fn closed_error(sequence: u64, message: impl Into<String>) -> NetworkHistoryError {
    let mut error = NetworkHistoryError::transient(NetworkHistoryFailureKind::Closed, message);
    error.last_accepted_sequence = sequence;
    error
}

fn read_only_error(sequence: u64) -> NetworkHistoryError {
    let mut error = NetworkHistoryError::transient(
        NetworkHistoryFailureKind::ReadOnly,
        "recovered network history is read-only",
    );
    error.last_accepted_sequence = sequence;
    error
}

fn io_error(error: io::Error) -> NetworkHistoryError {
    NetworkHistoryError::transient(
        NetworkHistoryFailureKind::Io,
        format!("network history I/O failed: {error}"),
    )
}

fn secure_create_dir(path: &Path) -> Result<(), NetworkHistoryError> {
    fs::create_dir_all(path).map_err(io_error)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(io_error)?;
    }
    Ok(())
}

fn secure_create_file(path: &Path) -> Result<File, NetworkHistoryError> {
    let mut options = OpenOptions::new();
    options.create_new(true).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path).map_err(io_error)
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<(), NetworkHistoryError> {
    File::open(path).and_then(|directory| directory.sync_all()).map_err(io_error)
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<(), NetworkHistoryError> {
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(&mut output, "{byte:02x}");
    }
    output
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & (0u32.wrapping_sub(crc & 1)));
        }
    }
    !crc
}

fn read_frame(
    file: &mut File,
    max_frame_bytes: usize,
) -> Result<Option<(JournalFrame, usize)>, String> {
    let mut first = [0u8; 1];
    match file.read(&mut first) {
        Ok(0) => return Ok(None),
        Ok(1) => {}
        Ok(_) => unreachable!(),
        Err(error) => return Err(format!("network history journal read failed: {error}")),
    }
    let mut header = [0u8; JOURNAL_HEADER_BYTES];
    header[0] = first[0];
    if let Err(error) = file.read_exact(&mut header[1..]) {
        return Err(format!("network history journal has a partial header: {error}"));
    }
    if &header[..JOURNAL_MAGIC.len()] != JOURNAL_MAGIC {
        return Err("network history journal magic mismatch".to_string());
    }
    let length_offset = JOURNAL_MAGIC.len();
    let payload_len = u64::from_le_bytes(
        header[length_offset..length_offset + 8].try_into().unwrap(),
    );
    let payload_len = usize::try_from(payload_len)
        .map_err(|_| "network history frame length does not fit usize".to_string())?;
    if payload_len > max_frame_bytes {
        return Err(format!(
            "network history frame length {payload_len} exceeds recovery limit {max_frame_bytes}"
        ));
    }
    let expected_crc = u32::from_le_bytes(
        header[length_offset + 8..length_offset + 12].try_into().unwrap(),
    );
    let position = file
        .stream_position()
        .map_err(|error| format!("network history journal seek failed: {error}"))?;
    let file_len = file
        .metadata()
        .map_err(|error| format!("network history journal metadata failed: {error}"))?
        .len();
    let remaining = file_len.saturating_sub(position);
    if u64::try_from(payload_len).unwrap_or(u64::MAX) > remaining {
        return Err(format!(
            "network history journal has a partial payload: expected {payload_len} bytes, found {remaining}"
        ));
    }
    let mut payload = vec![0u8; payload_len];
    if let Err(error) = file.read_exact(&mut payload) {
        return Err(format!("network history journal has a partial payload: {error}"));
    }
    if crc32(&payload) != expected_crc {
        return Err("network history journal checksum mismatch".to_string());
    }
    let frame = serde_json::from_slice(&payload)
        .map_err(|error| format!("network history journal payload is invalid: {error}"))?;
    Ok(Some((frame, JOURNAL_HEADER_BYTES + payload_len)))
}

fn apply_recovered_frame(
    inner: &mut Inner,
    frame: JournalFrame,
    frame_bytes: usize,
) -> Result<(), NetworkHistoryError> {
    if frame.schema_version != NETWORK_HISTORY_SCHEMA_VERSION
        || frame.history_id != inner.history_id
    {
        return Err(NetworkHistoryError::transient(
            NetworkHistoryFailureKind::Recovery,
            "network history journal schema/history id mismatch",
        ));
    }

    if inner.finalized {
        return Err(NetworkHistoryError::transient(
            NetworkHistoryFailureKind::Recovery,
            "network history journal contains data after context close",
        ));
    }
    let attempted_disk = inner.disk_bytes.checked_add(frame_bytes).unwrap_or(usize::MAX);
    if attempted_disk > inner.limits.disk_bytes {
        return Err(recovery_limit_error(
            NetworkHistoryFailureKind::DiskBytes,
            "diskBytes",
            inner.limits.disk_bytes,
            attempted_disk,
            inner.next_sequence,
        ));
    }
    let attempted_records = inner.records.len()
        .checked_add(frame.records.len())
        .unwrap_or(usize::MAX);
    if attempted_records > inner.limits.records {
        return Err(recovery_limit_error(
            NetworkHistoryFailureKind::Count,
            "records",
            inner.limits.records,
            attempted_records,
            inner.next_sequence,
        ));
    }
    let attempted_pages = inner.pages.len()
        .checked_add(frame.page_registrations.len())
        .unwrap_or(usize::MAX);
    if attempted_pages > inner.limits.records {
        return Err(recovery_limit_error(
            NetworkHistoryFailureKind::Count,
            "pageInstances",
            inner.limits.records,
            attempted_pages,
            inner.next_sequence,
        ));
    }
    if frame.context_closed
        && (!frame.page_registrations.is_empty()
            || !frame.records.is_empty()
            || !frame.blobs.is_empty()
            || !frame.page_closures.is_empty())
    {
        return Err(NetworkHistoryError::transient(
            NetworkHistoryFailureKind::Recovery,
            "network history context-close frame contains observations",
        ));
    }
    if frame.terminal_failure.is_some() && !frame.context_closed {
        return Err(NetworkHistoryError::transient(
            NetworkHistoryFailureKind::Recovery,
            "network history terminal failure is outside context-close frame",
        ));
    }
    let context_closed = frame.context_closed;
    let terminal_failure = frame.terminal_failure;

    // Validate and stage a whole frame before changing the accepted prefix.
    // A checksum-valid but semantically invalid frame is still an unaccepted
    // tail and must not leak registrations, blobs, or records into queries.
    let mut pages = inner.pages.clone();
    let mut closed_pages = inner.closed_pages.clone();
    let mut blobs = inner.blobs.clone();
    let mut body_versions = inner.body_versions.clone();
    let mut body_index = inner.body_index.clone();
    let mut next_page_instance = inner.next_page_instance;
    let mut next_sequence = inner.next_sequence;
    let mut metadata_bytes = inner.metadata_bytes;
    let mut body_bytes = inner.body_bytes;
    let mut body_entries = inner.body_entries;
    let mut recovered_records = Vec::with_capacity(frame.records.len());

    for registration in frame.page_registrations {
        if pages.contains_key(&registration.page_instance_id) {
            return Err(NetworkHistoryError::transient(
                NetworkHistoryFailureKind::Recovery,
                "duplicate network history page registration",
            ));
        }
        pages.insert(registration.page_instance_id, registration.display_page_id);
        next_page_instance = next_page_instance.checked_add(1).ok_or_else(|| {
            NetworkHistoryError::transient(
                NetworkHistoryFailureKind::Recovery,
                "network history page instance count overflow",
            )
        })?;
    }
    for blob in frame.blobs {
        if sha256_hex(&blob.bytes) != blob.sha256 {
            return Err(NetworkHistoryError::transient(
                NetworkHistoryFailureKind::Recovery,
                "network history body digest mismatch",
            ));
        }
        if let Some(existing) = blobs.get(&blob.sha256) {
            if existing.as_slice() != blob.bytes.as_slice() {
                return Err(NetworkHistoryError::transient(
                    NetworkHistoryFailureKind::Recovery,
                    "network history body hash collision",
                ));
            }
        } else {
            body_bytes = body_bytes.checked_add(blob.bytes.len()).ok_or_else(|| {
                NetworkHistoryError::transient(
                    NetworkHistoryFailureKind::Recovery,
                    "network history body byte count overflow",
                )
            })?;
            if body_bytes > inner.limits.body_bytes {
                return Err(recovery_limit_error(
                    NetworkHistoryFailureKind::BodyBytes,
                    "bodyBytes",
                    inner.limits.body_bytes,
                    body_bytes,
                    inner.next_sequence,
                ));
            }
            blobs.insert(blob.sha256, Arc::new(blob.bytes));
        }
    }
    for record in frame.records {
        if record.schema_version != NETWORK_HISTORY_SCHEMA_VERSION
            || record.history_id != inner.history_id
            || record.sequence != next_sequence.checked_add(1).unwrap_or(u64::MAX)
            || !pages.contains_key(&record.page_instance_id)
            || closed_pages.contains(&record.page_instance_id)
            || pages.get(&record.page_instance_id) != Some(&record.display_page_id)
        {
            return Err(NetworkHistoryError::transient(
                NetworkHistoryFailureKind::Recovery,
                "network history record sequence or ownership mismatch",
            ));
        }
        let metadata = serde_json::to_vec(&record).map_err(|error| {
            NetworkHistoryError::transient(
                NetworkHistoryFailureKind::Recovery,
                format!("network history recovered record cannot serialize: {error}"),
            )
        })?;
        if metadata.len() > inner.limits.single_record_bytes {
            return Err(recovery_limit_error(
                NetworkHistoryFailureKind::RecordBytes,
                "singleRecordBytes",
                inner.limits.single_record_bytes,
                metadata.len(),
                inner.next_sequence,
            ));
        }
        metadata_bytes = metadata_bytes.checked_add(metadata.len()).ok_or_else(|| {
            NetworkHistoryError::transient(
                NetworkHistoryFailureKind::Recovery,
                "network history metadata byte count overflow",
            )
        })?;
        if metadata_bytes > inner.limits.metadata_bytes {
            return Err(recovery_limit_error(
                NetworkHistoryFailureKind::MetadataBytes,
                "metadataBytes",
                inner.limits.metadata_bytes,
                metadata_bytes,
                inner.next_sequence,
            ));
        }
        let body_shapes_match = record.request_body.as_ref().map(|body| body.local_id.as_str())
            == record.event.request_body_request_id.as_deref()
            && record.request_body.is_some() == record.event.request_body_present
            && record.transport_request_body.as_ref().map(|body| body.local_id.as_str())
                == record.event.transport_request_body_request_id.as_deref()
            && record.transport_request_body.is_some()
                == record.event.transport_request_body_present
            && record.response_body.as_ref().map(|body| body.local_id.as_str())
                == record.event.response_body_request_id.as_deref()
            && record.request_body.as_ref().is_none_or(|body| {
                body.size == record.event.request_body_size
            })
            && record.transport_request_body.as_ref().is_none_or(|body| {
                body.size == record.event.transport_request_body_size
            })
            && record.response_body.as_ref().is_none_or(|body| {
                body.size == record.event.body_size
            });
        if !body_shapes_match {
            return Err(NetworkHistoryError::transient(
                NetworkHistoryFailureKind::Recovery,
                "network history record body presence/id mismatch",
            ));
        }
        for (expected_kind, body) in [
            (HistoryBodyKind::Request, record.request_body.as_ref()),
            (
                HistoryBodyKind::TransportRequest,
                record.transport_request_body.as_ref(),
            ),
            (HistoryBodyKind::Response, record.response_body.as_ref()),
        ] {
            let Some(body) = body else { continue; };
            if body.kind != expected_kind || body_index.contains_key(&body.key) {
                return Err(NetworkHistoryError::transient(
                    NetworkHistoryFailureKind::Recovery,
                    "network history body kind or key is invalid",
                ));
            }
            let blob = blobs.get(&body.sha256).ok_or_else(|| {
                NetworkHistoryError::transient(
                    NetworkHistoryFailureKind::Recovery,
                    format!("network history body blob {} is missing", body.sha256),
                )
            })?;
            if blob.len() != body.size {
                return Err(NetworkHistoryError::transient(
                    NetworkHistoryFailureKind::Recovery,
                    "network history body size mismatch",
                ));
            }
            body_entries = body_entries.checked_add(1).ok_or_else(|| {
                NetworkHistoryError::transient(
                    NetworkHistoryFailureKind::Recovery,
                    "network history body entry count overflow",
                )
            })?;
            if body_entries > inner.limits.body_entries {
                return Err(recovery_limit_error(
                    NetworkHistoryFailureKind::BodyEntries,
                    "bodyEntries",
                    inner.limits.body_entries,
                    body_entries,
                    inner.next_sequence,
                ));
            }
            body_index.insert(body.key.clone(), body.sha256.clone());
            let version_key = (
                record.page_instance_id.clone(),
                body.kind,
                body.local_id.clone(),
            );
            let previous = body_versions.get(&version_key).copied().unwrap_or(0);
            if body.version != previous.checked_add(1).unwrap_or(u64::MAX) {
                return Err(NetworkHistoryError::transient(
                    NetworkHistoryFailureKind::Recovery,
                    "network history body version is not contiguous",
                ));
            }
            body_versions.insert(version_key, body.version);
        }
        next_sequence = record.sequence;
        recovered_records.push(record);
    }
    for page in frame.page_closures {
        if !pages.contains_key(&page) || closed_pages.contains(&page) {
            return Err(NetworkHistoryError::transient(
                NetworkHistoryFailureKind::Recovery,
                "network history closes an unknown page",
            ));
        }
        closed_pages.insert(page);
    }
    if let Some(failure) = &terminal_failure {
        if failure.last_accepted_sequence != next_sequence {
            return Err(NetworkHistoryError::transient(
                NetworkHistoryFailureKind::Recovery,
                "network history terminal failure sequence mismatch",
            ));
        }
    }

    inner.pages = pages;
    inner.closed_pages = closed_pages;
    inner.blobs = blobs;
    inner.body_versions = body_versions;
    inner.body_index = body_index;
    inner.next_page_instance = next_page_instance;
    inner.next_sequence = next_sequence;
    inner.metadata_bytes = metadata_bytes;
    inner.body_bytes = body_bytes;
    inner.body_entries = body_entries;
    inner.records.extend(recovered_records);
    inner.finalized = context_closed;
    inner.terminal = terminal_failure.or_else(|| inner.terminal.clone());
    inner.disk_bytes = attempted_disk;
    Ok(())
}

fn recovery_limit_error(
    kind: NetworkHistoryFailureKind,
    resource: &str,
    limit: usize,
    attempted: usize,
    last_accepted_sequence: u64,
) -> NetworkHistoryError {
    NetworkHistoryError {
        kind,
        message: format!(
            "network history recovery {resource} limit {limit}, attempted {attempted}"
        ),
        resource: Some(resource.to_string()),
        limit: Some(limit),
        attempted: Some(attempted),
        last_accepted_sequence,
        page_instance_id: None,
        request_id: None,
    }
}

mod base64_bytes {
    use base64::Engine as _;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&base64::engine::general_purpose::STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        let value = String::deserialize(deserializer)?;
        base64::engine::general_purpose::STANDARD
            .decode(value)
            .map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> NetworkHistoryLimits {
        NetworkHistoryLimits {
            records: 8,
            metadata_bytes: 1024 * 1024,
            single_record_bytes: 512 * 1024,
            body_bytes: 1024 * 1024,
            body_entries: 32,
            disk_bytes: 4 * 1024 * 1024,
        }
    }

    fn event(request_id: &str) -> OwnedNetworkEvent {
        OwnedNetworkEvent {
            document_generation: 3,
            document_url: "https://example.test/document".into(),
            initiator_request_id: None,
            retired_document_url: None,
            pending: false,
            error: None,
            request_body_present: false,
            request_body_request_id: None,
            request_body_size: 0,
            transport_request_body_present: false,
            transport_request_body_request_id: None,
            transport_request_body_size: 0,
            request_started: false,
            redirect: false,
            response_body_request_id: None,
            response_body_capture_error: None,
            request_id: request_id.into(),
            url: format!("https://example.test/{request_id}"),
            method: "GET".into(),
            resource_type: "Fetch".into(),
            status: 200,
            status_text: "OK".into(),
            headers: HashMap::from([("authorization".into(), "Bearer exact".into())]),
            response_headers: HashMap::from([("set-cookie".into(), "a=1, b=2".into())]),
            raw_headers: Some(raw_capture("transportResponse")),
            request_raw_headers: Some(raw_capture("transportRequest")),
            body_size: 0,
            timestamp: 123.5,
        }
    }

    fn raw_capture(stage: &str) -> OwnedHeaderCapture {
        OwnedHeaderCapture {
            capture_stage: stage.into(),
            encoding: "base64".into(),
            fields: vec![
                OwnedRawHeader { name: b"Cookie".to_vec(), value: b"first=1".to_vec() },
                OwnedRawHeader { name: b"Cookie".to_vec(), value: vec![0, 0xff, b'='] },
                OwnedRawHeader { name: b"Authorization".to_vec(), value: (0..=255).collect() },
            ],
        }
    }

    fn candidate_with_all_bodies(request_id: &str) -> NetworkHistoryCandidate {
        let mut observation = event(request_id);
        observation.method = "POST".into();
        observation.request_body_present = true;
        observation.request_body_request_id = Some("standard".into());
        observation.request_body_size = 0;
        observation.transport_request_body_present = true;
        observation.transport_request_body_request_id = Some("transport".into());
        observation.transport_request_body_size = 256;
        observation.response_body_request_id = Some("response".into());
        observation.body_size = 256;
        NetworkHistoryCandidate {
            event: observation,
            request_body: Some(HistoryBodyCandidate::from_bytes("standard", Vec::new())),
            transport_request_body: Some(HistoryBodyCandidate::from_bytes(
                "transport",
                (0..=255).collect::<Vec<_>>(),
            )),
            response_body: Some(HistoryBodyCandidate::from_bytes(
                "response",
                (0..=255).rev().collect::<Vec<_>>(),
            )),
        }
    }

    fn temp_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "obscura-network-history-{label}-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn raw_headers_empty_absent_and_bodies_round_trip_exactly() {
        let history = NetworkHistory::with_limits("context", limits());
        let writer = history.register_page("page").unwrap();
        let records = writer
            .append_owned_batch(vec![candidate_with_all_bodies("same")])
            .unwrap();
        let record = &records[0];
        assert_eq!(record.event.request_raw_headers.as_ref().unwrap().fields[1].value, [0, 0xff, b'=']);
        assert_eq!(record.event.request_raw_headers.as_ref().unwrap().fields[2].value, (0..=255).collect::<Vec<_>>());
        let empty = history.read_body(&record.request_body.as_ref().unwrap().key, 0, 99).unwrap();
        assert!(empty.bytes.is_empty());
        assert!(empty.eof);
        assert_eq!(history.read_body(&record.transport_request_body.as_ref().unwrap().key, 0, 999).unwrap().bytes, (0..=255).collect::<Vec<_>>());
        assert_eq!(history.read_body(&record.response_body.as_ref().unwrap().key, 0, 999).unwrap().bytes, (0..=255).rev().collect::<Vec<_>>());

        let absent = writer.append_owned_batch(vec![NetworkHistoryCandidate::new(event("absent"))]).unwrap();
        assert!(absent[0].request_body.is_none());
        assert!(absent[0].response_body.is_none());
    }

    #[test]
    fn admission_is_atomic_sticky_and_preserves_the_accepted_prefix() {
        let mut small = limits();
        small.records = 2;
        let history = NetworkHistory::with_limits("context", small);
        let writer = history.register_page("page").unwrap();
        writer.append_owned_batch(vec![NetworkHistoryCandidate::new(event("accepted"))]).unwrap();
        let failure = writer.append_owned_batch(vec![
            NetworkHistoryCandidate::new(event("rejected-a")),
            NetworkHistoryCandidate::new(event("rejected-b")),
        ]).unwrap_err();
        assert_eq!(failure.kind, NetworkHistoryFailureKind::Count);
        assert_eq!(failure.last_accepted_sequence, 1);
        assert_eq!(writer.append_owned_batch(vec![NetworkHistoryCandidate::new(event("later"))]).unwrap_err(), failure);
        let page = history.query(NetworkHistoryQuery { after_sequence: 0, limit: 99, page_instance_id: None });
        assert_eq!(page.records.len(), 1);
        assert_eq!(page.records[0].event.request_id, "accepted");
        assert_eq!(page.terminal_failure, Some(failure));
    }

    #[test]
    fn context_sequence_and_composite_body_keys_isolate_same_local_ids() {
        let history = NetworkHistory::with_limits("context", limits());
        let left = history.register_page("left").unwrap();
        let right = history.register_page("right").unwrap();
        let left_record = left.append_owned_batch(vec![candidate_with_all_bodies("fetch-1")]).unwrap().remove(0);
        let right_record = right.append_owned_batch(vec![candidate_with_all_bodies("fetch-1")]).unwrap().remove(0);
        assert_eq!((left_record.sequence, right_record.sequence), (1, 2));
        assert_ne!(left.page_instance_id(), right.page_instance_id());
        assert_ne!(left_record.request_body.unwrap().key, right_record.request_body.unwrap().key);
        let right_only = history.query(NetworkHistoryQuery {
            after_sequence: 0,
            limit: 10,
            page_instance_id: Some(right.page_instance_id().clone()),
        });
        assert_eq!(right_only.records.len(), 1);
        assert_eq!(right_only.records[0].display_page_id, "right");
    }

    #[test]
    fn durable_archive_reopens_complete_records_and_body_bytes() {
        let root = temp_root("reopen");
        let history = NetworkHistory::persistent_with_limits("context", &root, limits()).unwrap();
        let writer = history.register_page("page").unwrap();
        let record = writer.append_owned_batch(vec![candidate_with_all_bodies("durable")]).unwrap().remove(0);
        writer.close().unwrap();
        history.finalize().unwrap();
        let directory = history.storage_path().unwrap();
        drop(writer);
        drop(history);

        let recovered = NetworkHistory::recover_archive(&directory, limits()).unwrap();
        let page = recovered.query(NetworkHistoryQuery { after_sequence: 0, limit: 10, page_instance_id: None });
        assert_eq!(page.records.len(), 1);
        assert!(page.finalized);
        assert!(page.closed_pages.contains(&record.page_instance_id));
        assert_eq!(recovered.read_body(&page.records[0].response_body.as_ref().unwrap().key, 0, 999).unwrap().bytes, (0..=255).rev().collect::<Vec<_>>());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn clean_close_persists_the_original_terminal_failure() {
        let root = temp_root("terminal");
        let mut small = limits();
        small.records = 1;
        let history = NetworkHistory::persistent_with_limits("context", &root, small).unwrap();
        let writer = history.register_page("page").unwrap();
        writer.append_owned_batch(vec![NetworkHistoryCandidate::new(event("accepted"))]).unwrap();
        let failure = writer
            .append_owned_batch(vec![NetworkHistoryCandidate::new(event("rejected"))])
            .unwrap_err();
        writer.close().unwrap();
        history.finalize().unwrap();
        let directory = history.storage_path().unwrap();
        drop(writer);
        drop(history);

        let recovered = NetworkHistory::recover_archive(&directory, small).unwrap();
        let page = recovered.query(NetworkHistoryQuery {
            after_sequence: 0,
            limit: 10,
            page_instance_id: None,
        });
        assert!(page.finalized);
        assert_eq!(page.records.len(), 1);
        assert_eq!(page.terminal_failure, Some(failure));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recovery_enforces_cumulative_limits_and_keeps_the_valid_prefix() {
        let root = temp_root("recovery-limits");
        let history = NetworkHistory::persistent_with_limits("context", &root, limits()).unwrap();
        let writer = history.register_page("page").unwrap();
        writer.append_owned_batch(vec![NetworkHistoryCandidate::new(event("first"))]).unwrap();
        writer.append_owned_batch(vec![NetworkHistoryCandidate::new(event("second"))]).unwrap();
        writer.close().unwrap();
        history.finalize().unwrap();
        let directory = history.storage_path().unwrap();
        drop(writer);
        drop(history);

        let mut smaller = limits();
        smaller.records = 1;
        let recovered = NetworkHistory::recover_archive(&directory, smaller).unwrap();
        let page = recovered.query(NetworkHistoryQuery {
            after_sequence: 0,
            limit: 10,
            page_instance_id: None,
        });
        assert_eq!(page.records.len(), 1);
        assert_eq!(page.records[0].event.request_id, "first");
        assert_eq!(page.terminal_failure.unwrap().kind, NetworkHistoryFailureKind::Count);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recovery_keeps_valid_prefix_before_partial_tail() {
        let root = temp_root("partial");
        let history = NetworkHistory::persistent_with_limits("context", &root, limits()).unwrap();
        let writer = history.register_page("page").unwrap();
        writer.append_owned_batch(vec![NetworkHistoryCandidate::new(event("accepted"))]).unwrap();
        let directory = history.storage_path().unwrap();
        drop(writer);
        drop(history);
        let mut file = OpenOptions::new().append(true).open(directory.join("journal.log")).unwrap();
        file.write_all(b"OBNH").unwrap();
        file.sync_data().unwrap();
        drop(file);

        let recovered = NetworkHistory::recover_archive(&directory, limits()).unwrap();
        let page = recovered.query(NetworkHistoryQuery { after_sequence: 0, limit: 10, page_instance_id: None });
        assert_eq!(page.records.len(), 1);
        assert_eq!(page.records[0].event.request_id, "accepted");
        assert_eq!(page.terminal_failure.unwrap().kind, NetworkHistoryFailureKind::Recovery);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recovery_keeps_valid_prefix_before_bad_checksum() {
        let root = temp_root("checksum");
        let history = NetworkHistory::persistent_with_limits("context", &root, limits()).unwrap();
        let writer = history.register_page("page").unwrap();
        writer.append_owned_batch(vec![NetworkHistoryCandidate::new(event("first"))]).unwrap();
        writer.append_owned_batch(vec![NetworkHistoryCandidate::new(event("second"))]).unwrap();
        let directory = history.storage_path().unwrap();
        drop(writer);
        drop(history);

        let path = directory.join("journal.log");
        let mut bytes = fs::read(&path).unwrap();
        let mut cursor = 0usize;
        for _ in 0..2 {
            let payload_len = u64::from_le_bytes(
                bytes[cursor + JOURNAL_MAGIC.len()..cursor + JOURNAL_MAGIC.len() + 8]
                    .try_into().unwrap(),
            ) as usize;
            cursor += JOURNAL_HEADER_BYTES + payload_len;
        }
        let payload_len = u64::from_le_bytes(
            bytes[cursor + JOURNAL_MAGIC.len()..cursor + JOURNAL_MAGIC.len() + 8]
                .try_into().unwrap(),
        ) as usize;
        bytes[cursor + JOURNAL_HEADER_BYTES + payload_len - 1] ^= 0x80;
        fs::write(&path, bytes).unwrap();

        let recovered = NetworkHistory::recover_archive(&directory, limits()).unwrap();
        let page = recovered.query(NetworkHistoryQuery { after_sequence: 0, limit: 10, page_instance_id: None });
        assert_eq!(page.records.len(), 1);
        assert_eq!(page.records[0].event.request_id, "first");
        assert_eq!(page.terminal_failure.unwrap().kind, NetworkHistoryFailureKind::Recovery);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn persistent_directories_and_files_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let root = temp_root("permissions");
        let history = NetworkHistory::persistent_with_limits("context", &root, limits()).unwrap();
        let directory = history.storage_path().unwrap();
        assert_eq!(fs::metadata(root.join("network-history")).unwrap().permissions().mode() & 0o777, 0o700);
        assert_eq!(fs::metadata(&directory).unwrap().permissions().mode() & 0o777, 0o700);
        assert_eq!(fs::metadata(directory.join("manifest.json")).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(fs::metadata(directory.join("journal.log")).unwrap().permissions().mode() & 0o777, 0o600);
        fs::remove_dir_all(root).unwrap();
    }
}
