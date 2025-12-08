# Reverse Proxy Requirements (Request Matching)

This document captures requirements for a reverse proxy whose routing model is inspired by Spring Cloud Gateway request predicate factories. It focuses on request matching and route selection; filter behavior and upstream resiliency are out of scope for now.

## Goals
- Route incoming HTTP(S) requests to upstream targets based on composable predicates.
- Support the core predicate types provided by Spring Cloud Gateway so existing mental models and configurations transfer easily.
- Allow configuration-driven behavior (declarative definitions) while enabling programmatic extension points for new predicates.
- Provide deterministic evaluation order and safe defaults to avoid surprising routing.

## Non-Goals
- Implementing full Spring Cloud Gateway filter chain semantics.
- Building a configuration server; configs are assumed to be local files or supplied by the host process.
- Transport protocols beyond HTTP/1.1 and HTTP/2.

## Terminology
- **Route**: A mapping containing an id, optional priority, one or more predicates, and an upstream target (cluster/service).
- **Predicate**: A boolean function over the incoming request (method, path, headers, etc.). All predicates on a route must pass (logical AND) for the route to match.
- **Predicate factory**: A parameterized predicate type (e.g., Path, Host) that yields an executable predicate when configured.

## Functional Requirements
- **Route definition**
  - Each route must have: `id`, `target` (upstream name or URL), optional `priority` (lower number = higher precedence), and an ordered list of predicates.
  - Route ids must be unique; duplicates are rejected at startup/config reload.
  - Routes with no predicates are invalid.

- **Predicate composition and evaluation**
  - All predicates on a route are combined with logical AND.
  - Routes are evaluated by ascending priority; ties break by declaration order. First matching route is selected.
  - If no route matches, respond with `404` (or configurable fallback).
  - Predicate evaluation must be side-effect free and must not consume the body unless explicitly required by a body-aware predicate.

- **Supported predicate factories (parity with Spring Cloud Gateway core)**
  - `After`: Match when `ZonedDateTime` now is after the configured instant.
  - `Before`: Match when now is before the configured instant.
  - `Between`: Match when now is between two instants (inclusive start, exclusive end).
  - `Cookie`: Match when a cookie with name exists and its value matches exact string or regex.
  - `Header`: Match when header exists and its value matches exact string or regex.
  - `Host`: Match when the Host header matches Ant-style patterns (e.g., `**.example.org`, `*.svc.internal`). Case-insensitive.
  - `Method`: Match when the HTTP method is in the configured set (e.g., GET, POST).
  - `Path`: Match when the path matches Ant-style patterns with template variables (e.g., `/foo/{segment}/**`). Support optional trailing slash matching.
  - `Query`: Match when query param exists and optionally matches regex/value.
  - `RemoteAddr`: Match when the remote IP is within configured CIDR ranges. Support both IPv4 and IPv6.
  - `Weight`: Participate in weighted routing. Multiple routes with same `group` compete; traffic is distributed according to configured weights. Must validate weights sum to >0 and normalize when needed.
  - `ReadBody` (optional but recommended): Match based on body predicate (e.g., JSON field). Requires buffering; must be opt-in with size limits and content-type guards.
  - `CloudFoundryRouteService` (optional): Treat `X-CF-Forwarded-Url` and related headers as match criteria; include only if CF support is required.

- **Configuration model**
  - Provide human-editable configuration (YAML/TOML/JSON). Example (YAML-like):
    ```yaml
    routes:
      - id: user-api
        priority: 10
        target: http://user-svc:8080
        predicates:
          - Path=/users/**, /profiles/**
          - Method=GET,POST
          - Host=api.example.com
      - id: canary-user-api
        priority: 5
        target: http://user-svc-canary:8080
        predicates:
          - Path=/users/**
          - Weight=group=user-api, weight=20
    ```
  - Support list or map syntax for predicates; validate syntax at load time with clear errors.
  - Allow hot-reload of configuration (signal or polling) with atomic swap and validation before activation.

- **Request attribute exposure**
  - Expose extracted path variables and query values to downstream filters/upstream request rewriting.
  - Preserve original Host and client IP (via `X-Forwarded-*` headers) unless configured otherwise.

- **Error handling and validation**
  - Startup/load fails fast on invalid predicate configuration (unknown factory, bad regex, invalid CIDR, missing parameters).
  - At runtime, predicate evaluation errors are treated as non-match and logged at warn level with route id.
  - Time-based predicates must validate ISO-8601 timestamps and time zones.

## Observability
- Emit structured logs for route selection including route id, predicate results (optional debug flag), and final upstream chosen.
- Provide metrics: total requests, matched/nomatch counts, per-route hits, and weight distribution effectiveness.
- Expose health/readiness endpoints to report config load status and last reload outcome.

## Performance and Limits
- Predicate evaluation must be non-blocking and avoid allocations where possible.
- Configure limits: max path pattern count per route, max predicate count, max body size for `ReadBody` predicates.
- Weighted routing must avoid global locks; use atomic counters or ring-based selection.

## Security
- Validate and sanitize regex inputs to prevent catastrophic backtracking where possible.
- CIDR parsing must reject invalid addresses; optionally enforce allowed ranges.
- For `ReadBody`, ensure body buffering respects global/proxy-level limits and does not expose bodies in logs by default.

## Extensibility
- Provide an interface/trait to register custom predicate factories with name, argument schema, and validation.
- Allow feature flags to enable optional predicates (`ReadBody`, `CloudFoundryRouteService`) to minimize baseline overhead.

## Open Questions
- Do we need per-route fallback order when multiple routes share the same priority beyond declaration order?
- Should configuration support expression language (e.g., SpEL-equivalent) or stay with factory syntax only?
- What is the default policy when weighted routes exhaust weight budget (e.g., rounding issues)?
