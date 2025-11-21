//! Forward proxy implementation supporting HTTP and HTTPS protocols.
//!
//! This module provides a forward proxy server with the following features:
//! - HTTP/HTTPS CONNECT tunneling for HTTPS requests
//! - Relay proxy support with domain-based routing
//! - Basic proxy authentication
//! - Connection pooling and timeout configuration

use crate::error::ProxyError;
use crate::config::{RelayProxyConfig, WebSocketConfig};
use crate::common::{ResponseBuilder, TlsConfig, is_websocket_upgrade};
use crate::rate_limit::RateLimiter;
use rustls::ServerConfig;
use hyper::{Request, Response, StatusCode, Uri, Method};
use hyper::body::{Bytes, Incoming};
use http_body_util::{BodyExt, Full};
use hyper::server::conn::http1::Builder as ServerBuilder;
use hyper::service::service_fn;
use log::{info, error, debug, warn};
use hyper_util::rt::TokioIo;
use hyper::header::{HOST, ORIGIN, PROXY_AUTHORIZATION, HeaderValue, SEC_WEBSOCKET_PROTOCOL};
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, copy_bidirectional};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::{Duration, timeout};
use url::Url;
use tokio_rustls::TlsAcceptor;
use base64::{Engine as _, engine::general_purpose};
use hyper_util::client::legacy::{Client, connect::HttpConnector};
use hyper_util::rt::{TokioExecutor, TokioTimer};

/// Forward proxy server implementation.
///
/// Supports both direct connections and relay proxy routing based on domain patterns.
pub struct ForwardProxy {
    connection_pool_enabled: bool,
    max_connection_lifetime: Duration,
    relay_proxies: Vec<RelayProxyWithAuth>,
    proxy_username: Option<String>,
    proxy_password: Option<String>,
    // Instance-specific HTTP client configured per ForwardProxy settings
    http_client: Arc<Client<HttpConnector, Incoming>>,
    websocket_config: WebSocketConfig,
    rate_limiter: Arc<RateLimiter>,
}

/// Internal structure to store relay proxy configuration with pre-computed authentication.
#[derive(Clone)]
struct RelayProxyWithAuth {
    url: String,
    /// Base64 encoded "Basic {credentials}" header value
    auth: Option<String>,
    /// Domain patterns in NO_PROXY format for routing decisions
    domains: Vec<String>,
}

impl ForwardProxy {
    /// Creates a new forward proxy with basic timeout configuration.
    ///
    /// # Arguments
    ///
    /// * `connect_timeout_secs` - Timeout for establishing connections
    /// * `idle_timeout_secs` - Idle timeout for pooled connections
    /// * `max_connection_lifetime_secs` - Maximum lifetime for any connection (enforced on CONNECT tunnels)
    pub fn new(connect_timeout_secs: u64, idle_timeout_secs: u64, max_connection_lifetime_secs: u64) -> Self {
        let http_client = Self::build_http_client(
            connect_timeout_secs,
            idle_timeout_secs,
            true,
        );
        
        Self {
            connection_pool_enabled: true,
            max_connection_lifetime: Duration::from_secs(max_connection_lifetime_secs),
            relay_proxies: Vec::new(),
            proxy_username: None,
            proxy_password: None,
            http_client: Arc::new(http_client),
            websocket_config: WebSocketConfig::default(),
            rate_limiter: Arc::new(RateLimiter::new(None)),
        }
    }

    /// Creates a new forward proxy with connection pool configuration.
    ///
    /// # Arguments
    ///
    /// * `connect_timeout_secs` - Timeout for establishing connections
    /// * `idle_timeout_secs` - Idle timeout for pooled connections
    /// * `max_connection_lifetime_secs` - Maximum lifetime for any connection (enforced on CONNECT tunnels)
    /// * `connection_pool_enabled` - Whether to enable connection pooling
    pub fn new_with_pool_config(
        connect_timeout_secs: u64,
        idle_timeout_secs: u64,
        max_connection_lifetime_secs: u64,
        connection_pool_enabled: bool,
    ) -> Self {
        let http_client = Self::build_http_client(
            connect_timeout_secs,
            idle_timeout_secs,
            connection_pool_enabled,
        );
        
        Self {
            connection_pool_enabled,
            max_connection_lifetime: Duration::from_secs(max_connection_lifetime_secs),
            relay_proxies: Vec::new(),
            proxy_username: None,
            proxy_password: None,
            http_client: Arc::new(http_client),
            websocket_config: WebSocketConfig::default(),
            rate_limiter: Arc::new(RateLimiter::new(None)),
        }
    }

    /// Creates a forward proxy with relay proxy support (legacy single relay).
    ///
    /// This method is maintained for backward compatibility. For multiple relay proxies,
    /// use `new_with_relay_proxies` instead.
    ///
    /// # Arguments
    ///
    /// * `relay_proxy_url` - URL of the relay proxy server
    /// * `relay_proxy_username` - Optional username for relay authentication
    /// * `relay_proxy_password` - Optional password for relay authentication
    /// * `relay_proxy_domain_suffixes` - Optional domain patterns for relay routing
    pub fn new_with_relay(
        connect_timeout_secs: u64,
        idle_timeout_secs: u64,
        max_connection_lifetime_secs: u64,
        connection_pool_enabled: bool,
        relay_proxy_url: Option<String>,
        relay_proxy_username: Option<String>,
        relay_proxy_password: Option<String>,
        relay_proxy_domain_suffixes: Option<Vec<String>>,
    ) -> Self {
        // Convert legacy single relay proxy to new format
        let relay_configs = if let Some(url) = relay_proxy_url {
            vec![RelayProxyConfig {
                relay_proxy_url: url,
                relay_proxy_username,
                relay_proxy_password,
                relay_proxy_domains: relay_proxy_domain_suffixes.unwrap_or_default(),
            }]
        } else {
            Vec::new()
        };

        Self::new_with_relay_proxies(
            connect_timeout_secs,
            idle_timeout_secs,
            max_connection_lifetime_secs,
            connection_pool_enabled,
            relay_configs,
            None,
            None,
            None,
            Arc::new(RateLimiter::new(None)),
        )
    }

