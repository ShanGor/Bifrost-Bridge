//! Real sockets: a response must arrive before the origin closes its SSE stream.
use bifrost_bridge::{Config, ProxyFactory};
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    task::JoinHandle,
    time::{Duration, timeout},
};
use tokio_util::sync::CancellationToken;

struct Running {
    addr: std::net::SocketAddr,
    task: JoinHandle<()>,
}
impl Drop for Running {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn origin() -> (Running, Arc<AtomicUsize>, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let accepts = Arc::new(AtomicUsize::new(0));
    let closed = Arc::new(AtomicUsize::new(0));
    let (a, c) = (accepts.clone(), closed.clone());
    let task = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            a.fetch_add(1, Ordering::SeqCst);
            let c = c.clone();
            tokio::spawn(async move {
                loop {
                    let mut request = Vec::new();
                    let mut byte = [0u8; 1];
                    while !request.ends_with(b"\r\n\r\n") {
                        if socket.read(&mut byte).await.unwrap_or(0) == 0 {
                            return;
                        }
                        request.push(byte[0]);
                    }
                    let request = String::from_utf8(request).unwrap();
                    if request.contains("/no-headers") {
                        let _ = socket.read(&mut byte).await;
                        c.fetch_add(1, Ordering::SeqCst);
                        return;
                    }
                    let finite = request.contains("/finite") || request.contains("/close");
                    // Assert request metadata survives proxying; deliberately nominate
                    // a private response header that must not cross the next hop.
                    if request.contains("/metadata") {
                        assert!(request.to_lowercase().contains("last-event-id: 42"));
                        assert!(request.contains("token=abc"));
                    }
                    if socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: Text/Event-Stream; charset=utf-8\r\nTransfer-Encoding: chunked\r\nConnection: keep-alive, x-private\r\nx-private: secret\r\nCache-Control: no-cache\r\n\r\n").await.is_err() { return; }
                    let event = b"id: 42\ndata: hello\n\n";
                    let chunk = format!("{:x}\r\n", event.len());
                    if socket.write_all(chunk.as_bytes()).await.is_err() {
                        return;
                    }
                    if socket.write_all(event).await.is_err() {
                        return;
                    }
                    if socket.write_all(b"\r\n").await.is_err() {
                        return;
                    }
                    if finite {
                        let _ = socket.write_all(b"0\r\n\r\n").await;
                        if request.contains("/close") {
                            return;
                        }
                        continue;
                    }
                    // Leave stream open and observe downstream cancellation.
                    let _ = socket.read(&mut byte).await;
                    c.fetch_add(1, Ordering::SeqCst);
                    return;
                }
            });
        }
    });
    (Running { addr, task }, accepts, closed)
}

