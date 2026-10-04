//! Real loopback protocol fixtures; no browser/public request is required.
use super::*;
use crate::runtime::ObscuraJsRuntime;
use tokio::net::TcpListener;
use std::sync::atomic::{AtomicUsize,Ordering};
use tokio::io::{AsyncReadExt,AsyncWriteExt};
use sha1::Digest;

fn runtime(origin:&str)->ObscuraJsRuntime {
    runtime_with_persona(origin,obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::WindowsChrome145))
}
fn runtime_with_persona(origin:&str,persona:obscura_net::EffectivePersona)->ObscuraJsRuntime {
    let mut rt=ObscuraJsRuntime::with_base_url(origin,persona.clone());
    rt.set_dom(obscura_dom::parse_html("<html><body><iframe id='child'></iframe></body></html>"));
    rt.set_url(origin);
    let jar=Arc::new(CookieJar::new());
    rt.set_http_client(Arc::new(ObscuraHttpClient::with_full_options(jar.clone(),None,true)));
    rt.set_stealth_client(Arc::new(StealthHttpClient::with_proxy(jar,None,true,&persona))).unwrap();
    rt.set_script_policy(crate::csp::ScriptPolicy::default());
    rt.run_page_init();rt
}
async fn upgrade<T:tokio::io::AsyncRead+tokio::io::AsyncWrite+Unpin>(tcp:&mut T,protocol:bool)->String {
    let mut request=Vec::new();
    while !request.ends_with(b"\r\n\r\n") { assert!(request.len()<16384);request.push(tcp.read_u8().await.unwrap()); }
    let request=String::from_utf8(request).unwrap();
    let key=request.lines().find_map(|line|line.split_once(':').filter(|(k,_)|k.eq_ignore_ascii_case("sec-websocket-key")).map(|(_,v)|v.trim())).unwrap();
    let accept=BASE64.encode(sha1::Sha1::digest(format!("{key}258EAFA5-E914-47DA-95CA-C5AB0DC85B11").as_bytes()));
    tcp.write_all(format!("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n{}\r\n",if protocol {"Sec-WebSocket-Protocol: echo.v1\r\n"}else{""}).as_bytes()).await.unwrap();
    request
}
async fn frame<T:tokio::io::AsyncRead+Unpin>(tcp:&mut T)->(u8,Vec<u8>) {
    let first=tcp.read_u8().await.unwrap();let second=tcp.read_u8().await.unwrap();
    assert_ne!(second&128,0,"browser client must mask every frame");
    let size=match second&127 {126=>tcp.read_u16().await.unwrap() as usize,127=>tcp.read_u64().await.unwrap() as usize,n=>n as usize};
    assert!(size<=1<<20);let mut mask=[0;4];tcp.read_exact(&mut mask).await.unwrap();
    let mut data=vec![0;size];tcp.read_exact(&mut data).await.unwrap();for(i,b)in data.iter_mut().enumerate(){*b^=mask[i%4];}(first,data)
}
async fn echo<T:tokio::io::AsyncWrite+Unpin>(tcp:&mut T,opcode:u8,data:&[u8]) {
    assert!(data.len()<126);tcp.write_all(&[opcode,data.len() as u8]).await.unwrap();tcp.write_all(data).await.unwrap();
}

// A bounded runtime pump returns on idle or its observation deadline, not on
// handshake completion. Keep driving real browser tasks until the specified
// browser state is observed, with a single explicit wall-clock deadline.
async fn pump_until(rt:&mut ObscuraJsRuntime,condition:&str,diagnostic:&str,accepts:&AtomicUsize,budget_ms:u64) {
    let started=tokio::time::Instant::now();
    let deadline=started+std::time::Duration::from_millis(budget_ms);
    loop {
        let ready=rt.evaluate(condition).unwrap()==serde_json::json!(true);
        if ready || tokio::time::Instant::now()>=deadline {
            let observed=rt.evaluate(diagnostic).unwrap();
            eprintln!("WS condition={condition} elapsed_ms={} accepted={} observed={observed}",started.elapsed().as_millis(),accepts.load(Ordering::SeqCst));
            assert!(ready,"browser condition did not complete within {budget_ms} ms: {observed}");
            return;
        }
        let remaining=deadline.saturating_duration_since(tokio::time::Instant::now()).as_millis().min(100) as u64;
        if remaining==0 {continue;}
        rt.run_event_loop_bounded(remaining).await.unwrap();
        tokio::task::yield_now().await;
    }
}

#[tokio::test(flavor="current_thread")]
async fn websocket_real_echo_tasks_binary_views_blob_and_close() {
    tokio::time::timeout(std::time::Duration::from_secs(6),async {
        let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
        let server=tokio::spawn(async move {
            let(mut tcp,_)=listener.accept().await.unwrap();let request=upgrade(&mut tcp,true).await;
            assert!(request.to_ascii_lowercase().contains(&format!("origin: http://{address}")));
            assert!(request.contains("secret=value"));
            let mut received=Vec::new();
            loop {let(op,data)=frame(&mut tcp).await;echo(&mut tcp,op,&data).await;if op&15==8 {break;}received.push((op&15,data));}
            received
        });
        let origin=format!("http://{address}/index");let mut rt=runtime(&origin);
        let child=crate::frame::FrameRealm::new(&mut rt,96,0,&origin,"<html><body></body></html>").unwrap();
        rt.state.borrow().cookie_jar.as_ref().unwrap().set_cookie("secret=value; HttpOnly; SameSite=Strict; Path=/",&url::Url::parse(&origin).unwrap());
        assert_eq!(rt.evaluate(&format!(r#"(()=>{{
            globalThis.events=[];globalThis.payloads=[];globalThis.socket=new WebSocket('ws://{address}/echo',['echo.v1']);
            socket.binaryType='arraybuffer';
            socket.onopen=e=>{{events.push(['open',e.isTrusted,socket.protocol]);socket.send('héllo');const a=new Uint8Array([9,1,2,8]);socket.send(a.subarray(1,3));a.fill(7);const child=__obscura_frameObjects[96].window;const blob=new child.Blob([new Uint8Array([3,4])]);blob._bytes.fill(9);child.WebSocket.prototype.send.call(socket,blob);}};
            socket.onmessage=e=>{{events.push(['message',e.isTrusted]);payloads.push(typeof e.data==='string'?e.data:Array.from(new Uint8Array(e.data)));if(payloads.length===3)socket.close(1000,'done');}};
            socket.onerror=()=>events.push(['error']);socket.onclose=e=>events.push(['close',e.code,e.reason,e.wasClean]);
            return [socket.readyState,socket.protocol,events.length];
        }})()"#)).unwrap(),serde_json::json!([0,"",0]));
        rt.run_event_loop_bounded(4000).await.unwrap();
        assert_eq!(rt.evaluate("payloads").unwrap(),serde_json::json!(["héllo",[1,2],[3,4]]));
        assert_eq!(rt.evaluate("[events,socket.readyState,socket.bufferedAmount]").unwrap(),serde_json::json!([[ ["open",true,"echo.v1"],["message",true],["message",true],["message",true],["close",1000,"done",true]],3,0]));
        assert_eq!(server.await.unwrap(),vec![(1,"héllo".as_bytes().to_vec()),(2,vec![1,2]),(2,vec![3,4])]);
        drop(child);
    }).await.unwrap();
}

