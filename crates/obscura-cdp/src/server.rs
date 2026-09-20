use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::sync::Notify;
use tokio_tungstenite::tungstenite::Message;
use tracing::{error, info, warn};

use crate::dispatch::{self, CdpContext};
use crate::outbound::{CloseReason as OutboundCloseReason, OutboundSender};

// PR #36 comment 4341743194: the deferral queue in `process_with_interception`
// must be bounded so a stalled navigation cannot OOM the process. When the cap
// is reached we return an explicit error response rather than silently dropping.
const MAX_DEFERRED_MESSAGES: usize = 256;

// The WS-stream forwarding channel must also be bounded: if the LocalSet
// (CDP processor + nav tasks) stalls, the accept thread keeps pushing
// `std::net::TcpStream`s into the queue. An unbounded channel would let
// that queue grow without limit and OOM the process. With a bounded
// capacity, when the LocalSet is saturated the accept thread closes the
// new connection on the spot instead of buffering it — the kernel TCP
// backlog still absorbs short-term spikes, but a long-term stall now
// fails loudly at accept time rather than silently piling up FDs.
const MAX_PENDING_WS_HANDOFFS: usize = 128;

// Cap on *live* CDP connections, each of which costs one OS thread and its own
// V8 isolates. `MAX_PENDING_WS_HANDOFFS` above bounds only the handoff queue —
// connections that have already been handed off are unbounded without this.
//
// 128 matches the handoff bound and is well above any real client fan-out
// (Playwright/Puppeteer use one connection per browser). Threads are what this
// actually bounds: with arenas capped by `cap_malloc_arenas`, 128 idle
// connections cost 146 threads, 33.2 GiB of reserved address space and 51 MiB
// resident -- and nearly all of that 33.2 GiB is V8's process-wide sandbox,
// which is there at zero connections. Override with `--max-connections`.
pub const DEFAULT_MAX_CONNECTIONS: usize = 128;

// How long shutdown waits for connection threads to finish before persisting
// the cookie jar. Well under the 10s `docker stop` gives us before SIGKILL.
const SHUTDOWN_DRAIN_MS: u64 = 3_000;

// A slow or disconnected DevTools client must not retain an unbounded writer
// future. The outbound queue has independent count and byte budgets; this
// deadline bounds the one envelope currently held by the websocket sink.
const OUTBOUND_SEND_TIMEOUT_MS: u64 = 10_000;

// Give a processor whose input side just closed a short opportunity to abort
// Fetch pauses and drop its owned pages before the connection thread tears
// down the LocalSet. Synchronous V8 work remains bounded by its own watchdog.
const CONNECTION_PROCESSOR_DRAIN_MS: u64 = 1_000;

// Sent to a client that arrives while the server is at `max_connections`, in
// place of dropping the socket unexplained. The client sees a refusal it can
// retry rather than a bare connection reset.
const CONNECTION_LIMIT_RESPONSE: &str = "HTTP/1.1 503 Service Unavailable\r\n\
    Content-Length: 0\r\nConnection: close\r\n\
    X-Obscura-Reason: max-connections\r\n\r\n";
use crate::types::CdpRequest;
use crate::types::CdpResponse;

struct CdpMessage {
    text: String,
    reply_tx: OutboundSender,
}

enum ServerMessage {
    Cdp(CdpMessage),
    NewConnection {
        reply_tx: OutboundSender,
    },
}

fn browser_close_response(req: &CdpRequest) -> Option<(CdpResponse, bool)> {
    if req.method != "Browser.close" {
        return None;
    }
    let valid = req.params.is_null()
        || req
            .params
            .as_object()
            .is_some_and(serde_json::Map::is_empty);
    let response = if valid {
        CdpResponse::success(req.id, json!({}), req.session_id.clone())
    } else {
        CdpResponse::error(
            req.id,
            -32601,
            "Browser.close supports only empty params".to_string(),
            req.session_id.clone(),
        )
    };
    Some((response, valid))
}

pub async fn start(port: u16, persona: obscura_net::EffectivePersona) -> anyhow::Result<()> {
    start_with_options(port, None, persona).await
}

pub async fn start_with_options(
    port: u16,
    proxy: Option<String>,
    persona: obscura_net::EffectivePersona,
) -> anyhow::Result<()> {
    start_with_full_options(port, proxy, None, persona).await
}

pub async fn start_with_full_options(
    port: u16,
    proxy: Option<String>,
    storage_dir: Option<std::path::PathBuf>,
    persona: obscura_net::EffectivePersona,
) -> anyhow::Result<()> {
    start_with_host(port, "127.0.0.1", proxy, storage_dir, persona).await
}

pub async fn start_with_host(
    port: u16,
    host: &str,
    proxy: Option<String>,
    storage_dir: Option<std::path::PathBuf>,
    persona: obscura_net::EffectivePersona,
) -> anyhow::Result<()> {
    start_with_host_and_security(port, host, proxy, false, storage_dir, persona).await
}

pub async fn start_with_host_and_security(
    port: u16,
    host: &str,
    proxy: Option<String>,
    allow_file_access: bool,
    storage_dir: Option<std::path::PathBuf>,
    persona: obscura_net::EffectivePersona,
) -> anyhow::Result<()> {
    start_with_full_serve_options(
        port, host, proxy, allow_file_access, storage_dir, false, persona,
    )
    .await
}

pub async fn start_with_host_security_and_storage(
    port: u16,
    host: &str,
    proxy: Option<String>,
    allow_file_access: bool,
    storage_dir: Option<std::path::PathBuf>,
    persona: obscura_net::EffectivePersona,
) -> anyhow::Result<()> {
    start_with_full_serve_options(
        port, host, proxy, allow_file_access, storage_dir, false, persona,
    )
    .await
}

/// Full serve entry point that also accepts `allow_private_network` (issue
/// #33). Older entry points default it to `false` so existing callers and
/// public API consumers are unaffected.
pub async fn start_with_full_serve_options(
    port: u16,
    host: &str,
    proxy: Option<String>,
    allow_file_access: bool,
    storage_dir: Option<std::path::PathBuf>,
    allow_private_network: bool,
    persona: obscura_net::EffectivePersona,
) -> anyhow::Result<()> {
    start_with_serve_options_and_limit(
        port,
        host,
        proxy,
        allow_file_access,
        storage_dir,
        allow_private_network,
        DEFAULT_MAX_CONNECTIONS,
        persona,
    )
    .await
}

/// As `start_with_full_serve_options`, with an explicit cap on live CDP
/// connections. Each connection owns an OS thread and its pages' V8 isolates,
/// so this is what bounds the server's thread and memory footprint.
#[allow(clippy::too_many_arguments)]
pub async fn start_with_serve_options_and_limit(
    port: u16,
    host: &str,
    proxy: Option<String>,
    allow_file_access: bool,
    storage_dir: Option<std::path::PathBuf>,
    allow_private_network: bool,
    max_connections: usize,
    persona: obscura_net::EffectivePersona,
) -> anyhow::Result<()> {
    obscura_net::activate_process_persona(&persona)?;
    let ip: std::net::IpAddr = host
        .parse()
        .map_err(|e| anyhow::anyhow!("invalid --host '{}': {}", host, e))?;
    let addr = SocketAddr::new(ip, port);

    // Issue #62: the HTTP control plane (/json/version, /json) must remain
    // reachable even while V8 JS evaluation blocks the tokio LocalSet thread.
    //
    // We use a dedicated OS thread with a blocking std::net::TcpListener so
    // the kernel's accept backlog is always drained promptly. HTTP endpoints
    // are served directly via blocking I/O; WebSocket connections are
    // forwarded to the existing LocalSet for CDP processing.
    let std_listener = std::net::TcpListener::bind(addr)
        .map_err(|e| anyhow::anyhow!("bind {}:{}: {}", host, port, e))?;
    // Non-blocking so the accept thread can alternate between draining the
    // backlog and re-polling parked connections (see the accept thread below).
    std_listener
        .set_nonblocking(true)
        .map_err(|e| anyhow::anyhow!("set_nonblocking: {}", e))?;

    info!("Obscura CDP server listening on ws://{}:{}", host, port);
    info!(
        "DevTools endpoint: ws://{}:{}/devtools/browser",
        host, port
    );
    if allow_file_access {
        info!("file:// navigation enabled (--allow-file-access). Do not expose this port to untrusted networks.");
    }

    let (ws_tx, mut ws_rx) = mpsc::channel::<std::net::TcpStream>(MAX_PENDING_WS_HANDOFFS);

    // Ctrl-C / graceful shutdown coordination.
    let shutdown_flag = Arc::new(AtomicBool::new(false));
    let shutdown_notify = Arc::new(Notify::new());

    // Dedicated accept thread: drains the kernel backlog immediately and
    // handles HTTP endpoints (/json/version, /json, /json/protocol) with
    // blocking I/O so they never contend with the LocalSet's V8 work.
    //
    // A connection that is accepted but never sends its request head
    // (speculative browser preconnects, port probes, slow-loris clients)
    // must not be able to park this single thread in a blocking read: every
    // later connection, including CDP clients like Playwright's
    // connectOverCDP, would then sit in the kernel backlog unanswered until
    // its own connect timeout (issue #715). The thread therefore never
    // blocks on a *stream* — undecided connections are parked and re-polled
    // every ACCEPT_POLL_INTERVAL, and dropped once they outlive
    // SILENT_CONNECTION_TTL without sending a request head.
    //
    // While nothing is parked the thread blocks in accept() itself, the
    // pre-#715 fast path: zero added latency for the next connection and no
    // CPU while idle. Blocking on the listener is safe — it waits for the
    // kernel, not for client bytes. While something is parked, the listener
    // is drained without blocking at least once per 1 ms poll round, far
    // above any real connect rate, so the kernel backlog cannot overflow
    // under a connection burst.
    let accept_flag = shutdown_flag.clone();
    let accept_persona = persona.clone();
    std::thread::Builder::new()
        .name("obscura-cdp-accept".into())
        .spawn(move || {
            let mut pending: Vec<(std::net::TcpStream, std::time::Instant)> = Vec::new();
            while !accept_flag.load(Ordering::Relaxed) {
                if pending.is_empty() {
                    // Fast path: nothing parked, block until a connection
                    // arrives, then classify it in the sweep below.
                    let _ = std_listener.set_nonblocking(false);
                    match std_listener.accept() {
                        Ok((stream, _)) => {
                            let _ = stream.set_nonblocking(true);
                            pending.push((stream, std::time::Instant::now()));
                        }
                        Err(e) => {
                            error!("Accept error: {}", e);
                            // A persistent error (e.g. EMFILE) must not turn
                            // into a log flood while the thread idles.
                            std::thread::sleep(ACCEPT_POLL_INTERVAL);
                        }
                    }
                    let _ = std_listener.set_nonblocking(true);
                } else {
                    // Drain everything the kernel has already queued for us.
                    loop {
                        match std_listener.accept() {
                            Ok((stream, _)) => {
                                let _ = stream.set_nonblocking(true);
                                if pending.len() < MAX_SILENT_PENDING {
                                    pending.push((stream, std::time::Instant::now()));
                                } else {
                                    warn!(
                                        "dropping connection: {} connections parked without a request head",
                                        MAX_SILENT_PENDING
                                    );
                                }
                            }
                            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                            Err(e) => {
                                error!("Accept error: {}", e);
                                break;
                            }
                        }
                    }
                }
                // Give every parked connection a chance to speak; keep the
                // ones still silent and inside the TTL, dispatch the ones
                // with a request head. Dropping a stream closes its socket.
                for (stream, since) in std::mem::take(&mut pending) {
                    if since.elapsed() >= SILENT_CONNECTION_TTL {
                        continue;
                    }
                    match peek_request_head(&stream) {
                        PeekStatus::NotReady => pending.push((stream, since)),
                        PeekStatus::Closed => {}
                        PeekStatus::Head(head) => {
                            if let Err(e) = accept_dispatch(stream, port, &ws_tx, &head, &accept_persona) {
                                if !format!("{}", e).contains("close") {
                                    error!("Accept dispatch error: {}", e);
                                }
                            }
                        }
                    }
                }
                if !pending.is_empty() {
                    std::thread::sleep(ACCEPT_POLL_INTERVAL);
                }
            }
        })?;

    // This context is a configuration and persistence template. Each WebSocket
    // gets an isolated copy with its own cookie jar and HTTP client (#449),
    // while the thread-per-connection layout from #430 still confines that
    // connection's V8 isolates to one OS thread.
    let bctx = obscura_browser::BrowserContext::with_options(
        "default".to_string(),
        persona.clone(),
        obscura_browser::BrowserContextOptions {
            proxy_url: proxy,
            storage_dir,
            allow_file_access,
            allow_private_network,
            ..Default::default()
        },
    );
    let shared_ctx = Arc::new(bctx);
    // Persistence is deliberately separate from the connection template.
    // Cookie deltas are merged here, but new connections always clone the
    // immutable startup snapshot and can never inherit another live client's
    // session state.
    let persistence_ctx = Arc::new(shared_ctx.isolated_copy("persistence".to_string(), true));
    let persistence_lock = Arc::new(std::sync::Mutex::new(()));

    // One graceful-shutdown watcher for the whole server. It flips the accept
    // flag (stopping the accept thread) and wakes every connection processor via
    // `notify_waiters()`. On its own thread so it needs no LocalSet and cannot be
    // starved by a connection's V8 work. Watches SIGTERM as well as Ctrl-C so
    // `docker stop` / `kill` also flush cookies (issue #333).
    {
        let sf = shutdown_flag.clone();
        let sn = shutdown_notify.clone();
        std::thread::Builder::new()
            .name("obscura-cdp-signal".into())
            .spawn(move || {
                if let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    rt.block_on(async {
                        #[cfg(unix)]
                        {
                            use tokio::signal::unix::{signal, SignalKind};
                            match signal(SignalKind::terminate()) {
                                Ok(mut term) => {
                                    tokio::select! {
                                        _ = tokio::signal::ctrl_c() => {}
                                        _ = term.recv() => {}
                                    }
                                }
                                Err(_) => {
                                    let _ = tokio::signal::ctrl_c().await;
                                }
                            }
                        }
                        #[cfg(not(unix))]
                        {
                            let _ = tokio::signal::ctrl_c().await;
                        }
                    });
                }
                sf.store(true, Ordering::Relaxed);
                sn.notify_waiters();
            })
            .ok();
    }

    // Force V8 and its process-global isolate tables (the leaptiering
    // JSDispatchTable / external-pointer tables) to initialize once on this main
    // thread before any connection thread creates an isolate. Creating the very
    // first isolate off the main thread segfaults inside
    // InitializeBuiltinJSDispatchTable (#430 thread-per-connection). Building and
    // dropping one runtime here does the one-time setup single-threaded.
    drop(obscura_js::runtime::ObscuraJsRuntime::new(persona));

    cap_malloc_arenas();

    // Live CDP connections, incremented on accept and decremented when a
    // connection thread exits (see `run_connection`).
    let live_connections = Arc::new(AtomicUsize::new(0));
    info!("Connection limit: {}", max_connections);

    // Accept loop: hand each WebSocket connection to its own OS thread so its
    // pages' isolates live on a dedicated thread.
    loop {
        let stream = tokio::select! {
            stream = ws_rx.recv() => stream,
            _ = shutdown_notify.notified() => None,
        };
        let stream = match stream {
            Some(s) => s,
            None => break,
        };
        // Nagle off + nonblocking on the std socket before it moves to the
        // connection thread. CDP exchanges many small (~100-byte) frames during
        // newPage()/navigate; with Nagle on, each small write waits on an ACK or
        // the 40ms delayed-ACK timer (~90ms on newPage, ~30ms on goto).
        stream
            .set_nonblocking(true)
            .map_err(|e| error!("set_nonblocking on WS stream: {}", e))
            .ok();
        stream
            .set_nodelay(true)
            .map_err(|e| error!("set_nodelay on WS stream: {}", e))
            .ok();
        // Reserve a slot before spawning. `fetch_update` (rather than a load
        // then a store) keeps the check atomic against the accept thread
        // handing off the next stream concurrently.
        let reserved = live_connections
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < max_connections).then_some(n + 1)
            })
            .is_ok();
        if !reserved {
            warn!(
                "refusing CDP connection: at --max-connections ({})",
                max_connections
            );
            refuse_connection(stream);
            continue;
        }
        run_connection(
            stream,
            shared_ctx.clone(),
            persistence_ctx.clone(),
            persistence_lock.clone(),
            shutdown_notify.clone(),
            live_connections.clone(),
        );
    }

    // Server is shutting down. Connection threads are detached, so saving the
    // jar right here would race them: a connection still writing a Set-Cookie
    // loses it, and the process then exits and kills the thread mid-flight.
    // Before the per-connection move, the single processor saved on its own way
    // out, ordered against all connection work on one LocalSet -- draining here
    // is what restores that ordering. `notify_waiters` above has already woken
    // every processor, so this is bounded in practice; the deadline only covers
    // a connection wedged in V8, where its own command watchdog is the backstop.
    let drain_deadline =
        tokio::time::Instant::now() + tokio::time::Duration::from_millis(SHUTDOWN_DRAIN_MS);
    loop {
        let live = live_connections.load(Ordering::Acquire);
        if live == 0 {
            break;
        }
        if tokio::time::Instant::now() >= drain_deadline {
            warn!(
                "shutting down with {} connection(s) still live after {}ms; \
                 cookies they write from here are lost",
                live, SHUTDOWN_DRAIN_MS
            );
            break;
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;
    }
    persistence_ctx.save_cookies();
    Ok(())
}

