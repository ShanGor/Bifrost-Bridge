use crate::common::{
    ConnectionTracker, PerformanceMetrics, RequestTimer, ResponseBuilder, is_websocket_upgrade,
};
use crate::config::{
    HeaderOverrideConfig, HealthCheckConfig, LoadBalancingPolicy, ReverseProxyConfig,
    ReverseProxyRouteConfig, ReverseProxyTargetConfig, RoutePredicateConfig, StickyConfig,
    StickyMode, WebSocketConfig,
};
use crate::error::ProxyError;
use crate::rate_limit::RateLimiter;
use chrono::{DateTime, FixedOffset, Utc};
use http_body_util::{BodyExt, Empty, Full};
use http_body_util::combinators::BoxBody;
use hyper::body::{Body as _, Bytes, Incoming};
use hyper::header::{HeaderName, HOST, ORIGIN};
use hyper::server::conn::http1::Builder as ServerBuilder;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode, Uri};
use hyper_util::client::legacy::{connect::HttpConnector, Client};
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use ipnet::IpNet;
use log::{debug, error, info, warn};
use rand::Rng;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::collections::hash_map::DefaultHasher;
use std::convert::Infallible;
use std::error::Error;
use std::hash::{Hash, Hasher};
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::copy_bidirectional;
use tokio::time::Duration;
use url::form_urlencoded;
use url::Url;

// Custom header names for X-Forwarded-* headers
static X_FORWARDED_FOR: HeaderName = HeaderName::from_static("x-forwarded-for");
static X_FORWARDED_PROTO: HeaderName = HeaderName::from_static("x-forwarded-proto");
static X_FORWARDED_HOST: HeaderName = HeaderName::from_static("x-forwarded-host");

type BoxError = Box<dyn Error + Send + Sync>;
type BoxedBody = BoxBody<Bytes, BoxError>;

/// Wrapper to store request data including client IP
#[derive(Clone, Debug)]
pub struct RequestContext {
    pub client_ip: Option<String>,
}

#[derive(Clone)]
struct WeightMeta {
    group: String,
    weight: u32,
}

#[derive(Clone)]
struct CompiledTarget {
    id: String,
    url: Url,
    weight: u32,
    enabled: bool,
    healthy: Arc<AtomicBool>,
    inflight: Arc<AtomicU64>,
}

struct CompiledRoute {
    id: String,
    targets: Vec<CompiledTarget>,
    http_client: Arc<Client<HttpConnector, BoxedBody>>,
    health_check_config: Option<HealthCheckConfig>,
    strip_path_prefix: Option<String>,
    priority: i32,
    predicates: Vec<Predicate>,
    weight: Option<WeightMeta>,
    original_index: usize,
    load_balancing: LoadBalancingPolicy,
    sticky: Option<StickyConfig>,
    header_override: Option<HeaderOverrideConfig>,
    retry_policy: Option<CompiledRetryPolicy>,
    rr_counter: AtomicU64,
}

#[derive(Clone)]
struct CompiledRetryPolicy {
    max_attempts: u32,
    retry_on_connect_error: bool,
    retry_on_statuses: HashSet<StatusCode>,
    methods: HashSet<Method>,
}

impl CompiledRetryPolicy {
    fn allows_method(&self, method: &Method) -> bool {
        self.methods.contains(method)
    }

    fn should_retry_status(&self, status: StatusCode) -> bool {
        self.retry_on_statuses.contains(&status)
    }

    fn should_retry_error(&self, error: &ProxyError) -> bool {
        if !self.retry_on_connect_error {
            return false;
        }
        matches!(error, ProxyError::Connection(_) | ProxyError::Hyper(_))
    }
}

struct TargetSelection<'a> {
    target: &'a CompiledTarget,
    set_cookie: Option<String>,
}

enum RequestFailure {
    Selection(ProxyError),
    Forward(ProxyError),
}

struct InflightGuard {
    counter: Arc<AtomicU64>,
}

impl InflightGuard {
    fn new(counter: Arc<AtomicU64>) -> Self {
        counter.fetch_add(1, Ordering::Relaxed);
        Self { counter }
    }
}

impl Drop for InflightGuard {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::Relaxed);
    }
}

#[derive(Clone)]
struct WeightedEntry {
    route_index: usize,
    weight: u32,
}

struct WeightedGroup {
    entries: Vec<WeightedEntry>,
    counter: AtomicU64,
}

struct RouteMatcher {
    routes: Vec<CompiledRoute>,
    weighted_groups: HashMap<String, WeightedGroup>,
}

impl RouteMatcher {
    fn new(
        route_configs: Vec<ReverseProxyRouteConfig>,
        connect_timeout_secs: u64,
        default_pool_config: Option<ReverseProxyConfig>,
    ) -> Result<Self, ProxyError> {
        if route_configs.is_empty() {
            return Err(ProxyError::Config(
                "At least one reverse proxy route must be defined".to_string(),
            ));
        }

        let mut ids = HashSet::new();
        let mut routes = Vec::new();
        let mut weighted_groups: HashMap<String, Vec<WeightedEntry>> = HashMap::new();

        for (idx, cfg) in route_configs.into_iter().enumerate() {
            if !ids.insert(cfg.id.clone()) {
                return Err(ProxyError::Config(format!(
                    "Duplicate reverse proxy route id: {}",
                    cfg.id
                )));
            }

            if cfg.predicates.is_empty() {
                return Err(ProxyError::Config(format!(
                    "Route {} must define at least one predicate",
                    cfg.id
                )));
            }

            let mut target_configs = cfg.targets;
            if !target_configs.is_empty() {
                if cfg.target.is_some() {
                    return Err(ProxyError::Config(format!(
                        "Route {} cannot define both target and targets",
                        cfg.id
                    )));
                }
            } else if let Some(target_url) = cfg.target {
                target_configs.push(ReverseProxyTargetConfig {
                    id: cfg.id.clone(),
                    url: target_url,
                    weight: 1,
                    enabled: true,
                });
            } else {
                return Err(ProxyError::Config(format!(
                    "Route {} must define a target or targets",
                    cfg.id
                )));
            }

            let pool_cfg = cfg
                .reverse_proxy_config
                .clone()
                .or_else(|| default_pool_config.clone())
                .unwrap_or_default();
            let http_client = Arc::new(ReverseProxy::build_http_client(
                connect_timeout_secs,
                pool_cfg.pool_max_idle_per_host,
                pool_cfg.pool_idle_timeout_secs,
            ));
            let health_check_config = pool_cfg.health_check.clone();

            let mut weight_meta = None;
            let predicates = cfg
                .predicates
                .into_iter()
                .map(|p| Predicate::try_from(p, &mut weight_meta))
                .collect::<Result<Vec<_>, _>>()?;

            if let Some(meta) = weight_meta.clone() {
                weighted_groups
                    .entry(meta.group.clone())
                    .or_default()
                    .push(WeightedEntry {
                        route_index: idx,
                        weight: meta.weight,
                    });
            }

            let mut target_ids = HashSet::new();
            let mut targets = Vec::new();
            for target_cfg in target_configs {
                if !target_ids.insert(target_cfg.id.clone()) {
                    return Err(ProxyError::Config(format!(
                        "Route {} has duplicate target id {}",
                        cfg.id, target_cfg.id
                    )));
                }
                if target_cfg.weight == 0 {
                    return Err(ProxyError::Config(format!(
                        "Route {} target {} must have weight >= 1",
                        cfg.id, target_cfg.id
                    )));
                }
                let url = Url::parse(&target_cfg.url).map_err(|e| {
                    ProxyError::Config(format!(
                        "Invalid target URL for {} ({}): {}",
                        cfg.id, target_cfg.id, e
                    ))
                })?;
                targets.push(CompiledTarget {
                    id: target_cfg.id,
                    url,
                    weight: target_cfg.weight,
                    enabled: target_cfg.enabled,
                    healthy: Arc::new(AtomicBool::new(true)),
                    inflight: Arc::new(AtomicU64::new(0)),
                });
            }

            if targets.is_empty() {
                return Err(ProxyError::Config(format!(
                    "Route {} must define at least one target",
                    cfg.id
                )));
            }

            if targets.iter().all(|t| !t.enabled) {
                return Err(ProxyError::Config(format!(
                    "Route {} must have at least one enabled target",
                    cfg.id
                )));
            }

            if let Some(header_override) = cfg.header_override.as_ref() {
                HeaderName::from_bytes(header_override.header_name.as_bytes()).map_err(|e| {
                    ProxyError::Config(format!(
                        "Invalid header override name for {}: {}",
                        cfg.id, e
                    ))
                })?;
                for target_id in header_override.allowed_values.values() {
                    if !target_ids.contains(target_id) {
                        return Err(ProxyError::Config(format!(
                            "Header override for {} references unknown target {}",
                            cfg.id, target_id
                        )));
                    }
                }
                for (group, group_targets) in &header_override.allowed_groups {
                    if group_targets.is_empty() {
                        return Err(ProxyError::Config(format!(
                            "Header override group {} for {} must include at least one target",
                            group, cfg.id
                        )));
                    }
                    for target_id in group_targets {
                        if !target_ids.contains(target_id) {
                            return Err(ProxyError::Config(format!(
                                "Header override group {} for {} references unknown target {}",
                                group, cfg.id, target_id
                            )));
                        }
                    }
                }
            }

            if let Some(sticky) = cfg.sticky.as_ref() {
                match sticky.mode {
                    StickyMode::Cookie => {
                        if sticky.cookie_name.as_ref().map(|n| n.is_empty()).unwrap_or(true) {
                            return Err(ProxyError::Config(format!(
                                "Route {} sticky cookie mode requires cookie_name",
                                cfg.id
                            )));
                        }
                    }
                    StickyMode::Header => {
                        let header_name = sticky.header_name.as_ref().ok_or_else(|| {
                            ProxyError::Config(format!(
                                "Route {} sticky header mode requires header_name",
                                cfg.id
                            ))
                        })?;
                        HeaderName::from_bytes(header_name.as_bytes()).map_err(|e| {
                            ProxyError::Config(format!(
                                "Invalid sticky header name for {}: {}",
                                cfg.id, e
                            ))
                        })?;
                    }
                    StickyMode::SourceIp => {}
                }
            }

            let retry_policy = if let Some(retry_policy) = cfg.retry_policy.as_ref() {
                if retry_policy.max_attempts == 0 {
                    return Err(ProxyError::Config(format!(
                        "Route {} retry policy max_attempts must be >= 1",
                        cfg.id
                    )));
                }

                let mut methods = HashSet::new();
                for method in &retry_policy.methods {
                    let parsed = Method::from_bytes(method.as_bytes()).map_err(|_| {
                        ProxyError::Config(format!(
                            "Route {} retry policy method is invalid: {}",
                            cfg.id, method
                        ))
                    })?;
                    methods.insert(parsed);
                }

                let mut retry_on_statuses = HashSet::new();
                for code in &retry_policy.retry_on_statuses {
                    let status = StatusCode::from_u16(*code).map_err(|_| {
                        ProxyError::Config(format!(
                            "Route {} retry policy status is invalid: {}",
                            cfg.id, code
                        ))
                    })?;
                    retry_on_statuses.insert(status);
                }

                Some(CompiledRetryPolicy {
                    max_attempts: retry_policy.max_attempts,
                    retry_on_connect_error: retry_policy.retry_on_connect_error,
                    retry_on_statuses,
                    methods,
                })
            } else {
                None
            };

            let load_balancing = cfg
                .load_balancing
                .clone()
                .unwrap_or_default()
                .policy;

            routes.push(CompiledRoute {
                id: cfg.id,
                targets,
                http_client,
                health_check_config,
                strip_path_prefix: cfg.strip_path_prefix,
                priority: cfg.priority.unwrap_or(0),
                predicates,
                weight: weight_meta,
                original_index: idx,
                load_balancing,
                sticky: cfg.sticky,
                header_override: cfg.header_override,
                retry_policy,
                rr_counter: AtomicU64::new(0),
            });
        }

        let weighted_groups = weighted_groups
            .into_iter()
            .map(|(group, entries)| {
                let total: u32 = entries.iter().map(|e| e.weight).sum();
                if total == 0 {
                    return Err(ProxyError::Config(format!(
                        "Weighted group {} has zero total weight",
                        group
                    )));
                }
                Ok((
                    group,
                    WeightedGroup {
                        entries,
                        counter: AtomicU64::new(0),
                    },
                ))
            })
            .collect::<Result<HashMap<_, _>, ProxyError>>()?;

        Ok(Self {
            routes,
            weighted_groups,
        })
    }

