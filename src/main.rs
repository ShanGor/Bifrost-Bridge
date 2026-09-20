use bifrost_bridge::{
    config::{Config, ProxyMode},
    logging,
    proxy::ProxyFactory,
    secrets::{SecretManager, config_has_encrypted_values},
};
use clap::Parser;
use log::{error, info, warn};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::signal;
use tokio_util::sync::CancellationToken;

const ENCRYPT_STDIN_PLACEHOLDER: &str = "__BIFROST_STDIN__";

#[derive(Parser)]
#[clap(
    version = env!("CARGO_PKG_VERSION"),
    author = "Rust Proxy Server",
    about = "A Rust proxy server that can function as both forward and reverse proxy"
)]
struct Args {
    #[clap(
        short,
        long,
        value_name = "MODE",
        help = "Proxy mode: forward or reverse"
    )]
    mode: Option<String>,

    #[clap(
        short,
        long,
        value_name = "ADDR",
        help = "Listen address (e.g., 127.0.0.1:8080)"
    )]
    listen: Option<String>,

    #[clap(
        short,
        long,
        value_name = "URL",
        help = "Target URL for reverse proxy (e.g., http://backend:3000)"
    )]
    target: Option<String>,

    #[clap(short, long, value_name = "FILE", help = "Configuration file path")]
    config: Option<String>,

    #[clap(
        long,
        help = "Reload the running server configuration and TLS material"
    )]
    reload: bool,

    #[clap(
        long,
        value_name = "FILE",
        help = "PID file used by the server and --reload (default: bifrost-bridge.pid)"
    )]
    pid_file: Option<PathBuf>,

    #[clap(long, value_name = "SECONDS", help = "Connection timeout in seconds")]
    connect_timeout: Option<u64>,

    #[clap(long, value_name = "SECONDS", help = "Idle timeout in seconds")]
    idle_timeout: Option<u64>,

    #[clap(
        long,
        value_name = "SECONDS",
        help = "Maximum connection lifetime in seconds"
    )]
    max_connection_lifetime: Option<u64>,

    #[clap(
        long,
        value_name = "SECONDS",
        help = "Request timeout in seconds (deprecated, use specific timeout options)"
    )]
    timeout: Option<u64>,

    #[clap(
        long,
        value_name = "FILE",
        help = "Generate a sample configuration file"
    )]
    generate_config: Option<String>,

    #[clap(
        long,
        value_name = "DIR",
        help = "Serve static files from this directory"
    )]
    static_dir: Option<String>,

    #[clap(
        long,
        value_name = "PATH:DIR",
        help = "Mount static files from PATH to DIR (can be used multiple times)"
    )]
    mount: Vec<String>,

    #[clap(long, help = "Enable SPA mode")]
    spa: bool,

    #[clap(
        long,
        value_name = "FILE",
        help = "SPA fallback file name (default: index.html)"
    )]
    spa_fallback: Option<String>,

    #[clap(
        long,
        value_name = "NUM",
        help = "Number of worker threads for reverse proxy and static file serving (shared)"
    )]
    worker_threads: Option<usize>,

    #[clap(
        long,
        value_name = "EXT:MIME",
        help = "Custom MIME type mapping (e.g., mjs:application/javascript), can be used multiple times"
    )]
    mime_type: Vec<String>,

    #[clap(long, value_name = "FILE", help = "Private key file path for HTTPS")]
    private_key: Option<String>,

    #[clap(long, value_name = "FILE", help = "Certificate file path for HTTPS")]
    certificate: Option<String>,

    #[clap(long, help = "Disable connection pooling (no-pool mode)")]
    no_connection_pool: bool,

    #[clap(
        long,
        value_name = "NUM",
        help = "Maximum idle connections per host for connection pooling"
    )]
    pool_max_idle: Option<usize>,

    #[clap(long, value_name = "BYTES", help = "Maximum HTTP header size in bytes")]
    max_header_size: Option<usize>,

    #[clap(
        long,
        value_name = "USERNAME",
        help = "Username for proxy authentication (Basic Auth)"
    )]
    proxy_username: Option<String>,

    #[clap(
        long,
        value_name = "PASSWORD",
        help = "Password for proxy authentication (Basic Auth)"
    )]
    proxy_password: Option<String>,

    #[clap(
        long,
        value_name = "LEVEL",
        help = "Set logging level (trace, debug, info, warn, error)"
    )]
    log_level: Option<String>,

    #[clap(
        long,
        value_name = "FORMAT",
        help = "Set log output format (text, json)"
    )]
    log_format: Option<String>,

    #[clap(long, help = "Initialize the ~/.bifrost encryption key and exit")]
    init_encryption_key: bool,

    #[clap(
        long,
        value_name = "PAYLOAD",
        num_args = 0..=1,
        default_missing_value = ENCRYPT_STDIN_PLACEHOLDER,
        help = "Encrypt a secret payload; omit PAYLOAD to read from stdin"
    )]
    encrypt: Option<String>,
}

