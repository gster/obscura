//! Context/origin-owned committed snapshots. A database lease serializes
//! transactions across documents; retirement releases abandoned leases.
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, Weak};
use base64::{Engine, engine::general_purpose::STANDARD};
use obscura_net::websocket::session::Owner;
use serde_json::{json, Value};

const MAX_DATABASE_BYTES: usize = 8 << 20;
const MAX_CONTEXT_BYTES: usize = 32 << 20;
const MAX_DATABASES: usize = 256;
#[derive(Default)]
pub struct Store { inner: Mutex<Inner>, pub(crate) notify: tokio::sync::Notify }
#[derive(Default)]
struct Inner {
    databases: HashMap<(String,String), Database>,
    leases: HashMap<(String,String), Lease>,
    intents: HashMap<(String,String), Lease>,
    waiters: HashMap<(String,String), VecDeque<Lease>>,
    connections: HashMap<(String,String), Vec<Connection>>,
    bytes: usize,
    next: u64,
}
struct Database { version: u64, bytes: Vec<u8> }
struct Lease { id: String, owner: Weak<Owner>, open: bool }
struct Connection { id: String, owner: Weak<Owner>, version: u64, notice: Option<Option<u64>>, notified: String, delivered: String }
impl Store {
    pub(crate) fn notices(&self, origin: &str, owner: &Arc<Owner>) -> Value {
        let mut inner=self.inner.lock().unwrap_or_else(|e|e.into_inner());let mut notices=Vec::new();
        for ((site,_),connections) in &mut inner.connections {
            if site!=origin {continue;}
            for connection in connections {
                if connection.owner.upgrade().is_some_and(|held|Arc::ptr_eq(&held,owner)) {
                    if let Some(version)=connection.notice.take() {notices.push(json!({"id":connection.id,"oldVersion":connection.version,"newVersion":version,"ticket":connection.notified}));}
                }
            }
        }
        json!(notices)
    }
}
impl Store {
    pub(crate) fn request(&self, origin: &str, owner: &Arc<Owner>, action: &str,
        name: &str, lease: &str, version: u64, bytes: &[u8]) -> Value {
        if !url::Url::parse(origin).ok().is_some_and(|url|
            matches!(url.scheme(),"http"|"https") && url.origin().ascii_serialization()==origin) {
            return json!({"error":"SecurityError"});
        }
        if !owner.active() { return json!({"error":"AbortError"}); }
        if name.len()>4096 { return json!({"error":"QuotaExceededError"}); }
        let mut inner=self.inner.lock().unwrap_or_else(|e|e.into_inner());
        // A frame or worker may disappear with requests queued. Its old lease
        // has no authority to block a successor document indefinitely.
        inner.intents.retain(|_,intent|intent.owner.upgrade().is_some_and(|owner|owner.active()));
        inner.leases.retain(|_,lease|lease.owner.upgrade().is_some_and(|owner|owner.active()));
        inner.waiters.retain(|_,queue| {queue.retain(|lease|lease.owner.upgrade().is_some_and(|owner|owner.active()));!queue.is_empty()});
        inner.connections.retain(|_,rows|{rows.retain(|row|row.owner.upgrade().is_some_and(|owner|owner.active()));!rows.is_empty()});
        if action=="catalog" {
            let rows:Vec<_>=inner.databases.iter().filter(|((site,_),_)|site==origin)
                .map(|((_,name),db)|json!({"name":name,"version":db.version})).collect();
            return json!({"databases":rows});
        }
        let key=(origin.to_owned(),name.to_owned());
        let snapshot=|inner:&Inner| inner.databases.get(&key).map_or_else(
            ||json!({"version":0,"data":""}),
            |db|json!({"version":db.version,"data":STANDARD.encode(&db.bytes)}));
        match action {
            "read" => snapshot(&inner),
            "open" => {
                let admitted=inner.leases.get(&key).is_some_and(|held|held.id==lease&&held.owner.upgrade().is_some_and(|held|Arc::ptr_eq(&held,owner)));
                if !admitted {return json!({"error":"AbortError"});}
                if inner.connections.values().map(Vec::len).sum::<usize>()>=MAX_DATABASES {return json!({"error":"QuotaExceededError"});}
                if inner.databases.get(&key).is_none_or(|db|db.version!=version) {return json!({"error":"InvalidStateError"});}
                inner.next+=1;let id=inner.next.to_string();
                inner.leases.remove(&key);
                inner.connections.entry(key).or_default().push(Connection{id:id.clone(),owner:Arc::downgrade(owner),version,notice:None,notified:String::new(),delivered:String::new()});
                json!({"connection":id})
            }
            "ack" => {
                if let Some(rows)=inner.connections.get_mut(&key) {
                    for row in rows {
                        if row.id==lease&&row.notified==version.to_string()&&row.owner.upgrade().is_some_and(|held|Arc::ptr_eq(&held,owner)) {
                            row.delivered=row.notified.clone();
                        }
                    }
                }
                json!({"ok":true})
            }
            "disconnect" => {
                if let Some(rows)=inner.connections.get_mut(&key) {rows.retain(|row|
                    !(row.id==lease && row.owner.upgrade().is_some_and(|held|Arc::ptr_eq(&held,owner))));}
                self.notify.notify_waiters();json!({"ok":true})
            }
            "prepare" => {
                let active=inner.leases.get(&key).is_some_and(|held|held.id==lease&&held.owner.upgrade().is_some_and(|held|Arc::ptr_eq(&held,owner)));
                let waiting=inner.intents.get(&key).is_some_and(|held|held.id==lease&&held.owner.upgrade().is_some_and(|held|Arc::ptr_eq(&held,owner)));
                if !active&&!waiting {return json!({"error":"AbortError"});}
                let rows=inner.connections.entry(key.clone()).or_default();
                for row in rows.iter_mut() {if row.notified!=lease {row.notified=lease.to_owned();row.notice=Some((version!=0).then_some(version));}}
                let blocked=!rows.is_empty();let awaiting=rows.iter().any(|row|row.delivered!=lease);
                if blocked&&active {
                    let intent=inner.leases.remove(&key).unwrap();inner.intents.insert(key.clone(),intent);
                }
                self.notify.notify_waiters();
                if blocked {return json!({"blocked":true,"awaiting":awaiting});}
                if waiting {
                    // Existing connections may create work before closing. Let
                    // that FIFO transaction queue drain without blocking on the
                    // upgrade intent, then regain the exclusive execution lease.
                    if inner.leases.contains_key(&key)||inner.waiters.get(&key).is_some_and(|queue|queue.iter().any(|entry|!entry.open)) {
                        return json!({"blocked":true});
                    }
                    let intent=inner.intents.remove(&key).unwrap();inner.leases.insert(key.clone(),intent);
                }
                let mut result=snapshot(&inner);result["blocked"]=json!(false);result["lease"]=json!(lease);result
            }
            "begin" => {
                let ticket=if lease.is_empty() {
                    if inner.waiters.values().map(VecDeque::len).sum::<usize>()+inner.leases.len()+inner.intents.len()>=MAX_DATABASES {
                        return json!({"error":"QuotaExceededError"});
                    }
                    inner.next+=1;let id=inner.next.to_string();
                    inner.waiters.entry(key.clone()).or_default().push_back(Lease{id:id.clone(),owner:Arc::downgrade(owner),open:version!=0});id
                } else {lease.to_owned()};
                let queued=inner.waiters.get(&key).and_then(|queue|queue.iter().find(|entry|entry.id==ticket));
                if queued.is_none_or(|entry|entry.owner.upgrade().is_none_or(|held|!Arc::ptr_eq(&held,owner))) {
                    return json!({"error":"AbortError"});
                }
                if inner.leases.contains_key(&key)||inner.waiters[&key].iter().find(|entry|!inner.intents.contains_key(&key)||!entry.open).is_none_or(|entry|entry.id!=ticket) {
                    return json!({"busy":true,"ticket":ticket});
                }
                let queue=inner.waiters.get_mut(&key).unwrap();
                let position=queue.iter().position(|entry|entry.id==ticket).unwrap();
                let admitted=queue.remove(position).unwrap();
                inner.leases.insert(key.clone(),admitted);
                let mut result=snapshot(&inner);result["lease"]=json!(ticket);result
            }
            "cancel" => {
                if let Some(queue)=inner.waiters.get_mut(&key) {queue.retain(|entry|
                    !(entry.id==lease&&entry.owner.upgrade().is_some_and(|held|Arc::ptr_eq(&held,owner))));}
                json!({"ok":true})
            }
            "abort" | "commit" | "upgrade" | "delete" => {
                if action=="abort"&&inner.intents.get(&key).is_some_and(|held|held.id==lease&&held.owner.upgrade().is_some_and(|held|Arc::ptr_eq(&held,owner))) {
                    inner.intents.remove(&key);return json!({"ok":true});
                }
                let admitted=inner.leases.get(&key).is_some_and(|held| held.id==lease &&
                    held.owner.upgrade().is_some_and(|held|Arc::ptr_eq(&held,owner)));
                if !admitted { return json!({"error":"AbortError"}); }
                inner.leases.remove(&key);
                if action=="abort" { return json!({"ok":true}); }
                let previous=inner.databases.get(&key).map_or(0,|db|db.bytes.len());
                if action=="delete" {
                    inner.databases.remove(&key);inner.bytes-=previous;return json!({"ok":true});
                }
                if bytes.len()>MAX_DATABASE_BYTES || inner.bytes-previous+bytes.len()>MAX_CONTEXT_BYTES
                    || (!inner.databases.contains_key(&key)&&inner.databases.len()>=MAX_DATABASES)
                    || (action=="upgrade"&&inner.connections.values().map(Vec::len).sum::<usize>()>=MAX_DATABASES) {
                    return json!({"error":"QuotaExceededError"});
                }
                inner.bytes=inner.bytes-previous+bytes.len();
                inner.databases.insert(key.clone(),Database{version,bytes:bytes.to_vec()});
                if action=="upgrade" {
                    inner.next+=1;let id=inner.next.to_string();
                    inner.connections.entry(key).or_default().push(Connection{id:id.clone(),owner:Arc::downgrade(owner),version,notice:None,notified:String::new(),delivered:String::new()});
                    json!({"ok":true,"connection":id})
                } else {json!({"ok":true})}
            }
            _ => json!({"error":"InvalidStateError"}),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn indexed_db_leases_isolate_origins_contexts_and_retired_documents() {
        let store=Store::default();let owner=Arc::new(Owner::new(Default::default()));
        let first=store.request("https://a.test",&owner,"begin","db","",0,&[]);
        let lease=first["lease"].as_str().unwrap();
        assert_eq!(store.request("https://a.test",&owner,"begin","db","",0,&[])["busy"],true);
        assert_eq!(store.request("https://a.test",&owner,"commit","db",lease,1,b"exact")["ok"],true);
        assert_eq!(store.request("https://a.test",&owner,"read","db","",0,&[])["data"],STANDARD.encode(b"exact"));
        assert_eq!(store.request("https://b.test",&owner,"read","db","",0,&[])["version"],0);
        assert_eq!(Store::default().request("https://a.test",&owner,"read","db","",0,&[])["version"],0);
        store.request("https://a.test",&owner,"begin","db","",0,&[]);
        owner.retire();let next=Arc::new(Owner::new(Default::default()));
        let admitted=store.request("https://a.test",&next,"begin","db","",0,&[]);
        assert!(admitted.get("lease").is_some());
        assert_eq!(store.request("https://a.test",&owner,"commit","db",lease,2,b"bad")["error"],"AbortError");
        assert_eq!(store.request("null",&next,"catalog","","",0,&[])["error"],"SecurityError");
    }
}
