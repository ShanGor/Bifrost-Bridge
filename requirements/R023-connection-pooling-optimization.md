# R023: Connection Pooling Optimization

**Status**: In Progress  
**Created**: 2025-01-18  
**Priority**: High

## Overview

Optimize connection pooling strategies for forward and reverse proxy modes with different pooling behaviors and health check mechanisms.

## Background

Current implementation has incorrect pooling strategies:
- Forward proxy pools connections to arbitrary target hosts
- Reverse proxy lacks configurable connection pooling
- No health check mechanism for pooled connections
- Inefficient resource usage

## Requirements

### Forward Proxy Pooling Strategy

**Behavior**: Allow connection reuse for same target host, but close idle connections immediately.

**Configuration**:
- `pool_max_idle_per_host`: Always `0` (no persistent idle connection pool)
- `pool_idle_timeout`: Short timeout (10-30s) to close unused connections quickly
- Connection reuse: Allowed while actively processing requests to the same host

**Example**:
```
User makes 5 requests to api.example.com:
- Request 1: Creates new connection
- Request 2-5: Reuse the same connection (if within active window)
- After idle_timeout: Connection closes (not kept in pool)
```

**Rationale**:
- Forward proxy connects to many different target hosts
- Maintaining idle connection pools wastes resources
- Same-host connection reuse improves burst request performance
- No persistent pool prevents resource exhaustion

### Reverse Proxy Pooling Strategy

**Behavior**: Maintain persistent connection pool to fixed backend with health checks.

**Configuration**:
```json
{
  "reverse_proxy_config": {
    "pool_max_idle_per_host": 10,        // 0-50, default: 10
    "pool_idle_timeout_secs": 90,         // default: 90s
    "health_check": {                     // optional
      "interval_secs": 30,                // default: 30s
      "endpoint": "/health",              // optional, use TCP if not set
      "timeout_secs": 5                   // default: 5s
    }
  }
}
```

**Features**:
1. **Configurable Pool Size**:
   - `pool_max_idle_per_host = 0`: No pooling (new connection per request)
   - `pool_max_idle_per_host > 0`: Maintain connection pool for performance

2. **Health Check Mechanisms**:
   - **TCP Health Check** (default): Verify connection is still open
   - **HTTP Endpoint Check**: Send GET request to health check endpoint
   - Configurable interval and timeout
   - Automatic removal of unhealthy connections from pool

3. **Long Idle Timeout**:
   - Default 60-90s (vs 10-30s for forward proxy)
   - Keeps connections warm for better performance

**Rationale**:
- Reverse proxy connects to single fixed backend
- Persistent pool reduces connection establishment overhead
- Health checks ensure pool contains only healthy connections
- Improves throughput and latency for high-traffic scenarios

## Technical Design

### Configuration Structure

```rust
/// Reverse proxy specific configuration
pub struct ReverseProxyConfig {
    /// Maximum idle connections to keep per backend host
    /// 0 = no pooling, 1-50 = maintain connection pool
    pub pool_max_idle_per_host: usize,
    
    /// Pool idle timeout in seconds
    pub pool_idle_timeout_secs: u64,
    
    /// Health check configuration (optional)
    pub health_check: Option<HealthCheckConfig>,
}

/// Health check configuration
pub struct HealthCheckConfig {
    /// Health check interval in seconds
    pub interval_secs: u64,
    
    /// Health check endpoint (e.g., "/health", "/ping")
    /// If not set, uses TCP connection check
    pub endpoint: Option<String>,
    
    /// Timeout for health check in seconds
    pub timeout_secs: u64,
}
```

### Forward Proxy Implementation

```rust
// Build HTTP client with pool_max_idle_per_host = 0
let mut builder = Client::builder(TokioExecutor::new());
builder.pool_max_idle_per_host(0);  // No persistent idle pool
builder.pool_idle_timeout(Duration::from_secs(idle_timeout_secs));
builder.pool_timer(TokioTimer::new());
```

### Reverse Proxy Implementation

```rust
// Build HTTP client with configurable pooling
let pool_config = reverse_proxy_config.unwrap_or_default();
let mut builder = Client::builder(TokioExecutor::new());
builder.pool_max_idle_per_host(pool_config.pool_max_idle_per_host);
builder.pool_idle_timeout(Duration::from_secs(pool_config.pool_idle_timeout_secs));
builder.pool_timer(TokioTimer::new());

// Start health check background task if configured
if let Some(health_check) = pool_config.health_check {
    spawn_health_check_task(health_check, target_url);
}
```