    fn route_count(&self) -> usize {
        self.routes.len()
    }

    fn routes_with_health_checks(
        &self,
    ) -> Vec<(Url, Arc<Client<HttpConnector, BoxedBody>>, HealthCheckConfig, Arc<AtomicBool>)> {
        let mut entries = Vec::new();
        for route in &self.routes {
            if let Some(cfg) = route.health_check_config.clone() {
                for target in &route.targets {
                    if target.enabled {
                        entries.push((
                            target.url.clone(),
                            route.http_client.clone(),
                            cfg.clone(),
                            target.healthy.clone(),
                        ));
                    }
                }
            }
        }
        entries
    }

    fn select_route<'a, B>(&'a self, req: &Request<B>, context: &RequestContext) -> Option<&'a CompiledRoute> {
        let mut matches: Vec<(&CompiledRoute, i32)> = Vec::new();
        for route in &self.routes {
            if route.matches(req, context) {
                matches.push((route, route.priority));
            }
        }

        if matches.is_empty() {
            return None;
        }

        let min_priority = matches
            .iter()
            .map(|(_, pri)| *pri)
            .min()
            .unwrap_or(i32::MAX);

        let mut filtered: Vec<&CompiledRoute> = matches
            .into_iter()
            .filter(|(_, pri)| *pri == min_priority)
            .map(|(r, _)| r)
            .collect();

        // Preserve declaration order within the same priority
        filtered.sort_by_key(|r| r.original_index);

        if let Some(first) = filtered.first().copied() {
            if let Some(weight_meta) = &first.weight {
                if let Some(group) = self.weighted_groups.get(&weight_meta.group) {
                    let mut active_entries = Vec::new();
                    for entry in &group.entries {
                        if filtered
                            .iter()
                            .any(|r| r.original_index == entry.route_index)
                        {
                            active_entries.push(entry);
                        }
                    }

                    let total_weight: u32 = active_entries.iter().map(|e| e.weight).sum();
                    if total_weight > 0 {
                        let seq = group.counter.fetch_add(1, Ordering::Relaxed);
                        let mut cursor = (seq % total_weight as u64) as u32;
                        for entry in active_entries {
                            if cursor < entry.weight {
                                return self.routes.get(entry.route_index);
                            }
                            cursor -= entry.weight;
                        }
                    }
                }
            }

            return Some(first);
        }

        // Should not reach here, but return first matched route to be safe
        if let Some(route) = filtered.first().copied() {
            return Some(route);
        }

        None
    }
}

