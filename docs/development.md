# Development Guide

This guide covers development setup, coding standards, and contribution guidelines.

## 📋 Table of Contents

- [Development Setup](#development-setup)
- [Project Structure](#project-structure)
- [Coding Standards](#coding-standards)
- [Testing](#testing)
- [Debugging](#debugging)
- [Building and Releasing](#building-and-releasing)
- [Documentation Updates](#documentation-updates)

## 🛠️ Development Setup

### Prerequisites
- Rust 1.70+
- Git
- Code editor (VS Code recommended)

### VS Code Setup
```bash
# Install recommended extensions
code --install-extension rust-lang.rust-analyzer
code --install-extension vadimcn.vscode-lldb
```

### Development Commands
```bash
# Clone repository
git clone <repository-url>
cd proxy-server

# Development build
cargo build

# Run tests
cargo test

# Run with debug logging
RUST_LOG=debug cargo run -- --config examples/config_spa.json

# Format code
cargo fmt

# Lint code
cargo clippy

# Check for unused dependencies
cargo machete
```

## 📁 Project Structure

```
proxy-server/
├── src/
│   ├── main.rs              # Application entry point
│   ├── config.rs            # Configuration structures
│   ├── error.rs             # Error handling
│   ├── static_files.rs      # Static file serving
│   ├── proxy.rs             # Proxy trait and implementations
│   ├── forward_proxy.rs     # Forward proxy implementation
│   └── reverse_proxy.rs     # Reverse proxy implementation
├── examples/                # Configuration examples
├── tests/                   # Integration tests
├── docs/                    # Documentation
├── requirements/            # Requirements tracking
├── Cargo.toml               # Project dependencies
└── README.md                # Project overview
```

### Code Organization Principles

1. **Single Responsibility**: Each module has a clear, single purpose
2. **Trait-based Architecture**: Use traits for extensibility
3. **Error Handling**: Comprehensive error types with `thiserror`
4. **Configuration**: Centralized configuration with serde
5. **Testing**: Unit tests alongside implementation

## 📝 Coding Standards

### Rust Conventions
- Follow official Rust style guide
- Use `cargo fmt` for formatting
- Use `cargo clippy` for linting
- Prefer `Result<T, E>` over panics
- Use meaningful variable and function names

### Code Style

```rust
// ✅ Good: Descriptive names, Result handling, documentation
use crate::error::ProxyError;

/// Handles static file requests for a specific mount point
pub async fn handle_request(&self, req: Request<Body>) -> Result<Response<Body>, ProxyError> {
    let path = req.uri().path();
    let mount_info = self.find_mount(path).ok_or(ProxyError::NotFound)?;

    self.serve_file(mount_info, path).await
}

// ❌ Bad: Vague names, unwrap(), no documentation
pub fn handle(&self, req: Request<Body>) -> Response<Body> {
    let path = req.uri().path();
    let mount = self.find_mount(path).unwrap();
    self.serve(mount, path).await.unwrap()
}
```

### Error Handling

```rust
// ✅ Good: Specific error types
#[derive(Debug, thiserror::Error)]
pub enum ProxyError {
    #[error("Configuration error: {0}")]
    Config(String),

    #[error("File not found: {0}")]
    NotFound(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

// ✅ Good: Use `?` operator for error propagation
pub fn read_config(path: &str) -> Result<Config, ProxyError> {
    let content = std::fs::read_to_string(path)?;
    let config: Config = serde_json::from_str(&content)?;
    Ok(config)
}
```

### Documentation

```rust
/// Represents a static file mount point
///
/// A mount maps a URL path prefix to a filesystem directory.
/// Mounts support inheritance from parent configuration.
///
/// # Examples
///
/// ```
/// let mount = StaticMount {
///     path: "/app".to_string(),
///     root_dir: "./frontend/dist".to_string(),
///     spa_mode: Some(true),
///     // inherits other settings from parent
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StaticMount {
    /// URL path prefix (e.g., "/app", "/api")
    pub path: String,

    /// Filesystem directory path
    pub root_dir: String,

    /// Enable SPA mode (inherits from parent if None)
    #[serde(default)]
    pub spa_mode: Option<bool>,
}
```

## 🧪 Testing

### Running Tests
```bash
# Run all tests
cargo test

# Run specific module tests
cargo test static_files::tests

# Run tests with output
cargo test -- --nocapture

# Run integration tests
cargo test --test integration

# Generate test coverage
cargo install cargo-tarpaulin
cargo tarpaulin --out Html
```

### Writing Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::StaticFileConfig;

    #[test]
    fn test_mount_inheritance() {
        // Arrange
        let config = StaticFileConfig {
            spa_mode: true,
            enable_directory_listing: false,
            mounts: vec![
                StaticMount {
                    path: "/app".to_string(),
                    root_dir: "./dist".to_string(),
                    spa_mode: None, // Should inherit true
                    enable_directory_listing: Some(true), // Override false
                    // ... other fields
                }
            ],
            // ... other fields
        };

        // Act
        let resolved = config.mounts[0].resolve_inheritance(&config);

        // Assert
        assert_eq!(resolved.spa_mode, true); // Inherited
        assert_eq!(resolved.enable_directory_listing, true); // Overridden
    }

    #[tokio::test]
    async fn test_file_serving() {
        // Async test example
        let handler = StaticFileHandler::new(config).unwrap();
        let request = Request::builder()
            .uri("/app/index.html")
            .body(Body::empty())
            .unwrap();

        let response = handler.handle_request(&request).await;
        assert!(response.is_ok());
    }
}
```

### Test Categories

1. **Unit Tests**: Test individual functions and methods
2. **Integration Tests**: Test complete workflows
3. **Property Tests**: Test with generated data (using proptest)
4. **Benchmarks**: Performance testing (using criterion)

## 🐛 Debugging

### Logging
```bash
# Enable debug logging
RUST_LOG=debug cargo run --config config.json

# Enable trace logging
RUST_LOG=trace cargo run --config config.json

# Log to file
RUST_LOG=info cargo run --config config.json 2>&1 | tee proxy-server.log
```

### Common Debugging Techniques

```rust
// 1. Debug prints
#[cfg(debug_assertions)]
eprintln!("DEBUG: Mount info: {:?}", mount_info);

// 2. Logging
use log::{debug, info, warn, error};

debug!("Processing request: {}", req.uri().path());
info!("Server started on {}", addr);
warn!("Configuration warning: {}", warning);
error!("Failed to serve file: {}", e);

// 3. Assert for debugging
debug_assert!(mount_info.path.starts_with('/'), "Mount paths should start with /");
```

### Debug Tools
```bash
# GDB debugging
rust-gdb target/debug/proxy-server

# LLDB debugging (macOS)
rust-lldb target/debug/proxy-server

# Memory profiling
valgrind --tool=massif target/release/proxy-server
```

## 🏗️ Building and Releasing

### Build Commands
```bash
# Development build
cargo build

# Release build
cargo build --release

# Cross-compilation
cargo build --release --target x86_64-unknown-linux-musl
```

### Release Checklist
- [ ] All tests pass: `cargo test`
- [ ] Code formatted: `cargo fmt`
- [ ] Code linted: `cargo clippy`
- [ ] Documentation updated
- [ ] Version bumped in Cargo.toml
- [ ] CHANGELOG.md updated
- [ ] Examples tested

### Release Process
```bash
# 1. Update version
cargo edit set-version 0.2.0

# 2. Update changelog
edit CHANGELOG.md

# 3. Tag release
git tag -a v0.2.0 -m "Release version 0.2.0"
git push origin v0.2.0

# 4. Build release binaries
cargo build --release

# 5. Create release package
tar -czf proxy-server-v0.2.0-linux-x64.tar.gz target/release/proxy-server

# 6. Test release
./proxy-server --version
```

## 📚 Documentation Updates

### When to Update Documentation

**Required Updates:**
- ✅ New features implemented
- ✅ Configuration changes
- ✅ API changes
- ✅ Breaking changes
- ✅ Security updates

**Recommended Updates:**
- 🔄 Bug fixes
- 🔄 Performance improvements
- 🔄 Code refactoring

### Update Process

1. **Update Requirements Log**
   ```bash
   # Add new requirement
   echo "RXXX: New feature description" >> requirements/README.md
   ```

2. **Update Documentation**
   ```bash
   # Update relevant docs
   edit docs/configuration.md
   edit docs/examples.md
   ```

3. **Update Examples**
   ```bash
   # Add or update example configurations
   edit examples/config_new_feature.json
   ```

4. **Update Changelog**
   ```bash
   edit CHANGELOG.md
   ```

### Documentation Standards

- Use clear, concise language
- Include working examples
- Update dates and versions
- Cross-reference related documentation
- Use consistent formatting

## 🔧 Development Workflow

### 1. Feature Development
```bash
# Create feature branch
git checkout -b feature/multiple-mounts

# Implement feature
# Write tests
# Update documentation

# Run quality checks
cargo fmt
cargo clippy
cargo test

# Commit changes
git add .
git commit -m "feat: Add multiple static mount support"
```

### 2. Code Review Checklist
- [ ] Code follows style guidelines
- [ ] Tests cover new functionality
- [ ] Documentation is updated
- [ ] No breaking changes without proper version bump
- [ ] Error handling is comprehensive
- [ ] Performance impact considered

### 3. Git Commit Convention
```
feat: Add new feature
fix: Bug fix
docs: Documentation update
style: Code formatting
refactor: Code refactoring
test: Test addition/modification
chore: Maintenance task
```

---

**Last Updated:** 2025-11-15
**See Also:** [Contributing Guide](./contributing.md), [Testing Guide](./testing.md)