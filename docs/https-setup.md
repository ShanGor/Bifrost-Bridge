# HTTPS Setup Guide

This guide covers how to configure the proxy server to use HTTPS with TLS/SSL certificates.

## 📋 Table of Contents

- [HTTPS Overview](#https-overview)
- [Certificate Formats](#certificate-formats)
- [Generating Certificates](#generating-certificates)
- [Configuration](#configuration)
- [Examples](#examples)
- [Troubleshooting](#troubleshooting)
- [Security Considerations](#security-considerations)

## 🔐 HTTPS Overview

The proxy server supports HTTPS/TLS using the rustls library. When both `private_key` and `certificate` are configured, the server automatically switches to HTTPS mode.

**Currently Supported:**
- ✅ Static file serving (reverse proxy mode without backend target)
- ✅ Forward proxy mode (HTTPS implemented)
- 🔄 Reverse proxy mode (HTTPS support planned)

For reverse proxy mode with backend target, HTTPS configuration is accepted but not yet implemented. The server will run in HTTP mode for these configurations.

### How it Works

1. **TLS Termination**: The proxy server handles TLS encryption/decryption
2. **Certificate Validation**: Server presents its certificate to clients
3. **Secure Communication**: All traffic between clients and the proxy is encrypted
4. **Backend Communication**: Proxy-to-backend communication remains HTTP unless otherwise configured

## 📄 Certificate Formats

The proxy server supports **PKCS#8 PEM format** for private keys and **PEM format** for certificates.

### Supported Formats

#### Private Key (PKCS#8 PEM)
```
-----BEGIN PRIVATE KEY-----
MIIJQ...
-----END PRIVATE KEY-----
```

#### Certificate (PEM)
```
-----BEGIN CERTIFICATE-----
MIID...
-----END CERTIFICATE-----
```

### Converting Between Formats

If you have certificates in other formats, you can convert them using OpenSSL:

#### From Traditional RSA Private Key to PKCS#8
```bash
openssl pkcs8 -topk8 -inform PEM -in traditional.key -outform PEM -nocrypt -out pkcs8.key
```

#### From PFX/P12 to PEM
```bash
# Extract private key
openssl pkcs12 -in certificate.pfx -nocerts -nodes -out private.key

# Convert to PKCS#8
openssl pkcs8 -topk8 -inform PEM -in private.key -outform PEM -nocrypt -out pkcs8.key

# Extract certificate
openssl pkcs12 -in certificate.pfx -clcerts -nokeys -out certificate.crt
```

## 🔧 Generating Certificates

### For Development: Self-Signed Certificates

#### Generate a Self-Signed Certificate (OpenSSL)
```bash
# Generate private key (PKCS#8 format)
openssl genpkey -algorithm RSA -out private.key -pkcs8

# Generate self-signed certificate
openssl req -new -x509 -key private.key -out certificate.crt -days 365

# Or with one command (RSA)
openssl req -x509 -newkey rsa:2048 -keyout private.key -out certificate.crt -days 365 -nodes -pkcs8

# Or with ECDSA (more modern)
openssl ecparam -name secp384r1 -genkey -noout -out private.key
openssl req -new -x509 -key private.key -out certificate.crt -days 365
```

#### Generate a Self-Signed Certificate (EasyRSA)
```bash
# Download and setup EasyRSA
git clone https://github.com/OpenVPN/easy-rsa.git
cd easy-rsa/easyrsa3

# Initialize PKI
./easyrsa init-pki
./easyrsa build-ca nopass

# Generate server certificate
./easyrsa gen-req server nopass
./easyrsa sign-req server server

# Copy certificates
cp pki/ca.crt certificate.crt
cp pki/issued/server.crt certificate.crt
cp pki/private/server.key private.key

# Convert to PKCS#8 if needed
openssl pkcs8 -topk8 -inform PEM -in pki/private/server.key -outform PEM -nocrypt -out private.key
```

### For Production: Let's Encrypt (Certbot)

#### Install Certbot
```bash
# Ubuntu/Debian
sudo apt update
sudo apt install certbot

# CentOS/RHEL
sudo dnf install certbot

# macOS
brew install certbot
```

#### Generate Certificate
```bash
# Generate certificate (requires domain ownership)
certbot certonly --standalone -d your-domain.com

# Copy certificates to proxy server location
sudo cp /etc/letsencrypt/live/your-domain.com/fullchain.pem certificate.crt
sudo cp /etc/letsencrypt/live/your-domain.com/privkey.pem private.key
```

#### Auto-Renewal
```bash
# Add to crontab for auto-renewal
0 12 * * * /usr/bin/certbot renew --quiet
```

## ⚙️ Configuration

### JSON Configuration

Add the `private_key` and `certificate` fields to your configuration:

```json
{
  "mode": "Reverse",
  "listen_addr": "0.0.0.0:8443",
  "static_files": {
    "mounts": [
      {
        "path": "/",
        "root_dir": "./dist",
        "spa_mode": true
      }
    ]
  },
  "private_key": "./certs/private-key.pem",
  "certificate": "./certs/certificate.pem"
}
```

### Command Line Arguments

```bash
cargo run -- \
  --mode reverse \
  --listen 0.0.0.0:8443 \
  --private-key ./certs/private-key.pem \
  --certificate ./certs/certificate.pem \
  --static-dir ./dist \
  --spa
```

### Configuration Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `private_key` | String | ✅ Yes | Path to PKCS#8 PEM format private key file |
| `certificate` | String | ✅ Yes | Path to PEM format certificate file |

### HTTPS Behavior

- **Port**: Typically use 443 (standard HTTPS) or 8443 (development)
- **Protocol**: Server will only accept HTTPS connections when both files are configured
- **Mixed Mode**: HTTP and HTTPS cannot run simultaneously; configure one or the other
- **Certificates**: Must be valid and not expired for production use

## 💡 Examples

### Example 1: Development HTTPS Server

**Directory Structure:**
```
project/
├── certs/
│   ├── private-key.pem
│   └── certificate.pem
├── dist/
│   └── index.html
└── config.json
```

**config.json:**
```json
{
  "mode": "Reverse",
  "listen_addr": "127.0.0.1:8443",
  "static_files": {
    "mounts": [
      {
        "path": "/",
        "root_dir": "./dist",
        "spa_mode": true
      }
    ]
  },
  "private_key": "./certs/private-key.pem",
  "certificate": "./certs/certificate.pem"
}
```

**Run Command:**
```bash
cargo run -- --config config.json
```

**Access:** https://localhost:8443

### Example 2: Production HTTPS Server

**config.json:**
```json
{
  "mode": "Reverse",
  "listen_addr": "0.0.0.0:443",
  "max_connections": 5000,
  "timeout_secs": 60,
  "static_files": {
    "mounts": [
      {
        "path": "/app",
        "root_dir": "/var/www/app/dist",
        "spa_mode": true
      },
      {
        "path": "/api-docs",
        "root_dir": "/var/www/api-docs",
        "enable_directory_listing": true
      }
    ],
    "enable_directory_listing": false
  },
  "private_key": "/etc/ssl/private/app.key",
  "certificate": "/etc/ssl/certs/app.crt"
}
```

**Systemd Service:**
```ini
[Unit]
Description=Proxy Server
After=network.target

[Service]
Type=simple
User=www-data
WorkingDirectory=/opt/proxy-server
ExecStart=/opt/proxy-server/target/release/proxy-server --config /opt/proxy-server/config.json
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
```

### Example 3: Multi-Environment Setup

**config.development.json:**
```json
{
  "mode": "Reverse",
  "listen_addr": "127.0.0.1:8443",
  "static_files": {
    "mounts": [
      {
        "path": "/",
        "root_dir": "./dist",
        "spa_mode": true
      }
    ]
  },
  "private_key": "./certs/dev-key.pem",
  "certificate": "./certs/dev-cert.pem"
}
```

**config.production.json:**
```json
{
  "mode": "Reverse",
  "listen_addr": "0.0.0.0:443",
  "static_files": {
    "mounts": [
      {
        "path": "/",
        "root_dir": "/var/www/html",
        "spa_mode": true
      }
    ]
  },
  "private_key": "/etc/ssl/private/production.key",
  "certificate": "/etc/ssl/certs/production.crt"
}
```

### Example 4: HTTPS Forward Proxy

**Command Line:**
```bash
cargo run -- \
  --mode forward \
  --listen 127.0.0.1:8888 \
  --private-key ./certs/private-key.pem \
  --certificate ./certs/certificate.pem
```

**JSON Configuration:**
```json
{
  "mode": "Forward",
  "listen_addr": "127.0.0.1:8888",
  "private_key": "./certs/private-key.pem",
  "certificate": "./certs/certificate.pem"
}
```

**Usage:** Configure your browser or application to use `https://127.0.0.1:8888` as the HTTPS proxy server. The proxy will handle TLS termination and forward requests to their destinations.

## 🔍 Troubleshooting

### Common Issues

#### Certificate Not Found
```
Error: Failed to open private key file: No such file or directory (os error 2)
```
**Solution:** Ensure file paths are correct and files exist with proper permissions.

#### Invalid Certificate Format
```
Error: Failed to read private key: no valid private key found
```
**Solution:** Convert private key to PKCS#8 PEM format:
```bash
openssl pkcs8 -topk8 -inform PEM -in your-key.key -outform PEM -nocrypt -out pkcs8-key.pem
```

#### Certificate Chain Issues
```
Error: Failed to create TLS config: invalid certificate
```
**Solution:** Ensure certificate file contains the full certificate chain:
```bash
# Combine server certificate with intermediate certificates
cat server.crt intermediate.crt ca.crt > fullchain.pem
```

#### Permission Denied
```
Error: Failed to open certificate file: Permission denied
```
**Solution:** Set proper file permissions:
```bash
chmod 600 private-key.pem  # Private key should be readable only by owner
chmod 644 certificate.pem  # Certificate can be readable by all
```

#### Port Already in Use
```
Error: Address already in use (os error 48)
```
**Solution:** Stop other services using the port or use a different port.

### Testing HTTPS Configuration

#### Test Certificate Validity
```bash
# Check certificate information
openssl x509 -in certificate.pem -text -noout

# Verify private key matches certificate
openssl x509 -noout -modulus -in certificate.pem | openssl md5
openssl rsa -noout -modulus -in private-key.pem | openssl md5
# Both commands should output the same MD5 hash

# Check certificate expiration
openssl x509 -in certificate.pem -noout -dates
```

#### Test HTTPS Connection
```bash
# Test with curl
curl -v https://localhost:8443

# Test with OpenSSL client
openssl s_client -connect localhost:8443 -servername localhost

# Test certificate chain
openssl s_client -connect localhost:8443 -verify_hostname localhost
```

## 🔒 Security Considerations

### Production Security

1. **Use Valid Certificates**: Don't use self-signed certificates in production
2. **Certificate Renewal**: Set up automated renewal for Let's Encrypt certificates
3. **File Permissions**: Restrict access to private key files (`chmod 600`)
4. **HTTPS Only**: Disable HTTP when using HTTPS in production
5. **HSTS**: Consider implementing HTTP Strict Transport Security
6. **Forward Secrecy**: Use modern cipher suites (handled by rustls)

### Certificate Best Practices

1. **Key Strength**: Use at least 2048-bit RSA or 256-bit ECDSA
2. **Certificate Validity**: Keep certificate validity periods short (1-2 years)
3. **Monitoring**: Monitor certificate expiration dates
4. **Backup**: Securely backup certificates and private keys
5. **Revocation**: Have a process for revoking compromised certificates

### HTTPS Headers (Future Enhancement)

Consider adding these security headers in future versions:

```http
Strict-Transport-Security: max-age=31536000; includeSubDomains
X-Frame-Options: DENY
X-Content-Type-Options: nosniff
Referrer-Policy: strict-origin-when-cross-origin
```

---

**Last Updated:** 2025-11-15
**See Also:** [Configuration Guide](./configuration.md), [Examples](../examples/)