//! A single task owns the upgraded socket. Cancellation drops both halves,
//! including a write blocked by its peer; no V8 value enters this task.
use std::sync::{Arc, atomic::{AtomicBool, AtomicU64, Ordering}};
use tokio::sync::{mpsc, watch, Semaphore, OwnedSemaphorePermit};
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::tungstenite::{Message, protocol::{CloseFrame, frame::coding::CloseCode}};
use super::{MAX_MESSAGE, OwnerPolicy};
use crate::StealthHttpClient;

const DIRECTION_BYTES: usize = 4 << 20;
const RUNTIME_BYTES: usize = 16 << 20;
const EVENT_OVERHEAD: usize = 128;
// Idle sockets do not reserve their maximum possible message. Tungstenite's
// parser is separately bounded by MAX_MESSAGE, a 16 KiB read buffer and the
// 128-session admission limit. Queue/copy memory is charged when a frame exists.

pub struct RuntimeBudget { bytes: Arc<Semaphore>, sessions: Arc<Semaphore> }
impl Default for RuntimeBudget {
    fn default() -> Self { Self { bytes: Arc::new(Semaphore::new(RUNTIME_BYTES)), sessions: Arc::new(Semaphore::new(128)) } }
}
pub struct Owner {
    pub budget: Arc<RuntimeBudget>,
    sessions: Arc<Semaphore>,
    retired: AtomicBool,
    cancel: watch::Sender<bool>,
    children: std::sync::Mutex<Vec<std::sync::Weak<Owner>>>,
    sockets: std::sync::Mutex<Vec<std::sync::Weak<Session>>>,
}
impl Owner {
    pub fn new(budget: Arc<RuntimeBudget>) -> Self {
        Self { budget, sessions: Arc::new(Semaphore::new(32)), retired: AtomicBool::new(false), cancel: watch::channel(false).0, children: std::sync::Mutex::new(Vec::new()), sockets: std::sync::Mutex::new(Vec::new()) }
    }
    pub fn child(parent: &Arc<Self>) -> Arc<Self> {
        let child=Arc::new(Self::new(parent.budget.clone()));
        let mut children=parent.children.lock().unwrap_or_else(|error|error.into_inner());
        children.retain(|owner|owner.strong_count()>0);
        children.push(Arc::downgrade(&child));
        if !parent.active() { child.retire(); }
        child
    }
    #[cfg(test)]
    pub fn usage(&self)->(usize,usize,usize) { (RUNTIME_BYTES-self.budget.bytes.available_permits(),128-self.budget.sessions.available_permits(),32-self.sessions.available_permits()) }
    pub fn subscribe(&self) -> watch::Receiver<bool> { self.cancel.subscribe() }
    pub fn retire(&self) {
        if self.retired.swap(true, Ordering::AcqRel) { return; }
        self.cancel.send_replace(true);
        for socket in self.sockets.lock().unwrap_or_else(|e|e.into_inner()).drain(..).filter_map(|s|s.upgrade()) { socket.discard_events(); }
        for child in self.children.lock().unwrap_or_else(|error|error.into_inner()).drain(..).filter_map(|child|child.upgrade()) { child.retire(); }
    }
    pub fn active(&self) -> bool { !self.retired.load(Ordering::Acquire) }
}
struct Reservation { _runtime: OwnedSemaphorePermit, _direction: OwnedSemaphorePermit }
impl Reservation {
    fn try_new(owner: &Owner, direction: &Arc<Semaphore>, count: usize) -> Option<Self> {
        Some(Self { _runtime: owner.budget.bytes.clone().try_acquire_many_owned(count as u32).ok()?,
            _direction: direction.clone().try_acquire_many_owned(count as u32).ok()? })
    }
    async fn read(owner: &Owner, direction: &Arc<Semaphore>, count: usize) -> Option<Self> {
        // Wait for direction space without holding shared budget needed by writers.
        let direction = direction.clone().acquire_many_owned(count as u32).await.ok()?;
        Some(Self { _runtime: owner.budget.bytes.clone().try_acquire_many_owned(count as u32).ok()?,
            _direction: direction })
    }
}
pub enum Event { Open(String), Text(String), Binary(Vec<u8>), Error, Close { code: u16, reason: String, clean: bool } }
pub struct Delivery { pub event: Event, _reservation: Option<Reservation> }
struct Write { message: Message, bytes: usize, _reservation: Reservation }
pub struct Session {
    owner: Arc<Owner>,
    commands: mpsc::Sender<Write>,
    events: std::sync::Mutex<mpsc::Receiver<Delivery>>,
    delivery: std::sync::Mutex<Option<Delivery>>,
    close_requested: tokio::sync::Notify,
    cancel: watch::Sender<bool>,
    buffered: AtomicU64,
    open: AtomicBool,
    closing: AtomicBool,
    outbound: Arc<Semaphore>,
}
impl Session {
    pub fn active(&self) -> bool { self.owner.active() }
    pub fn buffered(&self) -> u64 { self.buffered.load(Ordering::Acquire) }
    pub fn cancel(&self) { self.cancel.send_replace(true); self.discard_events(); }
    fn discard_events(&self) {
        self.delivery.lock().unwrap_or_else(|e|e.into_inner()).take();
        let mut events=self.events.lock().unwrap_or_else(|e|e.into_inner());
        while events.try_recv().is_ok() {}
    }
    pub fn hold_delivery(&self, delivery:Delivery) {
        let mut held=self.delivery.lock().unwrap_or_else(|e|e.into_inner());
        if self.active() { *held=Some(delivery); }
    }
    pub fn payload(&self)->Vec<u8> {
        let delivery=self.delivery.lock().unwrap_or_else(|e|e.into_inner());
        match delivery.as_ref().map(|d|&d.event) {
            Some(Event::Text(text))=>text.as_bytes().to_vec(),
            Some(Event::Binary(bytes))=>bytes.clone(),
            _=>Vec::new(),
        }
    }
    pub fn acknowledge(&self) { self.delivery.lock().unwrap_or_else(|e|e.into_inner()).take(); }
    pub async fn next(&self) -> Option<Delivery> {
        if !self.active() { return None; }
        let item = std::future::poll_fn(|cx|self.events.lock().unwrap_or_else(|e|e.into_inner()).poll_recv(cx)).await;
        if !self.active() { return None; }
        item
    }
    pub fn send(&self, data: &[u8], text: bool) -> Result<(), &'static str> {
        if !self.open.load(Ordering::Acquire) || self.closing.load(Ordering::Acquire) || !self.active() { return Err("WebSocket is not open"); }
        if data.len() > MAX_MESSAGE { self.cancel(); return Err("WebSocket message exceeds limit"); }
        let bytes = data.len();
        let reservation = Reservation::try_new(&self.owner, &self.outbound, 2 * bytes + EVENT_OVERHEAD)
            .ok_or_else(||{self.cancel();"WebSocket send budget exhausted"})?;
        // The native copy is made only after admission.
        let data=data.to_vec();
        let message = if text { Message::Text(String::from_utf8(data).map_err(|_|"Invalid UTF-8")?.into()) }
            else { Message::Binary(data.into()) };
        self.buffered.fetch_add(bytes as u64, Ordering::AcqRel);
        if self.commands.try_send(Write { message, bytes, _reservation: reservation }).is_err() {
            self.buffered.fetch_sub(bytes as u64, Ordering::AcqRel);
            self.cancel(); return Err("WebSocket send queue exhausted");
        }
        Ok(())
    }
    pub fn close(&self, code: Option<u16>, reason: String) -> Result<(), &'static str> {
        if self.closing.swap(true, Ordering::AcqRel) { return Ok(()); }
        self.close_requested.notify_one();
        if !self.open.load(Ordering::Acquire) { self.cancel(); return Ok(()); }
        let reservation = Reservation::try_new(&self.owner, &self.outbound, EVENT_OVERHEAD + reason.len())
            .ok_or_else(||{self.cancel();"WebSocket close budget exhausted"})?;
        let code = code.or_else(|| (!reason.is_empty()).then_some(1000));
        let frame = code.map(|code| CloseFrame { code: CloseCode::from(code), reason: reason.into() });
        self.commands.try_send(Write { message: Message::Close(frame), bytes: 0, _reservation: reservation })
            .map_err(|_| { self.cancel(); "WebSocket close queue exhausted" })
    }
}