    /// Creates a forward proxy with multiple relay proxy support.
    ///
    /// Supports routing different domains through different relay proxies based on
    /// domain matching patterns.
    ///
    /// # Arguments
    ///
    /// * `relay_configs` - List of relay proxy configurations with routing rules
    /// * `proxy_username` - Optional username for proxy authentication (client to this proxy)
    /// * `proxy_password` - Optional password for proxy authentication (client to this proxy)
    pub fn new_with_relay_proxies(
        connect_timeout_secs: u64,
        idle_timeout_secs: u64,
        max_connection_lifetime_secs: u64,
        connection_pool_enabled: bool,
        relay_configs: Vec<RelayProxyConfig>,
        proxy_username: Option<String>,
        proxy_password: Option<String>,
        websocket_config: Option<WebSocketConfig>,
        rate_limiter: Arc<RateLimiter>,
    ) -> Self {
        // Client approach removed - using direct TCP connections for CONNECT requests

        // Convert RelayProxyConfig to RelayProxyWithAuth
        let relay_proxies: Vec<RelayProxyWithAuth> = relay_configs
            .into_iter()
            .map(|config| {
                let auth = match (config.relay_proxy_username, config.relay_proxy_password) {
                    (Some(username), Some(password)) => {
                        let credentials = format!("{}:{}", username, password);
                        let encoded = general_purpose::STANDARD.encode(credentials.as_bytes());
                        Some(format!("Basic {}", encoded))
                    }
                    _ => None,
                };
                RelayProxyWithAuth {
                    url: config.relay_proxy_url,
                    auth,
                    domains: config.relay_proxy_domains,
                }
            })
            .collect();

        let http_client = Self::build_http_client(
            connect_timeout_secs,
            idle_timeout_secs,
            connection_pool_enabled,
        );

        Self {
            connection_pool_enabled,
            max_connection_lifetime: Duration::from_secs(max_connection_lifetime_secs),
            relay_proxies,
            proxy_username,
            proxy_password,
            http_client: Arc::new(http_client),
            websocket_config: websocket_config.unwrap_or_default(),
            rate_limiter,
        }
    }

    /// Build HTTP client for forward proxy.
    ///
    /// Forward proxy pooling strategy:
    /// - Connection reuse ENABLED for same target host (improves performance)
    /// - pool_idle_timeout controls when to close idle connections
    /// - Once idle timeout expires, connections are automatically closed
    /// - No need for pool_max_idle_per_host - let timeout do the cleanup
    ///
    /// How it works:
    /// - Multiple requests to api.example.com reuse the same connection (fast!)
    /// - After idle_timeout (10-30s), unused connections close automatically
    /// - No persistent "waiting pool" - connections close when truly idle
    ///
    /// This gives us the best of both worlds:
    /// - Performance: Connection reuse during active traffic
    /// - Resource efficiency: Auto-cleanup via timeout (no manual pool limits)
    fn build_http_client(
        connect_timeout_secs: u64,
        idle_timeout_secs: u64,
        pool_enabled: bool,
    ) -> Client<HttpConnector, Incoming> {
        let mut connector = HttpConnector::new();
        connector.set_connect_timeout(Some(Duration::from_secs(connect_timeout_secs)));
        connector.set_keepalive(Some(Duration::from_secs(idle_timeout_secs)));
        connector.set_nodelay(true); // Disable Nagle's algorithm for better latency
        
        let mut builder = Client::builder(TokioExecutor::new());
        
        if pool_enabled {
            // Enable connection reuse with automatic timeout-based cleanup
            // idle_timeout controls when idle connections are closed
            // No need to set pool_max_idle_per_host - timeout handles cleanup
            info!("Forward proxy: connection reuse enabled, idle timeout={}s (auto-cleanup)",
                  idle_timeout_secs);
            builder.pool_idle_timeout(Duration::from_secs(idle_timeout_secs));
            builder.pool_timer(TokioTimer::new());
            // NOTE: We do NOT set pool_max_idle_per_host(0) as that disables pooling entirely!
            // Hyper will manage the pool and close connections after idle_timeout
        } else {
            info!("Forward proxy: no-pool mode (new connection per request)");
            builder.pool_max_idle_per_host(0); // Only disable when explicitly requested
        }
        
        builder
            .http2_only(false)
            .build(connector)
    }

    pub async fn run(self, addr: SocketAddr) -> Result<(), ProxyError> {
        self.run_http(addr).await
    }

    pub async fn run_with_tls(self, addr: SocketAddr, tls_config: ServerConfig) -> Result<(), ProxyError> {
        self.run_https(addr, Some(Arc::new(tls_config))).await
    }

    pub async fn run_with_config(self, addr: SocketAddr, private_key: Option<String>, certificate: Option<String>) -> Result<(), ProxyError> {
        match (private_key, certificate) {
            (Some(private_key_path), Some(cert_path)) => {
                // HTTPS mode
                let tls_config = TlsConfig::create_config(&private_key_path, &cert_path)?;
                self.run_https(addr, Some(Arc::new(tls_config))).await
            }
            _ => {
                // HTTP mode
                self.run_http(addr).await
            }
        }
    }

    async fn run_http(self, addr: SocketAddr) -> Result<(), ProxyError> {
        let relay_proxies = self.relay_proxies.clone();
        let proxy_username = self.proxy_username;
        let proxy_password = self.proxy_password;
        let http_client = self.http_client; // Capture the HTTP client
        let websocket_config = self.websocket_config.clone();
        let rate_limiter = self.rate_limiter.clone();

        let listener = tokio::net::TcpListener::bind(addr).await
            .map_err(|e| ProxyError::Hyper(e.to_string()))?;

        info!("HTTP forward proxy listening on: http://{}", addr);

        loop {
            let (stream, remote_addr) = listener.accept().await
                .map_err(|e| ProxyError::Hyper(e.to_string()))?;

            let relay_proxies = relay_proxies.clone();
            let proxy_username = proxy_username.clone();
            let proxy_password = proxy_password.clone();
            let http_client = http_client.clone(); // Clone the Arc for the spawn
            let websocket_config = websocket_config.clone();
            let rate_limiter = rate_limiter.clone();
            let client_ip = remote_addr.ip().to_string();

            tokio::spawn(async move {
                // For CONNECT requests, we need to handle the tunnel manually
                // Try to peek at the first line to check if it's CONNECT
                let mut peek_buf = vec![0u8; 1024];

                // Try to peek at the first line without consuming
                match stream.peek(&mut peek_buf).await {
                    Ok(n) if n > 0 => {
                        let first_line = String::from_utf8_lossy(&peek_buf[..n]);
                        if first_line.starts_with("CONNECT ") {
                            // It's a CONNECT request, handle it manually at TCP level
                            let _ = ForwardProxy::handle_connect_raw(
                                stream,
                                remote_addr,
                                relay_proxies,
                                proxy_username,
                                proxy_password,
                                rate_limiter.clone(),
                            ).await;
                            return;
                        }
                    }
                    _ => {
                        // Can't peek or not enough data, treat as normal HTTP
                    }
                }

                // Not a CONNECT request, use normal HTTP handling
                let io = TokioIo::new(stream);
                let http_client = Arc::clone(&http_client);
                if let Err(err) = ServerBuilder::new()
                    .serve_connection(
                        io,
                        service_fn(move |req| {
                            let http_client = Arc::clone(&http_client);
                            let relay_proxies = relay_proxies.clone();
                            let proxy_username = proxy_username.clone();
                            let proxy_password = proxy_password.clone();
                            let websocket_config = websocket_config.clone();
                            let rate_limiter = rate_limiter.clone();
                            let client_ip = client_ip.clone();
                            async move {
                                // Check if this is a CONNECT request
                                if req.method() == Method::CONNECT {
                                    Self::handle_connect_tunnel_static(
                                        req,
                                        relay_proxies,
                                        websocket_config.clone(),
                                        rate_limiter.clone(),
                                        Some(client_ip.clone()),
                                    ).await
                                } else {
                                    Self::handle_request_static(
                                        req,
                                        http_client,
                                        relay_proxies,
                                        proxy_username,
                                        proxy_password,
                                        websocket_config,
                                        rate_limiter,
                                        Some(client_ip.clone()),
                                    ).await
                                }
                            }
                        })
                    )
                    .await
                {
                    error!("Error serving forward proxy connection: {}", err);
                }
            });
        }
    }