/// Cap the number of per-thread malloc arenas glibc will create.
///
/// glibc hands each new thread its own 64 MiB arena (up to 8x cores). With one
/// thread per connection that is the dominant per-connection memory term:
/// measured with `reliability/conn-scale.py`, 100 connections each running JS
/// reserve 90.0 GiB of address space uncapped and 83.5 GiB capped, and 100 idle
/// connections go from 65 MiB of reserved address space per connection to
/// 2.0 MiB.
///
/// For scale: at the same 100-connection JS workload `main` (one shared
/// isolate) reserves 83.6 GiB, so with the cap this server is level with it on
/// address space. Most of that total is V8's process-wide sandbox, which `main`
/// pays too as soon as it runs any JS at all.
///
/// The resident-set effect matters more than the reservation: freed chunks stay
/// in their arena rather than returning to the OS, so RSS tracks the *peak*
/// number of concurrent connections and never comes back down, which reads as a
/// leak. Measured in the container image against Google Maps, four concurrent
/// connections per round: 350 / 619 / 826 MiB over three rounds uncapped and
/// still climbing linearly, versus 166 / 235 / 269 MiB capped, on a
/// decelerating curve.
///
/// Two arenas cost no measurable throughput here (8 concurrent connections x 12
/// navigations: 1.53s uncapped, 1.50s capped): V8 allocates the JS heap through
/// its own allocator, and the Rust side is dominated by network I/O rather than
/// malloc traffic. Only `serve` calls this, and it owns the process. Respects a
/// caller-set `MALLOC_ARENA_MAX`.
fn cap_malloc_arenas() {
    #[cfg(target_env = "gnu")]
    {
        if std::env::var_os("MALLOC_ARENA_MAX").is_some() {
            return;
        }
        // M_ARENA_MAX is not exported by the libc crate.
        const M_ARENA_MAX: libc::c_int = -8;
        // SAFETY: mallopt is thread-safe; called once here before any
        // connection thread exists.
        if unsafe { libc::mallopt(M_ARENA_MAX, 2) } != 1 {
            warn!("mallopt(M_ARENA_MAX) failed; memory will scale with peak concurrency");
        }
    }
}

/// Return free glibc heap pages after the last CDP connection tears down.
///
/// A connection owns its pages, DOMs, render buffers, and V8 isolates. Dropping
/// those objects releases the allocations, but glibc normally keeps the freed
/// pages mapped for reuse, so a server that becomes idle can retain its peak RSS
/// indefinitely (#873). Trimming only when this is the final live connection
/// avoids imposing a process-wide allocator pause on active clients.
fn release_idle_connection_memory() {
    #[cfg(target_env = "gnu")]
    {
        // SAFETY: malloc_trim is process-wide and thread-safe. The caller has
        // already dropped this connection's LocalSet and Tokio runtime.
        unsafe {
            libc::malloc_trim(0);
        }
    }
}

/// Run one WebSocket connection on its own OS thread: a `current_thread` tokio
/// runtime + `LocalSet` hosting this connection's `cdp_processor` (with its own
/// `CdpContext` and pages) and its frame reader. Confining a connection's pages
/// to one thread is what removes the #430 abort; the interception handshake and
/// the nav `spawn_local` all stay on this one thread, so no cross-thread V8
/// plumbing is needed.
fn run_connection(
    std_stream: std::net::TcpStream,
    context_template: Arc<obscura_browser::BrowserContext>,
    persistence_context: Arc<obscura_browser::BrowserContext>,
    persistence_lock: Arc<std::sync::Mutex<()>>,
    shutdown_notify: Arc<Notify>,
    live_connections: Arc<AtomicUsize>,
) {
    // Releases the slot reserved by the accept loop when the thread unwinds,
    // however it exits — clean close, error return, or panic. A plain
    // decrement at the end of the closure would leak slots on the early
    // returns below until the cap wedged the server shut.
    struct SlotGuard(Option<Arc<AtomicUsize>>);
    impl SlotGuard {
        fn release(&mut self) -> Option<usize> {
            self.0
                .take()
                .map(|counter| counter.fetch_sub(1, Ordering::AcqRel).saturating_sub(1))
        }
    }
    impl Drop for SlotGuard {
        fn drop(&mut self) {
            self.release();
        }
    }

    let slot = live_connections.clone();
    let spawned = std::thread::Builder::new()
        .name("obscura-cdp-conn".into())
        .spawn(move || {
            let mut slot_guard = SlotGuard(Some(slot));
            let default_context = Arc::new(
                context_template.isolated_copy("default".to_string(), true),
            );
            let initial_cookies = default_context.cookie_jar.snapshot();
            let persisted_context = default_context.clone();
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(r) => r,
                Err(e) => {
                    error!("connection runtime build failed: {}", e);
                    return;
                }
            };
            let local = tokio::task::LocalSet::new();
            local.block_on(&rt, async move {
                let tokio_stream = match TcpStream::from_std(std_stream) {
                    Ok(s) => s,
                    Err(e) => {
                        error!("TcpStream::from_std failed: {}", e);
                        return;
                    }
                };
                let (msg_tx, msg_rx) = mpsc::unbounded_channel::<ServerMessage>();
                let mut processor = tokio::task::spawn_local(cdp_processor(
                    msg_rx,
                    default_context,
                    shutdown_notify,
                ));
                if let Err(e) = handle_connection_ws(tokio_stream, msg_tx).await {
                    error!("WebSocket connection error: {}", e);
                }
                // Dropping the handler's input sender closes the processor
                // channel. Let it run its Fetch-pause cleanup before using
                // abort as a bounded backstop for an in-flight operation.
                if tokio::time::timeout(
                    tokio::time::Duration::from_millis(CONNECTION_PROCESSOR_DRAIN_MS),
                    &mut processor,
                )
                .await
                .is_err()
                {
                    processor.abort();
                    let _ = processor.await;
                }
            });

            // `LocalSet` owns any detached local navigation tasks, and the
            // runtime owns their scheduler allocations. Drop both before the
            // idle trim so every page allocation is eligible to be returned.
            drop(local);
            drop(rt);

            // Apply only this connection's cookie changes to the persistence
            // template. Unchanged cookies cannot overwrite another connection's
            // updates, while explicit deletes and replacements still persist.
            if persistence_context.storage_dir.is_some() {
                let _guard = persistence_lock.lock().unwrap_or_else(|e| e.into_inner());
                persistence_context.cookie_jar.apply_snapshot_delta(
                    &initial_cookies,
                    &persisted_context.cookie_jar.snapshot(),
                );
                persistence_context.save_cookies();
            }

            drop(persisted_context);
            if slot_guard.release() == Some(0) {
                release_idle_connection_memory();
            }
        });

    // The closure never ran, so its `SlotGuard` never existed: release the
    // reserved slot here or the cap drifts down on every failed spawn.
    if let Err(e) = spawned {
        error!("connection thread spawn failed: {}", e);
        live_connections.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Turn away a connection that arrived while the server was at its limit.
///
/// Best-effort: the socket is going away either way, so a failed write just
/// means the client sees a reset instead of the 503.
fn refuse_connection(stream: std::net::TcpStream) {
    use std::io::{Read, Write};
    let mut stream = stream;
    let _ = stream.set_nonblocking(false);

    // The accept thread only peeked at the WebSocket handshake. Consume its
    // bounded HTTP header before closing: Windows resets a socket closed with
    // unread receive data, which can discard the queued 503 response.
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_millis(100)));
    let mut request = [0u8; HTTP_PEEK_BUF];
    let mut received = 0;
    while received < request.len() {
        match stream.read(&mut request[received..]) {
            Ok(0) => break,
            Ok(n) => {
                received += n;
                if request[..received].windows(4).any(|end| end == b"\r\n\r\n") {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    let _ = stream.write_all(CONNECTION_LIMIT_RESPONSE.as_bytes());
    let _ = stream.flush();
    let _ = stream.shutdown(std::net::Shutdown::Write);
}

const HTTP_PEEK_BUF: usize = 4096;

/// How long a freshly accepted connection may sit without sending a request
/// head before the accept thread drops it. Real clients send their handshake
/// immediately after connecting; only probes and preconnects linger.
const SILENT_CONNECTION_TTL: std::time::Duration = std::time::Duration::from_secs(10);

/// How often the accept thread re-polls parked connections that have not sent
/// a request head yet. Also the retry delay on a persistent accept error, so
/// it cannot become a log flood. Only paid while something is actually
/// parked; an idle server blocks in accept() and polls nothing.
const ACCEPT_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(1);

/// Cap on connections parked without a request head. Bounds the accept
/// thread's polling work and the server's fd usage under probe floods.
const MAX_SILENT_PENDING: usize = 256;

/// Result of polling a freshly accepted connection for its request head.
enum PeekStatus {
    /// No classifiable request head yet; poll again next accept round.
    NotReady,
    /// Peer went away without sending a full head.
    Closed,
    /// A classifiable request head.
    Head(String),
}

/// Peek — without consuming — at a freshly accepted connection's request
/// head. `GET` requests are only classified once the terminating blank line
/// has arrived, so `/json` route matching never sees a truncated head;
/// anything that cannot be a `GET` is handed over immediately so non-HTTP
/// garbage still gets tungstenite's prompt rejection instead of waiting out
/// the silent-connection TTL.
fn peek_request_head(stream: &std::net::TcpStream) -> PeekStatus {
    let mut buf = [0u8; HTTP_PEEK_BUF];
    let n = match stream.peek(&mut buf) {
        Ok(0) => return PeekStatus::Closed,
        Ok(n) => n,
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return PeekStatus::NotReady,
        Err(_) => return PeekStatus::Closed,
    };
    let head = &buf[..n];
    if n >= 4 && head[..4] != *b"GET " {
        return PeekStatus::Head(String::from_utf8_lossy(head).into_owned());
    }
    // A head that overflows the peek buffer is classified with what arrived,
    // matching the pre-polling behavior for oversized headers.
    let complete = n == HTTP_PEEK_BUF || head.windows(4).any(|w| w == b"\r\n\r\n");
    if !complete {
        return PeekStatus::NotReady;
    }
    PeekStatus::Head(String::from_utf8_lossy(head).into_owned())
}

/// Dispatch a freshly-accepted TCP connection on the dedicated accept thread.
///
/// The connection's request head has already been peeked by the accept loop
/// (`peek_request_head`) and is passed in as `head`:
/// - HTTP (`GET /json/*`): serve synchronously via blocking I/O so the
///   response is never stalled by the LocalSet.
/// - WebSocket: forward to the LocalSet for CDP processing.
fn accept_dispatch(
    stream: std::net::TcpStream,
    port: u16,
    ws_tx: &mpsc::Sender<std::net::TcpStream>,
    head: &str,
    persona: &obscura_net::EffectivePersona,
) -> anyhow::Result<()> {
    let endpoint = if head.contains("/json/version") {
        Some("version")
    } else if head.contains("/json/list") || head.contains("/json\r\n") || head.contains("/json HTTP") {
        Some("list")
    } else if head.contains("/json/protocol") {
        Some("protocol")
    } else {
        None
    };

    if let Some(ep) = endpoint {
        // The request head is already sitting in the kernel receive buffer;
        // switch back to blocking mode for the synchronous /json serve.
        let _ = stream.set_nonblocking(false);
        return handle_http_json_blocking(stream, port, ep, head, persona);
    }
    // Fall through: GET request that isn't a /json endpoint → treat as
    // WebSocket upgrade (Chromium DevTools clients issue GET with
    // Upgrade: websocket).

    // Try to hand off the WS stream to the LocalSet. If the bounded channel
    // is full the LocalSet is saturated — drop the connection cleanly
    // rather than blocking the accept thread (which would freeze the HTTP
    // control plane that this whole rework exists to keep alive). The
    // dropped `stream` closes itself; the client will see ECONNRESET and
    // can retry.
    ws_tx
        .try_send(stream)
        .map_err(|e| match e {
            mpsc::error::TrySendError::Full(_) => {
                warn!("WS handoff channel full ({}); dropping new WebSocket connection", MAX_PENDING_WS_HANDOFFS);
                anyhow::anyhow!("ws handoff channel full")
            }
            mpsc::error::TrySendError::Closed(_) => anyhow::anyhow!("accept channel closed"),
        })
}

/// Serve an HTTP `/json/*` endpoint with blocking I/O on the accept thread.
fn handle_http_json_blocking(
    mut stream: std::net::TcpStream,
    port: u16,
    endpoint: &str,
    request_head: &str,
    persona: &obscura_net::EffectivePersona,
) -> anyhow::Result<()> {
    use std::io::{Read, Write};

    let mut buf = vec![0u8; 4096];
    let _ = stream.read(&mut buf)?;
    let authority = websocket_authority(request_head, port);

    let body = match endpoint {
        "version" => serde_json::to_string_pretty(&json!({
            "Browser": format!("Chrome/{}", persona.full_version()),
            "Protocol-Version": "1.3",
            "User-Agent": persona.user_agent(),
            "V8-Version": "14.5.0.0",
            "WebKit-Version": "537.36",
            "webSocketDebuggerUrl": format!("ws://{}/devtools/browser", authority),
        }))?,
        "list" => serde_json::to_string_pretty(&json!([{
            "description": "",
            "devtoolsFrontendUrl": "",
            "id": "page-1",
            "title": "",
            "type": "page",
            "url": "about:blank",
            "webSocketDebuggerUrl": format!("ws://{}/devtools/page/page-1", authority),
        }]))?,
        "protocol" => {
            serde_json::to_string_pretty(&json!({ "version": { "major": "1", "minor": "3" } }))?
        }
        _ => "{}".to_string(),
    };

    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(), body,
    );
    stream.write_all(resp.as_bytes())?;
    stream.flush()?;
    Ok(())
}

fn websocket_authority(request_head: &str, port: u16) -> String {
    request_head
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("host"))
        .map(|(_, value)| value.trim())
        .filter(|value| {
            let Ok(url) = url::Url::parse(&format!("http://{value}/")) else {
                return false;
            };
            url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.path() == "/"
                && url.query().is_none()
                && url.fragment().is_none()
        })
        .map(str::to_owned)
        .unwrap_or_else(|| format!("127.0.0.1:{port}"))
}

/// Per-connection CDP processor. Each connection runs its own processor (with
/// its own `CdpContext` and pages) on its own OS thread, so every page's V8
/// isolate is confined to a single thread. This removes the #430 abort by
/// construction: V8's `heap->isolate() == Isolate::TryGetCurrent()` invariant is
/// per-thread, so two connections' isolates can never collide. All processors
/// own isolated `BrowserContext` (cookie jar and HTTP client). Cookie deltas are
/// merged into the persistence template when the connection thread exits.
struct InterceptedPause {
    stage: obscura_js::ops::InterceptionStage,
    redirect_response: bool,
    resolver: tokio::sync::oneshot::Sender<obscura_js::ops::InterceptResolution>,
}
type InterceptedPauses = HashMap<(Option<String>, String), InterceptedPause>;

async fn cdp_processor(
    mut rx: mpsc::UnboundedReceiver<ServerMessage>,
    default_context: Arc<obscura_browser::BrowserContext>,
    shutdown_notify: Arc<Notify>,
) {
    let mut ctx = CdpContext::new_with_shared_context(default_context);
    let (itx, irx) = mpsc::unbounded_channel::<crate::domains::fetch::RoutedInterceptedRequest>();
    ctx.intercept_tx = Some(itx);
    let mut intercept_rx: Option<mpsc::UnboundedReceiver<crate::domains::fetch::RoutedInterceptedRequest>> = Some(irx);
    let mut intercepted_paused: InterceptedPauses = HashMap::new();

    // Issue #19 follow-up: messages deferred from inside
    // `process_with_interception` because routing them through
    // `process_cdp_message → dispatch` while a nav was in flight would have
    // tripped V8's TryGetCurrent invariant. Drained at the top of each
    // outer iteration so they get processed sequentially with no other nav
    // in flight.
    let mut deferred: std::collections::VecDeque<ServerMessage> =
        std::collections::VecDeque::new();

    // Graceful shutdown: one signal watcher on the accept side flips the flag
    // and calls `notify_waiters()`. Polled once here (via the select! below) it
    // registers and stays registered across iterations, so a later
    // `notify_waiters()` wakes this processor even while it is mid-dispatch.
    let mut shutdown = Box::pin(shutdown_notify.notified());
    // Chromium's PageHandler receives compositor video frames continuously.
    // Obscura has no separate compositor thread yet, so active screencasts get
    // a bounded 30 Hz opportunity on this connection's owning LocalSet.
    let mut screencast_tick = tokio::time::interval(tokio::time::Duration::from_millis(33));
    screencast_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut connection_reply_tx: Option<OutboundSender> = None;
    // A real browser renderer continues servicing timers, networking, posted
    // tasks, and animation callbacks while its DevTools client is silent. Keep
    // one wake-driven deno_core turn armed after work may have been scheduled;
    // the future parks on the runtime's own waker and is cancelled whenever a
    // higher-priority protocol command arrives. Full idle disarms it until the
    // next command/navigation, so static pages consume no polling budget.
    let mut runtime_pump_armed = false;
    let mut runtime_pump_error_streak = 0_u8;

    loop {
        // Outbound overflow and writer failure are connection-fatal. Do not
        // keep executing queued commands whose responses can no longer reach
        // the client; dropping them also prevents an implicit replay contract.
        if connection_reply_tx
            .as_ref()
            .is_some_and(OutboundSender::is_closed)
        {
            break;
        }
        intercepted_paused.retain(|_, pause| !pause.resolver.is_closed());
        cleanup_detached_fetch_owners(&mut ctx, &mut intercepted_paused);
        // Drain any deferred messages from the previous interception window
        // before pulling new ones off the wire. Each is processed with no
        // nav-task spawn_local in flight, so this connection's only entered
        // Isolate is the one dispatch is about to touch.
        let msg = if let Some(d) = deferred.pop_front() {
            Some(d)
        } else {
            let screencast_active = has_active_screencast(&ctx);
            let has_intercept_rx = intercept_rx.is_some();
            let network_notifiers = ctx.pages.iter().map(|page| page.network_teardown_notify.clone()).collect();
            tokio::select! {
                biased;
                msg = rx.recv() => match msg {
                    Some(m) => Some(m),
                    None => break,
                },
                _ = &mut shutdown => {
                    tracing::info!("Shutdown signal received (connection processor)");
                    break;
                },
                _ = wait_network_teardown(network_notifiers) => {
                    sync_live_page_network_events(&mut ctx);
                    forward_pending_events(&mut ctx, connection_reply_tx.as_ref());
                    None
                },
                pump_result = pump_live_page_event_loop(&mut ctx), if runtime_pump_armed => {
                    match pump_result {
                        Ok(reached_idle) => {
                            runtime_pump_error_streak = 0;
                            runtime_pump_armed = !reached_idle;
                        }
                        Err(error) => {
                            runtime_pump_error_streak = runtime_pump_error_streak.saturating_add(1);
                            runtime_pump_armed = runtime_pump_error_streak <= 3
                                && ctx.pages.iter().any(|page| page.has_js());
                            tracing::warn!("autonomous page task failed: {error}");
                            tokio::task::yield_now().await;
                        }
                    }
                    service_live_page_render_resources(&mut ctx);
                    sync_live_page_network_events(&mut ctx);
                    dispatch::drain_runtime_events(&mut ctx);
                    dispatch::drain_binding_calls(&mut ctx);
                    dispatch::drain_frame_events(&mut ctx);
                    forward_pending_events(&mut ctx, connection_reply_tx.as_ref());
                    if let (Some(reply_tx), Some((session_id, url, method, body))) = (
                        connection_reply_tx.as_ref(),
                        take_live_pending_navigation(&ctx),
                    ) {
                        let navigation = json!({
                            "id": 0,
                            "method": "Page.navigate",
                            "params": {"url": url, "__method": method, "__body": body},
                            "sessionId": session_id,
                        })
                        .to_string();
                        process_with_interception(
                            &navigation,
                            &mut ctx,
                            reply_tx,
                            &mut rx,
                            &mut intercept_rx,
                            &mut intercepted_paused,
                            &mut deferred,
                            false,
                        )
                        .await;
                        runtime_pump_armed = ctx.pages.iter().any(|page| page.has_js());
                    }
                    None
                },
                Some(intercepted) = async {
                    if let Some(ref mut receiver) = intercept_rx {
                        receiver.recv().await
                    } else {
                        std::future::pending().await
                    }
                }, if has_intercept_rx => {
                    if let Some(reply_tx) = connection_reply_tx.as_ref() {
                        emit_routed_intercepted_request(intercepted, &mut ctx, reply_tx, &mut intercepted_paused);
                    } else {
                        let _ = intercepted.request.resolver.send(obscura_js::ops::InterceptResolution::Fail { reason: "Aborted".into() });
                    }
                    None
                },
                _ = screencast_tick.tick(), if screencast_active => {
                    pump_and_forward_screencast_frames(
                        &mut ctx,
                        connection_reply_tx.as_ref(),
                    ).await;
                    None
                }
            }
        };

        let Some(msg) = msg else {
            continue;
        };

        // The outbound writer can fail while this task is parked in select.
        // Recheck after wakeup before any queued command can mutate page or
        // Fetch state.
        if connection_reply_tx
            .as_ref()
            .is_some_and(OutboundSender::is_closed)
        {
            break;
        }

        match msg {
            ServerMessage::NewConnection { reply_tx } => {
                connection_reply_tx = Some(reply_tx.clone());
                let _ = reply_tx.send(
                    json!({"__init": true})
                        .to_string(),
                );
            }
            ServerMessage::Cdp(cdp_msg) => {
                // Route every Page.navigate through the spawn-and-defer path,
                // not just intercepted ones. Holding the V8 lock across a
                // multi-second navigate inside the regular dispatch wedges the
                // entire processor (40-site sweep: 39/40 timeouts). Spawning
                // navigation lets `cdp_processor` keep multiplexing other CDP
                // messages via the `process_with_interception` select loop;
                // unrelated requests get deferred only briefly and are drained
                // as soon as the nav settles.
                let is_navigation = is_navigate_method(&cdp_msg.text);

                if is_navigation {
                    process_with_interception(
                        &cdp_msg.text, &mut ctx, &cdp_msg.reply_tx, &mut rx,
                        &mut intercept_rx, &mut intercepted_paused,
                        &mut deferred, true,
                    ).await;
                } else {
                    let fetch_was_resolved = (cdp_msg.text.contains("Fetch.") || cdp_msg.text.contains("Network.getResponseBody"))
                        && handle_fetch_resolution(
                            &cdp_msg.text,
                            &mut ctx,
                            &cdp_msg.reply_tx,
                            &mut intercepted_paused,
                        );
                    if !fetch_was_resolved {
                        process_cdp_message(&cdp_msg.text, &mut ctx, &cdp_msg.reply_tx).await;
                    }
                }
            }
        }

        // Dispatch may have created a page or scheduled new asynchronous work.
        // A single live isolate is the connection's current active target; the
        // pump will park cheaply if its next task is a distant timer.
        runtime_pump_armed = ctx.pages.iter().any(|page| page.has_js());
        runtime_pump_error_streak = 0;

    }

    if let Some(receiver) = intercept_rx.as_mut() {
        receiver.close();
        while let Ok(routed) = receiver.try_recv() {
            let _ = routed.request.resolver.send(obscura_js::ops::InterceptResolution::Fail { reason: "Aborted".into() });
        }
    }
    for (_, pause) in intercepted_paused.drain() {
        let _ = pause.resolver.send(obscura_js::ops::InterceptResolution::Fail { reason: "Aborted".into() });
    }

    // The connection thread merges this context's cookie delta into the
    // persistence template after the processor stops.
    let _ = &ctx;
}

fn cleanup_detached_fetch_owners(ctx: &mut CdpContext, paused: &mut InterceptedPauses) {
    let detached: Vec<_> = ctx.fetch_intercept.owners.iter().filter(|(page_id, session)| {
        !ctx.has_page(page_id) || session.as_ref().is_some_and(|sid| ctx.sessions.get(sid) != Some(*page_id))
    }).map(|(page_id, session)| (page_id.clone(), session.clone())).collect();
    for (page_id, session) in detached {
        ctx.fetch_intercept.owners.remove(&page_id);
        if let Some(page) = ctx.get_page_mut(&page_id) {
            page.intercept_block_patterns.clear();
            page.intercept_request_patterns.clear();
            page.set_intercept_response_patterns(Vec::new());
            page.enable_intercept(false);
        }
        let keys: Vec<_> = paused.keys().filter(|(sid, _)| sid == &session).cloned().collect();
        for key in keys {
            if let Some(pause) = paused.remove(&key) {
                let _ = pause.resolver.send(obscura_js::ops::InterceptResolution::Fail { reason: "Aborted".into() });
            }
        }
    }
    ctx.fetch_intercept.enabled = !ctx.fetch_intercept.owners.is_empty();
}

fn abort_navigating_fetch_owner_for_lifecycle(
    text: &str,
    ctx: &mut CdpContext,
    paused: &mut InterceptedPauses,
) -> bool {
    let Ok(request) = serde_json::from_str::<CdpRequest>(text) else { return false; };
    let Some(page_id) = ctx.navigating_page_id.clone() else { return false; };
    let affects_owner = match request.method.as_str() {
        "Target.closeTarget" => request.params.as_object().is_some_and(|params|
            params.len() == 1 && params.get("targetId").and_then(serde_json::Value::as_str) == Some(page_id.as_str())),
        "Target.detachFromTarget" => {
            let session = request.params.get("sessionId").and_then(serde_json::Value::as_str);
            ctx.fetch_intercept.owners.get(&page_id).and_then(Option::as_deref) == session
                && session.is_some()
        }
        _ => false,
    };
    if !affects_owner { return false; }
    let Some(owner) = ctx.fetch_intercept.owners.remove(&page_id) else { return false; };
    ctx.pending_fetch_policy_cleanup.insert(page_id);
    ctx.fetch_intercept.enabled = !ctx.fetch_intercept.owners.is_empty();
    if !ctx.fetch_intercept.enabled { ctx.fetch_intercept.patterns.clear(); }
    let keys: Vec<_> = paused.keys().filter(|(session, _)| session == &owner).cloned().collect();
    for key in keys {
        if let Some(pause) = paused.remove(&key) {
            let _ = pause.resolver.send(obscura_js::ops::InterceptResolution::Fail { reason: "Aborted".into() });
        }
    }
    true
}

fn emit_routed_intercepted_request(
    routed: crate::domains::fetch::RoutedInterceptedRequest,
    ctx: &mut CdpContext,
    reply_tx: &OutboundSender,
    paused: &mut InterceptedPauses,
) {
    // The navigating Page may temporarily be outside ctx.pages. Its session
    // mapping remains authoritative; never substitute another live Page.
    let valid = match &routed.session_id {
        Some(session) => ctx.sessions.get(session) == Some(&routed.page_id),
        None => ctx.has_page(&routed.page_id),
    };
    if valid && ctx.fetch_intercept.owners.get(&routed.page_id) != Some(&routed.session_id) {
        let _ = routed.request.resolver.send(obscura_js::ops::InterceptResolution::Continue {
            url: None, method: None, headers: None, body: None,
        });
    } else if valid {
        let loader_id = ctx.current_loader_ids.get(&routed.page_id).cloned().unwrap_or_else(|| format!("loader-blank-{}", routed.page_id));
        let loader_id = if ctx.navigating_page_id.as_ref() == Some(&routed.page_id) {
            ctx.navigating_document_loader.as_ref().filter(|(old_generation, _)| routed.request.document_generation > *old_generation)
                .map(|(_, loader)| loader.clone()).unwrap_or(loader_id)
        } else { loader_id };
        let loader_id = ctx.document_loaders.entry((routed.page_id.clone(), routed.request.document_generation)).or_insert(loader_id).clone();
        let document_url = routed.request.document_url.clone();
        let owner_key = (routed.page_id, routed.request.network_id.clone());
        let owner_session = routed.session_id.clone();
        if emit_intercepted_request(routed.request, &routed.frame_id, &loader_id, &document_url,
            routed.session_id, reply_tx, paused)
        {
            ctx.network_owners.insert(owner_key, owner_session);
        }
    } else {
        let _ = routed.request.resolver.send(obscura_js::ops::InterceptResolution::Fail { reason: "Aborted".into() });
    }
}

fn raw_header_bytes_to_cdp_string(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(value) => value.to_owned(),
        Err(_) => bytes.iter().map(|byte| char::from(*byte)).collect(),
    }
}

