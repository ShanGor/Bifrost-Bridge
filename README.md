# Bifrost Bridge
A high-performance proxy server written in Rust that can function as both a forward proxy (正向代理) and reverse proxy (反向代理), combining capabilities of both Nginx and Squid.

## Features

- **Worker Separation Architecture**: Isolated workers for forward vs reverse proxy with separate resources, connection pools, and metrics
- **Advanced Error Recovery**: Circuit breaker pattern, worker isolation, and automatic retry with exponential backoff
- **Contextual Error Handling**: Rich error context with severity classification and recovery suggestions
- **Forward Proxy**: Acts as a forward proxy similar to Squid
- **Reverse Proxy**: Acts as a reverse proxy similar to Nginx
- **Multiple Static Roots**: Serve static files from multiple directories at different URL paths
- **High Performance**: Built on Tokio and Hyper for async I/O with configurable worker threads
- **Configurable**: Supports both command-line arguments and configuration files
- **Connection Pooling**: Efficient connection reuse with configurable pool settings per proxy type
- **Granular Timeout Control**: Three distinct timeout types (connect, idle, lifetime)
- **SPA Support**: Single Page Application fallback with proper MIME type handling
- **Optimized Static File Serving**: Uses Tokio worker threads for CPU-intensive operations (directory listings, MIME type detection)
- **Resource Isolation**: Independent resource limits and controls for each proxy type
- **Isolated Metrics**: Separate performance monitoring and debugging per proxy type
- **Graceful Shutdown**: Clean shutdown with Ctrl+C handling
- **Logging**: Comprehensive logging with env_logger

## Installation

### Prerequisites

- Rust 1.70+ (recommended)
- Cargo package manager

### Building from Source

```bash
git clone <repository-url>
cd proxy-server
cargo build --release
```

## Usage

### Command Line Options

```bash
# Generate sample configuration files
cargo run -- --generate-config config.json

# Run as forward proxy
cargo run -- --mode forward --listen 127.0.0.1:8080

# Run as reverse proxy
cargo run -- --mode reverse --listen 127.0.0.1:8080 --target http://backend:3000

# Using configuration file
cargo run -- --config config.json

# Set custom timeouts
cargo run -- --mode forward --listen 127.0.0.1:8080 \
  --connect-timeout 10 \
  --idle-timeout 90 \
  --max-connection-lifetime 300

# Serve static files
cargo run -- --mode reverse --listen 127.0.0.1:8080 --static-dir ./public

# Serve static files with SPA mode
cargo run -- --mode reverse --listen 127.0.0.1:8080 --static-dir ./dist --spa

# Serve static files with configurable worker threads for better performance
cargo run -- --mode reverse --listen 127.0.0.1:8080 --static-dir ./public --worker-threads 8

# Serve static files from multiple directories
cargo run -- --mode reverse --listen 127.0.0.1:8080 \
  --mount "/app:/path/to/app/dist" \
  --mount "/api:/path/to/api/docs" \
  --mount "/assets:/path/to/static/assets" \
  --worker-threads 4

# Mix single directory with multiple mounts
cargo run -- --mode reverse --listen 127.0.0.1:8080 \
  --static-dir ./main-app --spa \
  --mount "/admin:/path/to/admin-panel" \
  --mount "/docs:/path/to/documentation" \
  --worker-threads 6
```

### Multiple Static Roots

The proxy server supports serving static files from multiple directories at different URL paths, making it perfect for complex applications with multiple static content sources.

#### CLI Usage Examples

**Single Directory (Backward Compatible):**
```bash
cargo run -- --mode reverse --listen 127.0.0.1:8080 --static-dir ./dist --spa
```

**Multiple Mount Points:**
```bash
cargo run -- --mode reverse --listen 127.0.0.1:8080 \
  --mount "/app:/path/to/frontend/dist" \
  --mount "/api:/path/to/api/docs" \
  --mount "/assets:/path/to/static/files"
```

**Mixed Configuration:**
```bash
# Main application with SPA support
cargo run -- --mode reverse --listen 127.0.0.1:8080 \
  --static-dir ./main-app --spa \
  --mount "/admin:/path/to/admin-panel" \
  --mount "/docs:/path/to/api-documentation"
```

