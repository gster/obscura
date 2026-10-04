//! Proposed actual local transport fixtures. None executed by the draft author.
use super::*;
use futures_util::{SinkExt, StreamExt};
use tokio::{io::{AsyncReadExt,AsyncWriteExt},net::{TcpListener,TcpStream}};
use tokio_tungstenite::tungstenite::Message;

struct FixturePolicy { deny: bool }
impl OwnerPolicy for FixturePolicy {
    fn authorize(&self,_:&Url,http:&Url,jar:&CookieJar)->Result<Credentials,ObscuraNetError> {
        if self.deny {return Err(failed("fixture connect policy denied"))}
        // This fixture is deliberately same-site. It is not a production
        // SameSite selector or a substitute for actual owner CSP ingestion.
        Ok(Credentials {origin:http.origin().ascii_serialization(),cookie_header:jar.get_cookie_header(http),headers:Vec::new()})
    }
    fn response_cookies(&self,http:&Url,headers:&HeaderMap,jar:&CookieJar)->Result<(),ObscuraNetError> {
        for value in headers.get_all("set-cookie") {
            jar.set_cookie(value.to_str().map_err(|_|failed("invalid fixture cookie"))?,http);
        }
        Ok(())
    }
}
fn client(allow:bool)->Client {Client::new(crate::StealthProfile::MacChrome153,None,allow,None,None)}
async fn request<T: tokio::io::AsyncRead + Unpin>(stream:&mut T)->String {
    let mut data=Vec::new();let mut byte=[0;1];
    while !data.ends_with(b"\r\n\r\n") {
        assert!(data.len()<16*1024);stream.read_exact(&mut byte).await.unwrap();data.push(byte[0]);
    }
    String::from_utf8(data).unwrap()
}
fn field<'a>(request:&'a str,name:&str)->Option<&'a str> {
    request.lines().filter_map(|line|line.split_once(':')).find(|(key,_)|key.eq_ignore_ascii_case(name)).map(|(_,value)|value.trim())
}
fn response(request:&str,protocol:bool)->String {
    format!("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: keep-alive, Upgrade\r\nSec-WebSocket-Accept: {}\r\n{}Set-Cookie: echoed=yes; HttpOnly; Path=/\r\n\r\n",
        derive_accept_key(field(request,"sec-websocket-key").unwrap().as_bytes()),
        if protocol {"Sec-WebSocket-Protocol: fixture.v1\r\n"}else{""})
}

#[tokio::test]
async fn primp_upgrade_keeps_coalesced_first_frame_and_real_masked_echo() {
    tokio::time::timeout(Duration::from_secs(5),async {
        let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
        let server=tokio::spawn(async move {
            let (mut tcp,_)=listener.accept().await.unwrap();let request=request(&mut tcp).await;
            assert_eq!(field(&request,"origin"),Some(format!("http://{address}").as_str()));
            assert!(field(&request,"cookie").unwrap().contains("secret=value"));
            assert_eq!(field(&request,"sec-websocket-protocol"),Some("fixture.v1"));
            assert!(request.starts_with("GET /echo HTTP/1.1\r\n"));
            let mut joined=response(&request,true).into_bytes();joined.extend_from_slice(b"\x81\x05hello");
            tcp.write_all(&joined).await.unwrap();
            let mut framed=WebSocketStream::from_raw_socket(tcp,Role::Server,Some(WebSocketConfig::default().max_message_size(Some(MAX_MESSAGE)))).await;
            let text=framed.next().await.unwrap().unwrap();assert_eq!(text,Message::Text("exact α".into()));framed.send(text).await.unwrap();
            let binary=framed.next().await.unwrap().unwrap();assert_eq!(binary,Message::Binary(vec![0,1,255].into()));framed.send(binary).await.unwrap();
            assert!(matches!(framed.next().await.unwrap().unwrap(),Message::Close(_)));let _=framed.flush().await;
        });
        let jar=CookieJar::new();let http=Url::parse(&format!("http://{address}/echo")).unwrap();jar.set_cookie("secret=value; HttpOnly; Path=/",&http);
        let (_alive,mut cancel)=watch::channel(false);let transport=client(true);let policy=FixturePolicy{deny:false};
        let mut opened=open(&transport,&jar,true,&Url::parse(&format!("ws://{address}/echo")).unwrap(),&["fixture.v1".into()],Some(&policy),&mut cancel).await.unwrap();
        assert_eq!(opened.protocol,"fixture.v1");assert_eq!(opened.url.scheme(),"ws");
        assert_eq!(opened.stream.next().await.unwrap().unwrap(),Message::Text("hello".into()));
        opened.stream.send(Message::Text("exact α".into())).await.unwrap();assert_eq!(opened.stream.next().await.unwrap().unwrap(),Message::Text("exact α".into()));
        opened.stream.send(Message::Binary(vec![0,1,255].into())).await.unwrap();assert_eq!(opened.stream.next().await.unwrap().unwrap(),Message::Binary(vec![0,1,255].into()));
        opened.stream.close(None).await.unwrap();server.await.unwrap();
        assert!(jar.get_cookie_header(&http).contains("echoed=yes"));
        assert!(!jar.get_js_visible_cookies(&http).contains("secret=value"));
    }).await.expect("bounded actual echo fixture");
}

