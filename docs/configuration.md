# Configuration Guide

This guide covers all configuration options for the proxy server, including command-line arguments and JSON configuration files.

## 📋 Table of Contents

- [Command Line Interface](#command-line-interface)
- [JSON Configuration](#json-configuration)
- [Terminology](#terminology)
- [Static File Configuration](#static-file-configuration)
- [Multiple Mount Points](#multiple-mount-points)
- [Configuration Inheritance](#configuration-inheritance)
- [Multi-Target Reverse Proxy Routing](#multi-target-reverse-proxy-routing)
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
| `--target` | `-t` | Target URL for reverse proxy | `--target http://backend:3000` |
| `--generate-config` | | Generate sample configuration file | `--generate-config config.json` |

### Static File Options

| Argument | Description | Example |
|----------|-------------|---------|
| `--static-dir` | Single static directory (legacy) | `--static-dir ./public` |
| `--spa` | Enable SPA mode for single directory | `--spa` |
| `--spa-fallback` | SPA fallback file name | `--spa-fallback index.html` |
| `--mount` | Mount static directory at path | `--mount /app:./dist` |
| `--worker-threads` | Number of worker threads for static file serving | `--worker-threads 8` |
| `--mime-type` | Custom MIME type mapping | `--mime-type mjs:application/javascript` |

### Timeout Options

| Argument | Description | Default |
|----------|-------------|---------|
| `--connect-timeout` | Connection timeout in seconds | None |
| `--idle-timeout` | Idle timeout in seconds | None |
| `--max-connection-lifetime` | Maximum connection lifetime in seconds | None |
| `--timeout` | Request timeout in seconds (deprecated) | None |

### Proxy Options

| Argument | Description | Example |
|----------|-------------|---------|
| `--proxy-username` | Username for proxy authentication | `--proxy-username admin` |
| `--proxy-password` | Password for proxy authentication | `--proxy-password secret` |

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

### Advanced Options

| Argument | Description | Example |
|----------|-------------|---------|
| `--max-header-size` | Maximum HTTP header size in bytes | `--max-header-size 8192` |
| `--log-level` | Set logging level (trace, debug, info, warn, error) | `--log-level debug` |
| `--log-format` | Set log output format (text, json) | `--log-format json` |

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
| `mode` | String | Proxy mode: `"Forward"`, `"Reverse"`, or `"Combined"` | `"Forward"` |
| `listen_addr` | String | Server listen address | `"127.0.0.1:8080"` |
| `max_connections` | Number | Maximum concurrent connections | `1000` |
| `timeout_secs` | Number | Connection timeout in seconds | `30` |
| `reverse_proxy_target` | String | Legacy single target for reverse proxy (use `reverse_proxy_routes` instead) | `null` |
| `reverse_proxy_routes` | Array | Route list for reverse proxy (id, target, predicates, optional strip/pooling) | `[]` |
| `static_files` | Object | Static file serving configuration | `null` |
| `private_key` | String | Path to PKCS#8 PEM format private key file for HTTPS | `null` |
| `certificate` | String | Path to PEM format certificate file for HTTPS | `null` |
| `connection_pool_enabled` | Boolean | Enable HTTP connection pooling for forward proxy | `true` |
| `pool_max_idle_per_host` | Number | Maximum idle connections per host for connection pooling | `10` |
| `logging` | Object | Logging configuration (see below) | Default console logging |
| `monitoring` | Object | Monitoring endpoints configuration (see below) | Enabled with default endpoints |

## Terminology

If a term is unfamiliar (route, predicate, target, sticky session, header override, load balancing),
see the [glossary](./glossary.md). The sections below refer back to those definitions to avoid
duplicating explanations across docs.

## 📝 Logging Configuration

```json
{
  "logging": {
    "level": "info",
    "format": "text",
    "targets": [
      {
        "type": "stdout",
        "level": "info"
      }
    ]
  }
}
```

## 🔀 Reverse Proxy Routes (Predicate-Based)

Use `reverse_proxy_routes` to configure multiple upstream targets with predicate-driven matching.
Each route requires an `id`, at least one predicate, and either `target` (single target) or
`targets` (multi-target). See the [glossary](./glossary.md) for the meaning of routes, predicates,
and targets.

### Route Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `id` | String | ✅ Yes | Unique route id |
| `target` | String | ✅ Yes* | Upstream URL (single target) |
| `targets` | Array | ✅ Yes* | Multi-target list (see below) |
| `predicates` | Array | ✅ Yes | One or more predicates (all must pass) |
| `priority` | Number | ❌ No | Lower wins; ties use declaration order |
| `reverse_proxy_config` | Object | ❌ No | Per-route pooling/health checks |
| `strip_path_prefix` | String | ❌ No | Remove prefix before forwarding (e.g., `"/test"` → `/api`) |
| `retry_policy` | Object | ❌ No | Retry policy for upstream failures (see below) |

*Either `target` or `targets` is required. Defining both is invalid.

### Routing Guidelines

- Keep predicates specific and use `priority` to resolve overlaps deterministically.
- Use `strip_path_prefix` when upstreams do not expect the public-facing prefix.
- Prefer `targets` with weights for uneven capacity; disable a target to drain traffic.
- Use header overrides only for trusted clients and keep the allowlist narrow.
- Enable sticky sessions only when application state is not shared across targets.

### Supported Predicates
- `Path` with `patterns` (Ant-style) and `match_trailing_slash`
- `Host` with patterns (Ant-style)
- `Method` list (e.g., `["GET","POST"]`)
- `Header`, `Query`, `Cookie` (exact or regex)
- `RemoteAddr` (CIDR blocks)
- `After`, `Before`, `Between` (ISO-8601 timestamps)
- `Weight` (group + weight for weighted selection)

### Route Example (two patterns, prefix strip)
```json
{
  "mode": "Reverse",
  "listen_addr": "127.0.0.1:8080",
  "reverse_proxy_routes": [
    {
      "id": "api",
      "target": "http://backend:8080",
      "strip_path_prefix": "/api",
      "predicates": [
        { "type": "Path", "patterns": ["/api/{segment}", "/api/**"], "match_trailing_slash": true }
      ]
    }
  ]
}
```

## Multi-Target Reverse Proxy Routing

Multi-target routing selects a target within a matched route using this order:
1) Header override (if configured and allowed)
2) Sticky selection (if configured and key present)
3) Load balancing policy

### Target Fields

```json
{
  "id": "api-a",
  "url": "http://10.0.0.10:8080",
  "weight": 3,
  "enabled": true
}
```

| Field | Type | Required | Description | When to use |
|-------|------|----------|-------------|-------------|
| `id` | String | Yes | Unique target id within the route | Required for sticky/header override lookups |
| `url` | String | Yes | Absolute upstream URL | Required for every target |
| `weight` | Number | No | Weight for `weighted_round_robin` (>= 1, default 1) | Use to bias traffic to larger instances |
| `enabled` | Boolean | No | Enable/disable the target (default true) | Use to drain an instance without deleting config |

### Load Balancing Policies

```json
{ "load_balancing": { "policy": "round_robin" } }
```

| Policy | Behavior | When to use |
|--------|----------|-------------|
| `round_robin` | Cycles across healthy targets | Default choice for similar backends |
| `weighted_round_robin` | Uses target weights for selection | Gradual rollout or uneven capacity |
| `least_connections` | Picks target with fewest in-flight requests | Spiky or uneven request cost |
| `random` | Random healthy target | Simple fallback or large pools |

### Sticky Sessions

```json
{
  "sticky": {
    "mode": "cookie",
    "cookie_name": "BIFROST_STICKY",
    "ttl_seconds": 3600
  }
}
```

| Field | Type | Required | Description | When to use |
|-------|------|----------|-------------|-------------|
| `mode` | String | Yes | `cookie`, `header`, or `source_ip` | Choose a stable client key |
| `cookie_name` | String | Conditional | Cookie name for `cookie` mode | Browser sessions that must stay on one node |
| `header_name` | String | Conditional | Header name for `header` mode | API clients that provide a stable header |
| `ttl_seconds` | Number | No | Cookie max-age (seconds) | Limit stickiness duration |

Sticky cookie mode stores the selected target id in a cookie. If the cookie is missing or invalid,
the proxy selects a target using the load-balancing policy and sets a new cookie.

### Header Override Routing

```json
{
  "header_override": {
    "header_name": "X-Bifrost-Target",
    "allowed_values": { "canary": "api-b" },
    "allowed_groups": { "eu": ["api-a", "api-b"] }
  }
}
```

| Field | Type | Required | Description | When to use |
|-------|------|----------|-------------|-------------|
| `header_name` | String | Yes | Header used for override matching | Canary, region, or debug routing |
| `allowed_values` | Object | Yes | Map of header value -> target id | Restrict overrides to trusted values |
| `allowed_groups` | Object | No | Map of header value -> target id list | Allow region or cohort routing |

Header override is evaluated before sticky or load balancing. If the header is present but unmapped
or points to an unhealthy target, normal selection applies.

### Retry Policy

```json
{
  "retry_policy": {
    "max_attempts": 2,
    "retry_on_connect_error": true,
    "retry_on_statuses": [502, 503],
    "methods": ["GET", "HEAD"]
  }
}
```

| Field | Type | Required | Description | When to use |
|-------|------|----------|-------------|-------------|
| `max_attempts` | Number | Yes | Total attempts including the first try | Keep small (2-3) |
| `retry_on_connect_error` | Boolean | No | Retry when the connection fails before response | Flaky networks or cold backends |
| `retry_on_statuses` | Array | No | Retry on specific upstream status codes | Gateway errors (502/503/504) |
| `methods` | Array | No | Allowed HTTP methods for retries | Limit to safe/idempotent methods |

Retries are only attempted when a retry policy is configured. Requests are buffered in memory for
replay; avoid large payloads or high max attempts unless you can tolerate the memory use.

Example configs in `examples/`:
- `examples/config_reverse_multi_targets_round_robin.json` for a basic round-robin pool
- `examples/config_reverse_multi_targets_weighted.json` for uneven capacity rollout
- `examples/config_reverse_multi_targets_least_connections.json` for uneven request cost
- `examples/config_reverse_multi_targets_random.json` for random selection across healthy targets
- `examples/config_reverse_multi_targets_sticky_header_override.json` for sticky + header override
- `examples/config_reverse_multi_targets_sticky_header.json` for sticky routing via request header
- `examples/config_reverse_multi_targets_sticky_source_ip.json` for sticky routing via source IP
- `examples/config_reverse_multi_targets_header_override_groups.json` for header override groups
- `examples/config_reverse_multi_targets_retry_policy.json` for retry policy across multiple targets

`reverse_proxy_target` remains supported for single-target setups, but `reverse_proxy_routes` is preferred for predicate-based routing.

| Field | Type | Description | Default |
|-------|------|-------------|---------|
| `level` | String | Global logging level (trace, debug, info, warn, error) | `info` |
| `format` | String | Log output format (text, json) | `text` |
| `targets` | Array | List of logging output targets | `[{type: "stdout"}]` |

### Log Target Fields

| Field | Type | Description | Required |
|-------|------|-------------|----------|
| `type` | String | Output type: `stdout` or `file` | ✅ Yes |
| `path` | String | File path (required when type is `file`) | ❌ No |
| `level` | String | Override level for this target | ❌ No |

**Note:** CLI arguments (`--log-level`, `--log-format`) are used as fallback when no logging configuration is provided in the JSON file.

## 📡 Monitoring Configuration

```json
{
  "monitoring": {
    "enabled": true,
    "listen_address": "127.0.0.1:9900",
    "metrics_endpoint": "/metrics",
    "health_endpoint": "/health",
    "status_endpoint": "/status",
    "include_detailed_metrics": true
  }
}
```

| Field | Type | Description | Default |
|-------|------|-------------|---------|
| `enabled` | Boolean | Toggle the monitoring server on/off | `true` |
| `listen_address` | String | Address for the monitoring HTTP server | `127.0.0.1:9900` |
| `metrics_endpoint` | String | Prometheus-compatible metrics endpoint | `"/metrics"` |
| `health_endpoint` | String | JSON health endpoint for load balancers | `"/health"` |
| `status_endpoint` | String | Human-friendly HTML dashboard | `"/status"` |
| `include_detailed_metrics` | Boolean | Include extended fields in future responses | `true` |

Once enabled, the monitoring server exposes all three endpoints on the configured `listen_address`. The `/metrics` endpoint is safe for Prometheus scrapes, `/health` is optimized for fast JSON responses, and `/status` renders the built-in dashboard.

## 🌐 WebSocket Configuration

```json
{
  "websocket": {
    "enabled": true,
    "allowed_origins": ["*"],
    "supported_protocols": ["chat", "notification"],
    "timeout_seconds": 300
  }
}
```

| Field | Type | Description | Default |
|-------|------|-------------|---------|
| `enabled` | Boolean | Toggle WebSocket proxying | `true` |
| `allowed_origins` | Array | Allowed `Origin` values (`"*"` permits all) | `["*"]` |
| `supported_protocols` | Array | Allowed `Sec-WebSocket-Protocol` values (empty = any) | `[]` |
| `timeout_seconds` | Number | Idle timeout for upgraded tunnels | `300` |

The forward proxy supports direct WebSocket upgrades (and WSS via the existing CONNECT tunnel). Reverse proxy upgrades are automatically bridged to the backend using the same configuration. Relay proxies do not yet support WebSocket upgrades.

## 🚦 Rate Limiting Configuration

```json
{
  "rate_limiting": {
    "enabled": true,
    "default_limit": { "limit": 200, "window_secs": 60 },
    "rules": [
      {
        "id": "login-posts",
        "limit": 20,
        "window_secs": 60,
        "path_prefix": "/api/login",
        "methods": ["POST"]
      }
    ]
  }
}
```

| Field | Type | Description | Default |
|-------|------|-------------|---------|
| `enabled` | Boolean | Toggle rate limiting | `true` |
| `default_limit.limit` | Number | Requests allowed per IP in the default window | Required when `default_limit` is present |
| `default_limit.window_secs` | Number | Window length (seconds) for the default tier | Required when `default_limit` is present |
| `rules` | Array | Additional rule definitions (per endpoint/tier) | `[]` |
| `rules[].id` | String | Unique rule identifier for logging/metrics | — |
| `rules[].limit` | Number | Requests allowed per window for matching requests | — |
| `rules[].window_secs` | Number | Window size (seconds) for the rule | — |
| `rules[].path_prefix` | String | Optional path prefix match (e.g., `/api/admin`) | Matches all paths when omitted |
| `rules[].methods` | Array | Optional HTTP method filter (e.g., `["POST"]`) | Matches all methods when omitted |

Rules are evaluated in the order defined. A request can match multiple rules: the default tier plus any endpoint-specific tiers. Every rule maintains a per-IP counter; exceeding any limit triggers an HTTP `429 Too Many Requests` response with a `Retry-After` header. Forward proxy CONNECT/WebSocket requests, reverse proxy traffic, and static file responses all share the same limiter.

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

### Example 5: Development with Logging

This example shows a development setup with custom logging configuration:

```json
{
  "mode": "Reverse",
  "listen_addr": "127.0.0.1:3000",
  "logging": {
    "level": "debug",
    "format": "text",
    "targets": [
      {
        "type": "stdout",
        "level": "info"
      }
    ]
  },
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

---

**Last Updated:** 2025-11-21
**See Also:** [Examples](../examples/), [CLI Reference](../README.md)
