//! Native services exposed to access plugins through opaque JSON bridges.
//!
//! Secrets live in `CredentialVault` and are represented in JavaScript only by
//! random, invocation-local handles. Network access and JWT key locations are
//! constrained by process configuration rather than values embedded in a JWT.

use crate::config::{PluginJwtKeySourceConfig, PluginJwtVerifierConfig, PluginRuntimeConfig};
use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, KeyInit},
};
use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use futures::StreamExt;
use jsonwebtoken::jwk::{AlgorithmParameters, Jwk, JwkSet, KeyOperations, PublicKeyUse};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use rand::RngCore;
use reqwest::{Client, Url};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::{Mutex as AsyncMutex, watch};
use zeroize::{Zeroize, Zeroizing};

#[derive(Clone)]
pub(crate) struct HostRuntime {
    client: Client,
    allowed_hosts: Arc<Vec<String>>,
    timeout: Duration,
    max_response_bytes: usize,
    max_cache_entries: usize,
    max_cache_entry_bytes: usize,
    max_cache_bytes: usize,
    max_cache_ttl: Duration,
    cache: Arc<AsyncMutex<CacheState>>,
    jwt: Arc<AsyncMutex<HashMap<String, CachedKeys>>>,
    verifier_policies: Arc<HashMap<String, PluginJwtVerifierConfig>>,
    cache_encryption_key: Arc<Zeroizing<[u8; 32]>>,
}

impl HostRuntime {
    pub(crate) fn new(config: &PluginRuntimeConfig) -> Result<Self, String> {
        if config.http_timeout_millis == 0
            || config.max_response_bytes == 0
            || config.max_cache_entries == 0
            || config.max_cache_entry_bytes == 0
            || config.max_cache_bytes == 0
            || config.max_cache_ttl_seconds == 0
        {
            return Err("plugin host limits must be non-zero".to_string());
        }
        let timeout = Duration::from_millis(config.http_timeout_millis);
        if config.tls.client_certificate.is_some() != config.tls.client_private_key.is_some() {
            return Err(
                "plugin TLS client certificate and private key must be configured together"
                    .to_string(),
            );
        }
        let mut client_builder = Client::builder()
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .https_only(true);
        if let Some(path) = &config.tls.custom_ca_bundle {
            let bytes = fs::read(path).map_err(|error| {
                format!("cannot read plugin CA bundle {}: {error}", path.display())
            })?;
            let certificates = reqwest::Certificate::from_pem_bundle(&bytes)
                .or_else(|_| {
                    reqwest::Certificate::from_der(&bytes).map(|certificate| vec![certificate])
                })
                .map_err(|_| format!("plugin CA bundle {} is invalid", path.display()))?;
            for certificate in certificates {
                client_builder = client_builder.add_root_certificate(certificate);
            }
        }
        if let (Some(certificate), Some(private_key)) = (
            &config.tls.client_certificate,
            &config.tls.client_private_key,
        ) {
            let mut identity = fs::read(certificate).map_err(|error| {
                format!(
                    "cannot read plugin client certificate {}: {error}",
                    certificate.display()
                )
            })?;
            identity.extend_from_slice(b"\n");
            identity.extend_from_slice(&fs::read(private_key).map_err(|error| {
                format!(
                    "cannot read plugin client key {}: {error}",
                    private_key.display()
                )
            })?);
            let identity = reqwest::Identity::from_pem(&identity)
                .map_err(|_| "plugin TLS client identity is invalid".to_string())?;
            client_builder = client_builder.identity(identity);
        }
        let client = client_builder
            .build()
            .map_err(|error| format!("cannot build plugin HTTP client: {error}"))?;
        let allowed_hosts = config
            .allowed_egress_hosts
            .iter()
            .map(|host| host.to_ascii_lowercase())
            .collect();
        let mut cache_encryption_key = [0_u8; 32];
        rand::thread_rng().fill_bytes(&mut cache_encryption_key);
        let runtime = Self {
            client,
            allowed_hosts: Arc::new(allowed_hosts),
            timeout,
            max_response_bytes: config.max_response_bytes,
            max_cache_entries: config.max_cache_entries,
            max_cache_entry_bytes: config.max_cache_entry_bytes,
            max_cache_bytes: config.max_cache_bytes,
            max_cache_ttl: Duration::from_secs(config.max_cache_ttl_seconds),
            cache: Arc::new(AsyncMutex::new(CacheState::default())),
            jwt: Arc::new(AsyncMutex::new(HashMap::new())),
            verifier_policies: Arc::new(config.jwt_verifiers.clone()),
            cache_encryption_key: Arc::new(Zeroizing::new(cache_encryption_key)),
        };
        for (id, policy) in runtime.verifier_policies.iter() {
            if id.is_empty()
                || policy.issuer.is_empty()
                || policy.audiences.is_empty()
                || policy.refresh_interval_seconds == 0
                || policy.max_stale_seconds < policy.refresh_interval_seconds
            {
                return Err(format!("JWT verifier {id} has invalid limits or claims"));
            }
            let algorithms = parse_algorithms(&policy.allowed_algorithms)?;
            if algorithms.iter().any(|algorithm| {
                matches!(
                    algorithm,
                    Algorithm::HS256 | Algorithm::HS384 | Algorithm::HS512
                )
            }) {
                return Err(format!(
                    "JWT verifier {id} cannot use symmetric algorithms with remote public keys"
                ));
            }
            let source_url = match &policy.key_source {
                PluginJwtKeySourceConfig::Jwks { url }
                | PluginJwtKeySourceConfig::OidcDiscovery { url }
                | PluginJwtKeySourceConfig::Certificate { url } => url,
            };
            runtime.checked_url(source_url)?;
        }
        Ok(runtime)
    }

