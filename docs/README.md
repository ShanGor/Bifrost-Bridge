# Proxy Server Documentation

This folder contains comprehensive documentation for the proxy server application.

## 📚 Documentation Structure

### **User Documentation**
- [`installation.md`](./installation.md) - How to install and set up the proxy server
- [`quick-start.md`](./quick-start.md) - Get started quickly with basic usage
- [`configuration.md`](./configuration.md) - Detailed configuration options
- [`examples.md`](./examples.md) - Usage examples and common scenarios
- [`cli-reference.md`](./cli-reference.md) - Command-line interface reference

### **Technical Documentation**
- [`architecture.md`](./architecture.md) - System architecture and design
- [`api.md`](./api.md) - HTTP API and interfaces
- [`performance.md`](./performance.md) - Performance considerations and tuning
- [`security.md`](./security.md) - Security features and best practices

### **Development Documentation**
- [`development.md`](./development.md) - Development setup and guidelines
- [`contributing.md`](./contributing.md) - How to contribute to the project
- [`testing.md`](./testing.md) - Testing strategy and guidelines
- [`changelog.md`](./changelog.md`) - Version history and changes

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
- [Examples](../examples/) - Configuration file examples
- [Source Code](../src/) - Main application source code

---

**Last Updated:** 2025-11-15
**Version:** 0.1.0