# Glossary

Shared definitions for terms used across the docs. When you introduce new jargon, add it here and
link to this file instead of repeating the definition in multiple guides.

## Core Terms

| Term | Meaning |
|------|---------|
| Adapter | The runtime-facing wrapper that binds a listener and delegates to a proxy implementation. |
| Backend / Upstream | The service a reverse proxy forwards requests to. |
| Combined Mode | Reverse proxy + static file handling on the same listener. |
| Forward Proxy | A proxy used by clients to reach arbitrary destinations (supports CONNECT). |
| Reverse Proxy | A proxy that fronts known backends and routes to them by configuration. |
| Route | A rule that matches requests (via predicates) and selects a target. |
| Predicate | A match condition (path, host, method, header, etc.) for a route. |
| Target | A single upstream endpoint (URL + metadata) inside a route. |
| Target Group | A named subset of targets used for header override routing. |

## Routing and Load Balancing

| Term | Meaning |
|------|---------|
| Header Override | A trusted header that forces routing to a specific target or target group. |
| Sticky Session | Keeps a client on the same target using a cookie, header, or source IP. |
| Load Balancing | Selecting a target among healthy ones using a policy. |
| Round Robin | Cycles across targets in a fixed order. |
| Weighted Round Robin | Chooses targets proportional to their weight values. |
| Least Connections | Chooses the target with the fewest in-flight requests. |
| Health Check | Periodic probe that marks a target healthy or unhealthy. |
| Ant-style Pattern | Path matching where `*` matches a segment and `**` matches the rest of the path. |
| CIDR | IP range notation like `10.0.0.0/8` used for network matching. |

## Networking and HTTP

| Term | Meaning |
|------|---------|
| CONNECT | HTTP method used to establish a tunnel (typically for HTTPS in forward proxy mode). |
| Connection Pooling | Reusing outbound connections to reduce latency and overhead. |
| Relay Proxy | An upstream proxy that the forward proxy chains requests through. |
| TLS Termination | The proxy handles TLS and forwards plain HTTP to backends. |
| X-Forwarded-* | Standard headers that record original client/host/protocol data. |

## Static File Serving

| Term | Meaning |
|------|---------|
| SPA Mode | Single Page Application mode; falls back to a default file (e.g., `index.html`). |
| Strip Path Prefix | Removes a leading path segment before forwarding to the backend. |
| MIME Type | The media type sent in the response (e.g., `text/html`). |

## Operations

| Term | Meaning |
|------|---------|
| Rate Limiting | Throttling requests based on configurable limits. |
| Worker Threads | Tokio runtime threads used to run async tasks. |
| Prometheus Endpoint | Metrics endpoint scraped by Prometheus (`/metrics`). |
