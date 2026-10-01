use std::io::{Read, Write};
use std::time::Duration;
use bytes::Bytes;
use futures_util::StreamExt;

const REUSABLE_SIZES: [usize; 7] = [0, 1, 355, 1024, 1025, 65539, 262151];
const KNOWN_EOF_CASE: usize = REUSABLE_SIZES.len();
const UNKNOWN_EOF_CASE: usize = KNOWN_EOF_CASE + 1;
const EMPTY_STREAM_CASE: usize = UNKNOWN_EOF_CASE + 1;
const TRAILER_CASE: usize = EMPTY_STREAM_CASE + 1;
const RETRY_CASE_START: usize = TRAILER_CASE + 1;
const RETRY_ATTEMPTS: usize = 3;

fn body_bytes(size: usize) -> Vec<u8> {
    (0..size).map(|index| (index.wrapping_mul(31) ^ (index >> 8) ^ (index >> 16)) as u8).collect()
}

#[derive(Debug, Default)]
struct RequestFrames {
    frames: Vec<(u8, u8, usize)>,
    body: Vec<u8>,
}

fn write_frame(socket: &mut std::net::TcpStream, kind: u8, flags: u8, stream: u32, payload: &[u8]) {
    socket.write_all(&(payload.len() as u32).to_be_bytes()[1..]).unwrap();
    socket.write_all(&[kind, flags]).unwrap();
    socket.write_all(&stream.to_be_bytes()).unwrap();
    socket.write_all(payload).unwrap();
}

#[tokio::test]
async fn h2_body_framing_preserves_eof_flow_control_trailers_and_retry_bytes() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/upload", listener.local_addr().unwrap());
    let (first_chunk_sent, first_chunk_received) = tokio::sync::oneshot::channel();
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut preface = [0; 24];
        socket.read_exact(&mut preface).unwrap();
        assert_eq!(&preface, b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n");
        write_frame(&mut socket, 4, 0, 0, &[]);
        let mut indices = std::collections::HashMap::new();
        let mut requests: Vec<RequestFrames> = Vec::new();
        let mut first_chunk_sent = Some(first_chunk_sent);
        loop {
            let mut head = [0; 9];
            socket.read_exact(&mut head).unwrap();
            let size = u32::from_be_bytes([0, head[0], head[1], head[2]]) as usize;
            let kind = head[3]; let flags = head[4];
            let stream = u32::from_be_bytes(head[5..].try_into().unwrap()) & 0x7fffffff;
            let mut payload = vec![0; size];
            socket.read_exact(&mut payload).unwrap();
            if kind == 4 && flags & 1 == 0 { write_frame(&mut socket, 4, 1, 0, &[]); }
            if !matches!(kind, 0 | 1 | 9) { continue; }
            let index = *indices.entry(stream).or_insert_with(|| {
                requests.push(RequestFrames::default());
                requests.len() - 1
            });
            requests[index].frames.push((kind, flags, size));
            if kind == 0 {
                assert_eq!(flags & 8, 0, "fixture expects unpadded DATA");
                requests[index].body.extend_from_slice(&payload);
                if size > 0 {
                    let capacity = (size as u32).to_be_bytes();
                    write_frame(&mut socket, 8, 0, 0, &capacity);
                    write_frame(&mut socket, 8, 0, stream, &capacity);
                }
                if index == UNKNOWN_EOF_CASE && size > 0 {
                    if let Some(sender) = first_chunk_sent.take() { sender.send(()).unwrap(); }
                }
            }
            if matches!(kind, 0 | 1) && flags & 1 != 0 {
                if (RETRY_CASE_START..RETRY_CASE_START + RETRY_ATTEMPTS - 1).contains(&index) {
                    write_frame(&mut socket, 3, 0, stream, &7_u32.to_be_bytes());
                } else {
                    write_frame(&mut socket, 1, 5, stream, &[0x88]);
                }
                if index == RETRY_CASE_START + RETRY_ATTEMPTS - 1 { break; }
            }
        }
        requests
    });
    let client = primp::Client::builder().no_proxy().http2_prior_knowledge()
        .timeout(Duration::from_secs(5)).build().unwrap();
    for size in REUSABLE_SIZES {
        assert_eq!(client.post(&url).body(body_bytes(size)).send().await.unwrap().status(), http::StatusCode::OK);
    }
    let full = primp::Body::wrap(http_body_util::Full::new(Bytes::from(vec![b'f'; 355])));
    assert_eq!(client.post(&url).body(full).send().await.unwrap().status(), http::StatusCode::OK);
    let first = futures_util::stream::once(async { Ok::<_, std::io::Error>(Bytes::from_static(b"first")) });
    let second = futures_util::stream::once(async move {
        // A lookahead implementation would deadlock: the second chunk is not
        // available until the server has received the first DATA frame.
        first_chunk_received.await.unwrap();
        Ok::<_, std::io::Error>(Bytes::from_static(b"second"))
    });
    let unknown = primp::Body::wrap_stream(first.chain(second));
    assert_eq!(client.post(&url).body(unknown).send().await.unwrap().status(), http::StatusCode::OK);
    let empty = primp::Body::wrap_stream(futures_util::stream::iter([Ok::<_, std::io::Error>(Bytes::new())]));
    assert_eq!(client.post(&url).body(empty).send().await.unwrap().status(), http::StatusCode::OK);
    let mut trailers = http::HeaderMap::new();
    trailers.insert("x-final", http::HeaderValue::from_static("preserved"));
    let frames = futures_util::stream::iter([
        Ok::<_, std::io::Error>(http_body::Frame::data(Bytes::from_static(b"payload"))),
        Ok(http_body::Frame::trailers(trailers)),
    ]);
    let body = primp::Body::wrap(http_body_util::StreamBody::new(frames));
    assert_eq!(client.post(&url).body(body).send().await.unwrap().status(), http::StatusCode::OK);
    assert_eq!(client.post(&url).body(vec![b'r'; 355]).send().await.unwrap().status(), http::StatusCode::OK);
    let requests = server.join().unwrap();
    assert_eq!(requests.len(), RETRY_CASE_START + RETRY_ATTEMPTS);
    let data_frames = |row: &RequestFrames| row.frames.iter().copied().filter(|frame| frame.0 == 0).collect::<Vec<_>>();
    for (row, size) in requests.iter().zip(REUSABLE_SIZES) {
        assert_eq!(row.body, body_bytes(size));
        let data = data_frames(row);
        if size == 0 {
            assert!(data.is_empty());
            assert_ne!(row.frames[0].1 & 1, 0, "empty reusable body closes HEADERS");
        } else {
            assert!(data.iter().all(|frame| frame.2 > 0), "no empty terminal DATA: {row:?}");
            assert_eq!(data.iter().filter(|frame| frame.1 & 1 != 0).count(), 1);
            assert_ne!(data.last().unwrap().1 & 1, 0);
        }
    }
    assert_eq!(requests[KNOWN_EOF_CASE].body, vec![b'f'; 355]);
    assert_eq!(data_frames(&requests[KNOWN_EOF_CASE]), [(0, 1, 355)]);
    assert_eq!(requests[UNKNOWN_EOF_CASE].body, b"firstsecond");
    assert_eq!(data_frames(&requests[UNKNOWN_EOF_CASE]), [(0, 0, 5), (0, 0, 6), (0, 1, 0)]);
    assert!(requests[EMPTY_STREAM_CASE].body.is_empty());
    assert_eq!(data_frames(&requests[EMPTY_STREAM_CASE]), [(0, 1, 0)]);
    assert_eq!(requests[TRAILER_CASE].body, b"payload");
    assert_eq!(data_frames(&requests[TRAILER_CASE]), [(0, 0, 7)]);
    assert_eq!(requests[TRAILER_CASE].frames.iter().filter(|frame| frame.0 == 1).count(), 2);
    assert_eq!(requests[TRAILER_CASE].frames.last().unwrap().0, 1, "trailers close on HEADERS");
    assert_ne!(requests[TRAILER_CASE].frames.last().unwrap().1 & 1, 0);
    for row in &requests[RETRY_CASE_START..] {
        assert_eq!(row.body, vec![b'r'; 355], "every refused-stream retry keeps its bytes");
        assert_eq!(data_frames(row), [(0, 1, 355)]);
    }
}