    /// Handles CONNECT requests at the raw TCP level.
    ///
    /// This bypasses hyper's HTTP handling to establish a direct TCP tunnel,
    /// which is necessary for proper HTTPS proxy support through relay proxies.
    async fn handle_connect_raw(
        stream: TcpStream,
        remote_addr: SocketAddr,
        relay_proxies: Vec<RelayProxyWithAuth>,
        _proxy_username: Option<String>,
        _proxy_password: Option<String>,
        rate_limiter: Arc<RateLimiter>,
    ) -> Result<(), std::io::Error> {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

        let mut reader = BufReader::new(stream);
        
        // Read the CONNECT request line
        let mut request_line = String::new();
        reader.read_line(&mut request_line).await?;
        debug!("CONNECT request: {}", request_line.trim());

        // Parse the request
        let parts: Vec<&str> = request_line.trim().split(' ').collect();
        if parts.len() < 2 {
            let error = "HTTP/1.1 400 Bad Request\r\n\r\n";
            let mut stream = reader.into_inner();
            tokio::io::AsyncWriteExt::write_all(&mut stream, error.as_bytes()).await?;
            return Ok(());
        }

        let target = parts[1].to_string();

        // Parse target host and port
        let (target_host, target_port) = if let Some(colon_pos) = target.rfind(':') {
            let host = &target[..colon_pos];
            let port_str = &target[colon_pos + 1..];
            let port = port_str.parse::<u16>().unwrap_or(443);
            (host.to_string(), port)
        } else {
            (target.clone(), 443)
        };

        // Read and discard headers until empty line
        loop {
            let mut header_line = String::new();
            reader.read_line(&mut header_line).await?;
            if header_line.trim().is_empty() || header_line == "\r\n" {
                break;
            }
        }
        
        // Get the underlying stream back
        let mut stream = reader.into_inner();

        if rate_limiter.is_enabled() {
            let client_ip = remote_addr.ip().to_string();
            if let Err(hit) = rate_limiter
                .check_request(&client_ip, &Method::CONNECT, &target)
                .await
            {
                warn!(
                    "Forward proxy CONNECT rate limit hit for {} via rule {}",
                    client_ip, hit.rule_id
                );
                let body = format!(
                    "Rate limit '{}' exceeded. Please retry later.",
                    hit.rule_id
                );
                let response = format!(
                    "HTTP/1.1 429 Too Many Requests\r\nRetry-After: {}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\n\r\n{}",
                    hit.retry_after_secs,
                    body.len(),
                    body
                );
                tokio::io::AsyncWriteExt::write_all(&mut stream, response.as_bytes()).await?;
                return Ok(());
            }
        }

        // Find relay proxy if configured
        let relay_proxy = Self::find_relay_proxy_for_domain_static(&relay_proxies, &target_host);
        let target_desc = if let Some(relay) = &relay_proxy {
            format!("{} via relay {}", target, relay.url)
        } else {
            target.clone()
        };

        // Connect to target
        let target_result = if let Some(relay) = relay_proxy {
            debug!("Connecting to {} via relay proxy", target_desc);
            ForwardProxy::connect_via_relay(
                &relay.url,
                &relay.auth,
                &target_host,
                target_port,
            ).await
        } else {
            debug!("Direct connection to {}", target_desc);
            TcpStream::connect(format!("{}:{}", target_host, target_port)).await
        };

        let target_stream = match target_result {
            Ok(s) => s,
            Err(e) => {
                error!("Failed to connect to target: {}", e);
                let error_response = "HTTP/1.1 502 Bad Gateway\r\n\r\n";
                stream.write_all(error_response.as_bytes()).await?;
                return Err(e);
            }
        };

        info!("Successfully connected to target, setting up tunnel");

        // Send 200 OK to client
        let ok_response = "HTTP/1.1 200 Connection established\r\nProxy-agent: Rust-Proxy/1.0\r\n\r\n";
        stream.write_all(ok_response.as_bytes()).await
            .map_err(|e| {
                error!("Failed to send 200 OK response: {}", e);
                e
            })?;

        // Set up bidirectional tunnel with max lifetime enforcement
        let _ = ForwardProxy::setup_tunnel_with_lifetime(
            stream,
            target_stream,
            remote_addr,
            target_desc,
            Duration::from_secs(300), // Static method uses default 300s
        ).await;

        Ok(())
    }

    async fn run_https(self, addr: SocketAddr, tls_config: Option<Arc<ServerConfig>>) -> Result<(), ProxyError> {
        let relay_proxies = self.relay_proxies.clone();
        let proxy_username = self.proxy_username;
        let proxy_password = self.proxy_password;
        let connection_pool_enabled = self.connection_pool_enabled;
        let http_client = self.http_client; // Capture the HTTP client
        let websocket_config = self.websocket_config.clone();
        let rate_limiter = self.rate_limiter.clone();
        let tls_acceptor = if let Some(config) = tls_config {
            Some(TlsAcceptor::from(config))
        } else {
            None
        };

        let tcp_listener = TcpListener::bind(&addr).await
            .map_err(|e| ProxyError::Io(e))?;

        info!("HTTPS forward proxy listening on: https://{}", addr);
        if connection_pool_enabled {
            info!("Connection pooling enabled");
        } else {
            info!("Connection pooling disabled (no-pool mode)");
        }

        loop {
            let (tcp_stream, remote_addr) = tcp_listener.accept().await
                .map_err(|e| ProxyError::Io(e))?;

            let relay_proxies = relay_proxies.clone();
            let proxy_username = proxy_username.clone();
            let proxy_password = proxy_password.clone();
            let tls_acceptor = tls_acceptor.clone();
            let http_client = http_client.clone(); // Clone the Arc for the spawn
            let websocket_config = websocket_config.clone();
            let rate_limiter = rate_limiter.clone();
            let client_ip = remote_addr.ip().to_string();

            tokio::spawn(async move {
                if let Some(acceptor) = tls_acceptor {
                    // HTTPS mode
                    match acceptor.accept(tcp_stream).await {
                        Ok(tls_stream) => {
                            let http_client = Arc::clone(&http_client);
                            let service = service_fn(move |req| {
                                let http_client = Arc::clone(&http_client);
                                let relay_proxies = relay_proxies.clone();
                                let proxy_username = proxy_username.clone();
                                let proxy_password = proxy_password.clone();
                                let websocket_config = websocket_config.clone();
                                let rate_limiter = rate_limiter.clone();
                                let client_ip = client_ip.clone();
                                async move {
                                    // Check if this is a CONNECT request
                                    if req.method() == Method::CONNECT {
                                        ForwardProxy::handle_connect_tunnel_static(
                                            req,
                                            relay_proxies,
                                            websocket_config.clone(),
                                            rate_limiter.clone(),
                                            Some(client_ip.clone()),
                                        ).await
                                    } else {
                                        ForwardProxy::handle_request_static(
                                            req,
                                            http_client,
                                            relay_proxies,
                                            proxy_username,
                                            proxy_password,
                                            websocket_config,
                                            rate_limiter,
                                            Some(client_ip.clone()),
                                        ).await
                                    }
                                }
                            });

                            if let Err(e) = ServerBuilder::new()
                                .keep_alive(true)
                                .serve_connection(TokioIo::new(tls_stream), service)
                                .await
                            {
                                error!("Error serving HTTPS connection: {}", e);
                            }
                        }
                        Err(e) => {
                            error!("Error establishing TLS connection: {}", e);
                        }
                    }
                }
            });
        }
    }