#[tokio::test(flavor="current_thread")]
async fn websocket_connect_src_and_missing_policy_fail_before_tcp() {
    let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
    let mut rt=runtime(&format!("http://{address}/"));
    let mut policy=crate::csp::ScriptPolicy::default();policy.append("default-src *; connect-src 'none'");rt.set_script_policy(policy);
    rt.evaluate(&format!("(()=>{{globalThis.events=[];const w=new WebSocket('ws://{address}/');w.onopen=()=>events.push('open');w.onerror=()=>events.push('error');w.onclose=e=>events.push(e.code);return true;}})()")).unwrap();
    rt.run_event_loop_bounded(1000).await.unwrap();assert_eq!(rt.evaluate("events").unwrap(),serde_json::json!(["error",1006]));
    rt.state.borrow_mut().websocket_policy_known=false;
    rt.evaluate(&format!("(()=>{{const w=new WebSocket('ws://{address}/');w.onopen=()=>events.push('unexpected');w.onerror=()=>events.push('unknown');return true;}})()")).unwrap();
    rt.run_event_loop_bounded(1000).await.unwrap();assert_eq!(rt.evaluate("events").unwrap(),serde_json::json!(["error",1006,"unknown"]));
    assert!(tokio::time::timeout(std::time::Duration::from_millis(30),listener.accept()).await.is_err());
}

#[tokio::test(flavor="current_thread")]
async fn websocket_private_slots_and_trust_resist_prototype_overrides() {
    tokio::time::timeout(std::time::Duration::from_secs(6),async {
        let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
        let accepts=Arc::new(AtomicUsize::new(0));let server_accepts=accepts.clone();
        let server=tokio::spawn(async move {
            let(mut tcp,_)=listener.accept().await.unwrap();server_accepts.fetch_add(1,Ordering::SeqCst);upgrade(&mut tcp,false).await;
            let mut received=Vec::new();
            loop {let(op,data)=frame(&mut tcp).await;echo(&mut tcp,op,&data).await;if op&15==8 {break;}received.push((op&15,data));}
            received
        });
        let origin=format!("http://{address}/");let mut rt=runtime(&origin);
        // Trusted host fixture supplies the policy normally carried by the
        // native document response; an unknown child policy must fail closed.
        rt.state.borrow_mut().websocket_frame_policies.insert(96,crate::csp::ScriptPolicy::default());
        let child=crate::frame::FrameRealm::new(&mut rt,96,0,&origin,"<html><body></body></html>").unwrap();
        rt.evaluate(&format!(r#"(()=>{{
            globalThis.slotHooks=[];globalThis.trustHooks=[];globalThis.events=[];globalThis.payloads=[];
            const child=__obscura_frameObjects[96].window;
            const forged={{[Symbol.toStringTag]:'Blob',toString(){{return 'ordinary-text';}}}};
            const forgedSlot={{url:'ws://forged.invalid/',readyState:1,code:4444,reason:'forged',wasClean:false}};
            globalThis.restoreSlotHooks=[];
            for(const realm of [globalThis,child]) {{
                const map=realm.WeakMap.prototype,set=realm.WeakSet.prototype;
                const get=map.get,put=map.set,has=map.has,add=set.add,contains=set.has;
                restoreSlotHooks.push(()=>{{map.get=get;map.set=put;map.has=has;set.add=add;set.has=contains;}});
                map.get=function(key){{
                    const value=get.call(this,key);
                    if(key===forged || (value && (Object.prototype.hasOwnProperty.call(value,'handle') ||
                        (Object.prototype.hasOwnProperty.call(value,'code') && Object.prototype.hasOwnProperty.call(value,'wasClean')) ||
                        key instanceof Blob || key instanceof child.Blob))){{slotHooks.push('get');return forgedSlot;}}
                    return value;
                }};
                map.set=function(key,value){{
                    if(value && (Object.prototype.hasOwnProperty.call(value,'handle') ||
                        (Object.prototype.hasOwnProperty.call(value,'code') && Object.prototype.hasOwnProperty.call(value,'wasClean')) ||
                        key instanceof Blob || key instanceof child.Blob))slotHooks.push('set');
                    return put.call(this,key,value);
                }};
                map.has=function(key){{if(key===forged){{slotHooks.push('has');return true;}}return has.call(this,key);}};
                set.add=function(value){{if(value instanceof Event || value instanceof child.Event)trustHooks.push('add');return add.call(this,value);}};
                set.has=function(value){{if(value instanceof Event || value instanceof child.Event){{trustHooks.push('has');return true;}}return contains.call(this,value);}};
            }}
            const throws=f=>{{try{{f();return '';}}catch(e){{return e.name;}}}};
            const urlGet=Object.getOwnPropertyDescriptor(WebSocket.prototype,'url').get;
            const childGet=Object.getOwnPropertyDescriptor(child.WebSocket.prototype,'url').get;
            const codeGet=Object.getOwnPropertyDescriptor(CloseEvent.prototype,'code').get;
            globalThis.brands=[throws(()=>urlGet.call(forged)),throws(()=>childGet.call(forged)),throws(()=>codeGet.call(forged))];
            globalThis.synthetic=[new Event('open').isTrusted,new MessageEvent('message').isTrusted,new CloseEvent('close').isTrusted];
            globalThis.socket=new child.WebSocket('ws://{address}/');socket.binaryType='arraybuffer';
            brands.push(throws(()=>urlGet.call(new Proxy(socket,{{}}))),urlGet.call(socket)===childGet.call(socket));
            socket.onopen=e=>{{events.push(['open',e.isTrusted]);socket.send(new Blob([new Uint8Array([1,2])]));
                const blob=new child.Blob([new child.Uint8Array([3,4,5])]);socket.send(blob.slice(1));socket.send(forged);}};
            socket.onmessage=e=>{{events.push(['message',e.isTrusted]);payloads.push(typeof e.data==='string'?e.data:Array.from(new Uint8Array(e.data)));if(payloads.length===3)socket.close(1000,'done');}};
            socket.onerror=()=>events.push(['error']);socket.onclose=e=>{{events.push(['close',e.isTrusted,e.code,e.reason,e.wasClean]);globalThis.slotTestDone=true;}};
            return true;
        }})()"#)).unwrap();
        pump_until(&mut rt,"globalThis.slotTestDone===true","[events,payloads,slotHooks,trustHooks]",&accepts,4000).await;
        let result=rt.evaluate("(()=>{for(const restore of restoreSlotHooks)restore();return {brands,synthetic,events,payloads,slotHooks,trustHooks,readyState:socket.readyState};})()").unwrap();
        assert_eq!(result,serde_json::json!({
            "brands":["TypeError","TypeError","TypeError","TypeError",true],"synthetic":[false,false,false],
            "events":[["open",true],["message",true],["message",true],["message",true],["close",true,1000,"done",true]],
            "payloads":[[1,2],[4,5],"ordinary-text"],"slotHooks":[],"trustHooks":[],"readyState":3,
        }));
        assert_eq!(server.await.unwrap(),vec![(2,vec![1,2]),(2,vec![4,5]),(1,b"ordinary-text".to_vec())]);
        drop(child);
    }).await.unwrap();
}