    fn checked_url(&self, raw: &str) -> Result<Url, String> {
        let url = Url::parse(raw).map_err(|_| "invalid service URL".to_string())?;
        if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
            return Err("plugin egress requires an HTTPS URL without user information".to_string());
        }
        let host = url
            .host_str()
            .ok_or_else(|| "plugin egress URL has no host".to_string())?
            .to_ascii_lowercase();
        if !self.allowed_hosts.iter().any(|allowed| allowed == &host) {
            return Err("plugin egress host is not allowlisted".to_string());
        }
        Ok(url)
    }

    async fn json_response(&self, response: reqwest::Response) -> Result<Value, String> {
        let bytes = self.byte_response(response).await?;
        serde_json::from_slice(&bytes).map_err(|_| "remote response is not valid JSON".to_string())
    }

    async fn byte_response(&self, response: reqwest::Response) -> Result<Vec<u8>, String> {
        if !response.status().is_success() {
            return Err(format!(
                "remote service returned HTTP {}",
                response.status().as_u16()
            ));
        }
        if response
            .content_length()
            .is_some_and(|length| length > self.max_response_bytes as u64)
        {
            return Err("remote response exceeds the configured limit".to_string());
        }
        let mut bytes = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| "failed to read remote response".to_string())?;
            if bytes.len().saturating_add(chunk.len()) > self.max_response_bytes {
                return Err("remote response exceeds the configured limit".to_string());
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }

    pub(crate) async fn exchange(&self, vault: CredentialVault, input: &str) -> String {
        let result = async {
            let options: ExchangeOptions =
                serde_json::from_str(input).map_err(|_| "invalid exchange options".to_string())?;
            let url = self.checked_url(&options.url)?;
            let source = vault
                .resolve(&options.credential)
                .ok_or_else(|| "unknown credential handle".to_string())?;
            let mut body = options.body.as_object().cloned().unwrap_or_default();
            if options.credential_field.is_empty() {
                return Err("credential_field must not be empty".to_string());
            }
            body.insert(
                options.credential_field,
                Value::String(source.secret.clone()),
            );

            let mut request = self.client.post(url).json(&body);
            for (name, value) in options.headers {
                let lower = name.to_ascii_lowercase();
                if !matches!(lower.as_str(), "accept" | "content-type" | "user-agent") {
                    return Err("exchange requested a disallowed header".to_string());
                }
                request = request.header(&name, &value);
            }
            let response = request
                .send()
                .await
                .map_err(|_| "identity service is unavailable".to_string())?;
            let payload = self.json_response(response).await?;
            let token = payload
                .get(&options.token_field)
                .and_then(Value::as_str)
                .ok_or_else(|| "identity response has no credential field".to_string())?;
            if token.len() > self.max_cache_entry_bytes {
                return Err("exchanged credential exceeds the configured limit".to_string());
            }
            let now = unix_seconds();
            let response_expiry = payload
                .get(&options.expires_in_field)
                .and_then(Value::as_u64)
                .map(|seconds| now.saturating_add(seconds));
            let jwt_expiry = jwt_expiry_unverified(token);
            let expires_at = match (response_expiry, jwt_expiry) {
                (Some(left), Some(right)) => Some(left.min(right)),
                (left, right) => left.or(right),
            };
            let mut metadata = Map::new();
            for field in options.metadata_fields {
                if field != options.token_field {
                    if let Some(value) = payload.get(&field) {
                        metadata.insert(field, value.clone());
                    }
                }
            }
            let safe_metadata = Value::Object(metadata.clone());
            let credential = vault.insert(token.to_string(), expires_at, safe_metadata.clone());
            Ok(json!({
                "credential": credential,
                "expires_at": expires_at,
                "metadata": safe_metadata,
            }))
        }
        .await;
        envelope(result)
    }

    pub(crate) async fn cache_begin(
        &self,
        namespace: &str,
        vault: CredentialVault,
        key: &str,
    ) -> String {
        let complete_key = namespaced_key(namespace, key);
        loop {
            let waiter = {
                let mut cache = self.cache.lock().await;
                cache.remove_expired();
                if let Some(entry) = cache.entries.get(&complete_key) {
                    let secret = match self.decrypt_cache_secret(&entry.encrypted_secret) {
                        Ok(secret) => secret,
                        Err(error) => return envelope(Err(error)),
                    };
                    let handle =
                        vault.insert(secret, Some(entry.expires_at), entry.metadata.clone());
                    return envelope(Ok(json!({
                        "hit": true,
                        "credential": handle,
                        "expires_at": entry.expires_at,
                        "metadata": entry.metadata,
                    })));
                }
                if let Some(flight) = cache.flights.get(&complete_key) {
                    if flight.created_at.elapsed() >= self.timeout {
                        let flight = cache.flights.remove(&complete_key).expect("flight exists");
                        flight.changed.send_replace(true);
                        None
                    } else {
                        Some(flight.changed.subscribe())
                    }
                } else {
                    let lease = random_handle();
                    cache.flights.insert(
                        complete_key.clone(),
                        CacheFlight {
                            lease: lease.clone(),
                            changed: watch::channel(false).0,
                            created_at: Instant::now(),
                        },
                    );
                    return envelope(Ok(json!({"hit": false, "lease": lease})));
                }
            };
            if let Some(waiter) = waiter {
                if tokio::time::timeout(self.timeout, async move {
                    let mut waiter = waiter;
                    let _ = waiter.changed().await;
                })
                .await
                .is_err()
                {
                    return envelope(Err("cache loader timed out".to_string()));
                }
            }
        }
    }

    pub(crate) async fn cache_complete(
        &self,
        namespace: &str,
        vault: CredentialVault,
        input: &str,
    ) -> String {
        let result = async {
            let options: CacheCompleteOptions = serde_json::from_str(input)
                .map_err(|_| "invalid cache completion options".to_string())?;
            let complete_key = namespaced_key(namespace, &options.key);
            let credential = vault
                .resolve(&options.credential)
                .ok_or_else(|| "unknown credential handle".to_string())?;
            if credential.secret.len() > self.max_cache_entry_bytes {
                return Err("credential exceeds the configured cache entry limit".to_string());
            }
            let mut cache = self.cache.lock().await;
            let flight = cache
                .flights
                .remove(&complete_key)
                .ok_or_else(|| "cache lease no longer exists".to_string())?;
            if flight.lease != options.lease {
                cache.flights.insert(complete_key, flight);
                return Err("cache lease does not match".to_string());
            }
            let now = unix_seconds();
            let requested_expiry = now.saturating_add(options.ttl_seconds);
            let maximum_expiry = now.saturating_add(self.max_cache_ttl.as_secs());
            let expires_at = credential
                .expires_at
                .unwrap_or(requested_expiry)
                .min(requested_expiry)
                .min(maximum_expiry);
            if expires_at <= now {
                flight.changed.send_replace(true);
                return Err("credential is already expired".to_string());
            }
            let encrypted_secret = self.encrypt_cache_secret(credential.secret.as_bytes())?;
            let entry_size = encrypted_secret.len()
                + serde_json::to_vec(&credential.metadata)
                    .map_err(|_| "credential metadata is not serializable".to_string())?
                    .len()
                + options.tags.iter().map(String::len).sum::<usize>();
            if entry_size > self.max_cache_bytes {
                flight.changed.send_replace(true);
                return Err("credential cache entry exceeds the total cache limit".to_string());
            }
            while cache.entries.len() >= self.max_cache_entries
                || cache.total_bytes().saturating_add(entry_size) > self.max_cache_bytes
            {
                if let Some(oldest) = cache
                    .entries
                    .iter()
                    .min_by_key(|(_, entry)| entry.inserted_at)
                    .map(|(key, _)| key.clone())
                {
                    cache.entries.remove(&oldest);
                } else {
                    break;
                }
            }
            cache.entries.insert(
                complete_key,
                CacheEntry {
                    encrypted_secret,
                    metadata: credential.metadata.clone(),
                    expires_at,
                    inserted_at: Instant::now(),
                    tags: options.tags,
                    size_bytes: entry_size,
                },
            );
            flight.changed.send_replace(true);
            Ok(json!({"stored": true, "expires_at": expires_at}))
        }
        .await;
        envelope(result)
    }

    fn encrypt_cache_secret(&self, secret: &[u8]) -> Result<Vec<u8>, String> {
        let cipher = Aes256Gcm::new_from_slice(self.cache_encryption_key.as_slice())
            .map_err(|_| "cannot initialize cache encryption".to_string())?;
        let mut nonce = [0_u8; 12];
        rand::thread_rng().fill_bytes(&mut nonce);
        let ciphertext = cipher
            .encrypt(Nonce::from_slice(&nonce), secret)
            .map_err(|_| "cannot encrypt cached credential".to_string())?;
        let mut encrypted = Vec::with_capacity(nonce.len() + ciphertext.len());
        encrypted.extend_from_slice(&nonce);
        encrypted.extend_from_slice(&ciphertext);
        Ok(encrypted)
    }

    fn decrypt_cache_secret(&self, encrypted: &[u8]) -> Result<String, String> {
        if encrypted.len() <= 12 {
            return Err("cached credential is corrupt".to_string());
        }
        let cipher = Aes256Gcm::new_from_slice(self.cache_encryption_key.as_slice())
            .map_err(|_| "cannot initialize cache encryption".to_string())?;
        let plaintext = cipher
            .decrypt(Nonce::from_slice(&encrypted[..12]), &encrypted[12..])
            .map_err(|_| "cached credential cannot be decrypted".to_string())?;
        String::from_utf8(plaintext).map_err(|_| "cached credential is not UTF-8".to_string())
    }

    pub(crate) async fn cache_abort(&self, namespace: &str, input: &str) -> String {
        let result = async {
            let options: CacheLeaseOptions = serde_json::from_str(input)
                .map_err(|_| "invalid cache lease options".to_string())?;
            let complete_key = namespaced_key(namespace, &options.key);
            let mut cache = self.cache.lock().await;
            if let Some(flight) = cache.flights.get(&complete_key) {
                if flight.lease != options.lease {
                    return Err("cache lease does not match".to_string());
                }
            }
            if let Some(flight) = cache.flights.remove(&complete_key) {
                flight.changed.send_replace(true);
            }
            Ok(json!({"aborted": true}))
        }
        .await;
        envelope(result)
    }

    pub(crate) async fn cache_delete(&self, namespace: &str, key: &str) -> String {
        let removed = self
            .cache
            .lock()
            .await
            .entries
            .remove(&namespaced_key(namespace, key))
            .is_some();
        envelope(Ok(json!({"deleted": removed})))
    }

    pub(crate) async fn cache_invalidate_tag(&self, namespace: &str, tag: &str) -> String {
        let prefix = format!("{namespace}\u{1f}");
        let mut cache = self.cache.lock().await;
        let before = cache.entries.len();
        cache.entries.retain(|key, entry| {
            !key.starts_with(&prefix) || !entry.tags.iter().any(|candidate| candidate == tag)
        });
        envelope(Ok(json!({"invalidated": before - cache.entries.len()})))
    }

    pub(crate) async fn verify_jwt(&self, vault: CredentialVault, input: &str) -> String {
        let result = async {
            let options: VerifyOptions =
                serde_json::from_str(input).map_err(|_| "invalid JWT options".to_string())?;
            let credential = vault
                .resolve(&options.credential)
                .ok_or_else(|| "unknown credential handle".to_string())?;
            let policy = self
                .verifier_policies
                .get(&options.policy)
                .ok_or_else(|| "unknown JWT verifier policy".to_string())?;
            let header = match decode_header(&credential.secret) {
                Ok(header) => header,
                Err(_) => return Ok(json!({"valid": false, "reason": "invalid_token"})),
            };
            let allowed = parse_algorithms(&policy.allowed_algorithms)?;
            if !allowed.contains(&header.alg) {
                return Ok(json!({"valid": false, "reason": "invalid_token"}));
            }
            let mut keys = self.keys_for(&options.policy, policy, false).await?;
            let mut decoding_key = decoding_key_for(
                &keys,
                header.kid.as_deref(),
                policy.allow_missing_kid,
                header.alg,
            );
            if decoding_key.is_err() && header.kid.is_some() {
                keys = self.keys_for(&options.policy, policy, true).await?;
                decoding_key = decoding_key_for(
                    &keys,
                    header.kid.as_deref(),
                    policy.allow_missing_kid,
                    header.alg,
                );
            }
            let decoding_key = match decoding_key {
                Ok(key) => key,
                Err(_) => return Ok(json!({"valid": false, "reason": "invalid_token"})),
            };
            let mut validation = Validation::new(header.alg);
            validation.algorithms = allowed;
            validation.leeway = policy.clock_skew_seconds;
            validation.validate_nbf = true;
            validation.set_audience(&policy.audiences);
            validation.set_issuer(&[&policy.issuer]);
            validation.set_required_spec_claims(&["exp", "iss", "aud"]);
            let token = match decode::<Value>(&credential.secret, &decoding_key, &validation) {
                Ok(token) => token,
                Err(_) => return Ok(json!({"valid": false, "reason": "invalid_token"})),
            };
            Ok(json!({"valid": true, "claims": token.claims}))
        }
        .await;
        envelope(result)
    }

    pub(crate) fn preload_jwt_policies(&self) -> Result<(), String> {
        if self.verifier_policies.is_empty() {
            return Ok(());
        }
        let runtime = self.clone();
        std::thread::Builder::new()
            .name("bifrost-plugin-jwt-preload".to_string())
            .spawn(move || {
                let executor = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|error| error.to_string())?;
                executor.block_on(async {
                    for (id, policy) in runtime.verifier_policies.iter() {
                        runtime.keys_for(id, policy, true).await?;
                    }
                    Ok(())
                })
            })
            .map_err(|error| error.to_string())?
            .join()
            .map_err(|_| "JWT preload worker panicked".to_string())?
    }

    pub(crate) fn start_jwt_refresh(self: &Arc<Self>) -> Result<(), String> {
        if self.verifier_policies.is_empty() {
            return Ok(());
        }
        let weak = Arc::downgrade(self);
        let interval = self
            .verifier_policies
            .values()
            .map(|policy| policy.refresh_interval_seconds)
            .min()
            .unwrap_or(60)
            .clamp(1, 60);
        std::thread::Builder::new()
            .name("bifrost-plugin-jwt-refresh".to_string())
            .spawn(move || {
                let executor = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(executor) => executor,
                    Err(error) => {
                        log::error!("plugin JWT refresh runtime failed: {error}");
                        return;
                    }
                };
                loop {
                    std::thread::sleep(Duration::from_secs(interval));
                    let Some(runtime) = weak.upgrade() else {
                        return;
                    };
                    executor.block_on(async {
                        for (id, policy) in runtime.verifier_policies.iter() {
                            if let Err(error) = runtime.keys_for(id, policy, false).await {
                                log::warn!("plugin JWT verifier {id} refresh failed: {error}");
                            }
                        }
                    });
                }
            })
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    async fn keys_for(
        &self,
        id: &str,
        policy: &PluginJwtVerifierConfig,
        force: bool,
    ) -> Result<VerificationKeys, String> {
        // Hold the per-runtime verifier lock through refresh. Key downloads are
        // rare, bounded operations, and serialization gives every policy a
        // simple single-flight guarantee without exposing attacker-controlled
        // `kid` values as concurrent fetches.
        let mut state = self.jwt.lock().await;
        if let Some(cached) = state.get(id) {
            let age = cached.fetched_at.elapsed();
            let unknown_kid_cooldown = Duration::from_secs(30);
            if (!force && age < Duration::from_secs(policy.refresh_interval_seconds))
                || (force && age < unknown_kid_cooldown)
            {
                return Ok(cached.keys.clone());
            }
        }
        match self.fetch_keys(policy).await {
            Ok(keys) => {
                state.insert(
                    id.to_string(),
                    CachedKeys {
                        keys: keys.clone(),
                        fetched_at: Instant::now(),
                    },
                );
                Ok(keys)
            }
            Err(error) => {
                if let Some(cached) = state.get(id) {
                    if cached.fetched_at.elapsed() <= Duration::from_secs(policy.max_stale_seconds)
                    {
                        return Ok(cached.keys.clone());
                    }
                }
                Err(error)
            }
        }
    }

    async fn fetch_keys(
        &self,
        policy: &PluginJwtVerifierConfig,
    ) -> Result<VerificationKeys, String> {
        if let PluginJwtKeySourceConfig::Certificate { url } = &policy.key_source {
            let response = self
                .client
                .get(self.checked_url(url)?)
                .send()
                .await
                .map_err(|_| "JWT certificate service is unavailable".to_string())?;
            let bytes = self.byte_response(response).await?;
            let pem = if bytes.starts_with(b"-----BEGIN CERTIFICATE-----") {
                bytes
            } else {
                certificate_pem(&bytes).into_bytes()
            };
            return Ok(VerificationKeys::Certificate(Arc::new(pem)));
        }
        let jwks_url = match &policy.key_source {
            PluginJwtKeySourceConfig::Jwks { url } => self.checked_url(url)?,
            PluginJwtKeySourceConfig::OidcDiscovery { url } => {
                let discovery = self.checked_url(url)?;
                let response = self
                    .client
                    .get(discovery)
                    .send()
                    .await
                    .map_err(|_| "OIDC discovery service is unavailable".to_string())?;
                let metadata = self.json_response(response).await?;
                let uri = metadata
                    .get("jwks_uri")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "OIDC metadata has no jwks_uri".to_string())?;
                self.checked_url(uri)?
            }
            PluginJwtKeySourceConfig::Certificate { .. } => unreachable!(),
        };
        let response = self
            .client
            .get(jwks_url)
            .send()
            .await
            .map_err(|_| "JWT key service is unavailable".to_string())?;
        let value = self.json_response(response).await?;
        let set: JwkSet = serde_json::from_value(value)
            .map_err(|_| "JWT key service returned an invalid JWKS".to_string())?;
        if set.keys.is_empty() {
            return Err("JWT key set is empty".to_string());
        }
        Ok(VerificationKeys::Jwks(set))
    }
}

