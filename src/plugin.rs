//! Signed, capability-gated QuickJS access plugins.
//!
//! This module deliberately exposes a small capability-gated PDK. JavaScript never
//! receives a raw value for a header that its own attachment declares as a
//! credential, nor does it receive filesystem handles, sockets, or process
//! APIs. The service owner may deliberately grant another plugin access to
//! that header through its `permitted_headers`; plugins on one route are an
//! explicitly configured, trusted composition. Networked identity exchange,
//! sensitive caching, and JWT verification execute in native host services;
//! JavaScript receives promises and opaque credential handles.

use crate::config::{PluginRuntimeConfig, RoutePluginConfig};
use crate::plugin_host::{CredentialVault, HostRuntime};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use hmac::{Hmac, Mac};
use http::{HeaderMap, HeaderName, HeaderValue, Request, StatusCode};
use jsonschema::validator_for;
use rquickjs::{
    AsyncContext, AsyncRuntime, Promise, async_with,
    function::{Async, Func},
};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

const RESERVED_HEADERS: &[&str] = &[
    "authorization",
    "x-forwarded-for",
    "x-forwarded-host",
    "x-forwarded-proto",
    "x-bifrost-route",
    "x-bifrost-client-ip",
];

#[derive(Debug, Error)]
pub enum PluginError {
    #[error("plugin configuration error: {0}")]
    Config(String),
    #[error("plugin package error: {0}")]
    Package(String),
    #[error("plugin execution error: {0}")]
    Execution(String),
}

#[derive(Debug, Deserialize)]
struct Manifest {
    id: String,
    version: String,
    runtime: String,
    entrypoint: String,
    phases: Vec<String>,
    capabilities: Vec<String>,
    configuration_schema: String,
    publisher: String,
    /// Base64-encoded SHA-256 digests indexed by package-relative payload
    /// path. The signed manifest must cover every payload this runtime reads.
    #[serde(default)]
    files: HashMap<String, String>,
}

#[derive(Clone)]
struct LoadedPlugin {
    package: String,
    route_id: String,
    namespace: String,
    phases: HashSet<String>,
    capabilities: HashSet<String>,
    code: Arc<String>,
    attachment: RoutePluginConfig,
}

/// Compiled plugins for one route. The source and configuration are immutable
/// after configuration loading, making reloads naturally isolate namespaces.
#[derive(Clone, Default)]
pub struct PluginChain {
    plugins: Vec<LoadedPlugin>,
    fingerprint_key: [u8; 32],
    memory_limit_bytes: usize,
    execution_timeout_millis: u64,
    host: Option<Arc<HostRuntime>>,
}

impl PluginChain {
    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    /// Runs ordered access plugins and applies their approved header plan.
    /// A denied or malformed result fails closed.
    pub fn run_access<B>(&self, request: &mut Request<B>) -> Result<PluginOutcome, PluginError> {
        let (outcome, headers) = self.run_access_headers(
            request.headers().clone(),
            request.method().as_str(),
            request.uri().path(),
        )?;
        if matches!(outcome, PluginOutcome::Allow) {
            *request.headers_mut() = headers;
        }
        Ok(outcome)
    }