#[tokio::test(flavor="current_thread")]
async fn websocket_sync_real_frame_retirement_closes_idle_socket_before_host_drop() {
    tokio::time::timeout(std::time::Duration::from_secs(5),async {
        let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
        let accepts=Arc::new(AtomicUsize::new(0));let server_accepts=accepts.clone();
        let server=tokio::spawn(async move {let(mut tcp,_)=listener.accept().await.unwrap();server_accepts.fetch_add(1,Ordering::SeqCst);upgrade(&mut tcp,false).await;let mut byte=[0];assert_eq!(tcp.read(&mut byte).await.unwrap(),0);});
        let origin=format!("http://{address}/");let mut rt=runtime(&origin);
        rt.evaluate("(()=>{globalThis.f=document.getElementById('child');void f.contentDocument;f._frameId=95;__obscura_frameElements[95]=f;return true;})()").unwrap();
        // Trusted host fixture seeds the policy that a native response would carry.
        rt.state.borrow_mut().websocket_frame_policies.insert(95,crate::csp::ScriptPolicy::default());
        let child=crate::frame::FrameRealm::new(&mut rt,95,0,&origin,"<html><body>alive</body></html>").unwrap();
        child.execute_script(&mut rt,&format!("globalThis.events=[];globalThis.w=new WebSocket('ws://{address}/');w.onopen=()=>events.push('open');w.onmessage=()=>events.push('stale');w.onclose=()=>events.push('close');")).unwrap();
        pump_until(&mut rt,"__obscura_frameObjects[95].window.w.readyState!==WebSocket.CONNECTING",
            "[__obscura_frameObjects[95].window.w.readyState,__obscura_frameObjects[95].window.events]",&accepts,3000).await;
        assert_eq!(accepts.load(Ordering::SeqCst),1);
        assert_eq!(child.evaluate(&mut rt,"[w.readyState,events]").unwrap(),serde_json::json!([1,["open"]]));
        rt.evaluate("(()=>{f.remove();return true;})()").unwrap();
        server.await.unwrap();
        rt.run_event_loop_bounded(1000).await.unwrap();
        assert_eq!(child.evaluate(&mut rt,"[w.readyState,events]").unwrap(),serde_json::json!([3,["open"]]));
        assert_eq!(child.evaluate(&mut rt,"document.body.textContent").unwrap(),serde_json::json!("alive"));
        drop(child);
    }).await.unwrap();
}

#[test]
fn websocket_contextual_cookies_and_csp_are_actual_owner_inputs() {
    let jar=CookieJar::new();let endpoint=url::Url::parse("https://a.example.test/path").unwrap();
    jar.set_cookie("strict=s; Secure; SameSite=Strict; HttpOnly; Path=/",&endpoint);
    jar.set_cookie("lax=l; Secure; SameSite=Lax; Path=/",&endpoint);
    jar.set_cookie("none=n; Secure; SameSite=None; Path=/",&endpoint);
    assert!(jar.subresource_cookie_header(&endpoint,Some("https://example.test")).contains("strict=s"));
    assert_eq!(jar.subresource_cookie_header(&endpoint,None),"none=n");
    assert_eq!(jar.subresource_cookie_header(&endpoint,Some("http://example.test")),"none=n");
    jar.set_subresource_cookie("rejected=x; SameSite=Lax; Secure",&endpoint,None);
    jar.set_subresource_cookie("accepted=y; SameSite=None; Secure",&endpoint,None);
    assert!(!jar.get_cookie_header(&endpoint).contains("rejected="));assert!(jar.get_cookie_header(&endpoint).contains("accepted=y"));
    let owner=Arc::new(Owner::new(Arc::new(Default::default())));let mut csp=crate::csp::ScriptPolicy::default();
    csp.append("default-src 'none'; connect-src 'self' wss://other.test/path/");csp.append("connect-src wss:");
    let policy=Policy{document:endpoint.to_string(),site:Some("https://example.test".into()),csp,known:true,owner:owner.clone(),blocked:Vec::new(),headers:Vec::new()};
    assert!(policy.authorize(&url::Url::parse("wss://a.example.test/socket").unwrap(),&endpoint,&jar).is_ok());
    assert!(policy.authorize(&url::Url::parse("ws://a.example.test/socket").unwrap(),&endpoint,&jar).is_err());
    assert!(policy.authorize(&url::Url::parse("wss://other.test/not-path").unwrap(),&endpoint,&jar).is_err());
    owner.retire();assert!(policy.authorize(&url::Url::parse("wss://a.example.test/socket").unwrap(),&endpoint,&jar).is_err());
}