fn init_logging_from_config(
    config: &Config,
    args: Option<&Args>,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(logging_config) = &config.logging {
        // Use advanced logging configuration from file
        logging::CustomLogger::init(logging_config.clone())?;
        info!(
            "Initialized advanced logging system with {} targets",
            logging_config
                .targets
                .as_ref()
                .map(|t| t.len())
                .unwrap_or(0)
        );
    } else {
        // Fallback to CLI arguments or defaults
        let args = args.expect("Args required when no logging config provided");
        init_logging_from_args(args)?;
    }
    Ok(())
}

fn init_logging_from_args(args: &Args) -> Result<(), Box<dyn std::error::Error>> {
    let log_level = args.log_level.as_deref();
    let log_format = args.log_format.as_deref();

    // Use simple env_logger with CLI arguments
    logging::init_fallback(log_level, log_format)?;

    info!(
        "Initialized logging system - level: {}, format: {}",
        log_level.unwrap_or("info"),
        log_format.unwrap_or("text")
    );
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let pid_file = args
        .pid_file
        .clone()
        .unwrap_or_else(|| PathBuf::from("bifrost-bridge.pid"));

    // --reload is an operator command, not a second server instance. It only
    // needs the PID file and therefore must run before config/logging setup.
    if args.reload {
        return request_reload(&pid_file);
    }

    // Initialize logging based on configuration
    if let Some(config_file) = &args.config {
        // Load configuration first to get logging settings
        let config = Config::from_file(config_file)?;
        init_logging_from_config(&config, Some(&args))?;
    } else {
        // Use CLI arguments for logging configuration
        init_logging_from_args(&args)?;
    }

    if args.init_encryption_key && args.encrypt.is_some() {
        return Err("Cannot use --init-encryption-key and --encrypt simultaneously".into());
    }

    if args.init_encryption_key {
        let manager = SecretManager::new()?;
        manager.init_encryption_key(false)?;
        return Ok(());
    }

    if let Some(payload_spec) = &args.encrypt {
        let manager = SecretManager::new()?;
        let payload = if payload_spec == ENCRYPT_STDIN_PLACEHOLDER {
            read_secret_from_stdin()?
        } else {
            payload_spec.clone().into_bytes()
        };
        if payload.is_empty() {
            return Err("Secret payload cannot be empty".into());
        }
        let token = manager.encrypt_payload(&payload)?;
        println!("{}", token);
        return Ok(());
    }

    // Handle generate-config flag
    if let Some(config_file) = args.generate_config {
        generate_sample_config(&config_file)?;
        info!("Sample configuration file generated: {}", config_file);
        return Ok(());
    }

    // Load configuration
    let mut config = if let Some(config_file) = &args.config {
        if !Path::new(config_file).exists() {
            return Err(format!("Configuration file not found: {}", config_file).into());
        }
        Config::from_file(config_file)?
    } else {
        create_config_from_args(&args)?
    };

    if config_has_encrypted_values(&config) {
        let manager = SecretManager::new()?;
        manager.apply_to_config(&mut config)?;
    }

    // Validate configuration
    validate_config(&config)?;
    validate_tls_pair(&config)?;

    let pid_guard = PidFileGuard::acquire(&pid_file)?;

    // Create tokio runtime with custom thread pool if configured
    // Priority: static_files.worker_threads > top-level worker_threads > default
    let worker_threads = config
        .static_files
        .as_ref()
        .and_then(|sf| sf.worker_threads)
        .or(config.worker_threads);

    let runtime = if let Some(worker_threads) = worker_threads {
        info!(
            "Starting tokio runtime with {} worker threads (shared for reverse proxy and static files)",
            worker_threads
        );
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(worker_threads)
            .enable_all()
            .build()?
    } else {
        info!("Starting tokio runtime with default worker threads (CPU cores)");
        tokio::runtime::Runtime::new()?
    };

    // Run the async main function in the configured runtime
    let result = runtime.block_on(async_main(config, args.config.clone()));
    drop(pid_guard);
    result
}