    /// Evaluates access plugins using an owned header map. This makes the
    /// synchronous QuickJS portion safe to run on a dedicated worker thread;
    /// callers apply the returned headers only after an allow outcome.
    pub fn run_access_headers(
        &self,
        headers: HeaderMap,
        method: &str,
        path: &str,
    ) -> Result<(PluginOutcome, HeaderMap), PluginError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| PluginError::Execution(error.to_string()))?;
        runtime.block_on(self.run_access_headers_async(headers, method, path))
    }

    pub fn run_access_headers_with_cancel(
        &self,
        headers: HeaderMap,
        method: &str,
        path: &str,
        cancellation: CancellationToken,
    ) -> Result<(PluginOutcome, HeaderMap), PluginError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| PluginError::Execution(error.to_string()))?;
        runtime.block_on(async {
            tokio::select! {
                _ = cancellation.cancelled() => Err(PluginError::Execution("plugin request was cancelled".to_string())),
                result = self.run_access_headers_async(headers, method, path) => result,
            }
        })
    }

    /// Async implementation used by the dedicated plugin workers. QuickJS
    /// promises may suspend while native host services perform bounded I/O.
    pub async fn run_access_headers_async(
        &self,
        mut headers: HeaderMap,
        method: &str,
        path: &str,
    ) -> Result<(PluginOutcome, HeaderMap), PluginError> {
        // Remove an untrusted inbound bearer before any plugin plan is applied.
        // A later `upstream.authorization` plan may install a new value from an
        // opaque credential handle.
        headers.remove(http::header::AUTHORIZATION);
        for phase in ["ingress", "access", "upstream"] {
            for plugin in self
                .plugins
                .iter()
                .filter(|plugin| plugin.phases.contains(phase))
            {
                let (input, vault) = self.input_for(plugin, &headers, method, path);
                let result = self.run_one(plugin, phase, input, vault.clone()).await?;
                match result.outcome.as_str() {
                    "allow" => self.apply_upstream_plan(plugin, &mut headers, &result, &vault)?,
                    "deny" if phase == "access" => {
                        let status = result.status.unwrap_or(403);
                        let status = StatusCode::from_u16(status).unwrap_or(StatusCode::FORBIDDEN);
                        if !status.is_client_error() {
                            return Err(PluginError::Execution(format!(
                                "{} returned an invalid deny status",
                                plugin.package
                            )));
                        }
                        let code = result.code.unwrap_or_else(|| "access_denied".to_string());
                        if !is_safe_public_code(&code) {
                            return Err(PluginError::Execution(format!(
                                "{} returned an invalid public error code",
                                plugin.package
                            )));
                        }
                        return Ok((PluginOutcome::Deny { status, code }, headers));
                    }
                    "error" => return Ok((PluginOutcome::Error, headers)),
                    _ => {
                        return Err(PluginError::Execution(format!(
                            "{} returned an invalid {phase} outcome",
                            plugin.package
                        )));
                    }
                }
            }
        }
        // Credential and gateway-internal request headers are never forwarded
        // after a plugin has consumed them. This also prevents an externally
        // supplied forwarded-header value from becoming an upstream fact.
        for reserved in RESERVED_HEADERS {
            if *reserved != "authorization" {
                headers.remove(*reserved);
            }
        }
        for plugin in &self.plugins {
            for header in plugin.attachment.credential_headers.values() {
                headers.remove(header);
            }
        }
        Ok((PluginOutcome::Allow, headers))
    }

    pub fn run_response_headers_with_cancel(
        &self,
        headers: HeaderMap,
        status: StatusCode,
        method: &str,
        path: &str,
        cancellation: CancellationToken,
    ) -> Result<HeaderMap, PluginError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| PluginError::Execution(error.to_string()))?;
        runtime.block_on(async {
            tokio::select! {
                _ = cancellation.cancelled() => Err(PluginError::Execution("plugin request was cancelled".to_string())),
                result = self.run_response_headers_async(headers, status, method, path) => result,
            }
        })
    }

    async fn run_response_headers_async(
        &self,
        mut headers: HeaderMap,
        status: StatusCode,
        method: &str,
        path: &str,
    ) -> Result<HeaderMap, PluginError> {
        for plugin in self
            .plugins
            .iter()
            .filter(|plugin| plugin.phases.contains("response"))
        {
            let input = self.response_input_for(plugin, &headers, status, method, path);
            let result = self
                .run_one(plugin, "response", input, CredentialVault::default())
                .await?;
            if result.outcome != "allow" {
                return Err(PluginError::Execution(format!(
                    "{} returned an invalid response outcome",
                    plugin.package
                )));
            }
            self.apply_response_plan(plugin, &mut headers, &result)?;
        }
        Ok(headers)
    }

    pub fn run_log_with_cancel(
        &self,
        headers: HeaderMap,
        status: StatusCode,
        method: &str,
        path: &str,
        cancellation: CancellationToken,
    ) -> Result<(), PluginError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| PluginError::Execution(error.to_string()))?;
        runtime.block_on(async {
            tokio::select! {
                _ = cancellation.cancelled() => Err(PluginError::Execution("plugin log was cancelled".to_string())),
                result = self.run_log_async(headers, status, method, path) => result,
            }
        })
    }

    async fn run_log_async(
        &self,
        headers: HeaderMap,
        status: StatusCode,
        method: &str,
        path: &str,
    ) -> Result<(), PluginError> {
        for plugin in self
            .plugins
            .iter()
            .filter(|plugin| plugin.phases.contains("log"))
        {
            let input = self.response_input_for(plugin, &headers, status, method, path);
            let result = self
                .run_one(plugin, "log", input, CredentialVault::default())
                .await?;
            if result.outcome != "allow" {
                return Err(PluginError::Execution(format!(
                    "{} returned an invalid log outcome",
                    plugin.package
                )));
            }
        }
        Ok(())
    }

    fn response_input_for(
        &self,
        plugin: &LoadedPlugin,
        headers: &HeaderMap,
        status: StatusCode,
        method: &str,
        path: &str,
    ) -> Value {
        let safe_headers: serde_json::Map<String, Value> = headers
            .iter()
            .filter(|(name, _)| {
                !matches!(
                    name.as_str(),
                    "set-cookie" | "www-authenticate" | "proxy-authenticate"
                )
            })
            .filter_map(|(name, value)| {
                value
                    .to_str()
                    .ok()
                    .map(|value| (name.as_str().to_string(), Value::String(value.to_string())))
            })
            .collect();
        json!({
            "request": {"method": method, "path": path, "headers": {}},
            "response": {"status": status.as_u16(), "headers": safe_headers},
            "credentials": {"fingerprints": {}, "handles": {}},
            "config": plugin.attachment.config,
        })
    }

    fn input_for(
        &self,
        plugin: &LoadedPlugin,
        headers: &HeaderMap,
        method: &str,
        path: &str,
    ) -> (Value, CredentialVault) {
        let credential_headers: HashSet<String> = plugin
            .attachment
            .credential_headers
            .values()
            .map(|name| name.to_ascii_lowercase())
            .collect();
        let permitted: HashSet<String> = plugin
            .attachment
            .permitted_headers
            .iter()
            .map(|name| name.to_ascii_lowercase())
            .collect();

        let mut safe_headers = serde_json::Map::new();
        for (name, value) in headers {
            let name = name.as_str().to_ascii_lowercase();
            if permitted.contains(&name)
                && !credential_headers.contains(&name)
                && !RESERVED_HEADERS.contains(&name.as_str())
            {
                if let Ok(value) = value.to_str() {
                    safe_headers.insert(name, Value::String(value.to_string()));
                }
            }
        }

        let mut fingerprints = serde_json::Map::new();
        let mut handles = serde_json::Map::new();
        let vault = CredentialVault::default();
        for (credential, header) in &plugin.attachment.credential_headers {
            if let Some(value) = headers.get(header) {
                let mut mac = Hmac::<Sha256>::new_from_slice(&self.fingerprint_key)
                    .expect("HMAC accepts a 32 byte key");
                mac.update(value.as_bytes());
                fingerprints.insert(
                    credential.clone(),
                    Value::String(STANDARD.encode(mac.finalize().into_bytes())),
                );
                handles.insert(
                    credential.clone(),
                    Value::String(vault.insert_source(value.as_bytes())),
                );
            }
        }

        (
            json!({
                "request": { "method": method, "path": path, "headers": safe_headers },
                "credentials": { "fingerprints": fingerprints, "handles": handles },
                "config": plugin.attachment.config,
            }),
            vault,
        )
    }

    async fn run_one(
        &self,
        plugin: &LoadedPlugin,
        phase: &str,
        input: Value,
        vault: CredentialVault,
    ) -> Result<PluginResult, PluginError> {
        // QuickJS is created for every invocation, so globals cannot leak from
        // one request or tenant to another. There is deliberately no module
        // loader, which rules out imports and filesystem access.
        let runtime = AsyncRuntime::new().map_err(|e| PluginError::Execution(e.to_string()))?;
        runtime.set_memory_limit(self.memory_limit_bytes).await;
        runtime
            .set_max_stack_size(self.memory_limit_bytes.min(1024 * 1024))
            .await;
        let cpu_window = std::time::Duration::from_millis(self.execution_timeout_millis);
        let deadline = Arc::new(Mutex::new(std::time::Instant::now() + cpu_window));
        let interrupt_deadline = deadline.clone();
        runtime
            .set_interrupt_handler(Some(Box::new(move || {
                std::time::Instant::now()
                    >= *interrupt_deadline.lock().expect("plugin deadline lock")
            })))
            .await;
        let context = AsyncContext::full(&runtime)
            .await
            .map_err(|e| PluginError::Execution(e.to_string()))?;
        let input =
            serde_json::to_string(&input).map_err(|e| PluginError::Execution(e.to_string()))?;
        let host = self.host.clone().ok_or_else(|| {
            PluginError::Execution("plugin host runtime is unavailable".to_string())
        })?;
        let namespace = plugin.namespace.clone();
        let capabilities = plugin.capabilities.clone();
        let audits = Arc::new(Mutex::new(Vec::<Value>::new()));
        let audits_output = audits.clone();
        let phase_json = serde_json::to_string(phase)
            .map_err(|error| PluginError::Execution(error.to_string()))?;
        let bootstrap = format!(
            "'use strict';\nconst module={{exports:{{}}}}; const exports=module.exports;\n{}\n\nconst __unwrap = async p => {{ const r=JSON.parse(await p); if(!r.ok) throw new Error(r.error); return r.value; }};\nconst __need = (name, fn) => {{ if(typeof fn !== 'function') throw new Error('missing capability: '+name); return fn; }};\n__bifrost_input.identity={{exchange: o => __unwrap(__need('identity.exchange',globalThis.__bifrost_exchange)(JSON.stringify(o)))}};\n__bifrost_input.jwt={{verify: (credential,policy) => __unwrap(__need('jwt.verify',globalThis.__bifrost_jwt_verify)(JSON.stringify({{credential,policy}})))}};\n__bifrost_input.cache={{\n getOrLoad: async (key,options,loader) => {{ const first=await __unwrap(__need('cache.sensitive',globalThis.__bifrost_cache_begin)(String(key))); if(first.hit) return first; try {{ const loaded=await loader(); await __unwrap(__bifrost_cache_complete(JSON.stringify({{key:String(key),lease:first.lease,credential:loaded.credential,ttl_seconds:options.ttl_seconds,tags:options.tags||[]}}))); return loaded; }} catch(e) {{ await __unwrap(__bifrost_cache_abort(JSON.stringify({{key:String(key),lease:first.lease}}))); throw e; }} }},\n delete: key => __unwrap(__need('cache.sensitive',globalThis.__bifrost_cache_delete)(String(key))),\n invalidateTag: tag => __unwrap(__need('cache.sensitive',globalThis.__bifrost_cache_invalidate_tag)(String(tag)))\n}};\n__bifrost_input.audit={{emit: event => {{ const emit=__need('audit.emit',globalThis.__bifrost_audit); if(!emit(JSON.stringify(event))) throw new Error('audit event rejected'); }}}};\nObject.freeze(__bifrost_input.request); Object.freeze(__bifrost_input.credentials); Object.freeze(__bifrost_input.config);\nconst __phase={}; const __candidate = globalThis[__phase] || module.exports[__phase] || exports[__phase];\nif (typeof __candidate !== 'function') throw new Error('plugin must export '+__phase+'(ctx)');\nPromise.resolve(__candidate(Object.freeze(__bifrost_input))).then(r => JSON.stringify(r == null ? {{outcome:'allow'}} : r));",
            plugin.code, phase_json
        );
        // JSON is generated by serde, never by string interpolation from a request.
        let bootstrap = format!("const __bifrost_input = {};\n{}", input, bootstrap);
        let encoded: Result<String, rquickjs::Error> = async_with!(context => |ctx| {
            let globals = ctx.globals();
            if capabilities.contains("audit.emit") {
                let audits = audits.clone();
                globals.set("__bifrost_audit", Func::from(move |input: String| {
                    if input.len() > 8192 {
                        return false;
                    }
                    let Ok(event) = serde_json::from_str::<Value>(&input) else {
                        return false;
                    };
                    if !safe_audit_event(&event) {
                        return false;
                    }
                    audits.lock().expect("plugin audit lock").push(event);
                    true
                }))?;
            }
            if capabilities.contains("identity.exchange") {
                let host = host.clone();
                let vault = vault.clone();
                let deadline = deadline.clone();
                globals.set("__bifrost_exchange", Func::from(Async(move |input: String| {
                    let host = host.clone();
                    let vault = vault.clone();
                    let deadline = deadline.clone();
                    async move {
                        let result = host.exchange(vault, &input).await;
                        *deadline.lock().expect("plugin deadline lock") = std::time::Instant::now() + cpu_window;
                        result
                    }
                })))?;
            }
            if capabilities.contains("jwt.verify") {
                let host = host.clone();
                let vault = vault.clone();
                let deadline = deadline.clone();
                globals.set("__bifrost_jwt_verify", Func::from(Async(move |input: String| {
                    let host = host.clone();
                    let vault = vault.clone();
                    let deadline = deadline.clone();
                    async move {
                        let result = host.verify_jwt(vault, &input).await;
                        *deadline.lock().expect("plugin deadline lock") = std::time::Instant::now() + cpu_window;
                        result
                    }
                })))?;
            }
            if capabilities.contains("cache.sensitive") {
                let host_begin = host.clone();
                let vault_begin = vault.clone();
                let namespace_begin = namespace.clone();
                let deadline_begin = deadline.clone();
                globals.set("__bifrost_cache_begin", Func::from(Async(move |key: String| {
                    let host = host_begin.clone(); let vault = vault_begin.clone(); let namespace = namespace_begin.clone(); let deadline = deadline_begin.clone();
                    async move { let result = host.cache_begin(&namespace, vault, &key).await; *deadline.lock().expect("plugin deadline lock") = std::time::Instant::now() + cpu_window; result }
                })))?;
                let host_complete = host.clone();
                let vault_complete = vault.clone();
                let namespace_complete = namespace.clone();
                let deadline_complete = deadline.clone();
                globals.set("__bifrost_cache_complete", Func::from(Async(move |input: String| {
                    let host = host_complete.clone(); let vault = vault_complete.clone(); let namespace = namespace_complete.clone(); let deadline = deadline_complete.clone();
                    async move { let result = host.cache_complete(&namespace, vault, &input).await; *deadline.lock().expect("plugin deadline lock") = std::time::Instant::now() + cpu_window; result }
                })))?;
                let host_abort = host.clone();
                let namespace_abort = namespace.clone();
                let deadline_abort = deadline.clone();
                globals.set("__bifrost_cache_abort", Func::from(Async(move |input: String| {
                    let host = host_abort.clone(); let namespace = namespace_abort.clone(); let deadline = deadline_abort.clone();
                    async move { let result = host.cache_abort(&namespace, &input).await; *deadline.lock().expect("plugin deadline lock") = std::time::Instant::now() + cpu_window; result }
                })))?;
                let host_delete = host.clone();
                let namespace_delete = namespace.clone();
                let deadline_delete = deadline.clone();
                globals.set("__bifrost_cache_delete", Func::from(Async(move |key: String| {
                    let host = host_delete.clone(); let namespace = namespace_delete.clone(); let deadline = deadline_delete.clone();
                    async move { let result = host.cache_delete(&namespace, &key).await; *deadline.lock().expect("plugin deadline lock") = std::time::Instant::now() + cpu_window; result }
                })))?;
                let host_invalidate = host.clone();
                let namespace_invalidate = namespace.clone();
                let deadline_invalidate = deadline.clone();
                globals.set("__bifrost_cache_invalidate_tag", Func::from(Async(move |tag: String| {
                    let host = host_invalidate.clone(); let namespace = namespace_invalidate.clone(); let deadline = deadline_invalidate.clone();
                    async move { let result = host.cache_invalidate_tag(&namespace, &tag).await; *deadline.lock().expect("plugin deadline lock") = std::time::Instant::now() + cpu_window; result }
                })))?;
            }
            let promise: Promise = ctx.eval(bootstrap)?;
            promise.into_future::<String>().await
        }).await;
        let encoded =
            encoded.map_err(|e| PluginError::Execution(format!("{}: {}", plugin.package, e)))?;
        for event in audits_output.lock().expect("plugin audit lock").iter() {
            log::info!(
                target: "bifrost_plugin_audit",
                "route={} package={} phase={} event={}",
                plugin.route_id,
                plugin.package,
                phase,
                event
            );
        }
        serde_json::from_str(&encoded).map_err(|e| {
            PluginError::Execution(format!("{} returned non-JSON data: {}", plugin.package, e))
        })
    }

    fn apply_upstream_plan(
        &self,
        plugin: &LoadedPlugin,
        headers: &mut HeaderMap,
        result: &PluginResult,
        vault: &CredentialVault,
    ) -> Result<(), PluginError> {
        let Some(plan) = result.upstream.as_ref() else {
            return Ok(());
        };
        if !plan.headers.is_empty() && !plugin.capabilities.contains("upstream.headers") {
            return Err(PluginError::Execution(format!(
                "{} attempted an upstream header plan without upstream.headers capability",
                plugin.package
            )));
        }
        for (name, value) in &plan.headers {
            let lower = name.to_ascii_lowercase();
            if RESERVED_HEADERS.contains(&lower.as_str())
                || lower == "host"
                || lower == "authorization"
            {
                return Err(PluginError::Execution(format!(
                    "{} attempted to set reserved header {}",
                    plugin.package, name
                )));
            }
            let name = HeaderName::from_bytes(name.as_bytes()).map_err(|_| {
                PluginError::Execution(format!("{} returned invalid header name", plugin.package))
            })?;
            let value = HeaderValue::from_str(value).map_err(|_| {
                PluginError::Execution(format!("{} returned invalid header value", plugin.package))
            })?;
            headers.insert(name, value);
        }
        if let Some(handle) = plan.bearer.as_deref() {
            if !plugin.capabilities.contains("upstream.authorization") {
                return Err(PluginError::Execution(format!(
                    "{} attempted bearer injection without upstream.authorization capability",
                    plugin.package
                )));
            }
            let secret = vault.bearer(handle).ok_or_else(|| {
                PluginError::Execution(format!(
                    "{} returned an unknown credential handle",
                    plugin.package
                ))
            })?;
            let value = HeaderValue::from_str(&format!("Bearer {secret}"))
                .map_err(|_| PluginError::Execution("invalid bearer credential".to_string()))?;
            headers.insert(http::header::AUTHORIZATION, value);
        }
        Ok(())
    }

    fn apply_response_plan(
        &self,
        plugin: &LoadedPlugin,
        headers: &mut HeaderMap,
        result: &PluginResult,
    ) -> Result<(), PluginError> {
        let Some(plan) = result.response.as_ref() else {
            return Ok(());
        };
        if !plugin.capabilities.contains("response.headers") {
            return Err(PluginError::Execution(format!(
                "{} attempted a response plan without response.headers capability",
                plugin.package
            )));
        }
        for (name, value) in &plan.headers {
            let lower = name.to_ascii_lowercase();
            if matches!(
                lower.as_str(),
                "connection" | "content-length" | "transfer-encoding" | "upgrade"
            ) {
                return Err(PluginError::Execution(format!(
                    "{} attempted to set protected response header {name}",
                    plugin.package
                )));
            }
            let name = HeaderName::from_bytes(name.as_bytes()).map_err(|_| {
                PluginError::Execution(format!("{} returned invalid header name", plugin.package))
            })?;
            let value = HeaderValue::from_str(value).map_err(|_| {
                PluginError::Execution(format!("{} returned invalid header value", plugin.package))
            })?;
            headers.insert(name, value);
        }
        Ok(())
    }
}

