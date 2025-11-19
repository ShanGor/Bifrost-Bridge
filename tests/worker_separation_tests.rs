use bifrost_bridge::common::{
    ProxyType, IsolatedWorker, WorkerResourceLimits, WorkerConfiguration,
    ConnectionPoolManager
};
use std::sync::Arc;
use std::time::Duration;

/// Test basic worker creation and resource isolation
#[tokio::test]
async fn test_worker_creation_and_isolation() {
    // Create separate workers for different proxy types
    let forward_limits = WorkerResourceLimits::default();
    let forward_config = WorkerConfiguration::default();

    let reverse_limits = WorkerResourceLimits::default();
    let reverse_config = WorkerConfiguration::default();

    let forward_worker = IsolatedWorker::new(
        ProxyType::ForwardProxy,
        forward_limits,
        forward_config,
    );

    let reverse_worker = IsolatedWorker::new(
        ProxyType::ReverseProxy,
        reverse_limits,
        reverse_config,
    );

    // Verify workers are properly isolated
    assert_eq!(forward_worker.proxy_type, ProxyType::ForwardProxy);
    assert_eq!(reverse_worker.proxy_type, ProxyType::ReverseProxy);

    // Verify metrics are separate
    assert_eq!(forward_worker.metrics.connections_active(), 0);
    assert_eq!(reverse_worker.metrics.connections_active(), 0);
}

/// Test connection limit enforcement per worker
#[tokio::test]
async fn test_connection_limit_enforcement() {
    let resource_limits = WorkerResourceLimits::default();
    // Override to use a low limit for testing
    let mut test_limits = resource_limits.clone();
    test_limits.max_connections = 2;

    let worker = IsolatedWorker::new(
        ProxyType::ForwardProxy,
        test_limits,
        WorkerConfiguration::default(),
    );

    // Test initial state
    assert!(worker.can_accept_connection());

    // Simulate reaching connection limit
    for _ in 0..2 {
        worker.increment_connections();
    }

    // Should not accept more connections
    assert!(!worker.can_accept_connection());

    // Decrement one connection
    worker.decrement_connections();

    // Should accept connection again
    assert!(worker.can_accept_connection());

    // Clean up
    while worker.metrics.connections_active() > 0 {
        worker.decrement_connections();
    }
}

/// Test metrics isolation between workers
#[tokio::test]
async fn test_metrics_isolation() {
    let forward_worker = IsolatedWorker::new(
        ProxyType::ForwardProxy,
        WorkerResourceLimits::default(),
        WorkerConfiguration::default(),
    );

    let reverse_worker = IsolatedWorker::new(
        ProxyType::ReverseProxy,
        WorkerResourceLimits::default(),
        WorkerConfiguration::default(),
    );

    // Increment metrics on forward worker
    forward_worker.increment_connections();
    forward_worker.metrics.increment_requests_by(5);

    // Increment metrics on reverse worker
    reverse_worker.increment_connections();
    reverse_worker.metrics.increment_requests_by(10);

    // Verify metrics are isolated
    assert_eq!(forward_worker.metrics.connections_active(), 1);
    assert_eq!(reverse_worker.metrics.connections_active(), 1);

    assert_eq!(forward_worker.metrics.requests_total(), 5);
    assert_eq!(reverse_worker.metrics.requests_total(), 10);

    // Clean up
    forward_worker.decrement_connections();
    reverse_worker.decrement_connections();
}

/// Test worker resource limits validation
#[test]
fn test_worker_resource_limits_validation() {
    let valid_limits = WorkerResourceLimits::default();

    // Test valid limits
    assert!(valid_limits.validate().is_ok());

    // Test invalid connection limit
    let mut invalid_limits = valid_limits.clone();
    invalid_limits.max_connections = 0;
    assert!(invalid_limits.validate().is_err());

    // Test invalid memory limit
    let mut invalid_limits = valid_limits.clone();
    invalid_limits.max_memory_mb = 0;
    assert!(invalid_limits.validate().is_err());

    // Test invalid CPU percentage
    let mut invalid_limits = valid_limits.clone();
    invalid_limits.max_cpu_percent = 150.0;
    assert!(invalid_limits.validate().is_err());
}

