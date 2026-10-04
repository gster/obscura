//! One process-wide native inventory attempt; never runs native code on V8's thread.
//! Ready(empty) and failure are distinct. No retry/refresh or synthesis is implemented.
use crate::speech_protocol::{self, Snapshot, MAX_OUTPUT};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};
use tokio::sync::{oneshot, watch, OwnedSemaphorePermit, Semaphore};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ProviderError {
    UnsupportedPlatform, Thread, Runtime, Payload, Spawn, Io, OutputLimit, StderrLimit,
    Deadline, HistoryLimit, Cancelled, NativeExit(Option<i32>), Protocol, LocaleChanged, Panicked,
    Unreaped, OwnerRetired, RequestLimit,
}
impl std::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "Speech inventory {self:?}") }
}
#[derive(Clone, Debug)]
pub(crate) enum InventoryState { Pending, Ready(Arc<Snapshot>), Failed(ProviderError), Streaming(Progress) }
#[derive(Clone, Debug, Default)]
pub(crate) struct Progress {
    // Every admitted real revision is retained; limits fail explicitly rather
    // than silently suppress changes or report truncated startup as successful.
    snapshots: Vec<Arc<Snapshot>>,
    retained_charge: usize,
    terminal: Option<Result<(), ProviderError>>,
    resolved_default: Option<speech_protocol::ResolvedDefault>,
}
pub(crate) struct Delivery { pub revision: i32, pub snapshot: Option<Arc<Snapshot>>, pub done: bool }
fn finish_state(sender: &watch::Sender<InventoryState>, result: Result<(), ProviderError>) {
    sender.send_modify(|state| {
        if let InventoryState::Streaming(progress) = state { progress.terminal = Some(result); }
        else { *state = InventoryState::Streaming(Progress { snapshots: vec![], retained_charge: 0, terminal: Some(result), resolved_default: None }); }
    });
}
struct Inner {
    state: watch::Receiver<InventoryState>,
    shutdown: Mutex<Option<oneshot::Sender<()>>>,
    slots: Arc<Semaphore>,
    stopped: watch::Receiver<bool>,
}
impl Drop for Inner {
    fn drop(&mut self) {
        if let Some(stop) = self.shutdown.get_mut().unwrap_or_else(|e| e.into_inner()).take() { let _ = stop.send(()); }
    }
}
#[derive(Clone)]
pub(crate) struct Provider(Arc<Inner>);
pub(crate) struct Subscription {
    _provider: Provider,
    state: watch::Receiver<InventoryState>,
    _permit: OwnedSemaphorePermit,
    next_index: usize,
}
impl Subscription {
    pub(crate) async fn next(&mut self) -> Result<Delivery, ProviderError> {
        loop {
            let state = self.state.borrow_and_update().clone();
            match state {
                InventoryState::Streaming(progress) => {
                    if let Some(snapshot) = progress.snapshots.get(self.next_index) {
                        let revision = self.next_index as i32;
                        self.next_index += 1;
                        return Ok(Delivery { revision, snapshot: Some(snapshot.clone()),
                            done: self.next_index == progress.snapshots.len() && matches!(progress.terminal, Some(Ok(()))) });
                    }
                    if let Some(terminal) = progress.terminal {
                        terminal?;
                        return Ok(Delivery { revision: progress.snapshots.len() as i32 - 1, snapshot: None, done: true });
                    }
                }
                // Retained only for existing controlled tests and isolated callers.
                InventoryState::Ready(value) => return Ok(Delivery { revision: 0, snapshot: Some(value), done: true }),
                InventoryState::Failed(error) => return Err(error),
                InventoryState::Pending => {}
            }
            self.state.changed().await.map_err(|_| ProviderError::Cancelled)?;
        }
    }
    pub(crate) async fn ready(&mut self) -> Result<Arc<Snapshot>, ProviderError> {
        self.next().await?.snapshot.ok_or(ProviderError::Protocol)
    }
}
impl Provider {
    pub(crate) fn shared() -> Self {
        // A bounded process cache contains no document/V8 roots. Retaining failure
        // prevents hostile navigation/getVoices loops from restarting the native API.
        static SHARED: OnceLock<Provider> = OnceLock::new();
        SHARED.get_or_init(|| Self::start(Launch::Embedded, Duration::from_secs(8))).clone()
    }
    #[cfg(test)]
    pub(crate) fn controlled() -> (Self, watch::Sender<InventoryState>) {
        let (sender, state) = watch::channel(InventoryState::Pending);
        let (_, stopped) = watch::channel(true);
        (Self(Arc::new(Inner { state, shutdown: Mutex::new(None), slots: Arc::new(Semaphore::new(256)), stopped })), sender)
    }
    pub(crate) fn subscribe(&self) -> Result<Subscription, ProviderError> { self.subscribe_after(-1) }
    pub(crate) fn subscribe_after(&self, after: i32) -> Result<Subscription, ProviderError> {
        if after < -1 { return Err(ProviderError::Protocol); }
        let permit = self.0.slots.clone().try_acquire_owned().map_err(|_| ProviderError::RequestLimit)?;
        let next_index = if after < 0 {
            // Newly attached windows read the current cached snapshot. An enrolled
            // pending reader keeps index0 even if both updates arrive before poll.
            match &*self.0.state.borrow() {
                InventoryState::Streaming(progress) => progress.snapshots.len().saturating_sub(1),
                _ => 0,
            }
        } else { after as usize + 1 };
        Ok(Subscription { _provider: self.clone(), state: self.0.state.clone(), _permit: permit, next_index })
    }
    fn start(launch: Launch, budget: Duration) -> Self {
        let (state_tx, state) = watch::channel(InventoryState::Pending);
        let (shutdown, cancel) = oneshot::channel();
        let (stopped_tx, stopped) = watch::channel(false);
        let provider = Self(Arc::new(Inner { state, shutdown: Mutex::new(Some(shutdown)), slots: Arc::new(Semaphore::new(256)), stopped }));
        let failure_tx = state_tx.clone();
        let stopped_failure = stopped_tx.clone();
        let spawned = std::thread::Builder::new().name("obscura-speech-inventory".into()).spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(runtime) => runtime,
                Err(_) => { state_tx.send_replace(InventoryState::Failed(ProviderError::Runtime)); stopped_tx.send_replace(true); return; }
            };
            let mut job = Job::default();
            // Job owns Child outside the unwindable future, allowing panic cleanup.
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                runtime.block_on(job.run(launch, budget, cancel, state_tx.clone()))
            })).unwrap_or(Err(ProviderError::Panicked));
            let reaped = runtime.block_on(job.cleanup());
            if !reaped {
                finish_state(&state_tx, Err(ProviderError::Unreaped));
                // Retain the owned child/temp file and this single worker until the
                // kernel allows wait. No new child or automatic restart is admitted.
                if let Some(child) = job.child.as_mut() { let _ = runtime.block_on(child.wait()); }
                job.child.take();
            } else {
                finish_state(&state_tx, outcome);
            }
            drop(job);
            stopped_tx.send_replace(true);
            // A timed-out file extraction may still be in a blocking OS call.
            // It owns no Child and its late Payload is dropped, never executed.
            runtime.shutdown_background();
        });
        if spawned.is_err() {
            failure_tx.send_replace(InventoryState::Failed(ProviderError::Thread));
            stopped_failure.send_replace(true);
        }
        provider
    }
}

