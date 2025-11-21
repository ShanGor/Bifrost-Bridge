use crate::config::{RateLimitingConfig, RateLimitRuleConfig, RateLimitWindowConfig};
use hyper::Method;
use log::{debug, warn};
use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

#[derive(Clone, Debug)]
pub struct RateLimitHit {
    pub rule_id: String,
    pub retry_after_secs: u64,
}

#[derive(Clone)]
pub struct RateLimiter {
    enabled: bool,
    rules: Arc<Vec<RateLimitRule>>,
    buckets: Arc<Mutex<HashMap<BucketKey, RateWindow>>>,
}

impl RateLimiter {
    pub fn new(config: Option<RateLimitingConfig>) -> Self {
        if let Some(config) = config {
            let mut rules = Vec::new();

            if let Some(default_rule) = config.default_limit {
                rules.push(RateLimitRule::from_default(default_rule));
            }

            for rule in config.rules {
                if let Some(parsed) = RateLimitRule::from_rule_config(rule) {
                    rules.push(parsed);
                }
            }

            let enabled = config.enabled && !rules.is_empty();

            Self {
                enabled,
                rules: Arc::new(rules),
                buckets: Arc::new(Mutex::new(HashMap::new())),
            }
        } else {
            Self::disabled()
        }
    }

    pub fn disabled() -> Self {
        Self {
            enabled: false,
            rules: Arc::new(Vec::new()),
            buckets: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub async fn check_request(
        &self,
        client_ip: &str,
        method: &Method,
        path: &str,
    ) -> Result<(), RateLimitHit> {
        if !self.enabled {
            return Ok(());
        }

        let mut matched = Vec::new();
        for rule in self.rules.iter() {
            if rule.matches(method, path) {
                matched.push(rule.clone());
            }
        }

        if matched.is_empty() {
            return Ok(());
        }

        let now = Instant::now();
        let mut buckets = self.buckets.lock().await;

        for rule in matched {
            let key = BucketKey {
                rule_id: rule.id.clone(),
                client_id: client_ip.to_string(),
            };

            let entry = buckets.entry(key).or_insert_with(|| RateWindow {
                count: 0,
                window_start: now,
            });

            let elapsed = now.saturating_duration_since(entry.window_start);
            if elapsed >= rule.window {
                entry.count = 0;
                entry.window_start = now;
            }

            if entry.count >= rule.limit {
                let retry_after = rule
                    .window
                    .saturating_sub(now.saturating_duration_since(entry.window_start))
                    .as_secs()
                    .max(1);
                debug!(
                    "Rate limit exceeded for {} via rule {} (limit {}, window {:?})",
                    client_ip, rule.id, rule.limit, rule.window
                );
                return Err(RateLimitHit {
                    rule_id: rule.id.clone(),
                    retry_after_secs: retry_after,
                });
            }

            entry.count += 1;
        }

        Ok(())
    }
}

#[derive(Clone)]
struct RateLimitRule {
    id: String,
    limit: u64,
    window: Duration,
    path_prefix: Option<String>,
    methods: Option<HashSet<Method>>,
}

impl RateLimitRule {
    fn from_default(config: RateLimitWindowConfig) -> Self {
        Self {
            id: "default".to_string(),
            limit: config.limit,
            window: Duration::from_secs(config.window_secs),
            path_prefix: None,
            methods: None,
        }
    }

    fn from_rule_config(config: RateLimitRuleConfig) -> Option<Self> {
        if config.limit == 0 || config.window_secs == 0 {
            warn!(
                "Ignoring rate limit rule '{}' due to invalid limit ({}) or window ({}).",
                config.id, config.limit, config.window_secs
            );
            return None;
        }

        let path_prefix = config
            .path_prefix
            .as_ref()
            .and_then(|prefix| normalize_path_prefix(prefix));

        let methods = config.methods.as_ref().map(|list| {
            list.iter()
                .filter_map(|method| {
                    Method::from_bytes(method.trim().as_bytes()).ok().or_else(|| {
                        warn!("Unsupported HTTP method '{}' in rate limit rule {}", method, config.id);
                        None
                    })
                })
                .collect::<HashSet<_>>()
        });

        Some(Self {
            id: config.id,
            limit: config.limit,
            window: Duration::from_secs(config.window_secs),
            path_prefix,
            methods,
        })
    }

    fn matches(&self, method: &Method, path: &str) -> bool {
        if let Some(methods) = &self.methods {
            if !methods.contains(method) {
                return false;
            }
        }

        if let Some(prefix) = &self.path_prefix {
            path.starts_with(prefix)
        } else {
            true
        }
    }
}

#[derive(Hash, Eq, PartialEq)]
struct BucketKey {
    rule_id: String,
    client_id: String,
}

struct RateWindow {
    count: u64,
    window_start: Instant,
}

fn normalize_path_prefix(prefix: &str) -> Option<String> {
    let trimmed = prefix.trim();
    if trimmed.is_empty() {
        return None;
    }

    if trimmed.starts_with('/') {
        Some(trimmed.to_string())
    } else {
        Some(format!("/{}", trimmed))
    }
}
