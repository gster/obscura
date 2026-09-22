use std::sync::Arc;
use std::time::Instant;

use clap::{Parser, Subcommand};
use obscura_browser::{BrowserContext, Page};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command as TokioCommand;
use tokio::time::{timeout, Duration};

#[derive(Parser)]
#[command(
    name = "obscura",
    version = env!("OBSCURA_BUILD_VERSION"),
    about = "Obscura - A lightweight headless browser for web scraping and automation",
)]
struct Args {
    #[arg(short, long, global = true)]
    verbose: bool,

    /// Built-in persona name or path to a PersonaSpec JSON file. May also be
    /// supplied as OBSCURA_PERSONA. There is intentionally no implicit default.
    #[arg(long, global = true, value_name = "PRESET_OR_JSON")]
    persona: Option<String>,

    #[command(subcommand)]
    command: Option<Command>,

    #[arg(short, long, default_value_t = 9222)]
    port: u16,

    #[arg(long, global = true)]
    proxy: Option<String>,

    /// Respect robots.txt before navigating to an HTTP(S) URL.
    /// Global: applies to fetch and scrape.
    #[arg(long, global = true)]
    obey_robots: bool,

    #[arg(long)]
    storage_dir: Option<std::path::PathBuf>,

    /// Permit fetches to loopback, RFC1918, and link-local addresses.
    /// Default is to block them (SSRF fix from #4). Use this for local
    /// development against http://localhost:N or http://192.168.x.y.
    /// Equivalent to `OBSCURA_ALLOW_PRIVATE_NETWORK=1` but per-process
    /// and survives in command pipelines.
    #[arg(long, global = true)]
    allow_private_network: bool,

    /// Pass raw flags to V8, in the same form V8/Chromium/Node accept
    /// (e.g. `"--max-old-space-size=4096 --max-semi-space-size=64 --expose-gc"`).
    /// Applied once at startup before any isolate is created.
    #[arg(long, value_name = "FLAGS", allow_hyphen_values = true)]
    v8_flags: Option<String>,
}

#[derive(Subcommand)]
enum Command {
    Serve {
        #[arg(short, long, default_value_t = 9222)]
        port: u16,

        // Bind address. Defaults to 127.0.0.1 (loopback only) for safety.
        // Set to 0.0.0.0 to listen on all interfaces (e.g. inside a Docker
        // container where you want the port to be reachable from the host
        // via -p mapping).
        #[arg(long, default_value = "127.0.0.1")]
        host: String,

        /// Exact Host authorities accepted by the CDP control plane. Repeat
        /// for reverse-proxy and alternate public authorities.
        #[arg(long = "allow-host", value_name = "HOST[:PORT]")]
        allowed_hosts: Vec<String>,

        /// Additional browser Origin values accepted by the CDP control plane.
        #[arg(long = "allow-origin", value_name = "ORIGIN")]
        allowed_origins: Vec<String>,

        /// Read the CDP Bearer token from this file. The
        /// OBSCURA_CDP_TOKEN environment variable is the container-friendly
        /// alternative; configuring both is an error.
        #[arg(long, value_name = "FILE")]
        auth_token_file: Option<std::path::PathBuf>,

        /// Public root ws:// or wss:// URL returned by discovery endpoints.
        #[arg(long, value_name = "WS_URL")]
        advertise_websocket_url: Option<String>,

        /// Permit a non-loopback listener without a Bearer token. Use only
        /// behind a separately authenticated boundary.
        #[arg(long)]
        allow_unauthenticated_remote: bool,

        #[arg(long)]
        proxy: Option<String>,

        /// Number of worker processes. In multi-worker mode each worker gets
        /// its own `--max-connections` allowance.
        #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u16).range(1..))]
        workers: u16,

        /// Maximum admitted CDP WebSocket connections, including authorized
        /// upgrades waiting for processor handoff and active connections. Each
        /// active connection runs on its own OS thread with its own V8 isolates.
        /// This limit is per worker when `--workers` is greater than one; the
        /// parent admits at most workers times this many simultaneous
        /// byte-transparent relays. Connections beyond the limit are refused
        /// with a 503 rather than queued outside the allowance.
        #[arg(long, default_value_t = obscura_cdp::DEFAULT_MAX_CONNECTIONS)]
        max_connections: usize,

        /// Allow CDP clients to navigate to file:// URLs. Off by
        /// default so a CDP connection cannot read arbitrary local
        /// files. Enable only when serving local HTML for testing
        /// and the port is on a trusted network.
        #[arg(long)]
        allow_file_access: bool,

        #[arg(long)]
        storage_dir: Option<std::path::PathBuf>,

        /// Recursively load TTF, TTC, OTF, and OTC files from this directory.
        /// Repeat for multiple directories. Requires a render-enabled build.
        #[arg(long = "font-dir", value_name = "DIR")]
        font_dirs: Vec<std::path::PathBuf>,

        /// Internal parent/child lifecycle channel. Not a supported user
        /// interface; multi-worker supervisors set this on their children.
        #[arg(long, hide = true, value_name = "INDEX")]
        supervised_worker: Option<u16>,

        /// Suppress all logs (same as on `fetch`). Useful when scraping pages
        /// that flood the console with per-page script warnings (issue #264).
        #[arg(long)]
        quiet: bool,
    },

    Fetch {
        // Optional so a batch run can pass URLs via --file instead. A single
        // positional URL keeps the original one-shot behaviour.
        url: Option<String>,

        // Default is html. Kept as Option so we can tell whether --dump was
        // explicitly passed: a bare --eval returns its own value, while --eval
        // combined with --dump (or --selector) runs the eval, lets its async
        // work settle, then reads the page (issue #248).
        #[arg(long)]
        dump: Option<DumpFormat>,

        /// Read newline-delimited URLs from a file (one per line; blank lines
        /// and lines starting with `#` are skipped). Use `-` for stdin. Enables
        /// batch mode: every URL is fetched raw (--dump original) and one JSON
        /// status line is printed per URL. For rendered/DOM batch output use
        /// `scrape` instead (issue #349).
        #[arg(long)]
        file: Option<std::path::PathBuf>,

        /// Number of URLs fetched concurrently in batch mode. Ignored without
        /// --file.
        #[arg(long, default_value_t = std::num::NonZeroUsize::new(1).unwrap())]
        concurrency: std::num::NonZeroUsize,

        #[arg(long)]
        selector: Option<String>,

        /// Maximum adaptive post-load settle time in seconds. When supplied
        /// explicitly, this is a fixed delay; the default is a 5-second cap
        /// that returns once the page is quiescent.
        #[arg(long)]
        wait: Option<u64>,

        #[arg(long, default_value_t = 30, value_parser = clap::value_parser!(u64).range(1..))]
        timeout: u64,

        #[arg(long, default_value = "load")]
        wait_until: String,

        #[arg(long, short)]
        eval: Option<String>,

        #[arg(long, short = 'o')]
        output: Option<std::path::PathBuf>,

        #[arg(long, short)]
        quiet: bool,

        #[arg(long)]
        storage_dir: Option<std::path::PathBuf>,

        /// Capture the settled page as a PNG. Requires the `render` feature.
        #[arg(long, short = 's', value_name = "FILE", conflicts_with = "file")]
        screenshot: Option<std::path::PathBuf>,
    },

    Scrape {
        urls: Vec<String>,

        #[arg(long, short)]
        eval: Option<String>,

        #[arg(long, default_value_t = std::num::NonZeroUsize::new(10).unwrap())]
        concurrency: std::num::NonZeroUsize,

        #[arg(long, default_value = "json")]
        format: String,

        #[arg(long, default_value_t = 60, value_parser = clap::value_parser!(u64).range(1..))]
        timeout: u64,

        #[arg(long, short)]
        quiet: bool,
    },

    Mcp {
        #[arg(long)]
        storage_dir: Option<std::path::PathBuf>,

        #[arg(long)]
        http: bool,

        #[arg(long, default_value = "127.0.0.1")]
        host: String,

        #[arg(long, default_value_t = 3000)]
        port: u16,

        #[arg(long)]
        proxy: Option<String>,

    },
}

#[derive(Clone, Debug, clap::ValueEnum, PartialEq, Eq)]
enum DumpFormat {
    Html,
    Text,
    Links,
    Markdown,
    /// Stream the raw HTTP response body verbatim (binary-safe).
    /// Bypasses the browser/JS layer — useful for fetching images,
    /// JSON, JS, CSS, or any non-HTML resource (cf. issue #117).
    Original,
    /// One JSON object per line listing every sub-resource URL the
    /// rendered page references (script src, link href, img src,
    /// iframe src, media sources, embed/object data). Lets callers
    /// replay the asset graph with their own HTTP client when they
    /// need the originals alongside the page (cf. issue 124).
    Assets,
    /// Dump all cookies in the browser jar as a JSON array, including
    /// HttpOnly cookies that are inaccessible via document.cookie.
    /// Useful for extracting session tokens set by anti-bot challenges.
    Cookies,
}

