# Configuration Guide

This guide covers all configuration options for the proxy server, including command-line arguments and JSON configuration files.

## 📋 Table of Contents

- [Command Line Interface](#command-line-interface)
- [JSON Configuration](#json-configuration)
- [Static File Configuration](#static-file-configuration)
- [Multiple Mount Points](#multiple-mount-points)
- [Configuration Inheritance](#configuration-inheritance)
- [Reverse Proxy Headers](#reverse-proxy-headers)
- [Examples](#examples)

## 🖥️ Command Line Interface

### Basic Options

```bash
# Basic usage
cargo run -- [OPTIONS]

# Help
cargo run -- --help
```

### Core Arguments

| Argument | Short | Description | Example |
|----------|-------|-------------|---------|
| `--mode` | `-m` | Proxy mode: `forward` or `reverse` | `--mode reverse` |
| `--listen` | `-l` | Listen address for the server | `--listen 127.0.0.1:8080` |
| `--config` | `-c` | Path to JSON configuration file | `--config config.json` |

### Static File Options

| Argument | Description | Example |
|----------|-------------|---------|
| `--static-dir` | Single static directory (legacy) | `--static-dir ./public` |
| `--spa` | Enable SPA mode for single directory | `--spa` |
| `--mount` | Mount static directory at path | `--mount /app:./dist` |
| `--worker-threads` | Number of worker threads for static file serving | `--worker-threads 8` |

### Connection Options

| Argument | Description | Default |
|----------|-------------|---------|
| `--max-connections` | Maximum concurrent connections | `1000` |
| `--timeout-secs` | Connection timeout in seconds | `30` |

### Proxy Options

| Argument | Description | Example |
|----------|-------------|---------|
| `--forward-proxy-port` | Port for forward proxy mode | `--forward-proxy-port 3128` |
| `--reverse-proxy-target` | Target URL for reverse proxy | `--reverse-proxy-target http://backend:3000` |

### HTTPS Options

| Argument | Description | Example |
|----------|-------------|---------|
| `--private-key` | Path to PKCS#8 PEM format private key file | `--private-key ./certs/private-key.pem` |
| `--certificate` | Path to PEM format certificate file | `--certificate ./certs/certificate.pem` |

### Connection Pool Options

| Argument | Description | Example |
|----------|-------------|---------|
| `--no-connection-pool` | Disable connection pooling (no-pool mode) | `--no-connection-pool` |
| `--pool-max-idle` | Maximum idle connections per host for connection pooling | `--pool-max-idle 20` |

## 📄 JSON Configuration

### Basic Structure

```json
{
  "mode": "Reverse",
  "listen_addr": "127.0.0.1:8080",
  "max_connections": 1000,
  "timeout_secs": 30,
  "reverse_proxy_target": null,
  "static_files": {
    "mounts": [...],
    "enable_directory_listing": false,
    "index_files": ["index.html", "index.htm"],
    "spa_mode": false,
    "spa_fallback_file": "index.html"
  },
  "private_key": null,
  "certificate": null,
  "connection_pool_enabled": true,
  "pool_max_idle_per_host": 10
}
```

### Top-Level Fields

| Field | Type | Description | Default |
|-------|------|-------------|---------|
| `mode` | String | Proxy mode: `"Forward"` or `"Reverse"` | `"Forward"` |
| `listen_addr` | String | Server listen address | `"127.0.0.1:8080"` |
| `max_connections` | Number | Maximum concurrent connections | `1000` |
| `timeout_secs` | Number | Connection timeout in seconds | `30` |
| `reverse_proxy_target` | String | Target URL for reverse proxy | `null` |
| `static_files` | Object | Static file serving configuration | `null` |
| `private_key` | String | Path to PKCS#8 PEM format private key file for HTTPS | `null` |
| `certificate` | String | Path to PEM format certificate file for HTTPS | `null` |
| `connection_pool_enabled` | Boolean | Enable HTTP connection pooling for forward proxy | `true` |
| `pool_max_idle_per_host` | Number | Maximum idle connections per host for connection pooling | `10` |

## 📁 Static File Configuration

### StaticFileConfig Fields

| Field | Type | Description | Default |
|-------|------|-------------|---------|
| `mounts` | Array | List of mount configurations | `[]` |
| `enable_directory_listing` | Boolean | Enable directory listing globally | `false` |
| `index_files` | Array | Default index files | `["index.html", "index.htm"]` |
| `spa_mode` | Boolean | Global SPA mode setting | `false` |
| `spa_fallback_file` | String | Global SPA fallback file | `"index.html"` |
| `worker_threads` | Number | Number of worker threads for static file serving | `None` (uses OS default) |
| `custom_mime_types` | Object | Custom MIME type mappings (extension → MIME type) | `{}` |

### Mount Configuration

Each mount in the `mounts` array supports the following fields:

```json
{
  "path": "/app",
  "root_dir": "./frontend/dist",
  "enable_directory_listing": false,
  "index_files": ["index.html"],
  "spa_mode": true,
  "spa_fallback_file": "index.html"
}
```

#### Mount Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `path` | String | ✅ Yes | URL path prefix (e.g., "/app", "/api") |
| `root_dir` | String | ✅ Yes | Filesystem directory path |
| `enable_directory_listing` | Boolean | ❌ No | Enable directory listing for this mount |
| `index_files` | Array | ❌ No | Index files for this mount |
| `spa_mode` | Boolean | ❌ No | Enable SPA mode for this mount |
| `spa_fallback_file` | String | ❌ No | SPA fallback file for this mount |

**Note:** MIME type mappings are configured at the top-level `static_files` level and are inherited by all mounts automatically.

## 🔗 Multiple Mount Points

### Example Configuration

```json
{
  "mode": "Reverse",
  "listen_addr": "127.0.0.1:8080",
  "static_files": {
    "mounts": [
      {
        "path": "/app",
        "root_dir": "./frontend/dist",
        "spa_mode": true
      },
      {
        "path": "/api",
        "root_dir": "./api-docs",
        "enable_directory_listing": true
      },
      {
        "path": "/assets",
        "root_dir": "./static",
        "enable_directory_listing": false
      }
    ],
    "spa_mode": true,
    "enable_directory_listing": false
  }
}
```

### URL Mapping

| Request URL | Mount Match | File Path |
|-------------|-------------|-----------|
| `http://localhost:8080/app/dashboard` | `/app` | `./frontend/dist/dashboard` |
| `http://localhost:80.80/api/users` | `/api` | `./api-docs/users` |
| `http://localhost:8080/assets/style.css` | `/assets` | `./static/style.css` |

## 🧬 Configuration Inheritance

Mount configurations inherit values from the parent `static_files` configuration when not specified.

### Inheritance Rules

1. **Required fields** (`path`, `root_dir`) must always be specified
2. **Optional fields** inherit from parent if not set
3. **Mount-specific values** always override parent values

### Example: Clean Configuration

```json
{
  "static_files": {
    "spa_mode": true,
    "enable_directory_listing": false,
    "index_files": ["index.html", "index.htm"],
    "spa_fallback_file": "index.html",
    "mounts": [
      {
        "path": "/",
        "root_dir": "./frontend/dist"
        // Inherits: spa_mode=true, enable_directory_listing=false, etc.
      },
      {
        "path": "/docs",
        "root_dir": "./api-docs",
        "enable_directory_listing": true
        // Inherits: spa_mode=true, but overrides directory listing
      }
    ]
  }
}
```

## 🔧 Reverse Proxy Headers

When running in reverse proxy mode, the server automatically adds several HTTP headers to forwarded requests:

### Headers Added

| Header | Description | Example |
|--------|-------------|---------|
| `X-Forwarded-For` | Client IP address (extracted from connection) | `X-Forwarded-For: 192.168.1.100` |
| `X-Forwarded-Proto` | Protocol used by client | `X-Forwarded-Proto: https` |
| `X-Forwarded-Host` | Original Host header | `X-Forwarded-Host: example.com` |
| `X-Proxy-Server` | Proxy server identification | `X-Proxy-Server: rust-reverse-proxy` |

### Important Notes

- **Client IP Extraction:** The `X-Forwarded-For` header contains the actual client IP address extracted from the TCP connection, not a hardcoded value
- **Backend Access:** Backend servers can use the `X-Forwarded-For` header to log the real client IP addresses
- **Security:** The actual client IP is critical for access control, rate limiting, and security auditing
- **Multiple Proxies:** If requests pass through multiple proxies, this header preserves the entire chain

### Example Backend Usage

**Node.js/Express:**
```javascript
const clientIP = req.headers['x-forwarded-for'] || req.socket.remoteAddress;
console.log('Client IP:', clientIP);
```

**Python/Flask:**
```python
client_ip = request.headers.get('X-Forwarded-For', request.remote_addr)
print(f"Client IP: {client_ip}")
```

## 💡 Usage Examples

### Example 1: Simple SPA Server

**Command Line:**
```bash
cargo run -- --mode reverse --listen 127.0.0.1:8080 --static-dir ./dist --spa
```

**JSON Configuration:**
```json
{
  "mode": "Reverse",
  "listen_addr": "127.0.0.1:8080",
  "static_files": {
    "mounts": [
      {
        "path": "/",
        "root_dir": "./dist",
        "spa_mode": true
      }
    ]
  }
}
```

### Example 2: Multi-Mount Development Server

**Command Line:**
```bash
cargo run -- \
  --mount /app:./frontend/dist \
  --mount /api:./api-docs \
  --mount /assets:./static
```

**JSON Configuration:**
```json
{
  "mode": "Reverse",
  "listen_addr": "127.0.0.1:3000",
  "static_files": {
    "spa_mode": true,
    "enable_directory_listing": true,
    "mounts": [
      {
        "path": "/app",
        "root_dir": "./frontend/dist"
      },
      {
        "path": "/api",
        "root_dir": "./api-docs"
      },
      {
        "path": "/assets",
        "root_dir": "./static"
      }
    ]
  }
}
```

### Example 3: HTTPS Static Server

**Command Line:**
```bash
cargo run -- \
  --mode reverse \
  --listen 127.0.0.1:8443 \
  --private-key ./certs/private-key.pem \
  --certificate ./certs/certificate.pem \
  --static-dir ./dist \
  --spa
```

**JSON Configuration:**
```json
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

### Example 4: Production Microservices with HTTPS

**JSON Configuration:**
```json
{
  "mode": "Reverse",
  "listen_addr": "0.0.0.0:443",
  "max_connections": 5000,
  "timeout_secs": 60,
  "static_files": {
    "enable_directory_listing": false,
    "spa_mode": false,
    "mounts": [
      {
        "path": "/user-service",
        "root_dir": "/var/www/user-service/dist",
        "spa_mode": true
      },
      {
        "path": "/admin-panel",
        "root_dir": "/var/www/admin-panel/dist",
        "spa_mode": true
      },
      {
        "path": "/api-docs",
        "root_dir": "/var/www/api-docs",
        "enable_directory_listing": true
      }
    ]
  },
  "private_key": "/etc/ssl/private/app.key",
  "certificate": "/etc/ssl/certs/app.crt"
}
```

---

**Last Updated:** 2025-11-16
**See Also:** [Examples](./examples.md), [CLI Reference](./cli-reference.md)
