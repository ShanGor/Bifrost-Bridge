# Test and Performance Suites

## Test layers

- **Unit tests** cover isolated modules and types in `src/`.
- **Rust integration tests** in `tests/*.rs` exercise proxy behavior against local fixtures, including streaming, caching, worker isolation, and recovery.
- **Process-level regression tests** in `tests/e2e_regression.rs` launch the built executable with JSON config, then verify forward-proxy, static, SPA, and reverse-proxy traffic over real local sockets.
- **Criterion microbenchmarks** measure worker and metrics operations. The HTTP pipeline benchmark measures static-file and reverse-proxy request round trips over loopback.

The Python helper in `tests/reverse/multi_target_tests.py` is a local backend fixture for manual multi-target experiments; it is not itself an automated test suite.

## Run tests

```bash
# Unit tests in the library
cargo test-unit

# Process-level executable regression tests
cargo test-e2e

# All unit, integration, executable regression, and doc tests
cargo test --locked
```

The end-to-end test uses temporary config, content, and PID files, and binds to an available local port. It does not need public network access or a separately installed server.

## Run performance benchmarks

```bash
# HTTP request pipeline: static 1 KiB, static 64 KiB, reverse proxy 1 KiB
cargo bench-http

# Save and compare local Criterion baselines
cargo bench-http -- --save-baseline before
# Make a change, then run:
cargo bench-http -- --baseline before

# Existing worker / metrics microbenchmarks
cargo bench --bench simple_benchmarks
cargo bench --bench worker_separation_benchmarks
```

Criterion writes comparison data and HTML reports under `target/criterion`. Compare runs on the same machine with the same build profile and an idle system; the HTTP benchmark uses local loopback fixtures and does not claim production capacity. GitHub Actions runs it weekly and on manual dispatch, and retains the Criterion report as a workflow artifact. Benchmark numbers are observational and are not used as pass/fail thresholds because shared CI hardware is noisy.