fn emit_intercepted_request(
    intercepted: obscura_js::ops::InterceptedRequest,
    frame_id: &str,
    loader_id: &str,
    document_url: &str,
    session_id: Option<String>,
    reply_tx: &OutboundSender,
    intercepted_paused: &mut InterceptedPauses,
) -> bool {
    if intercepted.resolver.is_closed() { return false; }
    let emit_request_start = match intercepted.stage {
        obscura_js::ops::InterceptionStage::Request => {
            if intercepted.network_start.compare_exchange(0, 1,
                std::sync::atomic::Ordering::SeqCst, std::sync::atomic::Ordering::SeqCst).is_err() { return false; }
            true
        }
        obscura_js::ops::InterceptionStage::Response => intercepted.network_start.compare_exchange(0, 1,
            std::sync::atomic::Ordering::SeqCst, std::sync::atomic::Ordering::SeqCst).is_ok(),
    };
    tracing::info!(
        "INTERCEPTION: requestPaused for {} {} (sending to client)",
        intercepted.method,
        intercepted.url
    );
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    let request = json!({
        "url": intercepted.url,
        "method": intercepted.method,
        "headers": intercepted.headers,
        "rawHeaders": intercepted.request_raw_headers,
        "hasPostData": intercepted.request_body_size > 0,
        "bodySize": intercepted.request_body_size,
        "initialPriority": "High",
        "referrerPolicy": "strict-origin-when-cross-origin",
    });
    let request_will_be_sent = json!({
        "method": "Network.requestWillBeSent",
        "params": {
            "requestId": intercepted.network_id,
            "loaderId": loader_id,
            "documentURL": document_url,
            "redirectHasExtraInfo": false,
            "redirectResponse": intercepted.redirect_response.as_ref().and_then(|exchange| exchange.response.as_ref().map(|response| json!({
                "url": exchange.url, "status": response.status, "statusText": "", "headers": response.headers,
                "rawHeaders": response.raw_headers, "bodyRequestId": exchange.body_request_id,
                "mimeType": response.headers.get("content-type").cloned().unwrap_or_default(),
            }))),
            "request": request,
            "timestamp": now,
            "wallTime": now,
            "initiator": {"type": "script"},
            "type": intercepted.resource_type,
            "frameId": frame_id,
        },
        "sessionId": session_id,
    });
    if emit_request_start && reply_tx.send(request_will_be_sent.to_string()).is_err() {
        let _ = intercepted.resolver.send(obscura_js::ops::InterceptResolution::Fail { reason: "Aborted".into() });
        return false;
    }

    let response_headers = intercepted.response_raw_headers.as_ref().map(|capture| capture.fields.iter()
        .map(|field| json!({
            "name": raw_header_bytes_to_cdp_string(&field.name),
            "value": raw_header_bytes_to_cdp_string(&field.value),
        })).collect::<Vec<_>>()).or_else(|| intercepted.response_headers.as_ref().map(|headers| headers.iter()
            .map(|(name, value)| json!({"name": name, "value": value})).collect::<Vec<_>>()));
    let redirect_response = intercepted.response_status_code.is_some_and(|status| (300..400).contains(&status))
        && intercepted.response_headers.as_ref().is_some_and(|headers| headers.keys().any(|name| name.eq_ignore_ascii_case("location")));

    let request_paused = json!({
        "method": "Fetch.requestPaused",
        "params": {
            "requestId": intercepted.request_id,
            "request": request,
            "frameId": frame_id,
            "resourceType": intercepted.resource_type,
            "networkId": intercepted.network_id,
            "redirectedRequestId": intercepted.redirected_request_id,
            "responseErrorReason": null,
            "responseStatusCode": intercepted.response_status_code,
            "responseStatusText": intercepted.response_status_code.map(|_| ""),
            "responseHeaders": response_headers,
            "responseRawHeaders": intercepted.response_raw_headers,
            "responseBodyRequestId": intercepted.response_body_request_id,
        },
        "sessionId": session_id,
    });
    if reply_tx.send(request_paused.to_string()).is_err() {
        let _ = intercepted.resolver.send(obscura_js::ops::InterceptResolution::Fail { reason: "Aborted".into() });
        return false;
    }
    intercepted_paused.insert((session_id, intercepted.request_id), InterceptedPause {
        stage: intercepted.stage,
        redirect_response,
        resolver: intercepted.resolver,
    });
    true
}

