# Error Recovery Architecture

This document describes the comprehensive error handling and recovery system implemented in Bifrost Bridge. The system provides enterprise-grade reliability with automatic recovery, worker isolation, and sophisticated error management.

## Overview

The error recovery system consists of multiple interconnected components that work together to provide:

- **Automatic Error Recovery**: Intelligent retry mechanisms with exponential backoff
- **Circuit Breaker Pattern**: Prevention of cascade failures during system stress
- **Worker Health Monitoring**: Continuous health tracking and automatic recovery
- **Contextual Error Handling**: Rich error context with severity classification
- **Graceful Degradation**: System continues operating even under partial failures

## Architecture Components

### 1. Error Classification System

#### Error Types
```rust
pub enum ProxyError {
    // Traditional errors
    Io(std::io::Error),
    Http(String),
    Connection(String),
    Config(String),

    // Worker separation errors
    Worker(String),
    ResourceLimitExceeded(String),
    IsolationViolation(String),
    WorkerCreationFailed(String),
    ConnectionPoolExhausted(String),
    HealthCheckFailed(String),
    WorkerRecoveryFailed(String),
}
```

#### Severity Classification
- **Low**: Common operational issues (IO, HTTP, Connection errors)
- **Medium**: Resource issues that can be recovered (Auth, Metrics errors)
- **High**: Requires intervention but system can continue (Config, ResourceLimitExceeded)
- **Critical**: May require component shutdown (WorkerCreationFailed, IsolationViolation)

#### Contextual Errors
```rust
pub struct ContextualError {
    pub error: ProxyError,
    pub context: ErrorContext,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub recovery_attempts: u32,
}
```

### 2. Circuit Breaker Pattern

#### State Management
```rust
pub enum CircuitState {
    Closed,    // Normal operation
    Open,      // Failing, reject requests
    HalfOpen,  // Testing if failures have resolved
}
```

#### Circuit Breaker Behavior
1. **Closed State**: All requests pass normally
2. **Failure Threshold**: After N failures, circuit opens
3. **Open State**: Requests are rejected immediately
4. **Timeout Recovery**: After timeout, transitions to HalfOpen
5. **HalfOpen State**: Limited requests to test recovery
6. **Success Threshold**: After M successes, circuit closes again

#### Configuration
```rust
let circuit_breaker = CircuitBreaker::new(
    5,  // failure_threshold
    3,  // success_threshold
    Duration::from_secs(30) // timeout
);
```

### 3. Error Recovery Manager

#### Core Responsibilities
- **Worker Health Tracking**: Continuous monitoring of worker health
- **Error Aggregation**: Collection and analysis of error patterns
- **Recovery Coordination**: Orchestration of recovery actions
- **Resource Management**: Enforcement of resource limits and isolation

#### Health Monitoring
```rust
pub struct WorkerHealth {
    pub worker_id: String,
    pub proxy_type: ProxyType,
    pub is_healthy: bool,
    pub consecutive_failures: u32,
    pub last_check: chrono::DateTime<chrono::Utc>,
    pub last_success: Option<chrono::DateTime<chrono::Utc>>,
    pub is_isolated: bool,
    pub recovery_attempts: u32,
}
```

### 4. IsolatedProxyAdapter Integration

The `IsolatedProxyAdapter` integrates error recovery throughout the proxy infrastructure:

```rust
pub struct IsolatedProxyAdapter {
    handler: Arc<dyn Proxy + Send + Sync>,
    worker: Arc<IsolatedWorker>,
    addr: SocketAddr,
    private_key: Option<String>,
    certificate: Option<String>,
    error_recovery: Arc<ErrorRecoveryManager>,
}
```

#### Key Features
- **Automatic Retry**: Exponential backoff for failed operations
- **Circuit Breaker Protection**: Prevents cascade failures
- **Worker Registration**: Automatic registration with error recovery manager
- **Health Monitoring**: Periodic health checks with automatic recovery

## Error Recovery Flow

### 1. Error Detection
```rust
match operation {
    Ok(result) => {
        // Success path
        recovery_manager.update_worker_health(&worker_id, true).await;
        return Some(result);
    }
    Err(error) => {
        // Error path with recovery
        if !handle_error_with_recovery(error, operation).await {
            // Recovery failed, stop retrying
            return None;
        }
        // Continue with retry logic
    }
}
```

### 2. Error Classification
```rust
impl ProxyError {
    pub fn severity(&self) -> ErrorSeverity { /* ... */ }
    pub fn is_recoverable(&self) -> bool { /* ... */ }
    pub fn suggested_recovery(&self) -> RecoveryAction { /* ... */ }
    pub fn recovery_delay(&self) -> Duration { /* ... */ }
}
```