#[derive(Clone, Default)]
pub(crate) struct CredentialVault(Arc<Mutex<HashMap<String, Credential>>>);

impl CredentialVault {
    pub(crate) fn insert_source(&self, secret: &[u8]) -> String {
        self.insert(
            String::from_utf8_lossy(secret).into_owned(),
            None,
            Value::Null,
        )
    }

    fn insert(&self, secret: String, expires_at: Option<u64>, metadata: Value) -> String {
        let handle = random_handle();
        self.0.lock().expect("credential vault lock").insert(
            handle.clone(),
            Credential {
                secret,
                expires_at,
                metadata,
            },
        );
        handle
    }

    pub(crate) fn bearer(&self, handle: &str) -> Option<String> {
        self.resolve(handle)
            .map(|credential| credential.secret.clone())
    }

    fn resolve(&self, handle: &str) -> Option<Credential> {
        self.0
            .lock()
            .expect("credential vault lock")
            .get(handle)
            .cloned()
    }
}

#[derive(Clone)]
struct Credential {
    secret: String,
    expires_at: Option<u64>,
    metadata: Value,
}

impl Drop for Credential {
    fn drop(&mut self) {
        self.secret.zeroize();
    }
}

#[derive(Default)]
struct CacheState {
    entries: HashMap<String, CacheEntry>,
    flights: HashMap<String, CacheFlight>,
}