async fn async_main(
    config: Config,
    config_path: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    info!("Starting proxy server...");
    let mut signal_waiters = SignalWaiters::new()?;

    // The supervisor owns this socket for the entire process lifetime. Worker
    // generations only borrow it, so a reload never has to re-bind the port.
    let listener = Arc::new(TcpListener::bind(config.listen_addr).await?);
    let mut active_config = config.clone();
    let mut worker_shutdown = CancellationToken::new();
    let mut worker_handle = spawn_worker(
        ProxyFactory::create_proxy(config)?,
        listener.clone(),
        worker_shutdown.clone(),
    );
    let SignalWaiters { reload, terminate } = &mut signal_waiters;

    loop {
        tokio::select! {
            _ = signal::ctrl_c() => {
                info!("\n🛑 Received Ctrl+C, shutting down gracefully...");
                worker_shutdown.cancel();
                let _ = worker_handle.await;
                break;
            }
            _ = terminate.recv() => {
                info!("🛑 Received SIGTERM, shutting down gracefully...");
                worker_shutdown.cancel();
                let _ = worker_handle.await;
                break;
            }
            _ = reload.recv() => {
                let Some(config_path) = config_path.as_deref() else {
                    warn!("Ignoring reload request: --reload requires a server started with --config");
                    continue;
                };

                match load_runtime_config(config_path) {
                    Ok(new_config) => {
                        if new_config.listen_addr != active_config.listen_addr {
                            warn!(
                                "Ignoring reload: listen_addr cannot change while the supervisor owns the active listener ({} -> {})",
                                active_config.listen_addr,
                                new_config.listen_addr
                            );
                            continue;
                        }

                        if effective_worker_threads(&new_config) != effective_worker_threads(&active_config) {
                            warn!("Ignoring reload: worker thread count is fixed for the lifetime of the Tokio runtime; restart to change it");
                            continue;
                        }

                        let new_proxy = match ProxyFactory::create_proxy(new_config.clone()) {
                            Ok(proxy) => proxy,
                            Err(err) => {
                                error!("Rejected configuration reload: {}", err);
                                continue;
                            }
                        };

                        info!("Reloading configuration without closing the listening socket");
                        worker_shutdown.cancel();
                        if let Err(err) = worker_handle.await {
                            error!("Previous worker generation failed during reload: {}", err);
                        }

                        active_config = new_config;
                        worker_shutdown = CancellationToken::new();
                        worker_handle = spawn_worker(
                            new_proxy,
                            listener.clone(),
                            worker_shutdown.clone(),
                        );
                        info!("Configuration reload complete; new connections use the new generation");
                    }
                    Err(err) => error!("Rejected configuration reload: {}", err),
                }
            }
            result = &mut worker_handle => {
                match result {
                    Ok(Ok(())) => info!("Proxy worker stopped"),
                    Ok(Err(err)) => error!("Server error: {}", err),
                    Err(err) => error!("Server task error: {}", err),
                }
                break;
            }
        }
    }

    info!("👋 Proxy server stopped. Goodbye!");
    Ok(())
}

fn spawn_worker(
    proxy: Box<dyn bifrost_bridge::proxy::Proxy + Send>,
    listener: Arc<TcpListener>,
    shutdown: CancellationToken,
) -> tokio::task::JoinHandle<Result<(), bifrost_bridge::ProxyError>> {
    tokio::spawn(async move { proxy.run(listener, shutdown).await })
}

fn load_runtime_config(path: &str) -> Result<Config, Box<dyn std::error::Error>> {
    let mut config = Config::from_file(path)?;
    if config_has_encrypted_values(&config) {
        let manager = SecretManager::new()?;
        manager.apply_to_config(&mut config)?;
    }
    validate_config(&config)?;
    validate_tls_pair(&config)?;
    Ok(config)
}

fn effective_worker_threads(config: &Config) -> Option<usize> {
    config
        .static_files
        .as_ref()
        .and_then(|static_files| static_files.worker_threads)
        .or(config.worker_threads)
}

fn validate_tls_pair(config: &Config) -> Result<(), Box<dyn std::error::Error>> {
    match (&config.private_key, &config.certificate) {
        (Some(_), Some(_)) => {}
        (None, None) => {}
        _ => return Err("Both private_key and certificate must be configured together".into()),
    }
    Ok(())
}