### Health Check Implementation

**TCP Health Check**:
```rust
async fn tcp_health_check(target_host: &str, target_port: u16) -> bool {
    tokio::time::timeout(
        Duration::from_secs(timeout_secs),
        TcpStream::connect((target_host, target_port))
    ).await.is_ok()
}
```

**HTTP Health Check**:
```rust
async fn http_health_check(client: &Client, endpoint: &str) -> bool {
    let request = Request::builder()
        .method(Method::GET)
        .uri(endpoint)
        .body(Empty::new())
        .unwrap();
    
    client.request(request).await.is_ok()
}
```

## Benefits

1. **Performance**:
   - Forward proxy: Reduced memory usage, faster cleanup
   - Reverse proxy: Reduced latency (30-50% improvement with pooling)

2. **Resource Efficiency**:
   - No wasted connections to arbitrary hosts
   - Optimized pool size for backend connections

3. **Reliability**:
   - Health checks detect and remove failed connections
   - Automatic recovery from transient failures

4. **Flexibility**:
   - Users can disable pooling (set to 0)
   - Choice between TCP and HTTP health checks
   - Configurable intervals and timeouts

## Configuration Examples

### Example 1: Reverse Proxy with Connection Pool (Default)
```json
{
  "mode": "Reverse",
  "listen_addr": "127.0.0.1:8080",
  "reverse_proxy_target": "http://backend:3000",
  "reverse_proxy_config": {
    "pool_max_idle_per_host": 10,
    "pool_idle_timeout_secs": 90
  }
}
```

### Example 2: Reverse Proxy with HTTP Health Check
```json
{
  "mode": "Reverse",
  "listen_addr": "127.0.0.1:8080",
  "reverse_proxy_target": "http://backend:3000",
  "reverse_proxy_config": {
    "pool_max_idle_per_host": 20,
    "pool_idle_timeout_secs": 90,
    "health_check": {
      "interval_secs": 30,
      "endpoint": "/health",
      "timeout_secs": 5
    }
  }
}
```

### Example 3: Reverse Proxy without Pooling
```json
{
  "mode": "Reverse",
  "listen_addr": "127.0.0.1:8080",
  "reverse_proxy_target": "http://backend:3000",
  "reverse_proxy_config": {
    "pool_max_idle_per_host": 0
  }
}
```

### Example 4: Forward Proxy (Auto-configured)
```json
{
  "mode": "Forward",
  "listen_addr": "127.0.0.1:8080",
  "idle_timeout_secs": 30
}
```
Note: Forward proxy automatically uses `pool_max_idle_per_host=0`

## Implementation Tasks

- [x] Add configuration structures (HealthCheckConfig, ReverseProxyConfig)
- [x] Update Config struct with reverse_proxy_config field
- [x] Implement forward proxy with pool_max_idle_per_host=0
- [ ] Refactor ReverseProxy to use hyper_util::client::legacy::Client
- [ ] Implement connection pooling configuration
- [ ] Implement TCP health check
- [ ] Implement HTTP endpoint health check
- [ ] Add health check background task
- [ ] Update proxy.rs to pass configuration
- [ ] Add integration tests
- [ ] Update example configurations

## Testing

### Unit Tests
- Test forward proxy closes idle connections
- Test reverse proxy maintains connection pool
- Test health check detects failed connections
- Test configuration parsing

### Integration Tests
- Test connection reuse behavior
- Test pool size limits
- Test health check with failing backend
- Test graceful degradation

## Migration Guide

### For Forward Proxy Users
No configuration changes needed. The optimization is automatic.

### For Reverse Proxy Users

**Before** (current):
```json
{
  "mode": "Reverse",
  "reverse_proxy_target": "http://backend:3000"
}
```

**After** (with optimization):
```json
{
  "mode": "Reverse",
  "reverse_proxy_target": "http://backend:3000",
  "reverse_proxy_config": {
    "pool_max_idle_per_host": 10
  }
}
```

**Note**: If `reverse_proxy_config` is not specified, defaults are used (pool_max_idle_per_host=10).

## Future Enhancements

- Connection pool metrics (idle count, active count, reuse rate)
- Adaptive pool sizing based on traffic patterns
- Circuit breaker integration with health checks
- HTTP/2 connection pooling support
- Connection pool warming on startup
