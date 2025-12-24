# B002: Static Mount Prefix Collision Bypasses Reverse Proxy Routes

## Bug Information
- **Bug ID**: B002
- **Title**: Static mount prefix collision bypasses reverse proxy routes
- **Severity**: High
- **Status**: Fixed
- **Date Reported**: 2025-12-24
- **Date Fixed**: 2025-12-24
- **Reporter**: User
- **Component**: Static Files / Combined Proxy
- **Type**: Bug

## Description
When running combined reverse proxy + static file mode, a static mount that is a prefix of a reverse proxy route (e.g., mount `/blog` and route `/blog-api/**`) causes requests to `/blog-api/...` to be handled by the static file handler. With SPA mode enabled, the handler returns the SPA fallback (`index.html`) instead of forwarding to the backend.

## Reproduction Steps
1. Use `examples/config_sam-blog.json` (static mount `/blog`, SPA mode enabled, reverse route `/blog-api/**` with `strip_path_prefix`).
2. Start a backend on `127.0.0.1:8080`.
3. Request `http://127.0.0.1:8088/blog-api/health`.
4. Observe the response is SPA HTML instead of the backend response.

## Expected Behavior
`/blog-api/...` should not match the `/blog` static mount and should be routed to the reverse proxy route.

## Root Cause
`StaticFileHandler::find_mount_for_path` matched mounts using `starts_with` without checking a path-segment boundary. `/blog-api/...` matched `/blog`, so the static handler ran first and never fell through to the reverse proxy.

## Fix
- Normalize mount paths to trim trailing slashes (except `/`).
- Require a boundary after the mount path (end-of-string or `/`) when matching.
- Added a regression test for `/static` vs `/static-api`.

## Related Issues
- [B001](./2025-01-17-spa-cache-bug.md) - SPA/static file behavior in the same module.

## Change History
| Date | Author | Change |
|------|--------|--------|
| 2025-12-24 | User | Reported incorrect routing for `/blog-api` |
| 2025-12-24 | Assistant | Fixed mount matching boundary checks |