#### URL Mapping

- `/app/*` → `/path/to/frontend/dist/*`
- `/api/*` → `/path/to/api/docs/*`
- `/assets/*` → `/path/to/static/files/*`
- `/` → `/path/to/main-app/*` (main directory)

#### Multiple Mounts JSON Configuration

**Multi-Mount Example with Threading:**
```json
{
  "mode": "Reverse",
  "listen_addr": "127.0.0.1:8080",
  "static_files": {
    "mounts": [
      {
        "path": "/app",
        "root_dir": "./frontend/dist",
        "spa_mode": true,
        "spa_fallback_file": "index.html",
        "enable_directory_listing": false,
        "index_files": ["index.html"]
      },
      {
        "path": "/api",
        "root_dir": "./api-docs",
        "spa_mode": false,
        "enable_directory_listing": true,
        "index_files": ["index.html"]
      },
      {
        "path": "/assets",
        "root_dir": "./static",
        "spa_mode": false,
        "enable_directory_listing": false,
        "index_files": ["index.html"]
      }
    ],
    "enable_directory_listing": false,
    "index_files": ["index.html", "index.htm"],
    "spa_mode": false,
    "spa_fallback_file": "index.html",
    "worker_threads": 8
  }
}

### Configuration File

Create a JSON configuration file:

#### Forward Proxy Example (forward.json)
```json
{
  "mode": "Forward",
  "listen_addr": "127.0.0.1:8080",
  "forward_proxy_port": 3128,
  "max_connections": 1000,
  "connect_timeout_secs": 10,
  "idle_timeout_secs": 90,
  "max_connection_lifetime_secs": 300
}
```

#### Reverse Proxy Example (reverse.json)
```json
{
  "mode": "Reverse",
  "listen_addr": "127.0.0.1:8080",
  "reverse_proxy_target": "http://backend.example.com:3000",
  "max_connections": 1000,
  "connect_timeout_secs": 10,
  "idle_timeout_secs": 90,
  "max_connection_lifetime_secs": 300
}
```

#### SPA Example with Threading (spa.json)
```json
{
  "mode": "Reverse",
  "listen_addr": "127.0.0.1:8080",
  "reverse_proxy_target": "http://api.backend.com:3000",
  "max_connections": 1000,
  "connect_timeout_secs": 10,
  "idle_timeout_secs": 90,
  "max_connection_lifetime_secs": 300,
  "static_files": {
    "mounts": [
      {
        "path": "/",
        "root_dir": "./dist"
      }
    ],
    "enable_directory_listing": false,
    "index_files": ["index.html", "index.htm"],
    "spa_mode": true,
    "spa_fallback_file": "index.html",
    "worker_threads": 4
  }
}
```

## Forward Proxy (正向代理)

The forward proxy mode allows clients to make requests through the proxy to external websites. This is similar to Squid functionality.

### Usage Example

1. Start the forward proxy:
```bash
cargo run -- --mode forward --listen 127.0.0.1:8080
```

2. Configure your browser or application to use `127.0.0.1:8080` as the HTTP proxy

3. The proxy will forward requests to the target servers

### Features

- HTTP proxying
- Connection pooling with configurable idle and lifetime settings
- Granular timeout control (connect, idle, lifetime)
- Request timeout handling
- Error handling and logging

### Limitations

- HTTPS tunneling (CONNECT method) is not fully implemented in this example
- Authentication is not included

## Reverse Proxy (反向代理)

The reverse proxy mode routes incoming requests to backend servers. This is similar to Nginx reverse proxy functionality.

### Usage Example

1. Start the reverse proxy:
```bash
cargo run -- --mode reverse --listen 127.0.0.1:8080 --target http://localhost:3000
```

2. Access services through the proxy at `http://127.0.0.1:8080`

3. Requests will be forwarded to the backend server

### Features

- HTTP reverse proxying
- URL rewriting
- X-Forwarded-* headers
- Host header preservation
- Connection pooling
- Static file serving
- SPA (Single Page Application) support

### Headers Added

The reverse proxy automatically adds the following headers:

- `X-Forwarded-For`: Client IP address
- `X-Forwarded-Proto`: Protocol used by client
- `X-Forwarded-Host`: Original Host header
- `X-Proxy-Server`: Proxy server identification