/// Test connection pool manager isolation
#[tokio::test]
async fn test_connection_pool_isolation() {
    let forward_pool = ConnectionPoolManager::new(
        ProxyType::ForwardProxy,
        10, // max_idle_per_host
        Duration::from_secs(300), // pool_timeout
        true, // connection_pool_enabled
    );

    let reverse_pool = ConnectionPoolManager::new(
        ProxyType::ReverseProxy,
        20, // Different max_idle_per_host
        Duration::from_secs(600), // Different timeout
        true, // connection_pool_enabled
    );

    // Verify pools are created with different configurations
    assert_eq!(forward_pool.max_idle_per_host(), 10);
    assert_eq!(reverse_pool.max_idle_per_host(), 20);

    // Test connection increment/decrement
    forward_pool.increment_connections();
    assert_eq!(forward_pool.active_connections(), 1);
    assert_eq!(reverse_pool.active_connections(), 0);

    reverse_pool.increment_connections();
    assert_eq!(reverse_pool.active_connections(), 1);

    // Clean up
    forward_pool.decrement_connections();
    reverse_pool.decrement_connections();
}

/// Test worker health monitoring
#[tokio::test]
async fn test_worker_health_monitoring() {
    let worker = IsolatedWorker::new(
        ProxyType::ForwardProxy,
        WorkerResourceLimits::default(),
        WorkerConfiguration::default(),
    );

    // Test initial health state
    let health = worker.health_check();
    assert!(health.is_healthy());

    // Add some load
    for _ in 0..5 {
        worker.increment_connections();
    }

    let health = worker.health_check();
    assert!(health.is_healthy()); // Still healthy at low capacity

    // Clean up
    while worker.metrics.connections_active() > 0 {
        worker.decrement_connections();
    }
}

/// Test concurrent access to isolated workers
#[tokio::test]
async fn test_concurrent_worker_access() {
    let worker = Arc::new(IsolatedWorker::new(
        ProxyType::ForwardProxy,
        WorkerResourceLimits::default(),
        WorkerConfiguration::default(),
    ));

    // Spawn multiple concurrent tasks
    let mut handles = vec![];

    for i in 0..10 {
        let worker_clone = Arc::clone(&worker);
        let handle = tokio::spawn(async move {
            // Each task increments and decrements connections
            worker_clone.increment_connections();
            worker_clone.metrics.increment_requests();

            // Simulate some work
            tokio::time::sleep(Duration::from_millis(10)).await;

            worker_clone.decrement_connections();
            i
        });
        handles.push(handle);
    }

    // Wait for all tasks to complete
    let results: Vec<_> = futures::future::join_all(handles).await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    // Verify all tasks completed
    assert_eq!(results.len(), 10);

    // Verify worker is back to initial state
    assert_eq!(worker.metrics.connections_active(), 0);
    assert_eq!(worker.metrics.requests_total(), 10);
}

/// Test proxy type enumeration and display
#[test]
fn test_proxy_type_enum() {
    assert_eq!(ProxyType::ForwardProxy.to_string(), "ForwardProxy");
    assert_eq!(ProxyType::ReverseProxy.to_string(), "ReverseProxy");
    assert_eq!(ProxyType::StaticFiles.to_string(), "StaticFiles");
    assert_eq!(ProxyType::Combined.to_string(), "Combined");

    // Test equality
    assert!(ProxyType::ForwardProxy == ProxyType::ForwardProxy);
    assert!(ProxyType::ForwardProxy != ProxyType::ReverseProxy);

    // Test clone
    let cloned = ProxyType::ForwardProxy.clone();
    assert_eq!(cloned, ProxyType::ForwardProxy);
}

/// Test worker configuration defaults
#[test]
fn test_worker_configuration_defaults() {
    let config = WorkerConfiguration::default();

    // Test default values
    assert_eq!(config.enable_metrics, true);
    assert_eq!(config.enable_health_checks, true);
    assert_eq!(config.graceful_shutdown_timeout_secs, 30);
    assert_eq!(config.metrics_collection_interval_secs, 5);
}