async fn wait_network_teardown(notifiers: Vec<Arc<Notify>>) {
    if notifiers.is_empty() { std::future::pending::<()>().await; }
    let waiters: Vec<_> = notifiers.into_iter().map(|notify| Box::pin(notify.notified_owned())).collect();
    futures_util::future::select_all(waiters).await;
}

async fn pump_live_page_event_loop(ctx: &mut CdpContext) -> Result<bool, String> {
    // Several pages on one connection can be live at once (#872), each with its
    // own event loop, so pump a turn on *every* live page rather than only the
    // first. The pump stays armed until all live pages report idle.
    let live_ids: Vec<String> = ctx
        .pages
        .iter()
        .filter(|page| page.has_js())
        .map(|page| page.id.clone())
        .collect();
    if live_ids.is_empty() {
        return Ok(true);
    }
    let mut all_idle = true;
    for page_id in live_ids {
        if let Some(page) = ctx.get_page_mut(&page_id) {
            all_idle &= page.run_autonomous_event_loop_turn().await?;
        }
    }
    Ok(all_idle)
}

/// Apply finished background render-resource loads and start loads for
/// resources the last layout/paint missed, for every live page. Runs before a
/// command (so it observes bytes that landed while the client was silent),
/// after a command (so its layout misses start loading immediately) and after
/// each autonomous pump turn.
fn service_live_page_render_resources(ctx: &mut CdpContext) {
    for page in ctx.pages.iter_mut().filter(|page| page.has_js()) {
        page.queue_pending_render_resources();
    }
}

fn sync_live_page_network_events(ctx: &mut CdpContext) {
    // Emit script-initiated network events for every live page, each attributed
    // to its own session/frame — not just the first live page (#872).
    let live_ids: Vec<String> = ctx
        .pages
        .iter()
        .map(|page| page.id.clone())
        .collect();
    for page_id in live_ids {
        let Some(session_id) = ctx
            .sessions
            .iter()
            .find(|(_, pid)| *pid == &page_id)
            .map(|(session_id, _)| Some(session_id.clone()))
        else {
            continue;
        };
        let (frame_id, page_url, network_events) = {
            let Some(page) = ctx.get_page_mut(&page_id) else {
                continue;
            };
            page.sync_js_network_events();
            (
                page.frame_id.clone(),
                page.url_string(),
                page.network_events.drain(..).collect::<Vec<_>>(),
            )
        };
        if network_events.is_empty() {
            continue;
        }
        crate::domains::page::emit_runtime_network_events(
            ctx,
            &session_id,
            &frame_id,
            &page_url,
            &page_id,
            &network_events,
        );
    }
}

fn take_live_pending_navigation(
    ctx: &CdpContext,
) -> Option<(String, String, String, String)> {
    // With several live pages, a pending navigation may belong to any of them,
    // not just the first live page — scan until one yields a navigation (#872).
    for page in ctx.pages.iter().filter(|page| page.has_js()) {
        let Some(session_id) = ctx
            .sessions
            .iter()
            .find(|(_, page_id)| *page_id == &page.id)
            .map(|(session_id, _)| session_id.clone())
        else {
            continue;
        };
        if let Some((url, method, body)) = page.take_pending_navigation() {
            return Some((session_id, url, method, body));
        }
    }
    None
}

fn forward_pending_events(
    ctx: &mut CdpContext,
    reply_tx: Option<&OutboundSender>,
) {
    let Some(reply_tx) = reply_tx else {
        return;
    };
    for event in ctx.pending_events.drain(..) {
        if let Ok(json) = serde_json::to_string(&event) {
            let _ = reply_tx.send(json);
        }
    }
}

fn has_active_screencast(ctx: &CdpContext) -> bool {
    #[cfg(feature = "render")]
    {
        !ctx.screencasts.is_empty()
    }
    #[cfg(not(feature = "render"))]
    {
        let _ = ctx;
        false
    }
}

async fn pump_and_forward_screencast_frames(
    ctx: &mut CdpContext,
    reply_tx: Option<&OutboundSender>,
) {
    #[cfg(feature = "render")]
    crate::domains::page::pump_screencast_frames(ctx).await;
    #[cfg(not(feature = "render"))]
    let _ = ctx;

    forward_pending_events(ctx, reply_tx);
}

// Whether a raw CDP frame is exactly a `Page.navigate` call, and so should take
// the spawn-and-defer navigation path. Matching on the parsed method rather than
// a `contains("Page.navigate")` substring avoids catching
// `Page.navigateToHistoryEntry` (goBack / goForward), which has no `url` param
// and belongs to its own handler, or any other frame that merely embeds the
// literal text (e.g. a `Runtime.evaluate` expression). See issue #363.
fn is_navigate_method(text: &str) -> bool {
    serde_json::from_str::<CdpRequest>(text)
        .map(|req| req.method == "Page.navigate")
        .unwrap_or(false)
}

fn is_navigation_safe_body_command(text: &str) -> bool {
    serde_json::from_str::<CdpRequest>(text).is_ok_and(|request| matches!(request.method.as_str(),
        "Fetch.getResponseBody" | "Fetch.takeResponseBodyAsStream" | "Network.getResponseBody" | "IO.read" | "IO.close"))
}

// Keep CDP field order, spelling and full values until the HTTP boundary.
// Validate before taking the resolver so malformed input remains retryable.
pub(crate) fn parse_cdp_headers(params: &serde_json::Value) -> Result<Option<Vec<(String, String)>>, String> {
    params.get("headers").map(|headers| {
        headers.as_array().ok_or("headers must be an array")?.iter().map(|header| {
            let name = header.get("name").and_then(|v| v.as_str()).ok_or("headers require a string name")?;
            let value = header.get("value").and_then(|v| v.as_str()).ok_or("headers require a string value")?;
            http::header::HeaderName::from_bytes(name.as_bytes()).map_err(|_| "invalid header name")?;
            http::header::HeaderValue::from_bytes(value.as_bytes()).map_err(|_| "invalid header value")?;
            Ok((name.to_string(), value.to_string()))
        }).collect()
    }).transpose()
}

pub(crate) fn parse_continue_resolution(params: &serde_json::Value) -> Result<obscura_js::ops::InterceptResolution, String> {
    let object = params.as_object().ok_or("continueRequest params must be an object")?;
    if let Some(name) = object.keys().find(|name| !matches!(name.as_str(),
        "requestId" | "url" | "method" | "postData" | "headers" | "interceptResponse"))
    {
        return Err(format!("unsupported continueRequest parameter: {name}"));
    }
    if object.contains_key("interceptResponse") {
        return Err("continueRequest interceptResponse is not supported".into());
    }
    let body = parse_continue_post_data(params)?;
    let headers = parse_cdp_headers(params)?;
    let url = params.get("url").map(|value| value.as_str()
        .ok_or("url must be a string").map(str::to_string)).transpose()?;
    let method = params.get("method").map(|value| value.as_str()
        .ok_or("method must be a string").map(str::to_string)).transpose()?;
    Ok(match headers {
        Some(headers) => obscura_js::ops::InterceptResolution::ContinueWithHeaders { url, method, headers, body },
        None => obscura_js::ops::InterceptResolution::Continue { url, method, headers: None, body },
    })
}

// CDP binary fields use standard base64. Validate before taking the resolver
// so callers can correct malformed input and retry the same paused request.
pub(crate) fn parse_continue_post_data(params: &serde_json::Value) -> Result<Option<Vec<u8>>, String> {
    use base64::Engine as _;
    params.get("postData").map(|value| {
        let encoded = value.as_str().ok_or("postData must be a base64 string")?;
        base64::engine::general_purpose::STANDARD.decode(encoded)
            .map_err(|_| "postData must be valid base64".to_string())
    }).transpose()
}

fn parse_fulfill_headers(params: &serde_json::Value) -> Result<obscura_net::HeaderCapture, String> {
    use base64::Engine as _;
    let mut fields = Vec::new();
    if let Some(binary) = params.get("binaryResponseHeaders") {
        if params.get("responseHeaders").is_some() {
            return Err("supply responseHeaders or binaryResponseHeaders, not both".into());
        }
        let encoded = binary.as_str().ok_or("binaryResponseHeaders must be a base64 string")?;
        let bytes = base64::engine::general_purpose::STANDARD.decode(encoded)
            .map_err(|_| "binaryResponseHeaders must be valid base64")?;
        for field in bytes.split(|byte| *byte == 0).filter(|field| !field.is_empty()) {
            let colon = field.iter().position(|byte| *byte == b':')
                .filter(|index| *index > 0).ok_or("binaryResponseHeaders fields require name: value")?;
            let value = &field[colon + 1..];
            fields.push(obscura_net::RawHeader {
                name: field[..colon].to_vec(),
                value: value.strip_prefix(b" ").unwrap_or(value).to_vec(),
            });
        }
    } else if let Some(headers) = params.get("responseHeaders") {
        for header in headers.as_array().ok_or("responseHeaders must be an array")? {
            let name = header.get("name").and_then(|v| v.as_str()).ok_or("responseHeaders require a string name")?;
            let value = header.get("value").and_then(|v| v.as_str()).ok_or("responseHeaders require a string value")?;
            fields.push(obscura_net::RawHeader { name: name.as_bytes().to_vec(), value: value.as_bytes().to_vec() });
        }
    }
    Ok(obscura_net::HeaderCapture { capture_stage: "cdpFulfillResponse", encoding: "base64", fields })
}

pub(crate) fn parse_fulfill_resolution(params: &serde_json::Value) -> Result<obscura_js::ops::InterceptResolution, String> {
    use base64::Engine as _;
    let raw_headers = parse_fulfill_headers(params)?;
    let body_supplied = params.get("body").is_some();
    let body_base64 = match params.get("body") {
        Some(body) => body.as_str().ok_or("body must be a base64 string")?,
        None => "",
    };
    let bytes = base64::engine::general_purpose::STANDARD.decode(body_base64)
        .map_err(|_| "body must be valid base64")?;
    let body = String::from_utf8_lossy(&bytes).into_owned();
    let status_text = params.get("responsePhrase").map(|value| value.as_str()
        .ok_or("responsePhrase must be a string").map(str::to_string)).transpose()?;
    let mut text_projection = raw_headers.clone();
    for field in &mut text_projection.fields { field.name.make_ascii_lowercase(); }
    Ok(obscura_js::ops::InterceptResolution::FulfillWithHeaders {
        status: parse_response_code(params.get("responseCode").ok_or("fulfillRequest requires responseCode")?)?, status_text,
        headers: text_projection.text_headers(), raw_headers, body, body_base64: body_base64.to_string(), body_supplied,
    })
}

pub(crate) fn parse_error_reason(params: &serde_json::Value) -> Result<String, String> {
    let reason = params.get("errorReason").and_then(|value| value.as_str())
        .ok_or("failRequest requires a string errorReason")?;
    match reason {
        "Failed" | "Aborted" | "TimedOut" | "AccessDenied" | "ConnectionClosed"
        | "ConnectionReset" | "ConnectionRefused" | "ConnectionAborted" | "ConnectionFailed"
        | "NameNotResolved" | "InternetDisconnected" | "AddressUnreachable"
        | "BlockedByClient" | "BlockedByResponse" => Ok(reason.to_string()),
        _ => Err(format!("unsupported failRequest errorReason: {reason}")),
    }
}

fn parse_response_code(value: &serde_json::Value) -> Result<u16, String> {
    let value = value.as_u64().ok_or("responseCode must be an integer")?;
    let status = u16::try_from(value).map_err(|_| "responseCode is out of range")?;
    if !(100..=599).contains(&status) { return Err("responseCode must be between 100 and 599".into()); }
    Ok(status)
}

fn parse_continue_response_resolution(params: &serde_json::Value) -> Result<obscura_js::ops::InterceptResolution, String> {
    let object = params.as_object().ok_or("continueResponse params must be an object")?;
    if let Some(name) = object.keys().find(|name| !matches!(name.as_str(),
        "requestId" | "responseCode" | "responsePhrase" | "responseHeaders" | "binaryResponseHeaders"))
    {
        return Err(format!("unsupported continueResponse parameter: {name}"));
    }
    let status = params.get("responseCode").map(parse_response_code).transpose()?;
    let has_headers = params.get("responseHeaders").is_some() || params.get("binaryResponseHeaders").is_some();
    let phrase = params.get("responsePhrase").map(|value| value.as_str().ok_or("responsePhrase must be a string")).transpose()?;
    let overrides = status.is_some() || phrase.is_some() || has_headers;
    if overrides && (status.is_none() || !has_headers) {
        return Err("responseCode and responseHeaders or binaryResponseHeaders must be supplied together; responsePhrase is optional".into());
    }
    let (headers, raw_headers) = if has_headers {
        let raw_headers = parse_fulfill_headers(params)?;
        let mut text_projection = raw_headers.clone();
        for field in &mut text_projection.fields { field.name.make_ascii_lowercase(); }
        (Some(text_projection.text_headers()), Some(raw_headers))
    } else { (None, None) };
    Ok(obscura_js::ops::InterceptResolution::ContinueResponse {
        status, status_text: phrase.map(str::to_string), headers, raw_headers,
    })
}

