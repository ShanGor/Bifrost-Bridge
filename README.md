# Bifrost Bridge
A high-performance proxy server written in Rust that can function as both a forward proxy (正向代理) and reverse proxy (反向代理), combining capabilities of both Nginx and Squid.

## Features

- **Forward proxy mode** with HTTP/HTTPS CONNECT support, relay proxies, auth, and connection pooling
- **Reverse proxy mode** with connection pooling, configurable headers, and optional backend health checks
- **Static file server** with SPA fallback, multiple mount points, custom MIME types, and TLS support
- **Combined reverse proxy + static handler** so static routes and backend routes share a single listener
- **Tokio-based runtime** with configurable worker thread count for reverse proxy + static workloads
- **Granular timeout and limit controls** (connect/idle/lifetime timeouts, header size, connection caps)
- **Rate limiting & monitoring hooks** via the built-in rate limiter and optional Prometheus endpoint
- **CLI and JSON configuration** plus sample config generator and logging customization
- **Graceful shutdown & logging** with Ctrl+C handling and env_logger/CustomLogger backends
- **Encrypted configuration secrets** with AES-256 `{encrypted}` payloads backed by a masked key on disk

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

# Initialize the local encryption key (writes to ~/.bifrost)
cargo run -- --init-encryption-key

# Encrypt a secret (reads from stdin when value omitted)
echo "relay-secret" | cargo run -- --encrypt
```

### Secret Encryption Workflow

1. **Initialize Key Material**  
   Run `cargo run -- --init-encryption-key` once per machine. This creates `~/.bifrost/master_key.*` files with a masked AES-256 key (3 fragments + XOR mask) and enforces `0700` permissions.

2. **Encrypt Secrets**  
   Use `cargo run -- --encrypt <payload>` to encrypt short secrets. When `<payload>` is omitted the CLI reads from stdin, so you can pipe secrets from external tools:
   ```bash
   $ echo "relayPassword!" | cargo run -- --encrypt
   {encrypted}QmFzZTY0Tm9uY2VDb2RlCg==
   ```
   Copy the full `{encrypted}...` token.

3. **Reference in Config**  
   Place the token anywhere a secret is expected in `config.json`, for example:
   ```json
   {
     "relay_proxies": [{
       "relay_proxy_url": "https://relay.internal:8443",
       "relay_proxy_username": "service",
       "relay_proxy_password": "{encrypted}QmFzZTY0Tm9uY2VDb2RlCg=="
     }]
   }
   ```

4. **Automatic Decryption**  
   When the proxy boots it scans configuration fields for the `{encrypted}` prefix, reconstructs the AES key from `~/.bifrost`, decrypts secrets before use, and emits Prometheus counters plus logs for observability.

> **Tip:** Set the `BIFROST_SECRET_HOME` environment variable to override the default `~/.bifrost` directory—handy during testing.

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
```
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

The reverse proxy mode routes incoming requests to backend servers. Use predicate-based routes (similar to Spring Cloud Gateway) to target different upstreams.

### Usage Example (predicate routes)

```json
{
  "mode": "Reverse",
  "listen_addr": "127.0.0.1:8080",
  "reverse_proxy_routes": [
    {
      "id": "api",
      "target": "http://localhost:3000",
      "strip_path_prefix": "/api",
      "predicates": [
        { "type": "Path", "patterns": ["/api/{segment}", "/api/**"], "match_trailing_slash": true }
      ]
    },
    {
      "id": "docs",
      "target": "http://localhost:4000",
      "predicates": [
        { "type": "Host", "patterns": ["docs.local"] },
        { "type": "Path", "patterns": ["/docs/**"], "match_trailing_slash": true }
      ]
    }
  ]
}
```

Run with `cargo run -- --mode reverse --listen 127.0.0.1:8080 -c your-config.json`.

### Features

- Predicate-based routing (Path/Host/Method/Header/etc.) with priorities and weights
- Optional path-prefix stripping per route
- X-Forwarded-* headers and host preservation
- Connection pooling (per route) and health checks
- Static file serving + SPA support

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
- `cache_millisecs`: Cache duration in seconds for static files (default: `3600`)
- `no_cache_files`: Array of file patterns that should receive no-cache headers

### Cache Control Configuration

**Global Cache Settings:**
```json
{
  "static_files": {
    "cache_millisecs": 7200,
    "no_cache_files": ["*.html", "*.json", "config.js"]
  }
}
```

**Per-Mount Cache Settings:**
```json
{
  "static_files": {
    "mounts": [
      {
        "path": "/app",
        "root_dir": "./app",
        "spa_mode": true,
        "cache_millisecs": 1800,
        "no_cache_files": ["*.html", "manifest.json"]
      },
      {
        "path": "/assets",
        "root_dir": "./assets",
        "cache_millisecs": 86400,
        "no_cache_files": ["*.js"]
      }
    ]
  }
}
```

