# Bifrost Bridge Documentation

This folder contains documentation for the Bifrost Bridge proxy server.

## 📚 Documentation Structure

### **User Documentation**
- [`installation.md`](./installation.md) - How to install and set up the proxy server
- [`quick-start.md`](./quick-start.md) - Get started quickly with basic usage
- [`configuration.md`](./configuration.md) - Detailed configuration options (CLI and JSON)
- [`https-setup.md`](./https-setup.md) - HTTPS/SSL configuration guide
- [`glossary.md`](./glossary.md) - Shared definitions for proxy terms and jargon

### **Technical Documentation**
- [`architecture-overview.md`](./architecture-overview.md) - Current runtime/adapters architecture with diagrams
- [`error-recovery-architecture.md`](./error-recovery-architecture.md) - Error handling and recovery mechanisms
- [`maintenance.md`](./maintenance.md) - System maintenance and operational guidance

### **Development Documentation**
- [`development.md`](./development.md) - Development setup and guidelines

## 🚀 Quick Start

```bash
# Basic static file serving
cargo run -- --mode reverse --listen 127.0.0.1:8080 --static-dir ./public --spa

# With configuration file
cargo run -- --config examples/config_spa.json

# Multiple mount points
cargo run -- --mount /app:./frontend/dist --mount /api:./api-docs
```

## 🔗 Related Resources

- [Requirements Log](../requirements/README.md) - Track all feature requests and requirements
- [Configuration Examples](../examples/) - Configuration file examples
- [Source Code](../src/) - Main application source code
- [Performance Tests](../tests-and-performance.md) - Benchmark results and testing procedures

---

**Last Updated:** 2026-01-18
**Version:** 1.0.0
