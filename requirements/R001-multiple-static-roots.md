# R001: Multiple Static Roots Support

**Status:** ✅ Completed
**Date Completed:** 2025-11-15
**Category:** Core Feature

## 📋 Description

Enhance static file serving to support multiple root directories with different mount points, allowing the proxy server to serve static content from multiple directories at different URL paths.

## 🎯 Implementation

Added `StaticMount` struct with path-based routing that enables serving static files from multiple directories simultaneously.

## ⚙️ Configuration

### CLI Usage
```bash
# Mount multiple directories
cargo run -- \
  --mount "/app:/path/to/frontend/dist" \
  --mount "/api:/path/to/api/docs" \
  --mount "/assets:/path/to/static/files"
```

### JSON Configuration
```json
{
  "static_files": {
    "mounts": [
      {"path": "/app", "root_dir": "./frontend/dist"},
      {"path": "/api", "root_dir": "./api-docs"},
      {"path": "/assets", "root_dir": "./static"}
    ]
  }
}
```

## 🔗 URL Mapping

- `/app/*` → `/path/to/frontend/dist/*`
- `/api/*` → `/path/to/api/docs/*`
- `/assets/*` → `/path/to/static/files/*`

## 📁 Files Modified

- `src/config.rs`: Added `StaticMount` struct
- `src/static_files.rs`: Enhanced with multi-mount support
- `src/main.rs`: Added CLI argument parsing for mounts

## ✅ Benefits

- **Flexible Content Organization**: Serve different types of content from different directories
- **Microservices Support**: Multiple frontend applications on single proxy
- **Development Efficiency**: Separate static assets during development
- **Production Ready**: Clean separation of concerns for static content

**Back to:** [Requirements Index](../requirements/README.md)