**No-Cache File Patterns:**
- `*.js` - All JavaScript files (extension pattern)
- `config.json` - Exact filename match
- `*.html` - All HTML files
- Patterns are case-insensitive
- SPA index files automatically get no-cache headers in SPA mode

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

### Overview

Bifrost Bridge runs a single multi-threaded Tokio runtime per process. At startup the CLI/config data
is converted into a `Config`, then `ProxyFactory` instantiates the one adapter that matches that mode:

- **Forward proxy** – `ForwardProxyAdapter` binds a listener and drives `ForwardProxy`.
- **Reverse proxy** – `ReverseProxyAdapter` binds a listener and drives `ReverseProxy`.
- **Static-only** – `StaticFileProxyAdapter` serves mount points and SPA fallbacks.
- **Combined reverse + static** – `CombinedProxyAdapter` routes requests between the reverse proxy
  and the `StaticFileHandler` so both share the same port.

```
┌────────────────────────────────────────────────────────────────┐
│           Tokio Runtime                                        │
├────────────────────────────────────────────────────────────────┤
│ ProxyFactory                                                   │
│   ├ ForwardProxyAdapter  ─▶ ForwardProxy                       │
│   ├ ReverseProxyAdapter  ─▶ ReverseProxy                       │
│   ├ StaticFileProxyAdapter ─▶ StaticFileHandler                │
│   └ CombinedProxyAdapter ─▶ ReverseProxy + StaticFileHandler   │
└────────────────────────────────────────────────────────────────┘
```

All adapters share the same runtime, connection limits, and logging pipeline. Reverse proxy mode can
optionally embed the static handler; forward proxy mode cannot. Monitoring/rate limiting hooks are
enabled per adapter when configuration requests them.

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

#### Proxy Layer
- **`ProxyFactory`**: Reads the `Config` and constructs the adapter for forward, reverse, static, or combined mode.
- **`ForwardProxy`**: Handles HTTP/HTTPS proxying with optional relay proxies, authentication, and connection pooling.
- **`ReverseProxy`**: Proxies to a single backend with pooling, optional health checks, and rate limiting hooks.
- **`StaticFileHandler`** / **`StaticFileProxyAdapter`**: Serves mounted directories, SPA fallbacks, and TLS/static HTTP modes.
- **`CombinedProxyAdapter`**: Shares one listener while routing static paths to the handler and everything else to the reverse proxy.

#### Resource & Connection Management
- **Connection pooling**: Forward proxy can enable/disable pooling per CLI option; reverse proxy exposes per-host pool sizing and idle timeouts.
- **Rate limiting**: Shared `RateLimiter` enforces per-adapter rules when configured.
- **Monitoring**: `MonitoringServer` can export Prometheus metrics; adapters pass metrics handles to reverse/static handlers when monitoring is enabled.
- **Tokio runtime threads**: `worker_threads` controls the number of runtime worker threads for reverse/static workloads (forward mode uses the default runtime).

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

The running server relies on explicit timeouts, status-code mapping, and logging to surface problems.
Most failures are reported back to the client (e.g., `502 Bad Gateway` from the reverse proxy or
`429 Too Many Requests` from the rate limiter) and always logged with context.

### Built-in Safeguards
- **Configuration validation** rejects invalid listen addresses, missing reverse targets, or
  unsupported flag combinations before the runtime starts.
- **Timeout enforcement** (`connect`, `idle`, `max_connection_lifetime`) ensures both proxy modes
  tear down unhealthy connections automatically.
- **Reverse proxy resilience**: Backend errors propagate as HTTP errors while the proxy keeps its
  listener alive; optional health checks monitor the backend.
- **Static handler responses**: Unsupported methods return 405, missing files return 404, and SPA
  fallbacks keep SPAs functional without custom error logic.
- **Rate limiting**: When enabled, the shared rate limiter responds with `429` and optional
  `Retry-After` headers so abusive clients back off.

### Operational Visibility
- **Structured logging**: env_logger or the custom logging backend captures request outcomes and
  startup/shutdown events.
- **Monitoring server**: When enabled in config, Prometheus metrics expose connection counts,
  request timing, and rate-limiter events for the reverse/static adapters.

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

- **[Configuration Guide](docs/configuration.md)** - Comprehensive configuration options (CLI and JSON)
- **[Quick Start Guide](docs/quick-start.md)** - Get started quickly with examples
- **[Installation Guide](docs/installation.md)** - Setup and installation instructions
- **[HTTPS Setup Guide](docs/https-setup.md)** - SSL/TLS configuration
- **[Error Recovery Architecture](docs/error-recovery-architecture.md)** - Error handling and recovery mechanisms
- **[Development Guide](docs/development.md)** - Development setup and guidelines
- **[Requirements](requirements/)** - Detailed requirements and implementation status
- **[Configuration Examples](examples/)** - Ready-to-use configuration file examples

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