fn safe_audit_event(value: &Value) -> bool {
    match value {
        Value::Object(fields) => fields.iter().all(|(name, value)| {
            let name = name.to_ascii_lowercase();
            !["authorization", "credential", "password", "secret", "token"]
                .iter()
                .any(|reserved| name.contains(reserved))
                && safe_audit_event(value)
        }),
        Value::Array(values) => values.iter().all(safe_audit_event),
        Value::String(value) => value.len() <= 2048,
        _ => true,
    }
}

#[derive(Debug)]
pub enum PluginOutcome {
    Allow,
    Deny { status: StatusCode, code: String },
    Error,
}

#[derive(Debug, Deserialize)]
struct PluginResult {
    outcome: String,
    #[serde(default)]
    status: Option<u16>,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    upstream: Option<UpstreamPlan>,
    #[serde(default)]
    response: Option<ResponsePlan>,
}

#[derive(Debug, Deserialize)]
struct UpstreamPlan {
    #[serde(default)]
    headers: HashMap<String, String>,
    #[serde(default)]
    bearer: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ResponsePlan {
    #[serde(default)]
    headers: HashMap<String, String>,
}

/// Loads and validates package attachments. This is called during route
/// compilation, before the proxy begins accepting requests.
pub fn load_route_plugins(
    route_id: &str,
    attachments: &[RoutePluginConfig],
    config: &PluginRuntimeConfig,
) -> Result<PluginChain, PluginError> {
    if attachments.is_empty() {
        return Ok(PluginChain::default());
    }
    if config.memory_limit_bytes < 1024 * 1024 || config.execution_timeout_millis == 0 {
        return Err(PluginError::Config(
            "plugin memory_limit_bytes must be at least 1MiB and execution_timeout_millis must be non-zero".into(),
        ));
    }
    let host = Arc::new(HostRuntime::new(config).map_err(PluginError::Config)?);
    let mut plugins = Vec::with_capacity(attachments.len());
    for attachment in attachments {
        validate_attachment(route_id, attachment)?;
        plugins.push(load_plugin(route_id, attachment, config)?);
    }
    if plugins
        .iter()
        .any(|plugin| plugin.capabilities.contains("jwt.verify"))
    {
        host.preload_jwt_policies().map_err(PluginError::Config)?;
        host.start_jwt_refresh().map_err(PluginError::Config)?;
    }
    plugins.sort_by_key(|plugin| plugin.attachment.priority);
    if plugins.windows(2).any(|pair| {
        pair[0].attachment.priority == pair[1].attachment.priority
            && !pair[0].phases.is_disjoint(&pair[1].phases)
    }) {
        return Err(PluginError::Config(format!(
            "route {route_id} has ambiguous plugin priorities"
        )));
    }
    let fingerprint_key = rand::random();
    Ok(PluginChain {
        plugins,
        fingerprint_key,
        memory_limit_bytes: config.memory_limit_bytes,
        execution_timeout_millis: config.execution_timeout_millis,
        host: Some(host),
    })
}

fn validate_attachment(route_id: &str, attachment: &RoutePluginConfig) -> Result<(), PluginError> {
    if attachment.package.split('@').count() != 2 || attachment.package.starts_with('@') {
        return Err(PluginError::Config(format!(
            "route {} plugin package must be an immutable id@version reference",
            route_id
        )));
    }
    for name in attachment
        .permitted_headers
        .iter()
        .chain(attachment.credential_headers.values())
    {
        HeaderName::from_bytes(name.as_bytes()).map_err(|_| {
            PluginError::Config(format!(
                "route {} has an invalid plugin header name",
                route_id
            ))
        })?;
    }
    for name in &attachment.permitted_headers {
        if RESERVED_HEADERS.contains(&name.to_ascii_lowercase().as_str())
            || name.eq_ignore_ascii_case("host")
        {
            return Err(PluginError::Config(format!(
                "route {route_id} grants a reserved header to a plugin"
            )));
        }
    }
    if attachment
        .credential_headers
        .keys()
        .any(|name| name.is_empty())
    {
        return Err(PluginError::Config(format!(
            "route {route_id} has an empty plugin credential name"
        )));
    }
    Ok(())
}

fn plugin_namespace(route_id: &str, attachment: &RoutePluginConfig) -> String {
    let mut digest = Sha256::new();
    digest.update(route_id.as_bytes());
    digest.update([0]);
    digest.update(attachment.package.as_bytes());
    digest.update([0]);
    digest.update(serde_json::to_vec(&attachment.config).unwrap_or_default());
    format!("{route_id}:{}", STANDARD.encode(digest.finalize()))
}

fn load_plugin(
    route_id: &str,
    attachment: &RoutePluginConfig,
    config: &PluginRuntimeConfig,
) -> Result<LoadedPlugin, PluginError> {
    let directory = find_package_dir(&config.package_dir, &attachment.package)?;
    let manifest_path = directory.join("manifest.json");
    let manifest_bytes =
        fs::read(&manifest_path).map_err(|e| PluginError::Package(e.to_string()))?;
    let manifest: Manifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|e| PluginError::Package(format!("{}: {}", manifest_path.display(), e)))?;
    if format!("{}@{}", manifest.id, manifest.version) != attachment.package
        || manifest.runtime != "quickjs"
    {
        return Err(PluginError::Package(
            "manifest id/version/runtime does not match attachment".into(),
        ));
    }
    const SUPPORTED_PHASES: &[&str] = &["ingress", "access", "upstream", "response", "log"];
    if manifest.phases.is_empty()
        || manifest
            .phases
            .iter()
            .any(|phase| !SUPPORTED_PHASES.contains(&phase.as_str()))
    {
        return Err(PluginError::Package(
            "manifest contains no phases or an unsupported plugin phase".into(),
        ));
    }
    const SUPPORTED_CAPABILITIES: &[&str] = &[
        "identity.exchange",
        "jwt.verify",
        "cache.sensitive",
        "upstream.headers",
        "upstream.authorization",
        "response.headers",
        "audit.emit",
    ];
    if let Some(capability) = manifest
        .capabilities
        .iter()
        .find(|capability| !SUPPORTED_CAPABILITIES.contains(&capability.as_str()))
    {
        return Err(PluginError::Package(format!(
            "unsupported plugin capability {capability}"
        )));
    }
    if manifest
        .capabilities
        .iter()
        .any(|value| value == "identity.exchange")
        && config.allowed_egress_hosts.is_empty()
    {
        return Err(PluginError::Config(format!(
            "{} requires identity.exchange but plugin egress is disabled",
            attachment.package
        )));
    }
    if manifest
        .capabilities
        .iter()
        .any(|value| value == "jwt.verify")
        && config.jwt_verifiers.is_empty()
    {
        return Err(PluginError::Config(format!(
            "{} requires jwt.verify but no verifier policies are configured",
            attachment.package
        )));
    }
    if config.require_signatures {
        verify_signature(&directory, &manifest, &manifest_bytes, config)?;
    }
    let payloads = read_verified_payloads(&directory, &manifest, config.require_signatures)?;
    let schema: Value = serde_json::from_slice(&payloads.schema)
        .map_err(|e| PluginError::Package(format!("invalid configuration schema: {}", e)))?;
    let validator = validator_for(&schema)
        .map_err(|e| PluginError::Package(format!("invalid configuration schema: {}", e)))?;
    if let Err(error) = validator.validate(&attachment.config) {
        return Err(PluginError::Config(format!(
            "{} configuration is invalid: {}",
            attachment.package, error
        )));
    }
    let code = String::from_utf8(payloads.entrypoint)
        .map_err(|e| PluginError::Package(format!("entrypoint is not UTF-8: {}", e)))?;
    if code.contains("import ") || code.contains("import(") {
        return Err(PluginError::Package(
            "ES module imports are not permitted in plugins".into(),
        ));
    }
    Ok(LoadedPlugin {
        package: attachment.package.clone(),
        route_id: route_id.to_string(),
        namespace: plugin_namespace(route_id, attachment),
        phases: manifest.phases.into_iter().collect(),
        capabilities: manifest.capabilities.into_iter().collect(),
        code: Arc::new(code),
        attachment: attachment.clone(),
    })
}