## SPA (Single Page Application) Support

The proxy server includes built-in support for serving Single Page Applications with client-side routing.

### SPA Mode Features

- **Fallback to index file**: When a file is not found, automatically serves the configured fallback file (typically `index.html`)
- **Client-side routing support**: Enables frameworks like React, Vue, Angular, etc. to handle routing
- **Static asset serving**: Serves CSS, JS, images, and other static files normally
- **Custom fallback file**: Configure any file as the SPA fallback

### Usage Examples

#### CLI Usage
```bash
# Serve SPA from ./dist directory
cargo run -- --mode reverse --listen 127.0.0.1:8080 --static-dir ./dist --spa

# Custom fallback file
cargo run -- --mode reverse --listen 127.0.0.1:8080 --static-dir ./dist --spa --spa-fallback app.html
```

#### Configuration File
```json
{
  "mode": "Reverse",
  "listen_addr": "127.0.0.1:8080",
  "static_files": {
    "root_dir": "./dist",
    "spa_mode": true,
    "spa_fallback_file": "index.html",
    "enable_directory_listing": false
  }
}
```

### How SPA Mode Works

1. **Existing files**: If the requested file exists (e.g., `/main.js`, `/styles.css`), it's served normally
2. **Missing files**: If the file doesn't exist (e.g., `/dashboard`, `/profile`, `/api/users`), the proxy serves the `spa_fallback_file` (typically `index.html`)
3. **Directory requests**: Directories without index files also fall back to the SPA fallback file

### Popular Framework Examples

#### React SPA
```json
{
  "static_files": {
    "root_dir": "./build",
    "spa_mode": true,
    "spa_fallback_file": "index.html"
  }
}
```

#### Vue.js SPA
```json
{
  "static_files": {
    "root_dir": "./dist",
    "spa_mode": true,
    "spa_fallback_file": "index.html"
  }
}
```

#### Angular SPA
```json
{
  "static_files": {
    "root_dir": "./dist/your-app-name",
    "spa_mode": true,
    "spa_fallback_file": "index.html"
  }
}
```

### Configuration Options

- `spa_mode`: `true`/`false` - Enable SPA fallback behavior
- `spa_fallback_file`: Filename to serve for non-existent routes (default: `"index.html"`)
- `root_dir`: Directory containing the built SPA assets
- `enable_directory_listing`: Recommended to set to `false` for SPAs

### CLI Arguments

- `--static-dir <DIR>`: Serve static files from this directory
- `--spa`: Enable SPA mode (fallback to index file for non-found routes)
- `--spa-fallback <FILE>`: Custom SPA fallback file name (default: `index.html`)
- `--worker-threads <NUM>`: Number of worker threads for static file serving (default: CPU cores)

## Performance and Threading

The proxy server uses Tokio's runtime with configurable worker threads for optimal performance when serving static files.

### Worker Thread Configuration

**Command Line:**
```bash
# Use 8 worker threads for static file operations
cargo run -- --mode reverse --listen 127.0.0.1:8080 --static-dir ./public --worker-threads 8

# Use auto-detected CPU core count
cargo run -- --mode reverse --listen 127.0.0.1:8080 --static-dir ./public
```

**Configuration File:**
```json
{
  "static_files": {
    "worker_threads": 8,
    "mounts": [...]
  }
}
```

### Threading Implementation

The server uses **Tokio worker threads** for CPU-intensive static file operations:

- **Directory Listings**: CPU-intensive directory traversal and HTML generation run in `tokio::spawn_blocking`
- **MIME Type Detection**: File extension processing and MIME type lookup use blocking threads
- **File Operations**: File metadata and content reading remain async for I/O efficiency
- **Non-blocking**: Main async runtime stays responsive while CPU work happens in dedicated threads

### Performance Benefits

- **High Concurrency**: Multiple requests processed simultaneously without blocking
- **Optimized Resource Usage**: Tokio efficiently schedules CPU work across worker threads
- **Scalability**: Configurable thread count adapts to different hardware capabilities
- **Responsive**: Async operations continue while CPU-intensive work runs in background

### Recommended Settings