impl CacheState {
    fn remove_expired(&mut self) {
        let now = unix_seconds();
        self.entries.retain(|_, entry| entry.expires_at > now);
    }

    fn total_bytes(&self) -> usize {
        self.entries.values().map(|entry| entry.size_bytes).sum()
    }
}

struct CacheEntry {
    encrypted_secret: Vec<u8>,
    metadata: Value,
    expires_at: u64,
    inserted_at: Instant,
    tags: Vec<String>,
    size_bytes: usize,
}

impl Drop for CacheEntry {
    fn drop(&mut self) {
        self.encrypted_secret.zeroize();
    }
}

struct CacheFlight {
    lease: String,
    changed: watch::Sender<bool>,
    created_at: Instant,
}

#[derive(Clone)]
enum VerificationKeys {
    Jwks(JwkSet),
    Certificate(Arc<Vec<u8>>),
}

struct CachedKeys {
    keys: VerificationKeys,
    fetched_at: Instant,
}

#[derive(Deserialize)]
struct ExchangeOptions {
    url: String,
    credential: String,
    credential_field: String,
    #[serde(default)]
    body: Value,
    #[serde(default)]
    headers: HashMap<String, String>,
    #[serde(default = "default_token_field")]
    token_field: String,
    #[serde(default = "default_expires_in_field")]
    expires_in_field: String,
    #[serde(default)]
    metadata_fields: Vec<String>,
}

