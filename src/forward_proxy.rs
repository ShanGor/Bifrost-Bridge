//! Forward proxy implementation supporting HTTP and HTTPS protocols.
//!
//! This module provides a forward proxy server with the following features:
//! - HTTP/HTTPS CONNECT tunneling for HTTPS requests
//! - Relay proxy support with domain-based routing
//! - Basic proxy authentication
//! - Connection pooling and timeout configuration

use crate::error::ProxyError;
use crate::config::RelayProxyConfig;
use crate::common::{ResponseBuilder, TlsConfig};
use rustls::ServerConfig;
use hyper::{Request, Response, StatusCode, Uri, Method};
use hyper::body::{Bytes, Incoming};
use http_body_util::{BodyExt, Full};
use hyper::server::conn::http1::Builder as ServerBuilder;
use hyper::service::service_fn;
use log::{info, error, debug};
use hyper_util::rt::TokioIo;
use hyper::header::{HOST, PROXY_AUTHORIZATION, HeaderValue};
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::time::Duration;
use tokio::net::{TcpListener, TcpStream};
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
        }
    }

    /// Build HTTP client for forward proxy.
    ///
    /// Forward proxy pooling strategy:
    /// - Allows connection reuse for the SAME target host during active usage
    /// - Once a connection becomes idle (no requests for idle_timeout), it's closed
    /// - Does NOT maintain a persistent pool of idle connections waiting
    /// - pool_max_idle_per_host = 0: No idle connections are kept
    /// - pool_idle_timeout: Short timeout (10-30s) to close unused connections quickly
    ///
    /// Example: Multiple requests to api.example.com can reuse the same connection,
    /// but once requests stop, the connection closes after idle_timeout.
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
        
        // Forward proxy strategy: pool_max_idle_per_host = 0
        // This means: connections can be reused while active, but once idle, they close
        // We don't maintain a "waiting pool" of idle connections
        info!("Forward proxy: pool_max_idle_per_host=0 (no persistent idle connection pool)");
        builder.pool_max_idle_per_host(0);
        
        if pool_enabled {
            // Short idle timeout: close connections quickly when not in use
            // Typical value: 10-30s (configured by user via idle_timeout_secs)
            info!("Forward proxy: connection reuse enabled, idle connections close after {}s",
                  idle_timeout_secs);
            builder.pool_idle_timeout(Duration::from_secs(idle_timeout_secs));
            builder.pool_timer(TokioTimer::new());
        } else {
            info!("Forward proxy: no-pool mode (new connection per request)");
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
                            async move {
                                // Check if this is a CONNECT request
                                if req.method() == Method::CONNECT {
                                    Self::handle_connect_tunnel_static(req, relay_proxies).await
                                } else {
                                    Self::handle_request_static(req, http_client, relay_proxies, proxy_username, proxy_password).await
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
        _remote_addr: SocketAddr,
        relay_proxies: Vec<RelayProxyWithAuth>,
        _proxy_username: Option<String>,
        _proxy_password: Option<String>,
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
            _remote_addr,
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
            let (tcp_stream, _) = tcp_listener.accept().await
                .map_err(|e| ProxyError::Io(e))?;

            let relay_proxies = relay_proxies.clone();
            let proxy_username = proxy_username.clone();
            let proxy_password = proxy_password.clone();
            let tls_acceptor = tls_acceptor.clone();
            let http_client = http_client.clone(); // Clone the Arc for the spawn

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
                                async move {
                                    // Check if this is a CONNECT request
                                    if req.method() == Method::CONNECT {
                                        ForwardProxy::handle_connect_tunnel_static(req, relay_proxies).await
                                    } else {
                                        ForwardProxy::handle_request_static(req, http_client, relay_proxies, proxy_username, proxy_password).await
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

    async fn handle_request(&self, req: Request<Incoming>) -> Result<Response<Full<Bytes>>, Infallible> {
        match self.process_request(req).await {
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

    async fn process_request(&self, mut req: Request<Incoming>) -> Result<Response<Full<Bytes>>, ProxyError> {
        // Verify authentication credentials
        self.verify_authentication(&req)?;

        // Handle CONNECT method for HTTPS
        if *req.method() == Method::CONNECT {
            return match self.handle_connect_tunnel(req).await {
                Ok(response) => Ok(response),
                Err(_) => unreachable!(), // Infallible means this never happens
            };
        }

        // Extract target URL from request
        let target_uri = self.extract_target_uri(&req)?;

        // Log HTTP request forwarding
        let host = target_uri.host().unwrap_or("unknown");
        let port = target_uri.port_u16().unwrap_or(80);
        let scheme = target_uri.scheme_str().unwrap_or("http");
        
        // Find matching relay proxy for this domain
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

        // Reconstruct request for target server or relay proxy
        let response = if let Some(relay) = relay_proxy {
            // When using relay proxy for HTTP requests:
            // We need to connect to the relay proxy and send the request with absolute URI
            
            // Add Proxy-Authorization header if configured
            if let Some(ref auth) = relay.auth {
                let auth_value = HeaderValue::from_str(auth)
                    .map_err(|e| ProxyError::Config(format!("Invalid auth header: {}", e)))?;
                req.headers_mut().insert(PROXY_AUTHORIZATION, auth_value);
            }

            info!("Sending HTTP request to relay proxy {} for target {}", relay.url, target_uri);

            // For HTTP requests through a relay proxy:
            // The request must have an absolute URI and we connect to the relay
            
            // Parse relay proxy URL
            let relay_uri = relay.url.parse::<Uri>()
                .map_err(|e| ProxyError::Config(format!("Invalid relay proxy URL: {}", e)))?;
            
            let relay_host = relay_uri.host()
                .ok_or_else(|| ProxyError::Config("Relay proxy URL missing host".to_string()))?;
            let relay_port = relay_uri.port_u16().unwrap_or(8080);
            let relay_addr = format!("{}:{}", relay_host, relay_port);
            
            // Connect to relay proxy
            let mut stream = TcpStream::connect(&relay_addr).await
                .map_err(|e| ProxyError::Connection(format!("Failed to connect to relay proxy: {}", e)))?;
            
            // Build the HTTP request line with absolute URI
            let method = req.method().as_str();
            let uri = req.uri(); // This is the absolute target URI
            
            use tokio::io::{AsyncWriteExt, AsyncBufReadExt, BufReader};
            
            // Send request line
            let request_line = format!("{} {} HTTP/1.1\r\n", method, uri);
            stream.write_all(request_line.as_bytes()).await
                .map_err(|e| ProxyError::Connection(format!("Failed to send request line: {}", e)))?;
            
            // Send headers
            for (name, value) in req.headers() {
                let header_line = format!("{}: {}\r\n", name.as_str(), 
                    value.to_str().unwrap_or(""));
                stream.write_all(header_line.as_bytes()).await
                    .map_err(|e| ProxyError::Connection(format!("Failed to send header: {}", e)))?;
            }
            
            // End of headers
            stream.write_all(b"\r\n").await
                .map_err(|e| ProxyError::Connection(format!("Failed to send header end: {}", e)))?;
            
            // Send body if present
            let body_bytes = req.into_body().collect().await
                .map_err(|e| ProxyError::Http(format!("Failed to read request body: {}", e)))?;
            let body_data = body_bytes.to_bytes();
            if !body_data.is_empty() {
                stream.write_all(&body_data).await
                    .map_err(|e| ProxyError::Connection(format!("Failed to send body: {}", e)))?;
            }
            
            stream.flush().await
                .map_err(|e| ProxyError::Connection(format!("Failed to flush: {}", e)))?;
            
            // Read response
            let mut reader = BufReader::new(stream);
            let mut status_line = String::new();
            reader.read_line(&mut status_line).await
                .map_err(|e| ProxyError::Connection(format!("Failed to read status line: {}", e)))?;
            
            // Parse status line: HTTP/1.1 200 OK
            let parts: Vec<&str> = status_line.trim().split(' ').collect();
            let status_code = if parts.len() >= 2 {
                parts[1].parse::<u16>().unwrap_or(502)
            } else {
                502
            };
            
            // Read headers
            let mut response_headers = hyper::HeaderMap::new();
            let mut content_length: Option<usize> = None;
            let mut chunked = false;
            
            loop {
                let mut header_line = String::new();
                reader.read_line(&mut header_line).await
                    .map_err(|e| ProxyError::Connection(format!("Failed to read header: {}", e)))?;
                
                if header_line.trim().is_empty() {
                    break; // End of headers
                }
                
                // Parse header
                if let Some(colon_pos) = header_line.find(':') {
                    let name = &header_line[..colon_pos].trim();
                    let value = &header_line[colon_pos + 1..].trim();
                    
                    // Track content-length and transfer-encoding
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
            
            // Read body
            let body_bytes = if chunked {
                // Handle chunked encoding
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
                    use tokio::io::AsyncReadExt;
                    reader.read_exact(&mut chunk).await
                        .map_err(|e| ProxyError::Connection(format!("Failed to read chunk: {}", e)))?;
                    body.extend_from_slice(&chunk);
                    
                    // Read trailing \r\n
                    let mut trailing = [0u8; 2];
                    reader.read_exact(&mut trailing).await
                        .map_err(|e| ProxyError::Connection(format!("Failed to read chunk trailer: {}", e)))?;
                }
                body
            } else if let Some(len) = content_length {
                // Read exact content length
                let mut body = vec![0u8; len];
                use tokio::io::AsyncReadExt;
                reader.read_exact(&mut body).await
                    .map_err(|e| ProxyError::Connection(format!("Failed to read body: {}", e)))?;
                body
            } else {
                // Read until connection close
                let mut body = Vec::new();
                use tokio::io::AsyncReadExt;
                reader.read_to_end(&mut body).await
                    .map_err(|e| ProxyError::Connection(format!("Failed to read body: {}", e)))?;
                body
            };
            
            // Build response
            let mut response = Response::builder()
                .status(StatusCode::from_u16(status_code).unwrap_or(StatusCode::BAD_GATEWAY));
            
            // Add headers
            if let Some(headers) = response.headers_mut() {
                *headers = response_headers;
            }
            
            response.body(Full::new(Bytes::from(body_bytes)))
                .map_err(|e| ProxyError::Http(format!("Failed to build response: {}", e)))?
        } else {
            // Direct connection to target
            self.forward_direct_http_request(req, &target_uri).await?
        };

        // Log successful response
        debug!("Successfully forwarded request to {}://{}:{} - Status: {}", scheme, host, port, response.status());

        Ok(response)
    }

    /// Forwards an HTTP request directly to the target server.
    ///
    /// This method handles direct HTTP proxy connections without going through a relay.
    async fn forward_direct_http_request(
        &self,
        mut req: Request<Incoming>,
        target_uri: &Uri,
    ) -> Result<Response<Full<Bytes>>, ProxyError> {
        // Use instance-specific HTTP client that respects configuration
        let client = &self.http_client;

        // Update request URI to absolute form for target
        *req.uri_mut() = target_uri.clone();

        // Remove proxy-specific headers
        req.headers_mut().remove(PROXY_AUTHORIZATION);
        req.headers_mut().remove("Proxy-Connection");

        // Forward the request
        let response = client.request(req).await
            .map_err(|e| ProxyError::Connection(format!("Failed to forward request: {}", e)))?;

        // Convert response to Full<Bytes>
        let (parts, body) = response.into_parts();
        let body_bytes = body.collect().await
            .map_err(|e| ProxyError::Http(format!("Failed to read response body: {}", e)))?;

        Ok(Response::from_parts(parts, Full::new(body_bytes.to_bytes())))
    }

    async fn handle_connect_tunnel(&self, req: Request<Incoming>) -> Result<Response<Full<Bytes>>, Infallible> {
        // Extract host and port from CONNECT request
        let authority = match req.uri().authority() {
            Some(auth) => auth,
            None => return Ok(ResponseBuilder::error(StatusCode::BAD_REQUEST, "Invalid CONNECT target")),
        };

        let host = authority.host().to_string();
        let port = authority.port_u16().unwrap_or(443);

        info!("Handling CONNECT request to {}:{}", host, port);

        // Find matching relay proxy for this domain
        let relay_proxy = self.find_relay_proxy_for_domain(&host);
        let max_lifetime = self.max_connection_lifetime; // Capture max lifetime

        if let Some(relay) = &relay_proxy {
            info!("Connecting to {}:{} via relay proxy {}", host, port, relay.url);
        } else {
            info!("Direct connection to {}:{}", host, port);
        }

        // Spawn a task to handle the upgrade and tunnel
        tokio::spawn(async move {
            // Wait for the connection to be upgraded
            match hyper::upgrade::on(req).await {
                Ok(upgraded) => {
                    info!("Successfully upgraded connection for {}:{}", host, port);
                    
                    // Wrap upgraded connection with TokioIo for AsyncRead/AsyncWrite
                    let upgraded_io = TokioIo::new(upgraded);
                    
                    // Connect to the target (directly or via relay)
                    let target_stream = if let Some(relay) = relay_proxy {
                        match ForwardProxy::connect_via_relay(
                            &relay.url,
                            &relay.auth,
                            &host,
                            port
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
                    
                    // Set up bidirectional tunnel with max lifetime enforcement
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

                    // Run both directions concurrently with max lifetime timeout
                    let tunnel_future = async {
                        tokio::join!(client_to_target, target_to_client);
                    };

                    match tokio::time::timeout(max_lifetime, tunnel_future).await {
                        Ok(_) => {
                            info!("TCP tunnel closed normally for {}:{}", host, port);
                        }
                        Err(_) => {
                            info!("TCP tunnel max lifetime ({:?}) reached for {}:{}, closing connection", 
                                  max_lifetime, host, port);
                        }
                    }
                }
                Err(e) => {
                    error!("Failed to upgrade connection for {}:{}: {}", host, port, e);
                }
            }
        });

        // Return 200 OK to signal that tunnel is being established
        Ok(Response::builder()
            .status(StatusCode::OK)
            .body(Full::new(Bytes::new()))
            .unwrap())
    }

    /// Sets up a bidirectional TCP tunnel between client and target.
    ///
    /// Copies data in both directions until one side closes the connection.
    /// Set up bidirectional tunnel with maximum connection lifetime enforcement.
    ///
    /// Similar to Netty's maximum connection lifetime feature, this ensures that
    /// connections are automatically closed after the specified duration, regardless
    /// of activity. This is important for load balancing and preventing stale connections.
    async fn setup_tunnel_with_lifetime(
        client_stream: TcpStream,
        target_stream: TcpStream,
        client_addr: SocketAddr,
        target_desc: String,
        max_lifetime: Duration,
    ) -> Result<(), std::io::Error> {
        info!("Setting up bidirectional tunnel between {} and {} (max_lifetime: {:?})", 
              client_addr, target_desc, max_lifetime);

        // Wrap the tunnel operation with a timeout
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
                info!("Tunnel max lifetime reached ({:?}), closing connection between {} and {}",
                      max_lifetime, client_addr, target_desc);
                Ok(())
            }
        }
    }

    /// Set up bidirectional tunnel without lifetime limit (internal method).
    async fn setup_tunnel(
        client_stream: TcpStream,
        target_stream: TcpStream,
        client_addr: SocketAddr,
        target_desc: String,
    ) -> Result<(), std::io::Error> {
        info!("Setting up bidirectional tunnel between {} and {}", client_addr, target_desc);

        let (client_read, client_write) = client_stream.into_split();
        let (target_read, target_write) = target_stream.into_split();

        // Use asyncRwLock or spawn tasks with owned halves
        // Tunnel data from client to target
        let c2t = tokio::spawn(async move {
            let mut client_read = client_read;
            let mut target_write = target_write;
            if let Err(e) = tokio::io::copy(&mut client_read, &mut target_write).await {
                error!("Error copying client to target: {}", e);
            }
        });

        // Tunnel data from target to client
        let t2c = tokio::spawn(async move {
            let mut target_read = target_read;
            let mut client_write = client_write;
            if let Err(e) = tokio::io::copy(&mut target_read, &mut client_write).await {
                error!("Error copying target to client: {}", e);
            }
        });

        // Wait for both directions to complete
        let _ = tokio::join!(c2t, t2c);

        info!("Tunnel closed between {} and {}", client_addr, target_desc);
        Ok(())
    }

    /// Finds the first matching relay proxy for the given domain.
    ///
    /// Returns `None` if no relay proxy matches (indicating direct connection).
    /// Matches are based on NO_PROXY format patterns.
    fn find_relay_proxy_for_domain(&self, host: &str) -> Option<RelayProxyWithAuth> {
        for relay in &self.relay_proxies {
            // If no domains configured for this relay, it matches all domains
            if relay.domains.is_empty() {
                return Some(relay.clone());
            }
            
            // Check if host matches any domain pattern (NO_PROXY format)
            if Self::matches_no_proxy_pattern(host, &relay.domains) {
                return Some(relay.clone());
            }
        }
        None
    }

    /// Checks if a host matches any NO_PROXY format patterns.
    ///
    /// Supported NO_PROXY formats:
    /// - `"example.com"` - matches example.com and *.example.com
    /// - `".example.com"` - matches *.example.com only
    /// - `"*.example.com"` - matches *.example.com only
    /// - `"subdomain.example.com"` - matches subdomain.example.com exactly
    ///
    /// Matching is case-insensitive.
    fn matches_no_proxy_pattern(host: &str, patterns: &[String]) -> bool {
        let host_lower = host.to_lowercase();
        
        for pattern in patterns {
            let pattern_lower = pattern.to_lowercase();
            
            if pattern_lower.starts_with("*.") {
                // *.example.com - match any subdomain of example.com
                let domain = &pattern_lower[2..];
                if host_lower.ends_with(domain) && host_lower != domain {
                    return true;
                }
            } else if pattern_lower.starts_with(".") {
                // .example.com - match any subdomain of example.com
                let domain = &pattern_lower[1..];
                if host_lower.ends_with(domain) && host_lower != domain {
                    return true;
                }
            } else {
                // example.com - match exact domain or any subdomain
                if host_lower == pattern_lower || host_lower.ends_with(&(String::from(".") + &pattern_lower)) {
                    return true;
                }
            }
        }
        
        false
    }

    // Check if a domain should use relay proxy based on configured suffixes (DEPRECATED)
    #[allow(dead_code)]
    fn should_use_relay_proxy(&self, host: &str) -> bool {
        self.find_relay_proxy_for_domain(host).is_some()
    }

    /// Connects to a target host through a relay proxy.
    ///
    /// Establishes a CONNECT tunnel through the relay proxy to the target destination.
    async fn connect_via_relay(
        relay_url: &str,
        relay_auth: &Option<String>,
        target_host: &str,
        target_port: u16,
    ) -> Result<TcpStream, std::io::Error> {
        // Parse relay proxy URL
        let relay_parsed = Url::parse(relay_url)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
        
        let relay_host = relay_parsed.host_str()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid relay proxy host"))?;
        let relay_port = relay_parsed.port().unwrap_or(8080);

        // Connect to relay proxy
        let mut stream = TcpStream::connect(format!("{}:{}", relay_host, relay_port)).await?;

        // Send CONNECT request to relay proxy
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

        // Write CONNECT request
        tokio::io::AsyncWriteExt::write_all(&mut stream, connect_request.as_bytes()).await?;

        // Read response from relay proxy
        let mut response_buf = [0u8; 1024];
        let n = tokio::io::AsyncReadExt::read(&mut stream, &mut response_buf).await?;
        let response = String::from_utf8_lossy(&response_buf[..n]);

        // Check if connection was established
        if !response.starts_with("HTTP/1.1 200") && !response.starts_with("HTTP/1.0 200") {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!("Relay proxy rejected CONNECT: {}", response.lines().next().unwrap_or("Unknown error")),
            ));
        }

        Ok(stream)
    }

    fn extract_target_uri<B>(&self, req: &Request<B>) -> Result<Uri, ProxyError> {
        let original_uri = req.uri();

        // If URI is absolute (contains scheme and host), use it directly
        if original_uri.scheme().is_some() && original_uri.authority().is_some() {
            return Ok(original_uri.clone());
        }

        // For relative URIs, use Host header to construct absolute URI
        if let Some(host) = req.headers().get(HOST) {
            let host_str = host.to_str()
                .map_err(|e| ProxyError::Config(format!("Invalid Host header: {}", e)))?;

            let absolute_url = if original_uri.path_and_query().is_some() {
                format!("http://{}{}", host_str, original_uri)
            } else {
                format!("http://{}", host_str)
            };

            let url = Url::parse(&absolute_url)?;
            let uri: Uri = url.as_str().parse()
                .map_err(|e: hyper::http::uri::InvalidUri| ProxyError::Uri(e.to_string()))?;

            return Ok(uri);
        }

        Err(ProxyError::Config("Cannot determine target URI".to_string()))
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
        };
        proxy.handle_request(req).await
    }

    /// Static helper to handle CONNECT tunnels
    async fn handle_connect_tunnel_static(
        req: Request<Incoming>,
        relay_proxies: Vec<RelayProxyWithAuth>,
    ) -> Result<Response<Full<Bytes>>, Infallible> {
        // For CONNECT, we don't need the HTTP client
        let proxy = ForwardProxy {
            connection_pool_enabled: true,
            max_connection_lifetime: Duration::from_secs(300), // Default value for temporary instance
            relay_proxies,
            proxy_username: None,
            proxy_password: None,
            http_client: Arc::new(Self::build_http_client(10, 90, true)),
        };
        proxy.handle_connect_tunnel(req).await
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