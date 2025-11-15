# Installation Guide

This guide covers different ways to install and set up the proxy server.

## 📋 Table of Contents

- [System Requirements](#system-requirements)
- [Installation Methods](#installation-methods)
- [Building from Source](#building-from-source)
- [Configuration Setup](#configuration-setup)
- [Verification](#verification)
- [Troubleshooting](#troubleshooting)

## 🔧 System Requirements

### Minimum Requirements
- **Rust**: 1.70 or later
- **Operating System**: Windows 10+, macOS 10.14+, Linux (Ubuntu 18.04+)
- **Memory**: 512MB RAM minimum
- **Disk**: 50MB free space

### Recommended Requirements
- **Rust**: Latest stable version
- **Operating System**: Latest stable version
- **Memory**: 2GB RAM or more
- **CPU**: 2+ cores for production use

## 📦 Installation Methods

### Method 1: Install Rust and Build from Source (Recommended)

#### 1. Install Rust

**Windows:**
```powershell
# Download and run rustup-init.exe from https://rustup.rs/
# Or use winget:
winget install Rustlang.Rust.MSVC
```

**macOS/Linux:**
```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env
```

#### 2. Verify Installation
```bash
rustc --version
cargo --version
```

#### 3. Clone and Build
```bash
git clone <repository-url>
cd proxy-server
cargo build --release
```

### Method 2: Using Cargo Install (Future Release)

```bash
# When published to crates.io
cargo install proxy-server

# Install from git repository
cargo install --git <repository-url> proxy-server
```

### Method 3: Download Binary Release (Future Release)

```bash
# Download pre-compiled binary for your platform
wget https://github.com/user/proxy-server/releases/latest/download/proxy-server-linux-x64.tar.gz

# Extract
tar -xzf proxy-server-linux-x64.tar.gz

# Install
sudo cp proxy-server /usr/local/bin/
```

## 🏗️ Building from Source

### Prerequisites
- Git
- Rust toolchain
- Build essentials (Linux only)

### Build Steps

#### 1. Clone Repository
```bash
git clone <repository-url>
cd proxy-server
```

#### 2. Development Build
```bash
# Debug build (faster compilation)
cargo build

# Run tests
cargo test

# Check code
cargo check
```

#### 3. Production Build
```bash
# Optimized release build
cargo build --release

# Binary location
# Windows: target/release/proxy-server.exe
# Unix: target/release/proxy-server
```

### Build Options

```bash
# Build with optimizations
cargo build --release

# Build for specific target
cargo build --release --target x86_64-unknown-linux-musl

# Build with verbose output
cargo build --release --verbose

# Build without default features (if available)
cargo build --release --no-default-features
```

## ⚙️ Configuration Setup

### 1. Create Configuration Directory
```bash
# Create config directory
mkdir -p ~/.config/proxy-server

# Or use /etc for system-wide
sudo mkdir -p /etc/proxy-server
```

### 2. Create Basic Configuration
```bash
# Create basic config file
cat > ~/.config/proxy-server/config.json << EOF
{
  "mode": "Reverse",
  "listen_addr": "127.0.0.1:8080",
  "static_files": {
    "mounts": [
      {
        "path": "/",
        "root_dir": "./public",
        "spa_mode": true
      }
    ]
  }
}
EOF
```

### 3. Create Systemd Service (Linux)
```bash
# Create service file
sudo tee /etc/systemd/system/proxy-server.service > /dev/null << EOF
[Unit]
Description=Proxy Server
After=network.target

[Service]
Type=simple
User=www-data
WorkingDirectory=/opt/proxy-server
ExecStart=/opt/proxy-server/proxy-server --config /etc/proxy-server/config.json
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
EOF

# Enable and start service
sudo systemctl enable proxy-server
sudo systemctl start proxy-server
```

### 4. Create Windows Service
```powershell
# Using NSSM (Non-Sucking Service Manager)
# Download from https://nssm.cc/download

nssm install ProxyServer "C:\path\to\proxy-server.exe" --config "C:\path\to\config.json"
nssm start ProxyServer
```

## ✅ Verification

### 1. Test Installation
```bash
# Check if binary is available
proxy-server --version

# Show help
proxy-server --help
```

### 2. Test Basic Functionality
```bash
# Create test directory
mkdir -p /tmp/test-site
echo "<h1>Hello World</h1>" > /tmp/test-site/index.html

# Start server
proxy-server --mode reverse --listen 127.0.0.1:8080 --static-dir /tmp/test-site

# Test in another terminal
curl http://127.0.0.1:8080/
```

### 3. Test Configuration File
```bash
# Test with config file
proxy-server --config /path/to/config.json

# Verify server is running
curl -I http://127.0.0.1:8080/
```

## 🔍 Troubleshooting

### Common Issues

#### 1. Rust Installation Problems
```bash
# Update Rust toolchain
rustup update stable

# Check PATH
echo $PATH | grep cargo

# Reinstall Rust
rustup self uninstall
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

#### 2. Build Failures
```bash
# Clean build
cargo clean
cargo build --release

# Update dependencies
cargo update

# Check for missing system dependencies
# Ubuntu/Debian:
sudo apt-get install build-essential pkg-config libssl-dev

# CentOS/RHEL:
sudo yum groupinstall "Development Tools"
sudo yum install openssl-devel
```

#### 3. Runtime Issues
```bash
# Check logs with debug output
RUST_LOG=debug proxy-server --config config.json

# Check configuration syntax
cat config.json | python -m json.tool

# Check permissions
ls -la /path/to/static/files
```

#### 4. Port Already in Use
```bash
# Find process using port
# Linux/macOS:
lsof -i :8080

# Windows:
netstat -ano | findstr :8080

# Kill process
kill -9 <PID>

# Or use different port
proxy-server --listen 127.0.0.1:3000
```

### Performance Issues

#### 1. High Memory Usage
```bash
# Monitor with htop/Process Monitor
# Check file descriptor limits
ulimit -n

# Optimize configuration
# - Reduce max_connections
# - Enable file compression
# - Use CDN for static assets
```

#### 2. Slow Startup
```bash
# Check for large directories
# Disable directory listing
# Use specific mount points instead of recursive serving
```

### Getting Help

1. **Check logs**: Enable debug logging `RUST_LOG=debug`
2. **Verify configuration**: Use JSON validator
3. **Check permissions**: Ensure read access to static files
4. **Network issues**: Verify firewall and port availability
5. **Open an issue**: Include configuration file and error logs

---

**Still having issues?** Please open an issue on the project repository with:
- Operating system and version
- Rust version (`rustc --version`)
- Configuration file (remove sensitive data)
- Error messages and logs

**Last Updated:** 2025-11-15