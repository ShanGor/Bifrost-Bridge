//! HTTP request benchmarks through Bifrost's static and reverse proxy stacks.
//!
//! These measure the request pipeline and local socket round trip. They are
//! useful for comparing revisions on the same host; they are not production
//! capacity claims because they run against loopback fixtures.

use bifrost_bridge::{Config, ProxyFactory};
use bytes::Bytes;
use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use http_body_util::Full;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Response, StatusCode};
use hyper_util::rt::TokioIo;
use serde_json::json;
use std::fs;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::runtime::Runtime;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

struct BenchFixture {
    runtime: Runtime,
    client: reqwest::Client,
    static_addr: SocketAddr,
    reverse_addr: SocketAddr,
    _temp: TempDir,
    _tasks: Vec<JoinHandle<()>>,
}

fn serve_proxy(runtime: &Runtime, config: Config) -> (SocketAddr, JoinHandle<()>) {
    let proxy = ProxyFactory::create_proxy(config).expect("construct proxy");
    let listener = runtime
        .block_on(TcpListener::bind("127.0.0.1:0"))
        .expect("bind proxy listener");
    let addr = listener.local_addr().expect("proxy address");
    let task = runtime.spawn(async move {
        proxy
            .run(Arc::new(listener), CancellationToken::new())
            .await
            .expect("run proxy listener");
    });
    (addr, task)
}

fn setup() -> BenchFixture {
    let runtime = Runtime::new().expect("create benchmark runtime");
    let temp = TempDir::new().expect("create static fixture directory");
    let small_file = temp.path().join("small.txt");
    let large_file = temp.path().join("large.bin");
    fs::write(&small_file, vec![b'a'; 1024]).unwrap();
    fs::write(&large_file, vec![b'b'; 64 * 1024]).unwrap();

    let origin_listener = runtime
        .block_on(TcpListener::bind("127.0.0.1:0"))
        .expect("bind origin listener");
    let origin_addr = origin_listener.local_addr().unwrap();
    let origin_task = runtime.spawn(async move {
        loop {
            let Ok((stream, _)) = origin_listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let service = service_fn(|_request| async move {
                    Ok::<_, std::convert::Infallible>(
                        Response::builder()
                            .status(StatusCode::OK)
                            .header("content-type", "application/octet-stream")
                            .body(Full::new(Bytes::from(vec![b'r'; 1024])))
                            .unwrap(),
                    )
                });
                let _ = http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .await;
            });
        }
    });

    let static_config: Config = serde_json::from_value(json!({
        "mode": "Reverse",
        "listen_addr": "127.0.0.1:0",
        "static_files": {
            "mounts": [{ "path": "/", "root_dir": temp.path() }],
            "cache_millisecs": 0
        },
        "monitoring": { "enabled": false }
    }))
    .expect("parse static benchmark config");
    let reverse_config: Config = serde_json::from_value(json!({
        "mode": "Reverse",
        "listen_addr": "127.0.0.1:0",
        "reverse_proxy_target": format!("http://{origin_addr}"),
        "reverse_proxy_config": { "pool_max_idle_per_host": 8 },
        "monitoring": { "enabled": false }
    }))
    .expect("parse reverse benchmark config");

    let (static_addr, static_task) = serve_proxy(&runtime, static_config);
    let (reverse_addr, reverse_task) = serve_proxy(&runtime, reverse_config);
    let client = reqwest::Client::builder()
        .no_proxy()
        .pool_max_idle_per_host(8)
        .http1_only()
        .build()
        .expect("create HTTP benchmark client");

    BenchFixture {
        runtime,
        client,
        static_addr,
        reverse_addr,
        _temp: temp,
        _tasks: vec![origin_task, static_task, reverse_task],
    }
}

fn bench_http_pipeline(c: &mut Criterion) {
    let fixture = setup();
    let mut group = c.benchmark_group("http_pipeline");
    group.sample_size(30);
    group.measurement_time(Duration::from_secs(3));
    group.warm_up_time(Duration::from_secs(1));

    for (name, path, bytes) in [
        ("static_1k", "small.txt", 1024_u64),
        ("static_64k", "large.bin", 64 * 1024),
    ] {
        group.throughput(Throughput::Bytes(bytes));
        let url = format!("http://{}/{path}", fixture.static_addr);
        group.bench_function(BenchmarkId::new("request", name), |b| {
            b.iter(|| {
                let response = fixture
                    .runtime
                    .block_on(fixture.client.get(&url).send())
                    .expect("static HTTP request");
                assert_eq!(response.status(), StatusCode::OK);
                let body = fixture
                    .runtime
                    .block_on(response.bytes())
                    .expect("read static response");
                black_box(body.len());
            });
        });
    }

    group.throughput(Throughput::Bytes(1024));
    let reverse_url = format!("http://{}/bench", fixture.reverse_addr);
    group.bench_function("request/reverse_1k", |b| {
        b.iter(|| {
            let response = fixture
                .runtime
                .block_on(fixture.client.get(&reverse_url).send())
                .expect("reverse proxy HTTP request");
            assert_eq!(response.status(), StatusCode::OK);
            let body = fixture
                .runtime
                .block_on(response.bytes())
                .expect("read reverse response");
            black_box(body.len());
        });
    });

    group.finish();
}

criterion_group!(benches, bench_http_pipeline);
criterion_main!(benches);