impl CompiledRoute {
    fn select_target<'a, B>(
        &'a self,
        req: &Request<B>,
        context: &RequestContext,
    ) -> Result<TargetSelection<'a>, ProxyError> {
        let excluded = HashSet::new();
        self.select_target_with_exclusions(req, context, &excluded)
    }

    fn select_target_with_exclusions<'a, B>(
        &'a self,
        req: &Request<B>,
        context: &RequestContext,
        excluded: &HashSet<String>,
    ) -> Result<TargetSelection<'a>, ProxyError> {
        let eligible_targets: Vec<&CompiledTarget> = self
            .targets
            .iter()
            .filter(|t| {
                t.enabled && t.healthy.load(Ordering::Relaxed) && !excluded.contains(&t.id)
            })
            .collect();

        if eligible_targets.is_empty() {
            return Err(ProxyError::Connection(format!(
                "No healthy targets available for route {}",
                self.id
            )));
        }

        if let Some(header_override) = &self.header_override {
            if let Some(value) = req
                .headers()
                .get(header_override.header_name.as_str())
                .and_then(|v| v.to_str().ok())
            {
                if let Some(target_id) = header_override.allowed_values.get(value) {
                    if let Some(target) = eligible_targets
                        .iter()
                        .find(|t| t.id == target_id.as_str())
                        .copied()
                    {
                        return Ok(TargetSelection {
                            target,
                            set_cookie: None,
                        });
                    }
                }

                if let Some(group_targets) = header_override.allowed_groups.get(value) {
                    let group_targets: Vec<&CompiledTarget> = eligible_targets
                        .iter()
                        .filter(|t| group_targets.iter().any(|id| id == &t.id))
                        .copied()
                        .collect();
                    if let Some(target) = self.select_by_policy(&group_targets) {
                        return Ok(TargetSelection {
                            target,
                            set_cookie: None,
                        });
                    }
                }
            }
        }

        let mut needs_cookie = false;
        if let Some(sticky) = &self.sticky {
            match sticky.mode {
                StickyMode::Cookie => {
                    let cookie_name = sticky.cookie_name.as_ref().unwrap();
                    if let Some(value) = extract_cookie_value(req.headers(), cookie_name) {
                        if let Some(target) = eligible_targets
                            .iter()
                            .find(|t| t.id == value)
                            .copied()
                        {
                            return Ok(TargetSelection {
                                target,
                                set_cookie: None,
                            });
                        }
                        needs_cookie = true;
                    } else {
                        needs_cookie = true;
                    }
                }
                StickyMode::Header => {
                    let header_name = sticky.header_name.as_ref().unwrap();
                    if let Some(value) = req
                        .headers()
                        .get(header_name.as_str())
                        .and_then(|v| v.to_str().ok())
                    {
                        if let Some(target) = self.select_by_hash(value, &eligible_targets) {
                            return Ok(TargetSelection {
                                target,
                                set_cookie: None,
                            });
                        }
                    }
                }
                StickyMode::SourceIp => {
                    if let Some(ip) = context.client_ip.as_deref() {
                        if let Some(target) = self.select_by_hash(ip, &eligible_targets) {
                            return Ok(TargetSelection {
                                target,
                                set_cookie: None,
                            });
                        }
                    }
                }
            }
        }

        let target = self.select_by_policy(&eligible_targets).ok_or_else(|| {
            ProxyError::Connection(format!(
                "No available targets for route {}",
                self.id
            ))
        })?;

        let set_cookie = match &self.sticky {
            Some(sticky) if matches!(sticky.mode, StickyMode::Cookie) && needs_cookie => {
                let cookie_name = sticky.cookie_name.as_ref().unwrap();
                Some(build_sticky_cookie(
                    cookie_name,
                    &target.id,
                    sticky.ttl_seconds,
                ))
            }
            _ => None,
        };

        Ok(TargetSelection { target, set_cookie })
    }

    fn select_by_policy<'a>(&'a self, targets: &[&'a CompiledTarget]) -> Option<&'a CompiledTarget> {
        if targets.is_empty() {
            return None;
        }

        match self.load_balancing {
            LoadBalancingPolicy::RoundRobin => {
                let idx = (self.rr_counter.fetch_add(1, Ordering::Relaxed) as usize)
                    % targets.len();
                Some(targets[idx])
            }
            LoadBalancingPolicy::WeightedRoundRobin => {
                let total_weight: u32 = targets.iter().map(|t| t.weight).sum();
                if total_weight == 0 {
                    return Some(targets[0]);
                }
                let seq = self.rr_counter.fetch_add(1, Ordering::Relaxed);
                let mut cursor = (seq % total_weight as u64) as u32;
                for target in targets {
                    if cursor < target.weight {
                        return Some(*target);
                    }
                    cursor -= target.weight;
                }
                Some(targets[0])
            }
            LoadBalancingPolicy::LeastConnections => {
                targets
                    .iter()
                    .min_by_key(|t| t.inflight.load(Ordering::Relaxed))
                    .copied()
            }
            LoadBalancingPolicy::Random => {
                let idx = rand::thread_rng().gen_range(0..targets.len());
                Some(targets[idx])
            }
        }
    }

    fn select_by_hash<'a>(
        &'a self,
        key: &str,
        targets: &[&'a CompiledTarget],
    ) -> Option<&'a CompiledTarget> {
        if targets.is_empty() {
            return None;
        }
        let mut hasher = DefaultHasher::new();
        key.hash(&mut hasher);
        let idx = (hasher.finish() % targets.len() as u64) as usize;
        Some(targets[idx])
    }
}

#[derive(Clone)]
enum Predicate {
    Path(PathMatcher),
    Host(HostMatcher),
    Method(Vec<Method>),
    Header(HeaderMatcher),
    Query(QueryMatcher),
    Cookie(CookieMatcher),
    RemoteAddr(Vec<IpNet>),
    After(DateTime<FixedOffset>),
    Before(DateTime<FixedOffset>),
    Between(DateTime<FixedOffset>, DateTime<FixedOffset>),
}

impl Predicate {
    fn try_from(config: RoutePredicateConfig, weight_meta: &mut Option<WeightMeta>) -> Result<Self, ProxyError> {
        match config {
            RoutePredicateConfig::Path {
                patterns,
                match_trailing_slash,
            } => {
                if patterns.is_empty() {
                    return Err(ProxyError::Config(
                        "Path predicate requires at least one pattern".to_string(),
                    ));
                }
                let matcher = PathMatcher::from_patterns(patterns, match_trailing_slash)?;
                Ok(Predicate::Path(matcher))
            }
            RoutePredicateConfig::Host { patterns } => {
                if patterns.is_empty() {
                    return Err(ProxyError::Config(
                        "Host predicate requires at least one pattern".to_string(),
                    ));
                }
                let matcher = HostMatcher::from_patterns(patterns)?;
                Ok(Predicate::Host(matcher))
            }
            RoutePredicateConfig::Method { methods } => {
                if methods.is_empty() {
                    return Err(ProxyError::Config(
                        "Method predicate requires at least one method".to_string(),
                    ));
                }
                let parsed = methods
                    .into_iter()
                    .map(|m| Method::from_bytes(m.as_bytes()))
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|_| ProxyError::Config("Invalid HTTP method".to_string()))?;
                Ok(Predicate::Method(parsed))
            }
            RoutePredicateConfig::Header { name, value, regex } => {
                let matcher = HeaderMatcher::new(&name, value, regex)?;
                Ok(Predicate::Header(matcher))
            }
            RoutePredicateConfig::Query { name, value, regex } => {
                let matcher = QueryMatcher::new(&name, value, regex)?;
                Ok(Predicate::Query(matcher))
            }
            RoutePredicateConfig::Cookie { name, value, regex } => {
                let matcher = CookieMatcher::new(&name, value, regex)?;
                Ok(Predicate::Cookie(matcher))
            }
            RoutePredicateConfig::After { instant } => {
                let parsed = parse_instant(&instant)?;
                Ok(Predicate::After(parsed))
            }
            RoutePredicateConfig::Before { instant } => {
                let parsed = parse_instant(&instant)?;
                Ok(Predicate::Before(parsed))
            }
            RoutePredicateConfig::Between { start, end } => {
                let start = parse_instant(&start)?;
                let end = parse_instant(&end)?;
                Ok(Predicate::Between(start, end))
            }
            RoutePredicateConfig::RemoteAddr { cidrs } => {
                if cidrs.is_empty() {
                    return Err(ProxyError::Config(
                        "RemoteAddr predicate requires at least one CIDR".to_string(),
                    ));
                }
                let nets = cidrs
                    .into_iter()
                    .map(|c| c.parse::<IpNet>())
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| ProxyError::Config(format!("Invalid CIDR: {}", e)))?;
                Ok(Predicate::RemoteAddr(nets))
            }
            RoutePredicateConfig::Weight { group, weight } => {
                if weight == 0 {
                    return Err(ProxyError::Config(format!(
                        "Weight for group {} must be greater than zero",
                        group
                    )));
                }
                *weight_meta = Some(WeightMeta { group, weight });
                // Weight is not an executable predicate; always true
                Ok(Predicate::Method(vec![]))
            }
        }
    }

    fn evaluate<B>(&self, req: &Request<B>, context: &RequestContext) -> Result<bool, ProxyError> {
        match self {
            Predicate::Path(matcher) => Ok(matcher.matches(req.uri().path())),
            Predicate::Host(matcher) => {
                let host = req
                    .headers()
                    .get(HOST)
                    .and_then(|h| h.to_str().ok())
                    .or_else(|| req.uri().host());
                Ok(host.map(|h| matcher.matches(h)).unwrap_or(false))
            }
            Predicate::Method(methods) => {
                if methods.is_empty() {
                    Ok(true)
                } else {
                    Ok(methods.iter().any(|m| m == req.method()))
                }
            }
            Predicate::Header(matcher) => Ok(matcher.matches(req.headers())),
            Predicate::Query(matcher) => Ok(matcher.matches(req.uri())),
            Predicate::Cookie(matcher) => Ok(matcher.matches(req.headers())),
            Predicate::RemoteAddr(nets) => {
                if let Some(ip_str) = context.client_ip.as_deref() {
                    let ip: IpAddr = ip_str
                        .parse()
                        .map_err(|e| ProxyError::Config(format!("Invalid client IP: {}", e)))?;
                    Ok(nets.iter().any(|n| n.contains(&ip)))
                } else {
                    Ok(false)
                }
            }
            Predicate::After(instant) => {
                let now = Utc::now().with_timezone(instant.offset());
                Ok(now > *instant)
            }
            Predicate::Before(instant) => {
                let now = Utc::now().with_timezone(instant.offset());
                Ok(now < *instant)
            }
            Predicate::Between(start, end) => {
                let tz = start.offset();
                let now = Utc::now().with_timezone(tz);
                Ok(now >= *start && now < *end)
            }
        }
    }
}