#[tokio::test]
async fn missing_or_denied_owner_policy_performs_no_connection() {
    let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let url=Url::parse(&format!("ws://{}/",listener.local_addr().unwrap())).unwrap();
    let transport=client(true);let jar=CookieJar::new();let (_alive,mut cancel)=watch::channel(false);
    assert!(open(&transport,&jar,true,&url,&[],None,&mut cancel).await.is_err());
    assert!(open(&transport,&jar,true,&url,&[],Some(&FixturePolicy{deny:true}),&mut cancel).await.is_err());
    assert!(tokio::time::timeout(Duration::from_millis(40),listener.accept()).await.is_err());
}

#[tokio::test]
async fn mapped_ws_private_url_is_denied_before_dial_without_opt_in() {
    assert!(!crate::env_allows_private_network(),"negative fixture requires default private-network policy");
    let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
    let transport=client(false);let jar=CookieJar::new();let (_alive,mut cancel)=watch::channel(false);
    let url=Url::parse(&format!("ws://{address}/")).unwrap();
    let error=match open(&transport,&jar,false,&url,&[],Some(&FixturePolicy{deny:false}),&mut cancel).await { Err(error)=>error,Ok(_)=>panic!("private target connected") };
    assert!(error.to_string().contains("private/internal IP address"));
    assert!(tokio::time::timeout(Duration::from_millis(30),listener.accept()).await.is_err());
    // Exactly the same live endpoint succeeds when the actual context opts in.
    let server=tokio::spawn(async move {let(mut tcp,_)=listener.accept().await.unwrap();let request=request(&mut tcp).await;tcp.write_all(response(&request,false).as_bytes()).await.unwrap();});
    let allowed=client(true);assert!(open(&allowed,&jar,true,&url,&[],Some(&FixturePolicy{deny:false}),&mut cancel).await.is_ok());server.await.unwrap();
}

#[tokio::test]
async fn owner_retirement_cancels_pending_real_upgrade_and_releases_tcp() {
    tokio::time::timeout(Duration::from_secs(5),async {
        let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let url=Url::parse(&format!("ws://{}/",listener.local_addr().unwrap())).unwrap();
        let (started_tx,started_rx)=tokio::sync::oneshot::channel();
        let server=tokio::spawn(async move {
            let (mut tcp,_)=listener.accept().await.unwrap();let _=request(&mut tcp).await;started_tx.send(()).unwrap();
            let mut byte=[0;1];assert_eq!(tcp.read(&mut byte).await.unwrap(),0,"cancel must release actual pending socket");
        });
        let (retire,mut cancel)=watch::channel(false);
        let pending=tokio::spawn(async move {open(&client(true),&CookieJar::new(),true,&url,&[],Some(&FixturePolicy{deny:false}),&mut cancel).await});
        started_rx.await.unwrap();retire.send(true).unwrap();assert!(pending.await.unwrap().is_err());server.await.unwrap();
    }).await.expect("bounded pending-upgrade retirement fixture");
}

