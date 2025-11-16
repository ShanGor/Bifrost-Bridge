# Requirements Tracker

This folder tracks all requirements, feature requests, and issues raised for the proxy server project.

## 📋 Requirements Status

### ✅ **Completed Requirements**

| ID | Requirement | Status | Date Completed | Description |
|----|-------------|--------|---------------|-------------|
| R001 | Multiple Static Roots Support | ✅ Completed | 2025-11-15 | Support multiple static file directories with mount points |
| R002 | Configuration Inheritance | ✅ Completed | 2025-11-15 | Allow mount configurations to inherit from top-level settings |
| R003 | SPA Fallback Fix | ✅ Completed | 2025-11-15 | Fix JavaScript module loading issues in SPA mode |
| R004 | Graceful Shutdown | ✅ Completed | 2025-11-15 | Improve shutdown handling with Ctrl+C support |
| R005 | Compilation Cleanup | ✅ Completed | 2025-11-15 | Fix all compilation errors and warnings |
| R006 | Documentation Setup | ✅ Completed | 2025-11-15 | Create comprehensive documentation system with docs/ and requirements/ folders |
| R007 | Zero-Copy Static File Serving | ✅ Completed | 2025-11-15 | Optimize static file serving with zero-copy mechanisms for better performance |
| R008 | Custom Media Type Mappings | ✅ Completed | 2025-11-15 | Allow custom MIME type mappings like .mjs -> application/javascript |
| R009 | HTTPS Support | ✅ Completed | 2025-11-15 | Add HTTPS server with private key and certificate file paths |
| R010 | Configurable Connection Pooling | ✅ Completed | 2025-11-15 | Add pool/no-pool mode configuration for forward proxy connections |
| R011 | Granular Timeout Configuration | ✅ Completed | 2025-11-15 | Replace single timeout with three distinct timeout types |
| R012 | Basic Authentication for Forward Proxy | ✅ Completed | 2025-11-15 | Add Basic Authentication support for forward proxy clients |
| R013 | Client IP Detection Fix | ✅ Completed | 2025-11-16 | Fix hardcoded 127.0.0.1 to extract actual client IP from connection |
| R014 | Configurable Thread Pool | ✅ Completed | 2025-11-16 | Add worker_threads configuration to control concurrency - fully implemented with custom tokio runtime |

### 📝 **Pending Requirements**

| ID | Requirement | Status | Date Raised | Description |
|----|-------------|--------|------------|-------------|
| R015 | Logging System | ✅ Completed | 2025-11-16 | Add structured logging with configurable levels - comprehensive logging system fully implemented |
| R016 | Performance Monitoring | 📋 Pending | TBD | Add metrics and performance monitoring |
| R017 | WebSocket Support | 📋 Pending | TBD | Support WebSocket proxying |
| R018 | Rate Limiting | 📋 Pending | TBD | Add configurable rate limiting |
| R019 | Health Check Endpoint | 📋 Pending | TBD | Add health check endpoints |
| R020 | Documentation Maintenance | 📋 Pending | 2025-11-16 | Ensure documentation stays updated with code changes |

---

## 📝 Requirement Details

### R001: Multiple Static Roots Support ✅
**Description:** Enhance static file serving to support multiple root directories with different mount points
**Implementation:** Added `StaticMount` struct with path-based routing
**Configuration:** Supports both CLI `--mount` and JSON configuration
**Example:**
```json
{
  "static_files": {
    "mounts": [
      {"path": "/app", "root_dir": "./frontend/dist"},
      {"path": "/api", "root_dir": "./api-docs"}
    ]
  }
}
```

### R002: Configuration Inheritance ✅
**Description:** Allow mount configurations to inherit settings from parent static_files config
**Implementation:** Made mount fields optional with `resolve_inheritance()` method
**Benefits:** Cleaner configurations, DRY principle, backward compatibility
**Example:**
```json
{
  "static_files": {
    "spa_mode": true,
    "enable_directory_listing": false,
    "mounts": [
      {"path": "/", "root_dir": "./dist"} // Inherits spa_mode and other settings
    ]
  }
}
```

### R003: SPA Fallback Fix ✅
**Description:** Fix Chrome DevTools errors about JavaScript modules receiving text/html instead of application/javascript
**Implementation:** Added `is_asset_file()` check to prevent SPA fallback for asset files
**Files Modified:** `src/static_files.rs`

### R004: Graceful Shutdown ✅
**Description:** Improve application shutdown with proper signal handling
**Implementation:** Added tokio signal handling and improved shutdown messages
**Files Modified:** `src/main.rs`