#[derive(Clone)]
struct PathMatcher {
    regexes: Vec<Regex>,
}

impl PathMatcher {
    fn from_patterns(patterns: Vec<String>, match_trailing_slash: bool) -> Result<Self, ProxyError> {
        let regexes = patterns
            .iter()
            .map(|p| {
                build_ant_regex(p, match_trailing_slash, false)
                    .map_err(|e| ProxyError::Config(format!("Invalid path pattern {}: {}", p, e)))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { regexes })
    }

    fn matches(&self, path: &str) -> bool {
        self.regexes.iter().any(|r| r.is_match(path))
    }
}

#[derive(Clone)]
struct HostMatcher {
    regexes: Vec<Regex>,
}

impl HostMatcher {
    fn from_patterns(patterns: Vec<String>) -> Result<Self, ProxyError> {
        let regexes = patterns
            .iter()
            .map(|p| {
                build_ant_regex(p, false, true)
                    .map_err(|e| ProxyError::Config(format!("Invalid host pattern {}: {}", p, e)))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { regexes })
    }

    fn matches(&self, host: &str) -> bool {
        self.regexes.iter().any(|r| r.is_match(host))
    }
}

#[derive(Clone)]
struct HeaderMatcher {
    name: HeaderName,
    value: Option<String>,
    regex: Option<Regex>,
}

impl HeaderMatcher {
    fn new(name: &str, value: Option<String>, regex: Option<String>) -> Result<Self, ProxyError> {
        let name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|e| ProxyError::Config(format!("Invalid header name: {}", e)))?;
        let regex = if let Some(r) = regex {
            Some(Regex::new(&r).map_err(|e| ProxyError::Config(format!("Invalid header regex: {}", e)))?)
        } else {
            None
        };
        Ok(Self { name, value, regex })
    }

    fn matches(&self, headers: &hyper::HeaderMap) -> bool {
        let value = headers.get(&self.name).and_then(|v| v.to_str().ok());
        match (value, &self.value, &self.regex) {
            (Some(actual), Some(expected), None) => actual == expected,
            (Some(actual), None, Some(re)) => re.is_match(actual),
            (Some(actual), Some(expected), Some(re)) => actual == expected && re.is_match(actual),
            (Some(_), None, None) => true,
            _ => false,
        }
    }
}

#[derive(Clone)]
struct QueryMatcher {
    name: String,
    value: Option<String>,
    regex: Option<Regex>,
}

impl QueryMatcher {
    fn new(name: &str, value: Option<String>, regex: Option<String>) -> Result<Self, ProxyError> {
        if value.is_some() && regex.is_some() {
            return Err(ProxyError::Config(
                "Query predicate cannot specify both value and regex".to_string(),
            ));
        }
        let regex = if let Some(r) = regex {
            Some(Regex::new(&r).map_err(|e| ProxyError::Config(format!("Invalid query regex: {}", e)))?)
        } else {
            None
        };
        Ok(Self {
            name: name.to_string(),
            value,
            regex,
        })
    }

    fn matches(&self, uri: &Uri) -> bool {
        if let Some(query) = uri.query() {
            for (k, v) in form_urlencoded::parse(query.as_bytes()) {
                if k == self.name {
                    if let Some(expected) = &self.value {
                        return &v == expected;
                    }
                    if let Some(re) = &self.regex {
                        return re.is_match(&v);
                    }
                    return true;
                }
            }
        }
        false
    }
}

#[derive(Clone)]
struct CookieMatcher {
    name: String,
    value: Option<String>,
    regex: Option<Regex>,
}

impl CookieMatcher {
    fn new(name: &str, value: Option<String>, regex: Option<String>) -> Result<Self, ProxyError> {
        if value.is_some() && regex.is_some() {
            return Err(ProxyError::Config(
                "Cookie predicate cannot specify both value and regex".to_string(),
            ));
        }
        let regex = if let Some(r) = regex {
            Some(Regex::new(&r).map_err(|e| ProxyError::Config(format!("Invalid cookie regex: {}", e)))?)
        } else {
            None
        };
        Ok(Self {
            name: name.to_string(),
            value,
            regex,
        })
    }

    fn matches(&self, headers: &hyper::HeaderMap) -> bool {
        let mut found = false;
        for val in headers.get_all("cookie").iter() {
            if let Ok(cookie_str) = val.to_str() {
                for part in cookie_str.split(';') {
                    let trimmed = part.trim();
                    if let Some((name, value)) = trimmed.split_once('=') {
                        if name == self.name {
                            found = true;
                            if let Some(expected) = &self.value {
                                if value == expected {
                                    return true;
                                }
                            } else if let Some(re) = &self.regex {
                                if re.is_match(value) {
                                    return true;
                                }
                            } else {
                                return true;
                            }
                        }
                    }
                }
            }
        }
        found && self.value.is_none() && self.regex.is_none()
    }
}

impl CompiledRoute {
    fn matches<B>(&self, req: &Request<B>, context: &RequestContext) -> bool {
        for predicate in &self.predicates {
            match predicate.evaluate(req, context) {
                Ok(true) => continue,
                Ok(false) => return false,
                Err(e) => {
                    warn!("Predicate evaluation error on route {}: {}", self.id, e);
                    return false;
                }
            }
        }
        true
    }
}

fn parse_instant(raw: &str) -> Result<DateTime<FixedOffset>, ProxyError> {
    DateTime::parse_from_rfc3339(raw)
        .map_err(|e| ProxyError::Config(format!("Invalid timestamp {}: {}", raw, e)))
}

fn extract_cookie_value(headers: &hyper::HeaderMap, cookie_name: &str) -> Option<String> {
    for val in headers.get_all("cookie").iter() {
        if let Ok(cookie_str) = val.to_str() {
            for part in cookie_str.split(';') {
                let trimmed = part.trim();
                let mut pieces = trimmed.splitn(2, '=');
                let name = pieces.next()?.trim();
                if name == cookie_name {
                    let value = pieces.next().unwrap_or("").trim();
                    if !value.is_empty() {
                        return Some(value.to_string());
                    }
                }
            }
        }
    }
    None
}

fn build_sticky_cookie(name: &str, value: &str, ttl_seconds: Option<u64>) -> String {
    let mut cookie = format!("{}={}; Path=/; SameSite=Lax", name, value);
    if let Some(ttl) = ttl_seconds {
        cookie.push_str(&format!("; Max-Age={}", ttl));
    }
    cookie
}

fn build_ant_regex(
    pattern: &str,
    match_trailing_slash: bool,
    case_insensitive: bool,
) -> Result<Regex, regex::Error> {
    let mut regex = String::from("^");
    let mut chars = pattern.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '*' => {
                if chars.peek() == Some(&'*') {
                    chars.next();
                    regex.push_str(".*");
                } else {
                    regex.push_str("[^/]*");
                }
            }
            '{' => {
                while let Some(next) = chars.next() {
                    if next == '}' {
                        break;
                    }
                }
                regex.push_str("([^/]+)");
            }
            '?' => regex.push_str("."),
            '.' | '+' | '(' | ')' | '|' | '^' | '$' | '[' | ']' | '\\' => {
                regex.push('\\');
                regex.push(ch);
            }
            _ => regex.push(ch),
        }
    }
    if match_trailing_slash {
        regex.push_str("/?");
    }
    regex.push('$');
    if case_insensitive {
        regex.insert_str(0, "(?i)");
    }
    Regex::new(&regex)
}

pub struct ReverseProxy {
    routes: Arc<RouteMatcher>,
    preserve_host: bool,
    metrics: Arc<PerformanceMetrics>,
    websocket_config: WebSocketConfig,
    rate_limiter: Arc<RateLimiter>,
}

impl ReverseProxy {
    /// Creates a new reverse proxy with default pooling configuration (single route fallback)
    pub fn new(
        target_url: String,
        connect_timeout_secs: u64,
        idle_timeout_secs: u64,
        max_connection_lifetime_secs: u64,
    ) -> Result<Self, ProxyError> {
        Self::new_with_config(
            target_url,
            connect_timeout_secs,
            idle_timeout_secs,
            max_connection_lifetime_secs,
            None,
            None,
        )
    }

    /// Creates a new reverse proxy with custom pooling configuration (single route fallback)
    pub fn new_with_config(
        target_url: String,
        connect_timeout_secs: u64,
        idle_timeout_secs: u64,
        max_connection_lifetime_secs: u64,
        reverse_proxy_config: Option<ReverseProxyConfig>,
        websocket_config: Option<WebSocketConfig>,
    ) -> Result<Self, ProxyError> {
        let route = ReverseProxyRouteConfig {
            id: "default".to_string(),
            target: Some(target_url),
            targets: Vec::new(),
            load_balancing: None,
            sticky: None,
            header_override: None,
            retry_policy: None,
            reverse_proxy_config: reverse_proxy_config.clone(),
            strip_path_prefix: None,
            priority: Some(0),
            predicates: vec![RoutePredicateConfig::Path {
                patterns: vec!["/**".to_string()],
                match_trailing_slash: true,
            }],
        };
        Self::new_with_routes(
            vec![route],
            connect_timeout_secs,
            idle_timeout_secs,
            max_connection_lifetime_secs,
            reverse_proxy_config,
            websocket_config,
        )
    }

