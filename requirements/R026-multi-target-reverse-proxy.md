# R026: Multi-Target Reverse Proxy Routing

**Status**: Implemented
**Created**: 2025-12-24
**Updated**: 2025-02-25
**Priority**: High

## Overview

Extend reverse proxy routing to support multiple upstream targets per route with configurable
load-balancing, sticky sessions, and header-driven target overrides. This builds on R025 predicate
matching: routes are selected first, then a target is chosen within the matched route.

## Goals

- Allow multiple upstream targets per route with explicit IDs and optional weights.
- Provide round-robin and sticky session modes as first-class strategies.
- Support header-based target override ("special header routing") for canary/region/debug use cases.
- Preserve backward compatibility with single-target `target` configurations.
- Emit clear logs and metrics for target selection decisions.

## Non-Goals

- Dynamic service discovery (e.g., DNS SRV, Consul, Kubernetes endpoints).
- Cross-route failover or global traffic shaping.
- Full gateway filter chain semantics (out of scope per R025).

## Functional Requirements

### Configuration

- A route may define `targets` as a list of upstream entries:
  - `id` (string, unique within the route)
  - `url` (string, absolute URL)
  - `weight` (optional, integer >= 1; default 1)
  - `enabled` (optional, default true)
- Backward compatibility:
  - If `targets` is absent, `target` remains valid and is treated as a single-entry list.
  - If both are provided, configuration validation fails unless explicitly allowed by a flag.

### Target Selection Order

1. Select a route using existing predicate logic (R025).
2. Evaluate header override routing if configured and allowed.
3. Apply sticky session selection if configured and a key is present.
4. Otherwise, apply the configured load-balancing policy.
5. If no healthy targets remain, return 503 (Service Unavailable) with a clear error body.

### Load-Balancing Policies

- **Round-robin**: Simple modulo counter across healthy targets (default).
- **Weighted round-robin**: Weighted modulo selection across healthy targets.
- **Least connections**: Choose the target with the fewest in-flight requests.
- **Random**: Uniform random across healthy targets.

### Sticky Session Modes

- **Cookie**: Proxy sets a cookie (configurable name) with the selected target id. Support TTL.
- **Header**: Use a configured header value (e.g., `X-User-Id`) as the hash key.
- **Source IP**: Hash on client IP (best-effort; warning for NATed clients).
- Sticky selection must fall back to normal load-balancing when the mapped target is unhealthy.

### Header Override Routing (Special Header)

- Optional routing override using a configured header (e.g., `X-Bifrost-Target`).
- Header values map to target IDs or target groups (allowlist only).
- Disabled by default. Recommended to enable only for trusted clients or internal networks.
- If the header is present but unmapped or unhealthy, fall back to standard selection.

### Health and Resilience

- Per-target health checks reuse existing reverse proxy health check config, scoped to each target.
- Passive failure tracking (timeouts, 5xx threshold) is deferred.
- Optional slow-start for recovered targets is deferred.

### Observability

- Log fields: route_id, target_id (implemented via existing logs).
- Metrics: per-target request counts, errors, latency, and in-flight gauges are deferred.

## Design (Proposed)

### Configuration Model (JSON)

```json
{
  "reverse_proxy_routes": [
    {
      "id": "api",
      "predicates": [{ "type": "Path", "patterns": ["/api/**"], "match_trailing_slash": true }],
      "targets": [
        { "id": "api-a", "url": "http://10.0.0.10:8080", "weight": 3 },
        { "id": "api-b", "url": "http://10.0.0.11:8080", "weight": 1 }
      ],
      "load_balancing": {
        "policy": "weighted_round_robin"
      },
      "sticky": {
        "mode": "cookie",
        "cookie_name": "BIFROST_STICKY",
        "ttl_seconds": 3600
      },
      "header_override": {
        "header_name": "X-Bifrost-Target",
        "allowed_values": {
          "canary": "api-b"
        }
      }
    }
  ]
}
```

### Routing Strategy Best Practices

- **Consistent hashing** for sticky keys to minimize churn when targets change.
- **Smooth weighted round-robin** to avoid clumping on heavier targets.
- **Least connections** for uneven request cost profiles.
- **Jittered health checks** to avoid synchronized retries (thundering herd).
- **Outlier detection** (passive) to eject flaky targets quickly.
- **Fail-open selection**: if sticky or override fails, use standard policy to avoid outages.
- **Header override safety**: restrict to trusted networks and allowlisted values.

### Internal Structures (Draft)

```rust
struct TargetConfig {
    id: String,
    url: Url,
    weight: u32,
    enabled: bool,
}

struct LoadBalancingConfig {
    policy: LoadBalancingPolicy,
}

enum LoadBalancingPolicy {
    RoundRobin,
    WeightedRoundRobin,
    LeastConnections,
    EwmaLatency,
    Random,
}

struct StickyConfig {
    mode: StickyMode,
    cookie_name: Option<String>,
    header_name: Option<String>,
    ttl_seconds: Option<u64>,
}

struct HeaderOverrideConfig {
    header_name: String,
    allowed_values: HashMap<String, String>, // value -> target_id
}
```

### Selection Algorithm (Draft)

1. Build list of healthy targets for the matched route.
2. If header override is enabled and header value maps to a healthy target, select it.
3. If sticky is enabled and a key is present, hash to a target (consistent hash ring).
4. Otherwise, select by policy (round-robin / weighted / least connections / EWMA / random).
5. If no healthy targets, return 503 with a structured error message.

## Validation Rules

- At least one target is required after compatibility expansion.
- Target IDs must be unique per route.
- Weights must be >= 1 when using weighted policies.
- Header override mappings must reference known target IDs.
- Sticky cookie/header modes must provide a key source.

## Open Questions

- Should header override allow group-level selection (e.g., region) or only target IDs?
- Should sticky selection be "strict" (fail if mapped target unhealthy) or "best effort"?
- Do we need per-route retry policies when a target is ejected during request processing?

## Implementation Tasks (Design-Only)

### Completed

- [x] Extend config structs to accept `targets`, `load_balancing`, `sticky`, `header_override`.
- [x] Implement per-target health checks (active) and selection algorithms.
- [x] Update docs and examples for multi-target routes.

### Deferred

- [ ] Passive ejection/outlier detection and slow-start.
- [ ] Per-target metrics and additional structured logs.
- [ ] Tests for selection fairness, sticky behavior, and header overrides.

## Implementation Notes

- Sticky cookie mode stores the selected target id; invalid or missing cookies trigger
  standard selection and a new cookie.
- Header override uses exact match against an allowlist of header values.
- Weighted round-robin uses a simple weighted modulo selection; smooth WRR is deferred.

**Back to:** [Requirements Index](../requirements/README.md)
