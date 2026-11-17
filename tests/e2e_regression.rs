//! Process-level regression tests for the shipped executable.
//!
//! These tests load JSON configuration, start the actual `bifrost-bridge`
//! binary, and send HTTP traffic through its listening socket.

use http_body_util::{BodyExt, Full};
use hyper::body::{Bytes, Incoming};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use serde_json::{Value, json};
use std::fs;
use std::net::{SocketAddr, TcpListener as StdTcpListener};
use std::process::{Child, Command, Stdio};
use std::time::Duration;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio::time::{sleep, timeout};

struct Origin {
    addr: SocketAddr,
    task: JoinHandle<()>,
}

impl Drop for Origin {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct BridgeProcess {
    child: Child,
    addr: SocketAddr,
}

impl Drop for BridgeProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

async fn start_origin() -> Origin {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let service = service_fn(|mut request: Request<Incoming>| async move {
                    let body = request
                        .body_mut()
                        .collect()
                        .await
                        .map(|body| String::from_utf8_lossy(&body.to_bytes()).into_owned())
                        .unwrap_or_else(|_| "<body-read-error>".to_string());
                    let path = request
                        .uri()
                        .path_and_query()
                        .map(|path| path.as_str())
                        .unwrap_or("/")
                        .to_string();
                    let method = request.method().as_str().to_string();
                    let regression_header = request
                        .headers()
                        .get("x-regression-id")
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or("")
                        .to_string();
                    let response_status = if request.uri().path() == "/api/teapot" {
                        StatusCode::IM_A_TEAPOT
                    } else {
                        StatusCode::OK
                    };
                    let response_body = json!({
                        "method": method,
                        "path": path,
                        "regression_header": regression_header,
                        "body": body,
                    });
                    Ok::<_, std::convert::Infallible>(
                        Response::builder()
                            .status(response_status)
                            .header("content-type", "application/json")
                            .header("x-origin-response", "preserved")
                            .body(Full::new(Bytes::from(response_body.to_string())))
                            .unwrap(),
                    )
                });
                let _ = http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .await;
            });
        }
    });
    Origin { addr, task }
}