fn default_token_field() -> String {
    "access_token".to_string()
}

fn default_expires_in_field() -> String {
    "expires_in".to_string()
}

#[derive(Deserialize)]
struct CacheCompleteOptions {
    key: String,
    lease: String,
    credential: String,
    ttl_seconds: u64,
    #[serde(default)]
    tags: Vec<String>,
}

#[derive(Deserialize)]
struct CacheLeaseOptions {
    key: String,
    lease: String,
}

#[derive(Deserialize)]
struct VerifyOptions {
    policy: String,
    credential: String,
}

fn select_key<'a>(
    set: &'a JwkSet,
    kid: Option<&str>,
    allow_missing_kid: bool,
    algorithm: Algorithm,
) -> Result<&'a Jwk, String> {
    let eligible: Vec<&Jwk> = set
        .keys
        .iter()
        .filter(|key| {
            strong_public_key(key)
                && matches!(
                    key.common.public_key_use,
                    None | Some(PublicKeyUse::Signature)
                )
                && key
                    .common
                    .key_operations
                    .as_ref()
                    .is_none_or(|operations| operations.contains(&KeyOperations::Verify))
                && key
                    .common
                    .key_algorithm
                    .is_none_or(|declared| declared.to_string() == format!("{algorithm:?}"))
        })
        .collect();
    match kid {
        Some(kid) => {
            let matches: Vec<_> = eligible
                .into_iter()
                .filter(|key| key.common.key_id.as_deref() == Some(kid))
                .collect();
            if matches.len() == 1 {
                Ok(matches[0])
            } else {
                Err("JWT key id is unknown or ambiguous".to_string())
            }
        }
        None if allow_missing_kid && eligible.len() == 1 => Ok(eligible[0]),
        None => Err("JWT without a key id is ambiguous".to_string()),
    }
}