### 3. Recovery Actions
```rust
pub enum RecoveryAction {
    Retry,              // Simple retry with backoff
    Reconnect,          // Reconnect to backend
    RestartWorker,      // Restart the worker
    Throttle,           // Slow down operations
    IsolateWorker,      // Isolate problematic worker
    SkipWorker,         // Skip this worker
    ExpandPool,         // Expand connection pool
    ForceShutdown,      // Force worker shutdown
    Ignore,             // Continue without recovery
}
```

## Performance Considerations

### Circuit Breaker Overhead
- **Minimal Impact**: Circuit breaker checks are O(1) operations
- **Async Design**: Non-blocking state transitions
- **Memory Efficient**: Uses atomic counters and shared state

### Error Recovery Manager
- **Concurrency Control**: Semaphore limits concurrent recovery attempts
- **Resource Limits**: Bounded error history to prevent memory leaks
- **Efficient Indexing**: HashMap-based worker lookup

### Contextual Error Handling
- **Efficient Cloning**: String-based error representation for cloning
- **Metadata Optimization**: Optional metadata to minimize memory usage

## Monitoring and Observability

### Error Statistics
```rust
pub struct ErrorStatistics {
    pub total_errors: usize,
    pub severity_counts: HashMap<ErrorSeverity, u32>,
    pub error_type_counts: HashMap<String, u32>,
    pub healthy_workers: usize,
    pub isolated_workers: usize,
    pub total_workers: usize,
}
```

### Health Metrics
- **Worker Health Status**: Real-time health of all workers
- **Error Rates**: Error frequency and patterns
- **Recovery Success**: Recovery attempt success rates
- **Circuit Breaker States**: Current state of all circuit breakers

## Configuration

### Error Recovery Manager
```rust
let recovery_manager = ErrorRecoveryManager::new(
    3,    // max_recovery_attempts
    1000  // error_history_limit
);
```

### Circuit Breaker Settings
```rust
let circuit_breaker = CircuitBreaker::new(
    5,                              // failure_threshold
    3,                              // success_threshold
    Duration::from_secs(30)        // timeout
);
```

## Best Practices

### 1. Error Classification
- Use appropriate severity levels for different error types
- Provide meaningful error messages with context
- Include relevant metadata for debugging

### 2. Recovery Strategies
- Start with gentle recovery actions (retry, reconnect)
- Escalate to more drastic measures (restart, isolation) as needed
- Implement exponential backoff to prevent system overload

### 3. Circuit Breaker Usage
- Set appropriate thresholds based on expected load
- Use circuit breakers for critical external dependencies
- Monitor circuit breaker states for system health

### 4. Health Monitoring
- Implement regular health checks for all workers
- Set reasonable timeouts for health check responses
- Track recovery attempts and success rates

## Testing

The error recovery system includes comprehensive tests:

### Unit Tests
- **Circuit Breaker**: State transitions and threshold handling
- **Error Classification**: Severity and recovery action mapping
- **Worker Health**: Health tracking and recovery logic

### Integration Tests
- **Error Recovery Manager**: End-to-end error handling
- **Circuit Breaker Integration**: Real-world failure scenarios
- **Worker Isolation**: Worker health monitoring and isolation

### Performance Tests
- **Error Recovery Overhead**: Minimal performance impact
- **Circuit Breaker Performance**: Low-latency state checks
- **Concurrent Error Handling**: High-load stress testing

## Examples

### Basic Error Recovery
```rust
let recovery_manager = ErrorRecoveryManager::default();
let worker = Arc::new(IsolatedWorker::new(...));
recovery_manager.register_worker(&worker).await;

// Error handling with recovery
let result = adapter.execute_with_recovery("operation", || async {
    perform_operation().await
}).await;
```

### Circuit Breaker Usage
```rust
let circuit_breaker = CircuitBreaker::new(5, 3, Duration::from_secs(30));

let result = circuit_breaker.call(async {
    risky_operation().await
}).await;

match result {
    Ok(value) => { /* Success */ }
    Err(error) => { /* Circuit breaker tripped */ }
}
```

### Custom Error Recovery
```rust
let error = ProxyError::ResourceLimitExceeded("Memory limit exceeded".to_string());
let context = ErrorContext::new("memory_manager", "allocation")
    .with_worker_id("worker_123")
    .with_metadata("limit", "1GB");

let contextual_error = ContextualError::new(error, context);
recovery_manager.handle_error(contextual_error).await;
```

## Conclusion

The error recovery architecture provides enterprise-grade reliability for the Bifrost Bridge proxy server. It ensures system stability under adverse conditions while maintaining high performance and operational efficiency.

The system is designed to:
- **Prevent cascade failures** through circuit breaker patterns
- **Automatically recover** from transient failures
- **Isolate problematic components** while maintaining overall system operation
- **Provide comprehensive monitoring** for operational visibility
- **Scale efficiently** with minimal performance overhead

This architecture enables Bifrost Bridge to operate reliably in production environments with automatic recovery from failures and graceful degradation under stress conditions.