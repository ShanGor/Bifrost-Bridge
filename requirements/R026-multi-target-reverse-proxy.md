# R026: Multi-Target Reverse Proxy Routing

**Status**: Partially implemented
**Created**: 2025-12-24
**Updated**: 2026-01-18
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

## Resolved Decisions

- Header override supports target IDs and group-level selection.
- Sticky selection is best-effort: fall back to normal selection if the mapped target is unhealthy.
- Per-route retry policies are required when a target is ejected during request processing.

## Functional Requirements

### Configuration

- A route may define `targets` as a list of upstream entries:
  - `id` (string, unique within the route)
  - `url` (string, absolute URL)
  - `weight` (optional, integer >= 1; default 1)
  - `enabled` (optional, default true)
- Backward compatibility:
  - If `targets` is absent, `target` remains valid and is treated as a single-entry list.
  - If both are provided, configuration validation fails.

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
- Allowlist values map to target IDs and target groups.
- Group-level overrides select a target within the group using the route's policy.
- Disabled by default. Recommended to enable only for trusted clients or internal networks.
- If the header is present but unmapped or unhealthy, fall back to standard selection.

### Retry Policy

- Optional per-route retry policy (`retry_policy`).
- Retries apply on connection errors and configured upstream status codes.
- Retries are limited to an allowlist of HTTP methods (safe methods by default).
- Retries are best-effort and should avoid reusing the same target within a request.

### Health and Resilience

- Per-target health checks reuse existing reverse proxy health check config, scoped to each target.
- Per-route retry policy applies when a target is ejected during request processing.
- Passive failure tracking (timeouts, 5xx threshold) is deferred.
- Optional slow-start for recovered targets is deferred.

### Observability

- Log fields: route_id, target_id for selection decisions.
- Metrics: per-target request counts, errors, latency, and in-flight gauges.

### Testing Requirements (TDD-first)

- For any remaining work, tests must be written before implementation changes.
- Minimum coverage (non-exhaustive):
  - Header override group mapping and selection within a group.
  - Sticky best-effort fallback when a mapped target is unhealthy.
  - Retry behavior when a target is ejected mid-request.
  - Selection policy behavior (round-robin, weighted, least connections, random).
  - Config validation (duplicate target IDs, weight >= 1, invalid header names).

## Validation Rules

- At least one target is required after compatibility expansion.
- Target IDs must be unique per route.
- Weights must be >= 1 when using weighted policies.
- Header override allowlist values must reference known target IDs.
- When group overrides are added, groups must reference known target IDs.
- Sticky cookie/header modes must provide a key source (cookie_name/header_name).
- Header names must be valid HTTP header names.
- Retry policy max_attempts must be >= 1.
- Retry policy methods must be valid HTTP methods.
- Retry policy status codes must be valid HTTP status codes.

## Current Implementation Status (code audit 2026-01-18)

### Implemented

- Multi-target routes with `targets`, id/weight/enabled, and `target` fallback.
- Validation for duplicate target IDs, weight >= 1, and target vs targets conflicts.
- Load balancing policies: round-robin, weighted round-robin, least connections, random.
- Sticky selection: cookie (target id), header (hash), source IP (hash), best-effort fallback.
- Header override via `header_override.allowed_values` (value -> target id) and `allowed_groups`.
- Retry policy with max attempts, method allowlist, status-based retry, connect-error retry, and
  per-attempt target exclusion for non-WebSocket requests.
- Active per-target health checks and exclusion of unhealthy targets.
- 503 error when no healthy targets remain.
- Tests for header override group selection, selection exclusions, and retry policy validation.

### Missing or Divergent from Requirements

- Per-target metrics and explicit target_id logging for selection decisions.
- Passive ejection/outlier detection and slow-start.
- Tests for selection policies, sticky fallback behavior, and retry behavior (status/connect retries).

### Testing Progress

- Completed: header override group selection, selection exclusions for retries, retry policy validation.
- Pending: selection policy coverage, sticky fallback behavior, retry behavior on status/connect errors.

## Configuration Examples

### Current Schema (implemented)

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
      "load_balancing": { "policy": "weighted_round_robin" },
      "sticky": { "mode": "cookie", "cookie_name": "BIFROST_STICKY", "ttl_seconds": 3600 },
      "header_override": {
        "header_name": "X-Bifrost-Target",
        "allowed_values": { "canary": "api-b" },
        "allowed_groups": { "eu": ["api-a", "api-b"] }
      },
      "retry_policy": {
        "max_attempts": 2,
        "retry_on_connect_error": true,
        "retry_on_statuses": [502, 503],
        "methods": ["GET", "HEAD"]
      }
    }
  ]
}
```

## Selection Algorithm (current)

1. Build the list of enabled and healthy targets for the matched route.
2. If a header override is configured and the header value maps to a healthy target id
   or group, select within that target or group.
3. If sticky is enabled and a key is present, select by cookie (target id) or hash (header/source IP).
4. Otherwise, select by the configured policy.
5. If no healthy targets remain, return 503.

## Operational Guidance (non-normative)

- **Consistent hashing** for sticky keys to minimize churn when targets change.
- **Smooth weighted round-robin** to avoid clumping on heavier targets.
- **Least connections** for uneven request cost profiles.
- **Jittered health checks** to avoid synchronized retries (thundering herd).
- **Outlier detection** (passive) to eject flaky targets quickly.
- **Fail-open selection**: if sticky or override fails, use standard policy to avoid outages.
- **Header override safety**: restrict to trusted networks and allowlisted values.

**Back to:** [Requirements Index](../requirements/README.md)