fn decoding_key_for(
    keys: &VerificationKeys,
    kid: Option<&str>,
    allow_missing_kid: bool,
    algorithm: Algorithm,
) -> Result<DecodingKey, String> {
    match keys {
        VerificationKeys::Jwks(set) => {
            DecodingKey::from_jwk(select_key(set, kid, allow_missing_kid, algorithm)?)
                .map_err(|_| "JWT key is not usable".to_string())
        }
        VerificationKeys::Certificate(pem) => {
            if kid.is_none() && !allow_missing_kid {
                return Err("JWT without a key id is not allowed".to_string());
            }
            match algorithm {
                Algorithm::RS256
                | Algorithm::RS384
                | Algorithm::RS512
                | Algorithm::PS256
                | Algorithm::PS384
                | Algorithm::PS512 => DecodingKey::from_rsa_pem(pem),
                Algorithm::ES256 | Algorithm::ES384 => DecodingKey::from_ec_pem(pem),
                Algorithm::EdDSA => DecodingKey::from_ed_pem(pem),
                Algorithm::HS256 | Algorithm::HS384 | Algorithm::HS512 => {
                    return Err("symmetric JWT algorithms are not supported".to_string());
                }
            }
            .map_err(|_| "JWT certificate is not usable".to_string())
        }
    }
}

