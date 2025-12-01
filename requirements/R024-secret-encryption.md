# R024: Encrypted Secret Management

## 📋 Requirement Overview

**Requirement ID**: R024  
**Title**: Encrypted Secret Management  
**Status**: ✅ Completed  
**Priority**: High  
**Date Raised**: 2025-02-14  
**Date Implemented**: 2025-02-14  
**Owner**: Platform Security Guild

## 🎯 Objective

Protect sensitive values that appear in configuration files (for example relay proxy passwords and API tokens) by adding a first-party workflow for key generation, client-side encryption, and automatic decryption inside the proxy.

## 📝 Description

### Current Behavior
- All secrets inside `config.json` or mount overrides are stored in plain text.
- Administrators must rely on ad-hoc OS encryption tools, which leads to inconsistent onboarding and higher risk of key exposure.
- The runtime has no way to differentiate between plain text and encrypted payloads, so everything is parsed literally.

### Expected Behavior
- Provide a CLI flag `--init-encryption-key` that generates a 256-bit AES key, splits it into three obscured fragments, and stores every asset under `~/.bifrost`.
- Provide a CLI utility mode `--encrypt <payload>` that encrypts an arbitrary short secret using the stored key and prints a `{encrypted}<ciphertext>` token.
- When the application parses configuration files it automatically detects values that start with `{encrypted}` and decrypts them just-in-time before using them in network calls; values without the prefix are treated as plain text for backward compatibility.

## 🔧 Technical Requirements

### R024.1: Key Initialization Flow
- Add CLI flag `--init-encryption-key`.
- Generate a cryptographically secure random 32-byte AES-256 key.
- Split the key into three fragments (Part A/B/C) before touching the disk; each fragment must contain at least 8 bytes.
- Generate an additional 32-byte XOR mask; every fragment that reaches the filesystem must be XOR'ed with this mask.
- Persist the fragments as Base64 inside `~/.bifrost/master_key.part{n}` and the XOR mask inside `~/.bifrost/master_key.mask`.
- Ensure `~/.bifrost` is created with `0700` permissions; existing files must never be overwritten unless `--force` is explicitly provided (future enhancement placeholder).

### R024.2: Secret Encryption CLI
- Add CLI flag `--encrypt <payload>` that can encrypt either the provided argument or STDIN (when `<payload>` is omitted).
- Automatically load and reconstruct the AES key by Base64-decoding each fragment, XOR-ing them with the mask, then concatenating in the original order.
- Use AES-256-GCM for authenticated encryption with a random 96-bit nonce per invocation; encode nonce + ciphertext + tag as Base64.
- Print the result in the canonical form `{encrypted}<base64 bundle>`.
- Fail with a clear error message if the key has not been initialized yet.

### R024.3: Configuration Loader Integration
- During JSON/YAML parsing, inspect every string value assigned to secret-bearing fields (`password`, `api_token`, `shared_key`, `relay_proxy.password`, etc.).
- When a value starts with `{encrypted}`, remove the prefix, Base64-decode the payload, reconstruct the AES key, and decrypt transparently before storing it in memory.
- When a value lacks the prefix, keep the legacy behavior and use it as-is.
- Decryption failures must surface a fatal configuration error that clearly identifies the field path.

### R024.4: Observability and Monitoring
- Emit structured logs when the encryption key is initialized, when encrypted payloads are produced, and when configurations are decrypted (log only metadata, never plaintext).
- Add Prometheus counters for successful/failed decryptions to simplify rollout monitoring (exposed as `bifrost_config_secret_decrypt_success_total` and `_failure_total`).

## 🏗️ Implementation Details

- Reuse the existing CLI parsing layer (likely `clap`) so `--init-encryption-key` and `--encrypt` can be invoked as standalone modes that exit after completion.
- Key fragments suggestion:
  - `master_key.part1`: bytes 0-10
  - `master_key.part2`: bytes 11-21
  - `master_key.part3`: bytes 22-31
  - Prior to writing, XOR each fragment byte-for-byte with `master_key.mask`.
- Reconstruction algorithm:
  1. Load mask and fragments from `~/.bifrost`.
  2. Base64-decode each blob.
  3. XOR each fragment with the mask and concatenate.
  4. Zeroize buffers after use to limit leakage.
- Use the same AES implementation (e.g., `aes-gcm` crate) for both encryption and decryption. The `{encrypted}` payload should encode `nonce || ciphertext || tag`.
- Extend the configuration schema to accept encrypted strings everywhere secrets are currently defined; update documentation examples accordingly.

## 🧪 Testing Strategy

- **Unit Tests**
  - Key splitting/reconstruction round-trip.
  - AES-256-GCM encrypt/decrypt parity with `{encrypted}` prefix.
  - Config parser detection of encrypted vs plain strings.
- **Integration Tests**
  - CLI workflow: run `--init-encryption-key`, then `--encrypt`, then boot the proxy with the generated payload inside a config fixture.
    - Error handling when key files are missing or malformed.
    - Permission check when `~/.bifrost` is world-readable (should warn and refuse to continue).
- **Security Regression**
    - Ensure logs never include decrypted secrets.
    - Verify temporary buffers are zeroized after use.

## ✅ Acceptance Criteria

- [x] CLI exposes `--init-encryption-key` and `--encrypt`.
- [x] Running `--init-encryption-key` creates masked fragments plus mask files under `~/.bifrost` with secure permissions.
- [x] `--encrypt` produces deterministic `{encrypted}...` tokens for the same plaintext + nonce but different tokens across runs thanks to random nonce.
- [x] Config loader decrypts `{encrypted}` secrets transparently and errors on tampering.
- [x] Telemetry counters/logs record successful and failed decryptions without leaking sensitive data.
- [x] Documentation (README, sample configs) explains how to initialize, encrypt, and consume secrets.

## 🔗 Related Requirements

- **R012 – Basic Authentication**: Encrypted secrets will replace the existing plain password fields.
- **R015 – Logging System**: Structured logging must align with the observability requirements stated here.

---

## 🧾 Implementation Summary

- Added `SecretManager` with AES-256-GCM encryption, masked key fragments on disk, and secure permission enforcement.
- Extended the CLI with `--init-encryption-key` and `--encrypt` utility modes, including stdin support for piping secrets.
- Configuration loader detects `{encrypted}` tokens across proxy password fields, decrypts on boot, and surfaces errors with field context.
- New Prometheus counters (`bifrost_config_secret_decrypt_success_total`, `bifrost_config_secret_decrypt_failure_total`) plus structured logs provide rollout telemetry.
- Documentation updated with end-to-end workflows; tests cover key splitting and decrypt flows.

**Status**: Completed  
**Last Updated**: 2025-02-14