- **Small servers**: `--worker-threads 2-4`
- **Medium servers**: `--worker-threads 4-8`
- **Large servers**: `--worker-threads 8-16`
- **Default**: Auto-detect CPU core count

## Architecture

### 🏗️ Worker Separation Architecture

Bifrost Bridge implements a **hybrid worker separation architecture** that provides both security isolation and operational efficiency. This design ensures that forward proxy and reverse proxy operations run in completely isolated environments while sharing a common tokio runtime for efficiency.

#### Architecture Overview

```
┌─────────────────────────────────────────────────────────────┐
│                    Shared Tokio Runtime                      │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  ┌─────────────────┐  ┌─────────────────┐  ┌──────────────┐ │
│  │ Forward Proxy   │  │ Reverse Proxy   │  │ Static Files │ │
│  │   Worker        │  │    Worker       │  │   Worker     │ │
│  │                 │  │                 │  │              │ │
│  │ • Isolated      │  │ • Isolated      │  │ • Isolated   │ │
│  │ • Dedicated     │  │ • Dedicated     │  │ • Dedicated  │ │
│  │ • Resources     │  │ • Resources     │  │ • Resources  │ │
│  │ • Metrics       │  │ • Metrics       │  │ • Metrics    │ │
│  └─────────────────┘  └─────────────────┘  └──────────────┘ │
│         │                   │                   │           │
│  ┌─────────────────┐  ┌─────────────────┐  ┌──────────────┐ │
│  │ Connection      │  │ Connection      │  │ File         │ │
│  │ Pool Manager    │  │ Pool Manager    │  │ System       │ │
│  └─────────────────┘  └─────────────────┘  └──────────────┘ │
└─────────────────────────────────────────────────────────────┘
```

#### Key Benefits

**🔒 Security & Isolation**
- Separate worker processes for forward vs reverse proxy
- Independent resource limits and controls
- Isolated metrics collection per proxy type
- Prevents resource contention between proxy types

**⚡ Performance & Efficiency**
- Shared tokio runtime for optimal resource utilization
- Dedicated connection pools optimized for traffic patterns
- Proxy-specific configuration tuning
- Zero resource sharing between workers

**🔧 Operational Excellence**
- Independent monitoring and debugging per proxy type
- Granular resource control and limits
- Type-specific connection pooling strategies
- Configurable worker isolation levels

### Project Structure

```
src/
├── main.rs           # Main application entry point
├── lib.rs           # Library exports
├── config.rs        # Configuration management
├── error.rs         # Error handling
├── proxy.rs         # Proxy factory and IsolatedProxyAdapter
├── forward_proxy.rs # Forward proxy implementation
├── reverse_proxy.rs # Reverse proxy implementation
├── static_files.rs  # Static file serving with Tokio threading
└── common.rs        # Shared utilities and worker architecture
```

### Core Architecture Components

#### Worker Separation Layer
- **`ProxyType`**: Enumeration for ForwardProxy, ReverseProxy, StaticFiles, Combined modes
- **`IsolatedWorker`**: Dedicated worker with isolated resources, metrics, and configuration
- **`WorkerResourceLimits`**: Per-worker resource controls (connections, memory, CPU)
- **`WorkerConfiguration`**: Proxy-specific settings and optimization parameters

#### Resource Management
- **`ConnectionPoolManager`**: Type-specific connection pooling with independent settings
- **`PerformanceMetrics`**: Isolated metrics collection per proxy type
- **`WorkerManager`**: Coordinates isolated workers within shared runtime

#### Proxy Adapters
- **`IsolatedProxyAdapter`**: New adapter using worker-separated architecture
- **`SharedServer` trait**: Enhanced trait supporting proxy type separation
- **Traditional adapters**: Backward compatibility with existing implementations

### Key Components

1. **ProxyFactory**: Creates appropriate proxy instances with worker isolation
2. **IsolatedWorker**: Dedicated worker resources for each proxy type
3. **ForwardProxy**: Forward proxy with isolated worker and connection pool
4. **ReverseProxy**: Reverse proxy with isolated worker and connection pool
5. **StaticFileHandler**: Static file serving with dedicated worker and thread pool
6. **ConnectionPoolManager**: Type-specific connection pooling strategies
7. **PerformanceMetrics**: Isolated monitoring per proxy type