struct VerifiedPayloads {
    schema: Vec<u8>,
    entrypoint: Vec<u8>,
}

/// Reads each payload exactly once and retains those verified bytes for later
/// parsing/execution. Keeping the bytes closes the verify-then-read race that
/// would otherwise let a writable package directory replace a file after its
/// digest was checked.
fn read_verified_payloads(
    directory: &Path,
    manifest: &Manifest,
    require_signatures: bool,
) -> Result<VerifiedPayloads, PluginError> {
    let schema = read_payload(
        directory,
        &manifest.configuration_schema,
        manifest.files.get(&manifest.configuration_schema),
        require_signatures,
    )?;
    let entrypoint = read_payload(
        directory,
        &manifest.entrypoint,
        manifest.files.get(&manifest.entrypoint),
        require_signatures,
    )?;
    Ok(VerifiedPayloads { schema, entrypoint })
}

fn read_payload(
    directory: &Path,
    relative: &str,
    expected_digest: Option<&String>,
    require_signatures: bool,
) -> Result<Vec<u8>, PluginError> {
    let path = safe_package_file(directory, relative)?;
    let bytes = fs::read(&path).map_err(|e| PluginError::Package(e.to_string()))?;
    if require_signatures {
        let expected_digest = expected_digest.ok_or_else(|| {
            PluginError::Package(format!(
                "signed manifest is missing a digest for {}",
                relative
            ))
        })?;
        let actual_digest = STANDARD.encode(Sha256::digest(&bytes));
        if actual_digest != *expected_digest {
            return Err(PluginError::Package(format!(
                "payload digest verification failed for {}",
                relative
            )));
        }
    }
    Ok(bytes)
}