fn handle_fetch_resolution(
    text: &str,
    ctx: &mut CdpContext,
    reply_tx: &OutboundSender,
    intercepted_paused: &mut InterceptedPauses,
) -> bool {
    // A queued resolution may wake at the same instant that its websocket
    // writer fails. Treat it as handled without mutating pause state; the
    // processor's connection teardown will abort the resolver.
    if reply_tx.is_closed() {
        return true;
    }
    intercepted_paused.retain(|_, pause| !pause.resolver.is_closed());
    if let Ok(req) = serde_json::from_str::<CdpRequest>(text) {
        let method = req.method.as_str();
        if method == "Fetch.disable" {
            let allowed = match &req.session_id {
                Some(sid) => ctx.sessions.get(sid).is_some_and(|pid|
                    ctx.fetch_intercept.owners.get(pid).is_none_or(|owner| owner == &req.session_id)),
                None => ctx.page_count() == 1,
            };
            if allowed {
                let page_id = req.session_id.as_ref().and_then(|sid| ctx.sessions.get(sid))
                    .map(String::as_str).or_else(|| ctx.single_page_id()).map(str::to_string);
                let owner = page_id.as_ref().and_then(|pid| ctx.fetch_intercept.owners.remove(pid));
                if let Some(page_id) = &page_id {
                    if ctx.get_page(page_id).is_none() && ctx.navigating_page_id.as_ref() == Some(page_id) {
                        ctx.pending_fetch_policy_cleanup.insert(page_id.clone());
                    }
                }
                let keys: Vec<_> = intercepted_paused.keys()
                    .filter(|(sid, _)| owner.as_ref() == Some(sid)).cloned().collect();
                for key in keys {
                    if let Some(pause) = intercepted_paused.remove(&key) {
                        let body_taken = crate::domains::network::response_body_owner(ctx, &key.0, &key.1)
                            .err().is_some_and(|error| error.contains("response_body_already_consumed")
                                || error.contains("response_body_access_conflict"));
                        let resolution = if body_taken {
                            obscura_js::ops::InterceptResolution::Fail { reason: "Aborted".into() }
                        } else if pause.stage == obscura_js::ops::InterceptionStage::Response {
                            obscura_js::ops::InterceptResolution::ContinueResponse { status: None, status_text: None, headers: None, raw_headers: None }
                        } else { obscura_js::ops::InterceptResolution::Continue {
                            url: None, method: None, headers: None, body: None,
                        }};
                        let _ = pause.resolver.send(resolution);
                    }
                }
            }
            if !allowed {
                let response = CdpResponse::error(req.id, -32000,
                    "Fetch.disable requires the owning sessionId".into(), req.session_id);
                if let Ok(json) = serde_json::to_string(&response) { let _ = reply_tx.send(json); }
                return true;
            }
            // The normal handler updates the owning Page's interception policy.
            return false;
        }
        let request_id = req.params.get("requestId").and_then(|v| v.as_str()).unwrap_or("");
        if method == "Network.getResponseBody" && !request_id.starts_with("intercept-") { return false; }
        tracing::info!("INTERCEPTION resolution: {} for {}, paused_count={}", method, request_id, intercepted_paused.len());

        if !matches!(method, "Fetch.continueRequest" | "Fetch.continueResponse" | "Fetch.fulfillRequest" | "Fetch.failRequest"
            | "Fetch.getResponseBody" | "Fetch.takeResponseBodyAsStream" | "Network.getResponseBody") { return false; }
        let mut key = (req.session_id.clone(), request_id.to_string());
        let routing_error = if req.session_id.is_none() && ctx.page_count() > 1 {
            Some("Fetch request requires a sessionId when multiple Pages exist".to_string())
        } else if req.session_id.as_ref().is_some_and(|sid| !ctx.sessions.contains_key(sid)) {
            Some("Unknown Fetch sessionId".to_string())
        } else {
            if req.session_id.is_none() && !intercepted_paused.contains_key(&key) {
                let matches: Vec<_> = intercepted_paused.keys().filter(|(_, id)| id == request_id).cloned().collect();
                if matches.len() == 1 { key = matches[0].clone(); }
            }
            if !intercepted_paused.contains_key(&key)
                && intercepted_paused.keys().any(|(_, id)| id == request_id)
                // A completed alias on this Page must remain readable even
                // while another Page is paused on the same local ID.
                && !(matches!(method, "Fetch.getResponseBody" | "Fetch.takeResponseBodyAsStream" | "Network.getResponseBody")
                    && ctx.get_session_page(&req.session_id).is_some_and(|page| page.has_response_body(request_id)))
            {
                Some("Fetch requestId does not belong to this sessionId".to_string())
            } else { None }
        };
        if let Some(message) = routing_error {
            let response = CdpResponse::error(req.id, -32000, message, req.session_id);
            if let Ok(json) = serde_json::to_string(&response) { let _ = reply_tx.send(json); }
            return true;
        }

        if matches!(method, "Fetch.getResponseBody" | "Fetch.takeResponseBodyAsStream" | "Network.getResponseBody")
            && intercepted_paused.get(&key).is_some_and(|pause| pause.stage == obscura_js::ops::InterceptionStage::Request)
        {
            let response = crate::types::CdpResponse::error(
                req.id, -32000, crate::domains::fetch::response_body_not_ready(request_id), req.session_id,
            );
            if let Ok(json) = serde_json::to_string(&response) {
                let _ = reply_tx.send(json);
            }
            return true;
        }
        if matches!(method, "Fetch.getResponseBody" | "Fetch.takeResponseBodyAsStream")
            && intercepted_paused.get(&key).is_some_and(|pause| pause.redirect_response)
        {
            let response = crate::types::CdpResponse::error(
                req.id, -32000, "response body is unavailable for a redirect response".into(), req.session_id,
            );
            if let Ok(json) = serde_json::to_string(&response) { let _ = reply_tx.send(json); }
            return true;
        }
        if !matches!(method, "Fetch.continueRequest" | "Fetch.continueResponse" | "Fetch.fulfillRequest" | "Fetch.failRequest") {
            return false;
        }

        if request_id.starts_with("intercept-") && !intercepted_paused.contains_key(&key) {
            let response = CdpResponse::error(req.id, -32000,
                "Fetch requestId is not paused in this session".into(), req.session_id);
            if let Ok(json) = serde_json::to_string(&response) { let _ = reply_tx.send(json); }
            return true;
        }

        if matches!(method, "Fetch.continueRequest" | "Fetch.continueResponse")
            && intercepted_paused.get(&key).is_some_and(|pause| pause.stage == obscura_js::ops::InterceptionStage::Response)
        {
            if crate::domains::network::response_body_owner(ctx, &req.session_id, request_id)
                .err().is_some_and(|error| error.contains("response_body_already_consumed")
                    || error.contains("response_body_access_conflict"))
            {
                let response = CdpResponse::error(req.id, -32000,
                    "response body was taken as a stream; only failRequest or fulfillRequest may resolve this pause".into(), req.session_id);
                if let Ok(json) = serde_json::to_string(&response) { let _ = reply_tx.send(json); }
                return true;
            }
        }

        // Validate fields before consuming the pause so malformed input can
        // be corrected without stranding the in-flight fetch.
        let parsed_resolution = if intercepted_paused.contains_key(&key) {
            let stage = intercepted_paused[&key].stage;
            if method == "Fetch.continueResponse" && stage == obscura_js::ops::InterceptionStage::Request {
                let expected = if stage == obscura_js::ops::InterceptionStage::Response { "Fetch.continueResponse" } else { "Fetch.continueRequest" };
                let response = CdpResponse::error(req.id, -32000,
                    format!("{expected} is required for this Fetch pause stage"), req.session_id);
                if let Ok(json) = serde_json::to_string(&response) { let _ = reply_tx.send(json); }
                return true;
            }
            let result = match method {
                "Fetch.continueRequest" => {
                    parse_continue_resolution(&req.params).and_then(|resolution| {
                        if stage == obscura_js::ops::InterceptionStage::Response && !matches!(&resolution,
                            obscura_js::ops::InterceptResolution::Continue { url: None, method: None, headers: None, body: None })
                        {
                            Err("response-stage continueRequest does not accept request overrides".into())
                        } else { Ok(resolution) }
                    })
                },
                "Fetch.continueResponse" => parse_continue_response_resolution(&req.params),
                "Fetch.fulfillRequest" => parse_fulfill_resolution(&req.params),
                _ => parse_error_reason(&req.params)
                    .map(|reason| obscura_js::ops::InterceptResolution::Fail { reason }),
            };
            match result {
                Ok(resolution) => Some(resolution),
                Err(message) => {
                    let response = CdpResponse::error(req.id, -32602, message, req.session_id);
                    if let Ok(json) = serde_json::to_string(&response) { let _ = reply_tx.send(json); }
                    return true;
                }
            }
        } else { None };
        if let Some(pause) = intercepted_paused.remove(&key) {
            tracing::info!("INTERCEPTION resolved: {}", request_id);
            let resp = if pause.resolver.send(parsed_resolution.expect("validated fetch resolution")).is_ok() {
                crate::types::CdpResponse::success(req.id, json!({}), req.session_id)
            } else {
                crate::types::CdpResponse::error(req.id, -32000, "requestId is no longer paused".into(), req.session_id)
            };
            if let Ok(json) = serde_json::to_string(&resp) {
                let _ = reply_tx.send(json);
            }
            return true;
        }
    }
    false
}

async fn process_with_interception(
    text: &str,
    ctx: &mut CdpContext,
    reply_tx: &OutboundSender,
    rx: &mut mpsc::UnboundedReceiver<ServerMessage>,
    intercept_rx: &mut Option<mpsc::UnboundedReceiver<crate::domains::fetch::RoutedInterceptedRequest>>,
    intercepted_paused: &mut InterceptedPauses,
    deferred: &mut std::collections::VecDeque<ServerMessage>,
    send_command_response: bool,
) {
    if reply_tx.is_closed() {
        return;
    }
    let req: CdpRequest = match serde_json::from_str(text) {
        Ok(r) => r,
        Err(e) => {
            warn!("Invalid CDP: {}", e);
            return;
        }
    };

    tracing::info!("INTERCEPTION navigate: {} (id={})", req.method, req.id);

    let session_id = &req.session_id;
    let page_id = session_id
        .as_ref()
        .and_then(|sid| ctx.sessions.get(sid))
        .cloned();

    let page_id = match page_id {
        Some(id) => id,
        None => {
            process_cdp_message(text, ctx, reply_tx).await;
            return;
        }
    };

    let page_index = ctx.pages.iter().position(|p| p.id == page_id);
    let mut page = match page_index {
        Some(idx) => ctx.pages.remove(idx),
        None => {
            process_cdp_message(text, ctx, reply_tx).await;
            return;
        }
    };

    ctx.navigating_page_id = Some(page_id.clone());
    ctx.navigating_response_bodies = Some((page_id.clone(), page.response_body_store()));

    // V8 allows only ONE *entered* isolate per OS thread, but many *live*
    // ones. Since #756 every op enters its isolate only transiently (never
    // across an `.await`) and construction leaves the entry stack empty, so a
    // nav task's `init_js` can build a new isolate while other pages' isolates
    // are live without tripping `Context::Exit`'s
    // `heap->isolate() == Isolate::TryGetCurrent()` check. The old defensive
    // `suspend_js` of every other page here (which tore their heaps down and
    // was never resumed once the dispatch-path resume was removed in #872) is
    // therefore no longer needed and would strand concurrent pages.

    let url = req.params.get("url").and_then(|v| v.as_str()).unwrap_or("");
    let wait_until = crate::domains::page::parse_wait_until(&req.params);
    let nav_method = req.params.get("__method").and_then(|v| v.as_str()).unwrap_or("GET").to_string();
    let nav_body = req.params.get("__body").and_then(|v| v.as_str()).unwrap_or("").to_string();

    let preload_scripts: Vec<String> = ctx.preload_scripts.iter().map(|(_, s)| s.clone()).collect();

    let session_for_events = req.session_id.clone();
    let frame_id = page.frame_id.clone();
    let loader_id = format!("loader-{}", uuid::Uuid::new_v4());
    // The new runtime can pause before the navigating Page returns to ctx.
    // Retain the old generation mapping and pre-register the expected new one.
    ctx.navigating_document_loader = Some((page.network_document_generation, loader_id.clone()));
    ctx.document_loaders.insert((page_id.clone(), page.network_document_generation + 1), loader_id.clone());

    let (nav_done_tx, mut nav_done_rx) = mpsc::channel::<(obscura_browser::Page, Result<(), String>)>(1);
    let url_owned = url.to_string();
    let nav_v8_lock = ctx.v8_lock.clone();

    tokio::task::spawn_local(async move {
        // Issue #19: serialize this connection's V8 work across its pages. This
        // nav task runs while the connection's processor keeps pumping other CDP
        // messages via `dispatch` (which takes the same per-connection lock), so
        // both sides coordinate on one page's isolate at a time on this thread.
        // The lock is per-connection, so other connections are unaffected (#430).
        let _v8_guard = nav_v8_lock.lock_owned().await;
        // Preloads (addBinding shims, addScriptToEvaluateOnNewDocument sources)
        // must run BEFORE the page's own scripts (CDP contract). Hand them
        // to the page so navigate_single can inject them at the right point.
        page.set_preload_scripts(preload_scripts);
        let result = if nav_method == "POST" && !nav_body.is_empty() {
            page.navigate_with_wait_post(&url_owned, wait_until, &nav_method, &nav_body).await
        } else {
            page.navigate_with_wait(&url_owned, wait_until).await
        }
        .map_err(|e| e.to_string());
        drop(_v8_guard);
        let _ = nav_done_tx.send((page, result)).await;
    });

    let navigate_result: Result<(), String>;
    let page_back: Option<obscura_browser::Page>;

    // Issue #19 follow-up (PR #36 maintainer's fetch-intercept repro):
    // While the spawned nav task is executing V8 (potentially parked on
    // `op_fetch_url`'s `resolve_rx.await` *with Isolate-N still entered*),
    // we must NOT let the parent's `select!` route foreign Cdp messages
    // through `process_cdp_message → dispatch → page handlers`, because
    // those handlers call `get_session_page_mut` which `suspend_js`'es
    // OTHER pages (drops their `JsRuntime`, which calls
    // `JsRealmInner::destroy`). That trips V8's
    // `heap->isolate() == Isolate::TryGetCurrent()` invariant and aborts
    // the process via `V8_Fatal`.
    //
    // This connection's `ctx.v8_lock` doesn't save us here: it's a
    // `tokio::sync::Mutex` that is released around `.await`s inside V8
    // ops, so it doesn't actually keep the V8 enter/exit pair contiguous
    // on the thread.
    //
    // Park foreign Cdp messages into the outer deferred queue so the
    // outer `cdp_processor` loop processes them after this nav fully
    // completes (and its JsRuntime is no longer in flight on the
    // LocalSet).
    let mut connection_open = true;
    loop {
        if connection_open && reply_tx.is_closed() {
            connection_open = false;
            for (_, pause) in intercepted_paused.drain() {
                let _ = pause.resolver.send(
                    obscura_js::ops::InterceptResolution::Fail {
                        reason: "Aborted".into(),
                    },
                );
            }
        }
        let has_irx = intercept_rx.is_some();

        tokio::select! {
            Some((returned_page, result)) = nav_done_rx.recv() => {
                page_back = Some(returned_page);
                navigate_result = result;
                break;
            }
            Some(intercepted) = async {
                if let Some(ref mut irx) = intercept_rx {
                    irx.recv().await
                } else {
                    std::future::pending().await
                }
            }, if has_irx => {
                if connection_open {
                    emit_routed_intercepted_request(intercepted, ctx, reply_tx, intercepted_paused);
                } else {
                    let _ = intercepted.request.resolver.send(obscura_js::ops::InterceptResolution::Fail { reason: "Aborted".into() });
                }
                tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
            }
            msg = rx.recv(), if connection_open => {
                let Some(msg) = msg else {
                    connection_open = false;
                    for (_, pause) in intercepted_paused.drain() {
                        let _ = pause.resolver.send(obscura_js::ops::InterceptResolution::Fail { reason: "Aborted".into() });
                    }
                    continue;
                };
                if reply_tx.is_closed() {
                    connection_open = false;
                    for (_, pause) in intercepted_paused.drain() {
                        let _ = pause.resolver.send(
                            obscura_js::ops::InterceptResolution::Fail {
                                reason: "Aborted".into(),
                            },
                        );
                    }
                    continue;
                }
                tracing::info!("INTERCEPTION select: received CDP message during navigation");
                match msg {
                    ServerMessage::NewConnection { reply_tx: new_tx } => {
                        // Safe: no V8 enter, just bookkeeping.
                        let pid = ctx.create_page();
                        let sid = format!("{}-session", pid);
                        ctx.sessions.insert(sid.clone(), pid.clone());
                        let _ = new_tx.send(json!({"__init": true, "pageId": pid, "sessionId": sid}).to_string());
                    }
                    ServerMessage::Cdp(msg) => {
                        let lifecycle_released = abort_navigating_fetch_owner_for_lifecycle(
                            &msg.text, ctx, intercepted_paused,
                        );
                        if (msg.text.contains("Fetch.") || msg.text.contains("Network.getResponseBody")) && handle_fetch_resolution(
                            &msg.text, ctx, &msg.reply_tx, intercepted_paused,
                        ) {
                            // Safe: resolves the pause or rejects a premature
                            // body read without entering V8.
                        } else if is_navigation_safe_body_command(&msg.text) {
                            process_cdp_message(&msg.text, ctx, &msg.reply_tx).await;
                        } else {
                            // UNSAFE during nav: would route through dispatch,
                            // which can `suspend_js` other pages and trip the
                            // V8 invariant. Defer until nav completes —
                            // pushed to the outer `cdp_processor` queue so
                            // it's processed sequentially with no nav task
                            // in flight.
                            if deferred.len() >= MAX_DEFERRED_MESSAGES && !lifecycle_released {
                                tracing::warn!("INTERCEPTION: deferred queue full ({}), returning error to client", MAX_DEFERRED_MESSAGES);
                                if let Ok(req) = serde_json::from_str::<CdpRequest>(&msg.text) {
                                    let resp = crate::types::CdpResponse::error(
                                        req.id,
                                        -32000,
                                        "Server busy: navigation in progress, try again later".to_string(),
                                        req.session_id,
                                    );
                                    if let Ok(json) = serde_json::to_string(&resp) {
                                        let _ = msg.reply_tx.send(json);
                                    }
                                }
                            } else {
                                tracing::info!("INTERCEPTION: deferring CDP message until nav completes");
                                deferred.push_back(ServerMessage::Cdp(msg));
                            }
                        }
                    }
                }
            }
        }
    }

    // Deferred messages are handled by the outer `cdp_processor` loop
    // (it drains `deferred` before pulling the next message off `rx`).

    let mut page = page_back.expect("navigation task should return the page");
    if ctx.pending_fetch_policy_cleanup.remove(&page.id) {
        page.intercept_block_patterns.clear();
        page.intercept_request_patterns.clear();
        page.set_intercept_response_patterns(Vec::new());
        page.enable_intercept(false);
    }

    // Fold in network events for script-initiated requests (fetch/XHR/dynamic
    // resource) so they emit as Network.requestWillBeSent / responseReceived
    // alongside the static navigation subresources (#406).
    page.sync_js_network_events();
    let network_events: Vec<_> = page.network_events.drain(..).collect();
    let page_url = page.url_string();
    let page_id_for_events = page.id.clone();
    let reached_network_idle = page.lifecycle.is_network_idle();

    ctx.pages.push(page);
    ctx.navigating_page_id = None;
    ctx.navigating_response_bodies = None;
    ctx.navigating_document_loader = None;

    let navigation_succeeded = navigate_result.is_ok();
    let response = match navigate_result {
        Ok(()) => crate::types::CdpResponse::success(
            req.id,
            json!({"frameId": frame_id, "loaderId": loader_id}),
            req.session_id.clone(),
        ),
        Err(e) => crate::types::CdpResponse::error(req.id, -32000, e, req.session_id.clone()),
    };

    if send_command_response {
        if let Ok(json) = serde_json::to_string(&response) {
            let _ = reply_tx.send(json);
        }
    }

    // Shared event emission: includes the post-#190 Network.requestWillBeSent
    // -before-frameNavigated ordering, the #189 requestId=loaderId trick that
    // makes `page.goto()` resolve to a Response, and the #192 per-isolated-
    // world fresh context ids. Pushes to `ctx.pending_events`; we then drain
    // to the WS reply channel.
    if navigation_succeeded {
    crate::domains::page::emit_navigation_events(
        ctx,
        &session_for_events,
        &frame_id,
        &loader_id,
        &page_url,
        &page_id_for_events,
        &network_events,
        wait_until,
        reached_network_idle,
    );
    } else {
        crate::domains::page::emit_runtime_network_events(ctx, &session_for_events,
            &frame_id, &page_url, &page_id_for_events, &network_events);
    }
    #[cfg(feature = "render")]
    if navigation_succeeded {
        if let Err(error) = crate::domains::page::queue_screencast_frame(
            ctx, &session_for_events, false,
        ) {
            tracing::warn!("could not produce post-navigation screencast frame: {error}");
        }
    }
    for event in ctx.pending_events.drain(..) {
        if let Ok(json) = serde_json::to_string(&event) {
            let _ = reply_tx.send(json);
        }
    }
}