    async fn handle_request(&self, req: Request<Incoming>, client_ip: Option<String>) -> Result<Response<Full<Bytes>>, Infallible> {
        match self.process_request(req, client_ip).await {
            Ok(response) => Ok(response),
            Err(e) => {
                error!("Proxy error: {}", e);
                // Return 401 Unauthorized for authentication errors
                let status = if matches!(e, ProxyError::Auth(_)) {
                    StatusCode::UNAUTHORIZED
                } else {
                    StatusCode::BAD_GATEWAY
                };

                let mut response_builder = Response::builder()
                    .status(status)
                    .body(Full::new(Bytes::from(format!("Proxy Error: {}", e))))
                    .unwrap();

                // Add Proxy-Authenticate header for 401 responses
                if status == StatusCode::UNAUTHORIZED {
                    response_builder.headers_mut()
                        .insert("Proxy-Authenticate", HeaderValue::from_static("Basic realm=\"Proxy Server\""));
                }

                Ok(response_builder)
            }
        }
    }

    async fn process_request(&self, req: Request<Incoming>, client_ip: Option<String>) -> Result<Response<Full<Bytes>>, ProxyError> {
        self.verify_authentication(&req)?;

        if let Some(ip) = client_ip.as_deref() {
            if let Err(hit) = self
                .rate_limiter
                .check_request(
                    ip,
                    req.method(),
                    req.uri()
                        .path_and_query()
                        .map(|pq| pq.as_str())
                        .unwrap_or("/"),
                )
                .await
            {
                warn!("Forward proxy rate limit hit for {} via rule {}", ip, hit.rule_id);
                return Ok(ResponseBuilder::too_many_requests(&hit.rule_id, hit.retry_after_secs));
            }
        }

        if *req.method() == Method::CONNECT {
            return match self.handle_connect_tunnel(req, client_ip).await {
                Ok(response) => Ok(response),
                Err(_) => unreachable!(),
            };
        }

        let target_uri = self.extract_target_uri(&req)?;
        let host = target_uri.host().unwrap_or("unknown");
        let port = target_uri.port_u16().unwrap_or(80);
        let scheme = target_uri.scheme_str().unwrap_or("http");

        let relay_proxy = self.find_relay_proxy_for_domain(host);

        if let Some(relay) = &relay_proxy {
            debug!("HTTP request to {}://{}:{}{} via relay proxy {} (matched domain rule)",
                scheme, host, port,
                target_uri.path_and_query().map(|pq| pq.as_str()).unwrap_or(""),
                relay.url);
        } else {
            debug!("HTTP request to {}://{}:{}{} (direct connection)",
                scheme, host, port,
                target_uri.path_and_query().map(|pq| pq.as_str()).unwrap_or(""));
        }

        let is_websocket = is_websocket_upgrade(req.headers());

        if is_websocket {
            if let Err(reason) = self.validate_websocket_headers(req.headers()) {
                return Ok(ResponseBuilder::error(StatusCode::FORBIDDEN, &reason));
            }
        }

        if let Some(relay) = relay_proxy {
            if is_websocket {
                return match self.forward_websocket_via_relay(req, relay, &target_uri).await {
                    Ok(resp) => Ok(resp),
                    Err(e) => {
                        error!("Proxy error (relay websocket): {}", e);
                        Ok(ResponseBuilder::proxy_error("Failed to forward WebSocket request"))
                    }
                };
            }

            return match self.forward_http_via_relay(req, relay).await {
                Ok(resp) => Ok(resp),
                Err(e) => {
                    error!("Proxy error (relay): {}", e);
                    Ok(ResponseBuilder::proxy_error("Failed to forward request"))
                }
            };
        }

        if is_websocket {
            return match self.forward_websocket_direct(req, &target_uri).await {
                Ok(resp) => Ok(resp),
                Err(e) => {
                    error!("Proxy error (websocket): {}", e);
                    Ok(ResponseBuilder::proxy_error("Failed to forward WebSocket request"))
                }
            };
        }

        match self.forward_direct_http_request(req, &target_uri).await {
            Ok(response) => Ok(response),
            Err(e) => {
                error!("Proxy error (direct): {}", e);
                Ok(ResponseBuilder::proxy_error("Failed to forward request"))
            }
        }
    }

    async fn forward_direct_http_request(
        &self,
        mut req: Request<Incoming>,
        target_uri: &Uri,
    ) -> Result<Response<Full<Bytes>>, ProxyError> {
        let client = &self.http_client;

        let uri_to_use = if target_uri.scheme().is_some() && target_uri.authority().is_some() {
            target_uri.clone()
        } else {
            return Err(ProxyError::Config("Target URI missing scheme or authority".to_string()));
        };

        *req.uri_mut() = uri_to_use.clone();
        req.headers_mut().remove(PROXY_AUTHORIZATION);
        req.headers_mut().remove("Proxy-Connection");

        let response = client.request(req).await
            .map_err(|e| {
                error!("HTTP client error: {}", e);
                error!("  Target was: {}", uri_to_use);
                if e.is_connect() {
                    error!("  Error type: connection error (DNS failure, network unreachable, or timeout)");
                }
                ProxyError::Connection(format!("Failed to forward request: {}", e))
            })?;

        Self::finalize_standard_response(response).await
    }