fn print_banner(host: &str, port: u16) {
    let authority = host
        .parse::<std::net::IpAddr>()
        .map(|ip| match ip {
            std::net::IpAddr::V4(ip) => format!("{ip}:{port}"),
            std::net::IpAddr::V6(ip) => format!("[{ip}]:{port}"),
        })
        .unwrap_or_else(|_| format!("{host}:{port}"));
    println!(
        r#"
   ____  _                              
  / __ \| |                             
 | |  | | |__  ___  ___ _   _ _ __ __ _ 
 | |  | | '_ \/ __|/ __| | | | '__/ _` |
 | |__| | |_) \__ \ (__| |_| | | | (_| |
  \____/|_.__/|___/\___|\__,_|_|  \__,_|
                   
  Headless Browser v{}
  CDP server: ws://{}/devtools/browser
"#,
        env!("OBSCURA_BUILD_VERSION"),
        authority
    );
}

fn load_cdp_access_token(
    token_file: Option<&std::path::Path>,
) -> anyhow::Result<Option<String>> {
    let environment = match std::env::var("OBSCURA_CDP_TOKEN") {
        Ok(token) => Some(token),
        Err(std::env::VarError::NotPresent) => None,
        Err(std::env::VarError::NotUnicode(_)) => {
            anyhow::bail!("OBSCURA_CDP_TOKEN must be valid UTF-8")
        }
    };
    if token_file.is_some() && environment.is_some() {
        anyhow::bail!(
            "configure the CDP token with either --auth-token-file or OBSCURA_CDP_TOKEN, not both"
        );
    }
    if let Some(path) = token_file {
        let mut token = std::fs::read_to_string(path)
            .map_err(|error| anyhow::anyhow!("read --auth-token-file: {error}"))?;
        while token.ends_with(['\r', '\n']) {
            token.pop();
        }
        return Ok(Some(token));
    }
    Ok(environment)
}

fn effective_cdp_allowed_hosts(
    host: &str,
    port: u16,
    configured: Vec<String>,
) -> anyhow::Result<Vec<String>> {
    if !configured.is_empty() {
        return Ok(configured);
    }
    let ip: std::net::IpAddr = host
        .parse()
        .map_err(|error| anyhow::anyhow!("invalid --host {host:?}: {error}"))?;
    if !ip.is_loopback() {
        return Ok(Vec::new());
    }
    // Leave loopback defaults unresolved when the OS chooses the port. The
    // server compiles the policy after bind against the actual local address.
    if port == 0 {
        return Ok(Vec::new());
    }
    let direct = match ip {
        std::net::IpAddr::V4(ip) => format!("{ip}:{port}"),
        std::net::IpAddr::V6(ip) => format!("[{ip}]:{port}"),
    };
    Ok(vec![direct, format!("localhost:{port}")])
}

#[derive(Clone)]
struct CdpServeAccess {
    allowed_hosts: Vec<String>,
    allowed_origins: Vec<String>,
    bearer_token: Option<String>,
    advertised_websocket_url: Option<String>,
    allow_unauthenticated_remote: bool,
}

impl CdpServeAccess {
    fn options(&self) -> obscura_cdp::CdpAccessOptions {
        obscura_cdp::CdpAccessOptions::new()
            .with_allowed_hosts(self.allowed_hosts.clone())
            .with_allowed_origins(self.allowed_origins.clone())
            .with_bearer_token(self.bearer_token.clone())
            .with_advertised_websocket_url(self.advertised_websocket_url.clone())
            .allow_unauthenticated_remote(self.allow_unauthenticated_remote)
    }

    fn configure_worker(&self, command: &mut std::process::Command) {
        match &self.bearer_token {
            Some(token) => {
                command.env("OBSCURA_CDP_TOKEN", token);
            }
            None => {
                command.env_remove("OBSCURA_CDP_TOKEN");
            }
        }
        for allowed_host in &self.allowed_hosts {
            command.arg("--allow-host").arg(allowed_host);
        }
        for allowed_origin in &self.allowed_origins {
            command.arg("--allow-origin").arg(allowed_origin);
        }
        if let Some(url) = &self.advertised_websocket_url {
            command.arg("--advertise-websocket-url").arg(url);
        }
    }
}

fn select_log_filter(verbose: bool, quiet: bool) -> &'static str {
    if verbose {
        "debug"
    } else if quiet {
        "off"
    } else {
        "warn"
    }
}

fn is_quiet_command(cmd: &Option<Command>) -> bool {
    matches!(
        cmd,
        Some(Command::Fetch { quiet: true, .. })
            | Some(Command::Scrape { quiet: true, .. })
            | Some(Command::Serve { quiet: true, .. })
    )
}

fn configure_font_directories(font_dirs: &[std::path::PathBuf]) -> anyhow::Result<()> {
    if font_dirs.is_empty() {
        return Ok(());
    }
    for directory in font_dirs {
        if !directory.is_dir() {
            anyhow::bail!(
                "Font directory does not exist or is not a directory: {}",
                directory.display()
            );
        }
    }

    #[cfg(feature = "render")]
    {
        if !obscura_js::configure_font_directories(font_dirs.to_vec()) {
            anyhow::bail!("Font directories must be configured before the first render");
        }
        Ok(())
    }
    #[cfg(not(feature = "render"))]
    anyhow::bail!("--font-dir requires a render-enabled build")
}

fn merge_proxy(global_proxy: Option<String>, command_proxy: Option<String>) -> Option<String> {
    command_proxy.or(global_proxy)
}

/// Normalize a raw `--v8-flags` value into the string we'll hand to V8.
/// Returns `None` when the user didn't pass the flag, passed an empty string,
/// or passed only whitespace; in those cases V8 is left untouched.
fn normalize_v8_flags(raw: Option<&str>) -> Option<String> {
    let trimmed = raw?.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Default V8 flags applied at startup unless the user disabled them via
/// `--v8-flags`. The default heap matches headless Chrome (~4 GB) so pages
/// that ship heavy fingerprinting or analytics bundles
/// (e.g. demo.fingerprint.com — issue #199) don't SIGTRAP out of the box.
/// V8 parses flags left-to-right and later wins, so anything the user
/// passes via `--v8-flags` overrides these.
///
/// `--max-semi-space-size=4` caps V8's young generation (default 16 MB per
/// semi-space) so a parse/JS allocation burst does not inflate RSS, and
/// `--optimize-for-size` trades memory-heavy codegen choices for a smaller
/// footprint. Together they cut RSS ~18% on heavy pages (ycombinator.com
/// 173 MB -> 140 MB) at no measurable speed cost (V8 still JITs hot paths).
#[cfg(target_pointer_width = "64")]
const DEFAULT_V8_FLAGS: &str =
    "--max-old-space-size=4096 --max-semi-space-size=4 --optimize-for-size";
#[cfg(not(target_pointer_width = "64"))]
const DEFAULT_V8_FLAGS: &str =
    "--max-old-space-size=1024 --max-semi-space-size=4 --optimize-for-size";

fn effective_v8_flags(user: Option<&str>) -> String {
    match normalize_v8_flags(user) {
        Some(u) => format!("{} {}", DEFAULT_V8_FLAGS, u),
        None => DEFAULT_V8_FLAGS.to_string(),
    }
}

fn load_persona(input: &str) -> anyhow::Result<obscura_net::EffectivePersona> {
    if let Some(profile) = obscura_net::StealthProfile::from_name(input) {
        return Ok(obscura_net::EffectivePersona::builtin(profile));
    }
    let path = std::path::Path::new(input);
    let json = std::fs::read_to_string(path)
        .map_err(|error| anyhow::anyhow!("failed to read persona {}: {}", path.display(), error))?;
    obscura_net::PersonaSpec::from_json(&json)?
        .compile()
        .map_err(Into::into)
}

fn resolve_persona(
    persona_input: Option<&str>,
    persona_json: Option<&str>,
) -> anyhow::Result<obscura_net::EffectivePersona> {
    if let Some(input) = persona_input {
        load_persona(input)
    } else if let Some(json) = persona_json {
        obscura_net::PersonaSpec::from_json(json)?.compile().map_err(Into::into)
    } else {
        anyhow::bail!(
            "a persona is required; pass --persona <preset-or-json> or set OBSCURA_PERSONA"
        )
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let verbose = args.verbose;

    let persona_input = args
        .persona
        .as_deref()
        .map(str::to_owned)
        .or_else(|| std::env::var("OBSCURA_PERSONA").ok());
    let persona_json = std::env::var("OBSCURA_PERSONA_JSON").ok();
    let persona = resolve_persona(persona_input.as_deref(), persona_json.as_deref())?;

    obscura_net::activate_process_persona(&persona)?;

    let quiet = is_quiet_command(&args.command);
    let filter = select_log_filter(args.verbose, quiet);
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(filter)),
        )
        .with_writer(std::io::stderr)
        .init();

    let v8_flags = effective_v8_flags(args.v8_flags.as_deref());
    tracing::debug!("V8 flags: {}", v8_flags);
    obscura_js::try_set_v8_flags(&v8_flags)?;

    // The js-side fetch path (op_fetch_url) reads OBSCURA_ALLOW_PRIVATE_NETWORK
    // directly for its SSRF gate. Mirror the CLI flag into the env var so
    // iframe loads and JS fetch() see the same policy the http_client layer
    // already uses (issue #33).
    if args.allow_private_network {
        // SAFETY: set_var is unsafe in newer rustc; this runs before any
        // spawned thread inspects the env, so it's effectively single
        // threaded at this point.
        unsafe {
            std::env::set_var("OBSCURA_ALLOW_PRIVATE_NETWORK", "1");
        }
    }

    let global_proxy = args.proxy.clone();
    let obey_robots = args.obey_robots;

    match args.command {
        Some(Command::Serve {
            port,
            host,
            allowed_hosts,
            allowed_origins,
            auth_token_file,
            advertise_websocket_url,
            allow_unauthenticated_remote,
            proxy,
            workers,
            max_connections,
            allow_file_access,
            storage_dir,
            font_dirs,
            supervised_worker,
            quiet: _,
        }) => {
            if workers > 1 && port == 0 {
                anyhow::bail!("serve --workers greater than 1 requires a nonzero --port");
            }
            // Fall back to OBSCURA_PROXY so a proxy can be supplied without
            // putting credentials on the command line. The multi-worker load
            // balancer passes the proxy to each worker this way (issue #366).
            let proxy = merge_proxy(global_proxy.clone(), proxy).or_else(|| {
                std::env::var("OBSCURA_PROXY")
                    .ok()
                    .filter(|s| !s.is_empty())
            });
            let bearer_token = load_cdp_access_token(auth_token_file.as_deref())?;
            let effective_allowed_hosts =
                effective_cdp_allowed_hosts(&host, port, allowed_hosts)?;
            let access = CdpServeAccess {
                allowed_hosts: effective_allowed_hosts,
                allowed_origins,
                bearer_token,
                advertised_websocket_url: advertise_websocket_url,
                allow_unauthenticated_remote,
            };
            access.options().validate_for_bind(&host, port)?;
            if workers > 1 && storage_dir.is_some() {
                anyhow::bail!(
                    "serve --storage-dir is not supported with --workers greater than 1; \
                     sharing one persistence directory across worker processes has no defined ownership"
                );
            }
            if supervised_worker.is_some() && workers > 1 {
                anyhow::bail!("--supervised-worker requires --workers 1");
            }
            if supervised_worker.is_some() && storage_dir.is_some() {
                anyhow::bail!("--supervised-worker does not support --storage-dir");
            }
            configure_font_directories(&font_dirs)?;
            if supervised_worker.is_none() {
                print_banner(&host, port);
            }
            if let Some(ref dir) = storage_dir {
                tracing::info!("Storage dir: {}", dir.display());
            }
            if let Some(ref proxy) = proxy {
                tracing::info!("Using proxy: {}", proxy);
            }
            for directory in &font_dirs {
                tracing::info!("Font dir: {}", directory.display());
            }
            tracing::info!("Chrome identity and primp transport enabled");

            if workers > 1 {
                tracing::info!("{} worker processes", workers);
                run_multi_worker_serve(
                    port,
                    host,
                    workers,
                    proxy,
                    font_dirs,
                    max_connections,
                    access,
                    persona.clone(),
                    allow_file_access,
                    args.allow_private_network,
                    v8_flags.clone(),
                    verbose,
                    quiet,
                )
                .await?;
            } else if let Some(worker_index) = supervised_worker {
                run_supervised_multi_worker_child(
                    worker_index,
                    port,
                    &host,
                    proxy,
                    allow_file_access,
                    args.allow_private_network,
                    max_connections,
                    access.options(),
                    persona.clone(),
                )
                .await?;
            } else {
                obscura_cdp::start_with_serve_options_access_and_limit(
                    port,
                    &host,
                    proxy,
                    allow_file_access,
                    storage_dir,
                    args.allow_private_network,
                    max_connections,
                    access.options(),
                    persona.clone(),
                )
                .await?;
            }
        }
        Some(Command::Fetch {
            url,
            dump,
            selector,
            wait,
            timeout,
            wait_until,
            eval,
            output,
            quiet,
            storage_dir,
            file,
            concurrency,
            screenshot,
        }) => {
            if let Some(file) = file {
                if url.is_some() {
                    anyhow::bail!("Pass URLs via a positional argument or --file, not both.");
                }
                if screenshot.is_some() {
                    anyhow::bail!("--screenshot is only supported for a single URL, not --file batch mode.");
                }
                // Batch mode is raw HTTP only. Rendering each URL through the
                // browser/JS stack is what `scrape` is for.
                match dump {
                    None | Some(DumpFormat::Original) => {}
                    Some(_) => anyhow::bail!(
                        "batch mode (--file) only supports --dump original. Use `scrape` for rendered/DOM output."
                    ),
                }
                let urls = read_urls_from_file(&file)?;
                run_batch_fetch(
                    urls,
                    concurrency.get(),
                    timeout,
                    global_proxy,
                    output,
                    quiet,
                    persona.clone(),
                )
                .await?;
            } else {
                let url = url.ok_or_else(|| {
                    anyhow::anyhow!(
                        "No URL provided. Pass a URL, or a list of URLs with --file <path>."
                    )
                })?;
                let wait_is_fixed = wait.is_some();
                run_fetch(
                    &url,
                    dump,
                    selector,
                    wait.unwrap_or(5),
                    wait_is_fixed,
                    timeout,
                    &wait_until,
                    eval,
                    output,
                    quiet,
                    global_proxy,
                    storage_dir,
                    args.allow_private_network,
                    obey_robots,
                    screenshot,
                    persona.clone(),
                )
                .await?;
            }
        }
        Some(Command::Scrape {
            urls,
            eval,
            concurrency,
            format,
            timeout,
            quiet,
        }) => {
            run_parallel_scrape(
                urls,
                eval,
                concurrency.get(),
                &format,
                timeout,
                quiet,
                global_proxy,
                obey_robots,
                persona.clone(),
                v8_flags.clone(),
            )
            .await?;
        }
        Some(Command::Mcp {
            storage_dir,
            http,
            host,
            port,
            proxy,
        }) => {
            let mcp_proxy = merge_proxy(global_proxy.clone(), proxy);
            if http {
                obscura_mcp::http::run(host, port, mcp_proxy, persona.clone(), storage_dir).await?;
            } else {
                obscura_mcp::run(mcp_proxy, persona.clone(), storage_dir).await?;
            }
        }
        None => {
            print_banner("127.0.0.1", args.port);
            if let Some(ref proxy) = args.proxy {
                tracing::info!("Using proxy: {}", proxy);
            }
            let access = obscura_cdp::CdpAccessOptions::new()
                .with_bearer_token(load_cdp_access_token(None)?);
            obscura_cdp::start_with_serve_options_access_and_limit(
                args.port,
                "127.0.0.1",
                args.proxy,
                false,
                None,
                false,
                obscura_cdp::DEFAULT_MAX_CONNECTIONS,
                access,
                persona,
            )
            .await?;
        }
    }

    Ok(())
}

const MULTI_WORKER_CONTROL_PROTOCOL: &str = "obscura-multi-worker-control";
const MULTI_WORKER_CONTROL_VERSION: u8 = 1;
const MULTI_WORKER_SHUTDOWN_COMMAND: &[u8] = b"shutdown\n";
const MULTI_WORKER_CONTROL_LINE_LIMIT: u64 = 4_096;
const MULTI_WORKER_STARTUP_TIMEOUT: Duration = Duration::from_secs(10);
const MULTI_WORKER_HEALTH_INTERVAL: Duration = Duration::from_millis(100);
const MULTI_WORKER_CHILD_GRACEFUL_TIMEOUT: Duration = Duration::from_secs(5);
const MULTI_WORKER_CHILD_KILL_TIMEOUT: Duration = Duration::from_secs(2);
const MULTI_WORKER_RELAY_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct MultiWorkerReadyRecord {
    protocol: String,
    version: u8,
    event: String,
    worker: u16,
    port: u16,
    pid: u32,
}

struct SupervisedWorker {
    index: u16,
    port: u16,
    pid: u32,
    child: tokio::process::Child,
    stdin: Option<tokio::process::ChildStdin>,
    stdout: BufReader<tokio::process::ChildStdout>,
    exit_status: Option<std::process::ExitStatus>,
    shutdown_requested: bool,
}

#[derive(Debug, Clone, Copy)]
enum MultiWorkerParentSignal {
    Interrupt,
    Terminate,
}

#[cfg(unix)]
struct MultiWorkerParentSignals {
    interrupt: tokio::signal::unix::Signal,
    terminate: tokio::signal::unix::Signal,
}

#[cfg(unix)]
impl MultiWorkerParentSignals {
    fn new() -> std::io::Result<Self> {
        use tokio::signal::unix::{signal, SignalKind};
        Ok(Self {
            interrupt: signal(SignalKind::interrupt())?,
            terminate: signal(SignalKind::terminate())?,
        })
    }

    async fn recv(&mut self) -> MultiWorkerParentSignal {
        tokio::select! {
            _ = self.interrupt.recv() => MultiWorkerParentSignal::Interrupt,
            _ = self.terminate.recv() => MultiWorkerParentSignal::Terminate,
        }
    }
}

#[cfg(windows)]
struct MultiWorkerParentSignals {
    ctrl_c: tokio::signal::windows::CtrlC,
}

#[cfg(windows)]
impl MultiWorkerParentSignals {
    fn new() -> std::io::Result<Self> {
        Ok(Self {
            ctrl_c: tokio::signal::windows::ctrl_c()?,
        })
    }

    async fn recv(&mut self) -> MultiWorkerParentSignal {
        let _ = self.ctrl_c.recv().await;
        MultiWorkerParentSignal::Interrupt
    }
}

async fn run_supervised_multi_worker_child(
    worker_index: u16,
    port: u16,
    host: &str,
    proxy: Option<String>,
    allow_file_access: bool,
    allow_private_network: bool,
    max_connections: usize,
    access: obscura_cdp::CdpAccessOptions,
    persona: obscura_net::EffectivePersona,
) -> anyhow::Result<()> {
    if host != "127.0.0.1" {
        anyhow::bail!("supervised workers must bind 127.0.0.1, got {host:?}");
    }
    let control = obscura_cdp::CdpServerControl::new();
    let stdin_control = control.clone();
    let stdin_task = tokio::spawn(async move {
        let mut stdin = BufReader::new(tokio::io::stdin());
        let mut command = Vec::new();
        let read = (&mut stdin)
            .take(MULTI_WORKER_CONTROL_LINE_LIMIT + 1)
            .read_until(b'\n', &mut command)
            .await;
        match read {
            Ok(0) => tracing::info!("multi-worker parent control pipe closed"),
            Ok(_) if command == MULTI_WORKER_SHUTDOWN_COMMAND => {
                tracing::info!("multi-worker parent requested shutdown")
            }
            Ok(_) => tracing::error!(
                raw = ?command,
                "invalid multi-worker parent control record ({} raw bytes)",
                command.len()
            ),
            Err(error) => tracing::error!("multi-worker parent control read failed: {error}"),
        }
        stdin_control.cancel();
    });

    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let server = obscura_cdp::start_with_serve_options_access_limit_and_control(
        port,
        host,
        proxy,
        allow_file_access,
        None,
        allow_private_network,
        max_connections,
        access,
        persona,
        control,
        ready_tx,
    );
    tokio::pin!(server);
    let ready = tokio::select! {
        result = &mut server => {
            stdin_task.abort();
            let _ = stdin_task.await;
            return match result {
                Ok(()) => Err(anyhow::anyhow!("supervised CDP worker exited before readiness")),
                Err(error) => Err(error),
            };
        }
        ready = ready_rx => ready.map_err(|_| anyhow::anyhow!(
            "supervised CDP worker lost readiness channel"
        ))?,
    };
    if ready.local_addr.port() != port {
        stdin_task.abort();
        let _ = stdin_task.await;
        anyhow::bail!(
            "supervised CDP worker bound unexpected port {} instead of {}",
            ready.local_addr.port(),
            port
        );
    }
    let record = MultiWorkerReadyRecord {
        protocol: MULTI_WORKER_CONTROL_PROTOCOL.to_string(),
        version: MULTI_WORKER_CONTROL_VERSION,
        event: "ready".to_string(),
        worker: worker_index,
        port,
        pid: std::process::id(),
    };
    let mut stdout = tokio::io::stdout();
    stdout
        .write_all(format!("{}\n", serde_json::to_string(&record)?).as_bytes())
        .await?;
    stdout.flush().await?;

    let result = server.await;
    stdin_task.abort();
    let _ = stdin_task.await;
    result
}

fn poll_supervised_worker_exit(
    worker: &mut SupervisedWorker,
) -> anyhow::Result<Option<std::process::ExitStatus>> {
    poll_supervised_process_exit(
        worker.index,
        worker.port,
        worker.pid,
        &mut worker.child,
        &mut worker.exit_status,
    )
}

fn poll_supervised_process_exit(
    worker_index: u16,
    worker_port: u16,
    worker_pid: u32,
    child: &mut tokio::process::Child,
    exit_status: &mut Option<std::process::ExitStatus>,
) -> anyhow::Result<Option<std::process::ExitStatus>> {
    if let Some(status) = *exit_status {
        return Ok(Some(status));
    }
    let status = child.try_wait().map_err(|error| {
        anyhow::anyhow!(
            "poll worker {} (pid {}, port {}): {}",
            worker_index,
            worker_pid,
            worker_port,
            error
        )
    })?;
    if let Some(status) = status {
        *exit_status = Some(status);
    }
    Ok(status)
}

fn unexpected_supervised_worker_exit(
    workers: &mut [SupervisedWorker],
) -> anyhow::Result<Option<anyhow::Error>> {
    for worker in workers {
        if let Some(status) = poll_supervised_worker_exit(worker)? {
            if !worker.shutdown_requested {
                return Ok(Some(anyhow::anyhow!(
                    "multi-worker child {} (pid {}, port {}) exited unexpectedly with {}",
                    worker.index,
                    worker.pid,
                    worker.port,
                    status
                )));
            }
        }
    }
    Ok(None)
}

fn validate_multi_worker_ready_record(
    line: &[u8],
    worker_index: u16,
    worker_port: u16,
    worker_pid: u32,
) -> anyhow::Result<()> {
    if line.len() as u64 > MULTI_WORKER_CONTROL_LINE_LIMIT || !line.ends_with(b"\n") {
        anyhow::bail!(
            "worker {} readiness record is incomplete or exceeds {} bytes",
            worker_index,
            MULTI_WORKER_CONTROL_LINE_LIMIT
        );
    }
    let record: MultiWorkerReadyRecord = serde_json::from_slice(line)
        .map_err(|error| anyhow::anyhow!("parse worker {} readiness record: {error}", worker_index))?;
    if record.protocol != MULTI_WORKER_CONTROL_PROTOCOL
        || record.version != MULTI_WORKER_CONTROL_VERSION
        || record.event != "ready"
        || record.worker != worker_index
        || record.port != worker_port
        || record.pid != worker_pid
    {
        anyhow::bail!(
            "worker {} readiness record mismatch: protocol={:?} version={} event={:?} worker={} port={} pid={} expected port={} pid={}",
            worker_index,
            record.protocol,
            record.version,
            record.event,
            record.worker,
            record.port,
            record.pid,
            worker_port,
            worker_pid
        );
    }
    Ok(())
}

async fn wait_for_multi_worker_readiness(
    workers: &mut [SupervisedWorker],
) -> anyhow::Result<()> {
    let deadline = tokio::time::Instant::now() + MULTI_WORKER_STARTUP_TIMEOUT;
    for worker in workers {
        let worker_index = worker.index;
        let worker_port = worker.port;
        let worker_pid = worker.pid;
        let (stdout, child, exit_status) = (
            &mut worker.stdout,
            &mut worker.child,
            &mut worker.exit_status,
        );
        let mut line = Vec::new();
        let mut limited = stdout.take(MULTI_WORKER_CONTROL_LINE_LIMIT + 1);
        let read = limited.read_until(b'\n', &mut line);
        tokio::pin!(read);
        let mut poll = tokio::time::interval(MULTI_WORKER_HEALTH_INTERVAL);
        poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                result = &mut read => {
                    let bytes = result.map_err(|error| anyhow::anyhow!(
                        "read worker {} readiness: {error}", worker_index
                    ))?;
                    if bytes == 0 {
                        let status = child.wait().await.map_err(|error| anyhow::anyhow!(
                            "wait worker {} (pid {}, port {}) after readiness EOF: {}",
                            worker_index,
                            worker_pid,
                            worker_port,
                            error
                        ))?;
                        *exit_status = Some(status);
                        anyhow::bail!(
                            "worker {} (pid {}, port {}) exited before readiness with {}",
                            worker_index,
                            worker_pid,
                            worker_port,
                            status
                        );
                    }
                    let mut stdout = tokio::io::stdout();
                    stdout.write_all(&line).await?;
                    stdout.flush().await?;
                    validate_multi_worker_ready_record(
                        &line,
                        worker_index,
                        worker_port,
                        worker_pid,
                    )?;
                    tracing::info!(
                        "Worker {} ready on port {} (pid {})",
                        worker_index,
                        worker_port,
                        worker_pid
                    );
                    break;
                }
                _ = poll.tick() => {
                    if let Some(status) = poll_supervised_process_exit(
                        worker_index,
                        worker_port,
                        worker_pid,
                        child,
                        exit_status,
                    )? {
                        anyhow::bail!(
                            "worker {} (pid {}, port {}) exited before readiness with {}",
                            worker_index,
                            worker_pid,
                            worker_port,
                            status
                        );
                    }
                }
                _ = tokio::time::sleep_until(deadline) => {
                    anyhow::bail!(
                        "worker {} (pid {}, port {}) did not become ready within {:?}",
                        worker_index,
                        worker_pid,
                        worker_port,
                        MULTI_WORKER_STARTUP_TIMEOUT
                    );
                }
            }
        }
    }
    Ok(())
}

async fn request_multi_worker_children_shutdown(
    workers: &mut [SupervisedWorker],
) -> Vec<String> {
    let mut errors = Vec::new();
    for worker in workers {
        worker.shutdown_requested = true;
        if worker.exit_status.is_some() {
            continue;
        }
        if let Some(mut stdin) = worker.stdin.take() {
            if let Err(error) = stdin.write_all(MULTI_WORKER_SHUTDOWN_COMMAND).await {
                errors.push(format!(
                    "write shutdown to worker {} (pid {}): {}",
                    worker.index, worker.pid, error
                ));
            } else if let Err(error) = stdin.flush().await {
                errors.push(format!(
                    "flush shutdown to worker {} (pid {}): {}",
                    worker.index, worker.pid, error
                ));
            }
            if let Err(error) = stdin.shutdown().await {
                errors.push(format!(
                    "close control pipe for worker {} (pid {}): {}",
                    worker.index, worker.pid, error
                ));
            }
        }
    }
    errors
}

async fn reap_multi_worker_children(workers: &mut [SupervisedWorker]) -> anyhow::Result<()> {
    let mut errors = Vec::new();
    let graceful_deadline =
        tokio::time::Instant::now() + MULTI_WORKER_CHILD_GRACEFUL_TIMEOUT;
    loop {
        let mut all_exited = true;
        for worker in workers.iter_mut() {
            match poll_supervised_worker_exit(worker) {
                Ok(Some(_)) => {}
                Ok(None) => all_exited = false,
                Err(error) => {
                    errors.push(error.to_string());
                    all_exited = false;
                }
            }
        }
        if all_exited || tokio::time::Instant::now() >= graceful_deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    for worker in workers.iter_mut().filter(|worker| worker.exit_status.is_none()) {
        tracing::warn!(
            "force-killing worker {} (pid {}) after {:?}",
            worker.index,
            worker.pid,
            MULTI_WORKER_CHILD_GRACEFUL_TIMEOUT
        );
        if let Err(error) = worker.child.start_kill() {
            errors.push(format!(
                "force-kill worker {} (pid {}): {}",
                worker.index, worker.pid, error
            ));
        }
    }

    let kill_deadline = tokio::time::Instant::now() + MULTI_WORKER_CHILD_KILL_TIMEOUT;
    for worker in workers.iter_mut().filter(|worker| worker.exit_status.is_none()) {
        match tokio::time::timeout_at(kill_deadline, worker.child.wait()).await {
            Ok(Ok(status)) => worker.exit_status = Some(status),
            Ok(Err(error)) => errors.push(format!(
                "wait worker {} (pid {}) after kill: {}",
                worker.index, worker.pid, error
            )),
            Err(_) => errors.push(format!(
                "worker {} (pid {}) was not reaped within {:?} after kill",
                worker.index, worker.pid, MULTI_WORKER_CHILD_KILL_TIMEOUT
            )),
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        anyhow::bail!("multi-worker child cleanup failed: {}", errors.join("; "))
    }
}

async fn finish_multi_worker_children(
    workers: &mut [SupervisedWorker],
) -> anyhow::Result<()> {
    let request_errors = request_multi_worker_children_shutdown(workers).await;
    let reap = reap_multi_worker_children(workers).await;
    match (request_errors.is_empty(), reap) {
        (true, result) => result,
        (false, Ok(())) => anyhow::bail!(
            "multi-worker child shutdown request failed: {}",
            request_errors.join("; ")
        ),
        (false, Err(error)) => anyhow::bail!(
            "multi-worker child shutdown request failed: {}; {}",
            request_errors.join("; "),
            error
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn configure_multi_worker_child_command(
    command: &mut std::process::Command,
    v8_flags: &str,
    verbose: bool,
    quiet: bool,
    worker_index: u16,
    worker_port: u16,
    max_connections: usize,
    allow_file_access: bool,
    allow_private_network: bool,
    persona_json: &str,
    proxy: Option<&str>,
    access: &CdpServeAccess,
    font_dirs: &[std::path::PathBuf],
) {
    command.arg("--v8-flags").arg(v8_flags);
    if verbose {
        command.arg("--verbose");
    }
    command
        .arg("serve")
        .arg("--host")
        .arg("127.0.0.1")
        .arg("--port")
        .arg(worker_port.to_string())
        .arg("--workers")
        .arg("1")
        .arg("--max-connections")
        .arg(max_connections.to_string())
        .arg("--supervised-worker")
        .arg(worker_index.to_string());
    if quiet {
        command.arg("--quiet");
    }
    if allow_file_access {
        command.arg("--allow-file-access");
    }
    if allow_private_network {
        command.arg("--allow-private-network");
    }
    command.env_remove("OBSCURA_PERSONA");
    command.env("OBSCURA_PERSONA_JSON", persona_json);
    match proxy {
        Some(proxy) => {
            // Credentials remain outside argv and are inherited only by the
            // worker process that needs them.
            command.env("OBSCURA_PROXY", proxy);
        }
        None => {
            command.env_remove("OBSCURA_PROXY");
        }
    }
    access.configure_worker(command);
    for directory in font_dirs {
        command.arg("--font-dir").arg(directory);
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_multi_worker_serve(
    port: u16,
    host: String,
    workers: u16,
    proxy: Option<String>,
    font_dirs: Vec<std::path::PathBuf>,
    max_connections: usize,
    access: CdpServeAccess,
    persona: obscura_net::EffectivePersona,
    allow_file_access: bool,
    allow_private_network: bool,
    v8_flags: String,
    verbose: bool,
    quiet: bool,
) -> anyhow::Result<()> {
    use tokio::net::TcpListener;

    let worker_ports = multi_worker_ports(port, workers)?;
    let relay_limit = multi_worker_relay_limit(workers, max_connections)?;
    let mut signals = MultiWorkerParentSignals::new()
        .map_err(|error| anyhow::anyhow!("install multi-worker signal handlers: {error}"))?;

    // Bind the public listener before any child exists. A public-port conflict
    // therefore cannot strand workers that the failing parent never reaps.
    let listener = TcpListener::bind((host.as_str(), port))
        .await
        .map_err(|error| anyhow::anyhow!("bind multi-worker listener {host}:{port}: {error}"))?;

    let exe = std::env::current_exe()?;
    let persona_json = serde_json::to_string(&persona.to_spec())?;
    let mut supervised = Vec::new();
    for (offset, worker_port) in worker_ports.iter().copied().enumerate() {
        let index = u16::try_from(offset + 1).expect("worker index comes from u16 count");
        let mut cmd = TokioCommand::new(&exe);
        cmd.kill_on_drop(true);
        configure_multi_worker_child_command(
            cmd.as_std_mut(),
            &v8_flags,
            verbose,
            quiet,
            index,
            worker_port,
            max_connections,
            allow_file_access,
            allow_private_network,
            &persona_json,
            proxy.as_deref(),
            &access,
            &font_dirs,
        );
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::inherit());

        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(error) => {
                let cleanup = finish_multi_worker_children(&mut supervised).await;
                return match cleanup {
                    Ok(()) => Err(anyhow::anyhow!(
                        "spawn multi-worker child {index} on port {worker_port}: {error}"
                    )),
                    Err(cleanup) => Err(anyhow::anyhow!(
                        "spawn multi-worker child {index} on port {worker_port}: {error}; cleanup: {cleanup}"
                    )),
                };
            }
        };
        let pid = match child.id() {
            Some(pid) => pid,
            None => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                let cleanup = finish_multi_worker_children(&mut supervised).await;
                let error = anyhow::anyhow!(
                    "spawned worker {index} on port {worker_port} has no process id"
                );
                return match cleanup {
                    Ok(()) => Err(error),
                    Err(cleanup) => Err(error.context(format!("child cleanup also failed: {cleanup}"))),
                };
            }
        };
        let stdin = match child.stdin.take() {
            Some(stdin) => stdin,
            None => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                let cleanup = finish_multi_worker_children(&mut supervised).await;
                let error = anyhow::anyhow!(
                    "spawned worker {index} (pid {pid}) has no control stdin"
                );
                return match cleanup {
                    Ok(()) => Err(error),
                    Err(cleanup) => Err(error.context(format!("child cleanup also failed: {cleanup}"))),
                };
            }
        };
        let stdout = match child.stdout.take() {
            Some(stdout) => stdout,
            None => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                let cleanup = finish_multi_worker_children(&mut supervised).await;
                let error = anyhow::anyhow!(
                    "spawned worker {index} (pid {pid}) has no readiness stdout"
                );
                return match cleanup {
                    Ok(()) => Err(error),
                    Err(cleanup) => Err(error.context(format!("child cleanup also failed: {cleanup}"))),
                };
            }
        };
        supervised.push(SupervisedWorker {
            index,
            port: worker_port,
            pid,
            child,
            stdin: Some(stdin),
            stdout: BufReader::new(stdout),
            exit_status: None,
            shutdown_requested: false,
        });
    }

    enum StartupOutcome {
        Ready(anyhow::Result<()>),
        Signal(MultiWorkerParentSignal),
    }
    let startup = {
        let readiness = wait_for_multi_worker_readiness(&mut supervised);
        tokio::pin!(readiness);
        tokio::select! {
            biased;
            signal = signals.recv() => StartupOutcome::Signal(signal),
            result = &mut readiness => StartupOutcome::Ready(result),
        }
    };
    match startup {
        StartupOutcome::Ready(Ok(())) => {}
        StartupOutcome::Ready(Err(error)) => {
            tracing::error!("multi-worker startup failed: {error}");
            let cleanup = finish_multi_worker_children(&mut supervised).await;
            return match cleanup {
                Ok(()) => Err(error),
                Err(cleanup) => Err(error.context(format!("child cleanup also failed: {cleanup}"))),
            };
        }
        StartupOutcome::Signal(signal) => {
            tracing::info!("multi-worker parent received {:?} during startup", signal);
            return finish_multi_worker_children(&mut supervised).await;
        }
    }

    tracing::info!(
        "Load balancer on {}:{}, {} ready workers, {} relay slots",
        host,
        port,
        workers,
        relay_limit
    );
    let worker_addrs = worker_ports
        .into_iter()
        .map(|worker_port| std::net::SocketAddr::from(([127, 0, 0, 1], worker_port)))
        .collect();
    let (relay_shutdown_tx, relay_shutdown_rx) = tokio::sync::oneshot::channel();
    let relay = run_multi_worker_relay_loop(
        listener,
        worker_addrs,
        relay_limit,
        relay_shutdown_rx,
    );
    tokio::pin!(relay);
    let mut health = tokio::time::interval(MULTI_WORKER_HEALTH_INTERVAL);
    health.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut primary_error = None;
    let mut relay_result = None;
    tokio::select! {
        biased;
        signal = signals.recv() => {
            tracing::info!("multi-worker parent received {:?}", signal);
        }
        result = &mut relay => {
            primary_error = Some(match &result {
                Ok(()) => anyhow::anyhow!("multi-worker relay loop stopped without a shutdown request"),
                Err(error) => anyhow::anyhow!(error.to_string()),
            });
            relay_result = Some(result);
        }
        error = async {
            loop {
                health.tick().await;
                match unexpected_supervised_worker_exit(&mut supervised) {
                    Ok(Some(error)) => break error,
                    Ok(None) => {}
                    Err(error) => break error,
                }
            }
        } => {
            primary_error = Some(error);
        }
    }

    let _ = relay_shutdown_tx.send(());
    let relay_result = match relay_result {
        Some(result) => result,
        None => relay.await,
    };
    let request_errors = request_multi_worker_children_shutdown(&mut supervised).await;
    let reap_result = reap_multi_worker_children(&mut supervised).await;
    let cleanup_error = match (request_errors.is_empty(), relay_result, reap_result) {
        (true, Ok(()), Ok(())) => None,
        (request_ok, relay, reap) => Some(anyhow::anyhow!(
            "multi-worker shutdown incomplete: request_errors={:?}; relay={:?}; children={:?}",
            if request_ok { Vec::<String>::new() } else { request_errors },
            relay.err().map(|error| error.to_string()),
            reap.err().map(|error| error.to_string())
        )),
    };
    match (primary_error, cleanup_error) {
        (None, None) => Ok(()),
        (Some(error), None) => Err(error),
        (None, Some(error)) => Err(error),
        (Some(error), Some(cleanup)) => Err(error.context(cleanup.to_string())),
    }
}

const MULTI_WORKER_CONNECT_TIMEOUT: Duration = Duration::from_secs(1);
const MULTI_WORKER_REJECTION_TIMEOUT: Duration = Duration::from_millis(100);
const MAX_MULTI_WORKER_REJECTIONS: usize = 16;
const MULTI_WORKER_RELAY_LIMIT_RESPONSE: &[u8] = b"HTTP/1.1 503 Service Unavailable\r\n\
Content-Length: 0\r\nConnection: close\r\n\
X-Obscura-Reason: max-relays\r\n\r\n";
const MULTI_WORKER_BAD_GATEWAY_RESPONSE: &[u8] = b"HTTP/1.1 502 Bad Gateway\r\n\
Content-Length: 0\r\nConnection: close\r\n\
X-Obscura-Reason: worker-unreachable\r\n\r\n";

fn multi_worker_ports(port: u16, workers: u16) -> anyhow::Result<Vec<u16>> {
    (1..=workers)
        .map(|offset| {
            port.checked_add(offset).ok_or_else(|| {
                anyhow::anyhow!(
                    "serve --workers {} requires ports through {}, beyond u16 range",
                    workers,
                    u32::from(port) + u32::from(workers)
                )
            })
        })
        .collect()
}

fn multi_worker_relay_limit(workers: u16, max_connections: usize) -> anyhow::Result<usize> {
    let limit = usize::from(workers)
        .checked_mul(max_connections)
        .ok_or_else(|| anyhow::anyhow!("multi-worker relay limit overflow"))?;
    if limit > tokio::sync::Semaphore::MAX_PERMITS {
        anyhow::bail!(
            "multi-worker relay limit {} exceeds runtime maximum {}",
            limit,
            tokio::sync::Semaphore::MAX_PERMITS
        );
    }
    Ok(limit)
}

async fn reject_multi_worker_client(
    mut client: tokio::net::TcpStream,
    response: &'static [u8],
    _permit: tokio::sync::OwnedSemaphorePermit,
) {
    let result = timeout(MULTI_WORKER_REJECTION_TIMEOUT, async {
        // Send first so a slow or oversized request head cannot consume the
        // whole rejection budget before the fixed response is written. Keep
        // the socket open after shutting down the write half and drain input
        // within the same total deadline. This gives the peer time to receive
        // the complete response without allowing an unbounded drain task.
        client.write_all(response).await?;
        client.flush().await?;
        client.shutdown().await?;
        let mut discard = [0u8; 4096];
        loop {
            if client.read(&mut discard).await? == 0 {
                return Ok::<(), std::io::Error>(());
            }
        }
    })
    .await;
    match result {
        Ok(Ok(())) => {}
        Ok(Err(error)) => tracing::debug!("failed to send relay refusal: {}", error),
        Err(_) => tracing::debug!(
            "timed out sending relay refusal after {:?}",
            MULTI_WORKER_REJECTION_TIMEOUT
        ),
    }
}

async fn relay_multi_worker_connection(
    mut client: tokio::net::TcpStream,
    peer_addr: std::net::SocketAddr,
    worker_addr: std::net::SocketAddr,
    permit: tokio::sync::OwnedSemaphorePermit,
) {
    let mut worker = match timeout(
        MULTI_WORKER_CONNECT_TIMEOUT,
        tokio::net::TcpStream::connect(worker_addr),
    )
    .await
    {
        Ok(Ok(worker)) => worker,
        Ok(Err(error)) => {
            tracing::warn!("worker {} unreachable for {}: {}", worker_addr, peer_addr, error);
            reject_multi_worker_client(client, MULTI_WORKER_BAD_GATEWAY_RESPONSE, permit).await;
            return;
        }
        Err(_) => {
            tracing::warn!(
                "worker {} connect timed out for {} after {:?}",
                worker_addr,
                peer_addr,
                MULTI_WORKER_CONNECT_TIMEOUT
            );
            reject_multi_worker_client(client, MULTI_WORKER_BAD_GATEWAY_RESPONSE, permit).await;
            return;
        }
    };
    let _permit = permit;

    match tokio::io::copy_bidirectional(&mut client, &mut worker).await {
        Ok((client_to_worker, worker_to_client)) => tracing::debug!(
            "relay {} <-> {} closed after {} client bytes and {} worker bytes",
            peer_addr,
            worker_addr,
            client_to_worker,
            worker_to_client
        ),
        Err(error) => tracing::debug!(
            "relay {} <-> {} ended with I/O error: {}",
            peer_addr,
            worker_addr,
            error
        ),
    }
}

fn multi_worker_task_error(
    result: Result<(), tokio::task::JoinError>,
) -> Option<anyhow::Error> {
    result.err().map(|error| {
        tracing::error!("multi-worker relay task failed: {}", error);
        anyhow::anyhow!("multi-worker relay task failed: {error}")
    })
}

async fn run_multi_worker_relay_loop(
    listener: tokio::net::TcpListener,
    worker_addrs: Vec<std::net::SocketAddr>,
    relay_limit: usize,
    mut shutdown: tokio::sync::oneshot::Receiver<()>,
) -> anyhow::Result<()> {
    if worker_addrs.is_empty() {
        anyhow::bail!("multi-worker relay requires at least one worker address");
    }

    let relay_slots = Arc::new(tokio::sync::Semaphore::new(relay_limit));
    let rejection_slots = Arc::new(tokio::sync::Semaphore::new(MAX_MULTI_WORKER_REJECTIONS));
    let mut tasks = tokio::task::JoinSet::new();
    let mut next_worker = 0usize;
    let mut terminal_error = None;

    'accept: loop {
        while let Some(result) = tasks.try_join_next() {
            if let Some(error) = multi_worker_task_error(result) {
                terminal_error = Some(error);
                break 'accept;
            }
        }

        tokio::select! {
            _ = &mut shutdown => break,
            accepted = listener.accept() => {
                let (client, peer_addr) = match accepted {
                    Ok(accepted) => accepted,
                    Err(error) => {
                        terminal_error = Some(anyhow::anyhow!(
                            "accept multi-worker client: {error}"
                        ));
                        break 'accept;
                    }
                };
                let permit = match relay_slots.clone().try_acquire_owned() {
                    Ok(permit) => permit,
                    Err(_) => {
                        tracing::warn!(
                            "refusing multi-worker relay: at aggregate relay limit ({})",
                            relay_limit
                        );
                        match rejection_slots.clone().try_acquire_owned() {
                            Ok(rejection_permit) => {
                                tasks.spawn(reject_multi_worker_client(
                                    client,
                                    MULTI_WORKER_RELAY_LIMIT_RESPONSE,
                                    rejection_permit,
                                ));
                            }
                            Err(_) => {
                                tracing::warn!(
                                    "dropping multi-worker relay refusal: {} rejection tasks already active",
                                    MAX_MULTI_WORKER_REJECTIONS
                                );
                            }
                        }
                        continue;
                    }
                };

                let worker_addr = worker_addrs[next_worker % worker_addrs.len()];
                next_worker = next_worker.wrapping_add(1);
                tracing::debug!("Routing {} to worker {}", peer_addr, worker_addr);
                tasks.spawn(relay_multi_worker_connection(
                    client,
                    peer_addr,
                    worker_addr,
                    permit,
                ));
            }
            Some(result) = tasks.join_next(), if !tasks.is_empty() => {
                if let Some(error) = multi_worker_task_error(result) {
                    terminal_error = Some(error);
                    break 'accept;
                }
            }
        }
    }

    drop(listener);
    if terminal_error.is_some() {
        tasks.abort_all();
    }
    if tasks.is_empty() {
        return match terminal_error {
            Some(error) => Err(error),
            None => Ok(()),
        };
    }

    let drain = tokio::time::sleep(MULTI_WORKER_RELAY_DRAIN_TIMEOUT);
    tokio::pin!(drain);
    loop {
        tokio::select! {
            result = tasks.join_next(), if !tasks.is_empty() => {
                if let Some(result) = result {
                    if let Err(error) = result {
                        if !error.is_cancelled() {
                            tracing::error!("multi-worker relay task failed: {}", error);
                            if terminal_error.is_none() {
                                terminal_error = Some(anyhow::anyhow!(
                                    "multi-worker relay task failed: {error}"
                                ));
                            }
                            tasks.abort_all();
                        }
                    }
                }
                if tasks.is_empty() {
                    return match terminal_error {
                        Some(error) => Err(error),
                        None => Ok(()),
                    };
                }
            }
            _ = &mut drain => {
                tracing::warn!(
                    "aborting {} multi-worker relay task(s) after {:?}",
                    tasks.len(),
                    MULTI_WORKER_RELAY_DRAIN_TIMEOUT
                );
                tasks.abort_all();
                while let Some(result) = tasks.join_next().await {
                    if let Err(error) = result {
                        if !error.is_cancelled() {
                            tracing::error!("multi-worker relay task failed during abort: {}", error);
                            if terminal_error.is_none() {
                                terminal_error = Some(anyhow::anyhow!(
                                    "multi-worker relay task failed during abort: {error}"
                                ));
                            }
                        }
                    }
                }
                return match terminal_error {
                    Some(error) => Err(error),
                    None => Ok(()),
                };
            }
        }
    }
}

async fn settle_page(page: &mut Page, wait_secs: u64, fixed: bool) {
    let wait_ms = wait_secs.saturating_mul(1000);
    if fixed {
        page.settle_for_duration(wait_ms).await;
    } else {
        page.settle(wait_ms).await;
    }
}

fn configure_fetch_navigation_timeout(page: &mut Page, timeout_secs: u64) {
    page.set_navigation_timeout(Duration::from_secs(timeout_secs));
}

async fn run_fetch(
    url_str: &str,
    dump: Option<DumpFormat>,
    selector: Option<String>,
    wait_secs: u64,
    wait_is_fixed: bool,
    timeout_secs: u64,
    wait_until: &str,
    eval: Option<String>,
    output: Option<std::path::PathBuf>,
    quiet: bool,
    proxy: Option<String>,
    storage_dir: Option<std::path::PathBuf>,
    allow_private_network: bool,
    obey_robots: bool,
    screenshot: Option<std::path::PathBuf>,
    persona: obscura_net::EffectivePersona,
) -> anyhow::Result<()> {
    // Whether the user explicitly passed --dump. With --eval also present this
    // decides whether we return the eval value or read the page after the
    // eval's async work settles (issue #248).
    let dump_specified = dump.is_some();
    let dump = dump.unwrap_or(DumpFormat::Html);

    // --dump original short-circuits the browser stack entirely: fetch the raw
    // response body via HTTP and stream the bytes verbatim. Useful for binary
    // payloads (images, fonts, …) and any non-HTML resource where parsing the
    // body through the DOM/JS layer would corrupt or discard data.
    if dump == DumpFormat::Original {
        let bytes = fetch_original_bytes(url_str, proxy, timeout_secs, &persona)
        .await?;
        write_or_print_bytes(&bytes, output.as_ref()).await?;
        return Ok(());
    }

    let context = BrowserContext::with_options(
        "fetch".to_string(),
        persona,
        obscura_browser::BrowserContextOptions {
            proxy_url: proxy,
            storage_dir: storage_dir.clone(),
            allow_private_network,
            obey_robots,
            ..Default::default()
        },
    );
    let context = Arc::new(context);
    let mut page = Page::new("fetch-page".to_string(), context.clone());
    // Keep the browser's end-to-end navigation ceiling aligned with the CLI
    // request deadline. Previously Page retained its independent 30s default,
    // so `fetch --timeout 50` could still fail after 30 seconds.
    configure_fetch_navigation_timeout(&mut page, timeout_secs);
    // A screenshot viewport is also the navigation viewport: responsive
    // frameworks must build the DOM for the same dimensions we later paint.
    // Previously page JS saw a randomized screen-sized innerWidth while the
    // screenshot used these values only at the final raster step.
    let screenshot_viewport = screenshot.as_ref().map(|_| {
        let width = std::env::var("OBSCURA_SHOT_W")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(1280.0);
        let height = std::env::var("OBSCURA_SHOT_H")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(720.0);
        (width, height)
    });
    if let Some(viewport) = screenshot_viewport {
        page.set_viewport(viewport);
    }

    let wait_condition = obscura_browser::lifecycle::WaitUntil::from_str(wait_until);

    if !quiet {
        eprintln!("Fetching {}...", url_str);
    }

    // The paired corpus opts into a truthful capture boundary: its read-only
    // evaluation runs after all settle passes and the final scroll reassert,
    // immediately before screenshot paint. Ordinary CLI evaluation retains
    // its existing evaluate-then-settle behavior when this private variable is
    // absent.
    let eval_at_capture_boundary = screenshot.is_some()
        && eval.is_some()
        && std::env::var("OBSCURA_SHOT_EVAL_AT_CAPTURE").is_ok_and(|value| value == "1");
    let controlled_scroll_request = screenshot.as_ref().and_then(|_| {
        let raw_y = std::env::var("OBSCURA_SHOT_SCROLL_Y").ok()?;
        let x = match std::env::var("OBSCURA_SHOT_SCROLL_X") {
            Ok(raw) => raw.parse::<f64>().ok().filter(|value| value.is_finite())?,
            Err(_) => 0.0,
        };
        let requested_y = if raw_y.eq_ignore_ascii_case("bottom") {
            "document.documentElement.scrollHeight".to_string()
        } else {
            raw_y
                .parse::<f64>()
                .ok()
                .filter(|value| value.is_finite())
                .map(|value| value.to_string())?
        };
        Some((x, requested_y))
    });

    // Process-level hard deadline. A synchronous hang inside a Rust op invoked
    // from page JS cannot be cancelled by tokio (there is no await to interrupt)
    // nor by the V8 watchdog (terminate_execution only unwinds JS bytecode, not
    // native Rust running beneath a V8->op call). As an absolute backstop so one
    // fetch can never wedge the worker, a daemon thread force-exits if the whole
    // operation overruns navigation + every configured settle pass + grace. A
    // normal fetch returns first and the process exits before this fires.
    {
        let settle_passes = if eval_at_capture_boundary {
            1 + u64::from(controlled_scroll_request.is_some())
        } else if eval.is_some() && (screenshot.is_some() || selector.is_some() || dump_specified) {
            2
        } else {
            1
        };
        let hard = fetch_process_hard_timeout(
            timeout_secs,
            wait_secs,
            settle_passes,
            selector.is_some(),
        );
        std::thread::spawn(move || {
            std::thread::sleep(hard);
            eprintln!(
                "obscura: hard timeout exceeded ({}s); forcing exit",
                hard.as_secs()
            );
            std::process::exit(124);
        });
    }

    match timeout(
        Duration::from_secs(timeout_secs),
        page.navigate_with_wait(url_str, wait_condition),
    )
    .await
    {
        Ok(result) => {
            result.map_err(|e| anyhow::anyhow!("Failed to navigate to {}: {}", url_str, e))?
        }
        Err(_) => anyhow::bail!(
            "Timed out navigating to {} after {}s",
            url_str,
            timeout_secs
        ),
    }

    if !quiet {
        eprintln!("Page loaded: {} - \"{}\"", page.url_string(), page.title);
    }

    // --wait is a post-load settle: drive the event loop so timers, async work,
    // and completion callbacks (e.g. testharness's add_completion_callback) run
    // before we read the page. Returns early once the loop is idle, so static
    // pages stay fast.
    settle_page(&mut page, wait_secs, wait_is_fixed).await;

    let mut deferred_eval_output = None;
    let initial_controlled_scroll = if eval_at_capture_boundary {
        controlled_scroll_request.as_ref().map(|(x, requested_y)| {
            page.evaluate(&format!(
                "(()=>{{\
                 const requestedX={x},requestedY={requested_y};\
                 const preInitial={{x:window.scrollX,y:window.scrollY}};\
                 window.scrollTo(requestedX,requestedY);\
                 return {{requested:{{x:requestedX,y:requestedY}},\
                 preInitialActual:preInitial,\
                 postInitialActual:{{x:window.scrollX,y:window.scrollY}},\
                 initialBehavior:'authored',\
                 initialPhase:'before-controlled-scroll-settle'}}\
                 }})()"
            ))
        })
    } else {
        None
    };
    if initial_controlled_scroll.is_some() {
        settle_page(&mut page, wait_secs, wait_is_fixed).await;
    }

    if !eval_at_capture_boundary {
        if let Some(ref expr) = eval {
            // Bound the eval by the same budget as navigation so a runaway
            // expression (infinite loop, never-settling sync work) cannot hang.
            let result = page.evaluate_with_timeout(expr, Duration::from_secs(timeout_secs));

            // A bare --eval (no --selector, --dump, or --screenshot) returns the
            // eval value directly, so synchronous expressions
            // (JSON.stringify, ...) are unchanged. Screenshot captures continue
            // below so an evaluation such as scrollTo() affects the painted
            // viewport instead of being silently ignored.
            if !dump_specified && selector.is_none() && screenshot.is_none() {
                let rendered = match result {
                    serde_json::Value::String(s) => s,
                    serde_json::Value::Null => "null".to_string(),
                    other => other.to_string(),
                };
                write_or_print(rendered, output.as_ref()).await?;
                context.save_cookies();
                return Ok(());
            }
            if screenshot.is_some() {
                deferred_eval_output = Some(result);
            }

            // --eval combined with --selector, --dump, and/or --screenshot
            // typically kicks off async work (a fetch promise, a timer, a scroll
            // listener) that writes the DOM. Drive the event loop again so that
            // work completes, then fall through to selector/capture/dump instead
            // of returning the still-pending eval value (issue #248).
            settle_page(&mut page, wait_secs, wait_is_fixed).await;
        }
    }

    if let Some(ref sel) = selector {
        let found = wait_for_selector(&mut page, sel, wait_secs).await?;
        if !found {
            eprintln!("Warning: selector '{}' not found after {}s", sel, wait_secs);
        }
    }

    // --screenshot renders the settled, optionally evaluated page to a PNG.
    // Requires the render feature; without it, page.screenshot is absent and
    // we report clearly.
    if let Some(ref path) = screenshot {
        #[cfg(feature = "render")]
        {
            let resource_deadline_ms = std::env::var("OBSCURA_RENDER_RESOURCE_DEADLINE_MS")
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(3_000);
            let _ = page
                .prepare_screenshot_resources(resource_deadline_ms)
                .await;
            // Default CSS-pixel viewport, matching the engine's innerWidth/Height.
            // OBSCURA_SHOT_W / OBSCURA_SHOT_H override it (e.g. a tall viewport to
            // capture below-the-fold content in one shot).
            let viewport = screenshot_viewport.unwrap_or((1280.0, 720.0));
            // Ordinary screenshots sample the live document timeline. The
            // comparison harness can request an exact instant (normally T=0)
            // so both engines paint the same animation frame.
            let requested_animation_sample = std::env::var("OBSCURA_SHOT_ANIMATION_TIME_MS")
                .ok()
                .map(|raw| {
                    let milliseconds = raw.parse::<f32>().map_err(|_| {
                        anyhow::anyhow!(
                            "OBSCURA_SHOT_ANIMATION_TIME_MS must be a finite non-negative number"
                        )
                    })?;
                    if !milliseconds.is_finite() || milliseconds < 0.0 {
                        anyhow::bail!(
                            "OBSCURA_SHOT_ANIMATION_TIME_MS must be a finite non-negative number"
                        );
                    }
                    Ok(obscura_browser::AnimationSampleTime { milliseconds })
                })
                .transpose()?;
            let capture_screenshot = |page: &Page| match requested_animation_sample {
                Some(sample) => page.screenshot_at_animation_time(viewport, sample),
                None => page.screenshot(viewport),
            };
            // The parity harness performs one throwaway paint in both engines
            // before observing image/font readiness. Obscura resolves retained
            // render resources during prepare/paint, so sampling first would
            // compare pre-paint Obscura state with post-load Chromium state.
            // Keep this private opt-in out of ordinary CLI screenshots.
            let warmup_capture =
                std::env::var("OBSCURA_SHOT_RESOURCE_WARMUP").is_ok_and(|value| value == "1");
            if warmup_capture {
                if capture_screenshot(&page).is_none() {
                    anyhow::bail!("resource warm-up screenshot failed: page has no DOM to render");
                }
                // Give completion callbacks one bounded task turn before the
                // capture-boundary evaluation reads resource state.
                page.settle(1).await;
            }
            // Paired renderer captures need a stable final coordinate after
            // the post-eval settle. Authored smooth scrolling and scroll
            // anchoring may legitimately move an earlier scrollTo while the
            // page changes above the viewport, so the comparison harness opts
            // into one instant reassertion at the actual capture boundary.
            // Ordinary CLI screenshots are unchanged when these private
            // capture-environment variables are absent.
            let controlled_scroll = controlled_scroll_request
                .as_ref()
                .map(|(x, requested_y)| {
                    page.evaluate(&format!(
                        "(()=>{{\
                         const requestedX={x},requestedY={requested_y};\
                         const preReassert={{x:window.scrollX,y:window.scrollY}};\
                         const root=document.documentElement;\
                         const previous=root?root.style.getPropertyValue('scroll-behavior'):'';\
                         const priority=root?root.style.getPropertyPriority('scroll-behavior'):'';\
                         if(root)root.style.setProperty('scroll-behavior','auto','important');\
                         window.scrollTo(requestedX,requestedY);\
                         if(root){{if(previous)root.style.setProperty('scroll-behavior',previous,priority);\
                         else root.style.removeProperty('scroll-behavior')}}\
                         return {{requested:{{x:requestedX,y:requestedY}},\
                         preReassertActual:preReassert,\
                         finalReassertActual:{{x:window.scrollX,y:window.scrollY}},\
                         behavior:'instant',\
                         phase:'immediately-before-capture-state-and-screenshot'}}\
                         }})()"
                    ))
                });
            if eval_at_capture_boundary {
                if let Some(ref expr) = eval {
                    deferred_eval_output =
                        Some(page.evaluate_with_timeout(expr, Duration::from_secs(timeout_secs)));
                }
            }
            let capture_state = deferred_eval_output.as_ref().map(|_| {
                page.evaluate(
                    "(()=>({\
                     scrollX:window.scrollX,scrollY:window.scrollY,\
                     innerWidth:window.innerWidth,innerHeight:window.innerHeight,\
                     scrollWidth:document.documentElement?document.documentElement.scrollWidth:0,\
                     scrollHeight:document.documentElement?document.documentElement.scrollHeight:0\
                     }))()",
                )
            });
            match capture_screenshot(&page) {
                Some(bytes) => std::fs::write(path, &bytes)?,
                None => anyhow::bail!("screenshot failed: page has no DOM to render"),
            }
            // A screenshot+eval command used to ignore the expression
            // completely. Emit both its value and a standard state sampled
            // after the post-eval settle so automation can record the exact
            // live viewport that was painted.
            if let Some(result) = deferred_eval_output {
                let mut controlled_scroll_report = controlled_scroll;
                if let (Some(report), Some(initial)) = (
                    controlled_scroll_report.as_mut(),
                    initial_controlled_scroll.as_ref(),
                ) {
                    if let (Some(report), Some(initial)) =
                        (report.as_object_mut(), initial.as_object())
                    {
                        for key in [
                            "preInitialActual",
                            "postInitialActual",
                            "initialBehavior",
                            "initialPhase",
                        ] {
                            if let Some(value) = initial.get(key) {
                                report.insert(key.to_string(), value.clone());
                            }
                        }
                    }
                }
                println!(
                    "{}",
                    serde_json::json!({
                        "evaluation": result,
                        "controlledScroll": controlled_scroll_report,
                        "resourceWarmup": {
                            "performed": warmup_capture,
                            "discardedShots": if warmup_capture { 1 } else { 0 },
                            "taskTurnMs": if warmup_capture { 1 } else { 0 },
                            "phase": "before-final-scroll-reassert-and-state-sample",
                        },
                        "captureState": capture_state.unwrap_or(serde_json::Value::Null),
                    })
                );
            }
            if !quiet {
                eprintln!(
                    "Screenshot written: {} ({} bytes)",
                    path.display(),
                    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
                );
            }
            context.save_cookies();
            return Ok(());
        }
        #[cfg(not(feature = "render"))]
        {
            anyhow::bail!(
                "--screenshot {} requires a build with the render feature (cargo build --features render)",
                path.display()
            );
        }
    }

    let rendered = match dump {
        DumpFormat::Html => dump_html(&page),
        DumpFormat::Text => dump_text(&mut page),
        DumpFormat::Links => dump_links(&page),
        DumpFormat::Markdown => dump_markdown(&mut page),
        DumpFormat::Assets => dump_assets(&page),
        DumpFormat::Cookies => dump_cookies(&page),
        // Handled above via the short-circuit branch; unreachable here.
        DumpFormat::Original => unreachable!("Original dump handled before page navigation"),
    };
    write_or_print(rendered, output.as_ref()).await?;

    // Save cookies to disk if storage_dir is configured
    context.save_cookies();

    Ok(())
}

async fn fetch_original_response(
    url_str: &str,
    proxy: Option<String>,
    timeout_secs: u64,
    persona: &obscura_net::EffectivePersona,
) -> anyhow::Result<obscura_net::Response> {
    let url = url::Url::parse(url_str)
        .map_err(|e| anyhow::anyhow!("Invalid URL '{}': {}", url_str, e))?;

    // `--dump original` uses the standard persona transport, including its
    // file loader for local URLs.
    let client = obscura_net::StealthHttpClient::with_proxy(
        Arc::new(obscura_net::CookieJar::new()),
        proxy.as_deref(),
        false,
        persona,
    );
    match timeout(Duration::from_secs(timeout_secs), client.fetch(&url)).await {
        Ok(Ok(resp)) => Ok(resp),
        Ok(Err(e)) => anyhow::bail!("Failed to fetch {}: {}", url_str, e),
        Err(_) => anyhow::bail!("Timed out fetching {} after {}s", url_str, timeout_secs),
    }
}

async fn fetch_original_bytes(
    url_str: &str,
    proxy: Option<String>,
    timeout_secs: u64,
    persona: &obscura_net::EffectivePersona,
) -> anyhow::Result<Vec<u8>> {
    Ok(
        fetch_original_response(url_str, proxy, timeout_secs, persona)
            .await?
            .body,
    )
}

/// Read newline-delimited URLs from `path` (or stdin when `path` is `-`).
/// Blank lines and `#` comments are dropped, and surrounding whitespace is
/// trimmed so a list copy-pasted with indentation still works.
fn read_urls_from_file(path: &std::path::Path) -> anyhow::Result<Vec<String>> {
    let content = if path == std::path::Path::new("-") {
        use std::io::Read;
        let mut s = String::new();
        std::io::stdin()
            .read_to_string(&mut s)
            .map_err(|e| anyhow::anyhow!("Failed to read URLs from stdin: {}", e))?;
        s
    } else {
        std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("Failed to read {}: {}", path.display(), e))?
    };

    Ok(content
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(String::from)
        .collect())
}

/// Batch raw fetch: run `--dump original` over many URLs concurrently and print
/// one JSON status line per URL (issue #349). This is the raw-resource-check
/// counterpart to `scrape`; it never renders, so there is no browser/JS cost
/// per URL. Output stays in input order regardless of completion order.
async fn run_batch_fetch(
    urls: Vec<String>,
    concurrency: usize,
    timeout_secs: u64,
    proxy: Option<String>,
    output: Option<std::path::PathBuf>,
    quiet: bool,
    persona: obscura_net::EffectivePersona,
) -> anyhow::Result<()> {
    let total = urls.len();
    if total == 0 {
        anyhow::bail!("No URLs to fetch (--file was empty).");
    }

    if !quiet {
        eprintln!(
            "Fetching {} URLs with {} concurrent request(s) (per-fetch timeout: {}s)...",
            total, concurrency, timeout_secs
        );
    }

    let start = Instant::now();
    let semaphore = Arc::new(tokio::sync::Semaphore::new(concurrency));
    let proxy = Arc::new(proxy);
    let persona = Arc::new(persona);

    let mut handles = Vec::with_capacity(total);
    for (i, url) in urls.into_iter().enumerate() {
        let sem = semaphore.clone();
        let proxy = proxy.clone();
        let persona = persona.clone();

        handles.push(tokio::spawn(async move {
            let _permit = sem.acquire().await.unwrap();
            let task_start = Instant::now();
            let result = fetch_original_response(
                &url,
                (*proxy).clone(),
                timeout_secs,
                &persona,
            )
            .await;
            let elapsed_ms = task_start.elapsed().as_millis();

            let line = match result {
                Ok(resp) => serde_json::json!({
                    "url": url,
                    "ok": (200..400).contains(&resp.status),
                    "status": resp.status,
                    "content_type": resp.headers.get("content-type").cloned().unwrap_or_default(),
                    "bytes": resp.body.len(),
                    "elapsed_ms": elapsed_ms,
                }),
                Err(e) => serde_json::json!({
                    "url": url,
                    "ok": false,
                    "error": e.to_string(),
                    "elapsed_ms": elapsed_ms,
                }),
            };
            (i, line)
        }));
    }

    let mut results: Vec<Option<serde_json::Value>> = vec![None; total];
    let mut failures = 0usize;
    for handle in handles {
        if let Ok((i, line)) = handle.await {
            if !line["ok"].as_bool().unwrap_or(false) {
                failures += 1;
            }
            results[i] = Some(line);
        } else {
            failures += 1;
        }
    }

    let mut out = String::new();
    for line in results.into_iter().flatten() {
        out.push_str(&serde_json::to_string(&line).unwrap_or_default());
        out.push('\n');
    }

    if let Some(path) = output {
        tokio::fs::write(&path, out.as_bytes())
            .await
            .map_err(|e| anyhow::anyhow!("Failed to write {}: {}", path.display(), e))?;
    } else {
        let mut stdout = tokio::io::stdout();
        stdout
            .write_all(out.as_bytes())
            .await
            .map_err(|e| anyhow::anyhow!("Failed to write to stdout: {}", e))?;
        stdout
            .flush()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to flush stdout: {}", e))?;
    }

    if !quiet {
        eprintln!(
            "Done: {} URLs in {:.1}s ({} ok, {} failed).",
            total,
            start.elapsed().as_secs_f64(),
            total - failures,
            failures
        );
    }

    Ok(())
}

async fn write_or_print(
    content: String,
    output: Option<&std::path::PathBuf>,
) -> anyhow::Result<()> {
    if let Some(path) = output {
        tokio::fs::write(path, content)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to write {}: {}", path.display(), e))?;
    } else {
        println!("{}", content);
    }
    Ok(())
}

async fn write_or_print_bytes(
    bytes: &[u8],
    output: Option<&std::path::PathBuf>,
) -> anyhow::Result<()> {
    if let Some(path) = output {
        tokio::fs::write(path, bytes)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to write {}: {}", path.display(), e))?;
    } else {
        // Write raw bytes to stdout — never println! (would append a newline
        // and break binary payloads like JPEG/PNG).
        let mut stdout = tokio::io::stdout();
        stdout
            .write_all(bytes)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to write to stdout: {}", e))?;
        stdout
            .flush()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to flush stdout: {}", e))?;
    }
    Ok(())
}

async fn wait_for_selector(
    page: &mut Page,
    selector: &str,
    timeout_secs: u64,
) -> anyhow::Result<bool> {
    let timeout = std::time::Duration::from_secs(timeout_secs);
    let deadline = std::time::Instant::now()
        .checked_add(timeout)
        .ok_or_else(|| anyhow::anyhow!("selector wait timeout is too large"))?;
    match page.wait_for_selector(selector, deadline).await {
        Ok(obscura_browser::AutomationWait::Matched(_)) => Ok(true),
        Ok(obscura_browser::AutomationWait::TimedOut) => Ok(false),
        Err(error) => Err(anyhow::anyhow!("selector wait failed: {error}")),
    }
}

fn fetch_process_hard_timeout(
    navigation_secs: u64,
    wait_secs: u64,
    settle_passes: u64,
    has_selector_wait: bool,
) -> Duration {
    let wait_passes = settle_passes.saturating_add(u64::from(has_selector_wait));
    Duration::from_secs(
        navigation_secs
            .saturating_add(wait_secs.saturating_mul(wait_passes))
            .saturating_add(10),
    )
}

fn dump_cookies(page: &Page) -> String {
    let cookies = page.context.cookie_jar.get_all_cookies();
    serde_json::to_string_pretty(&cookies).unwrap_or_else(|_| "[]".to_string())
}

fn dump_html(page: &Page) -> String {
    page.with_dom(|dom| {
        if let Ok(Some(html_node)) = dom.query_selector("html") {
            let html = dom.outer_html(html_node);
            format!("<!DOCTYPE html>\n{}", html)
        } else {
            let doc = dom.document();
            dom.inner_html(doc)
        }
    })
    .unwrap_or_default()
}

fn dump_text(page: &mut Page) -> String {
    page.with_dom(|dom| {
        if let Ok(Some(body)) = dom.query_selector("body") {
            let text = extract_readable_text(dom, body);
            text.trim().to_string()
        } else {
            String::new()
        }
    })
    .unwrap_or_default()
}

fn dump_markdown(page: &mut Page) -> String {
    let result = page.evaluate(obscura_browser::HTML_TO_MARKDOWN_JS);
    result.as_str().unwrap_or_default().to_string()
}

fn is_html_whitespace(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\x0C' | '\r' | ' ')
}

fn append_readable_text_segment(result: &mut String, pending_space: &mut bool, contents: &str) {
    let trimmed = contents.trim_matches(is_html_whitespace);
    if trimmed.is_empty() {
        if contents.chars().any(is_html_whitespace) {
            *pending_space = true;
        }
        return;
    }

    let begins_with_space = contents.chars().next().is_some_and(is_html_whitespace);
    let result_ends_with_space = result.chars().next_back().is_some_and(char::is_whitespace);
    if (*pending_space || begins_with_space) && !result.is_empty() && !result_ends_with_space {
        result.push(' ');
    }
    result.push_str(trimmed);
    *pending_space = contents.chars().next_back().is_some_and(is_html_whitespace);
}

fn extract_readable_text(dom: &obscura_dom::DomTree, node_id: obscura_dom::NodeId) -> String {
    use obscura_dom::NodeData;

    // Iterative DFS over an explicit work stack. A recursive walk overflowed the
    // call stack (a hard abort, not a catchable panic) on deeply nested pages,
    // taking down the process on `--dump text` (issue #362, the CLI counterpart
    // of the serialize/textContent paths made iterative in obscura-dom). A
    // `Newline` work item emits a block element's trailing newline after its
    // children, matching the old pre/post-recursion output exactly.
    enum Work {
        Visit(obscura_dom::NodeId),
        Newline,
    }

    // Defense-in-depth cap mirroring DomTree::descendants; never reached on a
    // valid tree since append_child / insert_before reject cycles.
    const MAX_NODES: usize = 5_000_000;

    let mut result = String::new();
    let mut pending_space = false;
    let mut stack: Vec<Work> = vec![Work::Visit(node_id)];
    let mut visited = 0usize;

    while let Some(work) = stack.pop() {
        let id = match work {
            Work::Newline => {
                result.push('\n');
                pending_space = false;
                continue;
            }
            Work::Visit(id) => id,
        };

        visited += 1;
        if visited > MAX_NODES {
            break;
        }

        let node = match dom.get_node(id) {
            Some(n) => n,
            None => continue,
        };

        match &node.data {
            NodeData::Text { contents } => {
                append_readable_text_segment(&mut result, &mut pending_space, contents);
            }
            NodeData::Element { name, .. } => {
                let tag = name.local.as_ref();

                // Boilerplate elements rarely contain content the user wants to
                // scrape — strip them so `--dump text` returns the article body
                // instead of menus, footers, and cookie banners.
                if matches!(
                    tag,
                    "script" | "style" | "nav" | "header" | "footer" | "aside"
                ) {
                    continue;
                }

                let is_block = matches!(
                    tag,
                    "div"
                        | "p"
                        | "h1"
                        | "h2"
                        | "h3"
                        | "h4"
                        | "h5"
                        | "h6"
                        | "li"
                        | "tr"
                        | "br"
                        | "hr"
                        | "blockquote"
                        | "pre"
                        | "section"
                        | "article"
                        | "header"
                        | "footer"
                        | "nav"
                        | "main"
                        | "aside"
                        | "figure"
                        | "figcaption"
                        | "table"
                        | "thead"
                        | "tbody"
                        | "tfoot"
                        | "dl"
                        | "dt"
                        | "dd"
                        | "ul"
                        | "ol"
                );

                if is_block {
                    result.push('\n');
                    pending_space = false;
                    // Processed after all children (stack is LIFO): the trailing newline.
                    stack.push(Work::Newline);
                }
                // Push children in reverse so they pop in document order.
                for child_id in dom.children(id).into_iter().rev() {
                    stack.push(Work::Visit(child_id));
                }
            }
            _ => {
                for child_id in dom.children(id).into_iter().rev() {
                    stack.push(Work::Visit(child_id));
                }
            }
        }
    }

    result
}

async fn run_parallel_scrape(
    urls: Vec<String>,
    eval: Option<String>,
    concurrency: usize,
    format: &str,
    timeout_secs: u64,
    quiet: bool,
    proxy: Option<String>,
    obey_robots: bool,
    persona: obscura_net::EffectivePersona,
    v8_flags: String,
) -> anyhow::Result<()> {
    let total = urls.len();
    let start = Instant::now();

    if total == 0 {
        anyhow::bail!("No URLs provided. Pass at least one URL to scrape.");
    }

    if !quiet {
        eprintln!(
            "Scraping {} URLs with {} concurrent workers (per-worker timeout: {}s)...",
            total, concurrency, timeout_secs
        );
    }

    let worker_name = if cfg!(windows) {
        "obscura-worker.exe"
    } else {
        "obscura-worker"
    };
    let worker_path = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join(worker_name)))
        .unwrap_or_else(|| std::path::PathBuf::from(worker_name));

    if !worker_path.exists() {
        anyhow::bail!(
            "Worker binary not found at {}. Build with: cargo build --release",
            worker_path.display()
        );
    }

    let semaphore = Arc::new(tokio::sync::Semaphore::new(concurrency));
    let eval = Arc::new(eval);
    let worker_path = Arc::new(worker_path);
    let worker_timeout = Duration::from_secs(timeout_secs);
    let read_timeout = Duration::from_secs(timeout_secs.min(30));
    let shutdown_timeout = Duration::from_secs(5);
    let persona_json = Arc::new(serde_json::to_string(&persona.to_spec())?);
    let v8_flags = Arc::new(v8_flags);

    let mut handles = Vec::new();

    for (i, url) in urls.into_iter().enumerate() {
        let sem = semaphore.clone();
        let eval = eval.clone();
        let worker_path = worker_path.clone();
        let proxy = proxy.clone();
        let persona_json = persona_json.clone();
        let v8_flags = v8_flags.clone();

        let handle = tokio::spawn(async move {
            let _permit = sem.acquire().await.unwrap();
            let task_start = Instant::now();

            let mut child = match TokioCommand::new(worker_path.as_ref())
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .env("OBSCURA_PROXY", proxy.as_deref().unwrap_or(""))
                .env("OBSCURA_OBEY_ROBOTS", if obey_robots { "1" } else { "" })
                .env("OBSCURA_PERSONA_JSON", persona_json.as_str())
                .env("OBSCURA_V8_FLAGS", v8_flags.as_str())
                .spawn()
            {
                Ok(c) => c,
                Err(e) => {
                    return serde_json::json!({
                        "url": url,
                        "error": format!("Failed to spawn worker: {}", e),
                        "time_ms": task_start.elapsed().as_millis(),
                    });
                }
            };

            let mut stdin = match child.stdin.take() {
                Some(stdin) => stdin,
                None => {
                    let _ = timeout(shutdown_timeout, child.kill()).await;
                    return serde_json::json!({
                        "url": url,
                        "error": "Failed to open worker stdin",
                        "time_ms": task_start.elapsed().as_millis(),
                    });
                }
            };
            let stdout = match child.stdout.take() {
                Some(stdout) => stdout,
                None => {
                    let _ = timeout(shutdown_timeout, child.kill()).await;
                    return serde_json::json!({
                        "url": url,
                        "error": "Failed to open worker stdout",
                        "time_ms": task_start.elapsed().as_millis(),
                    });
                }
            };
            let mut reader = BufReader::new(stdout);

            let worker_result: Result<serde_json::Value, String> =
                match timeout(worker_timeout, async {
                    let nav_cmd = serde_json::json!({"cmd": "navigate", "url": url});
                    let mut line = serde_json::to_string(&nav_cmd).unwrap();
                    line.push('\n');
                    if stdin.write_all(line.as_bytes()).await.is_err() {
                        return Err("Write failed".to_string());
                    }
                    if stdin.flush().await.is_err() {
                        return Err("Write failed".to_string());
                    }

                    let mut resp_line = String::new();
                    match timeout(read_timeout, reader.read_line(&mut resp_line)).await {
                        Ok(Ok(bytes)) if bytes > 0 => {}
                        Ok(Ok(_)) | Ok(Err(_)) => return Err("Read failed".to_string()),
                        Err(_) => return Err("timeout".to_string()),
                    };

                    let nav_resp: serde_json::Value = serde_json::from_str(resp_line.trim())
                        .unwrap_or(serde_json::json!({"ok": false}));

                    if !nav_resp["ok"].as_bool().unwrap_or(false) {
                        return Err(nav_resp["error"]
                            .as_str()
                            .unwrap_or("navigate failed")
                            .to_string());
                    }

                    let title = nav_resp["result"]["title"]
                        .as_str()
                        .unwrap_or("")
                        .to_string();

                    let eval_result = if let Some(ref expr) = *eval {
                        let eval_cmd = serde_json::json!({"cmd": "evaluate", "expression": expr});
                        let mut line = serde_json::to_string(&eval_cmd).unwrap();
                        line.push('\n');
                        if stdin.write_all(line.as_bytes()).await.is_err() {
                            return Err("Write failed".to_string());
                        }
                        if stdin.flush().await.is_err() {
                            return Err("Write failed".to_string());
                        }

                        let mut resp_line = String::new();
                        match timeout(read_timeout, reader.read_line(&mut resp_line)).await {
                            Ok(Ok(bytes)) if bytes > 0 => {
                                let resp: serde_json::Value =
                                    serde_json::from_str(resp_line.trim())
                                        .unwrap_or(serde_json::json!({"ok": false}));
                                resp["result"].clone()
                            }
                            Ok(Ok(_)) | Ok(Err(_)) => return Err("Read failed".to_string()),
                            Err(_) => return Err("timeout".to_string()),
                        }
                    } else {
                        serde_json::Value::Null
                    };

                    let shutdown_cmd = serde_json::json!({"cmd": "shutdown"});
                    let mut line = serde_json::to_string(&shutdown_cmd).unwrap();
                    line.push('\n');
                    let _ = stdin.write_all(line.as_bytes()).await;
                    let _ = stdin.flush().await;
                    let _ = timeout(shutdown_timeout, child.wait()).await;

                    Ok(serde_json::json!({
                        "url": url,
                        "title": title,
                        "eval": eval_result,
                        "time_ms": task_start.elapsed().as_millis(),
                        "worker": i,
                    }))
                })
                .await
                {
                    Ok(result) => result,
                    Err(_) => Err("timeout".to_string()),
                };

            match worker_result {
                Ok(result) => result,
                Err(error) => {
                    let _ = timeout(shutdown_timeout, child.kill()).await;
                    serde_json::json!({
                        "url": url,
                        "error": error,
                        "time_ms": task_start.elapsed().as_millis(),
                    })
                }
            }
        });

        handles.push(handle);
    }

    let mut results = Vec::new();
    for handle in handles {
        match handle.await {
            Ok(result) => results.push(result),
            Err(e) => results.push(serde_json::json!({"error": e.to_string()})),
        }
    }

    let total_time = start.elapsed();

    if format == "json" {
        let output = serde_json::json!({
            "total_urls": total,
            "concurrency": concurrency,
            "total_time_ms": total_time.as_millis(),
            "avg_time_ms": total_time.as_millis() as f64 / total as f64,
            "results": results,
        });
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        for r in &results {
            let url = r["url"].as_str().unwrap_or("?");
            let title = r["title"].as_str().unwrap_or("");
            let time = r["time_ms"].as_u64().unwrap_or(0);
            let eval = &r["eval"];
            if eval.is_null() {
                println!("{}ms\t{}\t{}", time, url, title);
            } else {
                println!("{}ms\t{}\t{}", time, url, eval);
            }
        }
        if !quiet {
            eprintln!(
                "\nTotal: {}ms for {} URLs ({} concurrent)",
                total_time.as_millis(),
                total,
                concurrency
            );
        }
    }

    Ok(())
}