async fn process_cdp_message(
    text: &str,
    ctx: &mut CdpContext,
    reply_tx: &OutboundSender,
) {
    if reply_tx.is_closed() {
        return;
    }
    let req: CdpRequest = match serde_json::from_str(text) {
        Ok(r) => r,
        Err(e) => {
            warn!("Invalid CDP: {}: {}", e, crate::util::truncate_on_char_boundary(text, 200));
            return;
        }
    };

    tracing::debug!("CDP: {} (id={}, s={:?})", req.method, req.id, req.session_id);

    service_live_page_render_resources(ctx);
    let response = dispatch::dispatch(&req, ctx).await;
    service_live_page_render_resources(ctx);

    // Chromium CDP semantics: events emitted as a side-effect of a command
    // (e.g. Target.targetCreated + Target.attachedToTarget from
    // Target.createTarget) MUST arrive BEFORE the command's response.
    // Playwright awaits the response and immediately reads state wired up
    // by those events; if the response lands first, accessing
    // Target._page errors with "Cannot read properties of undefined".
    for event in ctx.pending_events.drain(..) {
        if let Ok(json) = serde_json::to_string(&event) {
            let _ = reply_tx.send(json);
        }
    }

    if let Ok(json) = serde_json::to_string(&response) {
        let _ = reply_tx.send(json);
    }

    if reply_tx.is_closed() {
        return;
    }

    if let Some((nav_url, nav_method, nav_body)) = check_pending_navigation(ctx, &req.session_id) {
        tracing::info!("JS-triggered nav: {} {} (body: {} bytes)", nav_method, nav_url, nav_body.len());
        let nav_req = CdpRequest {
            id: 0,
            method: "Page.navigate".to_string(),
            params: json!({"url": nav_url, "__method": nav_method, "__body": nav_body}),
            session_id: req.session_id.clone(),
        };
        let _ = dispatch::dispatch(&nav_req, ctx).await;
        for event in ctx.pending_events.drain(..) {
            if let Ok(json) = serde_json::to_string(&event) {
                let _ = reply_tx.send(json);
            }
        }
    }
}

fn check_pending_navigation(ctx: &CdpContext, session_id: &Option<String>) -> Option<(String, String, String)> {
    let page_id = session_id
        .as_ref()
        .and_then(|sid| ctx.sessions.get(sid))?;
    let page = ctx.pages.iter().find(|p| &p.id == page_id)?;
    page.take_pending_navigation()
}