fn unused_local_port() -> u16 {
    let listener = StdTcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

async fn start_bridge(config: Value, temp: &TempDir) -> BridgeProcess {
    let port = unused_local_port();
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let config_path = temp.path().join("e2e-config.json");
    let mut config = config;
    config["listen_addr"] = json!(addr.to_string());
    fs::write(&config_path, serde_json::to_vec_pretty(&config).unwrap()).unwrap();

    let child = Command::new(env!("CARGO_BIN_EXE_bifrost-bridge"))
        .arg("--config")
        .arg(&config_path)
        .arg("--pid-file")
        .arg(temp.path().join("bifrost.pid"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start bifrost-bridge binary");
    let mut process = BridgeProcess { child, addr };
    timeout(Duration::from_secs(10), async {
        loop {
            if let Some(status) = process.child.try_wait().unwrap() {
                panic!("bifrost-bridge exited before becoming ready: {status}");
            }
            if tokio::net::TcpStream::connect(process.addr).await.is_ok() {
                break;
            }
            sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("bifrost-bridge did not listen within 10 seconds");

    process
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .http1_only()
        .build()
        .unwrap()
}

#[tokio::test]
async fn binary_combined_mode_routes_static_spa_and_reverse_requests() {
    let temp = TempDir::new().unwrap();
    let static_root = temp.path().join("web");
    fs::create_dir_all(static_root.join("assets")).unwrap();
    fs::write(
        static_root.join("index.html"),
        "<!doctype html><title>regression-spa</title>",
    )
    .unwrap();
    fs::write(
        static_root.join("assets/app.js"),
        "window.regression = true;",
    )
    .unwrap();

    let origin = start_origin().await;
    let config = json!({
        "mode": "Reverse",
        "listen_addr": "127.0.0.1:0",
        "reverse_proxy_routes": [{
            "id": "api",
            "target": format!("http://{}", origin.addr),
            "order": 50,
            "predicates": [{ "type": "Path", "patterns": ["/api/**"] }]
        }],
        "static_files": {
            "mounts": [{
                "path": "/ui",
                "root_dir": static_root,
                "spa_mode": true,
                "spa_fallback_file": "index.html",
                "order": 100
            }],
            "index_files": ["index.html"],
            "spa_mode": true,
            "spa_fallback_file": "index.html",
            "cache_millisecs": 3600
        },
        "monitoring": { "enabled": false }
    });
    let bridge = start_bridge(config, &temp).await;
    let client = client();

    let asset = client
        .get(format!("http://{}/ui/assets/app.js", bridge.addr))
        .send()
        .await
        .unwrap();
    assert_eq!(asset.status(), StatusCode::OK);
    assert!(
        asset.headers()[reqwest::header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("text/javascript")
    );
    assert_eq!(
        asset.headers()[reqwest::header::CACHE_CONTROL],
        "public, max-age=3600"
    );
    assert_eq!(asset.text().await.unwrap(), "window.regression = true;");

    let spa = client
        .get(format!("http://{}/ui/projects/42", bridge.addr))
        .send()
        .await
        .unwrap();
    assert_eq!(spa.status(), StatusCode::OK);
    assert_eq!(
        spa.headers()[reqwest::header::CACHE_CONTROL],
        "no-cache, no-store, must-revalidate"
    );
    assert!(spa.text().await.unwrap().contains("regression-spa"));

    let proxied = client
        .post(format!("http://{}/api/echo?version=2", bridge.addr))
        .header("x-regression-id", "proxy-roundtrip")
        .body("payload-for-regression")
        .send()
        .await
        .unwrap();
    assert_eq!(proxied.status(), StatusCode::OK);
    assert_eq!(proxied.headers()["x-origin-response"], "preserved");
    let echoed: Value = proxied.json().await.unwrap();
    assert_eq!(echoed["method"], "POST");
    assert_eq!(echoed["path"], "/api/echo?version=2");
    assert_eq!(echoed["regression_header"], "proxy-roundtrip");
    assert_eq!(echoed["body"], "payload-for-regression");

    let upstream_error = client
        .get(format!("http://{}/api/teapot", bridge.addr))
        .send()
        .await
        .unwrap();
    assert_eq!(upstream_error.status(), StatusCode::IM_A_TEAPOT);

    let unmatched = client
        .get(format!("http://{}/unmatched", bridge.addr))
        .send()
        .await
        .unwrap();
    assert_eq!(unmatched.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn binary_forward_mode_proxies_http_requests() {
    let temp = TempDir::new().unwrap();
    let origin = start_origin().await;
    let config = json!({
        "mode": "Forward",
        "listen_addr": "127.0.0.1:0",
        "monitoring": { "enabled": false }
    });
    let bridge = start_bridge(config, &temp).await;
    let mut socket = tokio::net::TcpStream::connect(bridge.addr).await.unwrap();
    let request = format!(
        "GET http://{}/forward/probe?case=process-e2e HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
        origin.addr, origin.addr
    );
    socket.write_all(request.as_bytes()).await.unwrap();

    let mut headers = Vec::new();
    let mut byte = [0_u8; 1];
    while !headers.ends_with(b"\r\n\r\n") {
        assert_ne!(
            socket.read(&mut byte).await.unwrap(),
            0,
            "proxy closed before response headers"
        );
        headers.push(byte[0]);
    }
    let header_text = String::from_utf8(headers).unwrap();
    assert!(
        header_text.starts_with("HTTP/1.1 200 "),
        "unexpected proxy response: {header_text}"
    );
    let content_length = header_text
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().unwrap())
        })
        .expect("forwarded response should include content-length");
    let mut body = vec![0; content_length];
    socket.read_exact(&mut body).await.unwrap();
    let echoed: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(echoed["method"], "GET");
    assert_eq!(echoed["path"], "/forward/probe?case=process-e2e");
}
