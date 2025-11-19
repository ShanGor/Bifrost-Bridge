# R016: Performance Monitoring

**Status:** ✅ Completed
**Date Raised:** TBD
**Category:** Monitoring

## 📋 Description

Add metrics and performance monitoring capabilities to the proxy server for observability and operational insights.

## 🎯 Planned Features

### Metrics Collection
- Request rate and response times
- Connection pool statistics
- Memory and CPU usage
- Error rates and types
- Static file serving performance

### Endpoints
- `/metrics` - Prometheus-compatible metrics endpoint
- `/health` - Health check endpoint
- `/status` - Detailed server status

### Integration
- Prometheus metrics format
- Structured logging integration
- Real-time monitoring dashboards

## ✅ Implementation Summary

- Integrated the official `prometheus` crate and exposed counters/gauges for requests, bytes, connection errors, streaming files, and request latency histograms per proxy type.
- Forward, reverse, and static file paths now instrument every connection and request via the shared `PerformanceMetrics` helpers; zero-copy static responses report file bytes and stream counts.
- Added a dedicated monitoring HTTP server (`monitoring.listen_address`) that serves:
  - `/metrics` — Prometheus text exposition for all counters.
  - `/health` — Lightweight JSON health summary derived from the live metrics.
  - `/status` — The existing HTML dashboard rendered with aggregated statistics.
- Monitoring is fully configurable through the new `monitoring` block in `Config`, including endpoint paths and the bind address.

## 🔧 Configuration

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

**Back to:** [Requirements Index](../requirements/README.md)