async fn handle_connection_ws(
    stream: TcpStream,
    msg_tx: mpsc::UnboundedSender<ServerMessage>,
) -> anyhow::Result<()> {
    // tokio_tungstenite wraps the stream in a 128 KiB write BufWriter by
    // default. CDP traffic is many small (~100-byte) frames, and that buffer
    // adds extra latency per frame. write_buffer_size=0 makes every WS write
    // hit the socket directly. Combined with set_nodelay(true) above, gets
    // per-frame latency on localhost down toward ideal.
    use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
    let mut cfg = WebSocketConfig::default();
    cfg.write_buffer_size = 0;
    cfg.max_write_buffer_size = crate::outbound::DEFAULT_MAX_BYTES;
    let ws_stream = tokio_tungstenite::accept_async_with_config(stream, Some(cfg)).await?;
    info!("WebSocket connected");
    let (mut ws_sender, mut ws_receiver) = ws_stream.split();

    let (reply_tx, mut reply_rx, mut outbound_closed) = crate::outbound::channel();

    if msg_tx.send(ServerMessage::NewConnection {
        reply_tx: reply_tx.clone(),
    }).is_err() {
        reply_tx.close(OutboundCloseReason::ConnectionClosed);
        return Err(anyhow::anyhow!("CDP processor closed before connection init"));
    }
    if let Some(init_msg) = reply_rx.recv().await {
        let init_msg = init_msg.as_str();
        tracing::debug!("Connection init: {}", &init_msg[..init_msg.len().min(100)]);
    } else {
        return Err(anyhow::anyhow!("CDP processor did not initialize connection"));
    }

    let writer_reply_tx = reply_tx.clone();
    let mut send_task = tokio::task::spawn_local(async move {
        while let Some(envelope) = reply_rx.recv().await {
            if envelope.as_str().contains("\"__init\"") {
                continue;
            }
            let (message, reservation) = envelope.into_parts();
            let result = tokio::time::timeout(
                tokio::time::Duration::from_millis(OUTBOUND_SEND_TIMEOUT_MS),
                ws_sender.send(Message::Text(message.into())),
            )
            .await;
            drop(reservation);
            match result {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    warn!("WS write error: {error}");
                    writer_reply_tx.close(OutboundCloseReason::WriterIo);
                    break;
                }
                Err(_) => {
                    warn!("WS write timed out after {OUTBOUND_SEND_TIMEOUT_MS}ms");
                    writer_reply_tx.close(OutboundCloseReason::WriterTimeout);
                    break;
                }
            }
        }
    });

    let mut writer_joined = false;
    loop {
        let next = tokio::select! {
            changed = outbound_closed.changed() => {
                if changed.is_err() || *outbound_closed.borrow() {
                    warn!("closing CDP connection after outbound failure: {:?}", reply_tx.close_reason());
                    break;
                }
                continue;
            }
            writer = &mut send_task => {
                writer_joined = true;
                if let Err(error) = writer {
                    warn!("CDP writer task failed: {error}");
                }
                reply_tx.close(OutboundCloseReason::WriterIo);
                break;
            }
            message = ws_receiver.next() => message,
        };
        let Some(msg) = next else { break; };
        let msg = match msg {
            Ok(m) => m,
            Err(e) => {
                warn!("WS read error: {}", e);
                break;
            }
        };

        match msg {
            Message::Text(text) => {
                if let Ok(req) = serde_json::from_str::<CdpRequest>(&text) {
                    if let Some((resp, close_connection)) = browser_close_response(&req) {
                        if let Ok(json) = serde_json::to_string(&resp) {
                            let _ = reply_tx.send(json);
                        }
                        if close_connection {
                            // Seal admission before flushing so processor/event
                            // tasks cannot extend the drain indefinitely. The
                            // normal writer releases a reservation after the
                            // websocket send completes. Failure/cancellation
                            // also releases it and closes the connection, so
                            // this is a bounded flush attempt rather than an
                            // independent delivery acknowledgement.
                            reply_tx.close(OutboundCloseReason::ConnectionClosed);
                            if tokio::time::timeout(
                                tokio::time::Duration::from_millis(OUTBOUND_SEND_TIMEOUT_MS),
                                reply_tx.wait_empty(),
                            )
                            .await
                            .is_err()
                            {
                                warn!("Browser.close response flush timed out after {OUTBOUND_SEND_TIMEOUT_MS}ms");
                            }
                            break;
                        }
                        continue;
                    }
                }

                if msg_tx.send(ServerMessage::Cdp(CdpMessage {
                    text: text.to_string(),
                    reply_tx: reply_tx.clone(),
                })).is_err() {
                    reply_tx.close(OutboundCloseReason::ConnectionClosed);
                    break;
                }
            }
            Message::Close(_) => {
                info!("WS closed by client");
                break;
            }
            _ => {}
        }
    }

    reply_tx.close(OutboundCloseReason::ConnectionClosed);
    if !writer_joined {
        if !send_task.is_finished() {
            send_task.abort();
        }
        let _ = send_task.await;
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::{
        browser_close_response, handle_fetch_resolution, is_navigate_method,
        parse_cdp_headers, raw_header_bytes_to_cdp_string, websocket_authority, InterceptedPause,
    };
    #[cfg(feature = "render")]
    use super::{pump_and_forward_screencast_frames, pump_live_page_event_loop};
    use obscura_net::{CookieInfo, CookieJar};
    use serde_json::json;
    use std::collections::HashMap;

    fn request_pause(resolver: tokio::sync::oneshot::Sender<obscura_js::ops::InterceptResolution>) -> InterceptedPause {
        InterceptedPause { stage: obscura_js::ops::InterceptionStage::Request, redirect_response: false, resolver }
    }

    #[test]
    fn raw_header_projection_keeps_every_byte() {
        let projected = raw_header_bytes_to_cdp_string(b"\xff\xfeA");
        assert_eq!(
            projected.chars().map(u32::from).collect::<Vec<_>>(),
            vec![255, 254, 65]
        );
    }

    #[test]
    fn invalid_browser_close_is_an_error_and_keeps_the_connection_open() {
        let invalid: crate::types::CdpRequest = serde_json::from_value(json!({
            "id": 7,
            "method": "Browser.close",
            "params": {"invented": true},
        }))
        .unwrap();
        let (response, close_connection) =
            browser_close_response(&invalid).expect("Browser.close is intercepted");
        assert!(!close_connection);
        assert!(response.result.is_none());
        assert_eq!(
            response.error.as_ref().map(|error| error.message.as_str()),
            Some("Browser.close supports only empty params")
        );

        let valid: crate::types::CdpRequest = serde_json::from_value(json!({
            "id": 8,
            "method": "Browser.close",
            "params": {},
        }))
        .unwrap();
        assert!(browser_close_response(&valid).unwrap().1);

        let other: crate::types::CdpRequest = serde_json::from_value(json!({
            "id": 9,
            "method": "Browser.getVersion",
        }))
        .unwrap();
        assert!(browser_close_response(&other).is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn outbound_overflow_stops_before_later_queued_commands() {
        tokio::task::LocalSet::new()
            .run_until(async {
                let (server_tx, server_rx) = tokio::sync::mpsc::unbounded_channel();
                let (reply_tx, mut reply_rx, mut closed) =
                    crate::outbound::channel_with_limits(1, 1024 * 1024, 1024 * 1024);
                let shutdown = std::sync::Arc::new(tokio::sync::Notify::new());
                let default_context = crate::dispatch::CdpContext::new(
                    obscura_net::EffectivePersona::builtin(
                        obscura_net::StealthProfile::WindowsChrome145,
                    ),
                )
                .default_context;
                let processor = tokio::task::spawn_local(super::cdp_processor(
                    server_rx,
                    default_context,
                    shutdown,
                ));

                server_tx
                    .send(super::ServerMessage::NewConnection {
                        reply_tx: reply_tx.clone(),
                    })
                    .unwrap();
                drop(reply_rx.recv().await.expect("processor init"));

                server_tx
                    .send(super::ServerMessage::Cdp(super::CdpMessage {
                        text: json!({"id": 1, "method": "Browser.getVersion"}).to_string(),
                        reply_tx: reply_tx.clone(),
                    }))
                    .unwrap();
                tokio::time::timeout(std::time::Duration::from_secs(2), async {
                    while reply_tx.usage().0 != 1 {
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .expect("first response should occupy the queue");

                let (later_tx, mut later_rx, _) = crate::outbound::channel();
                server_tx
                    .send(super::ServerMessage::Cdp(super::CdpMessage {
                        text: json!({"id": 2, "method": "Browser.getVersion"}).to_string(),
                        reply_tx: reply_tx.clone(),
                    }))
                    .unwrap();
                server_tx
                    .send(super::ServerMessage::Cdp(super::CdpMessage {
                        text: json!({"id": 3, "method": "Browser.getVersion"}).to_string(),
                        reply_tx: later_tx,
                    }))
                    .unwrap();

                tokio::time::timeout(std::time::Duration::from_secs(2), closed.changed())
                    .await
                    .expect("outbound overflow should close the connection")
                    .expect("outbound close watch");
                assert_eq!(
                    reply_tx.close_reason(),
                    Some(crate::outbound::CloseReason::Count)
                );
                tokio::time::timeout(std::time::Duration::from_secs(2), processor)
                    .await
                    .expect("processor should stop after outbound overflow")
                    .expect("processor task");

                let first: serde_json::Value =
                    serde_json::from_str(reply_rx.try_recv().unwrap().as_str()).unwrap();
                assert_eq!(first["id"], 1);
                assert!(reply_rx.try_recv().is_err(), "overflow response must not be partial");
                assert!(
                    later_rx.try_recv().is_err(),
                    "a queued command after overflow must not be dispatched"
                );
            })
            .await;
    }

    #[test]
    fn closed_outbound_does_not_apply_queued_fetch_resolution() {
        let mut ctx = crate::dispatch::CdpContext::new(
            obscura_net::EffectivePersona::builtin(
                obscura_net::StealthProfile::WindowsChrome145,
            ),
        );
        let (reply_tx, _reply_rx, _) = crate::outbound::channel();
        let (resolver, mut resolved) = tokio::sync::oneshot::channel();
        let key = (None, "request-after-close".to_string());
        let mut paused = HashMap::from([(key.clone(), request_pause(resolver))]);
        reply_tx.close(crate::outbound::CloseReason::WriterIo);

        let command = json!({
            "id": 91,
            "method": "Fetch.continueRequest",
            "params": {"requestId": "request-after-close"}
        })
        .to_string();
        assert!(handle_fetch_resolution(&command, &mut ctx, &reply_tx, &mut paused));
        assert!(paused.contains_key(&key));
        assert!(matches!(
            resolved.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn browser_close_flushes_response_before_transport_shutdown() {
        use futures_util::{SinkExt as _, StreamExt as _};

        tokio::task::LocalSet::new()
            .run_until(async {
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                let address = listener.local_addr().unwrap();
                let (server_tx, mut server_rx) = tokio::sync::mpsc::unbounded_channel();

                let processor = tokio::task::spawn_local(async move {
                    while let Some(message) = server_rx.recv().await {
                        match message {
                            super::ServerMessage::NewConnection { reply_tx } => {
                                reply_tx.send(json!({"__init": true}).to_string()).unwrap();
                            }
                            super::ServerMessage::Cdp(_) => {
                                panic!("Browser.close must be handled at the transport boundary");
                            }
                        }
                    }
                });
                let server = tokio::task::spawn_local(async move {
                    let (stream, _) = listener.accept().await.unwrap();
                    super::handle_connection_ws(stream, server_tx).await
                });

                let (mut client, _) = tokio_tungstenite::connect_async(
                    format!("ws://{address}/devtools/browser"),
                )
                .await
                .unwrap();
                client
                    .send(tokio_tungstenite::tungstenite::Message::Text(
                        json!({"id": 77, "method": "Browser.close", "params": {}})
                            .to_string()
                            .into(),
                    ))
                    .await
                    .unwrap();
                let response = tokio::time::timeout(
                    std::time::Duration::from_secs(2),
                    client.next(),
                )
                .await
                .expect("Browser.close response timeout")
                .expect("transport closed before Browser.close response")
                .expect("Browser.close websocket response");
                let response: serde_json::Value = serde_json::from_str(
                    response.into_text().expect("text response").as_str(),
                )
                .unwrap();
                assert_eq!(response["id"], 77);
                assert_eq!(response["result"], json!({}));

                tokio::time::timeout(std::time::Duration::from_secs(2), server)
                    .await
                    .expect("server should close after flushing response")
                    .expect("server task")
                    .expect("connection handler");
                tokio::time::timeout(std::time::Duration::from_secs(2), processor)
                    .await
                    .expect("mock processor should stop with transport")
                    .expect("mock processor task");
            })
            .await;
    }

    #[test]
    fn discovery_uses_the_client_facing_http_authority() {
        let request = "GET /json/version HTTP/1.1\r\nhOsT: cdp.example.test:9222\r\n\r\n";
        assert_eq!(
            websocket_authority(request, 9223),
            "cdp.example.test:9222"
        );

        let malformed = "GET /json/version HTTP/1.1\r\nHost: attacker.test/path\r\n\r\n";
        assert_eq!(websocket_authority(malformed, 9223), "127.0.0.1:9223");
    }

    fn cookie(name: &str, value: &str) -> CookieInfo {
        CookieInfo {
            name: name.to_string(),
            value: value.to_string(),
            domain: "example.com".to_string(),
            path: "/".to_string(),
            secure: false,
            http_only: false,
            same_site: "Lax".to_string(),
            expires: None,
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn page_runtime_advances_while_cdp_client_is_silent() {
        tokio::task::LocalSet::new()
            .run_until(async {
                let (server_tx, server_rx) = tokio::sync::mpsc::unbounded_channel();
                let (reply_tx, mut reply_rx, _) = crate::outbound::channel();
                let shutdown = std::sync::Arc::new(tokio::sync::Notify::new());
                let default_context = crate::dispatch::CdpContext::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145)).default_context;
                let processor = tokio::task::spawn_local(super::cdp_processor(
                    server_rx,
                    default_context,
                    shutdown,
                ));

                server_tx
                    .send(super::ServerMessage::NewConnection {
                        reply_tx: reply_tx.clone(),
                    })
                    .unwrap();
                let init = reply_rx.recv().await.expect("processor init");
                assert!(init.contains("__init"));

                let send = |value: serde_json::Value| {
                    server_tx
                        .send(super::ServerMessage::Cdp(super::CdpMessage {
                            text: value.to_string(),
                            reply_tx: reply_tx.clone(),
                        }))
                        .unwrap();
                };
                send(json!({
                    "id": 1,
                    "method": "Target.createTarget",
                    "params": {"url": "about:blank"},
                }));

                let mut session_id = None;
                loop {
                    let value: serde_json::Value = serde_json::from_str(
                        &tokio::time::timeout(
                            std::time::Duration::from_secs(2),
                            reply_rx.recv(),
                        )
                        .await
                        .expect("create target response timeout")
                        .expect("create target response channel"),
                    )
                    .unwrap();
                    if session_id.is_none() {
                        session_id = value["params"]["sessionId"]
                            .as_str()
                            .map(str::to_string);
                    }
                    if value["id"] == 1 {
                        break;
                    }
                }
                let session_id = session_id.expect("attached page session");

                send(json!({
                    "id": 2,
                    "method": "Runtime.evaluate",
                    "sessionId": session_id,
                    "params": {
                        "expression": "(() => { setTimeout(() => globalThis.__autonomousDone = 'yes', 40); return 'armed'; })()",
                        "returnByValue": true,
                    },
                }));
                loop {
                    let value: serde_json::Value = serde_json::from_str(
                        &tokio::time::timeout(
                            std::time::Duration::from_secs(2),
                            reply_rx.recv(),
                        )
                        .await
                        .expect("timer arm response timeout")
                        .expect("timer arm response channel"),
                    )
                    .unwrap();
                    if value["id"] == 2 {
                        break;
                    }
                }

                // This is deliberately host/client time. No CDP message is sent
                // while the timeout becomes due; Chrome's renderer still runs,
                // and Obscura's connection-owned page pump must do the same.
                tokio::time::sleep(std::time::Duration::from_millis(120)).await;

                send(json!({
                    "id": 3,
                    "method": "Runtime.evaluate",
                    "sessionId": session_id,
                    "params": {
                        "expression": "globalThis.__autonomousDone || 'missing'",
                        "returnByValue": true,
                    },
                }));
                loop {
                    let value: serde_json::Value = serde_json::from_str(
                        &tokio::time::timeout(
                            std::time::Duration::from_secs(2),
                            reply_rx.recv(),
                        )
                        .await
                        .expect("timer observation response timeout")
                        .expect("timer observation response channel"),
                    )
                    .unwrap();
                    if value["id"] == 3 {
                        assert_eq!(value["result"]["result"]["value"], "yes");
                        break;
                    }
                }

                drop(server_tx);
                tokio::time::timeout(std::time::Duration::from_secs(2), processor)
                    .await
                    .expect("processor shutdown timeout")
                    .expect("processor task");
            })
            .await;
    }

    #[test]
    fn cookie_delta_merges_changes_without_reverting_other_connections() {
        let destination = CookieJar::new();
        destination.set_cookies_from_cdp(vec![cookie("sid", "newer"), cookie("other", "kept"), cookie("removed", "old")]);
        let connection = CookieJar::new();
        connection.set_cookies_from_cdp(vec![cookie("sid", "old"), cookie("removed", "old")]);
        let initial = connection.snapshot();
        connection.delete_cookies_filtered("removed", "example.com", Some("/"));
        connection.set_cookies_from_cdp(vec![cookie("added", "value")]);
        let host = url::Url::parse("https://example.com/").unwrap();
        connection.set_cookie("host=private; Path=/", &host);

        destination.apply_snapshot_delta(&initial, &connection.snapshot());
        assert!(destination.get_cookie_header(&host).contains("host=private"));
        assert!(!destination.get_cookie_header(&url::Url::parse("https://sub.example.com/").unwrap())
            .contains("host=private"));

        let cookies = destination.get_all_cookies();
        assert!(cookies.iter().any(|c| c.name == "sid" && c.value == "newer"));
        assert!(cookies.iter().any(|c| c.name == "other"));
        assert!(cookies.iter().any(|c| c.name == "added"));
        assert!(!cookies.iter().any(|c| c.name == "removed"));
    }

    // Issue #363: only an exact Page.navigate may take the spawn-and-defer
    // navigation path. A substring match also caught Page.navigateToHistoryEntry
    // (goBack / goForward), which has no `url` param, so it was misrouted into
    // the raw-navigate path and failed with "Invalid URL" instead of reaching
    // its real handler.
    #[test]
    fn only_exact_page_navigate_routes_as_navigation() {
        assert!(is_navigate_method(
            r#"{"id":1,"method":"Page.navigate","params":{"url":"https://example.com"}}"#
        ));
        assert!(!is_navigate_method(
            r#"{"id":2,"method":"Page.navigateToHistoryEntry","params":{"entryId":0}}"#
        ));
    }

    // A Runtime.evaluate whose expression merely contains the literal
    // "Page.navigate" must not be misrouted, and malformed input is not a
    // navigation.
    #[test]
    fn unrelated_methods_do_not_route_as_navigation() {
        assert!(!is_navigate_method(
            r#"{"id":3,"method":"Runtime.evaluate","params":{"expression":"'Page.navigate'"}}"#
        ));
        assert!(!is_navigate_method("not json"));
    }

    // Issue #365: Fetch.continueRequest header overrides must be parsed from the
    // CDP `[{name, value}]` list so they can be applied to the outgoing request.
    #[test]
    fn parse_cdp_headers_reads_name_value_pairs() {
        let params = json!({
            "headers": [
                {"name": "X-A", "value": "1"},
                {"name": "X-B", "value": "2"},
            ]
        });
        let headers = parse_cdp_headers(&params).unwrap().expect("headers present");
        assert_eq!(headers, vec![("X-A".into(), "1".into()), ("X-B".into(), "2".into())]);
    }

    // No `headers` field means "leave the request's headers untouched", which is
    // None, not an empty map that would clear them.
    #[test]
    fn parse_cdp_headers_absent_is_none() {
        assert!(parse_cdp_headers(&json!({"url": "https://example.com"})).unwrap().is_none());
    }

    pub(crate) fn continue_header_fields() -> serde_json::Value {
        json!([
            {"name":"X-Test","value":"first"},
            {"name":"Authorization","value":"Bearer complete-secret+/="},
            {"name":"x-test","value":"second"},
            {"name":"X-Test","value":""},
            {"name":"Cookie","value":"explicit=complete-secret+/="},
            {"name":"X-Empty","value":""},
            {"name":"cOoKiE","value":"second=keep; third=all"},
            {"name":"X-Unicode","value":"完整值"},
            {"name":"X-Complete","value":format!(" \t{} complete-secret+/=\t ", "full".repeat(2048))},
            {"name":"Accept-Language","value":"fr"},
            {"name":"accept-language","value":"de"}
        ])
    }

    pub(crate) fn malformed_continue_headers() -> Vec<serde_json::Value> {
        vec![json!(null), json!({}), json!(42), json!("headers"), json!([null]), json!([1]),
            json!([{}]), json!([{"name":"X"}]), json!([{"value":"v"}]),
            json!([{"name":1,"value":"v"}]), json!([{"name":"X","value":null}]),
            json!([{"name":"X","value":1}]), json!([{"name":"","value":"v"}]),
            json!([{"name":"Bad Name","value":"v"}]), json!([{"name":"é","value":"v"}]),
            json!([{"name":"X","value":"a\r\nb"}]), json!([{"name":"X","value":"a\0b"}]),
            json!([{"name":"Good","value":"keep"},{"name":"Bad:","value":"v"}])]
    }

    #[test]
    fn continue_headers_server_preserves_order_case_values_and_retry() {
        let mut ctx = crate::dispatch::CdpContext::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));
        let (reply_tx, mut reply_rx, _) = crate::outbound::channel();
        for supplied in [continue_header_fields(), json!([])] {
            let (resolver, mut resolved) = tokio::sync::oneshot::channel();
            let mut paused = HashMap::from([((None, "continued".to_string()), request_pause(resolver))]);
            for invalid in malformed_continue_headers() {
                let command = json!({"id":1,"method":"Fetch.continueRequest",
                    "params":{"requestId":"continued","headers":invalid}}).to_string();
                assert!(handle_fetch_resolution(&command, &mut ctx, &reply_tx, &mut paused));
                let reply: serde_json::Value = serde_json::from_str(&reply_rx.try_recv().unwrap()).unwrap();
                assert_eq!(reply["error"]["code"], -32602);
                assert_eq!(paused.len(), 1);
                assert!(matches!(resolved.try_recv(), Err(tokio::sync::oneshot::error::TryRecvError::Empty)));
            }
            let command = json!({"id":2,"method":"Fetch.continueRequest",
                "params":{"requestId":"continued","headers":supplied}}).to_string();
            assert!(handle_fetch_resolution(&command, &mut ctx, &reply_tx, &mut paused));
            let obscura_js::ops::InterceptResolution::ContinueWithHeaders { headers, .. } = resolved.try_recv().unwrap() else { panic!("expected ordered continue") };
            let roundtrip: Vec<_> = headers.into_iter().map(|(name, value)| json!({"name":name,"value":value})).collect();
            assert_eq!(json!(roundtrip), supplied);
            let reply: serde_json::Value = serde_json::from_str(&reply_rx.try_recv().unwrap()).unwrap();
            assert!(reply.get("error").is_none());
            assert!(paused.is_empty());
        }
    }

    #[test]
    fn continue_post_data_server_preserves_bytes_and_retryable_errors() {
        let mut ctx = crate::dispatch::CdpContext::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));
        let (reply_tx, mut reply_rx, _) = crate::outbound::channel();
        for (value, expected) in [(Some(json!("AP8A/w==")), Some(vec![0, 255, 0, 255])),
            (Some(json!("")), Some(vec![])), (None, None)] {
            let (resolver, mut resolved) = tokio::sync::oneshot::channel();
            let mut paused = HashMap::from([((None, "continued".to_string()), request_pause(resolver))]);
            for invalid in [json!("%"), json!("AP8"), json!("AP8=\n"), json!("AP9="), json!(7), json!(null), json!("%") ] {
                let command = json!({"id":1,"method":"Fetch.continueRequest",
                    "params":{"requestId":"continued","postData":invalid}}).to_string();
                assert!(handle_fetch_resolution(&command, &mut ctx, &reply_tx, &mut paused));
                let reply: serde_json::Value = serde_json::from_str(&reply_rx.try_recv().unwrap()).unwrap();
                assert_eq!(reply["error"]["code"], -32602);
                assert!(reply.get("sessionId").is_none());
                assert!(paused.contains_key(&(None, "continued".into())));
                assert!(matches!(resolved.try_recv(), Err(tokio::sync::oneshot::error::TryRecvError::Empty)));
            }
            let mut params = json!({"requestId":"continued","url":"https://example.com/new","method":"PUT",
                "headers":[{"name":"Authorization","value":"Bearer complete-secret"}]});
            if let Some(value) = value { params["postData"] = value; }
            let command = json!({"id":2,"method":"Fetch.continueRequest","params":params}).to_string();
            assert!(handle_fetch_resolution(&command, &mut ctx, &reply_tx, &mut paused));
            let obscura_js::ops::InterceptResolution::ContinueWithHeaders { body, url, method, headers } = resolved.try_recv().unwrap() else { panic!("expected continue") };
            assert_eq!(body, expected);
            assert_eq!(url.as_deref(), Some("https://example.com/new"));
            assert_eq!(method.as_deref(), Some("PUT"));
            assert_eq!(headers, vec![("Authorization".into(), "Bearer complete-secret".into())]);
            let reply: serde_json::Value = serde_json::from_str(&reply_rx.try_recv().unwrap()).unwrap();
            assert!(reply.get("error").is_none());
            assert!(paused.is_empty());
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn continue_post_data_and_headers_js_and_worker_send_exact_transport_bytes() {
        use base64::Engine as _;
        use std::io::{Read, Write};
        use std::sync::Arc;
        let binary: Vec<u8> = (0..1025).map(|i| (i % 256) as u8).collect();
        let expected_binary = binary.clone();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let proxy = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
            let mut seen = Vec::new();
            while seen.len() < 7 && std::time::Instant::now() < deadline {
                let (mut socket, _) = match listener.accept() {
                    Ok(socket) => socket,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(2)); continue;
                    }
                    Err(error) => panic!("{error}"),
                };
                socket.set_nonblocking(false).unwrap();
                socket.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
                let mut request = Vec::new();
                let header_end = loop {
                    let mut buf = [0; 2048];
                    let count = socket.read(&mut buf).unwrap();
                    assert!(count > 0); request.extend_from_slice(&buf[..count]);
                    if let Some(end) = request.windows(4).position(|part| part == b"\r\n\r\n") { break end + 4; }
                };
                let headers = std::str::from_utf8(&request[..header_end]).unwrap().to_string();
                let length = headers.lines().filter_map(|line| line.split_once(':'))
                    .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                    .map(|(_, value)| value.trim().parse::<usize>().unwrap()).unwrap_or(0);
                while request.len() < header_end + length {
                    let mut buf = [0; 2048]; let count = socket.read(&mut buf).unwrap();
                    assert!(count > 0); request.extend_from_slice(&buf[..count]);
                }
                let path = headers.split_whitespace().nth(1).unwrap();
                let body = &request[header_end..header_end + length];
                if path.ends_with("/binary") {
                    assert!(headers.starts_with("PUT "));
                    assert!(headers.contains("authorization: Bearer complete-secret+/=\r\n"));
                    let values = |name: &str| headers.lines().filter_map(|line| line.split_once(':'))
                        .filter(|(key, _)| key.eq_ignore_ascii_case(name))
                        .map(|(_, value)| value.strip_prefix(' ').unwrap_or(value).to_string()).collect::<Vec<_>>();
                    assert_eq!(values("x-test"), ["first", "second", ""]);
                    assert_eq!(values("cookie"), ["session=complete-secret", "explicit=complete-secret+/=", "second=keep; third=all"]);
                    assert_eq!(values("x-empty"), [""]);
                    assert_eq!(values("x-unicode"), ["完整值"]);
                    assert_eq!(values("x-complete"), [format!(" \t{} complete-secret+/=\t ", "full".repeat(2048))]);
                    assert!(values("x-original").is_empty());
                    assert_eq!(values("accept-language"), ["fr", "de"]);
                    assert_eq!(values("x-page"), ["retained"]);
                    assert_eq!(body, expected_binary);
                } else if path.ends_with("/empty") {
                    assert!(headers.starts_with("POST ")); assert!(body.is_empty());
                    assert!(!headers.to_ascii_lowercase().contains("x-original:"));
                    assert!(headers.to_ascii_lowercase().contains("x-test: page-default\r\n"));
                } else if path.ends_with("/original") {
                    assert!(headers.starts_with("POST ")); assert_eq!(body, [0, 254, 255, 65]);
                    assert!(headers.to_ascii_lowercase().contains("x-original: keep\r\n"));
                } else { assert_eq!(path, "http://continue.test/"); assert!(body.is_empty()); }
                if seen.len() > 0 { assert!(headers.to_ascii_lowercase().contains("cookie: session=complete-secret\r\n")); }
                seen.push(path.to_string());
                socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nSet-Cookie: session=complete-secret; Path=/\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok").unwrap();
            }
            assert_eq!(seen.len(), 7, "all page/Worker requests must reach transport: {seen:?}");
            for path in ["binary", "empty", "original"] {
                assert_eq!(seen.iter().filter(|url| url.ends_with(path)).count(), 2);
            }
        });
        let mut page_ctx = crate::dispatch::CdpContext::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));
        page_ctx.default_context = Arc::new(obscura_browser::BrowserContext::with_proxy("continue-body".into(), obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145), Some(proxy)));
        let page_id = page_ctx.create_page();
        let page = page_ctx.get_page_mut(&page_id).unwrap();
        page.stealth_client.set_extra_headers(HashMap::from([
            ("x-test".into(), "page-default".into()), ("X-Page".into(), "retained".into()),
        ])).await;
        page.navigate("http://continue.test/").await.unwrap();
        let mut requests = page.enable_interception();
        let mut resolver_ctx = crate::dispatch::CdpContext::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));
        let (reply_tx, mut reply_rx, _) = crate::outbound::channel();
        let respond = async {
            for _ in 0..6 {
                let request = requests.recv().await.unwrap();
                let mut params = json!({"requestId":request.request_id});
                if request.url.ends_with("/rewrite") {
                    params["url"] = json!("http://continue.test/binary");
                    params["method"] = json!("PUT");
                    params["headers"] = continue_header_fields();
                    params["postData"] = json!(base64::engine::general_purpose::STANDARD.encode(&binary));
                } else if request.url.ends_with("/empty") { params["postData"] = json!(""); params["headers"] = json!([]); }
                let mut paused = HashMap::from([((None, request.request_id.clone()), request_pause(request.resolver))]);
                // Invalid fields must neither send nor lose the actual Page/Worker request.
                for invalid in malformed_continue_headers() {
                    let command = json!({"id":1,"method":"Fetch.continueRequest","params":{
                        "requestId":request.request_id,"headers":invalid}}).to_string();
                    assert!(handle_fetch_resolution(&command, &mut resolver_ctx, &reply_tx, &mut paused));
                    let reply: serde_json::Value = serde_json::from_str(&reply_rx.try_recv().unwrap()).unwrap();
                    assert_eq!(reply["error"]["code"], -32602); assert_eq!(paused.len(), 1);
                }
                for _ in 0..2 {
                    let command = json!({"id":1,"method":"Fetch.continueRequest","params":{
                        "requestId":request.request_id,"postData":"%"}}).to_string();
                    assert!(handle_fetch_resolution(&command, &mut resolver_ctx, &reply_tx, &mut paused));
                    let reply: serde_json::Value = serde_json::from_str(&reply_rx.try_recv().unwrap()).unwrap();
                    assert_eq!(reply["error"]["code"], -32602); assert_eq!(paused.len(), 1);
                }
                let command = json!({"id":2,"method":"Fetch.continueRequest","params":params}).to_string();
                assert!(handle_fetch_resolution(&command, &mut resolver_ctx, &reply_tx, &mut paused));
                let reply: serde_json::Value = serde_json::from_str(&reply_rx.try_recv().unwrap()).unwrap();
                assert!(reply.get("error").is_none()); assert!(paused.is_empty());
            }
        };
        let result = page.evaluate_for_cdp(r#"(async () => {
            async function sendBodies() {
                const results = [];
                for (const path of ['rewrite', 'empty', 'original']) {
                    results.push(await (await fetch('http://continue.test/' + path,
                        {method:'POST', credentials:'include', headers:{'X-Original':'keep'}, body:new Uint8Array([0,254,255,65])})).text());
                }
                return results;
            }
            const parent = await sendBodies();
            const worker = new Worker(URL.createObjectURL(new Blob([
                sendBodies.toString() + '; sendBodies().then(postMessage);'
            ], {type:'application/javascript'})));
            const child = await new Promise(resolve => { worker.onmessage = e => resolve(e.data); });
            worker.terminate();
            return [parent, child];
        })()"#, true, true);
        let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(15), async {
            tokio::join!(result, respond)
        }).await.expect("Page/Worker Continue requests must complete within 15 seconds, including transport and interception");
        assert!(!result.thrown, "{result:?}");
        assert_eq!(result.value, Some(json!([["ok", "ok", "ok"], ["ok", "ok", "ok"]])));
        server.join().unwrap();
    }

    #[test]
    fn fulfilled_headers_preserve_duplicates_binary_values_and_capture_source() {
        use base64::Engine as _;
        let mut ctx = crate::dispatch::CdpContext::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));
        let (reply_tx, mut reply_rx, _) = crate::outbound::channel();
        let binary = b"Set-Cookie: session=first-secret\0sEt-CoOkIe: session=second-secret\0X-Bytes: \xff\xfe\0";
        for params in [
            json!({"responseHeaders":[{"name":"Set-Cookie","value":"session=first-secret"},{"name":"sEt-CoOkIe","value":"session=second-secret"},{"name":"X-Unicode","value":"原文"}]}),
            json!({"binaryResponseHeaders":base64::engine::general_purpose::STANDARD.encode(binary)}),
        ] {
            let (resolver, mut resolved) = tokio::sync::oneshot::channel();
            let mut paused = HashMap::from([((None, "fulfilled".to_string()), request_pause(resolver))]);
            let mut params = params;
            params["requestId"] = json!("fulfilled"); params["body"] = json!("AP8="); params["responseCode"] = json!(201);
            let command = json!({"id":1,"method":"Fetch.fulfillRequest","params":params}).to_string();
            assert!(handle_fetch_resolution(&command, &mut ctx, &reply_tx, &mut paused));
            let obscura_js::ops::InterceptResolution::FulfillWithHeaders { raw_headers, headers, body_base64, status, .. } = resolved.try_recv().unwrap() else { panic!("missing captured fulfill") };
            assert_eq!(status, 201);
            assert_eq!(body_base64, "AP8=");
            assert_eq!(raw_headers.capture_stage, "cdpFulfillResponse");
            assert_eq!(raw_headers.fields.len(), 3);
            assert_eq!(raw_headers.fields[0].name, b"Set-Cookie");
            assert_eq!(raw_headers.fields[1].name, b"sEt-CoOkIe");
            assert_eq!(raw_headers.fields[0].value, b"session=first-secret");
            assert_eq!(raw_headers.fields[1].value, b"session=second-secret");
            assert_eq!(headers["set-cookie"], "session=second-secret");
            if params.get("binaryResponseHeaders").is_some() {
                assert_eq!(raw_headers.fields[2].value, [0xff, 0xfe]);
                assert!(!headers.contains_key("x-bytes"));
            } else {
                assert_eq!(raw_headers.fields[2].value, "原文".as_bytes());
            }
            let reply: serde_json::Value = serde_json::from_str(&reply_rx.try_recv().unwrap()).unwrap();
            assert!(reply.get("error").is_none());
            assert!(paused.is_empty());
        }
    }

    #[test]
    fn fulfilled_invalid_headers_keep_request_paused_for_retry() {
        use base64::Engine as _;
        let mut ctx = crate::dispatch::CdpContext::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));
        let (reply_tx, mut reply_rx, _) = crate::outbound::channel();
        let (resolver, mut resolved) = tokio::sync::oneshot::channel();
        let mut paused = HashMap::from([((None, "fulfilled".to_string()), request_pause(resolver))]);
        for params in [json!({"binaryResponseHeaders":"%"}), json!({"body":"%"}), json!({"body":7}),
            json!({"binaryResponseHeaders":base64::engine::general_purpose::STANDARD.encode(b"missing colon")}),
            json!({"responseHeaders":{},"binaryResponseHeaders":""}),
            json!({"responseHeaders":[{"name":"missing-value"}]}),
        ] {
            let mut params = params; params["requestId"] = json!("fulfilled"); params["responseCode"] = json!(200);
            let command = json!({"id":1,"method":"Fetch.fulfillRequest","params":params}).to_string();
            assert!(handle_fetch_resolution(&command, &mut ctx, &reply_tx, &mut paused));
            let reply: serde_json::Value = serde_json::from_str(&reply_rx.try_recv().unwrap()).unwrap();
            assert_eq!(reply["error"]["code"], -32602);
            assert!(paused.contains_key(&(None, "fulfilled".into())));
            assert!(matches!(resolved.try_recv(), Err(tokio::sync::oneshot::error::TryRecvError::Empty)));
        }
        let command = json!({"id":2,"method":"Fetch.fulfillRequest","params":{"requestId":"fulfilled","responseCode":200,"body":""}}).to_string();
        assert!(handle_fetch_resolution(&command, &mut ctx, &reply_tx, &mut paused));
        let obscura_js::ops::InterceptResolution::FulfillWithHeaders { body, body_base64, .. } = resolved.try_recv().unwrap() else { panic!("missing captured fulfill") };
        assert!(body.is_empty()); assert!(body_base64.is_empty());
    }

    #[test]
    fn fetch_body_read_during_request_pause_errors_without_dropping_resolver() {
        let mut ctx = crate::dispatch::CdpContext::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));
        let (reply_tx, mut reply_rx, _) = crate::outbound::channel();
        let (resolver, mut resolved) = tokio::sync::oneshot::channel();
        let mut paused = HashMap::from([((None, "paused".to_string()), request_pause(resolver))]);
        for method in ["Fetch.getResponseBody", "Fetch.takeResponseBodyAsStream"] {
            let command = json!({"id": 1, "method": method, "params": {"requestId": "paused"}}).to_string();
            assert!(handle_fetch_resolution(&command, &mut ctx, &reply_tx, &mut paused));
            let reply: serde_json::Value = serde_json::from_str(&reply_rx.try_recv().unwrap()).unwrap();
            assert_eq!(reply["id"], 1);
            assert!(reply.get("sessionId").is_none());
            assert!(reply["error"]["message"].as_str().unwrap().contains("response_body_not_ready"));
            assert!(paused.contains_key(&(None, "paused".into())));
            assert!(matches!(resolved.try_recv(), Err(tokio::sync::oneshot::error::TryRecvError::Empty)));
        }
        let unrelated = json!({"id": 2, "method": "Fetch.unsupported", "params": {"requestId": "paused"}}).to_string();
        assert!(!handle_fetch_resolution(&unrelated, &mut ctx, &reply_tx, &mut paused));
        assert!(paused.contains_key(&(None, "paused".into())));
        let command = json!({"id": 3, "method": "Fetch.continueRequest", "params": {"requestId": "paused"}}).to_string();
        assert!(handle_fetch_resolution(&command, &mut ctx, &reply_tx, &mut paused));
        assert!(matches!(resolved.try_recv().unwrap(), obscura_js::ops::InterceptResolution::Continue { .. }));
    }

    #[test]
    fn fetch_resolution_is_handled_once_by_the_outer_processor() {
        let (resolution_tx, mut resolution_rx) = tokio::sync::oneshot::channel();
        let mut paused = HashMap::from([((None, "request-1".to_string()), request_pause(resolution_tx))]);
        let (reply_tx, mut reply_rx, _) = crate::outbound::channel();
        let mut ctx = crate::dispatch::CdpContext::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));

        assert!(handle_fetch_resolution(
            r#"{"id":17,"method":"Fetch.continueRequest","params":{"requestId":"request-1"}}"#,
            &mut ctx,
            &reply_tx,
            &mut paused,
        ));
        assert!(matches!(
            resolution_rx.try_recv(),
            Ok(obscura_js::ops::InterceptResolution::Continue { .. })
        ));
        let response: serde_json::Value =
            serde_json::from_str(&reply_rx.try_recv().expect("one command response")).unwrap();
        assert_eq!(response["id"], 17);
        assert!(reply_rx.try_recv().is_err(), "must not emit a duplicate response");
    }

    #[cfg(feature = "render")]
    #[tokio::test(flavor = "current_thread")]
    async fn autonomous_screencast_pumps_timers_and_retains_backpressured_damage() {
        let mut ctx = crate::dispatch::CdpContext::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));
        let page_id = ctx.create_page();
        let session_id = format!("{page_id}-session");
        ctx.sessions.insert(session_id.clone(), page_id);
        let session = Some(session_id.clone());
        ctx.get_session_page_mut(&session)
            .expect("page")
            .set_viewport((96.0, 64.0));
        crate::domains::page::handle(
            "navigate",
            &json!({
                "url": "data:text/html,<html style='margin:0'><body style='margin:0;width:96px;height:64px;background:red'></body></html>",
                "waitUntil": "load",
            }),
            &mut ctx,
            &session,
        )
        .await
        .expect("navigate screencast fixture");
        ctx.pending_events.clear();
        crate::domains::page::handle(
            "startScreencast",
            &json!({}),
            &mut ctx,
            &session,
        )
        .await
        .expect("start screencast");
        let stream_id = ctx
            .pending_events
            .iter()
            .find(|event| event.method == "Page.screencastFrame")
            .and_then(|event| event.params["sessionId"].as_i64())
            .expect("initial stream id");
        ctx.pending_events.clear();

        let (reply_tx, mut reply_rx, _) = crate::outbound::channel();
        ctx.get_session_page_mut(&session)
            .expect("page")
            .evaluate(
                "setTimeout(() => document.body.setAttribute('style', 'margin:0;width:96px;height:64px;background:green'), 0)",
            );
        pump_live_page_event_loop(&mut ctx).await.unwrap();
        pump_and_forward_screencast_frames(&mut ctx, Some(&reply_tx)).await;
        let first_update: serde_json::Value = serde_json::from_str(
            &reply_rx.try_recv().expect("timer mutation should emit a frame"),
        )
        .unwrap();
        assert_eq!(first_update["method"], "Page.screencastFrame");
        assert_eq!(first_update["params"]["sessionId"], stream_id);
        assert!(reply_rx.try_recv().is_err());
        assert_eq!(ctx.screencasts[&session_id].frames_in_flight, 2);

        // The second mutation is pumped while the two-frame acknowledgement
        // window is full. It must not emit yet, but its damage must remain
        // pending and appear immediately after capacity is returned.
        ctx.get_session_page_mut(&session)
            .expect("page")
            .evaluate(
                "setTimeout(() => document.body.setAttribute('style', 'margin:0;width:96px;height:64px;background:blue'), 0)",
            );
        pump_live_page_event_loop(&mut ctx).await.unwrap();
        pump_and_forward_screencast_frames(&mut ctx, Some(&reply_tx)).await;
        assert!(reply_rx.try_recv().is_err());
        assert!(ctx.screencasts[&session_id].autonomous_frame_pending);

        crate::domains::page::handle(
            "screencastFrameAck",
            &json!({"sessionId": stream_id}),
            &mut ctx,
            &session,
        )
        .await
        .expect("ack current frame");
        pump_and_forward_screencast_frames(&mut ctx, Some(&reply_tx)).await;
        let after_ack: serde_json::Value = serde_json::from_str(
            &reply_rx
                .try_recv()
                .expect("backpressured damage should emit after ack"),
        )
        .unwrap();
        assert_eq!(after_ack["method"], "Page.screencastFrame");
        assert_eq!(after_ack["params"]["sessionId"], stream_id);
        assert!(!ctx.screencasts[&session_id].autonomous_frame_pending);
    }

    #[cfg(feature = "render")]
    #[tokio::test(flavor = "current_thread")]
    async fn autonomous_screencast_observes_raf_visual_mutations() {
        let mut ctx = crate::dispatch::CdpContext::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145));
        let page_id = ctx.create_page();
        let session_id = format!("{page_id}-session");
        ctx.sessions.insert(session_id.clone(), page_id);
        let session = Some(session_id.clone());
        ctx.get_session_page_mut(&session)
            .expect("page")
            .set_viewport((96.0, 64.0));
        crate::domains::page::handle(
            "navigate",
            &json!({
                "url": "data:text/html,<html style='margin:0'><body style='margin:0;width:96px;height:64px;background:red'></body></html>",
                "waitUntil": "load",
            }),
            &mut ctx,
            &session,
        )
        .await
        .expect("navigate visual-damage fixture");
        ctx.pending_events.clear();
        crate::domains::page::handle(
            "startScreencast",
            &json!({}),
            &mut ctx,
            &session,
        )
        .await
        .expect("start screencast");
        let initial = ctx
            .pending_events
            .iter()
            .find(|event| event.method == "Page.screencastFrame")
            .expect("initial frame");
        let stream_id = initial.params["sessionId"].as_i64().unwrap();
        let initial_data = initial.params["data"].as_str().unwrap().to_string();
        ctx.pending_events.clear();
        crate::domains::page::handle(
            "screencastFrameAck",
            &json!({"sessionId": stream_id}),
            &mut ctx,
            &session,
        )
        .await
        .expect("ack initial frame");

        // Bypass CDP dispatch after scheduling the callback. The only path
        // which can deliver and capture this update is the active stream's
        // periodic event-loop/render pump.
        ctx.get_session_page_mut(&session)
            .expect("page")
            .evaluate(
                "requestAnimationFrame(() => document.body.setAttribute('style','margin:0;width:96px;height:64px;background:lime'))",
            );
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        pump_live_page_event_loop(&mut ctx).await.unwrap();
        pump_and_forward_screencast_frames(&mut ctx, None).await;
        let raf_frame = ctx
            .pending_events
            .iter()
            .find(|event| event.method == "Page.screencastFrame")
            .expect("RAF visual mutation must autonomously emit a frame");
        assert_ne!(
            raf_frame.params["data"].as_str().unwrap(),
            initial_data,
            "RAF-driven paint must capture the updated visible state"
        );
    }
}

#[cfg(test)]
#[path = "server/fetch_isolation_tests.rs"]
mod fetch_isolation_tests;