#[test]
fn handshake_validation_rejects_invalid_status_accept_protocol_and_extensions() {
    let key="dGhlIHNhbXBsZSBub25jZQ==";
    let mut headers=HeaderMap::new();
    headers.insert("upgrade",HeaderValue::from_static("websocket"));headers.insert("connection",HeaderValue::from_static("keep-alive, Upgrade"));
    headers.insert("sec-websocket-accept",HeaderValue::from_static("s3pPLMBiTxaQ9kYGzzhZRbK+xOo="));
    assert!(validate_response(StatusCode::SWITCHING_PROTOCOLS,&headers,key,&[]).is_ok());
    for status in [StatusCode::OK,StatusCode::FOUND,StatusCode::UNAUTHORIZED] {assert!(validate_response(status,&headers,key,&[]).is_err());}
    assert!(validate_response(StatusCode::SWITCHING_PROTOCOLS,&headers,key,&["fixture.v1".into()]).is_err());
    headers.insert("sec-websocket-protocol",HeaderValue::from_static("fixture.v1"));
    assert_eq!(validate_response(StatusCode::SWITCHING_PROTOCOLS,&headers,key,&["fixture.v1".into()]).unwrap(),"fixture.v1");
    assert!(validate_response(StatusCode::SWITCHING_PROTOCOLS,&headers,key,&[]).is_err());headers.remove("sec-websocket-protocol");
    headers.insert("sec-websocket-extensions",HeaderValue::from_static("permessage-deflate"));assert!(validate_response(StatusCode::SWITCHING_PROTOCOLS,&headers,key,&[]).is_err());headers.remove("sec-websocket-extensions");
    headers.append("sec-websocket-accept",HeaderValue::from_static("s3pPLMBiTxaQ9kYGzzhZRbK+xOo="));assert!(validate_response(StatusCode::SWITCHING_PROTOCOLS,&headers,key,&[]).is_err());
}

#[test]
fn protocol_and_url_validation_preserve_real_inputs() {
    assert!(!protocols_valid(&["same".into(),"same".into()]));assert!(!protocols_valid(&["bad token".into()]));
    assert!(protocols_valid(&["same".into(),"Same".into()]));
    assert!(mapped_url(&Url::parse("ws://example.test/#fragment").unwrap()).is_err());
    assert_eq!(mapped_url(&Url::parse("ws://user:pass@example.test/").unwrap()).unwrap().0.as_str(),"ws://user:pass@example.test/");
    let (socket,http)=mapped_url(&Url::parse("https://example.test:8443/a?q=1").unwrap()).unwrap();
    assert_eq!(socket.as_str(),"wss://example.test:8443/a?q=1");assert_eq!(http.as_str(),"https://example.test:8443/a?q=1");
}

