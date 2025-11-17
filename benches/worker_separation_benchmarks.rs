//! Performance benchmarks for worker separation architecture
//!
//! This benchmark suite measures the performance characteristics and overhead
//! of the worker separation architecture, including:
//! - Worker creation overhead
//! - Connection limit enforcement performance
//! - Metrics collection overhead
//! - Resource isolation performance
//! - Concurrent access patterns
//! - Memory usage patterns

use criterion::{black_box, criterion_group, criterion_main, Criterion, BenchmarkId};
use bifrost_bridge::common::{
    ProxyType, IsolatedWorker, WorkerResourceLimits, WorkerConfiguration,
    ConnectionPoolManager, PerformanceMetrics
};
use std::sync::Arc;
use std::time::Duration;

/// Benchmark worker creation overhead
fn bench_worker_creation(c: &mut Criterion) {
    let mut group = c.benchmark_group("worker_creation");

    // Benchmark creation with default limits
    group.bench_function("create_worker_default", |b| {
        b.iter(|| {
            let worker = IsolatedWorker::new(
                ProxyType::ForwardProxy,
                WorkerResourceLimits::default(),
                WorkerConfiguration::default(),
            );
            black_box(worker);
        });
    });

    // Benchmark creation with custom limits
    let custom_limits = WorkerResourceLimits {
        max_connections: 5000,
        max_memory_mb: 2048,
        max_requests_per_second: 25000,
        max_file_size_mb: 500,
        connection_timeout: Duration::from_secs(15),
        request_timeout: Duration::from_secs(30),
        max_cpu_percent: 75.0,
        connection_timeout_secs: 15,
        idle_timeout_secs: 60,
        max_connection_lifetime_secs: 300,
    };

    group.bench_function("create_worker_custom", |b| {
        b.iter(|| {
            let worker = IsolatedWorker::new(
                ProxyType::ReverseProxy,
                custom_limits.clone(),
                WorkerConfiguration::default(),
            );
            black_box(worker);
        });
    });

    // Benchmark creation for different proxy types
    for proxy_type in [ProxyType::ForwardProxy, ProxyType::ReverseProxy, ProxyType::StaticFiles, ProxyType::Combined] {
        group.bench_with_input(
            BenchmarkId::new("create_by_type", format!("{:?}", proxy_type)),
            &proxy_type,
            |b, proxy_type: &ProxyType| {
                b.iter(|| {
                    let worker = IsolatedWorker::new(
                        proxy_type.clone(),
                        WorkerResourceLimits::default(),
                        WorkerConfiguration::default(),
                    );
                    black_box(worker);
                });
            },
        );
    }

    group.finish();
}