#[cfg(unix)]
fn request_reload(pid_file: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let pid: libc::pid_t = std::fs::read_to_string(pid_file)
        .map_err(|err| format!("Cannot read PID file {}: {}", pid_file.display(), err))?
        .trim()
        .parse()
        .map_err(|err| format!("Invalid PID file {}: {}", pid_file.display(), err))?;
    if pid <= 1 {
        return Err(format!("Refusing to signal unsafe PID {} from {}", pid, pid_file.display()).into());
    }

    let result = unsafe { libc::kill(pid, libc::SIGHUP) };
    if result != 0 {
        return Err(format!(
            "Cannot send reload signal to PID {}: {}",
            pid,
            std::io::Error::last_os_error()
        )
        .into());
    }

    println!("Reload signal sent to bifrost-bridge PID {}", pid);
    Ok(())
}

#[cfg(not(unix))]
fn request_reload(_pid_file: &Path) -> Result<(), Box<dyn std::error::Error>> {
    Err("--reload is currently supported on Unix platforms only".into())
}

struct PidFileGuard {
    path: PathBuf,
    pid: u32,
}

impl PidFileGuard {
    fn acquire(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let pid = std::process::id();
        if let Ok(existing) = std::fs::read_to_string(path) {
            if let Ok(existing_pid) = existing.trim().parse::<i32>() {
                #[cfg(unix)]
                {
                    let alive = existing_pid > 1
                        && (unsafe { libc::kill(existing_pid, 0) == 0 }
                            || std::io::Error::last_os_error().raw_os_error()
                                == Some(libc::EPERM));
                    if alive {
                        return Err(format!(
                            "Another bifrost-bridge process is using {}",
                            path.display()
                        )
                        .into());
                    }
                }
            }
        }
        std::fs::write(path, pid.to_string())?;
        Ok(Self {
            path: path.to_path_buf(),
            pid,
        })
    }
}

impl Drop for PidFileGuard {
    fn drop(&mut self) {
        if std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|value| value.trim().parse::<u32>().ok())
            == Some(self.pid)
        {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

struct SignalWaiters {
    reload: SignalStream,
    terminate: SignalStream,
}

enum SignalStream {
    #[cfg(unix)]
    Unix(signal::unix::Signal),
    #[cfg(not(unix))]
    Disabled,
}

impl SignalStream {
    async fn recv(&mut self) {
        match self {
            #[cfg(unix)]
            SignalStream::Unix(signal) => {
                let _ = signal.recv().await;
            }
            #[cfg(not(unix))]
            SignalStream::Disabled => std::future::pending::<()>().await,
        }
    }
}

impl SignalWaiters {
    fn new() -> Result<Self, std::io::Error> {
        #[cfg(unix)]
        {
            Ok(Self {
                reload: SignalStream::Unix(signal::unix::signal(
                    signal::unix::SignalKind::hangup(),
                )?),
                terminate: SignalStream::Unix(signal::unix::signal(
                    signal::unix::SignalKind::terminate(),
                )?),
            })
        }
        #[cfg(not(unix))]
        {
            Ok(Self {
                reload: SignalStream::Disabled,
                terminate: SignalStream::Disabled,
            })
        }
    }
}

fn read_secret_from_stdin() -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut buffer = Vec::new();
    std::io::stdin().read_to_end(&mut buffer)?;
    while matches!(buffer.last(), Some(b) if *b == b'\n' || *b == b'\r') {
        buffer.pop();
    }
    if buffer.is_empty() {
        return Err("Secret payload cannot be empty".into());
    }
    Ok(buffer)
}

fn generate_sample_config(file_path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let sample_forward = r#"{
  "mode": "Forward",
  "listen_addr": "127.0.0.1:8080",
  "max_connections": 1000,
  "connect_timeout_secs": 10,
  "idle_timeout_secs": 90,
  "max_connection_lifetime_secs": 300,
  "max_header_size": 16384,
  "connection_pool_enabled": true
}"#;