fn dump_links(page: &Page) -> String {
    let base_url = page.url.clone();
    page.with_dom(|dom| {
        let mut rendered = Vec::new();
        let links = dom.query_selector_all("a").unwrap_or_default();
        for link_id in links {
            if let Some(node) = dom.get_node(link_id) {
                let href = node.get_attribute("href").unwrap_or_default().to_string();
                let text = dom.text_content(link_id);
                let text = text.trim();

                let full_url = if href.starts_with("http://") || href.starts_with("https://") {
                    href.clone()
                } else if let Some(ref base) = base_url {
                    base.join(&href)
                        .map(|u| u.to_string())
                        .unwrap_or(href.clone())
                } else {
                    href.clone()
                };

                if !full_url.is_empty() {
                    if text.is_empty() {
                        rendered.push(full_url);
                    } else {
                        rendered.push(format!("{}\t{}", full_url, text));
                    }
                }
            }
        }
        rendered.join("\n")
    })
    .unwrap_or_default()
}

/// Selectors paired with the attribute whose URL we extract and the
/// asset kind we surface. Order is stable so the output of
/// `--dump assets` is deterministic across runs.
const ASSET_SELECTORS: &[(&str, &str, &str)] = &[
    ("script[src]", "src", "script"),
    ("link[href]", "href", "link"),
    ("img[src]", "src", "image"),
    ("iframe[src]", "src", "iframe"),
    ("source[src]", "src", "media"),
    ("video[src]", "src", "video"),
    ("audio[src]", "src", "audio"),
    ("embed[src]", "src", "embed"),
    ("object[data]", "data", "object"),
];