fn certificate_pem(der: &[u8]) -> String {
    let encoded = STANDARD.encode(der);
    let mut pem = String::from("-----BEGIN CERTIFICATE-----\n");
    for chunk in encoded.as_bytes().chunks(64) {
        pem.push_str(std::str::from_utf8(chunk).expect("base64 is UTF-8"));
        pem.push('\n');
    }
    pem.push_str("-----END CERTIFICATE-----\n");
    pem
}

fn strong_public_key(key: &Jwk) -> bool {
    match &key.algorithm {
        AlgorithmParameters::RSA(parameters) => {
            URL_SAFE_NO_PAD.decode(&parameters.n).is_ok_and(|modulus| {
                modulus.first().is_some_and(|first| {
                    (modulus.len().saturating_sub(1) * 8) + (8 - first.leading_zeros() as usize)
                        >= 2048
                })
            })
        }
        AlgorithmParameters::EllipticCurve(_) | AlgorithmParameters::OctetKeyPair(_) => true,
        AlgorithmParameters::OctetKey(_) => false,
    }
}

fn parse_algorithms(names: &[String]) -> Result<Vec<Algorithm>, String> {
    if names.is_empty() {
        return Err("JWT verifier has no allowed algorithms".to_string());
    }
    names
        .iter()
        .map(|name| Algorithm::from_str(name).map_err(|_| "unsupported JWT algorithm".to_string()))
        .collect()
}

