use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::PathBuf;

fn default_cache_millisecs() -> u64 {
    3600
}

fn default_monitoring_enabled() -> bool {
    true
}

fn default_metrics_endpoint() -> String {
    "/metrics".to_string()
}

fn default_health_endpoint() -> String {
    "/health".to_string()
}

fn default_status_endpoint() -> String {
    "/status".to_string()
}

fn default_monitoring_listen_addr() -> Option<SocketAddr> {
    "127.0.0.1:9900".parse().ok()
}

fn default_rate_limiting_enabled() -> bool {
    true
}

fn default_websocket_enabled() -> bool {
    true
}

fn default_websocket_allowed_origins() -> Vec<String> {
    vec!["*".to_string()]
}

fn default_websocket_timeout() -> u64 {
    300
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl Default for LogLevel {
    fn default() -> Self {
        LogLevel::Info
    }
}

impl std::fmt::Display for LogLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LogLevel::Trace => write!(f, "trace"),
            LogLevel::Debug => write!(f, "debug"),
            LogLevel::Info => write!(f, "info"),
            LogLevel::Warn => write!(f, "warn"),
            LogLevel::Error => write!(f, "error"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    Text,
    Json,
}

impl Default for LogFormat {
    fn default() -> Self {
        LogFormat::Text
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogOutputType {
    Stdout,
    File,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogTarget {
    #[serde(rename = "type")]
    pub output_type: LogOutputType,
    pub path: Option<PathBuf>,
    pub level: Option<LogLevel>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    pub level: Option<LogLevel>,
    pub format: Option<LogFormat>,
    pub targets: Option<Vec<LogTarget>>,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: Some(LogLevel::Info),
            format: Some(LogFormat::Text),
            targets: Some(vec![LogTarget {
                output_type: LogOutputType::Stdout,
                path: None,
                level: None,
            }]),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonitoringConfig {
    #[serde(default = "default_monitoring_enabled")]
    pub enabled: bool,
    #[serde(default = "default_metrics_endpoint")]
    pub metrics_endpoint: String,
    #[serde(default = "default_health_endpoint")]
    pub health_endpoint: String,
    #[serde(default = "default_status_endpoint")]
    pub status_endpoint: String,
    #[serde(default)]
    pub include_detailed_metrics: bool,
    #[serde(default = "default_monitoring_listen_addr")]
    pub listen_address: Option<SocketAddr>,
}

impl Default for MonitoringConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            metrics_endpoint: default_metrics_endpoint(),
            health_endpoint: default_health_endpoint(),
            status_endpoint: default_status_endpoint(),
            include_detailed_metrics: true,
            listen_address: default_monitoring_listen_addr(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitingConfig {
    #[serde(default = "default_rate_limiting_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub default_limit: Option<RateLimitWindowConfig>,
    #[serde(default)]
    pub rules: Vec<RateLimitRuleConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitWindowConfig {
    pub limit: u64,
    pub window_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitRuleConfig {
    pub id: String,
    pub limit: u64,
    pub window_secs: u64,
    #[serde(default)]
    pub path_prefix: Option<String>,
    #[serde(default)]
    pub methods: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebSocketConfig {
    #[serde(default = "default_websocket_enabled")]
    pub enabled: bool,
    #[serde(default = "default_websocket_allowed_origins")]
    pub allowed_origins: Vec<String>,
    #[serde(default)]
    pub supported_protocols: Vec<String>,
    #[serde(default = "default_websocket_timeout")]
    pub timeout_seconds: u64,
}

impl Default for WebSocketConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            allowed_origins: default_websocket_allowed_origins(),
            supported_protocols: Vec::new(),
            timeout_seconds: default_websocket_timeout(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ProxyMode {
    Forward,
    Reverse,
}

/// Health check configuration for reverse proxy connection pool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthCheckConfig {
    /// Health check interval in seconds (default: 30s)
    #[serde(default = "default_health_check_interval")]
    pub interval_secs: u64,
    /// Health check endpoint (e.g., "/health", "/ping")
    /// If not set, uses TCP connection check
    #[serde(default)]
    pub endpoint: Option<String>,
    /// Timeout for health check in seconds (default: 5s)
    #[serde(default = "default_health_check_timeout")]
    pub timeout_secs: u64,
}

fn default_health_check_interval() -> u64 {
    30
}

fn default_health_check_timeout() -> u64 {
    5
}

impl Default for HealthCheckConfig {
    fn default() -> Self {
        Self {
            interval_secs: 30,
            endpoint: None,
            timeout_secs: 5,
        }
    }
}

/// Reverse proxy specific configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReverseProxyConfig {
    /// Maximum idle connections to keep per backend host
    /// 0 = no pooling (create new connection per request)
    /// 1-50 = maintain connection pool for better performance
    /// Default: 10
    #[serde(default = "default_pool_max_idle_per_host")]
    pub pool_max_idle_per_host: usize,
    /// Pool idle timeout in seconds (how long to keep idle connections)
    /// Default: 90s
    #[serde(default = "default_pool_idle_timeout")]
    pub pool_idle_timeout_secs: u64,
    /// Health check configuration (optional)
    #[serde(default)]
    pub health_check: Option<HealthCheckConfig>,
}

fn default_pool_max_idle_per_host() -> usize {
    10
}

fn default_pool_idle_timeout() -> u64 {
    90
}

impl Default for ReverseProxyConfig {
    fn default() -> Self {
        Self {
            pool_max_idle_per_host: 10,
            pool_idle_timeout_secs: 90,
            health_check: None,
        }
    }
}

fn default_target_weight() -> u32 {
    1
}

fn default_target_enabled() -> bool {
    true
}

/// Reverse proxy target configuration for multi-target routing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReverseProxyTargetConfig {
    /// Unique target id (within the route)
    pub id: String,
    /// Upstream target URL
    pub url: String,
    /// Optional weight for weighted routing (>= 1)
    #[serde(default = "default_target_weight")]
    pub weight: u32,
    /// Optional flag to disable the target
    #[serde(default = "default_target_enabled")]
    pub enabled: bool,
}

/// Load balancing configuration for multi-target routing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoadBalancingConfig {
    #[serde(default)]
    pub policy: LoadBalancingPolicy,
}

impl Default for LoadBalancingConfig {
    fn default() -> Self {
        Self {
            policy: LoadBalancingPolicy::RoundRobin,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoadBalancingPolicy {
    RoundRobin,
    WeightedRoundRobin,
    LeastConnections,
    Random,
}

impl Default for LoadBalancingPolicy {
    fn default() -> Self {
        LoadBalancingPolicy::RoundRobin
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StickyMode {
    Cookie,
    Header,
    SourceIp,
}

/// Sticky session configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StickyConfig {
    pub mode: StickyMode,
    #[serde(default)]
    pub cookie_name: Option<String>,
    #[serde(default)]
    pub header_name: Option<String>,
    #[serde(default)]
    pub ttl_seconds: Option<u64>,
}

/// Header override routing configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeaderOverrideConfig {
    pub header_name: String,
    #[serde(default)]
    pub allowed_values: std::collections::HashMap<String, String>,
    #[serde(default)]
    pub allowed_groups: std::collections::HashMap<String, Vec<String>>,
}

fn default_retry_max_attempts() -> u32 {
    1
}

fn default_retry_on_connect_error() -> bool {
    true
}

fn default_retry_methods() -> Vec<String> {
    vec![
        "GET".to_string(),
        "HEAD".to_string(),
        "OPTIONS".to_string(),
    ]
}

/// Retry policy for reverse proxy routes
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryPolicyConfig {
    /// Total attempts including the initial request
    #[serde(default = "default_retry_max_attempts")]
    pub max_attempts: u32,
    /// Retry when connection errors occur before a response is received
    #[serde(default = "default_retry_on_connect_error")]
    pub retry_on_connect_error: bool,
    /// Retry when the upstream responds with one of these status codes
    #[serde(default)]
    pub retry_on_statuses: Vec<u16>,
    /// Allowed HTTP methods for retries (defaults to safe methods)
    #[serde(default = "default_retry_methods")]
    pub methods: Vec<String>,
}

/// Reverse proxy route configuration supporting multiple targets and predicates
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReverseProxyRouteConfig {
    /// Unique route id
    pub id: String,
    /// Upstream target URL
    #[serde(default)]
    pub target: Option<String>,
    /// Multi-target configuration (preferred when set)
    #[serde(default)]
    pub targets: Vec<ReverseProxyTargetConfig>,
    /// Optional load balancing policy for multi-target routing
    #[serde(default)]
    pub load_balancing: Option<LoadBalancingConfig>,
    /// Optional sticky session configuration
    #[serde(default)]
    pub sticky: Option<StickyConfig>,
    /// Optional header override routing
    #[serde(default)]
    pub header_override: Option<HeaderOverrideConfig>,
    /// Optional retry policy for upstream failures
    #[serde(default)]
    pub retry_policy: Option<RetryPolicyConfig>,
    /// Optional reverse proxy connection config for this route
    #[serde(default)]
    pub reverse_proxy_config: Option<ReverseProxyConfig>,
    /// Optional path prefix to strip before forwarding (e.g., "/test" -> "/api")
    #[serde(default)]
    pub strip_path_prefix: Option<String>,
    /// Optional priority (lower number = higher priority). Defaults to 0.
    #[serde(default)]
    pub priority: Option<i32>,
    /// Predicate list (logical AND). Empty list is invalid.
    #[serde(default)]
    pub predicates: Vec<RoutePredicateConfig>,
}

/// Predicate configuration for reverse proxy routing
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum RoutePredicateConfig {
    /// Path matching using Ant-style patterns (supports ** and *)
    Path {
        patterns: Vec<String>,
        #[serde(default)]
        match_trailing_slash: bool,
    },
    /// Host header matching with Ant-style patterns
    Host {
        patterns: Vec<String>,
    },
    /// Allowed HTTP methods
    Method {
        methods: Vec<String>,
    },
    /// Header match by exact value or regex
    Header {
        name: String,
        #[serde(default)]
        value: Option<String>,
        #[serde(default)]
        regex: Option<String>,
    },
    /// Query param match by presence, exact value, or regex
    Query {
        name: String,
        #[serde(default)]
        value: Option<String>,
        #[serde(default)]
        regex: Option<String>,
    },
    /// Cookie match by name with optional exact value or regex
    Cookie {
        name: String,
        #[serde(default)]
        value: Option<String>,
        #[serde(default)]
        regex: Option<String>,
    },
    /// Time-based predicates
    After { instant: String },
    Before { instant: String },
    Between { start: String, end: String },
    /// Remote address in CIDR ranges
    RemoteAddr { cidrs: Vec<String> },
    /// Weighted routing participation
    Weight { group: String, weight: u32 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StaticMount {
    pub path: String,        // URL path prefix (e.g., "/app", "/api", "/assets")
    pub root_dir: String,     // Filesystem directory path
    #[serde(default)]
    pub enable_directory_listing: Option<bool>,
    #[serde(default)]
    pub index_files: Option<Vec<String>>,
    #[serde(default)]
    pub spa_mode: Option<bool>,
    #[serde(default)]
    pub spa_fallback_file: Option<String>,
    #[serde(default)]
    pub no_cache_files: Option<Vec<String>>,
    #[serde(default)]
    pub cache_millisecs: Option<u64>,
}

impl StaticMount {
    pub fn resolve_inheritance(&self, parent_config: &StaticFileConfig) -> ResolvedStaticMount {
        ResolvedStaticMount {
            path: self.path.clone(),
            root_dir: self.root_dir.clone(),
            enable_directory_listing: self.enable_directory_listing
                .unwrap_or(parent_config.enable_directory_listing),
            index_files: self.index_files
                .clone()
                .unwrap_or_else(|| parent_config.index_files.clone()),
            spa_mode: self.spa_mode
                .unwrap_or(parent_config.spa_mode),
            spa_fallback_file: self.spa_fallback_file
                .clone()
                .unwrap_or_else(|| parent_config.spa_fallback_file.clone()),
            no_cache_files: self.no_cache_files
                .clone()
                .unwrap_or_else(|| parent_config.no_cache_files.clone()),
            cache_millisecs: self.cache_millisecs
                .unwrap_or(parent_config.cache_millisecs),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedStaticMount {
    pub path: String,
    pub root_dir: String,
    pub enable_directory_listing: bool,
    pub index_files: Vec<String>,
    pub spa_mode: bool,
    pub spa_fallback_file: String,
    pub no_cache_files: Vec<String>,
    pub cache_millisecs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StaticFileConfig {
    pub mounts: Vec<StaticMount>,
    pub enable_directory_listing: bool,
    pub index_files: Vec<String>,
    pub spa_mode: bool,
    pub spa_fallback_file: String,
    pub worker_threads: Option<usize>,
    #[serde(default)]
    pub custom_mime_types: std::collections::HashMap<String, String>,
    #[serde(default)]
    pub no_cache_files: Vec<String>,
    #[serde(default = "default_cache_millisecs")]
    pub cache_millisecs: u64,
}

// For backward compatibility
impl Default for StaticFileConfig {
    fn default() -> Self {
        Self {
            mounts: vec![StaticMount {
                path: "/".to_string(),
                root_dir: "./public".to_string(),
                enable_directory_listing: None, // Will inherit from parent
                index_files: None, // Will inherit from parent
                spa_mode: None, // Will inherit from parent
                spa_fallback_file: None, // Will inherit from parent
                no_cache_files: None, // Will inherit from parent
                cache_millisecs: None, // Will inherit from parent
            }],
            enable_directory_listing: false,
            index_files: vec!["index.html".to_string(), "index.htm".to_string()],
            spa_mode: false,
            spa_fallback_file: "index.html".to_string(),
            worker_threads: None,
            custom_mime_types: std::collections::HashMap::new(),
            no_cache_files: vec![],
            cache_millisecs: 3600,
        }
    }
}

impl StaticFileConfig {
    pub fn single(root_dir: String, spa_mode: bool) -> Self {
        Self {
            mounts: vec![StaticMount {
                path: "/".to_string(),
                root_dir,
                enable_directory_listing: None, // Will inherit from parent
                index_files: None, // Will inherit from parent
                spa_mode: Some(spa_mode), // Override SPA mode
                spa_fallback_file: None, // Will inherit from parent
                no_cache_files: None, // Will inherit from parent
                cache_millisecs: None, // Will inherit from parent
            }],
            enable_directory_listing: false,
            index_files: vec!["index.html".to_string(), "index.htm".to_string()],
            spa_mode,
            spa_fallback_file: "index.html".to_string(),
            worker_threads: None,
            custom_mime_types: std::collections::HashMap::new(),
            no_cache_files: vec![],
            cache_millisecs: 3600,
        }
    }

    pub fn add_mount(&mut self, path: String, root_dir: String, spa_mode: bool) {
        self.mounts.push(StaticMount {
            path,
            root_dir,
            enable_directory_listing: None, // Will inherit from parent
            index_files: None, // Will inherit from parent
            spa_mode: Some(spa_mode), // Override SPA mode
            spa_fallback_file: None, // Will inherit from parent
            no_cache_files: None, // Will inherit from parent
            cache_millisecs: None, // Will inherit from parent
        });
    }

    pub fn add_custom_mime_type(&mut self, extension: String, mime_type: String) {
        // Remove leading dot if present
        let clean_ext = extension.strip_prefix('.').unwrap_or(&extension).to_lowercase();
        self.custom_mime_types.insert(clean_ext, mime_type);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelayProxyConfig {
    pub relay_proxy_url: String,
    #[serde(default)]
    pub relay_proxy_username: Option<String>,
    #[serde(default)]
    pub relay_proxy_password: Option<String>,
    // Domain patterns in NO_PROXY format
    // Supports: "example.com", ".example.com", "*.example.com", "subdomain.example.com"
    #[serde(default)]
    pub relay_proxy_domains: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub mode: ProxyMode,
    pub listen_addr: SocketAddr,
    pub reverse_proxy_target: Option<String>,
    #[serde(default)]
    pub reverse_proxy_routes: Vec<ReverseProxyRouteConfig>,
    pub max_connections: Option<usize>,
    // New timeout configurations
    #[serde(default)]
    pub connect_timeout_secs: Option<u64>,
    #[serde(default)]
    pub idle_timeout_secs: Option<u64>,
    #[serde(default)]
    pub max_connection_lifetime_secs: Option<u64>,
    // Legacy timeout field for backward compatibility
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    // Worker threads for reverse proxy and static file serving (shared)
    #[serde(default)]
    pub worker_threads: Option<usize>,
    pub static_files: Option<StaticFileConfig>,
    #[serde(default)]
    pub private_key: Option<String>,
    #[serde(default)]
    pub certificate: Option<String>,
    #[serde(default)]
    pub connection_pool_enabled: Option<bool>,
    #[serde(default = "default_max_header_size")]
    pub max_header_size: Option<usize>,
    // Multiple relay proxy configurations
    #[serde(default)]
    pub relay_proxies: Option<Vec<RelayProxyConfig>>,
    // Legacy single relay proxy fields (deprecated, use relay_proxies instead)
    #[serde(default)]
    pub relay_proxy_url: Option<String>,
    #[serde(default)]
    pub relay_proxy_username: Option<String>,
    #[serde(default)]
    pub relay_proxy_password: Option<String>,
    #[serde(default)]
    pub relay_proxy_domain_suffixes: Option<Vec<String>>,
    // Basic authentication for forward proxy
    #[serde(default)]
    pub proxy_username: Option<String>,
    #[serde(default)]
    pub proxy_password: Option<String>,
    // Reverse proxy specific configuration
    #[serde(default)]
    pub reverse_proxy_config: Option<ReverseProxyConfig>,
    // Logging configuration
    #[serde(default)]
    pub logging: Option<LoggingConfig>,
    #[serde(default)]
    pub monitoring: MonitoringConfig,
    #[serde(default)]
    pub websocket: Option<WebSocketConfig>,
    #[serde(default)]
    pub rate_limiting: Option<RateLimitingConfig>,
}

fn default_max_header_size() -> Option<usize> {
    Some(16 * 1024) // 16KB default header size limit
}

impl Default for Config {
    fn default() -> Self {
        Self {
            mode: ProxyMode::Forward,
            listen_addr: "127.0.0.1:8080".parse().unwrap(),
            reverse_proxy_target: None,
            reverse_proxy_routes: Vec::new(),
            max_connections: Some(1000),
            connect_timeout_secs: Some(10),
            idle_timeout_secs: Some(90),
            max_connection_lifetime_secs: Some(300),
            timeout_secs: None,
            worker_threads: None,
            static_files: None,
            private_key: None,
            certificate: None,
            connection_pool_enabled: Some(true),
            max_header_size: default_max_header_size(),
            relay_proxies: None,
            relay_proxy_url: None,
            relay_proxy_username: None,
            relay_proxy_password: None,
            relay_proxy_domain_suffixes: None,
            proxy_username: None,
            proxy_password: None,
            reverse_proxy_config: None,
            logging: None,
            monitoring: MonitoringConfig::default(),
            websocket: None,
            rate_limiting: None,
        }
    }
}

impl Config {
    pub fn from_file(path: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let content = std::fs::read_to_string(path)?;
        let config: Config = serde_json::from_str(&content)?;
        Ok(config)
    }

    pub fn to_file(&self, path: &str) -> Result<(), Box<dyn std::error::Error>> {
        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }
}
