# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- **Zero-downtime configuration reloads**
  - Added a supervisor-owned listener and replaceable worker generations
  - Added `--reload`/`--pid-file` using Unix `SIGHUP`
  - Reloads validate config, encrypted secrets, routes, and renewed TLS material before activation
  - Existing connections drain while new connections use the new configuration
- Comprehensive documentation system in `docs/` folder
- Requirements tracking system in `requirements/` folder
- Development guidelines and contribution standards
- **Reverse Proxy TLS Termination**
  - Reverse proxy mode now honors `private_key`/`certificate` config and terminates TLS on the listener (HTTPS/WSS capable)
  - Reverse proxy can now forward to `https://` backends (native-tls connector with system root certificates)
  - Route targets may use `ws://`/`wss://` schemes, normalized to `http://`/`https://` internally; `wss://` implies TLS to the backend
  - New example configs: `config_reverse_websocket.json`, `config_reverse_wss_tls_termination.json`, `config_reverse_wss.json`

### Changed
- Updated example configurations to use inheritance
- Improved code organization and documentation
- Health check default port now respects the target URL scheme (443 for HTTPS/WSS)

### Fixed
- **Client IP Detection Fix** (R014)
  - Fixed hardcoded "127.0.0.1" client IP in reverse proxy to extract actual client IP from connection
  - Now properly sets X-Forwarded-For header with real client IP address
  - Critical for access logging, rate limiting, and security auditing
  - Added comprehensive unit tests for client IP extraction
  - Updated documentation in `docs/configuration.md`
- **WebSocket Reverse Proxy Tunnel Failure**
  - Fixed broken WebSocket tunneling: hyper's `serve_connection` requires `.with_upgrades()` or every `hyper::upgrade::on()` fails with "upgrade expected but low level API in use", so the 101 handshake completed but no data flowed
  - Applied to reverse proxy, combined (reverse + static) proxy, and forward proxy serving paths
  - Fixed backend TLS connect failure (`invalid URL, scheme is not http`) by disabling `enforce_http` on the inner connector
- Fixed duplicate `order` field in `tests/spa_cache_tests.rs` that prevented the test suite from compiling

## [0.1.0] - 2025-11-15

### Added
- **Multiple Static Roots Support** (R001)
  - Support for multiple static file directories with mount points
  - Path-based routing for different mount points
  - CLI support with `--mount` argument
  - JSON configuration support

- **Configuration Inheritance** (R002)
  - Optional mount fields inherit from parent static_files config
  - Clean, DRY configuration files
  - Backward compatibility maintained

- **SPA Fallback Improvements** (R003)
  - Fixed JavaScript module MIME type issues
  - Added `is_asset_file()` check to prevent incorrect SPA fallback
  - Improved asset file detection

- **Graceful Shutdown** (R004)
  - Added tokio signal handling
  - Improved Ctrl+C shutdown behavior
  - Better shutdown messages

- **Enhanced CLI Interface**
  - Modern clap derive syntax
  - Support for multiple mount arguments
  - Better help text and error messages

- **Comprehensive Error Handling**
  - Detailed error types with thiserror
  - Better error messages and context
  - Improved debugging capabilities

### Changed
- **Configuration Structure**
  - Restructured `StaticFileConfig` to support multiple mounts
  - Added `StaticMount` struct for individual mount definitions
  - Implemented inheritance resolution with `ResolvedStaticMount`

- **Static File Handler**
  - Complete refactor to support multi-mount architecture
  - Added mount resolution and path matching
  - Improved performance with pre-computed mount information

- **Code Quality**
  - Removed unused imports and dead code
  - Fixed all compilation warnings
  - Improved test coverage and test structure

### Fixed
- **Compilation Errors**
  - Fixed hyper header import issues
  - Resolved trait object lifetime problems
  - Fixed closure ownership issues

- **JavaScript Module Loading** (R003)
  - Fixed Chrome DevTools errors about JS modules receiving text/html
  - Proper MIME type handling for static assets
  - Correct SPA fallback logic

- **Test Infrastructure**
  - Fixed test configurations for new mount system
  - Updated tests to work with inheritance
  - Improved test reliability

### Documentation
- **README.md Updates**
  - Added multiple static roots documentation
  - Updated feature list and usage examples
  - Added architecture section

- **Configuration Examples**
  - `config_multi_mount.json` - Multi-mount setup
  - `config_microservices.json` - Microservices architecture
  - `config_development.json` - Development environment
  - `config_production.json` - Production-ready setup

- **Comprehensive Documentation**
  - Installation guide
  - Configuration reference
  - Quick start guide
  - Development guidelines

### Technical Details
- **Dependencies**
  - Updated clap to v4.x with derive macros
  - Added tokio for async runtime and signal handling
  - Maintained compatibility with existing dependencies

- **Architecture**
  - Trait-based proxy system for extensibility
  - Modular configuration system
  - Efficient mount point lookup with sorting

- **Performance**
  - Optimized mount point resolution
  - Reduced memory allocation in hot paths
  - Improved error handling overhead

## [Previous Versions]

### Initial Development
- Basic forward and reverse proxy functionality
- Simple static file serving
- JSON configuration support
- Command-line interface

---

**Versioning:** This project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

**Release Process:** See [Development Guide](./docs/development.md) for release procedures.

**Migration Guide:** See [Configuration Guide](./docs/configuration.md) for migration instructions between versions.

**Requirements Tracking:** See [Requirements](./requirements/README.md) for detailed feature tracking.