#[tokio::test(flavor="current_thread")]
async fn websocket_interception_url_rewrite_rechecks_connect_src() {
    let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
    let mut rt=runtime(&format!("http://{address}/"));let mut policy=crate::csp::ScriptPolicy::default();policy.append("connect-src 'self'");rt.set_script_policy(policy);
    let(tx,mut rx)=tokio::sync::mpsc::unbounded_channel();{let mut state=rt.state.borrow_mut();state.intercept_enabled=true;state.intercept_tx=Some(tx);}
    rt.evaluate(&format!("(()=>{{globalThis.events=[];const w=new WebSocket('ws://{address}/');w.onopen=()=>events.push('open');w.onerror=()=>events.push('error');w.onclose=e=>events.push(e.code);return true;}})()")).unwrap();
    let resolve=async {let request=rx.recv().await.unwrap();assert_eq!(request.resource_type,"WebSocket");request.resolver.send(InterceptResolution::Continue{url:Some(format!("ws://localhost:{}/",address.port())),method:None,headers:None,body:None}).unwrap();};
    let(result,())=tokio::join!(rt.run_event_loop_bounded(1000),resolve);result.unwrap();
    assert_eq!(rt.evaluate("events").unwrap(),serde_json::json!(["error",1006]));assert!(tokio::time::timeout(std::time::Duration::from_millis(30),listener.accept()).await.is_err());
}

async fn failed_handshake_cookie_context(cross:bool) {
    let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
    let server=tokio::spawn(async move {
        let(mut tcp,_)=listener.accept().await.unwrap();let mut data=Vec::new();while !data.ends_with(b"\r\n\r\n") {data.push(tcp.read_u8().await.unwrap());assert!(data.len()<16384);}
        let request=String::from_utf8(data).unwrap();
        assert!(request.contains("none=n"));assert_eq!(request.contains("strict=s"),!cross);assert_eq!(request.contains("lax=l"),!cross);
        tcp.write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nSet-Cookie: failed=1; HttpOnly; SameSite=Lax; Path=/\r\nConnection: close\r\n\r\n").await.unwrap();
        drop(tcp);let(mut probe,_)=listener.accept().await.unwrap();let mut data=Vec::new();
        while !data.ends_with(b"\r\n\r\n") {data.push(probe.read_u8().await.unwrap());assert!(data.len()<16384);}
        let request=String::from_utf8(data).unwrap();assert!(request.starts_with("GET /probe "));
        assert_eq!(request.contains("failed=1"),!cross);
        probe.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok").await.unwrap();
    });
    let target=format!("http://{address}/");let origin=if cross {format!("http://localhost:{}/",address.port())}else{target.clone()};
    let mut rt=runtime(&origin);let jar=rt.state.borrow().cookie_jar.as_ref().unwrap().clone();let url=url::Url::parse(&target).unwrap();
    for value in ["strict=s; SameSite=Strict; HttpOnly; Path=/","lax=l; SameSite=Lax; HttpOnly; Path=/","none=n; SameSite=None; Secure; HttpOnly; Path=/"] {jar.set_cookie(value,&url);}
    rt.evaluate(&format!("(()=>{{globalThis.events=[];const w=new WebSocket('ws://{address}/');events.push(['constructor',w.readyState]);w.onerror=e=>events.push(['error',w.readyState]);w.onclose=e=>events.push(['close',w.readyState,e instanceof CloseEvent,e.code,e.wasClean]);return true;}})()")).unwrap();
    rt.run_event_loop_bounded(1000).await.unwrap();
    let client=rt.state.borrow().stealth_client.as_ref().unwrap().clone();
    assert_eq!(client.fetch(&url.join("probe").unwrap()).await.unwrap().status,200);server.await.unwrap();
    assert_eq!(rt.evaluate("events").unwrap(),serde_json::json!([["constructor",0],["error",3],["close",3,true,1006,false]]));
    assert_eq!(jar.get_cookie_header(&url).contains("failed=1"),!cross);
}
#[tokio::test(flavor="current_thread")]
async fn websocket_http401_same_site_stores_cookie_before_failed_upgrade() {failed_handshake_cookie_context(false).await;}
#[tokio::test(flavor="current_thread")]
async fn websocket_http401_cross_site_rejects_lax_response_cookie() {failed_handshake_cookie_context(true).await;}

#[tokio::test(flavor="current_thread")]
async fn websocket_idle_sockets_leave_send_budget_and_reason_only_close_is_transmitted() {
    tokio::time::timeout(std::time::Duration::from_secs(8),async {
        let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address=listener.local_addr().unwrap();
        let accepts=Arc::new(AtomicUsize::new(0));let server_accepts=accepts.clone();
        let server=tokio::spawn(async move {
            let mut tasks=Vec::new();
            for index in 0..5 {
                let (mut tcp,_)=listener.accept().await.unwrap();server_accepts.fetch_add(1,Ordering::SeqCst);
                tasks.push(tokio::spawn(async move {
                    upgrade(&mut tcp,false).await;
                    if index==0 {
                        let (opcode,data)=frame(&mut tcp).await;
                        assert_eq!(data,b"x");echo(&mut tcp,opcode,&data).await;
                    }
                    let (opcode,data)=frame(&mut tcp).await;assert_eq!(opcode&15,8);
                    assert_eq!(u16::from_be_bytes([data[0],data[1]]),1000);
                    if index==4 { assert_eq!(&data[2..],b"bye"); }
                    echo(&mut tcp,opcode,&data).await;
                }));
            }
            for task in tasks {task.await.unwrap();}
        });
        let mut rt=runtime(&format!("http://{address}/page"));
        rt.evaluate(&format!(r#"(()=>{{globalThis.sockets=[];globalThis.echoed=false;
          for(let i=0;i<4;i++) sockets.push(new WebSocket('ws://{address}/'+i));
          sockets[0].onmessage=()=>{{echoed=true;}};return true;}})()"#)).unwrap();
        pump_until(&mut rt,"sockets.every(s=>s.readyState===1)","sockets.map(s=>s.readyState)",&accepts,2000).await;
        rt.run_event_loop_bounded(30).await.unwrap();
        rt.evaluate(&format!("(()=>{{sockets[0].send('x');sockets.push(new WebSocket('ws://{address}/4'));return true;}})()")).unwrap();
        pump_until(&mut rt,"echoed&&sockets.every(s=>s.readyState===1)","[echoed,sockets.map(s=>s.readyState)]",&accepts,2000).await;
        rt.evaluate("(()=>{sockets[4].close(undefined,'bye');for(let i=0;i<4;i++)sockets[i].close(1000);return true;})()").unwrap();
        pump_until(&mut rt,"sockets.every(s=>s.readyState===3)","sockets.map(s=>s.readyState)",&accepts,2000).await;
        server.await.unwrap();
    }).await.unwrap();
}