/// Benchmark connection limit enforcement
fn bench_connection_limit_enforcement(c: &mut Criterion) {
    let mut group = c.benchmark_group("connection_limit_enforcement");

    let worker = Arc::new(IsolatedWorker::new(
        ProxyType::ForwardProxy,
        WorkerResourceLimits::default(),
        WorkerConfiguration::default(),
    ));

    group.bench_function("can_accept_connection_check", |b| {
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

    group.bench_function("increment_decrement_pair", |b| {
        b.iter(|| {
            worker.increment_connections();
            worker.decrement_connections();
        });
    });

    group.finish();
}

/// Benchmark metrics collection overhead
fn bench_metrics_collection(c: &mut Criterion) {
    let mut group = c.benchmark_group("metrics_collection");

    let metrics = Arc::new(PerformanceMetrics::new());

    group.bench_function("increment_requests", |b| {
        b.iter(|| {
            metrics.increment_requests();
        });
    });

    group.bench_function("increment_connections", |b| {
        b.iter(|| {
            metrics.increment_connections();
        });
    });

    group.bench_function("record_response_bytes", |b| {
        b.iter(|| {
            metrics.record_response_bytes(1024);
        });
    });

    group.bench_function("update_average_response_time", |b| {
        b.iter(|| {
            metrics.update_average_response_time(50);
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

/// Benchmark connection pool management
fn bench_connection_pool_management(c: &mut Criterion) {
    let mut group = c.benchmark_group("connection_pool_management");

    let pool = Arc::new(ConnectionPoolManager::new(
        ProxyType::ForwardProxy,
        100, // max_idle_per_host
        Duration::from_secs(300),
        true, // connection_pool_enabled
    ));

    group.bench_function("increment_connections", |b| {
        b.iter(|| {
            pool.increment_connections();
        });
    });

    group.bench_function("decrement_connections", |b| {
        b.iter(|| {
            pool.decrement_connections();
        });
    });

    group.bench_function("can_accept_connection", |b| {
        b.iter(|| {
            let result = pool.can_accept_connection();
            black_box(result);
        });
    });

    group.bench_function("active_connections", |b| {
        b.iter(|| {
            let count = pool.active_connections();
            black_box(count);
        });
    });

    group.finish();
}

/// Benchmark concurrent worker access
fn bench_concurrent_worker_access(c: &mut Criterion) {
    let mut group = c.benchmark_group("concurrent_access");

    // Benchmark with different numbers of workers
    for worker_count in [1, 4, 8, 16] {
        group.bench_with_input(
            BenchmarkId::new("concurrent_operations", worker_count),
            &worker_count,
            |b, &worker_count| {
                let workers: Vec<Arc<IsolatedWorker>> = (0..worker_count)
                    .enumerate().map(|(i, _)| {
                        let mut limits = WorkerResourceLimits::default();
                        limits.max_connections = 1000 + i as usize;

                        Arc::new(IsolatedWorker::new(
                            if i % 2 == 0 { ProxyType::ForwardProxy } else { ProxyType::ReverseProxy },
                            limits,
                            WorkerConfiguration::default(),
                        ))
                    })
                    .collect();

                b.iter(|| {
                    // Perform concurrent operations on all workers
                    for worker in &workers {
                        worker.can_accept_connection();
                        worker.metrics.requests_total.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                });
            },
        );
    }

    group.finish();
}

/// Benchmark memory usage patterns
fn bench_memory_usage(c: &mut Criterion) {
    let mut group = c.benchmark_group("memory_usage");

    group.bench_function("worker_memory_footprint", |b| {
        b.iter(|| {
            // Create many workers to test memory usage
            let workers: Vec<IsolatedWorker> = (0..100)
                .map(|_i| {
                    IsolatedWorker::new(
                        ProxyType::ForwardProxy,
                        WorkerResourceLimits::default(),
                        WorkerConfiguration::default(),
                    )
                })
                .collect();
            black_box(workers);
        });
    });

    group.bench_function("metrics_memory_footprint", |b| {
        b.iter(|| {
            // Create many metrics instances
            let metrics: Vec<PerformanceMetrics> = (0..1000)
                .map(|_| PerformanceMetrics::new())
                .collect();
            black_box(metrics);
        });
    });

    group.finish();
}

/// Benchmark configuration validation
fn bench_configuration_validation(c: &mut Criterion) {
    let mut group = c.benchmark_group("configuration_validation");

    use bifrost_bridge::config_validation::WorkerSeparationValidator;

    group.bench_function("validation_basic", |b| {
        b.iter(|| {
            let validator = WorkerSeparationValidator::new()
                .with_enabled_types(true, false, false, false)
                .with_listen_address(ProxyType::ForwardProxy, "127.0.0.1:3128".to_string())
                .with_resource_limits(ProxyType::ForwardProxy, WorkerResourceLimits::default());

            let result = validator.validate();
            black_box(result);
        });
    });

    group.bench_function("validation_complex", |b| {
        b.iter(|| {
            let validator = WorkerSeparationValidator::new()
                .with_enabled_types(true, true, true, false)
                .with_listen_address(ProxyType::ForwardProxy, "127.0.0.1:3128".to_string())
                .with_listen_address(ProxyType::ReverseProxy, "127.0.0.1:8080".to_string())
                .with_listen_address(ProxyType::StaticFiles, "127.0.0.1:9000".to_string())
                .with_resource_limits(ProxyType::ForwardProxy, WorkerResourceLimits::default())
                .with_resource_limits(ProxyType::ReverseProxy, WorkerResourceLimits::default())
                .with_resource_limits(ProxyType::StaticFiles, WorkerResourceLimits::default());

            let result = validator.validate();
            black_box(result);
        });
    });

    group.finish();
}

/// Benchmark health checks
fn bench_health_checks(c: &mut Criterion) {
    let mut group = c.benchmark_group("health_checks");

    let worker = Arc::new(IsolatedWorker::new(
        ProxyType::ForwardProxy,
        WorkerResourceLimits::default(),
        WorkerConfiguration::default(),
    ));

    group.bench_function("health_check", |b| {
        b.iter(|| {
            let health = worker.health_check();
            black_box(health);
        });
    });

    // Benchmark with different load levels
    for load_factor in [10, 25, 50, 75] {
        group.bench_with_input(
            BenchmarkId::new("health_check_under_load", load_factor),
            &load_factor,
            |b, &load_factor| {
                // Simulate load
                let target_connections = (worker.resource_limits.max_connections as f64 * load_factor as f64 / 100.0) as usize;
                for _ in 0..target_connections {
                    worker.increment_connections();
                }

                b.iter(|| {
                    let health = worker.health_check();
                    black_box(health);
                });

                // Clean up
                for _ in 0..target_connections {
                    worker.decrement_connections();
                }
            },
        );
    }

    group.finish();
}

/// Benchmark proxy type operations
fn bench_proxy_type_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("proxy_type_operations");

    let proxy_types = vec![
        ProxyType::ForwardProxy,
        ProxyType::ReverseProxy,
        ProxyType::StaticFiles,
        ProxyType::Combined,
    ];

    group.bench_function("clone_proxy_type", |b| {
        b.iter(|| {
            for proxy_type in &proxy_types {
                let cloned = proxy_type.clone();
                black_box(cloned);
            }
        });
    });

    group.bench_function("display_proxy_type", |b| {
        b.iter(|| {
            for proxy_type in &proxy_types {
                let display = format!("{}", proxy_type);
                black_box(display);
            }
        });
    });

    group.bench_function("hash_proxy_type", |b| {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        b.iter(|| {
            for proxy_type in &proxy_types {
                let mut hasher = DefaultHasher::new();
                proxy_type.hash(&mut hasher);
                black_box(hasher.finish());
            }
        });
    });

    group.finish();
}

/// Benchmark scenario: Mixed workload simulation
fn bench_mixed_workload_scenario(c: &mut Criterion) {
    let mut group = c.benchmark_group("mixed_workload_scenario");

    let forward_worker = Arc::new(IsolatedWorker::new(
        ProxyType::ForwardProxy,
        WorkerResourceLimits::default(),
        WorkerConfiguration::default(),
    ));

    let reverse_worker = Arc::new(IsolatedWorker::new(
        ProxyType::ReverseProxy,
        WorkerResourceLimits::default(),
        WorkerConfiguration::default(),
    ));

    group.bench_function("mixed_operations", |b| {
        b.iter(|| {
            // Simulate mixed workload
            for i in 0..100 {
                if i % 3 == 0 {
                    forward_worker.can_accept_connection();
                    forward_worker.increment_connections();
                    forward_worker.metrics.requests_total.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                } else if i % 3 == 1 {
                    reverse_worker.can_accept_connection();
                    reverse_worker.increment_connections();
                    reverse_worker.metrics.requests_total.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                } else {
                    forward_worker.decrement_connections();
                    reverse_worker.decrement_connections();
                }
            }
        });
    });

    group.bench_function("resource_limit_enforcement", |b| {
        b.iter(|| {
            // Test limit enforcement under load
            for _ in 0..1000 {
                forward_worker.can_accept_connection();
                reverse_worker.can_accept_connection();
            }
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_worker_creation,
    bench_connection_limit_enforcement,
    bench_metrics_collection,
    bench_connection_pool_management,
    bench_concurrent_worker_access,
    bench_memory_usage,
    bench_configuration_validation,
    bench_health_checks,
    bench_proxy_type_operations,
    bench_mixed_workload_scenario
);

criterion_main!(benches);