# B001: SPA Cache Control Bug

## Bug Information
- **Bug ID**: B001
- **Title**: SPA cache control for index files and fallbacks
- **Severity**: Medium
- **Status**: 🟢 Fixed
- **Date Reported**: 2025-01-17
- **Date Fixed**: 2025-01-17
- **Implementation Ready**: ✅ Yes - Detailed plan available
- **Reporter**: User
- **Component**: Static Files / SPA Mode
- **Type**: Enhancement/Requirement Bug

## Description

### Current Behavior
When serving static files in SPA (Single Page Application) mode, the system applies the same caching strategy to all files, including:
- Index files (index.html, index.htm, etc.)
- Fallback responses to index.html for SPA routing
- Regular static assets

### Expected Behavior
For SPA applications, we expect **no caching** for:
1. **Index files** (index.html, index.htm) - because they contain dynamic content and version info
2. **Fallback responses** to index.html - because these serve dynamic SPA content
3. **Not found paths** that fall back to index file - these should not be cached

Regular static assets (CSS, JS, images, etc.) should still use normal caching.

### User Justification

> "for SPA product, other files will change their name if there are changes, but index files will not"

This is a standard SPA development practice where:
- Static assets get unique filenames with hashes (app.abc123.css, main.def456.js)
- Index.html files remain the same name but contain dynamic content
- Fallback routing in SPA should not be cached to ensure fresh content

## Technical Details

### Current Implementation
The caching is likely handled uniformly in the static files module without considering SPA-specific caching needs.

### SPA Caching Requirements
1. **Index files**: Cache-Control: no-cache, no-store, must-revalidate
2. **SPA fallbacks**: Same as index files (no caching)
3. **Static assets**: Normal caching based on file extension and config
4. **404 responses in SPA mode**: Should not be cached when falling back to index

### File Locations to Investigate
- `src/static_files.rs` - Main static file serving logic
- Response headers configuration
- SPA fallback implementation
- Cache control header setting

## Reproduction Steps

1. Configure Bifrost Bridge in SPA mode
2. Create an SPA application with index.html and static assets
3. Make requests to:
   - `/index.html` - should be cached (currently)
   - `/non-existent-route` - falls back to index.html (currently cached)
   - `/assets/app.css` - should be cached normally
4. Check response headers for Cache-Control
5. Expected: index files and fallbacks should have no-cache headers

## Impact Assessment

### Impact on Users
- **SPA Developers**: May see stale content if index.html is cached
- **End Users**: Might not see the latest SPA version after deployment
- **Deployment Process**: Cache invalidation complexity for SPA deployments

### Impact on System
- **Performance**: May slightly reduce performance for SPA index files
- **Caching Efficiency**: More granular caching control improves overall effectiveness

## Proposed Solutions

### Option 1: SPA-Aware Caching (Recommended)
- Detect SPA mode in static file handler
- Apply no-cache headers to index files and fallback responses
- Maintain normal caching for other static assets

### Option 2: Configuration-Based SPA Caching
- Add SPA-specific cache control configuration options
- Allow users to customize SPA caching behavior
- Provide sensible defaults for SPA mode

### Option 3: File-Pattern Based Caching
- Use file patterns to determine caching strategy
- `index.*` files get no-cache headers in SPA mode
- Fallback responses inherit SPA caching rules

## Acceptance Criteria

- [ ] SPA mode detected correctly
- [ ] Index files (index.html, index.htm) have no-cache headers
- [ ] SPA fallback responses to index.html have no-cache headers
- [ ] 404 → index.html fallbacks are not cached
- [ ] Regular static assets still use normal caching
- [ ] Configuration allows SPA caching customization
- [ ] Performance impact is minimal
- [ ] Backwards compatibility maintained for non-SPA modes

## Implementation Notes

### Areas to Modify
1. **FileStreaming::create_optimized_response()** in `src/common.rs` (line 166)
   - **Current**: Always sets `Cache-Control: public, max-age=3600`
   - **Fix**: Add SPA mode parameter and conditional cache headers

2. **StaticFileHandler::handle_file()** in `src/static_files.rs` (line 279)
   - **Current**: Always calls `create_optimized_response()` with standard caching
   - **Fix**: Pass SPA mode information to file handling

3. **StaticFileHandler::handle_spa_fallback_in_mount()** in `src/static_files.rs` (line 164)
   - **Current**: Uses `handle_file()` which applies normal caching
   - **Fix**: Ensure SPA fallbacks get no-cache headers

4. **StaticFileHandler::handle_directory_in_mount()** in `src/static_files.rs` (line 175)
   - **Current**: Index files served with normal caching in SPA mode
   - **Fix**: Detect index files in SPA mode and apply no-cache

### Implementation Details Found

**Root Cause**: Line 192 in `src/common.rs`:
```rust
.header("Cache-Control", "public, max-age=3600")
```
This is applied to ALL file responses, including SPA index files and fallbacks.

**SPA Detection Available**:
- `mount_info.resolved_mount.spa_mode` - Boolean flag indicating SPA mode
- `spa_fallback_file` - Fallback file name (usually "index.html")
- `index_files` - Index file patterns (["index.html", "index.htm"])

**Current SPA Flow**:
1. Missing file + SPA mode + not asset → `handle_spa_fallback_in_mount()`
2. Directory + no index + SPA mode → `handle_spa_fallback_in_mount()`
3. Directory + index file found → `handle_file()` → `create_optimized_response()`
4. All above paths lead to same caching header (3600s max-age)

### Proposed Implementation Plan

**Step 1**: Modify `create_optimized_response()` signature
```rust
pub async fn create_optimized_response(
    file_path: &Path,
    content_type: &str,
    file_size: u64,
    is_head: bool,
    spa_mode: bool, // NEW PARAMETER
    is_spa_fallback: bool, // NEW PARAMETER
) -> Result<Response<Full<Bytes>>, ProxyError>
```

**Step 2**: Add SPA-aware cache control logic
```rust
let cache_control = if spa_mode || is_spa_fallback {
    "no-cache, no-store, must-revalidate"
} else {
    "public, max-age=3600"
};
```

**Step 3**: Update all callers to pass SPA context
- `handle_file()` → needs mount info for SPA mode
- `handle_spa_fallback_in_mount()` → set is_spa_fallback=true
- Directory index handling → detect index files in SPA mode

**Step 4**: Add SPA file detection helper
```rust
fn is_index_file(path: &Path) -> bool {
    if let Some(file_name) = path.file_name().and_then(|n| n.to_str()) {
        file_name == "index.html" || file_name == "index.htm"
    } else {
        false
    }
}
```

### Testing Requirements
- Unit tests for SPA caching logic
- Integration tests for different SPA scenarios
- Performance tests to ensure minimal impact
- Backwards compatibility tests for non-SPA modes

## Dependencies

- None identified (internal enhancement)

## Risks and Mitigations

### Risks
- **Performance Impact**: No-cache headers may reduce performance
- **Backwards Compatibility**: Changes might affect non-SPA setups

### Mitigations
- **Selective Application**: Only affect SPA mode, not regular static serving
- **Configuration Override**: Allow users to override SPA caching defaults
- **Performance Testing**: Benchmark to ensure minimal impact

## Documentation Updates

Required documentation changes:
- README.md SPA section
- Configuration documentation
- API documentation for caching options
- SPA deployment guide

## Related Issues

- No related issues currently tracked

## Change History

| Date | Author | Change |
|------|--------|--------|
| 2025-01-17 | User | Initial bug report and analysis |

---

**Status**: 🔴 **Open** - This bug has been identified and analyzed but not yet implemented. Waiting for prioritization and development assignment.