#[tokio::test]
async fn h2_body_framing_preserves_decoded_trailer_fields() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/trailers", listener.local_addr().unwrap());
    let (client_done_send, client_done) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut connection = h2_fixture::server::handshake(socket).await.unwrap();
        let (request, mut response) = connection.accept().await.unwrap().unwrap();
        let mut handler = tokio::spawn(async move {
            let mut body = request.into_body();
            let mut bytes = Vec::new();
            while let Some(chunk) = body.data().await {
                let chunk = chunk.unwrap();
                bytes.extend_from_slice(&chunk);
                body.flow_control().release_capacity(chunk.len()).unwrap();
            }
            let trailers = body.trailers().await.unwrap().unwrap();
            response.send_response(http::Response::builder().status(200).body(()).unwrap(), true).unwrap();
            client_done.await.unwrap();
            (bytes, trailers)
        });
        loop {
            tokio::select! {
                result = &mut handler => break result.unwrap(),
                request = connection.accept() => { panic!("unexpected extra request or close: {request:?}"); }
            }
        }
    });
    let mut trailers = http::HeaderMap::new();
    trailers.insert("x-final", http::HeaderValue::from_static("preserved"));
    trailers.append("x-repeated", http::HeaderValue::from_static("first"));
    trailers.append("x-repeated", http::HeaderValue::from_static("second"));
    let frames = futures_util::stream::iter([
        Ok::<_, std::io::Error>(http_body::Frame::data(Bytes::from_static(b"before-trailers"))),
        Ok(http_body::Frame::trailers(trailers.clone())),
    ]);
    let client = primp::Client::builder().no_proxy().http2_prior_knowledge()
        .timeout(Duration::from_secs(5)).build().unwrap();
    let body = primp::Body::wrap(http_body_util::StreamBody::new(frames));
    assert_eq!(client.post(url).body(body).send().await.unwrap().status(), http::StatusCode::OK);
    client_done_send.send(()).unwrap();
    let (received, received_trailers) = server.await.unwrap();
    assert_eq!(received, b"before-trailers");
    assert_eq!(received_trailers, trailers);
}