/// Map a `<link>` element's `rel` token to a more specific asset
/// kind so consumers can filter (e.g. just stylesheets, just icons).
/// Unknown / missing `rel` falls back to a generic "link" so the
/// caller still sees the URL.
fn link_kind_from_rel(rel: &str) -> &'static str {
    match rel
        .split_ascii_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "stylesheet" => "stylesheet",
        "icon" | "shortcut" => "icon",
        "manifest" => "manifest",
        "preload" => "preload",
        "prefetch" => "prefetch",
        "modulepreload" => "modulepreload",
        "dns-prefetch" => "dns-prefetch",
        "preconnect" => "preconnect",
        "alternate" => "alternate",
        _ => "link",
    }
}

/// Resolve a raw `src`/`href`/`data` attribute against the page's
/// base URL. Mirrors `dump_links`'s logic so `--dump assets` and
/// `--dump links` agree on absolute-URL semantics.
fn resolve_asset_url(raw: &str, base_url: Option<&url::Url>) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        return Some(trimmed.to_string());
    }
    if let Some(base) = base_url {
        if let Ok(joined) = base.join(trimmed) {
            return Some(joined.to_string());
        }
    }
    Some(trimmed.to_string())
}

/// Walk the rendered DOM and emit one NDJSON line per discoverable
/// sub-resource. Pure over `DomTree`/`Url` so unit tests can drive
/// it from a fixture HTML without standing up a browser.
fn extract_assets(dom: &obscura_dom::DomTree, base_url: Option<&url::Url>) -> String {
    let mut out: Vec<String> = Vec::new();
    for (selector, attr, default_kind) in ASSET_SELECTORS {
        let nodes = dom.query_selector_all(selector).unwrap_or_default();
        for node_id in nodes {
            let Some(node) = dom.get_node(node_id) else {
                continue;
            };
            let raw = node.get_attribute(attr).unwrap_or_default().to_string();
            let Some(url) = resolve_asset_url(&raw, base_url) else {
                continue;
            };

            let kind = if *default_kind == "link" {
                let rel = node.get_attribute("rel").unwrap_or_default().to_string();
                link_kind_from_rel(&rel)
            } else {
                *default_kind
            };

            let record = serde_json::json!({
                "url": url,
                "type": kind,
            });
            out.push(record.to_string());
        }
    }
    out.join("\n")
}