    /// Creates a new reverse proxy from multi-route configuration
    pub fn new_with_routes(
        routes: Vec<ReverseProxyRouteConfig>,
        connect_timeout_secs: u64,
        _idle_timeout_secs: u64,
        _max_connection_lifetime_secs: u64,
        reverse_proxy_config: Option<ReverseProxyConfig>,
        websocket_config: Option<WebSocketConfig>,
    ) -> Result<Self, ProxyError> {
        let router = Arc::new(RouteMatcher::new(
            routes,
            connect_timeout_secs,
            reverse_proxy_config,
        )?);

        info!("Reverse proxy configuration: {} routes", router.route_count());

        Ok(Self {
            routes: router,
            preserve_host: true,
            metrics: Arc::new(PerformanceMetrics::new()),
            websocket_config: websocket_config.unwrap_or_default(),
            rate_limiter: Arc::new(RateLimiter::new(None)),
        })
    }

    /// Build HTTP client for reverse proxy with connection pooling
    fn build_http_client(
        connect_timeout_secs: u64,
        pool_max_idle_per_host: usize,
        pool_idle_timeout_secs: u64,
    ) -> Client<HttpConnector, BoxedBody> {
        let mut connector = HttpConnector::new();
        connector.set_connect_timeout(Some(Duration::from_secs(connect_timeout_secs)));
        connector.set_keepalive(Some(Duration::from_secs(pool_idle_timeout_secs)));
        connector.set_nodelay(true);

        let mut builder = Client::builder(TokioExecutor::new());

        if pool_max_idle_per_host == 0 {
            info!("Reverse proxy: connection pooling DISABLED (pool_max_idle_per_host=0)");
            builder.pool_max_idle_per_host(0);
        } else {
            info!(
                "Reverse proxy: connection pooling ENABLED (pool_max_idle_per_host={}, idle_timeout={}s)",
                pool_max_idle_per_host, pool_idle_timeout_secs
            );
            builder.pool_max_idle_per_host(pool_max_idle_per_host);
            builder.pool_idle_timeout(Duration::from_secs(pool_idle_timeout_secs));
            builder.pool_timer(TokioTimer::new());
        }

        builder.http2_only(false).build(connector)
    }

    pub fn with_preserve_host(mut self, preserve_host: bool) -> Self {
        self.preserve_host = preserve_host;
        self
    }

    pub fn with_metrics(mut self, metrics: Arc<PerformanceMetrics>) -> Self {
        self.metrics = metrics;
        self
    }

    pub fn with_rate_limiter(mut self, rate_limiter: Arc<RateLimiter>) -> Self {
        self.rate_limiter = rate_limiter;
        self
    }

    /// Public method for handling individual requests (used by CombinedProxyAdapter)
    pub async fn handle_request_with_context(
        &self,
        req: Request<Incoming>,
        context: RequestContext,
    ) -> Result<Response<Full<Bytes>>, Infallible> {
        Self::handle_request_static(
            req,
            context,
            self.routes.clone(),
            self.preserve_host,
            Arc::new(self.websocket_config.clone()),
            self.metrics.clone(),
            self.rate_limiter.clone(),
        )
        .await
    }

    pub async fn run(self, addr: SocketAddr) -> Result<(), ProxyError> {
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| ProxyError::Hyper(e.to_string()))?;

        info!("Reverse proxy listening on: {}", addr);

        for (target_url, client, cfg, healthy) in self.routes.routes_with_health_checks() {
            tokio::spawn(async move {
                Self::health_check_loop(client, target_url, cfg, healthy).await;
            });
        }

        let routes = self.routes.clone();
        let preserve_host = self.preserve_host;
        let websocket_config = Arc::new(self.websocket_config.clone());
        let metrics = self.metrics.clone();
        let rate_limiter = self.rate_limiter.clone();

