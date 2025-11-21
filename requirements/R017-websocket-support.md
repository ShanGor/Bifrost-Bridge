# R017: WebSocket Support

**Status:** ✅ Completed
**Date Raised:** TBD
**Category:** Protocol Support

## 📋 Description

Support WebSocket proxying to enable real-time communication through the proxy server.

## 🎯 Planned Features

### WebSocket Proxying
- Forward WebSocket connections in forward proxy mode
- WebSocket reverse proxy support
- Connection upgrade handling
- Subprotocol negotiation

### Configuration
- WebSocket-specific routing rules
- Connection timeout configuration
- Origin validation
- Protocol filtering

## ✅ Implementation Summary

- Added automatic detection of `Upgrade: websocket` requests and bridged them end-to-end using Hyper's upgrade API for both forward and reverse proxy modes.
- Reverse proxy now validates `Origin` and `Sec-WebSocket-Protocol` headers against the new configuration block before establishing tunnels, and streams bytes between client and backend using Tokio's bidirectional copy.
- Forward proxy supports direct HTTP(S) WebSocket upgrades (WSS via CONNECT already worked) and rejects relay-configured routes until dedicated handling is implemented.
- Shared `websocket` configuration controls allowed origins, subprotocols, and per-tunnel timeouts, and is exposed alongside monitoring config in `docs/configuration.md`.

## 🔧 Configuration

```json
{
  "websocket": {
    "enabled": true,
    "allowed_origins": ["*"],
    "supported_protocols": ["chat", "notification"],
    "timeout_seconds": 300
  }
}
```

**Back to:** [Requirements Index](../requirements/README.md)
