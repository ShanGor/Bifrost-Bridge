# Improvement plan — 2026-09-20

This is a tracked remediation plan created from the documentation and implementation audit.

## Scope and progress

- [x] Make advertised configuration limits and CLI options effective.
- [x] Correct combined static/reverse route dispatch and cover it with tests.
- [x] Make documented static-file JSON examples valid by supplying serde defaults.
- [x] Correct reverse-proxy timeout and forwarded-header behavior.
- [x] Repair the failing static-file unit tests and run the full suite.
- [x] Reconcile the user, architecture, requirements, and status documentation.

## Implementation work

1. **Configuration limits and pool sizing**
   - Wire `--pool-max-idle` into reverse-proxy configuration.
   - Enforce `max_connections` at each listener and apply `max_header_size` to HTTP/1 server builders.
   - Validate unusable values before startup.

2. **Routing and request semantics**
   - Dispatch each configured static mount independently in combined mode, including mount-prefix boundaries.
   - Allow a static 404 to fall through to the next configured route.
   - Select the route represented by a combined-mode reverse route table entry rather than ignoring its ID.
   - Preserve an inbound `X-Forwarded-For` chain and derive `X-Forwarded-Proto` from listener TLS state.
   - Apply the documented lifetime timeout to accepted reverse-proxy HTTP connections and upgraded tunnels; keep backend-pool cleanup governed by its idle timeout.

3. **Tests**
   - Use temporary directories in static-file mount tests.
   - Add focused tests for the repaired routing/configuration behavior where practical.

## Documentation work

1. Replace obsolete `proxy-server` commands and paths with `bifrost-bridge`.
2. Correct the configuration reference: valid modes, actual pooling shape, defaults, limits, and header behavior.
3. Replace incomplete static-file JSON snippets or make those fields safely defaultable.
4. Reconcile TLS, plugin, requirements, and future-feature status statements.
5. Remove broken links/references and repair invalid JSON examples.

## Verification record

- 2026-09-20: Initial audit complete. `cargo test --all-targets` failed: two static-file tests used a non-existent `test-temp` directory.
- 2026-09-20: Implemented listener connection/header limits, timeout propagation, CLI reverse-pool sizing, route-specific combined dispatch, static-config defaults, and forwarded-header corrections.
- 2026-09-20: Added regression coverage for documented static defaults, shipped example deserialization, listener-limit validation, route-ID selection, and forwarded headers.
- 2026-09-20: Reconciled TLS/plugin/error-recovery/requirements status, binary naming, broken references, and JSON examples.
- 2026-09-20: `cargo test` passed (81 tests total), `cargo check --all-targets` passed, concrete JSON fences in the changed guides parsed, and `git diff --check` passed.
- 2026-09-20: Strict `cargo clippy --all-targets -- -D warnings` is not a project baseline yet; it reports the existing style/refactor backlog (derivable defaults, long argument lists, collapsible conditionals, and similar non-functional lints).