    async fn handle_connect_tunnel(&self, req: Request<Incoming>, _client_ip: Option<String>) -> Result<Response<Full<Bytes>>, Infallible> {
        let authority = match req.uri().authority() {
            Some(auth) => auth,
            None => return Ok(ResponseBuilder::error(StatusCode::BAD_REQUEST, "Invalid CONNECT target")),
        };

        let host = authority.host().to_string();
        let port = authority.port_u16().unwrap_or(443);

        info!("Handling CONNECT request to {}:{}", host, port);

        let relay_proxy = self.find_relay_proxy_for_domain(&host);
        let max_lifetime = self.max_connection_lifetime;

        if let Some(relay) = &relay_proxy {
            info!("Connecting to {}:{} via relay proxy {}", host, port, relay.url);
        } else {
            info!("Direct connection to {}:{}", host, port);
        }

        tokio::spawn(async move {
            match hyper::upgrade::on(req).await {
                Ok(upgraded) => {
                    info!("Successfully upgraded connection for {}:{}", host, port);

                    let upgraded_io = TokioIo::new(upgraded);

                    let target_stream = if let Some(relay) = relay_proxy {
                        match ForwardProxy::connect_via_relay(
                            &relay.url,
                            &relay.auth,
                            &host,
                            port,
                        ).await {
                            Ok(stream) => stream,
                            Err(e) => {
                                error!("Failed to connect via relay to {}:{}: {}", host, port, e);
                                return;
                            }
                        }
                    } else {
                        match TcpStream::connect(format!("{}:{}", host, port)).await {
                            Ok(stream) => stream,
                            Err(e) => {
                                error!("Failed to connect to {}:{}: {}", host, port, e);
                                return;
                            }
                        }
                    };

                    info!("Successfully connected to target {}:{}", host, port);

                    let (mut client_read, mut client_write) = tokio::io::split(upgraded_io);
                    let (mut target_read, mut target_write) = target_stream.into_split();

                    let client_to_target = async {
                        match tokio::io::copy(&mut client_read, &mut target_write).await {
                            Ok(bytes) => info!("Client -> Target: {} bytes for {}:{}", bytes, host, port),
                            Err(e) => error!("Error in client->target tunnel for {}:{}: {}", host, port, e),
                        }
                    };

                    let target_to_client = async {
                        match tokio::io::copy(&mut target_read, &mut client_write).await {
                            Ok(bytes) => info!("Target -> Client: {} bytes for {}:{}", bytes, host, port),
                            Err(e) => error!("Error in target->client tunnel for {}:{}: {}", host, port, e),
                        }
                    };

                    let tunnel_future = async {
                        tokio::join!(client_to_target, target_to_client);
                    };

                    match tokio::time::timeout(max_lifetime, tunnel_future).await {
                        Ok(_) => {
                            info!("TCP tunnel closed normally for {}:{}", host, port);
                        }
                        Err(_) => {
                            info!(
                                "TCP tunnel max lifetime ({:?}) reached for {}:{}, closing connection",
                                max_lifetime, host, port
                            );
                        }
                    }
                }
                Err(e) => {
                    error!("Failed to upgrade connection for {}:{}: {}", host, port, e);
                }
            }
        });

        Ok(Response::builder()
            .status(StatusCode::OK)
            .body(Full::new(Bytes::new()))
            .unwrap())
    }

    async fn forward_websocket_direct(
        &self,
        mut req: Request<Incoming>,
        target_uri: &Uri,
    ) -> Result<Response<Full<Bytes>>, ProxyError> {
        let client_upgrade = hyper::upgrade::on(&mut req);
        let tunnel_timeout = Duration::from_secs(self.websocket_config.timeout_seconds);
        let target_desc = target_uri.to_string();

        *req.uri_mut() = target_uri.clone();
        req.headers_mut().remove(PROXY_AUTHORIZATION);
        req.headers_mut().remove("Proxy-Connection");

        let mut response = self.http_client.request(req).await
            .map_err(|e| ProxyError::Connection(format!("Failed to forward WebSocket request: {}", e)))?;

        if response.status() != StatusCode::SWITCHING_PROTOCOLS {
            return Self::finalize_standard_response(response).await;
        }

        let backend_upgrade = hyper::upgrade::on(&mut response);
        let (parts, _) = response.into_parts();
        let switch_response = Response::from_parts(parts, Full::new(Bytes::new()));

        tokio::spawn(async move {
            match (client_upgrade.await, backend_upgrade.await) {
                (Ok(client_stream), Ok(backend_stream)) => {
                    let mut client_io = TokioIo::new(client_stream);
                    let mut backend_io = TokioIo::new(backend_stream);
                    let tunnel = async {
                        if let Err(e) = copy_bidirectional(&mut client_io, &mut backend_io).await {
                            error!("WebSocket tunnel error: {}", e);
                        }
                    };
                    if timeout(tunnel_timeout, tunnel).await.is_err() {
                        info!("WebSocket tunnel timeout reached for {}", target_desc);
                    }
                }
                (Err(e), _) => error!("Client WebSocket upgrade failed: {}", e),
                (_, Err(e)) => error!("Backend WebSocket upgrade failed: {}", e),
            }
        });

        Ok(switch_response)
    }

    async fn forward_http_via_relay(
        &self,
        req: Request<Incoming>,
        relay: RelayProxyWithAuth,
    ) -> Result<Response<Full<Bytes>>, ProxyError> {
        let mut reader = self.open_relay_stream(req, &relay).await?;
        let (status_code, response_headers, content_length, chunked) =
            Self::parse_relay_status_and_headers(&mut reader).await?;
        let body = Self::read_relay_body(&mut reader, content_length, chunked).await?;

        let mut response = Response::builder()
            .status(StatusCode::from_u16(status_code).unwrap_or(StatusCode::BAD_GATEWAY));
        if let Some(headers) = response.headers_mut() {
            *headers = response_headers;
        }

        response
            .body(Full::new(Bytes::from(body)))
            .map_err(|e| ProxyError::Http(e.to_string()))
    }