### R005: Compilation Cleanup ✅
**Description:** Fix all compilation errors and warnings in the codebase
**Implementation:** Removed unused imports, fixed trait object issues, updated test configurations
**Files Modified:** `src/static_files.rs`, `src/config.rs`, tests

### R007: Zero-Copy Static File Serving ✅
**Description:** Optimize static file serving with zero-copy mechanisms for better performance
**Implementation:** Uses `tokio_util::io::ReaderStream` to wrap async file reads directly into HTTP response body without intermediate buffering
**Technical Details:**
- Uses `tokio_util::io::ReaderStream::new(file)` to create an efficient async stream
- Wrapped with `Body::wrap_stream(stream)` to pass data directly to hyper
- Eliminates unnecessary memory copies between file system and network
- Async file operations with proper streaming for large files
- Maintains all HTTP headers (Content-Type, Content-Length, Last-Modified, Cache-Control)
**Performance Benefits:**
- Reduced memory usage (no large buffer allocations)
- Lower CPU usage (no data copying)
- Better scalability for concurrent file serving
- Streamed responses enable serving large files without loading entirely into memory

### R008: Custom Media Type Mappings ✅
**Description:** Allow custom MIME type mappings like .mjs -> application/javascript
**Implementation:** Added `custom_mime_types` HashMap to StaticFileConfig with CLI and JSON configuration support
**Technical Details:**
- Added `custom_mime_types: HashMap<String, String>` field to StaticFileConfig
- Enhanced `guess_mime_type()` method to check custom mappings before default ones
- CLI support with `--mime-type EXT:MIME` argument (can be used multiple times)
- Added `add_custom_mime_type()` helper method
- Pre-populated common modern web extensions (.mjs, .jsx, .tsx, .ts, .wasm, etc.)
**Usage Examples:**
```bash
# CLI
cargo run -- --mime-type mjs:application/javascript --mime-type wasm:application/wasm

# JSON Configuration
{
  "static_files": {
    "custom_mime_types": {
      "mjs": "application/javascript",
      "jsx": "text/jsx; charset=utf-8",
      "wasm": "application/wasm"
    }
  }
}
```
**Benefits:**
- Support for ES modules (.mjs) and other modern file types
- Flexible mapping for custom file extensions
- Extensible for project-specific MIME types

### R009: HTTPS Support ✅
**Description:** Add HTTPS server with private key and certificate file paths
**Implementation:** Full HTTPS/TLS support using rustls library with PKCS#8 private key and PEM certificate format support
**Technical Details:**
- Added `private_key` and `certificate` fields to main Config struct
- Implemented TLS server configuration using rustls and tokio-rustls
- Added `create_tls_config()` helper function for certificate loading
- Supports PKCS#8 PEM format for private keys and PEM format for certificates
- Automatic HTTPS mode when both certificate files are configured
- Uses rustls for secure, modern TLS implementation with safe defaults
**Mode Support:**
- ✅ **Static File Serving**: Full HTTPS implementation for reverse proxy without backend target
- ✅ **Forward Proxy**: Complete HTTPS support with TLS termination and request forwarding
- 🔄 **Reverse Proxy**: HTTPS configuration accepted but not yet implemented (backend target mode)
**Certificate Support:**
- **Private Key Format:** PKCS#8 PEM (recommended for modern security)
- **Certificate Format:** PEM with full certificate chain
- **Key Types:** RSA and ECDSA supported
- **Security:** Uses rustls with safe cipher suites and TLS 1.2+ support
**Usage Examples:**
```bash
# CLI
cargo run -- \
  --mode reverse \
  --listen 127.0.0.1:8443 \
  --private-key ./certs/private-key.pem \
  --certificate ./certs/certificate.pem \
  --static-dir ./dist \
  --spa

# JSON Configuration
{
  "mode": "Reverse",
  "listen_addr": "127.0.0.1:8443",
  "static_files": {
    "mounts": [
      {
        "path": "/",
        "root_dir": "./dist",
        "spa_mode": true
      }
    ]
  },
  "private_key": "./certs/private-key.pem",
  "certificate": "./certs/certificate.pem"
}
```
**Certificate Generation:**
```bash
# Generate self-signed certificate for development
openssl req -x509 -newkey rsa:2048 -keyout private-key.pem -out certificate.pem -days 365 -nodes -pkcs8

# Or with ECDSA (more modern)
openssl ecparam -name secp384r1 -genkey -noout -out private-key.pem
openssl req -new -x509 -key private-key.pem -out certificate.pem -days 365
```
**Benefits:**
- Secure HTTPS communication for production deployments
- Modern TLS implementation with rustls (secure by default)
- Support for standard certificate formats
- Automatic HTTPS mode detection
- Comprehensive certificate handling and validation
- Full documentation with troubleshooting guide

