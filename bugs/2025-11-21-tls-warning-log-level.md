# Not a Bug: TLS Warning Log Level

## Bug Report

**Bug ID**: NB002  
**Title**: TLS warning emitted when browser hits HTTPS endpoint with untrusted cert  
**Severity**: Information  
**Status**: ✅ **Not a Bug - System Working as Intended**  
**Date Reported**: 2025-11-21  
**Date Resolved**: 2025-11-21  
**Type**: Logging behavior clarification

## Context

- HTTPS static file server logs `WARN  ... Error establishing TLS connection ... CertificateUnknown`
- Browser attempted to reach `https://127.0.0.1:8443` but rejected the certificate during the handshake
- Question raised: should this warning be downgraded to `DEBUG` because it can be triggered by routine browser behavior?

## Discussion Summary

1. TLS handshake failed before any HTTP request was processed, so from the server perspective a client connection definitively failed.  
2. `CertificateUnknown` indicates that the peer rejected the certificate trust chain (self-signed, mismatched hostname, etc.), which typically requires user/operator intervention.  
3. Operators usually rely on warning-level logs to surface TLS misconfiguration or hostile probes; emitting only at debug would hide actionable information in production.  
4. Legitimate clients would never complete a request until the certificate issue is fixed, so surfacing the failure at warning level helps diagnose connectivity problems quickly.

## Resolution

- The existing `warn!("Error establishing TLS connection from {}: {}", remote_addr, e);` log in `src/proxy.rs` is appropriate.  
- Certificate trust failures are significant enough to stay at warning level because they block successful HTTPS sessions.  
- Local development annoyance can be mitigated by trusting the certificate or adding a configuration flag, but the default behavior remains unchanged.

### Final Assessment

**Status**: ✅ **NOT A BUG** — The warning accurately reflects a failed TLS handshake and should remain a warning by default to alert operators to certificate problems.

**Follow-up**: None required.