    async fn forward_websocket_via_relay(
        &self,
        mut req: Request<Incoming>,
        relay: RelayProxyWithAuth,
        target_uri: &Uri,
    ) -> Result<Response<Full<Bytes>>, ProxyError> {
        debug!("WebSocket upgrade via relay {} for {}", relay.url, target_uri);
        let client_upgrade = hyper::upgrade::on(&mut req);
        let tunnel_timeout = Duration::from_secs(self.websocket_config.timeout_seconds);
        let target_desc = target_uri.to_string();
        let mut reader = self.open_relay_stream(req, &relay).await?;
        let (status_code, mut response_headers, content_length, chunked) =
            Self::parse_relay_status_and_headers(&mut reader).await?;

        if status_code != 101 {
            let body = Self::read_relay_body(&mut reader, content_length, chunked).await?;
            let mut response = Response::builder()
                .status(StatusCode::from_u16(status_code).unwrap_or(StatusCode::BAD_GATEWAY));
            if let Some(headers) = response.headers_mut() {
                *headers = response_headers;
            }
            return response
                .body(Full::new(Bytes::from(body)))
                .map_err(|e| ProxyError::Http(e.to_string()));
        }

        // Remove proxy-specific headers before returning
        response_headers.remove("proxy-connection");
        response_headers.remove("proxy-authenticate");

        let mut response = Response::builder()
            .status(StatusCode::SWITCHING_PROTOCOLS);
        if let Some(headers) = response.headers_mut() {
            *headers = response_headers;
        }

        let backend_stream = reader.into_inner();
        tokio::spawn(async move {
            match client_upgrade.await {
                Ok(client_stream) => {
                    let mut client_io = TokioIo::new(client_stream);
                    let mut backend_stream = backend_stream;
                    let tunnel = async {
                        if let Err(e) = copy_bidirectional(&mut client_io, &mut backend_stream).await {
                            error!("WebSocket relay tunnel error: {}", e);
                        }
                    };
                    if timeout(tunnel_timeout, tunnel).await.is_err() {
                        info!("WebSocket relay tunnel timeout reached for {}", target_desc);
                    }
                }
                Err(e) => error!("Client WebSocket upgrade failed: {}", e),
            }
        });

        response
            .body(Full::new(Bytes::new()))
            .map_err(|e| ProxyError::Http(e.to_string()))
    }

    async fn open_relay_stream(
        &self,
        mut req: Request<Incoming>,
        relay: &RelayProxyWithAuth,
    ) -> Result<BufReader<TcpStream>, ProxyError> {
        if let Some(ref auth) = relay.auth {
            let auth_value = HeaderValue::from_str(auth)
                .map_err(|e| ProxyError::Config(format!("Invalid auth header: {}", e)))?;
            req.headers_mut().insert(PROXY_AUTHORIZATION, auth_value);
        }

        let relay_uri = relay.url.parse::<Uri>()
            .map_err(|e| ProxyError::Config(format!("Invalid relay proxy URL: {}", e)))?;

        let relay_host = relay_uri.host()
            .ok_or_else(|| ProxyError::Config("Relay proxy URL missing host".to_string()))?;
        let relay_port = relay_uri.port_u16().unwrap_or(8080);
        let relay_addr = format!("{}:{}", relay_host, relay_port);

        let mut stream = TcpStream::connect(&relay_addr).await
            .map_err(|e| ProxyError::Connection(format!("Failed to connect to relay proxy: {}", e)))?;

        let request_line = format!("{} {} HTTP/1.1\r\n", req.method(), req.uri());
        stream.write_all(request_line.as_bytes()).await
            .map_err(|e| ProxyError::Connection(format!("Failed to send request line: {}", e)))?;

        for (name, value) in req.headers() {
            let header_line = format!("{}: {}\r\n", name.as_str(), value.to_str().unwrap_or(""));
            stream.write_all(header_line.as_bytes()).await
                .map_err(|e| ProxyError::Connection(format!("Failed to send header: {}", e)))?;
        }

        stream.write_all(b"\r\n").await
            .map_err(|e| ProxyError::Connection(format!("Failed to terminate headers: {}", e)))?;

        let body_bytes = req.into_body().collect().await
            .map_err(|e| ProxyError::Http(format!("Failed to read request body: {}", e)))?
            .to_bytes();

        if !body_bytes.is_empty() {
            stream.write_all(&body_bytes).await
                .map_err(|e| ProxyError::Connection(format!("Failed to send body: {}", e)))?;
        }

        stream.flush().await
            .map_err(|e| ProxyError::Connection(format!("Failed to flush relay request: {}", e)))?;

        Ok(BufReader::new(stream))
    }

    async fn parse_relay_status_and_headers(
        reader: &mut BufReader<TcpStream>,
    ) -> Result<(u16, hyper::HeaderMap, Option<usize>, bool), ProxyError> {
        let mut status_line = String::new();
        reader.read_line(&mut status_line).await
            .map_err(|e| ProxyError::Connection(format!("Failed to read status line: {}", e)))?;

        let parts: Vec<&str> = status_line.trim().split(' ').collect();
        let status_code = if parts.len() >= 2 {
            parts[1].parse::<u16>().unwrap_or(502)
        } else {
            502
        };

        let mut response_headers = hyper::HeaderMap::new();
        let mut content_length: Option<usize> = None;
        let mut chunked = false;

        loop {
            let mut header_line = String::new();
            reader.read_line(&mut header_line).await
                .map_err(|e| ProxyError::Connection(format!("Failed to read header: {}", e)))?;

            if header_line.trim().is_empty() {
                break;
            }

            if let Some(colon_pos) = header_line.find(':') {
                let name = header_line[..colon_pos].trim();
                let value = header_line[colon_pos + 1..].trim();

                if name.eq_ignore_ascii_case("content-length") {
                    content_length = value.parse().ok();
                } else if name.eq_ignore_ascii_case("transfer-encoding") && value.contains("chunked") {
                    chunked = true;
                }

                if let Ok(header_name) = hyper::header::HeaderName::from_bytes(name.as_bytes()) {
                    if let Ok(header_value) = hyper::header::HeaderValue::from_str(value) {
                        response_headers.insert(header_name, header_value);
                    }
                }
            }
        }

        Ok((status_code, response_headers, content_length, chunked))
    }

    async fn read_relay_body(
        reader: &mut BufReader<TcpStream>,
        content_length: Option<usize>,
        chunked: bool,
    ) -> Result<Vec<u8>, ProxyError> {
        if chunked {
            let mut body = Vec::new();
            loop {
                let mut chunk_size_line = String::new();
                reader.read_line(&mut chunk_size_line).await
                    .map_err(|e| ProxyError::Connection(format!("Failed to read chunk size: {}", e)))?;

                let chunk_size = usize::from_str_radix(chunk_size_line.trim(), 16)
                    .map_err(|e| ProxyError::Http(format!("Invalid chunk size: {}", e)))?;

                if chunk_size == 0 {
                    break;
                }

                let mut chunk = vec![0u8; chunk_size];
                reader.read_exact(&mut chunk).await
                    .map_err(|e| ProxyError::Connection(format!("Failed to read chunk: {}", e)))?;
                body.extend_from_slice(&chunk);

                let mut trailing = [0u8; 2];
                reader.read_exact(&mut trailing).await
                    .map_err(|e| ProxyError::Connection(format!("Failed to read chunk trailer: {}", e)))?;
            }
            Ok(body)
        } else if let Some(len) = content_length {
            let mut body = vec![0u8; len];
            reader.read_exact(&mut body).await
                .map_err(|e| ProxyError::Connection(format!("Failed to read body: {}", e)))?;
            Ok(body)
        } else {
            let mut body = Vec::new();
            reader.read_to_end(&mut body).await
                .map_err(|e| ProxyError::Connection(format!("Failed to read body: {}", e)))?;
            Ok(body)
        }
    }