#[tokio::test(flavor="current_thread")]
async fn websocket_webidl_slots_and_exceptions_match_chrome153_control() {
    let mut rt=runtime("http://127.0.0.1:50031/");let mut policy=crate::csp::ScriptPolicy::default();policy.append("connect-src 'none'");rt.set_script_policy(policy);
    let child=crate::frame::FrameRealm::new(&mut rt,95,0,"http://127.0.0.1:50031/child","<html><body></body></html>").unwrap();
    assert_eq!(rt.evaluate(r#"(()=>{
        const throws=f=>{try{f();return '';}catch(e){return e.name;}};
        const w=new WebSocket('ws://owned:fixture@127.0.0.1:50031/');
        const child=__obscura_frameObjects[95].window;
        const get=Object.getOwnPropertyDescriptor(WebSocket.prototype,'readyState').get;
        const childGet=Object.getOwnPropertyDescriptor(child.WebSocket.prototype,'readyState').get;
        let childError=false;try{childGet.call({});}catch(e){childError=e instanceof child.TypeError&&!(e instanceof TypeError);}
        const object={handleEvent(){}};w.onopen=object;
        const result=[new WebSocket('ws://127.0.0.1:50031/',7).readyState,new WebSocket('ws://127.0.0.1:50031/',null).readyState,
            throws(()=>new WebSocket('ws://127.0.0.1:50031/',Symbol())),throws(()=>new WebSocket(Symbol())),w.url,
            throws(()=>w.close(66536)),throws(()=>w.close(NaN)),throws(()=>w.close(-64536)),throws(()=>w.close(Symbol())),
            w.onopen===object,throws(()=>w.send(Symbol())),throws(()=>w.send()),throws(()=>get.call(new Proxy(w,{}))),childGet.call(w),childError];
        w.close();return result;
    })()"#).unwrap(),serde_json::json!([0,0,"TypeError","TypeError","ws://owned:fixture@127.0.0.1:50031/","InvalidAccessError","InvalidAccessError","InvalidAccessError","TypeError",true,"TypeError","TypeError","TypeError",0,true]));
    rt.run_event_loop_bounded(100).await.unwrap();drop(child);
}

#[tokio::test(flavor="current_thread")]
async fn websocket_constructor_captures_meta_policy_in_actual_script_order() {
    tokio::time::timeout(std::time::Duration::from_secs(5),async {
        let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
        let server=tokio::spawn(async move {
            let(mut tcp,_)=listener.accept().await.unwrap();let request=upgrade(&mut tcp,false).await;
            assert!(request.starts_with("GET /before "));assert!(!request.to_ascii_lowercase().contains("authorization:"));
            let(op,data)=frame(&mut tcp).await;assert_eq!(data,b"before");echo(&mut tcp,op,&data).await;
            let(op,data)=frame(&mut tcp).await;assert_eq!(op&15,8);echo(&mut tcp,op,&data).await;
            assert!(tokio::time::timeout(std::time::Duration::from_millis(100),listener.accept()).await.is_err());
        });
        let mut rt=runtime(&format!("http://{address}/page"));
        rt.evaluate(&format!(r#"(()=>{{globalThis.timeline=[];
            const a=new WebSocket('ws://user:pass@{address}/before');
            a.onopen=()=>a.send('before');a.onmessage=()=>{{timeline.push('echo');a.close(1000);}};
            const meta=document.createElement('meta');meta.setAttribute('http-equiv','Content-Security-Policy');meta.setAttribute('content',"connect-src 'none'");document.head.appendChild(meta);
            const b=new WebSocket('ws://{address}/after');b.onerror=()=>timeline.push(['denied',b.readyState]);
            return true;
        }})()"#)).unwrap();
        rt.run_event_loop_bounded(2000).await.unwrap();server.await.unwrap();
        assert_eq!(rt.evaluate("timeline.some(x=>x==='echo')&&timeline.some(x=>Array.isArray(x)&&x[0]==='denied'&&x[1]===3)").unwrap(),serde_json::json!(true));
    }).await.unwrap();
}