pub fn start(client: Arc<StealthHttpClient>, owner: Arc<Owner>, policy: Arc<dyn OwnerPolicy>,
    url: url::Url, protocols: Vec<String>,
) -> Result<Arc<Session>, &'static str> {
    if !owner.active() { return Err("WebSocket owner retired"); }
    let owner_permit = owner.sessions.clone().try_acquire_owned().map_err(|_|"WebSocket owner session limit")?;
    let runtime_permit = owner.budget.sessions.clone().try_acquire_owned().map_err(|_|"WebSocket runtime session limit")?;
    let (commands, mut receiver) = mpsc::channel::<Write>(64);
    let (events, event_receiver) = mpsc::channel(64);
    let (cancel, mut cancel_rx) = watch::channel(false);
    let mut owner_cancel = owner.cancel.subscribe();
    let session = Arc::new(Session { owner: owner.clone(), commands, events: std::sync::Mutex::new(event_receiver), delivery:std::sync::Mutex::new(None), close_requested:tokio::sync::Notify::new(), cancel,
        buffered: AtomicU64::new(0), open: AtomicBool::new(false), closing: AtomicBool::new(false),
        outbound: Arc::new(Semaphore::new(DIRECTION_BYTES)) });
    {
        let mut sockets=owner.sockets.lock().unwrap_or_else(|e|e.into_inner());
        sockets.retain(|s|s.strong_count()>0);
        sockets.push(Arc::downgrade(&session));
        if !owner.active() { session.cancel(); return Err("WebSocket owner retired"); }
    }
    let task_session = session.clone();
    tokio::spawn(async move {
        let (_owner_permit, _runtime_permit) = (owner_permit, runtime_permit);
        let inbound = Arc::new(Semaphore::new(DIRECTION_BYTES));
        let run = async {
            let mut handshake_cancel = owner.cancel.subscribe();
            let opened = client.open_websocket_transport(&url, &protocols, Some(policy.as_ref()), &mut handshake_cancel).await
                .map_err(|_|())?;
            if !owner.active() || *task_session.cancel.borrow() { return Err(()); }
            task_session.open.store(true, Ordering::Release);
            events.send(Delivery { event: Event::Open(opened.protocol), _reservation: None }).await.map_err(|_|())?;
            let (mut sink, mut stream) = opened.stream.split();
            let (flush_tx, mut flush_rx) = mpsc::channel::<tokio::sync::oneshot::Sender<()>>(1);
            // A write is never cancelled to service another ordinary message.
            // Only whole-session retirement/close timeout tears down a write.
            let writer = async {
                loop {
                    tokio::select! {
                        control = flush_rx.recv() => {
                            let Some(done) = control else { return Err(()); };
                            sink.flush().await.map_err(|_|())?;
                            let _=done.send(());
                        }
                        write = receiver.recv() => {
                            let Some(write) = write else { return Err(()); };
                            sink.send(write.message).await.map_err(|_|())?;
                            task_session.buffered.fetch_sub(write.bytes as u64, Ordering::AcqRel);
                            drop(write._reservation);
                        }
                    }
                }
            };
            let reader = async {
                loop {
                    let message = stream.next().await.ok_or(())?.map_err(|_|())?;
                    let bytes = message.len();
                    let reservation = Reservation::read(&owner, &inbound, 2 * bytes + EVENT_OVERHEAD).await.ok_or(())?;
                    let event = match message {
                        Message::Text(value) => Event::Text(value.to_string()),
                        Message::Binary(value) => Event::Binary(value.to_vec()),
                        Message::Close(frame) => {
                            let (code, reason) = frame.map(|frame|(u16::from(frame.code), frame.reason.to_string())).unwrap_or((1005,String::new()));
                            let (done, wait)=tokio::sync::oneshot::channel();
                            flush_tx.send(done).await.map_err(|_|())?;
                            wait.await.map_err(|_|())?;
                            return Ok((code,reason));
                        }
                        Message::Ping(_) => {
                            let (done,wait)=tokio::sync::oneshot::channel();
                            flush_tx.send(done).await.map_err(|_|())?;
                            wait.await.map_err(|_|())?;continue;
                        }
                        Message::Pong(_) => continue,
                        _ => return Err(()),
                    };
                    events.send(Delivery { event, _reservation: Some(reservation) }).await.map_err(|_|())?;
                }
            };
            let close_timeout=async { task_session.close_requested.notified().await; tokio::time::sleep(std::time::Duration::from_secs(5)).await; };
            tokio::select! { result=reader=>result, result=writer=>result, _=close_timeout=>Err(()) }
        };
        let result = tokio::select! {
            biased;
            _=owner_cancel.changed()=>None,
            _=cancel_rx.changed()=>Some(Err(())),
            result=run=>Some(result),
        };
        task_session.open.store(false, Ordering::Release);
        task_session.buffered.store(0, Ordering::Release);
        // Drop unsent frames and their reservations even if JS retains the handle.
        while receiver.try_recv().is_ok() {}
        if !owner.active() { task_session.discard_events(); }
        // `run` (including the stream) is dropped before waiting on event space.
        if let Some(result) = result.filter(|_|owner.active()) {
            let finish = async {
                let event = match result {
                    Ok((code,reason))=>Event::Close {code,reason,clean:true},
                    Err(())=>{
                        let _=events.send(Delivery {event:Event::Error,_reservation:None}).await;
                        Event::Close {code:1006,reason:String::new(),clean:false}
                    }
                };
                let _=events.send(Delivery {event,_reservation:None}).await;
            };
            tokio::select! { _=owner_cancel.changed()=>{}, _=finish=>{} }
            if !owner.active() { task_session.discard_events(); }
        }
    });
    Ok(session)
}
