//! Simple performance benchmarks for worker separation architecture

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use bifrost_bridge::common::{
    ProxyType, IsolatedWorker, WorkerResourceLimits, WorkerConfiguration,
    ConnectionPoolManager, PerformanceMetrics
};
use std::sync::Arc;
use std::time::Duration;

/// Benchmark basic worker operations
fn bench_basic_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("basic_operations");

    let worker = Arc::new(IsolatedWorker::new(
        ProxyType::ForwardProxy,
        WorkerResourceLimits::default(),
        WorkerConfiguration::default(),
    ));

    group.bench_function("can_accept_connection", |b| {
        b.iter(|| {
            let result = worker.can_accept_connection();
            black_box(result);
        });
    });

    group.bench_function("increment_connection", |b| {
        b.iter(|| {
            worker.increment_connections();
        });
    });

    group.bench_function("decrement_connection", |b| {
        b.iter(|| {
            worker.decrement_connections();
        });
    });

    group.finish();
}

/// Benchmark metrics operations
fn bench_metrics(c: &mut Criterion) {
    let mut group = c.benchmark_group("metrics");

    let metrics = Arc::new(PerformanceMetrics::new());

    group.bench_function("increment_requests", |b| {
        b.iter(|| {
            metrics.increment_requests();
        });
    });

    group.bench_function("record_response_bytes", |b| {
        b.iter(|| {
            metrics.record_response_bytes(1024);
        });
    });

    group.bench_function("get_metrics_summary", |b| {
        b.iter(|| {
            let summary = metrics.get_metrics_summary();
            black_box(summary);
        });
    });

    group.finish();
}

/// Benchmark connection pool operations
fn bench_connection_pool(c: &mut Criterion) {
    let mut group = c.benchmark_group("connection_pool");

    let pool = Arc::new(ConnectionPoolManager::new(
        ProxyType::ForwardProxy,
        100,
        Duration::from_secs(300),
        true,
    ));

    group.bench_function("increment_connections", |b| {
        b.iter(|| {
            pool.increment_connections();
        });
    });

    group.bench_function("can_accept_connection", |b| {
        b.iter(|| {
            let result = pool.can_accept_connection();
            black_box(result);
        });
    });

    group.finish();
}

/// Benchmark worker creation
fn bench_worker_creation(c: &mut Criterion) {
    let mut group = c.benchmark_group("worker_creation");

    group.bench_function("create_forward_proxy_worker", |b| {
        b.iter(|| {
            let worker = IsolatedWorker::new(
                ProxyType::ForwardProxy,
                WorkerResourceLimits::default(),
                WorkerConfiguration::default(),
            );
            black_box(worker);
        });
    });

    group.bench_function("create_reverse_proxy_worker", |b| {
        b.iter(|| {
            let worker = IsolatedWorker::new(
                ProxyType::ReverseProxy,
                WorkerResourceLimits::default(),
                WorkerConfiguration::default(),
            );
            black_box(worker);
        });
    });

    group.bench_function("create_static_files_worker", |b| {
        b.iter(|| {
            let worker = IsolatedWorker::new(
                ProxyType::StaticFiles,
                WorkerResourceLimits::default(),
                WorkerConfiguration::default(),
            );
            black_box(worker);
        });
    });

    group.finish();
}

criterion_group!(benches, bench_basic_operations, bench_metrics, bench_connection_pool, bench_worker_creation);
criterion_main!(benches);