### R010: Configurable Connection Pooling ✅
**Description:** Add pool/no-pool mode configuration for forward proxy connections
**Implementation:** Full connection pooling configuration with HTTP client pool management
**Technical Details:**
- Added `connection_pool_enabled` and `pool_max_idle_per_host` fields to main Config struct
- Enhanced ForwardProxy with configurable connection pool settings
- Added `new_with_pool_config()` method for custom pool configuration
- Implemented pool/no-pool mode switching with hyper client builder
- Added CLI arguments: `--no-connection-pool` and `--pool-max-idle`
- Real-time pool status display on server startup
**Pool Configuration:**
- **Connection Pool Mode:** Reuses HTTP connections for better performance (default: enabled)
- **No-Pool Mode:** Creates new connection for each request (resource isolation)
- **Pool Size Control:** Configurable maximum idle connections per host
- **Performance Impact:** Pool mode reduces TCP handshake overhead and latency
**Usage Examples:**
```bash
# CLI - Enable connection pool (default)
cargo run -- --mode forward --listen 127.0.0.1:8888 --pool-max-idle 20

# CLI - Disable connection pool (no-pool mode)
cargo run -- --mode forward --listen 127.0.0.1:8888 --no-connection-pool

# JSON Configuration
{
  "mode": "Forward",
  "listen_addr": "127.0.0.1:8888",
  "connection_pool_enabled": true,
  "pool_max_idle_per_host": 15
}
```
**Performance Benefits:**
- **Pool Mode:** Faster response times, reduced TCP overhead, better resource utilization
- **No-Pool Mode:** Connection isolation, better for security-sensitive applications
- **Configurable:** Tunable based on workload patterns and performance requirements
- **Real-time Feedback:** Server startup messages indicate current pool configuration

### R011: Granular Timeout Configuration ✅
**Description:** Replace single timeout_secs with three distinct timeout types for better connection management
**Implementation:** Added connect_timeout_secs, idle_timeout_secs, and max_connection_lifetime_secs
**Technical Details:**
- **Connect Timeout**: Controls timeout for establishing new connections to target servers
- **Idle Timeout**: Controls how long idle connections remain in the connection pool
- **Max Connection Lifetime**: Controls maximum lifetime of any connection before being closed
- Applied to both ForwardProxy and ReverseProxy implementations
- Hyper client configured with pool_idle_timeout for efficient connection management
- Backward compatibility maintained with legacy timeout_secs field
**Default Values:**
- `connect_timeout_secs`: 10 seconds (connection establishment)
- `idle_timeout_secs`: 90 seconds (pool idle timeout)
- `max_connection_lifetime_secs`: 300 seconds (5 minutes total lifetime)
**Usage Examples:**
```bash
# CLI
cargo run -- --mode forward --listen 127.0.0.1:8080 \
  --connect-timeout 10 \
  --idle-timeout 90 \
  --max-connection-lifetime 300

# JSON Configuration
{
  "mode": "Forward",
  "listen_addr": "127.0.0.1:8080",
  "connect_timeout_secs": 10,
  "idle_timeout_secs": 90,
  "max_connection_lifetime_secs": 300,
  "connection_pool_enabled": true
}
```
**Migration Guide:**
- Old `timeout_secs` field is deprecated but still supported
- If old field is present, it will be used as `connect_timeout_secs`
- New configurations should use the three new timeout fields
- All example configuration files updated to new format
**Benefits:**
- Fine-grained control over connection behavior
- Better resource management with idle timeout
- Prevents stale connections with lifetime limits
- Improved load balancing capabilities
- More predictable proxy behavior under different load conditions