enum Launch {
    Embedded,
    #[cfg(test)] External { executable: PathBuf, arguments: Vec<String> },
}
struct Payload { path: PathBuf, directory: PathBuf }
impl Drop for Payload {
    fn drop(&mut self) {
        // Remove only our two owned paths, never recursive/unrelated cleanup.
        let _ = std::fs::remove_file(&self.path);
        let _ = std::fs::remove_dir(&self.directory);
    }
}
#[cfg(target_os = "macos")]
fn materialize() -> Result<Payload, ProviderError> {
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    const HELPER: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/obscura-speech-helper"));
    let mut random = [0u8; 16];
    getrandom::getrandom(&mut random).map_err(|_| ProviderError::Payload)?;
    let suffix: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
    let directory = std::env::temp_dir().join(format!("obscura-speech-{}-{suffix}", std::process::id()));
    std::fs::DirBuilder::new().mode(0o700).create(&directory).map_err(|_| ProviderError::Payload)?;
    let payload = Payload { path: directory.join("inventory"), directory };
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o700).open(&payload.path).map_err(|_| ProviderError::Payload)?;
    file.write_all(HELPER).map_err(|_| ProviderError::Payload)?;
    drop(file);
    Ok(payload)
}
#[cfg(not(target_os = "macos"))]
fn materialize() -> Result<Payload, ProviderError> { Err(ProviderError::UnsupportedPlatform) }