fn verify_signature(
    directory: &Path,
    manifest: &Manifest,
    bytes: &[u8],
    config: &PluginRuntimeConfig,
) -> Result<(), PluginError> {
    let key = config
        .trusted_publishers
        .get(&manifest.publisher)
        .ok_or_else(|| {
            PluginError::Package(format!("publisher {} is not trusted", manifest.publisher))
        })?;
    let key = STANDARD
        .decode(key)
        .map_err(|_| PluginError::Package("publisher key is not base64".into()))?;
    let key: [u8; 32] = key
        .try_into()
        .map_err(|_| PluginError::Package("publisher key is not an Ed25519 key".into()))?;
    let signature = STANDARD
        .decode(
            fs::read_to_string(directory.join("manifest.sig"))
                .map_err(|_| PluginError::Package("manifest.sig is required".into()))?
                .trim(),
        )
        .map_err(|_| PluginError::Package("manifest.sig is not base64".into()))?;
    let signature = Signature::from_slice(&signature)
        .map_err(|_| PluginError::Package("manifest.sig is not an Ed25519 signature".into()))?;
    VerifyingKey::from_bytes(&key)
        .map_err(|_| PluginError::Package("invalid publisher key".into()))?
        .verify(bytes, &signature)
        .map_err(|_| PluginError::Package("manifest signature verification failed".into()))
}