    async fn finalize_standard_response(
        response: Response<Incoming>,
    ) -> Result<Response<Full<Bytes>>, ProxyError> {
        let (parts, body) = response.into_parts();
        let body_bytes = body.collect().await
            .map_err(|e| ProxyError::Http(format!("Failed to read response body: {}", e)))?;

        Ok(Response::from_parts(parts, Full::new(body_bytes.to_bytes())))
    }

    async fn setup_tunnel_with_lifetime(
        client_stream: TcpStream,
        target_stream: TcpStream,
        client_addr: SocketAddr,
        target_desc: String,
        max_lifetime: Duration,
    ) -> Result<(), std::io::Error> {
        info!(
            "Setting up bidirectional tunnel between {} and {} (max_lifetime: {:?})",
            client_addr, target_desc, max_lifetime
        );

        let tunnel_future = Self::setup_tunnel(
            client_stream,
            target_stream,
            client_addr.clone(),
            target_desc.clone(),
        );

        match tokio::time::timeout(max_lifetime, tunnel_future).await {
            Ok(result) => {
                info!("Tunnel closed normally between {} and {}", client_addr, target_desc);
                result
            }
            Err(_) => {
                info!(
                    "Tunnel max lifetime reached ({:?}), closing connection between {} and {}",
                    max_lifetime, client_addr, target_desc
                );
                Ok(())
            }
        }
    }

    async fn setup_tunnel(
        client_stream: TcpStream,
        target_stream: TcpStream,
        client_addr: SocketAddr,
        target_desc: String,
    ) -> Result<(), std::io::Error> {
        info!(
            "Setting up bidirectional tunnel between {} and {}",
            client_addr, target_desc
        );

        let (client_read, client_write) = client_stream.into_split();
        let (target_read, target_write) = target_stream.into_split();

        let c2t = tokio::spawn(async move {
            let mut client_read = client_read;
            let mut target_write = target_write;
            if let Err(e) = tokio::io::copy(&mut client_read, &mut target_write).await {
                error!("Error copying client to target: {}", e);
            }
        });

        let t2c = tokio::spawn(async move {
            let mut target_read = target_read;
            let mut client_write = client_write;
            if let Err(e) = tokio::io::copy(&mut target_read, &mut client_write).await {
                error!("Error copying target to client: {}", e);
            }
        });

        let _ = tokio::join!(c2t, t2c);

        info!("Tunnel closed between {} and {}", client_addr, target_desc);
        Ok(())
    }

    fn find_relay_proxy_for_domain(&self, host: &str) -> Option<RelayProxyWithAuth> {
        for relay in &self.relay_proxies {
            if relay.domains.is_empty() {
                return Some(relay.clone());
            }

            if Self::matches_no_proxy_pattern(host, &relay.domains) {
                return Some(relay.clone());
            }
        }
        None
    }

    fn matches_no_proxy_pattern(host: &str, patterns: &[String]) -> bool {
        let host_lower = host.to_lowercase();

        for pattern in patterns {
            let pattern_lower = pattern.to_lowercase();

            if pattern_lower.starts_with("*.") {
                let domain = &pattern_lower[2..];
                if host_lower.ends_with(domain) && host_lower != domain {
                    return true;
                }
            } else if pattern_lower.starts_with('.') {
                let domain = &pattern_lower[1..];
                if host_lower.ends_with(domain) && host_lower != domain {
                    return true;
                }
            } else if host_lower == pattern_lower
                || host_lower.ends_with(&(String::from(".") + &pattern_lower))
            {
                return true;
            }
        }

        false
    }

    #[allow(dead_code)]
    fn should_use_relay_proxy(&self, host: &str) -> bool {
        self.find_relay_proxy_for_domain(host).is_some()
    }