#[derive(Default)]
struct Job { child: Option<Child>, payload: Option<Payload> }
impl Job {
    async fn run(&mut self, launch: Launch, budget: Duration, mut cancel: oneshot::Receiver<()>, sender: watch::Sender<InventoryState>) -> Result<(), ProviderError> {
        let deadline = tokio::time::Instant::now() + budget;
        let (path, args) = match launch {
            Launch::Embedded => {
                // File extraction has no authority to start the child. A late
                // result after timeout/cancellation only drops its owned paths.
                let mut preparation = tokio::task::spawn_blocking(materialize);
                let payload = tokio::select! {
                    biased;
                    _ = &mut cancel => return Err(ProviderError::Cancelled),
                    _ = tokio::time::sleep_until(deadline) => return Err(ProviderError::Deadline),
                    value = &mut preparation => value.map_err(|_| ProviderError::Panicked)??,
                };
                let path = payload.path.clone(); self.payload = Some(payload);
                (path, vec!["inventory".to_owned()])
            }
            #[cfg(test)] Launch::External { executable, arguments } => (executable, arguments),
        };
        if tokio::time::Instant::now() >= deadline { return Err(ProviderError::Deadline); }
        if cancel.try_recv().is_ok() { return Err(ProviderError::Cancelled); }
        let child = Command::new(path).args(args).stdin(Stdio::null()).stdout(Stdio::piped())
            .stderr(Stdio::piped()).kill_on_drop(true).spawn().map_err(|_| ProviderError::Spawn)?;
        self.child = Some(child);
        let child = self.child.as_mut().ok_or(ProviderError::Spawn)?;
        let stdout = child.stdout.take().ok_or(ProviderError::Io)?;
        let stderr = child.stderr.take().ok_or(ProviderError::Io)?;
        let collect = async {
            let out = read_stream(stdout, sender);
            let err = read_bounded(stderr, 64 * 1024, ProviderError::StderrLimit);
            let wait = async { child.wait().await.map_err(|_| ProviderError::Io) };
            let (decoder, _stderr, status) = tokio::try_join!(out, err, wait)?;
            if !status.success() { return Err(ProviderError::NativeExit(status.code())); }
            // EOF alone is not a successful child completion. Keep draining
            // bounded stderr and observe exit before diagnosing a missing terminal.
            decoder.finish()
        };
        tokio::select! {
            biased;
            _ = &mut cancel => Err(ProviderError::Cancelled),
            _ = tokio::time::sleep_until(deadline) => Err(ProviderError::Deadline),
            result = collect => result,
        }
    }
    async fn cleanup(&mut self) -> bool {
        let Some(child) = self.child.as_mut() else { return true; };
        if matches!(child.try_wait(), Ok(Some(_))) { self.child.take(); return true; }
        // Kill only the Child owned by this Job, never a PID/process-group search.
        let _ = child.start_kill();
        if matches!(tokio::time::timeout(Duration::from_secs(1), child.wait()).await, Ok(Ok(_))) {
            self.child.take(); true
        } else { false }
    }
}
const MAX_HISTORY_CHARGE: usize = 8 * 1024 * 1024;
fn snapshot_charge(snapshot: &Snapshot) -> Option<usize> {
    // Charges retained String capacities, records, Arc headers and two reference
    // slots per snapshot. This accounting policy does not exactly track the
    // history Vec capacity, allocator metadata, or total RSS.
    let mut bytes = std::mem::size_of::<Snapshot>()
        .checked_add(4 * std::mem::size_of::<usize>())?
        .checked_add(2 * std::mem::size_of::<Arc<Snapshot>>())?
        .checked_add(snapshot.default_branch.capacity())?;
    for voice in snapshot.voices.iter() {
        bytes = bytes.checked_add(std::mem::size_of::<speech_protocol::NativeVoice>())?
            .checked_add(voice.native_identifier.capacity())?
            .checked_add(voice.web.voice_uri.capacity())?
            .checked_add(voice.web.name.capacity())?
            .checked_add(voice.web.lang.capacity())?;
    }
    Some(bytes)
}
async fn read_stream(mut reader: impl AsyncRead + Unpin, sender: watch::Sender<InventoryState>) -> Result<speech_protocol::StreamDecoder, ProviderError> {
    let mut decoder = speech_protocol::StreamDecoder::default();
    let mut total = 0usize;
    let mut line = Vec::new();
    let mut block = [0u8; 8192];
    loop {
        let count = reader.read(&mut block).await.map_err(|_| ProviderError::Io)?;
        if count == 0 {
            if !line.is_empty() { return Err(ProviderError::Protocol); }
            return Ok(decoder);
        }
        if count > MAX_OUTPUT.saturating_sub(total) { return Err(ProviderError::OutputLimit); }
        total += count;
        for byte in &block[..count] {
            if *byte != b'\n' { line.push(*byte); continue; }
            match decoder.accept(&line)? {
                speech_protocol::StreamEvent::Snapshot(snapshot) => {
                    // Capacity-based decoded payload charge, separate from cumulative
                    // wire bytes. No fixed query/frame count or success truncation.
                    let charge = snapshot_charge(&snapshot).ok_or(ProviderError::HistoryLimit)?;
                    let mut exceeded = false;
                    sender.send_modify(|state| {
                        if !matches!(state, InventoryState::Streaming(_)) {
                            *state = InventoryState::Streaming(Progress::default());
                        }
                        let InventoryState::Streaming(progress) = state else { unreachable!() };
                        match progress.retained_charge.checked_add(charge) {
                            Some(total) if total <= MAX_HISTORY_CHARGE => {
                                progress.retained_charge = total; progress.snapshots.push(snapshot);
                            }
                            _ => exceeded = true,
                        }
                    });
                    if exceeded { return Err(ProviderError::HistoryLimit); }
                }
                // Do not report terminal success until EOF, byte limits and child
                // success are also confirmed. Already published snapshots survive failure.
                speech_protocol::StreamEvent::Terminal(selection) => sender.send_modify(|state| {
                    if let InventoryState::Streaming(progress) = state { progress.resolved_default = Some(selection); }
                }),
            }
            line.clear();
        }
    }
}
async fn read_bounded(mut reader: impl AsyncRead + Unpin, limit: usize, error: ProviderError) -> Result<Vec<u8>, ProviderError> {
    let mut output = Vec::new();
    let mut block = [0u8; 8192];
    loop {
        let count = reader.read(&mut block).await.map_err(|_| ProviderError::Io)?;
        if count == 0 { return Ok(output); }
        if count > limit.saturating_sub(output.len()) { return Err(error); }
        output.extend_from_slice(&block[..count]);
    }
}
#[cfg(test)]
#[path = "speech_provider_tests.rs"]
mod tests;