#[tokio::test(flavor="current_thread")]
async fn websocket_initial_blank_owns_socket_and_nested_retirement_without_parent_alias() {
    tokio::time::timeout(std::time::Duration::from_secs(6),async {
        let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
        let(tx,mut accepted)=tokio::sync::mpsc::channel(4);
        let accepts=Arc::new(AtomicUsize::new(0));let server_accepts=accepts.clone();
        let server=tokio::spawn(async move {
            let mut tasks=Vec::new();
            for _ in 0..2 {let(mut tcp,_)=listener.accept().await.unwrap();server_accepts.fetch_add(1,Ordering::SeqCst);let tx=tx.clone();tasks.push(tokio::spawn(async move {
                let request=upgrade(&mut tcp,false).await;
                let path=request.split_whitespace().nth(1).unwrap().to_string();tx.send(path.clone()).await.unwrap();
                if path=="/blank" {let mut byte=[0];assert_eq!(tcp.read(&mut byte).await.unwrap(),0);}
                else {let(op,data)=frame(&mut tcp).await;assert_eq!(data,b"parent-alive");echo(&mut tcp,op,&data).await;let(op,data)=frame(&mut tcp).await;assert_eq!(op&15,8);echo(&mut tcp,op,&data).await;}
            }));}
            for task in tasks {task.await.unwrap();}
        });
        let mut rt=runtime(&format!("http://{address}/page"));
        rt.evaluate(&format!(r#"(()=>{{
            globalThis.f=document.getElementById('child');globalThis.oldBlank=f.contentWindow;
            const nested=oldBlank.document.createElement('iframe');oldBlank.document.body.appendChild(nested);
            globalThis.retained=nested.contentWindow;globalThis.blankSocket=new retained.WebSocket('/blank');
            globalThis.parentSocket=new WebSocket('ws://{address}/parent');globalThis.echoed=false;
            parentSocket.onmessage=()=>{{echoed=true;parentSocket.close(1000);}};return true;
        }})()"#)).unwrap();
        pump_until(&mut rt,"blankSocket.readyState!==WebSocket.CONNECTING&&parentSocket.readyState!==WebSocket.CONNECTING",
            "[blankSocket.readyState,parentSocket.readyState,echoed]",&accepts,3000).await;
        assert_eq!(accepts.load(Ordering::SeqCst),2);
        let mut paths=vec![accepted.recv().await.unwrap(),accepted.recv().await.unwrap()];paths.sort();assert_eq!(paths,vec!["/blank","/parent"]);
        assert_eq!(rt.evaluate("[blankSocket.readyState,parentSocket.readyState]").unwrap(),serde_json::json!([1,1]));
        assert_eq!(rt.evaluate("(()=>{f.remove();parentSocket.send('parent-alive');document.body.appendChild(f);return [blankSocket.readyState,parentSocket.readyState,f.contentWindow!==oldBlank];})()").unwrap(),serde_json::json!([3,1,true]));
        pump_until(&mut rt,"parentSocket.readyState===WebSocket.CLOSED",
            "[blankSocket.readyState,parentSocket.readyState,echoed]",&accepts,2000).await;
        server.await.unwrap();assert_eq!(rt.evaluate("echoed").unwrap(),serde_json::json!(true));
    }).await.unwrap();
}

#[tokio::test(flavor="current_thread")]
async fn websocket_initial_blank_meta_is_local_and_inherited_policy_denies_before_tcp() {
    let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
    let mut rt=runtime(&format!("http://{address}/"));
    rt.evaluate(&format!(r#"(()=>{{globalThis.failures=[];const f=document.getElementById('child');const w=f.contentWindow;
        const meta=w.document.createElement('meta');meta.setAttribute('http-equiv','Content-Security-Policy');meta.setAttribute('content',"connect-src 'none'");w.document.head.appendChild(meta);
        const a=new w.WebSocket('ws://{address}/local');a.onerror=()=>failures.push(a.readyState);
        const m=document.createElement('meta');m.setAttribute('http-equiv','Content-Security-Policy');m.setAttribute('content',"connect-src 'none'");document.head.appendChild(m);
        const next=document.createElement('iframe');document.body.appendChild(next);const b=new next.contentWindow.WebSocket('ws://{address}/inherited');b.onerror=()=>failures.push(b.readyState);return true;
    }})()"#)).unwrap();
    rt.run_event_loop_bounded(500).await.unwrap();assert_eq!(rt.evaluate("failures").unwrap(),serde_json::json!([3,3]));
    assert!(tokio::time::timeout(std::time::Duration::from_millis(50),listener.accept()).await.is_err());
}

#[test]
fn websocket_schemeful_sites_preserve_ip_identity_and_private_suffix() {
    let site=|text|obscura_net::cookies::schemeful_site(&url::Url::parse(text).unwrap()).unwrap();
    assert_eq!(site("http://127.0.0.1/"),"http://127.0.0.1");
    assert_ne!(site("http://127.0.0.1/"),site("http://10.0.0.1/"));
    assert_eq!(site("https://[::1]/"),"https://[::1]");
    assert_ne!(site("https://a.github.io/"),site("https://b.github.io/"));
    assert_eq!(site("https://a.example.com/"),site("https://b.example.com/"));
    let jar=CookieJar::new();let url=url::Url::parse("https://a.example.com/").unwrap();
    jar.set_cookie("chip=1; Secure; SameSite=None; Partitioned",&url);
    jar.set_cookie_from_js("jschip=1; Secure; SameSite=None; Partitioned",&url);
    assert!(jar.subresource_cookie_header(&url,None).is_empty());
}

#[tokio::test(flavor="current_thread")]
async fn websocket_blob_worker_inherits_effective_dynamic_connection_policy() {
    let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
    let mut rt=runtime(&format!("http://{address}/"));
    rt.evaluate(&format!(r#"(()=>{{globalThis.workerResult=null;
        const meta=document.createElement('meta');meta.setAttribute('http-equiv','Content-Security-Policy');meta.setAttribute('content',"connect-src 'none'");document.head.appendChild(meta);
        const source="const w=new WebSocket('ws://{address}/worker');w.onopen=()=>postMessage('unexpected');w.onerror=()=>postMessage(w.readyState);";
        globalThis.worker=new Worker(URL.createObjectURL(new Blob([source],{{type:'application/javascript'}})));
        worker.onmessage=e=>{{workerResult=e.data;worker.terminate();}};return true;
    }})()"#)).unwrap();
    rt.run_event_loop_bounded(1500).await.unwrap();
    assert_eq!(rt.evaluate("workerResult===WebSocket.CLOSED").unwrap(),serde_json::json!(true));
    assert!(tokio::time::timeout(std::time::Duration::from_millis(50),listener.accept()).await.is_err());
    rt.shutdown_workers(std::time::Duration::from_secs(2)).unwrap();
}

