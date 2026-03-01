# R027: Environment Variable Interpolation in Configuration

## 📋 Requirement Overview

**Requirement ID**: R027  
**Title**: Environment Variable Interpolation in Configuration  
**Status**: ✅ Completed  
**Priority**: High  
**Date Raised**: 2026-03-01  
**Date Implemented**: 2026-03-01  
**Owner**: Platform Runtime Guild

## 🎯 Objective

Allow configuration string values to reference process environment variables so operators can keep credentials and per-environment values outside static config files.

## 📝 Description

### Current Behavior
- Configuration values are parsed literally from JSON.
- Secret-bearing fields support plain text and `{encrypted}...` values, but do not support `${VAR}` / `$VAR` expansion.
- Operators must duplicate environment-specific values directly in config files.

### Expected Behavior
- Interpolate environment variables in all JSON string values during config loading.
- Support both `$VAR` and `${VAR}` forms.
- Support literal dollar escaping with `$$`.
- Produce a clear configuration error when a referenced environment variable is missing or malformed.

## 🔧 Technical Requirements

### R027.1: Interpolation Syntax
- Recognize unbraced variables with shell-style name rules:
  - `$VAR`
  - variable name pattern: `[A-Za-z_][A-Za-z0-9_]*`
- Recognize braced variables:
  - `${VAR}`
  - same name rules as above
- Recognize escaped dollar:
  - `$$` resolves to `$`

### R027.2: Config Loader Integration
- Apply interpolation before deserializing config JSON into typed structs.
- Traverse the full JSON value tree and interpolate string values recursively:
  - top-level fields
  - nested objects
  - arrays
- Preserve non-string values unchanged.

### R027.3: Error Handling
- Fail fast when a variable reference is present but not set in the environment.
- Fail fast for malformed interpolation tokens (for example unterminated `${...}` or invalid variable names).
- Error message must include both:
  - variable name (when available)
  - JSON field path of the failing string

### R027.4: Compatibility Rules
- Existing plain text config values must continue to work without changes.
- Existing `{encrypted}...` secret flow remains supported.
- Interpolation occurs as part of config loading and does not change runtime proxy behavior.

## 🧪 Testing Strategy

- **Unit Tests**
  - `$VAR` and `${VAR}` both resolve correctly.
  - Mixed string interpolation resolves multiple variables in one value.
  - `$$` correctly emits literal `$`.
  - Missing variable produces path-aware failure.
  - Invalid `${...}` syntax is rejected.
- **Integration Tests**
  - `Config::from_file` loads a config containing interpolation placeholders and produces resolved typed values.
  - Nested fields (for example relay proxy URL entries) are interpolated correctly.

## ✅ Acceptance Criteria

- [x] Config strings support `$VAR` and `${VAR}` interpolation.
- [x] `$$` escapes to a literal `$`.
- [x] Missing env vars produce a clear startup error with field path context.
- [x] Interpolation is applied recursively across config string fields.
- [x] Existing plain text and `{encrypted}` values remain backward compatible.
- [x] Tests cover syntax, failures, and config loader integration.

## 🔗 Related Requirements

- **R024 – Encrypted Secret Management**: interpolation complements encrypted/plain secret inputs by supporting environment-sourced values.
- **R006 – Documentation Setup**: requirement index and feature documentation must stay current.

---

## 🧾 Implementation Summary

- Added recursive environment interpolation in the config loading pipeline.
- Added support for `$VAR`, `${VAR}`, and escaped `$$`.
- Added strict validation and path-aware errors for missing/malformed references.
- Added unit tests for interpolation parsing and config-level integration.

**Status**: Completed  
**Last Updated**: 2026-03-01
