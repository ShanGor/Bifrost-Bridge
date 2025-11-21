# Quick Start Guide

Get up and running with the proxy server in minutes!

## 🚀 Prerequisites

- **Rust** (1.70 or later)
- **Cargo** (comes with Rust)

### Install Rust
```bash
# On Unix/macOS
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# On Windows
# Download and run rustup-init.exe from https://rustup.rs/
```

## 🏃‍♂️ Quick Start

### 1. Clone and Build

```bash
git clone <repository-url>
cd proxy-server
cargo build --release
```

### 2. Basic Usage

#### Simple Static File Server
```bash
# Serve files from ./public directory
cargo run -- --mode reverse --listen 127.0.0.1:8080 --static-dir ./public
```

#### SPA (Single Page Application) Server
```bash
# Serve SPA with fallback to index.html
cargo run -- --mode reverse --listen 127.0.0.1:8080 --static-dir ./dist --spa
```

#### Multiple Mount Points
```bash
# Serve multiple directories at different paths
cargo run -- \
  --mount /app:./frontend/dist \
  --mount /api:./api-docs \
  --mount /assets:./static
```

#### Using Configuration File
```bash
# Use JSON configuration
cargo run -- --config examples/config_spa.json
```

### 3. Test Your Server

```bash
# Test the server
curl http://127.0.0.1:8080/
curl http://127.0.0.1:8080/app/dashboard  # if using /app mount
```

## 📁 Example Configurations

### SPA Configuration (`spa.json`)
```json
{
  "mode": "Reverse",
  "listen_addr": "127.0.0.1:8080",
  "static_files": {
    "spa_mode": true,
    "mounts": [
      {
        "path": "/",
        "root_dir": "./dist"
      }
    ]
  }
}
```

### Multi-Mount Configuration (`multi-mount.json`)
```json
{
  "mode": "Reverse",
  "listen_addr": "127.0.0.1:8080",
  "static_files": {
    "mounts": [
      {
        "path": "/app",
        "root_dir": "./frontend/dist",
        "spa_mode": true
      },
      {
        "path": "/api",
        "root_dir": "./api-docs",
        "enable_directory_listing": true
      },
      {
        "path": "/assets",
        "root_dir": "./static"
      }
    ]
  }
}
```

## 🎯 Common Use Cases

### Frontend Development Server
```bash
# Serve React/Vue/Angular app
cargo run -- --mode reverse --listen 127.0.0.1:3000 --static-dir ./dist --spa
```

### API Documentation Server
```bash
# Serve API docs with directory listing
cargo run -- --mode reverse --listen 127.0.0.1:8080 --static-dir ./api-docs
```

### Microservices Static Assets
```bash
# Multiple frontend applications
cargo run -- \
  --mount /user-app:./services/user-app/dist \
  --mount /admin-app:./services/admin-app/dist \
  --mount /shared:./shared/assets
```

## 🔧 Configuration Options

### Command Line Arguments
| Argument | Description | Example |
|----------|-------------|---------|
| `--mode` | Proxy mode | `reverse` |
| `--listen` | Listen address | `127.0.0.1:8080` |
| `--static-dir` | Single directory | `./public` |
| `--spa` | Enable SPA mode | |
| `--mount` | Mount directory | `/app:./dist` |
| `--config` | Config file | `config.json` |

### JSON Configuration
See [Configuration Guide](./configuration.md) for detailed options.

## 🛠️ Development Mode

### Hot Reload Configuration
```json
{
  "mode": "Reverse",
  "listen_addr": "127.0.0.1:3000",
  "static_files": {
    "enable_directory_listing": true,
    "spa_mode": true,
    "mounts": [
      {
        "path": "/",
        "root_dir": "./src"
      },
      {
        "path": "/dist",
        "root_dir": "./dist"
      }
    ]
  }
}
```

### Multiple Environments
```bash
# Development
cargo run -- --config examples/config_development.json

# Production
cargo run -- --config examples/config_production.json

# Testing
cargo run -- --config examples/config_test.json
```

## 🔍 Troubleshooting

### Common Issues

1. **Port already in use**
   ```bash
   # Try a different port
   cargo run -- --listen 127.0.0.1:3000
   ```

2. **Directory not found**
   ```bash
   # Check directory exists
   ls -la ./dist
   # Use absolute path if needed
   cargo run -- --static-dir /full/path/to/dist
   ```

3. **Permission denied**
   ```bash
   # Check directory permissions
   ls -ld ./dist
   # Ensure readable by current user
   ```

### Debug Mode
```bash
# Enable debug logging
RUST_LOG=debug cargo run -- --config config.json
```

### Health Check
```bash
# Test server is responding
curl -I http://127.0.0.1:8080/
```

## 📚 Next Steps

- Read the [Configuration Guide](./configuration.md) for advanced options
- Check [Configuration Examples](../examples/) for more scenarios
- Review [Error Recovery Architecture](./error-recovery-architecture.md) for understanding internals
- See [Development Guide](./development.md) for contributing

---

**Need help?** Check the [troubleshooting section](#troubleshooting) or open an issue.

**Last Updated:** 2025-11-15