#[tokio::test]
async fn websocket_wss_uses_configured_ca_and_http1_alpn_with_real_echo() {
    use std::sync::Arc;
    tokio::time::timeout(Duration::from_secs(6),async {
        let ca_key=rcgen::KeyPair::generate().unwrap();let mut ca_params=rcgen::CertificateParams::new(Vec::new()).unwrap();
        ca_params.is_ca=rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);let ca=ca_params.self_signed(&ca_key).unwrap();
        let key=rcgen::KeyPair::generate().unwrap();let cert=rcgen::CertificateParams::new(vec!["127.0.0.1".into()]).unwrap().signed_by(&key,&ca,&ca_key).unwrap();
        let root=tempfile::NamedTempFile::new().unwrap();std::fs::write(root.path(),ca.pem()).unwrap();
        std::env::set_var("SSL_CERT_FILE",root.path());
        let mut config=tokio_rustls::rustls::ServerConfig::builder_with_provider(Arc::new(tokio_rustls::rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions().unwrap().with_no_client_auth().with_single_cert(
                vec![tokio_rustls::rustls::pki_types::CertificateDer::from(cert.der().to_vec())],
                tokio_rustls::rustls::pki_types::PrivateKeyDer::Pkcs8(tokio_rustls::rustls::pki_types::PrivatePkcs8KeyDer::from(key.serialize_der()))).unwrap();
        config.alpn_protocols=vec![b"h2".to_vec(),b"http/1.1".to_vec()];
        let acceptor=tokio_rustls::TlsAcceptor::from(Arc::new(config));
        let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
        let server=tokio::spawn(async move {
            let(tcp,_)=listener.accept().await.unwrap();let mut tls=acceptor.accept(tcp).await.unwrap();
            assert_eq!(tls.get_ref().1.alpn_protocol(),Some(b"http/1.1".as_slice()));
            let request=request(&mut tls).await;tls.write_all(response(&request,false).as_bytes()).await.unwrap();
            let mut socket=WebSocketStream::from_raw_socket(tls,Role::Server,None).await;
            let message=socket.next().await.unwrap().unwrap();assert_eq!(message,Message::Text("encrypted echo".into()));socket.send(message).await.unwrap();
            assert!(matches!(socket.next().await.unwrap().unwrap(),Message::Close(_)));let _=socket.flush().await;
        });
        let transport=client(true);let jar=CookieJar::new();let(_alive,mut cancel)=watch::channel(false);
        let mut opened=open(&transport,&jar,true,&Url::parse(&format!("wss://{address}/" )).unwrap(),&[],Some(&FixturePolicy{deny:false}),&mut cancel).await.unwrap();
        opened.stream.send(Message::Text("encrypted echo".into())).await.unwrap();assert_eq!(opened.stream.next().await.unwrap().unwrap(),Message::Text("encrypted echo".into()));
        opened.stream.close(None).await.unwrap();server.await.unwrap();std::env::remove_var("SSL_CERT_FILE");
    }).await.unwrap();
}

#[tokio::test]
async fn websocket_http_error_still_receives_contextual_response_cookie() {
    let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
    let server=tokio::spawn(async move {let(mut tcp,_)=listener.accept().await.unwrap();request(&mut tcp).await;
        tcp.write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nSet-Cookie: challenge=received; HttpOnly; Path=/\r\nConnection: close\r\n\r\n").await.unwrap();});
    let transport=client(true);let jar=CookieJar::new();let(_alive,mut cancel)=watch::channel(false);
    assert!(open(&transport,&jar,true,&Url::parse(&format!("ws://{address}/")).unwrap(),&[],Some(&FixturePolicy{deny:false}),&mut cancel).await.is_err());
    assert_eq!(jar.get_cookie_header(&Url::parse(&format!("http://{address}/next")).unwrap()),"challenge=received");
    assert!(jar.get_js_visible_cookies(&Url::parse(&format!("http://{address}/next")).unwrap()).is_empty());server.await.unwrap();
}

#[tokio::test]
async fn websocket_session_idle_owner_retirement_and_zero_message_queue_are_bounded() {
    use std::sync::Arc;
    use super::session::{Owner,RuntimeBudget,Event};
    tokio::time::timeout(Duration::from_secs(5),async {
        let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
        let server=tokio::spawn(async move {let(mut tcp,_)=listener.accept().await.unwrap();let request=request(&mut tcp).await;
            tcp.write_all(response(&request,false).as_bytes()).await.unwrap();let mut byte=[0];assert_eq!(tcp.read(&mut byte).await.unwrap(),0);});
        let owner=Arc::new(Owner::new(Arc::new(RuntimeBudget::default())));
        let child=Owner::child(&owner);
        let persona=crate::EffectivePersona::builtin(crate::StealthProfile::WindowsChrome145);
        let client=Arc::new(crate::StealthHttpClient::with_proxy(Arc::new(CookieJar::new()),None,true,&persona));
        let session=super::session::start(client,child.clone(),Arc::new(FixturePolicy{deny:false}),Url::parse(&format!("ws://{address}/")).unwrap(),Vec::new()).unwrap();
        assert!(matches!(session.next().await.unwrap().event,Event::Open(_)));
        // Do not yield to the writer. Empty messages consume actual queue slots.
        for _ in 0..64 {session.send(&[],false).unwrap();}
        assert!(session.send(&[],false).is_err());
        owner.retire();assert!(!child.active());assert!(session.next().await.is_none());server.await.unwrap();
        tokio::task::yield_now().await;assert_eq!(child.usage(),(0,0,0));assert_eq!(session.buffered(),0);
    }).await.unwrap();
}

#[tokio::test]
async fn websocket_retained_delivery_and_session_release_all_native_budget_on_retire() {
    use super::session::{Owner,RuntimeBudget,Event};use std::sync::Arc;
    tokio::time::timeout(Duration::from_secs(5),async {
        let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
        let server=tokio::spawn(async move {let(mut tcp,_)=listener.accept().await.unwrap();let req=request(&mut tcp).await;
            tcp.write_all(response(&req,false).as_bytes()).await.unwrap();
            tcp.write_all(&[0x81,127,0,0,0,0,0,16,0,0]).await.unwrap();tcp.write_all(&vec![0;MAX_MESSAGE]).await.unwrap();
            let mut byte=[0];assert_eq!(tcp.read(&mut byte).await.unwrap(),0);
        });
        let owner=Arc::new(Owner::new(Arc::new(RuntimeBudget::default())));
        let persona=crate::EffectivePersona::builtin(crate::StealthProfile::WindowsChrome145);
        let client=Arc::new(crate::StealthHttpClient::with_proxy(Arc::new(CookieJar::new()),None,true,&persona));
        let session=super::session::start(client,owner.clone(),Arc::new(FixturePolicy{deny:false}),Url::parse(&format!("ws://{address}/")).unwrap(),vec![]).unwrap();
        assert!(matches!(session.next().await.unwrap().event,Event::Open(_)));
        let delivery=session.next().await.unwrap();assert!(matches!(&delivery.event,Event::Text(value) if value.len()==MAX_MESSAGE));
        session.hold_delivery(delivery);assert_eq!(session.payload().len(),MAX_MESSAGE);assert!(owner.usage().0>0);
        owner.retire();assert!(session.payload().is_empty());server.await.unwrap();tokio::task::yield_now().await;
        assert_eq!(owner.usage(),(0,0,0));assert!(session.next().await.is_none());
    }).await.unwrap();
}

#[tokio::test]
async fn websocket_close_deadline_starts_while_peer_blocks_an_inflight_write() {
    use super::session::{Owner,RuntimeBudget,Event};use std::sync::Arc;
    tokio::time::timeout(Duration::from_secs(10),async {
        let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
        let server=tokio::spawn(async move {let(mut tcp,_)=listener.accept().await.unwrap();
            socket2::SockRef::from(&tcp).set_recv_buffer_size(1024).unwrap();
            let req=request(&mut tcp).await;tcp.write_all(response(&req,false).as_bytes()).await.unwrap();
            tokio::time::sleep(Duration::from_millis(5700)).await;
            let mut bytes=Vec::new();tcp.read_to_end(&mut bytes).await.unwrap();assert!(!bytes.is_empty(),"write must reach peer before being blocked");
        });
        let owner=Arc::new(Owner::new(Arc::new(RuntimeBudget::default())));
        let persona=crate::EffectivePersona::builtin(crate::StealthProfile::WindowsChrome145);
        let client=Arc::new(crate::StealthHttpClient::with_proxy(Arc::new(CookieJar::new()),None,true,&persona));
        let session=super::session::start(client,owner.clone(),Arc::new(FixturePolicy{deny:false}),Url::parse(&format!("ws://{address}/")).unwrap(),vec![]).unwrap();
        assert!(matches!(session.next().await.unwrap().event,Event::Open(_)));
        let payload=vec![7;MAX_MESSAGE];
        for _ in 0..8 {session.send(&payload,false).unwrap();tokio::time::sleep(Duration::from_millis(25)).await;if session.buffered()>0 {break;}}
        assert!(session.buffered()>0,"fixture must actually block a pending write");
        session.close(Some(1000),"stop".into()).unwrap();
        tokio::time::sleep(Duration::from_millis(5300)).await;
        assert_eq!(session.buffered(),0);assert_eq!(owner.usage(),(0,0,0));
        assert!(matches!(session.next().await.unwrap().event,Event::Error));
        assert!(matches!(session.next().await.unwrap().event,Event::Close{code:1006,clean:false,..}));
        server.await.unwrap();
    }).await.unwrap();
}

#[tokio::test]
async fn websocket_oversized_failed_response_is_bounded_before_cookie_side_effects() {
    tokio::time::timeout(Duration::from_secs(3),async {
        let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
        let server=tokio::spawn(async move {let(mut tcp,_)=listener.accept().await.unwrap();let _=request(&mut tcp).await;
            let mut response=String::from("HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nSet-Cookie: oversized=forbidden; Path=/\r\n");
            for _ in 0..64 {response.push_str(&format!("X-Padding: {}\r\n","x".repeat(520)));}response.push_str("\r\n");
            let _=tcp.write_all(response.as_bytes()).await;
        });
        let jar=CookieJar::new();let(_alive,mut cancel)=watch::channel(false);let url=Url::parse(&format!("ws://{address}/")).unwrap();
        let result=open(&client(true),&jar,true,&url,&[],Some(&FixturePolicy{deny:false}),&mut cancel).await;
        assert!(result.is_err());assert!(jar.get_cookie_header(&Url::parse(&format!("http://{address}/")).unwrap()).is_empty());server.await.unwrap();
        let mut headers=HeaderMap::new();headers.insert("x-padding",HeaderValue::from_str(&"x".repeat(32769)).unwrap());
        assert!(validate_header_budget(&headers).unwrap_err().to_string().contains("headers exceed limit"));
    }).await.unwrap();
}