fn random_handle() -> String {
    let mut bytes = [0_u8; 24];
    rand::thread_rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn namespaced_key(namespace: &str, key: &str) -> String {
    let digest = Sha256::digest(key.as_bytes());
    format!("{namespace}\u{1f}{}", URL_SAFE_NO_PAD.encode(digest))
}

fn envelope(result: Result<Value, String>) -> String {
    match result {
        Ok(value) => json!({"ok": true, "value": value}).to_string(),
        Err(error) => json!({"ok": false, "error": error}).to_string(),
    }
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn jwt_expiry_unverified(token: &str) -> Option<u64> {
    let encoded = token.split('.').nth(1)?;
    let decoded = URL_SAFE_NO_PAD.decode(encoded).ok()?;
    serde_json::from_slice::<Value>(&decoded)
        .ok()?
        .get("exp")?
        .as_u64()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cache_waiter_observes_value_published_during_wait_registration() {
        let host = HostRuntime::new(&PluginRuntimeConfig::default()).unwrap();
        let vault = CredentialVault::default();
        let first: Value = serde_json::from_str(
            &&host
                .cache_begin("route:plugin", vault.clone(), "semantic-key")
                .await,
        )
        .unwrap();
        let lease = first["value"]["lease"].as_str().unwrap().to_string();
        let waiter_host = host.clone();
        let waiter_vault = vault.clone();
        let waiter = tokio::spawn(async move {
            waiter_host
                .cache_begin("route:plugin", waiter_vault, "semantic-key")
                .await
        });
        tokio::task::yield_now().await;

        let credential = vault.insert_source(b"short-lived-jwt");
        let completed: Value = serde_json::from_str(
            &host
                .cache_complete(
                    "route:plugin",
                    vault,
                    &json!({
                        "key": "semantic-key",
                        "lease": lease,
                        "credential": credential,
                        "ttl_seconds": 30,
                        "tags": ["session"]
                    })
                    .to_string(),
                )
                .await,
        )
        .unwrap();
        assert_eq!(completed["ok"], true);

        let published: Value = serde_json::from_str(
            &tokio::time::timeout(Duration::from_secs(1), waiter)
                .await
                .expect("cache waiter should wake")
                .unwrap(),
        )
        .unwrap();
        assert_eq!(published["ok"], true);
        assert_eq!(published["value"]["hit"], true);
    }
}