    async fn connect_via_relay(
        relay_url: &str,
        relay_auth: &Option<String>,
        target_host: &str,
        target_port: u16,
    ) -> Result<TcpStream, std::io::Error> {
        let relay_parsed = Url::parse(relay_url)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;

        let relay_host = relay_parsed.host_str().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid relay proxy host")
        })?;
        let relay_port = relay_parsed.port().unwrap_or(8080);

        let mut stream = TcpStream::connect(format!("{}:{}", relay_host, relay_port)).await?;

        let connect_request = if let Some(auth) = relay_auth {
            format!(
                "CONNECT {}:{} HTTP/1.1\r\nHost: {}:{}\r\nProxy-Authorization: {}\r\n\r\n",
                target_host, target_port, target_host, target_port, auth
            )
        } else {
            format!(
                "CONNECT {}:{} HTTP/1.1\r\nHost: {}:{}\r\n\r\n",
                target_host, target_port, target_host, target_port
            )
        };

        tokio::io::AsyncWriteExt::write_all(&mut stream, connect_request.as_bytes()).await?;

        let mut response_buf = [0u8; 1024];
        let n = tokio::io::AsyncReadExt::read(&mut stream, &mut response_buf).await?;
        let response = String::from_utf8_lossy(&response_buf[..n]);

        if !response.starts_with("HTTP/1.1 200") && !response.starts_with("HTTP/1.0 200") {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!(
                    "Relay proxy rejected CONNECT: {}",
                    response.lines().next().unwrap_or("Unknown error")
                ),
            ));
        }

        Ok(stream)
    }

    fn extract_target_uri<B>(&self, req: &Request<B>) -> Result<Uri, ProxyError> {
        let original_uri = req.uri();

        if original_uri.scheme().is_some() && original_uri.authority().is_some() {
            return Ok(original_uri.clone());
        }

        if let Some(host) = req.headers().get(HOST) {
            let host_str = host
                .to_str()
                .map_err(|e| ProxyError::Config(format!("Invalid Host header: {}", e)))?;

            let absolute_url = if original_uri.path_and_query().is_some() {
                format!("http://{}{}", host_str, original_uri)
            } else {
                format!("http://{}", host_str)
            };

            let url = Url::parse(&absolute_url)?;
            let uri: Uri = url
                .as_str()
                .parse()
                .map_err(|e: hyper::http::uri::InvalidUri| ProxyError::Uri(e.to_string()))?;

            return Ok(uri);
        }

        Err(ProxyError::Config("Cannot determine target URI".to_string()))
    }

    fn validate_websocket_headers(&self, headers: &hyper::HeaderMap) -> Result<(), String> {
        if !self.websocket_config.enabled {
            return Err("WebSocket support is disabled".to_string());
        }

        if self.websocket_config.allowed_origins.iter().all(|origin| origin != "*") {
            let origin = headers.get(ORIGIN)
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| "Origin header is required for WebSocket requests".to_string())?;

            if !self.websocket_config.allowed_origins.iter().any(|allowed| allowed.eq_ignore_ascii_case(origin)) {
                return Err("Origin not allowed".to_string());
            }
        }

        if !self.websocket_config.supported_protocols.is_empty() {
            let offered = headers.get(SEC_WEBSOCKET_PROTOCOL)
                .and_then(|v| v.to_str().ok())
                .map(|raw| raw.split(',').map(|s| s.trim().to_string()).collect::<Vec<_>>())
                .unwrap_or_else(Vec::new);

            if offered.is_empty() {
                return Err("WebSocket subprotocol required".to_string());
            }

            if !offered.iter().any(|offer| {
                self.websocket_config.supported_protocols.iter().any(|allowed| allowed.eq_ignore_ascii_case(offer))
            }) {
                return Err("Unsupported WebSocket subprotocol".to_string());
            }
        }

        Ok(())
    }

    /// Verify Basic Authentication credentials from Proxy-Authorization header
    fn verify_authentication(&self, req: &Request<Incoming>) -> Result<(), ProxyError> {
        // If no credentials are configured, allow all requests
        if self.proxy_username.is_none() && self.proxy_password.is_none() {
            return Ok(());
        }

        // Check for Proxy-Authorization header
        let auth_header = req.headers()
            .get("Proxy-Authorization")
            .ok_or_else(|| ProxyError::Auth("Missing Proxy-Authorization header".to_string()))?;

        // Parse the header value
        let auth_str = auth_header.to_str()
            .map_err(|_| ProxyError::Auth("Invalid Proxy-Authorization header".to_string()))?;

        // Check if it starts with "Basic "
        if !auth_str.starts_with("Basic ") {
            return Err(ProxyError::Auth("Unsupported authentication method".to_string()));
        }

        // Decode the base64 credentials
        let encoded = &auth_str[6..]; // Remove "Basic " prefix
        let decoded = general_purpose::STANDARD.decode(encoded)
            .map_err(|_| ProxyError::Auth("Invalid base64 encoding".to_string()))?;
        let credentials = String::from_utf8(decoded)
            .map_err(|_| ProxyError::Auth("Invalid UTF-8 in credentials".to_string()))?;

        // Split username:password
        let parts: Vec<&str> = credentials.splitn(2, ':').collect();
        if parts.len() != 2 {
            return Err(ProxyError::Auth("Invalid credentials format".to_string()));
        }

        let (username, password) = (parts[0], parts[1]);

        // Verify credentials
        if Some(username) == self.proxy_username.as_deref() && Some(password) == self.proxy_password.as_deref() {
            Ok(())
        } else {
            Err(ProxyError::Auth("Invalid username or password".to_string()))
        }
    }

    /// Static helper method to find relay proxy for a domain
    fn find_relay_proxy_for_domain_static(relay_proxies: &[RelayProxyWithAuth], host: &str) -> Option<RelayProxyWithAuth> {
        for relay in relay_proxies {
            if relay.domains.is_empty() {
                return Some(relay.clone());
            }
            if Self::matches_no_proxy_pattern(host, &relay.domains) {
                return Some(relay.clone());
            }
        }
        None
    }

    /// Static helper to handle HTTP requests
    async fn handle_request_static(
        req: Request<Incoming>,
        http_client: Arc<Client<HttpConnector, Incoming>>,
        relay_proxies: Vec<RelayProxyWithAuth>,
        proxy_username: Option<String>,
        proxy_password: Option<String>,
        websocket_config: WebSocketConfig,
        rate_limiter: Arc<RateLimiter>,
        client_ip: Option<String>,
    ) -> Result<Response<Full<Bytes>>, Infallible> {
        // Create a temporary proxy instance for request handling
        // Note: HTTP client is passed in, not using instance's client
        let proxy = ForwardProxy {
            connection_pool_enabled: true,
            max_connection_lifetime: Duration::from_secs(300), // Default value for temporary instance
            relay_proxies,
            proxy_username,
            proxy_password,
            http_client,
            websocket_config,
            rate_limiter,
        };
        proxy.handle_request(req, client_ip).await
    }

    /// Static helper to handle CONNECT tunnels
    async fn handle_connect_tunnel_static(
        req: Request<Incoming>,
        relay_proxies: Vec<RelayProxyWithAuth>,
        websocket_config: WebSocketConfig,
        rate_limiter: Arc<RateLimiter>,
        client_ip: Option<String>,
    ) -> Result<Response<Full<Bytes>>, Infallible> {
        // For CONNECT, we don't need the HTTP client
        let proxy = ForwardProxy {
            connection_pool_enabled: true,
            max_connection_lifetime: Duration::from_secs(300), // Default value for temporary instance
            relay_proxies,
            proxy_username: None,
            proxy_password: None,
            http_client: Arc::new(Self::build_http_client(10, 90, true)),
            websocket_config,
            rate_limiter,
        };
        proxy.handle_connect_tunnel(req, client_ip).await
    }

}

// TLS configuration is now handled by TlsConfig::create_config in common.rs

#[cfg(test)]
mod tests {
    use super::*;
    use hyper::{Method, Uri};
    use http_body_util::Empty;

    #[test]
    fn test_target_uri_extraction() {
        let proxy = ForwardProxy::new(10, 90, 300);

        // Test absolute URI
        let absolute_uri: Uri = "http://example.com/path".parse().unwrap();
        let req = Request::builder()
            .method(Method::GET)
            .uri(absolute_uri.clone())
            .body(Empty::<Bytes>::new())
            .unwrap();

        let extracted = proxy.extract_target_uri(&req).unwrap();
        assert_eq!(extracted, absolute_uri);
    }

    #[test]
    fn test_no_proxy_pattern_matching() {
        // Test exact domain match
        assert!(ForwardProxy::matches_no_proxy_pattern("example.com", &["example.com".to_string()]));
        
        // Test subdomain match with plain domain
        assert!(ForwardProxy::matches_no_proxy_pattern("sub.example.com", &["example.com".to_string()]));
        
        // Test wildcard pattern
        assert!(ForwardProxy::matches_no_proxy_pattern("sub.example.com", &["*.example.com".to_string()]));
        assert!(!ForwardProxy::matches_no_proxy_pattern("example.com", &["*.example.com".to_string()]));
        
        // Test dot prefix pattern
        assert!(ForwardProxy::matches_no_proxy_pattern("sub.example.com", &[".example.com".to_string()]));
        assert!(!ForwardProxy::matches_no_proxy_pattern("example.com", &[".example.com".to_string()]));
        
        // Test no match
        assert!(!ForwardProxy::matches_no_proxy_pattern("other.com", &["example.com".to_string()]));
        
        // Test case insensitivity
        assert!(ForwardProxy::matches_no_proxy_pattern("EXAMPLE.COM", &["example.com".to_string()]));
    }
}