#[tokio::test(flavor="current_thread")]
async fn websocket_fetched_provisional_frame_cannot_borrow_parent_allow_policy() {
    tokio::time::timeout(std::time::Duration::from_secs(4),async {
        let http=TcpListener::bind("127.0.0.1:0").await.unwrap();let address=http.local_addr().unwrap();
        let ws=TcpListener::bind("127.0.0.1:0").await.unwrap();let target=ws.local_addr().unwrap();
        let server=tokio::spawn(async move {let(mut tcp,_)=http.accept().await.unwrap();let mut request=Vec::new();
            while !request.ends_with(b"\r\n\r\n") {request.push(tcp.read_u8().await.unwrap());assert!(request.len()<16384);}
            let body="<html><body>response-owned</body></html>";
            tcp.write_all(format!("HTTP/1.1 200 OK\r\nContent-Security-Policy: connect-src 'none'\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
        });
        let mut rt=runtime(&format!("http://{address}/parent"));
        rt.evaluate(&format!(r#"(()=>{{globalThis.result=[];const f=document.getElementById('child');
            f.onload=()=>{{result.push('loaded');globalThis.provisional=f.contentWindow;const w=new provisional.WebSocket('ws://{target}/should-not-connect');w.onerror=()=>result.push(w.readyState);}};
            f.src='/child';return true;
        }})()"#)).unwrap();
        rt.run_event_loop_bounded(1000).await.unwrap();server.await.unwrap();
        assert_eq!(rt.evaluate("result").unwrap(),serde_json::json!(["loaded",3]));
        assert_eq!(rt.state.borrow().pending_frames.len(),1,"must exercise observable shim before native realm publication");
        assert!(tokio::time::timeout(std::time::Duration::from_millis(50),ws.accept()).await.is_err());
    }).await.unwrap();
}

#[tokio::test(flavor="current_thread")]
async fn websocket_buffer_source_brand_uses_real_slots_across_realms() {
    tokio::time::timeout(std::time::Duration::from_secs(5),async {
        let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
        let server=tokio::spawn(async move {let(mut tcp,_)=listener.accept().await.unwrap();upgrade(&mut tcp,false).await;
            let mut out=Vec::new();for _ in 0..3 {let(op,data)=frame(&mut tcp).await;out.push((op&15,data.clone()));echo(&mut tcp,op,&data).await;}
            let(op,data)=frame(&mut tcp).await;assert_eq!(op&15,8);echo(&mut tcp,op,&data).await;out
        });
        let origin=format!("http://{address}/");let mut rt=runtime(&origin);
        let child=crate::frame::FrameRealm::new(&mut rt,96,0,&origin,"<html><body></body></html>").unwrap();
        rt.evaluate(&format!(r#"(()=>{{globalThis.result=[];const w=new WebSocket('ws://{address}/');w.binaryType='arraybuffer';
            w.onopen=()=>{{const c=__obscura_frameObjects[96].window;const fake={{[Symbol.toStringTag]:'ArrayBuffer',byteLength:99999999,toString(){{return 'genuine-string';}}}};w.send(fake);
                const ab=new c.ArrayBuffer(2);new c.Uint8Array(ab).set([4,5]);w.send(ab);
                const view=new Uint8Array([9,6,7,8]).subarray(1,3);Object.defineProperties(view,{{byteLength:{{value:10000000}},byteOffset:{{value:0}},buffer:{{value:new ArrayBuffer(0)}}}});w.send(view);}};
            w.onmessage=e=>{{result.push(typeof e.data==='string'?e.data:Array.from(new Uint8Array(e.data)));if(result.length===3)w.close(1000);}};return true;
        }})()"#)).unwrap();
        rt.run_event_loop_bounded(2000).await.unwrap();assert_eq!(rt.evaluate("result").unwrap(),serde_json::json!(["genuine-string",[4,5],[6,7]]));
        assert_eq!(server.await.unwrap(),vec![(1,b"genuine-string".to_vec()),(2,vec![4,5]),(2,vec![6,7])]);drop(child);
    }).await.unwrap();
}