    let sample_reverse = r#"{
  "mode": "Reverse",
  "listen_addr": "127.0.0.1:8080",
  "reverse_proxy_target": "http://backend.example.com:3000",
  "max_connections": 1000,
  "connect_timeout_secs": 10,
  "idle_timeout_secs": 90,
  "max_connection_lifetime_secs": 300,
  "max_header_size": 16384,
  "worker_threads": 4
}"#;

    let sample_spa = r#"{
  "mode": "Reverse",
  "listen_addr": "127.0.0.1:8080",
  "max_connections": 1000,
  "connect_timeout_secs": 10,
  "idle_timeout_secs": 90,
  "max_connection_lifetime_secs": 300,
  "max_header_size": 16384,
  "static_files": {
    "mounts": [{
      "path": "/",
      "root_dir": "./dist",
      "spa_mode": true
    }],
    "enable_directory_listing": false,
    "index_files": ["index.html"],
    "spa_mode": true,
    "spa_fallback_file": "index.html"
  }
}"#;

    let path = Path::new(file_path);
    let extension = path.extension().and_then(|s| s.to_str());

    let content = match extension {
        Some("forward") | Some("fwd") => sample_forward,
        Some("spa") => sample_spa,
        _ => sample_reverse,
    };

    std::fs::write(file_path, content)?;
    Ok(())
}

fn create_config_from_args(args: &Args) -> Result<Config, Box<dyn std::error::Error>> {
    let mode_str = args.mode.as_deref().unwrap_or("forward");
    let mode = match mode_str {
        "forward" => ProxyMode::Forward,
        "reverse" => ProxyMode::Reverse,
        _ => return Err("Invalid mode. Use 'forward' or 'reverse'".into()),
    };

    let listen_addr = args.listen.as_deref().unwrap_or("127.0.0.1:8080");
    let listen_addr: std::net::SocketAddr = listen_addr.parse()?;

    let mut config = Config {
        mode,
        listen_addr,
        reverse_proxy_target: args.target.clone(),
        reverse_proxy_routes: Vec::new(),
        max_connections: Some(1000),
        connect_timeout_secs: args.connect_timeout,
        idle_timeout_secs: args.idle_timeout,
        max_connection_lifetime_secs: args.max_connection_lifetime,
        timeout_secs: args.timeout,
        worker_threads: args.worker_threads,
        static_files: None,
        private_key: args.private_key.clone(),
        certificate: args.certificate.clone(),
        connection_pool_enabled: Some(!args.no_connection_pool),
        max_header_size: args.max_header_size,
        relay_proxies: None,
        relay_proxy_url: None,
        relay_proxy_username: None,
        relay_proxy_password: None,
        relay_proxy_domain_suffixes: None,
        proxy_username: args.proxy_username.clone(),
        proxy_password: args.proxy_password.clone(),
        reverse_proxy_config: None,
        logging: None,
        monitoring: bifrost_bridge::config::MonitoringConfig::default(),
        websocket: None,
        rate_limiting: None,
        plugin_runtime: bifrost_bridge::config::PluginRuntimeConfig::default(),
    };

    // Configure static files if specified
    if args.static_dir.is_some() || !args.mount.is_empty() {
        let mut static_config = if let Some(static_dir) = &args.static_dir {
            // Single directory mode (backward compatibility)
            let mut config =
                bifrost_bridge::config::StaticFileConfig::single(static_dir.clone(), args.spa);
            config.worker_threads = args.worker_threads;
            config.custom_mime_types = std::collections::HashMap::new();
            config
        } else {
            // Multiple mounts mode
            bifrost_bridge::config::StaticFileConfig {
                mounts: Vec::new(),
                enable_directory_listing: false,
                index_files: vec!["index.html".to_string(), "index.htm".to_string()],
                spa_mode: args.spa,
                spa_fallback_file: args
                    .spa_fallback
                    .clone()
                    .unwrap_or_else(|| "index.html".to_string()),
                worker_threads: args.worker_threads,
                custom_mime_types: std::collections::HashMap::new(),
                no_cache_files: vec![],
                cache_millisecs: 3600,
            }
        };

        // Process mounts
        for mount_spec in &args.mount {
            let parts: Vec<&str> = mount_spec.splitn(2, ':').collect();
            if parts.len() != 2 {
                return Err(format!(
                    "Invalid mount specification: '{}'. Use format 'PATH:DIR'",
                    mount_spec
                )
                .into());
            }

            let path = parts[0].trim();
            let dir = parts[1].trim();

            // Ensure path starts with /
            let normalized_path = if !path.starts_with('/') {
                format!("/{}", path)
            } else {
                path.to_string()
            };

            static_config.add_mount(normalized_path, dir.to_string(), args.spa);
        }

        // Process custom MIME types
        for mime_spec in &args.mime_type {
            let parts: Vec<&str> = mime_spec.splitn(2, ':').collect();
            if parts.len() != 2 {
                return Err(format!(
                    "Invalid MIME type specification: '{}'. Use format 'EXT:MIME'",
                    mime_spec
                )
                .into());
            }

            let extension = parts[0].trim();
            let mime_type = parts[1].trim();
            static_config.add_custom_mime_type(extension.to_string(), mime_type.to_string());
        }

        config.static_files = Some(static_config);
    }

    Ok(config)
}

