use clap::Parser;
use log::info;
use bifrost_bridge::{config::{Config, ProxyMode}, proxy::ProxyFactory};
use std::path::Path;
use tokio::signal;
use tokio::sync::oneshot;

#[derive(Parser)]
#[clap(
    version = "1.0.0",
    author = "Rust Proxy Server",
    about = "A Rust proxy server that can function as both forward and reverse proxy"
)]
struct Args {
    #[clap(short, long, value_name = "MODE", help = "Proxy mode: forward or reverse")]
    mode: Option<String>,

    #[clap(short, long, value_name = "ADDR", help = "Listen address (e.g., 127.0.0.1:8080)")]
    listen: Option<String>,

    #[clap(short, long, value_name = "URL", help = "Target URL for reverse proxy (e.g., http://backend:3000)")]
    target: Option<String>,

    #[clap(short, long, value_name = "FILE", help = "Configuration file path")]
    config: Option<String>,

    #[clap(long, value_name = "SECONDS", help = "Connection timeout in seconds")]
    connect_timeout: Option<u64>,

    #[clap(long, value_name = "SECONDS", help = "Idle timeout in seconds")]
    idle_timeout: Option<u64>,

    #[clap(long, value_name = "SECONDS", help = "Maximum connection lifetime in seconds")]
    max_connection_lifetime: Option<u64>,

    #[clap(long, value_name = "SECONDS", help = "Request timeout in seconds (deprecated, use specific timeout options)")]
    timeout: Option<u64>,

    #[clap(long, value_name = "FILE", help = "Generate a sample configuration file")]
    generate_config: Option<String>,

    #[clap(long, value_name = "DIR", help = "Serve static files from this directory")]
    static_dir: Option<String>,

    #[clap(long, value_name = "PATH:DIR", help = "Mount static files from PATH to DIR (can be used multiple times)")]
    mount: Vec<String>,

    #[clap(long, help = "Enable SPA mode")]
    spa: bool,

    #[clap(long, value_name = "FILE", help = "SPA fallback file name (default: index.html)")]
    spa_fallback: Option<String>,

    #[clap(long, value_name = "NUM", help = "Number of worker threads for static file serving")]
    worker_threads: Option<usize>,

    #[clap(long, value_name = "EXT:MIME", help = "Custom MIME type mapping (e.g., mjs:application/javascript), can be used multiple times")]
    mime_type: Vec<String>,

    #[clap(long, value_name = "FILE", help = "Private key file path for HTTPS")]
    private_key: Option<String>,

    #[clap(long, value_name = "FILE", help = "Certificate file path for HTTPS")]
    certificate: Option<String>,

    #[clap(long, help = "Disable connection pooling (no-pool mode)")]
    no_connection_pool: bool,

    #[clap(long, value_name = "NUM", help = "Maximum idle connections per host for connection pooling")]
    pool_max_idle: Option<usize>,

    #[clap(long, value_name = "BYTES", help = "Maximum HTTP header size in bytes")]
    max_header_size: Option<usize>,

    #[clap(long, value_name = "USERNAME", help = "Username for proxy authentication (Basic Auth)")]
    proxy_username: Option<String>,

    #[clap(long, value_name = "PASSWORD", help = "Password for proxy authentication (Basic Auth)")]
    proxy_password: Option<String>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let args = Args::parse();

    // Handle generate-config flag
    if let Some(config_file) = args.generate_config {
        generate_sample_config(&config_file)?;
        println!("Sample configuration file generated: {}", config_file);
        return Ok(());
    }

    // Load configuration
    let config = if let Some(config_file) = &args.config {
        if !Path::new(config_file).exists() {
            return Err(format!("Configuration file not found: {}", config_file).into());
        }
        Config::from_file(config_file)?
    } else {
        create_config_from_args(&args)?
    };

    // Validate configuration
    validate_config(&config)?;

    // Create and run proxy with graceful shutdown
    info!("Starting proxy server...");

    let proxy = ProxyFactory::create_proxy(config)?;

    // Create a shutdown signal
    let (_shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();

    // Spawn the server in a task
    let server_handle = tokio::spawn(async move {
        if let Err(e) = proxy.run().await {
            eprintln!("Server error: {}", e);
        }
    });

    // Wait for Ctrl+C signal
    tokio::select! {
        _ = signal::ctrl_c() => {
            info!("\n🛑 Received Ctrl+C, shutting down gracefully...");
        }
        _ = &mut shutdown_rx => {
            info!("🛑 Shutdown signal received, shutting down gracefully...");
        }
        result = server_handle => {
            if let Err(e) = result {
                eprintln!("Server task error: {}", e);
            }
        }
    }

    info!("👋 Proxy server stopped. Goodbye!");
    Ok(())
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
  "connection_pool_enabled": true,
  "pool_max_idle_per_host": 10
}"#;

    let sample_reverse = r#"{
  "mode": "Reverse",
  "listen_addr": "127.0.0.1:8080",
  "reverse_proxy_target": "http://backend.example.com:3000",
  "max_connections": 1000,
  "connect_timeout_secs": 10,
  "idle_timeout_secs": 90,
  "max_connection_lifetime_secs": 300,
  "max_header_size": 16384
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
        forward_proxy_port: Some(3128),
        max_connections: Some(1000),
        connect_timeout_secs: args.connect_timeout,
        idle_timeout_secs: args.idle_timeout,
        max_connection_lifetime_secs: args.max_connection_lifetime,
        timeout_secs: args.timeout,
        static_files: None,
        private_key: args.private_key.clone(),
        certificate: args.certificate.clone(),
        connection_pool_enabled: Some(!args.no_connection_pool),
        pool_max_idle_per_host: args.pool_max_idle,
        max_header_size: args.max_header_size,
        relay_proxies: None,
        relay_proxy_url: None,
        relay_proxy_username: None,
        relay_proxy_password: None,
        relay_proxy_domain_suffixes: None,
        proxy_username: args.proxy_username.clone(),
        proxy_password: args.proxy_password.clone(),
    };

    // Configure static files if specified
    if args.static_dir.is_some() || !args.mount.is_empty() {
        let mut static_config = if let Some(static_dir) = &args.static_dir {
            // Single directory mode (backward compatibility)
            let mut config = bifrost_bridge::config::StaticFileConfig::single(static_dir.clone(), args.spa);
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
                spa_fallback_file: args.spa_fallback.clone().unwrap_or_else(|| "index.html".to_string()),
                worker_threads: args.worker_threads,
                custom_mime_types: std::collections::HashMap::new(),
            }
        };

        // Process mounts
        for mount_spec in &args.mount {
            let parts: Vec<&str> = mount_spec.splitn(2, ':').collect();
            if parts.len() != 2 {
                return Err(format!("Invalid mount specification: '{}'. Use format 'PATH:DIR'", mount_spec).into());
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
                return Err(format!("Invalid MIME type specification: '{}'. Use format 'EXT:MIME'", mime_spec).into());
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
            if config.reverse_proxy_target.is_none() && config.static_files.is_none() {
                return Err("Reverse proxy mode requires either a target URL or static files configuration".into());
            }
        }
        ProxyMode::Forward => {
            // Forward proxy specific validation
            if config.static_files.is_some() {
                return Err("Static files are not supported in forward proxy mode".into());
            }
        }
    }

    Ok(())
}