async fn proxy(value: serde_json::Value) -> Running {
    let config: Config = serde_json::from_value(value).unwrap();
    let proxy = ProxyFactory::create_proxy(config).unwrap();
    let listener = Arc::new(TcpListener::bind("127.0.0.1:0").await.unwrap());
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        proxy.run(listener, CancellationToken::new()).await.unwrap();
    });
    Running { addr, task }
}
fn reverse_config(addr: std::net::SocketAddr) -> serde_json::Value {
    json!({"mode":"Reverse", "listen_addr":"127.0.0.1:0",
        "reverse_proxy_target":format!("http://{addr}"),
        "max_connection_lifetime_secs":0,
        "reverse_proxy_config":{"pool_max_idle_per_host":4,"pool_idle_timeout_secs":1,
            "response_header_timeout_secs":1,"stream_idle_timeout_secs":1},
        "monitoring":{"enabled":false}})
}
fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .http1_only()
        .build()
        .unwrap()
}
async fn wait_counter(counter: &AtomicUsize, expected: usize) {
    timeout(Duration::from_secs(3), async {
        while counter.load(Ordering::SeqCst) < expected {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn reverse_streams_metadata_and_releases_on_client_disconnect() {
    let (origin, accepts, closed) = origin().await;
    let proxy = proxy(reverse_config(origin.addr)).await;
    let mut response = timeout(
        Duration::from_secs(2),
        client()
            .get(format!("http://{}/metadata?token=abc", proxy.addr))
            .header("Last-Event-ID", "42")
            .send(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(response.status(), 200);
    assert!(!response.headers().contains_key("x-private"));
    assert_eq!(response.headers()["cache-control"], "no-cache");
    assert_eq!(response.headers()["x-accel-buffering"], "no");
    let first = timeout(Duration::from_millis(500), response.chunk())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(first, "id: 42\ndata: hello\n\n");
    // An occupied HTTP/1 SSE connection must not block an unrelated request.
    let other = client()
        .get(format!("http://{}/finite", proxy.addr))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(other.contains("hello"));
    assert_eq!(accepts.load(Ordering::SeqCst), 2);
    drop(response);
    wait_counter(&closed, 1).await;
}

#[tokio::test]
async fn reverse_reuses_completed_streams_evicts_idle_and_recovers_closed_connections() {
    let (origin, accepts, _) = origin().await;
    let proxy = proxy(reverse_config(origin.addr)).await;
    let client = client();
    for _ in 0..2 {
        let text = client
            .get(format!("http://{}/finite", proxy.addr))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert!(text.contains("hello"));
    }
    assert_eq!(
        accepts.load(Ordering::SeqCst),
        1,
        "completed SSE connection should be reused"
    );
    tokio::time::sleep(Duration::from_millis(1250)).await;
    client
        .get(format!("http://{}/close", proxy.addr))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(
        accepts.load(Ordering::SeqCst),
        2,
        "idle upstream should be evicted"
    );
    tokio::time::sleep(Duration::from_millis(30)).await;
    client
        .get(format!("http://{}/finite", proxy.addr))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(
        accepts.load(Ordering::SeqCst),
        3,
        "closed socket must not poison subsequent requests"
    );
}

#[tokio::test]
async fn reverse_bounds_header_wait_and_stream_silence() {
    let (origin, _, closed) = origin().await;
    let proxy = proxy(reverse_config(origin.addr)).await;
    let client = client();
    client
        .get(format!("http://{}/finite", proxy.addr))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let response = timeout(
        Duration::from_secs(3),
        client
            .get(format!("http://{}/no-headers", proxy.addr))
            .send(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(response.status(), 504);
    let mut response = client
        .get(format!("http://{}/hold", proxy.addr))
        .send()
        .await
        .unwrap();
    response.chunk().await.unwrap().unwrap();
    assert!(
        timeout(Duration::from_secs(3), response.chunk())
            .await
            .unwrap()
            .is_err()
    );
    wait_counter(&closed, 2).await;
    assert_eq!(
        client
            .get(format!("http://{}/finite", proxy.addr))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
}

#[tokio::test]
async fn zero_lifetime_and_zero_idle_allow_long_silent_stream() {
    let (origin, _, _) = origin().await;
    let mut config = reverse_config(origin.addr);
    config["reverse_proxy_config"]["stream_idle_timeout_secs"] = json!(0);
    let proxy = proxy(config).await;
    let mut response = client()
        .get(format!("http://{}/hold", proxy.addr))
        .send()
        .await
        .unwrap();
    response.chunk().await.unwrap().unwrap();
    assert!(
        timeout(Duration::from_millis(1300), response.chunk())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn forward_and_relay_deliver_events_without_waiting_for_eof() {
    let (origin, _, closed) = origin().await;
    let direct = proxy(json!({"mode":"Forward","listen_addr":"127.0.0.1:0",
        "max_connection_lifetime_secs":0,"monitoring":{"enabled":false},
        "forward_proxy_config":{"response_header_timeout_secs":1,"stream_idle_timeout_secs":1}}))
    .await;
    for relay in [false, true] {
        let outer = if relay {
            Some(proxy(json!({"mode":"Forward","listen_addr":"127.0.0.1:0",
                "max_connection_lifetime_secs":0,"monitoring":{"enabled":false},
                "relay_proxies":[{"relay_proxy_url":format!("http://{}",direct.addr),"relay_proxy_domains":["127.0.0.1"]}]})).await)
        } else {
            None
        };
        let addr = outer.as_ref().map_or(direct.addr, |p| p.addr);
        let client = reqwest::Client::builder()
            .proxy(reqwest::Proxy::http(format!("http://{addr}")).unwrap())
            .build()
            .unwrap();
        let mut response = timeout(
            Duration::from_secs(2),
            client.get(format!("http://{}/hold", origin.addr)).send(),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(response.status(), 200);
        assert!(!response.headers().contains_key("x-private"));
        assert!(
            timeout(Duration::from_millis(500), response.chunk())
                .await
                .unwrap()
                .unwrap()
                .unwrap()
                .starts_with(b"id: 42")
        );
        drop(response);
        wait_counter(&closed, if relay { 2 } else { 1 }).await;
    }
}

#[tokio::test]
async fn forward_honors_configured_header_and_idle_deadlines() {
    let (origin, _, _) = origin().await;
    let proxy = proxy(json!({"mode":"Forward","listen_addr":"127.0.0.1:0",
        "max_connection_lifetime_secs":0,"monitoring":{"enabled":false},
        "forward_proxy_config":{"response_header_timeout_secs":1,"stream_idle_timeout_secs":1}}))
    .await;
    let client = reqwest::Client::builder()
        .proxy(reqwest::Proxy::http(format!("http://{}", proxy.addr)).unwrap())
        .build()
        .unwrap();
    let response = timeout(
        Duration::from_secs(3),
        client
            .get(format!("http://{}/no-headers", origin.addr))
            .send(),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(response.status(), 504);
    let mut response = client
        .get(format!("http://{}/hold", origin.addr))
        .send()
        .await
        .unwrap();
    response.chunk().await.unwrap().unwrap();
    assert!(
        timeout(Duration::from_secs(3), response.chunk())
            .await
            .unwrap()
            .is_err()
    );
}

#[tokio::test]
async fn streaming_changes_preserve_websocket_upgrade_on_both_proxy_modes() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let backend = Running {
        addr,
        task: tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                tokio::spawn(async move {
                    let mut headers = Vec::new();
                    let mut byte = [0];
                    while !headers.ends_with(b"\r\n\r\n") {
                        if socket.read(&mut byte).await.unwrap_or(0) == 0 {
                            return;
                        }
                        headers.push(byte[0]);
                    }
                    let text = String::from_utf8(headers).unwrap().to_lowercase();
                    assert!(text.contains("upgrade: websocket"));
                    assert!(text.contains("connection: upgrade"));
                    socket.write_all(b"HTTP/1.1 101 Switching Protocols\r\nConnection: upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n\r\n").await.unwrap();
                    let (mut r, mut w) = socket.split();
                    let _ = tokio::io::copy(&mut r, &mut w).await;
                });
            }
        }),
    };
    for mode in ["Forward", "Reverse"] {
        let proxy = proxy(json!({"mode":mode,"listen_addr":"127.0.0.1:0",
            "reverse_proxy_target":format!("http://{}",backend.addr),
            "max_connection_lifetime_secs":0,"monitoring":{"enabled":false}}))
        .await;
        let mut socket = tokio::net::TcpStream::connect(proxy.addr).await.unwrap();
        let target = if mode == "Forward" {
            format!("http://{}/ws", backend.addr)
        } else {
            "/ws".into()
        };
        socket.write_all(format!("GET {target} HTTP/1.1\r\nHost: {}\r\nConnection: upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",backend.addr).as_bytes()).await.unwrap();
        let mut headers = Vec::new();
        timeout(Duration::from_secs(2), async {
            let mut byte = [0];
            while !headers.ends_with(b"\r\n\r\n") {
                assert_eq!(socket.read(&mut byte).await.unwrap(), 1);
                headers.push(byte[0]);
            }
        })
        .await
        .unwrap();
        assert!(
            headers.starts_with(b"HTTP/1.1 101"),
            "{}",
            String::from_utf8_lossy(&headers)
        );
        socket.write_all(b"ping").await.unwrap();
        let mut reply = [0; 4];
        timeout(Duration::from_secs(2), socket.read_exact(&mut reply))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&reply, b"ping");
    }
}