fn validate_config(config: &Config) -> Result<(), Box<dyn std::error::Error>> {
    match config.mode {
        ProxyMode::Reverse => {
            let has_target = config.reverse_proxy_target.is_some();
            let has_routes = !config.reverse_proxy_routes.is_empty();
            info!(
                "Reverse proxy validation: target_present={}, routes_count={}, static_files={}",
                has_target,
                config.reverse_proxy_routes.len(),
                config.static_files.is_some()
            );
            if !has_target && !has_routes && config.static_files.is_none() {
                return Err("Reverse proxy mode requires either a target URL, reverse_proxy_routes, or static files configuration".into());
            }
        }
        ProxyMode::Forward => {
            // Forward proxy specific validation
            if config.static_files.is_some() {
                return Err("Static files are not supported in forward proxy mode".into());
            }
        }
    }

    // Validate worker_threads configuration
    // Check top-level worker_threads first (shared for reverse proxy + static files)
    if let Some(worker_threads) = config.worker_threads {
        if worker_threads == 0 {
            return Err("worker_threads must be greater than 0".into());
        }
        if worker_threads > 512 {
            return Err("worker_threads cannot exceed 512".into());
        }
        info!(
            "Configuration validated: shared worker_threads = {}",
            worker_threads
        );
    }

    // Check static_files specific worker_threads (backward compatibility)
    if let Some(static_files) = &config.static_files {
        if let Some(worker_threads) = static_files.worker_threads {
            if worker_threads == 0 {
                return Err("worker_threads must be greater than 0".into());
            }
            if worker_threads > 512 {
                return Err("worker_threads cannot exceed 512".into());
            }
            info!(
                "Configuration validated: static_files worker_threads = {} (takes priority over shared worker_threads)",
                worker_threads
            );
        }
    }

    Ok(())
}

#[cfg(test)]
mod config_validation_tests {
    use super::*;
    use bifrost_bridge::config::{ReverseProxyRouteConfig, RoutePredicateConfig};

    #[test]
    fn reverse_mode_accepts_routes_without_target() {
        let route = ReverseProxyRouteConfig {
            id: "test".to_string(),
            target: Some("http://localhost:3000".to_string()),
            targets: Vec::new(),
            load_balancing: None,
            sticky: None,
            header_override: None,
            retry_policy: None,
            reverse_proxy_config: None,
            strip_path_prefix: None,
            priority: Some(0),
            order: None,
            predicates: vec![RoutePredicateConfig::Path {
                patterns: vec!["/**".to_string()],
                match_trailing_slash: true,
            }],
            plugins: Vec::new(),
        };

        let config = Config {
            mode: ProxyMode::Reverse,
            listen_addr: "127.0.0.1:8080".parse().unwrap(),
            reverse_proxy_target: None,
            reverse_proxy_routes: vec![route],
            static_files: None,
            ..Default::default()
        };

        assert!(validate_config(&config).is_ok());
    }

    #[test]
    fn tls_key_and_certificate_must_be_configured_as_a_pair() {
        let config = Config {
            private_key: Some("key.pem".to_string()),
            ..Default::default()
        };

        assert!(validate_tls_pair(&config).is_err());
    }

    #[test]
    fn static_worker_threads_override_top_level_value() {
        let config = Config {
            worker_threads: Some(4),
            static_files: Some(bifrost_bridge::config::StaticFileConfig {
                worker_threads: Some(8),
                ..Default::default()
            }),
            ..Default::default()
        };

        assert_eq!(effective_worker_threads(&config), Some(8));
    }

    #[cfg(unix)]
    #[test]
    fn pid_file_guard_removes_its_pid_file_on_drop() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let path = file.path().to_path_buf();
        drop(file);

        let guard = PidFileGuard::acquire(&path).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            std::process::id().to_string()
        );
        drop(guard);
        assert!(!path.exists());
    }
}