### R012: Basic Authentication for Forward Proxy ✅
**Description:** Add Basic Authentication support for forward proxy clients to control access
**Implementation:** Full Basic Authentication implementation with Proxy-Authorization header validation
**Technical Details:**
- Added `proxy_username` and `proxy_password` fields to main Config struct
- Added CLI arguments `--proxy-username` and `--proxy-password` for credential configuration
- Implemented `verify_authentication()` method to validate Base64 encoded credentials
- Returns HTTP 401 Unauthorized for missing or invalid credentials
- Sends `Proxy-Authenticate: Basic realm="Proxy Server"` header for browser compatibility
- When credentials are not configured, proxy operates in open mode (no authentication required)
- Supports both CLI arguments and JSON configuration file
- Compatible with all HTTP methods (GET, POST, PUT, DELETE, CONNECT, etc.)
**Security Features:**
- Credentials validated using case-sensitive comparison
- Invalid credentials return generic error message to prevent enumeration attacks
- Removes authentication headers before forwarding requests to target servers
- Maintains relay proxy authentication independently from client authentication
**Usage Examples:**
```bash
# CLI - With authentication
cargo run -- --mode forward --listen 127.0.0.1:8080 --proxy-username admin --proxy-password secret123

# CLI - Without authentication (open proxy)
cargo run -- --mode forward --listen 127.0.0.1:8080

# JSON Configuration
{
  "mode": "Forward",
  "listen_addr": "127.0.0.1:8080",
  "proxy_username": "admin",
  "proxy_password": "secret123",
  "connection_pool_enabled": true
}
```
**Client Configuration Examples:**
```bash
# cURL with authentication
curl -U admin:secret123 -x http://127.0.0.1:8080 http://example.com

# Environment variables (Linux/Mac)
export http_proxy=http://admin:secret123@127.0.0.1:8080
export https_proxy=http://admin:secret123@127.0.0.1:8080

# Firefox Network Settings
Manual Proxy Configuration:
- HTTP Proxy: 127.0.0.1 Port: 8080
- Check "Proxy authentication required"
- Username: admin
- Password: secret123
```
**Benefits:**
- Secure access control for proxy services
- Prevents unauthorized proxy usage
- Standard HTTP Basic Auth support
- Easy to configure and use
- Works with all browsers and HTTP clients
- Independent of upstream relay proxy authentication

### R013: Client IP Detection Fix ✅
**Description:** Fix hardcoded "127.0.0.1" client IP in reverse proxy to extract actual client IP from connection
**Implementation:** Extract client IP from hyper connection context and pass via RequestContext
**Files Modified:** `src/reverse_proxy.rs`
**Technical Details:**
- Added `RequestContext` struct to store client IP extracted from connection
- Modified `make_service_fn` to accept `&hyper::server::conn::AddrStream` parameter
- Extracts client IP using `conn.remote_addr().ip().to_string()` from connection
- Created new `handle_request_with_context()` and `process_request_with_context()` methods
- Added `modify_request_with_context()` to use actual client IP from context
- Updated `modify_request()` for backward compatibility (delegates to context version)
- Added comprehensive unit tests for both with and without client IP scenarios
**Test Coverage:**
- `test_modify_request_with_client_ip()`: Verifies X-Forwarded-For header is set correctly with actual IP
- `test_modify_request_without_client_ip()`: Verifies header is not set when IP is None
**Benefits:**
- Fixes critical security issue where actual client IP was not being reported
- Enables proper access logging and monitoring with real client IPs
- Allows IP-based access control and rate limiting
- Fixes X-Forwarded-For header accuracy for backend servers
- Maintains backward compatibility with existing code
- All tests pass successfully

### R014: Configurable Thread Pool ✅
**Description:** Add worker_threads configuration to control concurrency
**Implementation:** Fully implemented with custom tokio runtime builder and thread pool management
**Technical Details:**
- Replaced `#[tokio::main]` attribute with custom runtime creation in `src/main.rs`
- Added `tokio::runtime::Builder::new_multi_thread()` with configurable worker threads
- Implemented runtime configuration that uses `worker_threads` value when specified
- Added comprehensive validation (must be > 0 and <= 512 threads)
- Supports both JSON configuration and CLI argument (`--worker-threads`)
- Logs thread pool configuration on server startup
**Key Implementation Changes:**
- **Main Function:** Converted from `#[tokio::main]` to sync function that creates custom runtime
- **Runtime Builder:** Uses `tokio::runtime::Builder` to configure thread count
- **Async Logic:** Moved all async code to separate `async_main()` function
- **Configuration Processing:** Worker threads read from `config.static_files.worker_threads`
- **Validation:** Added bounds checking and logging in `validate_config()`
**Usage Examples:**
```bash
# CLI - Custom thread count
cargo run -- --mode reverse --listen 127.0.0.1:8080 --static-dir ./public --worker-threads 4

# JSON Configuration
{
  "mode": "Reverse",
  "listen_addr": "127.0.0.1:8080",
  "static_files": {
    "mounts": [{"path": "/", "root_dir": "./public"}],
    "worker_threads": 4
  }
}
```
**Runtime Behavior:**
- **Custom Threads:** "Starting tokio runtime with X worker threads"
- **Default Threads:** "Starting tokio runtime with default worker threads (CPU cores)"
- **Validation:** "Configuration validated: worker_threads = X"
**Benefits:**
- Full control over server thread pool size for performance tuning
- Better resource management in production environments
- Ability to optimize for specific hardware configurations
- Proper thread pool configuration for high-concurrency scenarios
- Backward compatible (defaults to CPU core count)

