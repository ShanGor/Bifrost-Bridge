# R018: Rate Limiting

**Status:** ✅ Completed
**Date Raised:** TBD
**Category**: Security

## 📋 Description

Add configurable rate limiting to prevent abuse and protect server resources.

## 🎯 Planned Features

- Request rate limiting by IP
- Configurable time windows
- Multiple rate limit tiers
- Custom rate limit rules per endpoint

## ✅ Implementation Summary

- Added a shared asynchronous rate limiter with token-bucket semantics and HTTP `429` responses surfaced across forward proxy (including raw CONNECT), reverse proxy, and static/combined adapters so every entry point enforces limits by client IP.
- Introduced a `rate_limiting` configuration block (`enabled`, `default_limit`, and per-rule path/method tiers) that supports multiple windows and endpoint-specific overrides without impacting existing configs.
- Updated documentation and monitoring logs to highlight rate-limit events and exposed human-friendly error bodies plus `Retry-After` headers for easier client back-off handling.

**Back to:** [Requirements Index](../requirements/README.md)

**Back to:** [Requirements Index](../requirements/README.md)
