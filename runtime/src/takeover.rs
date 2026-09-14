//! Bounded local transport; browser state stays on the owning thread.
use serde::Deserialize;
use serde_json::{json, Value};
use std::{io, os::unix::fs::PermissionsExt, path::Path, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::UnixStream,
    sync::{mpsc, watch},
    task::JoinHandle,
};

pub const LIMIT: usize = 2 * 1024 * 1024;

async fn read_frame(reader: &mut (impl AsyncRead + Unpin)) -> io::Result<Value> {
    let size = reader.read_u32().await? as usize;
    if !(2..=LIMIT).contains(&size) {
        return Err(io::Error::other("TAKEOVER_FRAME_LIMIT"));
    }
    let mut data = vec![0; size];
    reader.read_exact(&mut data).await?;
    if data[0] != 1 {
        return Err(io::Error::other("TAKEOVER_FRAME_KIND"));
    }
    serde_json::from_slice(&data[1..]).map_err(io::Error::other)
}

async fn write_frame(writer: &mut (impl AsyncWrite + Unpin), value: &Value) -> io::Result<()> {
    let data = serde_json::to_vec(value)?;
    if data.len() + 1 > LIMIT {
        return Err(io::Error::other("TAKEOVER_FRAME_LIMIT"));
    }
    writer.write_u32((data.len() + 1) as u32).await?;
    writer.write_u8(1).await?;
    writer.write_all(&data).await?;
    writer.flush().await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Credentials {
    version: String,
    attempt_id: String,
    local_token: String,
}

pub struct Channel {
    pub attempt_id: String,
    input: mpsc::Receiver<Value>,
    output: mpsc::Sender<Vec<u8>>,
    closed: watch::Receiver<bool>,
    tasks: Vec<JoinHandle<()>>,
}

impl Drop for Channel {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

impl Channel {
    pub async fn connect(workspace: &Path) -> io::Result<Self> {
        let path = workspace.join("takeover.json");
        let metadata = std::fs::symlink_metadata(&path)?;
        if !metadata.is_file()
            || metadata.len() > 4096
            || metadata.permissions().mode() & 0o077 != 0
        {
            return Err(io::Error::other("TAKEOVER_RESOURCE_INVALID"));
        }
        let resource = std::fs::read(&path)?;
        let credentials: Credentials = serde_json::from_slice(&resource)?;
        if credentials.version != "1"
            || credentials.attempt_id.is_empty()
            || credentials.attempt_id.len() > 128
            || credentials.local_token.len() != 64
            || !credentials
                .local_token
                .bytes()
                .all(|b| b.is_ascii_hexdigit())
            || std::env::current_dir()? != workspace
        {
            return Err(io::Error::other("TAKEOVER_RESOURCE_INVALID"));
        }
        let mut stream = UnixStream::connect("t.sock").await?;
        write_frame(
            &mut stream,
            &json!({"version":"1", "attempt_id":credentials.attempt_id,
            "local_token":credentials.local_token}),
        )
        .await?;
        let reply = read_frame(&mut stream).await?;
        if reply
            != json!({"version":"1", "attempt_id":credentials.attempt_id, "authenticated":true})
            || path.exists()
        {
            return Err(io::Error::other("TAKEOVER_AUTH_FAILED"));
        }
        let (mut reader, mut writer) = stream.into_split();
        let (input_tx, input) = mpsc::channel(64);
        let (output, mut output_rx) = mpsc::channel::<Vec<u8>>(16);
        let (closed_tx, closed) = watch::channel(false);
        let read_closed = closed_tx.clone();
        let reader_task = tokio::spawn(async move {
            while let Ok(value) = read_frame(&mut reader).await {
                if input_tx.try_send(value).is_err() {
                    break;
                }
            }
            let _ = read_closed.send(true);
        });
        let writer_task = tokio::spawn(async move {
            while let Some(value) = output_rx.recv().await {
                if !matches!(
                    tokio::time::timeout(Duration::from_secs(2), async {
                        writer.write_all(&value).await?;
                        writer.flush().await
                    })
                    .await,
                    Ok(Ok(()))
                ) {
                    break;
                }
            }
            let _ = closed_tx.send(true);
        });
        Ok(Self {
            attempt_id: credentials.attempt_id,
            input,
            output,
            closed,
            tasks: vec![reader_task, writer_task],
        })
    }

    pub fn send(&self, value: Value) -> bool {
        let Ok(payload) = serde_json::to_vec(&value) else {
            return false;
        };
        self.send_payload(1, payload)
    }

    pub fn closure(&self) -> watch::Receiver<bool> {
        self.closed.clone()
    }

    fn send_payload(&self, kind: u8, payload: Vec<u8>) -> bool {
        if payload.len() + 1 > LIMIT {
            return false;
        }
        let mut data = Vec::with_capacity(payload.len() + 5);
        data.extend_from_slice(&((payload.len() + 1) as u32).to_be_bytes());
        data.push(kind);
        data.extend(payload);
        self.output.try_send(data).is_ok()
    }

    pub fn send_image(&self, metadata: Value, image: Vec<u8>) -> bool {
        let Ok(metadata) = serde_json::to_vec(&metadata) else {
            return false;
        };
        if metadata.len() > 8192 {
            return false;
        }
        let mut payload = Vec::with_capacity(4 + metadata.len() + image.len());
        payload.extend_from_slice(&(metadata.len() as u32).to_be_bytes());
        payload.extend(metadata);
        payload.extend(image);
        self.send_payload(2, payload)
    }
}

pub async fn receive(channel: &mut Option<Channel>) -> Option<Value> {
    match channel {
        Some(channel) => {
            if *channel.closed.borrow() {
                return None;
            }
            tokio::select! {
                biased;
                _ = channel.closed.changed() => None,
                value = channel.input.recv() => value,
            }
        }
        None => std::future::pending().await,
    }
}

pub async fn closed(receiver: Option<watch::Receiver<bool>>) {
    match receiver {
        Some(mut receiver) => {
            if !*receiver.borrow() {
                let _ = receiver.changed().await;
            }
        }
        None => std::future::pending().await,
    }
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub attempt_id: String,
    pub generation: u64,
    pub session_id: String,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Control {
    Open {
        attempt_id: String,
        generation: u64,
        session_id: String,
        ttl_ms: u64,
        #[serde(default)]
        manual_receipt: Option<ManualReceipt>,
        #[serde(default)]
        lease_ms: Option<u64>,
    },
    Lease {
        attempt_id: String,
        generation: u64,
        session_id: String,
        lease_ms: u64,
    },
    View {
        attempt_id: String,
        generation: u64,
        session_id: String,
        page_id: String,
    },
    FrameAck {
        attempt_id: String,
        generation: u64,
        session_id: String,
        frame_seq: u64,
    },
    Input {
        attempt_id: String,
        generation: u64,
        session_id: String,
        input_seq: u64,
        page_id: String,
        navigation_generation: u64,
        frame_seq: u64,
        width: u32,
        height: u32,
        dpr: f64,
        operation: Operation,
    },
    Close {
        attempt_id: String,
        generation: u64,
        session_id: String,
    },
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManualReceipt {
    pub manual_effect_seq: u64,
    pub journal_sha256: String,
}

#[derive(Clone, Deserialize, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    PointerClick { x: f32, y: f32 },
    TextInsert { text: String },
    KeyPress { key: String },
}

#[derive(Clone)]
pub struct Frame {
    pub page_id: String,
    pub navigation_generation: u64,
    pub sequence: u64,
    pub width: u32,
    pub height: u32,
    pub sha256: String,
    pub text_identity: Option<(u64, Option<u64>)>,
}

pub struct Session {
    pub binding: Binding,
    pub deadline: tokio::time::Instant,
    pub lease_deadline: tokio::time::Instant,
    pub next_frame: tokio::time::Instant,
    pub page_id: Option<String>,
    pub frame_sequence: u64,
    pub pending_frame: Option<Frame>,
    pub displayed_frame: Option<Frame>,
    pub input_allowed: bool,
    pub last_input: Option<(u64, Value, Value)>,
}

impl Binding {
    pub fn reply(&self, kind: &str, reason: &str) -> Value {
        json!({"type":kind,"attempt_id":self.attempt_id,"generation":self.generation,
            "session_id":self.session_id,"reason":reason,"capabilities":["control"]})
    }
}

impl Control {
    pub fn binding(&self) -> Binding {
        let (attempt_id, generation, session_id) = match self {
            Self::Open {
                attempt_id,
                generation,
                session_id,
                ..
            }
            | Self::Close {
                attempt_id,
                generation,
                session_id,
            }
            | Self::Lease {
                attempt_id,
                generation,
                session_id,
                ..
            }
            | Self::View {
                attempt_id,
                generation,
                session_id,
                ..
            }
            | Self::FrameAck {
                attempt_id,
                generation,
                session_id,
                ..
            }
            | Self::Input {
                attempt_id,
                generation,
                session_id,
                ..
            } => (attempt_id, generation, session_id),
        };
        Binding {
            attempt_id: attempt_id.clone(),
            generation: *generation,
            session_id: session_id.clone(),
        }
    }
}