        loop {
            let (stream, remote_addr) = listener
                .accept()
                .await
                .map_err(|e| ProxyError::Hyper(e.to_string()))?;

            let routes = routes.clone();
            let metrics = metrics.clone();
            let websocket_cfg = websocket_config.clone();
            let rate_limiter = rate_limiter.clone();

            tokio::spawn(async move {
                let _connection = ConnectionTracker::new(metrics.clone());
                let io = TokioIo::new(stream);

                if let Err(err) = ServerBuilder::new()
                    .serve_connection(
                        io,
                        service_fn(move |req| {
                            let routes = routes.clone();
                            let client_ip = Some(remote_addr.ip().to_string());
                            let metrics = metrics.clone();
                            let websocket_cfg = websocket_cfg.clone();
                            let rate_limiter = rate_limiter.clone();

                            let context = RequestContext {
                                client_ip: client_ip.clone(),
                            };

                            async move {
                                metrics.increment_requests();
                                let timer = RequestTimer::with_metrics(metrics.clone());
                                let result = Self::handle_request_static(
                                    req,
                                    context,
                                    routes,
                                    preserve_host,
                                    websocket_cfg,
                                    metrics.clone(),
                                    rate_limiter.clone(),
                                )
                                .await;

                                if let Some(len) = result
                                    .as_ref()
                                    .ok()
                                    .and_then(|response| response.body().size_hint().exact())
                                {
                                    metrics.record_response_bytes(len as u64);
                                }
                                timer.finish();
                                result
                            }
                        }),
                    )
                    .await
                {
                    error!("Error serving reverse proxy connection: {}", err);
                }
            });
        }
    }

    /// Static method to handle requests (used in service_fn)
    async fn handle_request_static(
        req: Request<Incoming>,
        context: RequestContext,
        routes: Arc<RouteMatcher>,
        preserve_host: bool,
        websocket_config: Arc<WebSocketConfig>,
        metrics: Arc<PerformanceMetrics>,
        rate_limiter: Arc<RateLimiter>,
    ) -> Result<Response<Full<Bytes>>, Infallible> {
        if rate_limiter.is_enabled() {
            if let Some(client_ip) = context.client_ip.as_deref() {
                if let Err(hit) = rate_limiter
                    .check_request(
                        client_ip,
                        req.method(),
                        req.uri()
                            .path_and_query()
                            .map(|pq| pq.as_str())
                            .unwrap_or("/"),
                    )
                    .await
                {
                    warn!(
                        "Reverse proxy rate limit hit for {} via rule {}",
                        client_ip, hit.rule_id
                    );
                    return Ok(ResponseBuilder::too_many_requests(
                        &hit.rule_id,
                        hit.retry_after_secs,
                    ));
                }
            }
        }

        let selected_route = match routes.select_route(&req, &context) {
            Some(route) => route,
            None => return Ok(ResponseBuilder::error(StatusCode::NOT_FOUND, "No matching route")),
        };

        if is_websocket_upgrade(req.headers()) {
            let TargetSelection { target, set_cookie } =
                match selected_route.select_target(&req, &context) {
                    Ok(selection) => selection,
                    Err(e) => {
                        warn!(
                            "Target selection failed for route {}: {}",
                            selected_route.id, e
                        );
                        return Ok(ResponseBuilder::error(
                            StatusCode::SERVICE_UNAVAILABLE,
                            &e.to_string(),
                        ));
                    }
                };
            let mut response = match Self::handle_websocket_request(
                req,
                context,
                selected_route,
                target,
                preserve_host,
                websocket_config,
            )
            .await {
                Ok(response) => response,
                Err(e) => match e {},
            };
            if let Some(cookie) = set_cookie {
                if let Ok(value) = cookie.parse() {
                    response.headers_mut().append("Set-Cookie", value);
                }
            }
            return Ok(response);
        }

        match Self::process_request_with_retries(req, context, selected_route, preserve_host).await {
            Ok((mut response, set_cookie)) => {
                if let Some(cookie) = set_cookie {
                    if let Ok(value) = cookie.parse() {
                        response.headers_mut().append("Set-Cookie", value);
                    }
                }
                Ok(response)
            }
            Err(RequestFailure::Selection(e)) => {
                warn!("Target selection failed for route {}: {}", selected_route.id, e);
                Ok(ResponseBuilder::error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    &e.to_string(),
                ))
            }
            Err(RequestFailure::Forward(e)) => {
                error!("Proxy error: {}", e);
                let body = Full::new(Bytes::from(format!("Proxy Error: {}", e)));
                let error_response = Response::builder()
                    .status(StatusCode::BAD_GATEWAY)
                    .body(body)
                    .unwrap();
                metrics.increment_connection_errors();
                Ok(error_response)
            }
        }
    }

    /// Process request using HTTP client with connection pooling
    async fn process_request_static(
        req: Request<Incoming>,
        context: RequestContext,
        selected_route: &CompiledRoute,
        selected_target: &CompiledTarget,
        preserve_host: bool,
    ) -> Result<Response<Full<Bytes>>, ProxyError> {
        let _inflight = InflightGuard::new(selected_target.inflight.clone());
        let prepared = Self::rewrite_backend_request(
            req,
            &context,
            &selected_target.url,
            preserve_host,
            false,
            selected_route.strip_path_prefix.as_deref(),
        )?;

        let prepared = Self::box_incoming_request(prepared);
        let response = selected_route
            .http_client
            .request(prepared)
            .await
            .map_err(|e| ProxyError::Connection(format!("Failed to forward request: {}", e)))?;

        Self::finalize_backend_response(response, false).await
    }

    async fn process_buffered_request(
        req: Request<Full<Bytes>>,
        context: RequestContext,
        selected_route: &CompiledRoute,
        selected_target: &CompiledTarget,
        preserve_host: bool,
    ) -> Result<Response<Full<Bytes>>, ProxyError> {
        let _inflight = InflightGuard::new(selected_target.inflight.clone());
        let prepared = Self::rewrite_backend_request(
            req,
            &context,
            &selected_target.url,
            preserve_host,
            false,
            selected_route.strip_path_prefix.as_deref(),
        )?;

        let prepared = Self::box_infallible_request(prepared);
        let response = selected_route
            .http_client
            .request(prepared)
            .await
            .map_err(|e| ProxyError::Connection(format!("Failed to forward request: {}", e)))?;

        Self::finalize_backend_response(response, false).await
    }

    async fn process_request_with_retries(
        req: Request<Incoming>,
        context: RequestContext,
        selected_route: &CompiledRoute,
        preserve_host: bool,
    ) -> Result<(Response<Full<Bytes>>, Option<String>), RequestFailure> {
        let retry_policy = selected_route.retry_policy.as_ref();

        if retry_policy
            .map(|policy| policy.max_attempts <= 1 || !policy.allows_method(req.method()))
            .unwrap_or(true)
        {
            let TargetSelection { target, set_cookie } =
                selected_route.select_target(&req, &context).map_err(RequestFailure::Selection)?;
            let response =
                Self::process_request_static(req, context, selected_route, target, preserve_host)
                    .await
                    .map_err(RequestFailure::Forward)?;
            return Ok((response, set_cookie));
        }

        let retry_policy = retry_policy.unwrap();
        let (parts, body) = req.into_parts();
        let body_bytes = body.collect().await.map_err(|e| {
            RequestFailure::Forward(ProxyError::Http(format!(
                "Failed to collect request body: {}",
                e
            )))
        })?;
        let body_bytes = body_bytes.to_bytes();

        let mut excluded = HashSet::new();
        let mut last_error: Option<ProxyError> = None;
        let mut last_response: Option<(Response<Full<Bytes>>, Option<String>)> = None;

        for attempt in 0..retry_policy.max_attempts {
            let attempt_request =
                Request::from_parts(parts.clone(), Full::new(body_bytes.clone()));
            let selection = match selected_route.select_target_with_exclusions(
                &attempt_request,
                &context,
                &excluded,
            ) {
                Ok(selection) => selection,
                Err(err) => {
                    if let Some(response) = last_response {
                        return Ok(response);
                    }
                    if let Some(error) = last_error {
                        return Err(RequestFailure::Forward(error));
                    }
                    return Err(RequestFailure::Selection(err));
                }
            };
            let TargetSelection { target, set_cookie } = selection;

            excluded.insert(target.id.clone());
            let set_cookie_clone = set_cookie.clone();

            match Self::process_buffered_request(
                attempt_request,
                context.clone(),
                selected_route,
                target,
                preserve_host,
            )
            .await
            {
                Ok(response) => {
                    if retry_policy.should_retry_status(response.status())
                        && attempt + 1 < retry_policy.max_attempts
                    {
                        last_response = Some((response, set_cookie_clone));
                        continue;
                    }
                    return Ok((response, set_cookie));
                }
                Err(err) => {
                    if retry_policy.should_retry_error(&err)
                        && attempt + 1 < retry_policy.max_attempts
                    {
                        last_error = Some(err);
                        continue;
                    }
                    return Err(RequestFailure::Forward(err));
                }
            }
        }

        if let Some(response) = last_response {
            return Ok(response);
        }

        Err(RequestFailure::Forward(
            last_error.unwrap_or_else(|| {
                ProxyError::Connection("Retry attempts exhausted".to_string())
            }),
        ))
    }

    async fn handle_websocket_request(
        mut req: Request<Incoming>,
        context: RequestContext,
        selected_route: &CompiledRoute,
        selected_target: &CompiledTarget,
        preserve_host: bool,
        websocket_config: Arc<WebSocketConfig>,
    ) -> Result<Response<Full<Bytes>>, Infallible> {
        if let Err(reason) = Self::validate_websocket_headers(req.headers(), &websocket_config) {
            return Ok(ResponseBuilder::error(StatusCode::FORBIDDEN, &reason));
        }

        let target_url = selected_target.url.clone();
        let http_client = selected_route.http_client.clone();

        let client_upgrade = hyper::upgrade::on(&mut req);
        let prepared_request =
            match Self::rewrite_backend_request(
                req,
                &context,
                &target_url,
                preserve_host,
                true,
                selected_route.strip_path_prefix.as_deref(),
            ) {
                Ok(request) => request,
                Err(e) => {
                    error!("WebSocket request rewrite failed: {}", e);
                    return Ok(ResponseBuilder::error(
                        StatusCode::BAD_GATEWAY,
                        "Invalid WebSocket request",
                    ));
                }
            };
        let prepared_request = Self::box_incoming_request(prepared_request);

        let mut backend_response = match http_client.request(prepared_request).await {
            Ok(resp) => resp,
            Err(e) => {
                error!("WebSocket backend request failed: {}", e);
                return Ok(ResponseBuilder::error(
                    StatusCode::BAD_GATEWAY,
                    "WebSocket backend error",
                ));
            }
        };

        if backend_response.status() != StatusCode::SWITCHING_PROTOCOLS {
            return match Self::finalize_backend_response(backend_response, false).await {
                Ok(resp) => Ok(resp),
                Err(e) => {
                    error!("Failed to finalize backend response: {}", e);
                    Ok(ResponseBuilder::error(
                        StatusCode::BAD_GATEWAY,
                        "WebSocket backend error",
                    ))
                }
            };
        }

        let backend_upgrade = hyper::upgrade::on(&mut backend_response);
        let (parts, _) = backend_response.into_parts();
        let switch_response = Response::from_parts(parts, Full::new(Bytes::new()));

        let inflight = selected_target.inflight.clone();
        tokio::spawn(async move {
            let _inflight = InflightGuard::new(inflight);
            match (client_upgrade.await, backend_upgrade.await) {
                (Ok(client_stream), Ok(backend_stream)) => {
                    let mut client_io = TokioIo::new(client_stream);
                    let mut backend_io = TokioIo::new(backend_stream);
                    if let Err(e) = copy_bidirectional(&mut client_io, &mut backend_io).await {
                        error!("WebSocket tunnel error: {}", e);
                    }
                }
                (Err(e), _) => error!("Client WebSocket upgrade failed: {}", e),
                (_, Err(e)) => error!("Backend WebSocket upgrade failed: {}", e),
            }
        });

        Ok(switch_response)
    }

    fn validate_websocket_headers(headers: &hyper::HeaderMap, config: &WebSocketConfig) -> Result<(), String> {
        if !config.enabled {
            return Err("WebSocket support is disabled".to_string());
        }

        if config.allowed_origins.iter().all(|o| o != "*") {
            let origin = headers
                .get(ORIGIN)
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| "Origin header is required for WebSocket requests".to_string())?;

            if !config
                .allowed_origins
                .iter()
                .any(|allowed| allowed.eq_ignore_ascii_case(origin))
            {
                return Err("Origin not allowed".to_string());
            }
        }

        if !config.supported_protocols.is_empty() {
            let offered = headers
                .get("Sec-WebSocket-Protocol")
                .and_then(|v| v.to_str().ok())
                .map(|raw| raw.split(',').map(|s| s.trim().to_string()).collect::<Vec<_>>())
                .unwrap_or_else(|| Vec::new());

            if offered.is_empty() {
                return Err("WebSocket subprotocol required".to_string());
            }

            let supported = config
                .supported_protocols
                .iter()
                .map(|p| p.to_ascii_lowercase())
                .collect::<Vec<_>>();
            if !offered
                .iter()
                .any(|offer| supported.iter().any(|allowed| allowed == &offer.to_ascii_lowercase()))
            {
                return Err("Unsupported WebSocket subprotocol".to_string());
            }
        }

        Ok(())
    }

    fn rewrite_backend_request<B>(
        mut req: Request<B>,
        context: &RequestContext,
        target_url: &Url,
        preserve_host: bool,
        keep_upgrade: bool,
        strip_path_prefix: Option<&str>,
    ) -> Result<Request<B>, ProxyError> {
        let path_and_query = req
            .uri()
            .path_and_query()
            .ok_or_else(|| ProxyError::Config("Invalid URI path".to_string()))?;

        let raw = path_and_query.as_str();
        let (raw_path, raw_query) = raw
            .split_once('?')
            .map(|(p, q)| (p, Some(q)))
            .unwrap_or((raw, None));

        let mut path = raw_path.to_string();
        if let Some(prefix) = strip_path_prefix {
            if path.starts_with(prefix) {
                path = path[prefix.len()..].to_string();
                if path.is_empty() {
                    path = "/".to_string();
                } else if !path.starts_with('/') {
                    path = format!("/{}", path);
                }
            }
        }

        let new_path_and_query = if let Some(query) = raw_query {
            format!("{}?{}", path, query)
        } else {
            path
        };

        let target_url_string = format!(
            "{}{}",
            target_url.as_str().trim_end_matches('/'),
            new_path_and_query
        );

        let target_uri: Uri = target_url_string
            .parse()
            .map_err(|e: hyper::http::uri::InvalidUri| ProxyError::Uri(e.to_string()))?;

        let original_host = req.headers().get(HOST).cloned();
        *req.uri_mut() = target_uri.clone();

        let headers = req.headers_mut();

        if !preserve_host {
            if let Some(authority) = target_uri.authority() {
                headers.insert(HOST, authority.to_string().parse().unwrap());
            }
        }

        if let Some(client_ip) = &context.client_ip {
            headers.insert(X_FORWARDED_FOR.clone(), client_ip.parse().unwrap());
        }
        headers.insert(X_FORWARDED_PROTO.clone(), "https".parse().unwrap());
        if let Some(host) = original_host {
            headers.insert(X_FORWARDED_HOST.clone(), host);
        }

        Self::strip_request_headers(headers, keep_upgrade);
        Ok(req)
    }

    fn strip_request_headers(headers: &mut hyper::HeaderMap, keep_upgrade: bool) {
        if !keep_upgrade {
            headers.remove("Connection");
            headers.remove("Upgrade");
        }
        headers.remove("Keep-Alive");
        headers.remove("Proxy-Authenticate");
        headers.remove("Proxy-Authorization");
        headers.remove("TE");
        headers.remove("Trailers");
        headers.remove("Transfer-Encoding");
    }

    fn box_incoming_request(req: Request<Incoming>) -> Request<BoxedBody> {
        let (parts, body) = req.into_parts();
        let body = body.map_err(|err| Box::new(err) as BoxError).boxed();
        Request::from_parts(parts, body)
    }

    fn box_infallible_request<B>(req: Request<B>) -> Request<BoxedBody>
    where
        B: hyper::body::Body<Data = Bytes, Error = Infallible> + Send + Sync + 'static,
    {
        let (parts, body) = req.into_parts();
        let body = body.map_err(|err| match err {}).boxed();
        Request::from_parts(parts, body)
    }

    async fn finalize_backend_response(
        response: Response<Incoming>,
        keep_upgrade: bool,
    ) -> Result<Response<Full<Bytes>>, ProxyError> {
        let (mut parts, body) = response.into_parts();
        let body_bytes = body
            .collect()
            .await
            .map_err(|e| ProxyError::Http(format!("Failed to collect response body: {}", e)))?;

        Self::strip_response_headers(&mut parts.headers, keep_upgrade);
        parts
            .headers
            .insert("X-Proxy-Server", "rust-reverse-proxy".parse().unwrap());

        Ok(Response::from_parts(parts, Full::new(body_bytes.to_bytes())))
    }

    fn strip_response_headers(headers: &mut hyper::HeaderMap, keep_upgrade: bool) {
        if !keep_upgrade {
            headers.remove("Connection");
            headers.remove("Upgrade");
        }
        headers.remove("Keep-Alive");
        headers.remove("Proxy-Authenticate");
        headers.remove("Proxy-Authorization");
        headers.remove("TE");
        headers.remove("Trailers");
        headers.remove("Transfer-Encoding");
    }

    /// Health check loop (runs in background)
    async fn health_check_loop(
        http_client: Arc<Client<HttpConnector, BoxedBody>>,
        target_url: Url,
        config: HealthCheckConfig,
        healthy: Arc<AtomicBool>,
    ) {
        let interval = Duration::from_secs(config.interval_secs);
        let timeout = Duration::from_secs(config.timeout_secs);
        let endpoint = config.endpoint;

        info!("Starting health check loop for {}", target_url);

        let mut interval_timer = tokio::time::interval(interval);
        loop {
            interval_timer.tick().await;

            let is_healthy = if let Some(ref endpoint) = endpoint {
                Self::http_health_check(&http_client, &target_url, endpoint, timeout).await
            } else {
                Self::tcp_health_check(&target_url, timeout).await
            };

            if is_healthy {
                healthy.store(true, Ordering::Relaxed);
                debug!("Health check passed for {}", target_url);
            } else {
                healthy.store(false, Ordering::Relaxed);
                warn!("Health check failed for {}", target_url);
            }
        }
    }

    /// TCP health check (default)
    async fn tcp_health_check(target_url: &Url, timeout: Duration) -> bool {
        let host = match target_url.host_str() {
            Some(h) => h,
            None => return false,
        };
        let port = target_url.port().unwrap_or(80);

        match tokio::time::timeout(timeout, tokio::net::TcpStream::connect((host, port))).await {
            Ok(Ok(_)) => true,
            Ok(Err(e)) => {
                debug!("TCP health check failed: {}", e);
                false
            }
            Err(_) => {
                debug!("TCP health check timeout");
                false
            }
        }
    }

    /// HTTP endpoint health check
    async fn http_health_check(
        _http_client: &Client<HttpConnector, BoxedBody>,
        target_url: &Url,
        endpoint: &str,
        timeout: Duration,
    ) -> bool {
        let health_url = format!("{}{}", target_url.as_str().trim_end_matches('/'), endpoint);

        // Use a simple HTTP client for health check (not the pooled client)
        let connector = HttpConnector::new();
        let simple_client: Client<HttpConnector, Empty<Bytes>> =
            Client::builder(TokioExecutor::new()).build(connector);

        let request = match Request::builder()
            .method(Method::GET)
            .uri(health_url)
            .body(Empty::<Bytes>::new())
        {
            Ok(req) => req,
            Err(e) => {
                debug!("Failed to build health check request: {}", e);
                return false;
            }
        };

        match tokio::time::timeout(timeout, simple_client.request(request)).await {
            Ok(Ok(response)) => {
                let status = response.status();
                status.is_success() || status.is_redirection()
            }
            Ok(Err(e)) => {
                debug!("HTTP health check failed: {}", e);
                false
            }
            Err(_) => {
                debug!("HTTP health check timeout");
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use http_body_util::Empty;
    use std::collections::{HashMap, HashSet};
    use crate::config::RetryPolicyConfig;

    #[test]
    fn test_reverse_proxy_creation() {
        let result = ReverseProxy::new(
            "http://backend.example.com".to_string(),
            10,
            90,
            300,
        );
        assert!(result.is_ok());

        let invalid_url = ReverseProxy::new("not-a-url".to_string(), 10, 90, 300);
        assert!(invalid_url.is_err());
    }

    #[test]
    fn test_route_matching_priority() {
        let routes = vec![
            ReverseProxyRouteConfig {
                id: "high".to_string(),
                target: Some("http://h.example.com".to_string()),
                targets: Vec::new(),
                load_balancing: None,
                sticky: None,
                header_override: None,
                retry_policy: None,
                reverse_proxy_config: None,
                strip_path_prefix: None,
                priority: Some(1),
                predicates: vec![RoutePredicateConfig::Path {
                    patterns: vec!["/api/**".to_string()],
                    match_trailing_slash: true,
                }],
            },
            ReverseProxyRouteConfig {
                id: "low".to_string(),
                target: Some("http://l.example.com".to_string()),
                targets: Vec::new(),
                load_balancing: None,
                sticky: None,
                header_override: None,
                retry_policy: None,
                reverse_proxy_config: None,
                strip_path_prefix: None,
                priority: Some(5),
                predicates: vec![RoutePredicateConfig::Path {
                    patterns: vec!["/**".to_string()],
                    match_trailing_slash: true,
                }],
            },
        ];
        let matcher = RouteMatcher::new(routes, 10, None).unwrap();

        let req = Request::builder()
            .method(Method::GET)
            .uri("/api/users")
            .body(Empty::<Bytes>::new())
            .unwrap();
        let route = matcher
            .select_route(&req, &RequestContext { client_ip: None })
            .unwrap();
        assert_eq!(route.id, "high");
    }

    #[test]
    fn test_weighted_selection_single_group() {
        let routes = vec![
            ReverseProxyRouteConfig {
                id: "a".to_string(),
                target: Some("http://a.example.com".to_string()),
                targets: Vec::new(),
                load_balancing: None,
                sticky: None,
                header_override: None,
                retry_policy: None,
                reverse_proxy_config: None,
                strip_path_prefix: None,
                priority: Some(0),
                predicates: vec![
                    RoutePredicateConfig::Path {
                        patterns: vec!["/**".to_string()],
                        match_trailing_slash: true,
                    },
                    RoutePredicateConfig::Weight {
                        group: "g".to_string(),
                        weight: 1,
                    },
                ],
            },
            ReverseProxyRouteConfig {
                id: "b".to_string(),
                target: Some("http://b.example.com".to_string()),
                targets: Vec::new(),
                load_balancing: None,
                sticky: None,
                header_override: None,
                retry_policy: None,
                reverse_proxy_config: None,
                strip_path_prefix: None,
                priority: Some(0),
                predicates: vec![
                    RoutePredicateConfig::Path {
                        patterns: vec!["/**".to_string()],
                        match_trailing_slash: true,
                    },
                    RoutePredicateConfig::Weight {
                        group: "g".to_string(),
                        weight: 3,
                    },
                ],
            },
        ];
        let matcher = RouteMatcher::new(routes, 10, None).unwrap();
        let req = Request::builder()
            .method(Method::GET)
            .uri("/anything")
            .body(Empty::<Bytes>::new())
            .unwrap();
        let first = matcher
            .select_route(&req, &RequestContext { client_ip: None })
            .unwrap();
        assert!(first.id == "a" || first.id == "b");
    }

    #[test]
    fn test_header_override_group_selection_stays_in_group() {
        let mut allowed_groups = HashMap::new();
        allowed_groups.insert(
            "blue".to_string(),
            vec!["a".to_string(), "b".to_string()],
        );

        let routes = vec![ReverseProxyRouteConfig {
            id: "api".to_string(),
            target: None,
            targets: vec![
                ReverseProxyTargetConfig {
                    id: "a".to_string(),
                    url: "http://a.example.com".to_string(),
                    weight: 1,
                    enabled: true,
                },
                ReverseProxyTargetConfig {
                    id: "b".to_string(),
                    url: "http://b.example.com".to_string(),
                    weight: 1,
                    enabled: true,
                },
                ReverseProxyTargetConfig {
                    id: "c".to_string(),
                    url: "http://c.example.com".to_string(),
                    weight: 1,
                    enabled: true,
                },
            ],
            load_balancing: None,
            sticky: None,
            header_override: Some(HeaderOverrideConfig {
                header_name: "X-Bifrost-Target".to_string(),
                allowed_values: HashMap::new(),
                allowed_groups,
            }),
            reverse_proxy_config: None,
            strip_path_prefix: None,
            priority: Some(0),
            predicates: vec![RoutePredicateConfig::Path {
                patterns: vec!["/api/**".to_string()],
                match_trailing_slash: true,
            }],
            retry_policy: None,
        }];

        let matcher = RouteMatcher::new(routes, 10, None).unwrap();
        let req = Request::builder()
            .method(Method::GET)
            .uri("/api/users")
            .header("X-Bifrost-Target", "blue")
            .body(Empty::<Bytes>::new())
            .unwrap();

        let route = matcher
            .select_route(&req, &RequestContext { client_ip: None })
            .unwrap();
        let selection = route
            .select_target(&req, &RequestContext { client_ip: None })
            .unwrap();

        assert!(selection.target.id == "a" || selection.target.id == "b");
        assert_ne!(selection.target.id, "c");
    }

    #[test]
    fn test_header_override_group_unhealthy_falls_back() {
        let mut allowed_groups = HashMap::new();
        allowed_groups.insert("blue".to_string(), vec!["a".to_string()]);

        let routes = vec![ReverseProxyRouteConfig {
            id: "api".to_string(),
            target: None,
            targets: vec![
                ReverseProxyTargetConfig {
                    id: "a".to_string(),
                    url: "http://a.example.com".to_string(),
                    weight: 1,
                    enabled: true,
                },
                ReverseProxyTargetConfig {
                    id: "b".to_string(),
                    url: "http://b.example.com".to_string(),
                    weight: 1,
                    enabled: true,
                },
            ],
            load_balancing: None,
            sticky: None,
            header_override: Some(HeaderOverrideConfig {
                header_name: "X-Bifrost-Target".to_string(),
                allowed_values: HashMap::new(),
                allowed_groups,
            }),
            reverse_proxy_config: None,
            strip_path_prefix: None,
            priority: Some(0),
            predicates: vec![RoutePredicateConfig::Path {
                patterns: vec!["/api/**".to_string()],
                match_trailing_slash: true,
            }],
            retry_policy: None,
        }];

        let matcher = RouteMatcher::new(routes, 10, None).unwrap();
        let req = Request::builder()
            .method(Method::GET)
            .uri("/api/users")
            .header("X-Bifrost-Target", "blue")
            .body(Empty::<Bytes>::new())
            .unwrap();

        let route = matcher
            .select_route(&req, &RequestContext { client_ip: None })
            .unwrap();

        if let Some(target) = route.targets.iter().find(|t| t.id == "a") {
            target.healthy.store(false, Ordering::Relaxed);
        }

        let selection = route
            .select_target(&req, &RequestContext { client_ip: None })
            .unwrap();

        assert_eq!(selection.target.id, "b");
    }

    #[test]
    fn test_select_target_excludes_attempted() {
        let routes = vec![ReverseProxyRouteConfig {
            id: "api".to_string(),
            target: None,
            targets: vec![
                ReverseProxyTargetConfig {
                    id: "a".to_string(),
                    url: "http://a.example.com".to_string(),
                    weight: 1,
                    enabled: true,
                },
                ReverseProxyTargetConfig {
                    id: "b".to_string(),
                    url: "http://b.example.com".to_string(),
                    weight: 1,
                    enabled: true,
                },
            ],
            load_balancing: None,
            sticky: None,
            header_override: None,
            reverse_proxy_config: None,
            strip_path_prefix: None,
            priority: Some(0),
            predicates: vec![RoutePredicateConfig::Path {
                patterns: vec!["/api/**".to_string()],
                match_trailing_slash: true,
            }],
            retry_policy: None,
        }];

        let matcher = RouteMatcher::new(routes, 10, None).unwrap();
        let req = Request::builder()
            .method(Method::GET)
            .uri("/api/users")
            .body(Empty::<Bytes>::new())
            .unwrap();
        let route = matcher
            .select_route(&req, &RequestContext { client_ip: None })
            .unwrap();

        let mut excluded = HashSet::new();
        excluded.insert("a".to_string());

        let selection = route
            .select_target_with_exclusions(&req, &RequestContext { client_ip: None }, &excluded)
            .unwrap();

        assert_eq!(selection.target.id, "b");
    }

    #[test]
    fn test_retry_policy_rejects_invalid_method() {
        let routes = vec![ReverseProxyRouteConfig {
            id: "api".to_string(),
            target: Some("http://a.example.com".to_string()),
            targets: Vec::new(),
            load_balancing: None,
            sticky: None,
            header_override: None,
            reverse_proxy_config: None,
            strip_path_prefix: None,
            priority: Some(0),
            predicates: vec![RoutePredicateConfig::Path {
                patterns: vec!["/api/**".to_string()],
                match_trailing_slash: true,
            }],
            retry_policy: Some(RetryPolicyConfig {
                max_attempts: 2,
                retry_on_connect_error: true,
                retry_on_statuses: Vec::new(),
                methods: vec!["BAD METHOD".to_string()],
            }),
        }];

        let err = match RouteMatcher::new(routes, 10, None) {
            Ok(_) => panic!("expected config error"),
            Err(err) => err,
        };
        match err {
            ProxyError::Config(message) => {
                assert!(message.contains("retry policy method"));
            }
            _ => panic!("expected config error"),
        }
    }
}