fn find_package_dir(root: &Path, reference: &str) -> Result<PathBuf, PluginError> {
    let direct = root.join(reference);
    if direct.join("manifest.json").is_file() {
        return Ok(direct);
    }
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir)
            .map_err(|e| PluginError::Package(format!("cannot read package directory: {}", e)))?
        {
            let entry = entry.map_err(|e| PluginError::Package(e.to_string()))?;
            let path = entry.path();
            // Do not recurse through directory symlinks. Besides making the
            // package root less surprising, this prevents a symlink cycle
            // from making configuration activation recurse forever.
            if entry
                .file_type()
                .map_err(|e| PluginError::Package(e.to_string()))?
                .is_dir()
            {
                stack.push(path.clone());
            }
            if path.file_name().and_then(|x| x.to_str()) == Some("manifest.json") {
                let bytes = fs::read(&path).map_err(|e| PluginError::Package(e.to_string()))?;
                if let Ok(value) = serde_json::from_slice::<Manifest>(&bytes) {
                    if format!("{}@{}", value.id, value.version) == reference {
                        return path
                            .parent()
                            .map(Path::to_path_buf)
                            .ok_or_else(|| PluginError::Package("invalid manifest path".into()));
                    }
                }
            }
        }
    }
    Err(PluginError::Package(format!(
        "package {} was not found",
        reference
    )))
}