## Performance Considerations

- Built on Tokio for async I/O with configurable worker threads
- Optimized static file serving with CPU-intensive operations in dedicated blocking threads
- Connection pooling for backend requests with configurable pool settings
- Idle timeout for efficient pool management
- Maximum connection lifetime prevents stale connections
- Configurable connection limits
- Granular timeout handling (connect, idle, lifetime) to prevent hanging requests
- Thread pool configuration for static file operations (directory listings, MIME type detection)

## Error Handling & Recovery

The proxy includes sophisticated error handling and recovery mechanisms:

### Error Classification System
- **Severity Levels**: Low, Medium, High, Critical error classification
- **Contextual Errors**: Rich error context with worker ID, operation, metadata
- **Recovery Actions**: Automatic suggestions for error recovery strategies
- **Error History**: Comprehensive error tracking and statistics

### Circuit Breaker Pattern
- **Cascade Failure Prevention**: Automatic circuit breaking to prevent system overload
- **Configurable Thresholds**: Customizable failure/success thresholds and timeouts
- **State Management**: Closed → Open → HalfOpen → Closed state transitions
- **Timeout Recovery**: Automatic recovery attempts after configurable timeouts

### Worker Health Monitoring
- **Health Checks**: Continuous worker health monitoring with periodic checks
- **Automatic Recovery**: Worker restart and recovery mechanisms
- **Isolation**: Automatic worker isolation for problematic instances
- **Resource Limits**: Enforcement of resource limits to prevent resource exhaustion

### Retry Mechanisms
- **Exponential Backoff**: Intelligent retry with exponential backoff delays
- **Max Retry Limits**: Configurable maximum retry attempts per operation
- **Circuit Breaker Integration**: Retry attempts coordinated with circuit breaker state

### Graceful Degradation
- **System Resilience**: Continued operation under error load conditions
- **Resource Contention**: Handling of resource conflicts and contention
- **Performance Monitoring**: Real-time performance metrics and error statistics

### Traditional Error Handling
- Connection errors with connect timeout
- HTTP parsing errors
- Configuration errors
- Timeout errors (connect, idle, lifetime)
- Forward error responses to clients

## Logging

Configure logging using environment variables:

```bash
# Default log level
RUST_LOG=info cargo run

# Debug logging
RUST_LOG=debug cargo run

# Trace logging
RUST_LOG=trace cargo run
```

## Testing

Run the test suite:

```bash
cargo test
```

## Examples

### Basic Forward Proxy

```bash
# Start forward proxy
cargo run -- --mode forward --listen 0.0.0.0:8080

# Test with curl
curl -x http://localhost:8080 http://example.com
```

### Basic Reverse Proxy

```bash
# Start a simple backend server
python -m http.server 3000

# Start reverse proxy
cargo run -- --mode reverse --listen 0.0.0.0:8080 --target http://localhost:3000

# Test with curl
curl http://localhost:8080
```

## Future Enhancements

Potential improvements for production use:

- HTTPS/TLS support
- Authentication and authorization
- Load balancing for multiple backends
- Health checking
- Metrics and monitoring
- Web interface for configuration
- WebSocket proxying
- HTTP/2 support

## Documentation

- **[Worker Separation Architecture](docs/worker-separation-architecture.md)** - Detailed architecture documentation
- **[Configuration Guide](docs/configuration.md)** - Comprehensive configuration options
- **[Installation Guide](docs/installation.md)** - Setup and installation instructions
- **[Requirements](requirements/)** - Detailed requirements and implementation status

## Requirements

All project requirements, features, and implementation details are documented in the [requirements/](requirements/) folder with individual files for each requirement:

- [Requirements Index](requirements/README.md) - Overview of all requirements
- [R001: Multiple Static Roots](requirements/R001-multiple-static-roots.md)
- [R021: Tokio Worker Threads](requirements/R021-tokio-worker-threads.md) - Latest implementation
- And many more...

## License

This project is licensed under the MIT License.

## Contributing

1. Fork the repository
2. Create a feature branch
3. Make your changes
4. Add tests
5. Submit a pull request