// Source provenance: obscura-net's configured-CA HTTP and native WSS fixtures
// use the same already-locked rcgen 0.13 / tokio-rustls 0.26 server stack.
// This fixture adds the complete runtime JS -> primp WSS -> actual peer path.
// tokio-rustls is only the owned test server, never the browser client backend.
struct FixtureRootEnv(Option<std::ffi::OsString>);
impl FixtureRootEnv {
    fn set(path:&std::path::Path)->Self {
        let old=std::env::var_os("SSL_CERT_FILE");std::env::set_var("SSL_CERT_FILE",path);Self(old)
    }
}
impl Drop for FixtureRootEnv {
    fn drop(&mut self) {
        if let Some(old)=self.0.as_ref() {std::env::set_var("SSL_CERT_FILE",old);} else {std::env::remove_var("SSL_CERT_FILE");}
    }
}
fn local_tls_fixture()->(String,tokio_rustls::TlsAcceptor) {
    let ca_key=rcgen::KeyPair::generate().unwrap();
    let mut ca_params=rcgen::CertificateParams::new(Vec::new()).unwrap();
    ca_params.is_ca=rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let ca=ca_params.self_signed(&ca_key).unwrap();
    let key=rcgen::KeyPair::generate().unwrap();
    let cert=rcgen::CertificateParams::new(vec!["127.0.0.1".into()]).unwrap().signed_by(&key,&ca,&ca_key).unwrap();
    let mut config=tokio_rustls::rustls::ServerConfig::builder_with_provider(Arc::new(tokio_rustls::rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions().unwrap().with_no_client_auth().with_single_cert(
            vec![tokio_rustls::rustls::pki_types::CertificateDer::from(cert.der().to_vec())],
            tokio_rustls::rustls::pki_types::PrivateKeyDer::Pkcs8(tokio_rustls::rustls::pki_types::PrivatePkcs8KeyDer::from(key.serialize_der()))).unwrap();
    config.alpn_protocols=vec![b"h2".to_vec(),b"http/1.1".to_vec()];
    (ca.pem(),tokio_rustls::TlsAcceptor::from(Arc::new(config)))
}

#[tokio::test(flavor="current_thread")]
async fn websocket_runtime_wss_real_ping_echo_close_and_untrusted_ca_negative() {
    tokio::time::timeout(std::time::Duration::from_secs(10),async {
        let persona=obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::MacChrome153);
        let expected_ua=persona.user_agent().to_string();
        let(root_pem,acceptor)=local_tls_fixture();
        let root=tempfile::NamedTempFile::new().unwrap();std::fs::write(root.path(),root_pem).unwrap();
        let _root_env=FixtureRootEnv::set(root.path());
        let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
        let accepts=Arc::new(AtomicUsize::new(0));let server_accepts=accepts.clone();
        let server=tokio::spawn(async move {
            let(tcp,_)=listener.accept().await.unwrap();server_accepts.fetch_add(1,Ordering::SeqCst);
            let mut tls=acceptor.accept(tcp).await.unwrap();
            assert_eq!(tls.get_ref().1.alpn_protocol(),Some(b"http/1.1".as_slice()));
            let request=upgrade(&mut tls,false).await;
            assert!(request.starts_with("GET /secure-echo HTTP/1.1\r\n"));
            let actual_ua=request.lines().find_map(|line|line.split_once(':').filter(|(name,_)|name.eq_ignore_ascii_case("user-agent")).map(|(_,value)|value.trim()));
            assert_eq!(actual_ua,Some(expected_ua.as_str()));
            assert!(expected_ua.contains("Chrome/153.")&&expected_ua.contains("Macintosh"));
            assert!(request.to_ascii_lowercase().contains(&format!("origin: https://{address}\r\n")));
            assert!(!request.to_ascii_lowercase().contains("sec-websocket-protocol:"));
            let(opcode,payload)=frame(&mut tls).await;assert_eq!(opcode,0x81);
            let ping:serde_json::Value=serde_json::from_slice(&payload).unwrap();
            assert_eq!(ping["type"],"ping");assert!(ping["clientSentAt"].as_f64().unwrap()>0.0);
            let received_at=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
            let reply=serde_json::json!({"clientSentAt":ping["clientSentAt"],"serverReceivedAt":received_at,"metadata":{"owned":true}}).to_string();
            echo(&mut tls,0x81,reply.as_bytes()).await;
            let(opcode,close)=frame(&mut tls).await;assert_eq!(opcode,0x88);
            assert_eq!(u16::from_be_bytes([close[0],close[1]]),1000);
            assert_eq!(&close[2..],b"complete");echo(&mut tls,opcode,&close).await;
            let mut tail=[0;1];
            match tls.read(&mut tail).await {
                Ok(0)=>{},
                // Dropping a successfully closed WebSocket can close TCP
                // without an additional TLS close_notify record.
                Err(error) if error.kind()==std::io::ErrorKind::UnexpectedEof=>{},
                other=>panic!("JS close did not release the actual TLS socket: {other:?}"),
            }
            ping
        });
        let mut rt=runtime_with_persona(&format!("https://{address}/page"),persona.clone());
        assert_eq!(rt.evaluate("navigator.userAgent").unwrap(),serde_json::json!(persona.user_agent()));
        assert_eq!(rt.evaluate(&format!(r#"(()=>{{
            globalThis.secureEvents=[];globalThis.secureReply=null;globalThis.secureTiming=null;
            const before=performance.now();globalThis.secureSocket=new WebSocket('wss://{address}/secure-echo');
            let opened=0;globalThis.sentAt=0;
            secureSocket.onopen=e=>{{secureEvents.push(['open',secureSocket.readyState,e.isTrusted]);opened=performance.now();sentAt=Date.now();secureSocket.send(JSON.stringify({{type:'ping',clientSentAt:sentAt}}));}};
            secureSocket.onmessage=e=>{{secureEvents.push(['message',secureSocket.readyState,e instanceof MessageEvent,e.isTrusted]);secureReply=JSON.parse(e.data);secureTiming=[opened-before,performance.now()-opened];secureSocket.close(1000,'complete');}};
            secureSocket.onerror=()=>secureEvents.push(['error',secureSocket.readyState]);
            secureSocket.onclose=e=>secureEvents.push(['close',secureSocket.readyState,e instanceof CloseEvent,e.code,e.reason,e.wasClean]);
            return [secureSocket.readyState,secureEvents.length];
        }})()"#)).unwrap(),serde_json::json!([0,0]));
        pump_until(&mut rt,"secureSocket.readyState===WebSocket.CLOSED&&secureEvents.some(e=>e[0]==='close')",
            "[secureSocket.readyState,secureEvents,secureReply,secureTiming]",&accepts,4000).await;
        assert_eq!(rt.evaluate("secureEvents").unwrap(),serde_json::json!([["open",1,true],["message",1,true,true],["close",3,true,1000,"complete",true]]));
        assert_eq!(rt.evaluate("secureReply.clientSentAt===sentAt&&secureReply.metadata.owned===true&&Number.isFinite(secureReply.serverReceivedAt)&&secureTiming.every(n=>Number.isFinite(n)&&n>=0)&&secureSocket.bufferedAmount===0").unwrap(),serde_json::json!(true));
        assert_eq!(accepts.load(Ordering::SeqCst),1);let ping=server.await.unwrap();
        assert_eq!(ping["clientSentAt"].as_f64(),rt.evaluate("sentAt").unwrap().as_f64());

        // A second independently generated CA is deliberately not installed.
        // Observe real TCP admission but a TLS error before any HTTP Upgrade.
        let(_,untrusted)=local_tls_fixture();
        let rejected=TcpListener::bind("127.0.0.1:0").await.unwrap();let bad_address=rejected.local_addr().unwrap();
        let failures=Arc::new(AtomicUsize::new(0));let server_failures=failures.clone();
        let negative=tokio::spawn(async move {
            let(tcp,_)=rejected.accept().await.unwrap();server_failures.fetch_add(1,Ordering::SeqCst);
            assert!(untrusted.accept(tcp).await.is_err(),"untrusted CA must be rejected by the actual primp TLS client before HTTP Upgrade");
        });
        rt.evaluate(&format!(r#"(()=>{{globalThis.failedEvents=[];globalThis.failedSocket=new WebSocket('wss://{bad_address}/must-not-upgrade');
            failedSocket.onopen=()=>failedEvents.push(['unexpected-open']);failedSocket.onmessage=()=>failedEvents.push(['unexpected-message']);
            failedSocket.onerror=()=>failedEvents.push(['error',failedSocket.readyState]);
            failedSocket.onclose=e=>failedEvents.push(['close',failedSocket.readyState,e.code,e.wasClean]);return true;
        }})()"#)).unwrap();
        pump_until(&mut rt,"failedEvents.some(e=>e[0]==='close')",
            "[failedSocket.readyState,failedEvents]",&failures,4000).await;
        assert_eq!(rt.evaluate("failedEvents").unwrap(),serde_json::json!([["error",3],["close",3,1006,false]]));
        assert_eq!(failures.load(Ordering::SeqCst),1);negative.await.unwrap();
    }).await.unwrap();
}