fn dump_assets(page: &Page) -> String {
    let base_url = page.url.clone();
    let dom_ndjson = page
        .with_dom(|dom| extract_assets(dom, base_url.as_ref()))
        .unwrap_or_default();

    let mut lines: Vec<String> = dom_ndjson
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect();

    // URLs already listed from static DOM attributes, so a resource the script
    // fetches that the markup also references is not emitted twice.
    let mut seen: std::collections::HashSet<String> = lines
        .iter()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter_map(|v| v.get("url").and_then(|u| u.as_str()).map(|s| s.to_string()))
        .collect();

    // Resources pulled in by JS fetch()/XHR, which leave no static DOM tag
    // (issue #301).
    for url in page.fetched_urls() {
        if seen.insert(url.clone()) {
            lines.push(serde_json::json!({ "url": url, "type": "fetch" }).to_string());
        }
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::{
        configure_fetch_navigation_timeout, configure_multi_worker_child_command,
        effective_v8_flags, extract_assets,
        effective_cdp_allowed_hosts,
        extract_readable_text, fetch_original_bytes, fetch_process_hard_timeout,
        is_quiet_command, link_kind_from_rel,
        merge_proxy, multi_worker_ports, multi_worker_relay_limit, normalize_v8_flags,
        read_urls_from_file, resolve_asset_url, run_multi_worker_relay_loop, select_log_filter,
        resolve_persona, validate_multi_worker_ready_record, write_or_print,
        write_or_print_bytes, Args, CdpServeAccess, Command, DumpFormat, DEFAULT_V8_FLAGS,
        MULTI_WORKER_BAD_GATEWAY_RESPONSE, MULTI_WORKER_CONTROL_LINE_LIMIT,
        MULTI_WORKER_RELAY_LIMIT_RESPONSE,
    };
    use clap::Parser;
    use obscura_dom::parse_html;

    async fn spawn_echo_worker(
    ) -> (
        std::net::SocketAddr,
        tokio::sync::mpsc::UnboundedReceiver<&'static str>,
        tokio::task::JoinHandle<()>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        let (events_tx, events_rx) = tokio::sync::mpsc::unbounded_channel();
        let task = tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                let events_tx = events_tx.clone();
                events_tx.send("accepted").unwrap();
                tokio::spawn(async move {
                    let (mut read, mut write) = stream.into_split();
                    let _ = tokio::io::copy(&mut read, &mut write).await;
                    let _ = events_tx.send("closed");
                });
            }
        });
        (addr, events_rx, task)
    }

    async fn spawn_test_relay(
        worker_addr: std::net::SocketAddr,
        relay_limit: usize,
    ) -> (
        std::net::SocketAddr,
        tokio::sync::oneshot::Sender<()>,
        tokio::task::JoinHandle<anyhow::Result<()>>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(run_multi_worker_relay_loop(
            listener,
            vec![worker_addr],
            relay_limit,
            shutdown_rx,
        ));
        (addr, shutdown_tx, task)
    }

    async fn round_trip_and_close(addr: std::net::SocketAddr, payload: &[u8]) -> Vec<u8> {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        for chunk in payload.chunks(137) {
            stream.write_all(chunk).await.unwrap();
        }
        stream.shutdown().await.unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).await.unwrap();
        response
    }

    #[test]
    fn multi_worker_ports_reject_wraparound_before_spawn() {
        assert_eq!(multi_worker_ports(9_222, 2).unwrap(), [9_223, 9_224]);
        let error = multi_worker_ports(u16::MAX - 1, 2).unwrap_err();
        assert!(error.to_string().contains("beyond u16 range"));
    }

    #[test]
    fn multi_worker_relay_limit_is_aggregate_and_checked() {
        assert_eq!(multi_worker_relay_limit(3, 7).unwrap(), 21);
        assert_eq!(multi_worker_relay_limit(2, 0).unwrap(), 0);
        let error = multi_worker_relay_limit(2, usize::MAX).unwrap_err();
        assert!(error.to_string().contains("overflow"));
    }

    #[test]
    fn multi_worker_readiness_record_is_exact_and_bounded() {
        let valid = br#"{"protocol":"obscura-multi-worker-control","version":1,"event":"ready","worker":2,"port":9224,"pid":1234}
"#;
        validate_multi_worker_ready_record(valid, 2, 9_224, 1_234).unwrap();

        for invalid in [
            br#"{"protocol":"obscura-multi-worker-control","version":1,"event":"ready","worker":3,"port":9224,"pid":1234}
"#.as_slice(),
            br#"{"protocol":"obscura-multi-worker-control","version":1,"event":"ready","worker":2,"port":9225,"pid":1234}
"#.as_slice(),
            br#"{"protocol":"obscura-multi-worker-control","version":1,"event":"ready","worker":2,"port":9224,"pid":9999}
"#.as_slice(),
            br#"{"protocol":"obscura-multi-worker-control","version":1,"event":"ready","worker":2,"port":9224,"pid":1234,"extra":true}
"#.as_slice(),
            br#"{"protocol":"obscura-multi-worker-control","version":1,"event":"ready","worker":2,"port":9224,"pid":1234}"#.as_slice(),
        ] {
            assert!(validate_multi_worker_ready_record(invalid, 2, 9_224, 1_234).is_err());
        }

        let oversized = vec![b'x'; MULTI_WORKER_CONTROL_LINE_LIMIT as usize + 1];
        assert!(validate_multi_worker_ready_record(&oversized, 2, 9_224, 1_234).is_err());
    }

    #[test]
    fn multi_worker_child_command_propagates_runtime_options_without_secret_argv() {
        let access = CdpServeAccess {
            allowed_hosts: vec!["public.example:9443".into()],
            allowed_origins: vec!["https://console.example".into()],
            bearer_token: Some("complete-token-secret".into()),
            advertised_websocket_url: Some("wss://public.example/cdp".into()),
            allow_unauthenticated_remote: false,
        };
        let mut command = std::process::Command::new("obscura");
        configure_multi_worker_child_command(
            &mut command,
            "--max-old-space-size=1024 --expose-gc",
            true,
            true,
            2,
            9_224,
            7,
            true,
            true,
            r#"{"persona_id":"complete-persona"}"#,
            Some("http://user:complete-proxy-secret@gate.example:8080"),
            &access,
            &[std::path::PathBuf::from("/complete/fonts")],
        );

        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            args,
            [
                "--v8-flags",
                "--max-old-space-size=1024 --expose-gc",
                "--verbose",
                "serve",
                "--host",
                "127.0.0.1",
                "--port",
                "9224",
                "--workers",
                "1",
                "--max-connections",
                "7",
                "--supervised-worker",
                "2",
                "--quiet",
                "--allow-file-access",
                "--allow-private-network",
                "--allow-host",
                "public.example:9443",
                "--allow-origin",
                "https://console.example",
                "--advertise-websocket-url",
                "wss://public.example/cdp",
                "--font-dir",
                "/complete/fonts",
            ]
        );
        let argv = args.join(" ");
        assert!(!argv.contains("complete-token-secret"));
        assert!(!argv.contains("complete-proxy-secret"));

        let env = command
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect::<std::collections::HashMap<_, _>>();
        assert_eq!(env["OBSCURA_CDP_TOKEN"].as_deref(), Some("complete-token-secret"));
        assert_eq!(
            env["OBSCURA_PROXY"].as_deref(),
            Some("http://user:complete-proxy-secret@gate.example:8080")
        );
        assert_eq!(
            env["OBSCURA_PERSONA_JSON"].as_deref(),
            Some(r#"{"persona_id":"complete-persona"}"#)
        );
        assert_eq!(env["OBSCURA_PERSONA"], None);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn silent_client_does_not_block_following_byte_transparent_relay() {
        let (worker_addr, mut worker_events, worker_task) = spawn_echo_worker().await;
        let (relay_addr, relay_shutdown, relay_task) = spawn_test_relay(worker_addr, 2).await;

        let silent = tokio::net::TcpStream::connect(relay_addr).await.unwrap();
        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_secs(1), worker_events.recv())
                .await
                .unwrap(),
            Some("accepted")
        );

        let payload: Vec<u8> = (0..8_192)
            .map(|index| match index % 257 {
                0 => 0,
                1 => 0xff,
                _ => (index % 251) as u8,
            })
            .collect();
        let echoed = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            round_trip_and_close(relay_addr, &payload),
        )
        .await
        .expect("a silent first client must not stall the accept loop");
        assert_eq!(echoed, payload);

        drop(silent);
        relay_shutdown.send(()).unwrap();
        relay_task.await.unwrap().unwrap();
        worker_task.abort();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn relay_limit_is_explicit_and_recovers_after_release() {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let (worker_addr, mut worker_events, worker_task) = spawn_echo_worker().await;
        let (relay_addr, relay_shutdown, relay_task) = spawn_test_relay(worker_addr, 2).await;
        let first = tokio::net::TcpStream::connect(relay_addr).await.unwrap();
        let second = tokio::net::TcpStream::connect(relay_addr).await.unwrap();
        for _ in 0..2 {
            assert_eq!(
                tokio::time::timeout(std::time::Duration::from_secs(1), worker_events.recv())
                    .await
                    .unwrap(),
                Some("accepted")
            );
        }

        let mut request = b"GET /json/version HTTP/1.1\r\nHost: 127.0.0.1\r\nX-Fill: ".to_vec();
        request.extend(std::iter::repeat(b'x').take(6_000));
        request.extend_from_slice(b"\r\n\r\n");
        let mut refused = tokio::net::TcpStream::connect(relay_addr).await.unwrap();
        refused.write_all(&request).await.unwrap();
        let mut response = Vec::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            refused.read_to_end(&mut response),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(response, MULTI_WORKER_RELAY_LIMIT_RESPONSE);
        assert!(worker_events.try_recv().is_err());

        drop(first);
        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_secs(1), worker_events.recv())
                .await
                .unwrap(),
            Some("closed")
        );

        let recovery_deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(1);
        let recovered = loop {
            let response = tokio::time::timeout_at(
                recovery_deadline,
                round_trip_and_close(relay_addr, b"recovered\x00\xff"),
            )
            .await
            .expect("a released relay permit must admit a later client");
            if response == b"recovered\x00\xff" {
                break response;
            }
            assert_eq!(response, MULTI_WORKER_RELAY_LIMIT_RESPONSE);
            assert!(
                tokio::time::Instant::now() < recovery_deadline,
                "a released relay permit must admit a later client"
            );
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        };
        assert_eq!(recovered, b"recovered\x00\xff");

        drop(second);
        relay_shutdown.send(()).unwrap();
        relay_task.await.unwrap().unwrap();
        worker_task.abort();
    }

    #[tokio::test(flavor = "current_thread")]
    async fn unreachable_worker_returns_complete_502_and_releases_permit() {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let reservation = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap();
        let worker_addr = reservation.local_addr().unwrap();
        drop(reservation);
        let (relay_addr, relay_shutdown, relay_task) = spawn_test_relay(worker_addr, 1).await;

        let request = b"GET /json/version HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n";
        let mut first = tokio::net::TcpStream::connect(relay_addr).await.unwrap();
        first.write_all(request).await.unwrap();
        first.shutdown().await.unwrap();
        let mut response = Vec::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            first.read_to_end(&mut response),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(response, MULTI_WORKER_BAD_GATEWAY_RESPONSE);

        let worker = tokio::net::TcpListener::bind(worker_addr).await.unwrap();
        let worker_task = tokio::spawn(async move {
            let (stream, _) = worker.accept().await.unwrap();
            let (mut read, mut write) = stream.into_split();
            tokio::io::copy(&mut read, &mut write).await.unwrap();
        });
        let recovered = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            round_trip_and_close(relay_addr, b"after-502"),
        )
        .await
        .expect("worker connect failure must release the relay permit");
        assert_eq!(recovered, b"after-502");

        relay_shutdown.send(()).unwrap();
        relay_task.await.unwrap().unwrap();
        worker_task.abort();
    }

    #[test]
    fn startup_persona_is_required_and_compiled_before_work_begins() {
        assert!(resolve_persona(None, None)
            .unwrap_err()
            .to_string()
            .contains("persona is required"));

        let builtin = resolve_persona(Some("macos_chrome153"), None).unwrap();
        assert_eq!(builtin.profile(), obscura_net::StealthProfile::MacChrome153);

        let invalid = r#"{"schema_version":"99","persona_id":"bad","revision":"1","profile":"windows_chrome145"}"#;
        assert!(resolve_persona(None, Some(invalid)).is_err());
    }

    // Issue #117 — `--dump original` short-circuits the browser stack and
    // streams the raw response body verbatim, including for binary payloads.
    //
    // Two tests below pin the behaviour:
    //   1. clap accepts `--dump original` as a valid DumpFormat variant.
    //   2. `fetch_original_bytes` returns the exact bytes a `file://` URL
    //      points at (binary-safe round-trip — no UTF-8 decode, no trailing
    //      newline, no DOM mutation).
    //   3. `write_or_print_bytes` writes raw bytes to a file without the
    //      trailing newline that `println!` would add.
    #[test]
    fn parsed_fetch_dump_original_is_accepted_by_clap() {
        let args = Args::try_parse_from([
            "obscura",
            "fetch",
            "--dump",
            "original",
            "https://example.com/image.jpg",
        ])
        .expect("clap should accept --dump original");
        match args.command {
            Some(Command::Fetch { dump, .. }) => {
                assert_eq!(dump, Some(DumpFormat::Original));
            }
            _ => panic!("expected Fetch command"),
        }
    }

    // Issue #349 — batch mode: `fetch --file urls.txt --dump original
    // --concurrency N` with no positional URL.
    #[test]
    fn parsed_fetch_file_and_concurrency() {
        let args = Args::try_parse_from([
            "obscura",
            "fetch",
            "--file",
            "urls.txt",
            "--dump",
            "original",
            "--concurrency",
            "25",
        ])
        .expect("clap should accept --file with --concurrency and no positional URL");
        match args.command {
            Some(Command::Fetch {
                url,
                file,
                concurrency,
                dump,
                ..
            }) => {
                assert!(url.is_none());
                assert_eq!(file, Some(std::path::PathBuf::from("urls.txt")));
                assert_eq!(concurrency.get(), 25);
                assert_eq!(dump, Some(DumpFormat::Original));
            }
            _ => panic!("expected Fetch command"),
        }
    }

    #[test]
    fn concurrency_rejects_zero() {
        // NonZeroUsize means --concurrency 0 is a parse error, not a silent hang
        // on a zero-permit semaphore.
        let err =
            Args::try_parse_from(["obscura", "fetch", "--file", "u.txt", "--concurrency", "0"]);
        assert!(err.is_err());
    }

    #[test]
    fn removed_stealth_flag_is_rejected() {
        let error = Args::try_parse_from([
            "obscura",
            "--stealth",
            "fetch",
            "https://example.com",
        ]);
        assert!(error.is_err(), "stealth is an invariant, not a runtime option");
    }

    #[test]
    fn user_agent_is_not_a_cli_parameter() {
        for args in [
            vec!["obscura", "--user-agent", "Custom/1.0", "fetch", "https://example.com"],
            vec!["obscura", "fetch", "--user-agent", "Custom/1.0", "https://example.com"],
        ] {
            assert!(Args::try_parse_from(args).is_err());
        }
    }

    #[test]
    fn serve_access_flags_and_loopback_hosts_are_explicit() {
        let args = Args::try_parse_from([
            "obscura",
            "serve",
            "--allow-host",
            "cdp.example.test:443",
            "--allow-origin",
            "https://cdp.example.test",
            "--auth-token-file",
            "/run/secrets/cdp-token",
            "--advertise-websocket-url",
            "wss://cdp.example.test",
        ])
        .unwrap();
        let Some(Command::Serve {
            allowed_hosts,
            allowed_origins,
            auth_token_file,
            advertise_websocket_url,
            ..
        }) = args.command
        else {
            panic!("expected serve command");
        };
        assert_eq!(allowed_hosts, ["cdp.example.test:443"]);
        assert_eq!(allowed_origins, ["https://cdp.example.test"]);
        assert_eq!(
            auth_token_file.unwrap(),
            std::path::PathBuf::from("/run/secrets/cdp-token")
        );
        assert_eq!(
            advertise_websocket_url.as_deref(),
            Some("wss://cdp.example.test")
        );

        assert_eq!(
            effective_cdp_allowed_hosts("127.0.0.1", 9222, Vec::new()).unwrap(),
            ["127.0.0.1:9222", "localhost:9222"]
        );
        assert!(
            effective_cdp_allowed_hosts("0.0.0.0", 9222, Vec::new())
                .unwrap()
                .is_empty()
        );
        assert!(
            effective_cdp_allowed_hosts("127.0.0.1", 0, Vec::new())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn read_urls_skips_blanks_and_comments() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("obscura_urls_{}.txt", std::process::id()));
        std::fs::write(
            &path,
            "https://a.example/one.js\n\n  # a comment\n   https://b.example/two.css  \nhttps://c.example/three.json\n",
        )
        .unwrap();
        let urls = read_urls_from_file(&path).unwrap();
        std::fs::remove_file(&path).ok();
        assert_eq!(
            urls,
            vec![
                "https://a.example/one.js".to_string(),
                "https://b.example/two.css".to_string(),
                "https://c.example/three.json".to_string(),
            ]
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn fetch_original_bytes_returns_file_contents_verbatim() {
        // A real binary payload: a 1×1 transparent PNG (89 50 4E 47 …) —
        // exactly the kind of resource #117 wants to stream without HTML/
        // JS rendering.
        const PNG_BYTES: &[u8] = &[
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00,
            0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78,
            0x9C, 0x63, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00,
            0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
        ];

        let path = std::env::temp_dir().join(format!(
            "obscura-fetch-original-test-{}.png",
            std::process::id()
        ));
        let _ = tokio::fs::remove_file(&path).await;
        tokio::fs::write(&path, PNG_BYTES)
            .await
            .expect("seed temp PNG fixture");

        let file_url = format!("file://{}", path.display());
        let bytes = fetch_original_bytes(
            &file_url,
            None,
            5,
            &obscura_net::EffectivePersona::builtin(
                obscura_net::StealthProfile::WindowsChrome145,
            ),
        )
            .await
            .expect("fetch_original_bytes should round-trip the file body");

        let _ = tokio::fs::remove_file(&path).await;

        assert_eq!(
            bytes, PNG_BYTES,
            "raw response body must match the file byte-for-byte"
        );
    }

    // `--dump original` routes network URLs through primp, which only speaks
    // http(s). file:// must remain on the file-capable policy client (#482).
    #[tokio::test(flavor = "current_thread")]
    async fn fetch_original_bytes_file_url_bypasses_network_transport() {
        const PNG_BYTES: &[u8] = &[
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00,
            0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78,
            0x9C, 0x63, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00,
            0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
        ];

        let path = std::env::temp_dir().join(format!(
            "obscura-fetch-original-file-test-{}.png",
            std::process::id()
        ));
        let _ = tokio::fs::remove_file(&path).await;
        tokio::fs::write(&path, PNG_BYTES)
            .await
            .expect("seed temp PNG fixture");

        let file_url = format!("file://{}", path.display());
        let bytes = fetch_original_bytes(
            &file_url,
            None,
            5,
            &obscura_net::EffectivePersona::builtin(
                obscura_net::StealthProfile::WindowsChrome145,
            ),
        )
            .await
            .expect("fetch_original_bytes should round-trip file://");

        let _ = tokio::fs::remove_file(&path).await;

        assert_eq!(bytes, PNG_BYTES, "file:// must not be sent to primp");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn write_or_print_bytes_writes_without_trailing_newline() {
        // Regression guard for #117: stdout must receive raw bytes. The file
        // path used here exercises the file-output branch — println!-style
        // output (used by write_or_print) would append a 0x0A byte and
        // corrupt binary payloads. write_or_print_bytes must not.
        let payload: &[u8] = &[0x00, 0xFF, b'h', b'i', 0x00];
        let path = std::env::temp_dir().join(format!(
            "obscura-write-bytes-test-{}.bin",
            std::process::id()
        ));
        let _ = tokio::fs::remove_file(&path).await;

        write_or_print_bytes(payload, Some(&path))
            .await
            .expect("write_or_print_bytes should write the file");

        let read_back = tokio::fs::read(&path).await.expect("read back");
        let _ = tokio::fs::remove_file(&path).await;

        assert_eq!(
            read_back, payload,
            "file bytes must match the payload exactly"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn write_or_print_writes_output_file_with_tokio_fs() {
        let path = std::env::temp_dir().join(format!(
            "obscura-fetch-output-test-{}.txt",
            std::process::id()
        ));
        let _ = tokio::fs::remove_file(&path).await;

        write_or_print("rendered output".to_string(), Some(&path))
            .await
            .expect("write output file");

        let content = tokio::fs::read_to_string(&path)
            .await
            .expect("read output file");
        let _ = tokio::fs::remove_file(&path).await;

        assert_eq!(content, "rendered output");
    }

    #[test]
    fn default_filter_is_warn() {
        assert_eq!(select_log_filter(false, false), "warn");
    }

    #[test]
    fn verbose_filter_is_debug() {
        assert_eq!(select_log_filter(true, false), "debug");
    }

    #[test]
    fn quiet_filter_is_off() {
        assert_eq!(select_log_filter(false, true), "off");
    }

    #[test]
    fn verbose_wins_over_quiet() {
        assert_eq!(select_log_filter(true, true), "debug");
    }

    #[test]
    fn parsed_fetch_with_quiet_flag_is_detected() {
        let args = Args::try_parse_from(["obscura", "fetch", "--quiet", "https://example.com"])
            .expect("clap should accept --quiet on fetch");
        assert!(is_quiet_command(&args.command));
    }

    #[test]
    fn parsed_fetch_without_quiet_is_not_detected() {
        let args = Args::try_parse_from(["obscura", "fetch", "https://example.com"])
            .expect("clap should accept fetch without --quiet");
        assert!(!is_quiet_command(&args.command));
    }

    #[test]
    fn parsed_serve_command_is_not_quiet() {
        let args = Args::try_parse_from(["obscura", "serve"]).expect("clap should accept serve");
        assert!(!is_quiet_command(&args.command));
    }

    #[test]
    fn parsed_serve_accepts_repeated_font_directories() {
        let args = Args::try_parse_from([
            "obscura",
            "serve",
            "--font-dir",
            "/fonts/cjk",
            "--font-dir",
            "/fonts/brand",
        ])
        .expect("clap should accept repeatable --font-dir");
        match args.command {
            Some(Command::Serve { font_dirs, .. }) => assert_eq!(
                font_dirs,
                [
                    std::path::PathBuf::from("/fonts/cjk"),
                    std::path::PathBuf::from("/fonts/brand"),
                ]
            ),
            _ => panic!("expected Serve command"),
        }
    }

    #[test]
    fn no_subcommand_is_not_quiet() {
        assert!(!is_quiet_command(&None));
    }

    #[test]
    fn parsed_v8_flags_global_arg() {
        let args = Args::try_parse_from([
            "obscura",
            "--v8-flags",
            "--max-old-space-size=4096 --max-semi-space-size=64",
            "fetch",
            "https://example.com",
        ])
        .expect("clap should accept --v8-flags as a global arg");
        assert_eq!(
            args.v8_flags.as_deref(),
            Some("--max-old-space-size=4096 --max-semi-space-size=64"),
        );
    }

    #[test]
    fn v8_flags_default_is_none() {
        let args = Args::try_parse_from(["obscura", "fetch", "https://example.com"])
            .expect("clap should accept fetch without --v8-flags");
        assert!(args.v8_flags.is_none());
    }

    #[test]
    fn parsed_v8_flags_with_serve_subcommand() {
        let args = Args::try_parse_from([
            "obscura",
            "--v8-flags",
            "--max-old-space-size=2048",
            "serve",
            "--port",
            "9333",
        ])
        .expect("clap should accept --v8-flags with serve");
        assert_eq!(args.v8_flags.as_deref(), Some("--max-old-space-size=2048"));
    }

    #[test]
    fn parsed_v8_flags_with_scrape_subcommand() {
        let args = Args::try_parse_from([
            "obscura",
            "--v8-flags",
            "--expose-gc",
            "scrape",
            "https://a.com",
            "https://b.com",
        ])
        .expect("clap should accept --v8-flags with scrape");
        assert_eq!(args.v8_flags.as_deref(), Some("--expose-gc"));
    }

    #[test]
    fn parsed_v8_flags_empty_string_is_accepted() {
        let args =
            Args::try_parse_from(["obscura", "--v8-flags", "", "fetch", "https://example.com"])
                .expect("clap should accept empty --v8-flags value");
        assert_eq!(args.v8_flags.as_deref(), Some(""));
    }

    #[test]
    fn normalize_v8_flags_returns_none_when_unset() {
        assert_eq!(normalize_v8_flags(None), None);
    }

    #[test]
    fn normalize_v8_flags_returns_none_for_empty_or_whitespace() {
        assert_eq!(normalize_v8_flags(Some("")), None);
        assert_eq!(normalize_v8_flags(Some("   ")), None);
        assert_eq!(normalize_v8_flags(Some("\t\n")), None);
    }

    #[test]
    fn normalize_v8_flags_trims_surrounding_whitespace() {
        assert_eq!(
            normalize_v8_flags(Some("  --max-old-space-size=4096  ")).as_deref(),
            Some("--max-old-space-size=4096"),
        );
    }

    #[test]
    fn normalize_v8_flags_preserves_multi_flag_string() {
        let input = "--max-old-space-size=4096 --max-semi-space-size=64 --expose-gc";
        assert_eq!(normalize_v8_flags(Some(input)).as_deref(), Some(input));
    }

    #[test]
    fn effective_v8_flags_returns_default_when_unset() {
        assert_eq!(effective_v8_flags(None), DEFAULT_V8_FLAGS);
        assert_eq!(effective_v8_flags(Some("")), DEFAULT_V8_FLAGS);
        assert_eq!(effective_v8_flags(Some("   ")), DEFAULT_V8_FLAGS);
    }

    #[test]
    fn effective_v8_flags_user_overrides_default() {
        // V8 parses left-to-right and later wins, so the user value must
        // come after the default in the merged string.
        let user = "--max-old-space-size=8192";
        let merged = effective_v8_flags(Some(user));
        assert!(merged.starts_with(DEFAULT_V8_FLAGS));
        assert!(merged.ends_with(user));
    }

    #[test]
    fn effective_v8_flags_appends_user_extras() {
        let merged = effective_v8_flags(Some("--expose-gc"));
        assert!(merged.contains(DEFAULT_V8_FLAGS));
        assert!(merged.contains("--expose-gc"));
    }

    #[test]
    fn parsed_fetch_quiet_resolves_to_off_filter() {
        let args =
            Args::try_parse_from(["obscura", "fetch", "--quiet", "https://example.com"]).unwrap();
        let filter = select_log_filter(args.verbose, is_quiet_command(&args.command));
        assert_eq!(filter, "off");
    }

    #[test]
    fn fetch_wait_distinguishes_adaptive_default_from_fixed_delay() {
        let default = Args::try_parse_from(["obscura", "fetch", "https://example.com"]).unwrap();
        match default.command {
            Some(Command::Fetch { wait, .. }) => assert_eq!(wait, None),
            _ => panic!("expected Fetch command"),
        }

        let fixed =
            Args::try_parse_from(["obscura", "fetch", "https://example.com", "--wait", "0"])
                .unwrap();
        match fixed.command {
            Some(Command::Fetch { wait, .. }) => assert_eq!(wait, Some(0)),
            _ => panic!("expected Fetch command"),
        }
    }

    #[test]
    fn fetch_screenshot_has_a_short_alias_and_rejects_batch_mode() {
        let args = Args::try_parse_from([
            "obscura",
            "fetch",
            "https://example.com",
            "-s",
            "page.png",
        ])
        .unwrap();
        match args.command {
            Some(Command::Fetch { screenshot, .. }) => {
                assert_eq!(screenshot, Some(std::path::PathBuf::from("page.png")));
            }
            _ => panic!("expected Fetch command"),
        }

        assert!(Args::try_parse_from([
            "obscura",
            "fetch",
            "--file",
            "urls.txt",
            "--screenshot",
            "page.png",
        ])
        .is_err());
    }

    fn configured_fetch_timeout(args: Args) -> std::time::Duration {
        let timeout = match args.command {
            Some(Command::Fetch { timeout, .. }) => timeout,
            _ => panic!("expected Fetch command"),
        };
        let context = std::sync::Arc::new(
            obscura_browser::BrowserContext::with_storage_and_network(
                "cli-timeout-test".to_string(),
                obscura_net::EffectivePersona::builtin(
                    obscura_net::StealthProfile::WindowsChrome145,
                ),
                None,
                None,
                true,
            ),
        );
        let mut page = obscura_browser::Page::new("cli-timeout-test".to_string(), context);
        configure_fetch_navigation_timeout(&mut page, timeout);
        page.navigation_timeout()
    }

    #[test]
    fn fetch_timeout_sets_the_page_navigation_budget() {
        let args = Args::try_parse_from([
            "obscura",
            "fetch",
            "https://example.com",
            "--timeout",
            "50",
        ])
        .unwrap();
        assert_eq!(
            configured_fetch_timeout(args),
            std::time::Duration::from_secs(50)
        );
    }

    #[test]
    fn fetch_default_navigation_budget_remains_thirty_seconds() {
        let args = Args::try_parse_from(["obscura", "fetch", "https://example.com"]).unwrap();
        assert_eq!(
            configured_fetch_timeout(args),
            std::time::Duration::from_secs(30)
        );
    }

    #[test]
    fn fetch_process_deadline_counts_selector_wait_separately() {
        assert_eq!(
            fetch_process_hard_timeout(30, 5, 2, true),
            std::time::Duration::from_secs(55),
        );
        assert_eq!(
            fetch_process_hard_timeout(30, 5, 2, false),
            std::time::Duration::from_secs(50),
        );
    }

    #[test]
    fn matcher_still_uses_fetch_variant() {
        let cmd = Some(Command::Fetch {
            url: Some("https://x".to_string()),
            dump: Some(super::DumpFormat::Html),
            selector: None,
            file: None,
            concurrency: std::num::NonZeroUsize::new(1).unwrap(),
            wait: Some(5),
            timeout: 30,
            wait_until: "load".to_string(),
            eval: None,
            quiet: true,
            output: None,
            storage_dir: None,
            screenshot: None,
        });
        assert!(is_quiet_command(&cmd));
    }

    fn body_text(html: &str) -> String {
        let dom = parse_html(html);
        let body = dom
            .query_selector("body")
            .ok()
            .flatten()
            .expect("body must exist");
        extract_readable_text(&dom, body)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn skips_nav_header_footer_aside() {
        let text = body_text(
            r#"<html><body>
                <header>SITE HEADER</header>
                <nav>NAV LINKS</nav>
                <aside>SIDEBAR</aside>
                <main><p>Article body.</p></main>
                <footer>FOOTER</footer>
            </body></html>"#,
        );
        assert!(text.contains("Article body."), "main content kept: {text}");
        for boilerplate in ["SITE HEADER", "NAV LINKS", "SIDEBAR", "FOOTER"] {
            assert!(
                !text.contains(boilerplate),
                "boilerplate '{boilerplate}' leaked through: {text}"
            );
        }
    }

    #[test]
    fn still_skips_script_and_style() {
        // Regression guard for the original skip list.
        let text = body_text(
            r#"<html><body>
                <p>Hello.</p>
                <script>console.log("nope")</script>
                <style>.x { color: red }</style>
            </body></html>"#,
        );
        assert!(text.contains("Hello."));
        assert!(!text.contains("console.log"));
        assert!(!text.contains("color: red"));
    }

    #[test]
    fn command_proxy_overrides_global_proxy() {
        let proxy = merge_proxy(
            Some("http://global.example:8080".to_string()),
            Some("socks5://127.0.0.1:1080".to_string()),
        );

        assert_eq!(proxy.as_deref(), Some("socks5://127.0.0.1:1080"));
    }

    #[test]
    fn global_proxy_is_used_when_command_proxy_is_absent() {
        let proxy = merge_proxy(Some("http://global.example:8080".to_string()), None);

        assert_eq!(proxy.as_deref(), Some("http://global.example:8080"));
    }

    #[test]
    fn parsed_fetch_dump_assets_is_accepted_by_clap() {
        let args = Args::try_parse_from([
            "obscura",
            "fetch",
            "--dump",
            "assets",
            "https://example.com",
        ])
        .expect("clap should accept --dump assets");
        match args.command {
            Some(Command::Fetch { dump, .. }) => {
                assert_eq!(dump, Some(DumpFormat::Assets));
            }
            _ => panic!("expected Fetch command"),
        }
    }

    #[test]
    fn resolve_asset_url_keeps_absolute_unchanged() {
        let base = url::Url::parse("https://page.test/a/b").unwrap();
        let abs = "https://cdn.test/x.js";
        assert_eq!(resolve_asset_url(abs, Some(&base)).as_deref(), Some(abs));
    }

    #[test]
    fn resolve_asset_url_joins_relative_against_base() {
        let base = url::Url::parse("https://page.test/a/b").unwrap();
        let rel = "/static/x.js";
        assert_eq!(
            resolve_asset_url(rel, Some(&base)).as_deref(),
            Some("https://page.test/static/x.js"),
        );
    }

    #[test]
    fn resolve_asset_url_drops_empty() {
        let base = url::Url::parse("https://page.test/").unwrap();
        assert!(resolve_asset_url("", Some(&base)).is_none());
        assert!(resolve_asset_url("   ", Some(&base)).is_none());
    }

    #[test]
    fn link_kind_from_rel_handles_common_values() {
        assert_eq!(link_kind_from_rel("stylesheet"), "stylesheet");
        assert_eq!(link_kind_from_rel("icon"), "icon");
        // First token wins for multi-token rel (e.g. "shortcut icon").
        assert_eq!(link_kind_from_rel("shortcut icon"), "icon");
        assert_eq!(link_kind_from_rel("manifest"), "manifest");
        assert_eq!(link_kind_from_rel("preload"), "preload");
        assert_eq!(link_kind_from_rel("prefetch"), "prefetch");
        assert_eq!(link_kind_from_rel("modulepreload"), "modulepreload");
        assert_eq!(link_kind_from_rel("dns-prefetch"), "dns-prefetch");
        assert_eq!(link_kind_from_rel("preconnect"), "preconnect");
        assert_eq!(link_kind_from_rel("alternate"), "alternate");
        // Empty / unknown falls back to generic "link" so URL is still emitted.
        assert_eq!(link_kind_from_rel(""), "link");
        assert_eq!(link_kind_from_rel("noopener"), "link");
    }

    #[test]
    fn extract_assets_covers_every_resource_tag() {
        let html = r#"<html><head>
            <link rel="stylesheet" href="/site.css">
            <link rel="icon" href="/favicon.ico">
            <link rel="preload" href="/font.woff2">
            <link href="/no-rel.css">
            <script src="/app.js"></script>
        </head><body>
            <img src="/logo.png">
            <iframe src="/frame.html"></iframe>
            <video src="/clip.mp4"><source src="/clip.webm"></video>
            <audio src="/track.mp3"></audio>
            <embed src="/widget.swf">
            <object data="/doc.pdf"></object>
        </body></html>"#;
        let dom = obscura_dom::parse_html(html);
        let base = url::Url::parse("https://example.test/page").unwrap();
        let ndjson = extract_assets(&dom, Some(&base));
        let records: Vec<serde_json::Value> = ndjson
            .lines()
            .map(|line| serde_json::from_str(line).expect("each line must be valid JSON"))
            .collect();

        // Every emitted record must have an absolute URL on example.test
        // and a non-empty type string. Pin specific entries so a regression
        // in selectors or kind mapping fails loudly.
        for r in &records {
            let url = r["url"].as_str().unwrap();
            assert!(
                url.starts_with("https://example.test/"),
                "url not absolute: {url}",
            );
            assert!(!r["type"].as_str().unwrap().is_empty());
        }

        let pairs: Vec<(String, String)> = records
            .iter()
            .map(|r| {
                (
                    r["url"].as_str().unwrap().to_string(),
                    r["type"].as_str().unwrap().to_string(),
                )
            })
            .collect();

        assert!(pairs.contains(&(
            "https://example.test/app.js".to_string(),
            "script".to_string(),
        )));
        assert!(pairs.contains(&(
            "https://example.test/site.css".to_string(),
            "stylesheet".to_string(),
        )));
        assert!(pairs.contains(&(
            "https://example.test/favicon.ico".to_string(),
            "icon".to_string(),
        )));
        assert!(pairs.contains(&(
            "https://example.test/font.woff2".to_string(),
            "preload".to_string(),
        )));
        assert!(pairs.contains(&(
            "https://example.test/no-rel.css".to_string(),
            "link".to_string(),
        )));
        assert!(pairs.contains(&(
            "https://example.test/logo.png".to_string(),
            "image".to_string(),
        )));
        assert!(pairs.contains(&(
            "https://example.test/frame.html".to_string(),
            "iframe".to_string(),
        )));
        assert!(pairs.contains(&(
            "https://example.test/clip.mp4".to_string(),
            "video".to_string(),
        )));
        assert!(pairs.contains(&(
            "https://example.test/clip.webm".to_string(),
            "media".to_string(),
        )));
        assert!(pairs.contains(&(
            "https://example.test/track.mp3".to_string(),
            "audio".to_string(),
        )));
        assert!(pairs.contains(&(
            "https://example.test/widget.swf".to_string(),
            "embed".to_string(),
        )));
        assert!(pairs.contains(&(
            "https://example.test/doc.pdf".to_string(),
            "object".to_string(),
        )));
    }

    #[test]
    fn extract_assets_skips_empty_attributes() {
        let html = r#"<html><body>
            <script src=""></script>
            <img src="   ">
            <iframe src="/ok.html"></iframe>
        </body></html>"#;
        let dom = obscura_dom::parse_html(html);
        let base = url::Url::parse("https://example.test/").unwrap();
        let ndjson = extract_assets(&dom, Some(&base));
        let lines: Vec<&str> = ndjson.lines().collect();
        // Only the iframe with a non-empty src survives.
        assert_eq!(lines.len(), 1, "got {lines:?}");
        assert!(lines[0].contains("\"https://example.test/ok.html\""));
        assert!(lines[0].contains("\"iframe\""));
    }
}
