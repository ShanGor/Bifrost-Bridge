# R009: HTTPS Support

**Status:** ✅ Completed
**Date Completed:** 2025-11-15
**Category:** Security

## 📋 Description

Add HTTPS server with private key and certificate file paths using rustls library with PKCS#8 private key and PEM certificate format support.

## 🎯 Implementation

Full HTTPS/TLS support using rustls and tokio-rustls with secure defaults and comprehensive certificate handling.

## 🔧 Technical Details

- Added `private_key` and `certificate` fields to main Config struct
- Implemented TLS server configuration using rustls and tokio-rustls
- Added `create_tls_config()` helper function for certificate loading
- Supports PKCS#8 PEM format for private keys and PEM format for certificates
- Automatic HTTPS mode when both certificate files are configured
- Uses rustls for secure, modern TLS implementation with safe defaults

## ⚙️ Configuration

### Command Line
```bash
cargo run -- \
  --mode reverse \
  --listen 127.0.0.1:8443 \
  --private-key ./certs/private-key.pem \
  --certificate ./certs/certificate.pem \
  --static-dir ./dist \
  --spa
```

### JSON Configuration
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

## 🔒 Certificate Support

### Formats
- **Private Key Format:** PKCS#8 PEM (recommended for modern security)
- **Certificate Format:** PEM with full certificate chain
- **Key Types:** RSA and ECDSA supported
- **Security:** Uses rustls with safe cipher suites and TLS 1.2+ support

### Certificate Generation
```bash
# Generate self-signed certificate for development
openssl req -x509 -newkey rsa:2048 -keyout private-key.pem -out certificate.pem -days 365 -nodes -pkcs8

# Or with ECDSA (more modern)
openssl ecparam -name secp384r1 -genkey -noout -out private-key.pem
openssl req -new -x509 -key private-key.pem -out certificate.pem -days 365
```

## 📁 Mode Support

- ✅ **Static File Serving**: Full HTTPS implementation for reverse proxy without backend target
- ✅ **Forward Proxy**: Complete HTTPS support with TLS termination and request forwarding
- 🔄 **Reverse Proxy**: HTTPS configuration accepted but not yet implemented (backend target mode)

## 📁 Files Modified

- `src/config.rs`: Added certificate and private key fields
- `src/proxy.rs`: Added HTTPS/TLS server implementation
- `src/main.rs`: Added certificate validation

## ✅ Benefits

- **Secure Communication**: HTTPS encryption for production deployments
- **Modern TLS**: Rustls provides secure-by-default implementation
- **Standard Formats**: Support for standard certificate formats
- **Auto Detection**: Automatic HTTPS mode detection
- **Comprehensive**: Complete certificate handling and validation

## 🔍 Troubleshooting

### Browser Connection Issues
If you see `InvalidContentType` errors:
1. Ensure you're using `https://` instead of `http://` in the browser URL
2. Accept the self-signed certificate warning in development
3. Check that certificate files exist and are readable

### Certificate Validation
- Verify certificate files are in correct PEM format
- Ensure private key matches certificate
- Check file permissions and paths

**Back to:** [Requirements Index](../requirements/README.md)