### R015: Logging System ✅
**Description:** Add structured logging with configurable levels
**Implementation:** Comprehensive logging system with multiple output formats, targets, and configuration options
**Technical Details:**
- **Custom Logger Implementation:** Created `CustomLogger` in `src/logging.rs` with full feature support
- **CLI Arguments:** Added `--log-level` and `--log-format` arguments for runtime configuration
- **JSON Configuration:** Advanced logging configuration with targets, levels, and formats
- **Multiple Output Formats:** Support for both text and JSON log formats
- **Multiple Output Targets:** Support for stdout and file logging simultaneously
- **Structured Logging:** JSON logs with timestamp, level, target, file, line, and message fields
- **Level Filtering:** Individual target filtering (trace, debug, info, warn, error)
- **Comprehensive Integration:** Added detailed logging throughout proxy core functionality
**Key Components:**
- **Logging Enums:** `LogLevel`, `LogFormat`, `LogOutputType`, `LogTarget`
- **Configuration Struct:** `LoggingConfig` with JSON serialization support
- **Custom Logger:** `CustomLogger` implementing `log::Log` trait with advanced features
- **Fallback System:** `env_logger` fallback for simple configurations
**Usage Examples:**
```bash
# CLI - Basic logging with custom level
cargo run -- --mode forward --listen 127.0.0.1:8080 --log-level debug --log-format text

# CLI - JSON format logging
cargo run -- --mode reverse --listen 127.0.0.1:8080 --log-level info --log-format json

# Environment variable (backward compatible)
RUST_LOG=debug cargo run -- --mode forward --listen 127.0.0.1:8080
```
**JSON Configuration:**
```json
{
  "mode": "Reverse",
  "listen_addr": "127.0.0.1:8080",
  "static_files": {
    "mounts": [{"path": "/", "root_dir": "./public"}]
  },
  "logging": {
    "level": "debug",
    "format": "json",
    "targets": [
      {
        "type": "stdout",
        "level": "info"
      },
      {
        "type": "file",
        "path": "./logs/proxy.log",
        "level": "debug"
      }
    ]
  }
}
```
**Log Output Examples:**
```bash
# Text Format
2025-11-16 11:32:15.123 [INFO] [bifrost_bridge::proxy] [src/proxy.rs:65] Creating proxy instance for mode: Forward

# JSON Format
{
  "timestamp": "2025-11-16T11:32:15.123Z",
  "level": "info",
  "target": "bifrost_bridge::proxy",
  "module": "bifrost_bridge::proxy",
  "file": "src/proxy.rs",
  "line": 65,
  "message": "Creating proxy instance for mode: Forward"
}
```
**Enhanced Logging Coverage:**
- **Proxy Factory:** Mode detection, configuration validation, TLS setup
- **Connection Handling:** TCP/HTTPS binding, client connections, error tracking
- **Static File Serving:** Mount resolution, file access, SPA handling
- **Forward/Reverse Proxy:** Request routing, target communication, authentication
- **Error Conditions:** Comprehensive error and warning logging with context
**Dependencies Added:**
- `chrono = { version = "0.4.38", features = ["serde"] }` for timestamp handling
- Enhanced JSON serialization support for structured logging
**Benefits:**
- **Production Ready:** Structured JSON logs for log aggregation systems
- **Flexible Configuration:** Both CLI and JSON configuration support
- **Multiple Outputs:** Simultaneous logging to console and files
- **Level Control:** Granular control over log verbosity per target
- **Monitoring Ready:** Rich context and metadata for observability
- **Backward Compatible:** Existing `RUST_LOG` environment variable support
- **Performance Optimized:** Efficient async logging with minimal overhead

---

## 🎯 Next Priorities

1. **Medium Priority:** Add performance monitoring (R016)
2. **Low Priority:** Add WebSocket support (R017)

## ✅ **Recent Implementations Completed**

- **R014 (Thread Pool):** ✅ **FIXED** - Previously only configuration fields existed, now fully implemented with custom tokio runtime builder that actually uses the `worker_threads` value to configure thread pool size
- **R015 (Logging System):** ✅ **COMPLETED** - Enhanced from basic env_logger to comprehensive structured logging system with JSON support, multiple output targets, CLI configuration, and detailed logging throughout the codebase

---

**Last Updated:** 2025-11-16
**Maintainer:** Development Team