fn safe_package_file(directory: &Path, relative: &str) -> Result<PathBuf, PluginError> {
    let directory = fs::canonicalize(directory).map_err(|e| {
        PluginError::Package(format!("cannot canonicalize package directory: {}", e))
    })?;
    let path = fs::canonicalize(directory.join(relative)).map_err(|_| {
        PluginError::Package("package file escapes its directory or does not exist".into())
    })?;
    if !path.starts_with(&directory) || !path.is_file() {
        return Err(PluginError::Package(
            "package file escapes its directory or does not exist".into(),
        ));
    }
    Ok(path)
}

fn is_safe_public_code(code: &str) -> bool {
    !code.is_empty()
        && code.len() <= 64
        && code
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use http::Request;
    use tempfile::tempdir;

    #[cfg(unix)]
    use std::os::unix::fs::symlink;

    fn package(root: &Path, code: &str) -> PluginRuntimeConfig {
        let dir = root.join("example");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("manifest.json"),
            r#"{
                "id":"com.example.access", "version":"1.0.0", "runtime":"quickjs",
                "entrypoint":"index.js", "phases":["access"],
                "capabilities":["upstream.headers"], "configuration_schema":"schema.json",
                "publisher":"test"
            }"#,
        )
        .unwrap();
        fs::write(
            dir.join("schema.json"),
            r#"{"type":"object","properties":{"role":{"type":"string"}},"required":["role"]}"#,
        )
        .unwrap();
        fs::write(dir.join("index.js"), code).unwrap();
        PluginRuntimeConfig {
            package_dir: root.to_path_buf(),
            require_signatures: false,
            ..PluginRuntimeConfig::default()
        }
    }

    fn attachment() -> RoutePluginConfig {
        RoutePluginConfig {
            package: "com.example.access@1.0.0".to_string(),
            config: json!({"role":"reader"}),
            priority: 0,
            permitted_headers: vec!["x-request-id".to_string()],
            credential_headers: HashMap::from([("am".to_string(), "x-am-token".to_string())]),
        }
    }

    fn signed_package(root: &Path, code: &str) -> PluginRuntimeConfig {
        let dir = root.join("signed-example");
        let schema =
            r#"{"type":"object","properties":{"role":{"type":"string"}},"required":["role"]}"#;
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("schema.json"), schema).unwrap();
        fs::write(dir.join("index.js"), code).unwrap();

        let manifest = json!({
            "id": "com.example.access",
            "version": "1.0.0",
            "runtime": "quickjs",
            "entrypoint": "index.js",
            "phases": ["access"],
            "capabilities": ["upstream.headers"],
            "configuration_schema": "schema.json",
            "publisher": "test",
            "files": {
                "index.js": STANDARD.encode(Sha256::digest(code.as_bytes())),
                "schema.json": STANDARD.encode(Sha256::digest(schema.as_bytes())),
            },
        });
        let manifest = serde_json::to_vec(&manifest).unwrap();
        let signing_key = SigningKey::from_bytes(&[7; 32]);
        fs::write(dir.join("manifest.json"), &manifest).unwrap();
        fs::write(
            dir.join("manifest.sig"),
            STANDARD.encode(signing_key.sign(&manifest).to_bytes()),
        )
        .unwrap();

        PluginRuntimeConfig {
            package_dir: root.to_path_buf(),
            trusted_publishers: HashMap::from([(
                "test".to_string(),
                STANDARD.encode(signing_key.verifying_key().as_bytes()),
            )]),
            ..PluginRuntimeConfig::default()
        }
    }

    #[test]
    fn access_plugin_gets_redacted_context_and_sanitizes_forwarded_headers() {
        let temp = tempdir().unwrap();
        let config = package(
            temp.path(),
            r#"
            function access(ctx) {
              if (ctx.request.headers['x-request-id'] !== 'req-1') throw new Error('missing request id');
              if (!ctx.credentials.fingerprints.am || ctx.credentials.fingerprints.am === 'secret') throw new Error('credential leaked');
              return {outcome: 'allow', upstream: {headers: {'x-plugin-role': ctx.config.role}}};
            }
        "#,
        );
        let chain = load_route_plugins("orders", &[attachment()], &config).unwrap();
        let mut request = Request::builder()
            .uri("/orders")
            .header("x-request-id", "req-1")
            .header("x-am-token", "secret")
            .header("authorization", "Bearer attacker-value")
            .body(())
            .unwrap();

        assert!(matches!(
            chain.run_access(&mut request).unwrap(),
            PluginOutcome::Allow
        ));
        assert_eq!(request.headers()["x-plugin-role"], "reader");
        assert!(request.headers().get("x-am-token").is_none());
        assert!(request.headers().get("authorization").is_none());
    }

    #[test]
    fn service_owner_can_grant_a_credential_header_to_another_plugin() {
        let temp = tempdir().unwrap();
        let config = package(
            temp.path(),
            r#"
            function access(ctx) {
              if (ctx.config.role === 'reader') {
                if (!ctx.credentials.fingerprints.am || ctx.request.headers['x-am-token']) {
                  throw new Error('credential plugin received the wrong context');
                }
              } else if (ctx.request.headers['x-am-token'] !== 'secret') {
                throw new Error('explicit header grant was not honored');
              }
              return {outcome: 'allow'};
            }
        "#,
        );
        let credential_consumer = attachment();
        let explicitly_granted_reader = RoutePluginConfig {
            package: "com.example.access@1.0.0".to_string(),
            config: json!({"role":"trusted-reader"}),
            priority: 1,
            permitted_headers: vec!["x-am-token".to_string()],
            credential_headers: HashMap::new(),
        };
        let chain = load_route_plugins(
            "orders",
            &[credential_consumer, explicitly_granted_reader],
            &config,
        )
        .unwrap();
        let mut request = Request::builder()
            .uri("/orders")
            .header("x-am-token", "secret")
            .body(())
            .unwrap();

        assert!(matches!(
            chain.run_access(&mut request).unwrap(),
            PluginOutcome::Allow
        ));
        assert!(request.headers().get("x-am-token").is_none());
    }

    #[test]
    fn lifecycle_phases_apply_request_and_response_plans() {
        let temp = tempdir().unwrap();
        let config = package(
            temp.path(),
            r#"
            function ingress() {
              return {outcome: 'allow', upstream: {headers: {'x-ingress': 'yes'}}};
            }
            function access(ctx) {
              if (ctx.request.headers['x-request-id'] !== 'req-1') throw new Error('request context missing');
              return {outcome: 'allow'};
            }
            function upstream(ctx) {
              return {outcome: 'allow', upstream: {headers: {'x-upstream': 'yes'}}};
            }
            function response(ctx) {
              if (ctx.response.status !== 200) throw new Error('response status missing');
              ctx.audit.emit({event: 'response_ready', status: ctx.response.status});
              return {outcome: 'allow', response: {headers: {'x-response': 'yes'}}};
            }
            function log(ctx) {
              ctx.audit.emit({event: 'request_complete', status: ctx.response.status});
              return {outcome: 'allow'};
            }
        "#,
        );
        fs::write(
            temp.path().join("example/manifest.json"),
            r#"{
                "id":"com.example.access", "version":"1.0.0", "runtime":"quickjs",
                "entrypoint":"index.js",
                "phases":["ingress","access","upstream","response","log"],
                "capabilities":["upstream.headers","response.headers","audit.emit"],
                "configuration_schema":"schema.json", "publisher":"test"
            }"#,
        )
        .unwrap();
        let chain = load_route_plugins("orders", &[attachment()], &config).unwrap();
        let mut request = Request::builder()
            .uri("/orders")
            .header("x-request-id", "req-1")
            .body(())
            .unwrap();

        assert!(matches!(
            chain.run_access(&mut request).unwrap(),
            PluginOutcome::Allow
        ));
        assert_eq!(request.headers()["x-ingress"], "yes");
        assert_eq!(request.headers()["x-upstream"], "yes");

        let response_headers = chain
            .run_response_headers_with_cancel(
                HeaderMap::new(),
                StatusCode::OK,
                "GET",
                "/orders",
                CancellationToken::new(),
            )
            .unwrap();
        assert_eq!(response_headers["x-response"], "yes");
        chain
            .run_log_with_cancel(
                response_headers,
                StatusCode::OK,
                "GET",
                "/orders",
                CancellationToken::new(),
            )
            .unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn package_discovery_skips_directory_symlink_cycles() {
        let temp = tempdir().unwrap();
        symlink(temp.path(), temp.path().join("cycle")).unwrap();

        assert!(matches!(
            find_package_dir(temp.path(), "com.example.missing@1.0.0"),
            Err(PluginError::Package(message)) if message.contains("was not found")
        ));
    }

    #[test]
    fn deny_result_is_returned_without_exposing_plugin_details() {
        let temp = tempdir().unwrap();
        let config = package(
            temp.path(),
            "function access() { return {outcome: 'deny', status: 401, code: 'invalid_credential'}; }",
        );
        let chain = load_route_plugins("orders", &[attachment()], &config).unwrap();
        let mut request = Request::builder().uri("/orders").body(()).unwrap();
        match chain.run_access(&mut request).unwrap() {
            PluginOutcome::Deny { status, code } => {
                assert_eq!(status, StatusCode::UNAUTHORIZED);
                assert_eq!(code, "invalid_credential");
            }
            _ => panic!("expected denial"),
        }
    }

    #[test]
    fn interrupt_handler_stops_runaway_plugin_code() {
        let temp = tempdir().unwrap();
        let mut config = package(temp.path(), "function access() { while (true) {} }");
        config.execution_timeout_millis = 1;
        let chain = load_route_plugins("orders", &[attachment()], &config).unwrap();
        let mut request = Request::builder().uri("/orders").body(()).unwrap();
        assert!(matches!(
            chain.run_access(&mut request),
            Err(PluginError::Execution(_))
        ));
    }

    #[test]
    fn signed_package_rejects_tampered_entrypoint_and_schema() {
        let entrypoint_temp = tempdir().unwrap();
        let entrypoint_config = signed_package(
            entrypoint_temp.path(),
            "function access() { return {outcome: 'allow'}; }",
        );
        assert!(load_route_plugins("orders", &[attachment()], &entrypoint_config).is_ok());
        fs::write(
            entrypoint_temp.path().join("signed-example/index.js"),
            "function access() { return {outcome: 'deny'}; }",
        )
        .unwrap();
        assert!(matches!(
            load_route_plugins("orders", &[attachment()], &entrypoint_config),
            Err(PluginError::Package(message)) if message.contains("index.js")
        ));

        let schema_temp = tempdir().unwrap();
        let schema_config = signed_package(
            schema_temp.path(),
            "function access() { return {outcome: 'allow'}; }",
        );
        assert!(load_route_plugins("orders", &[attachment()], &schema_config).is_ok());
        fs::write(
            schema_temp.path().join("signed-example/schema.json"),
            "false",
        )
        .unwrap();
        assert!(matches!(
            load_route_plugins("orders", &[attachment()], &schema_config),
            Err(PluginError::Package(message)) if message.contains("schema.json")
        ));
    }
}
