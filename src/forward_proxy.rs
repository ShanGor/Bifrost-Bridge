use crate::error::ProxyError;
use crate::config::RelayProxyConfig;
use hyper::{Body, Client, Method, Request, Response, Server, StatusCode, Uri};
use hyper::client::HttpConnector;
use hyper_tls::HttpsConnector;
use hyper::service::{make_service_fn, service_fn};
use hyper::header::{HOST, CONNECTION, PROXY_AUTHORIZATION, HeaderValue};
use std::convert::Infallible;
use std::fs::File;
use std::io::BufReader;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::time::{timeout, Duration};
use tokio::net::{TcpListener, TcpStream};
use url::Url;
use rustls::ServerConfig;
use tokio_rustls::TlsAcceptor;
use base64::{Engine as _, engine::general_purpose};

pub struct ForwardProxy {
    client: Client<HttpsConnector<HttpConnector>>, // HTTPS client for direct connections
    connect_timeout: Duration,
    idle_timeout: Duration,
    max_connection_lifetime: Duration,
    connection_pool_enabled: bool,
    pool_max_idle_per_host: usize,
    relay_proxies: Vec<RelayProxyWithAuth>, // Multiple relay proxies with pre-computed auth
    proxy_username: Option<String>, // Username for proxy authentication
    proxy_password: Option<String>, // Password for proxy authentication
}

// Internal structure to store relay proxy config with pre-computed auth header
#[derive(Clone)]
struct RelayProxyWithAuth {
    url: String,
    auth: Option<String>, // Base64 encoded "username:password"
    domains: Vec<String>,  // Domain patterns in NO_PROXY format
}

impl ForwardProxy {
    pub fn new(connect_timeout_secs: u64, idle_timeout_secs: u64, max_connection_lifetime_secs: u64) -> Self {
        Self {
            client: Client::builder()
                .pool_max_idle_per_host(10)
                .pool_idle_timeout(Duration::from_secs(idle_timeout_secs))
                .build(HttpsConnector::new()),
            connect_timeout: Duration::from_secs(connect_timeout_secs),
            idle_timeout: Duration::from_secs(idle_timeout_secs),
            max_connection_lifetime: Duration::from_secs(max_connection_lifetime_secs),
            connection_pool_enabled: true,
            pool_max_idle_per_host: 10,
            relay_proxies: Vec::new(),
            proxy_username: None,
            proxy_password: None,
        }
    }

    pub fn new_with_pool_config(
        connect_timeout_secs: u64,
        idle_timeout_secs: u64,
        max_connection_lifetime_secs: u64,
        connection_pool_enabled: bool,
        pool_max_idle_per_host: usize,
    ) -> Self {
        Self {
            client: Client::builder()
                .pool_max_idle_per_host(pool_max_idle_per_host)
                .pool_idle_timeout(Duration::from_secs(idle_timeout_secs))
                .build(HttpsConnector::new()),
            connect_timeout: Duration::from_secs(connect_timeout_secs),
            idle_timeout: Duration::from_secs(idle_timeout_secs),
            max_connection_lifetime: Duration::from_secs(max_connection_lifetime_secs),
            connection_pool_enabled,
            pool_max_idle_per_host,
            relay_proxies: Vec::new(),
            proxy_username: None,
            proxy_password: None,
        }
    }

    pub fn new_with_relay(
        connect_timeout_secs: u64,
        idle_timeout_secs: u64,
        max_connection_lifetime_secs: u64,
        connection_pool_enabled: bool,
        pool_max_idle_per_host: usize,
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
            pool_max_idle_per_host,
            relay_configs,
            None,
            None,
        )
    }

    pub fn new_with_relay_proxies(
        connect_timeout_secs: u64,
        idle_timeout_secs: u64,
        max_connection_lifetime_secs: u64,
        connection_pool_enabled: bool,
        pool_max_idle_per_host: usize,
        relay_configs: Vec<RelayProxyConfig>,
        proxy_username: Option<String>,
        proxy_password: Option<String>,
    ) -> Self {
        let client = if connection_pool_enabled {
            Client::builder()
                .pool_max_idle_per_host(pool_max_idle_per_host)
                .pool_idle_timeout(Duration::from_secs(idle_timeout_secs))
                .build(HttpsConnector::new())
        } else {
            Client::builder()
                .pool_max_idle_per_host(0)
                .build(HttpsConnector::new())
        };

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

        Self {
            client,
            connect_timeout: Duration::from_secs(connect_timeout_secs),
            idle_timeout: Duration::from_secs(idle_timeout_secs),
            max_connection_lifetime: Duration::from_secs(max_connection_lifetime_secs),
            connection_pool_enabled,
            pool_max_idle_per_host,
            relay_proxies,
            proxy_username,
            proxy_password,
        }
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
                let tls_config = create_tls_config(&private_key_path, &cert_path)?;
                self.run_https(addr, Some(Arc::new(tls_config))).await
            }
            _ => {
                // HTTP mode
                self.run_http(addr).await
            }
        }
    }

    async fn run_http(self, addr: SocketAddr) -> Result<(), ProxyError> {
        let connect_timeout = self.connect_timeout;
        let idle_timeout = self.idle_timeout;
        let max_connection_lifetime = self.max_connection_lifetime;
        let connection_pool_enabled = self.connection_pool_enabled;
        let pool_max_idle_per_host = self.pool_max_idle_per_host;
        let relay_proxies = self.relay_proxies.clone();
        let proxy_username = self.proxy_username;
        let proxy_password = self.proxy_password;

        let make_svc = make_service_fn(move |_conn| {
            let connect_timeout = connect_timeout;
            let idle_timeout = idle_timeout;
            let max_connection_lifetime = max_connection_lifetime;
            let connection_pool_enabled = connection_pool_enabled;
            let pool_max_idle_per_host = pool_max_idle_per_host;
            let relay_proxies = relay_proxies.clone();
            let proxy_username = proxy_username.clone();
            let proxy_password = proxy_password.clone();
            async move {
                Ok::<_, Infallible>(service_fn(move |req| {
                    let client = if connection_pool_enabled {
                        Client::builder()
                            .pool_max_idle_per_host(pool_max_idle_per_host)
                            .pool_idle_timeout(idle_timeout)
                            .build(HttpsConnector::new())
                    } else {
                        Client::builder()
                            .pool_max_idle_per_host(0)
                            .build(HttpsConnector::new())
                    };
                    let proxy = ForwardProxy {
                        client,
                        connect_timeout,
                        idle_timeout,
                        max_connection_lifetime,
                        connection_pool_enabled,
                        pool_max_idle_per_host,
                        relay_proxies: relay_proxies.clone(),
                        proxy_username: proxy_username.clone(),
                        proxy_password: proxy_password.clone(),
                    };
                    async move {
                        proxy.handle_request(req).await
                    }
                }))
            }
        });

        let server = Server::bind(&addr).serve(make_svc);
        println!("HTTP forward proxy listening on: {}", addr);

        if let Err(e) = server.await {
            eprintln!("Server error: {}", e);
            return Err(ProxyError::Hyper(e.to_string()));
        }

        Ok(())
    }

    async fn run_https(self, addr: SocketAddr, tls_config: Option<Arc<ServerConfig>>) -> Result<(), ProxyError> {
        let connect_timeout = self.connect_timeout;
        let idle_timeout = self.idle_timeout;
        let max_connection_lifetime = self.max_connection_lifetime;
        let connection_pool_enabled = self.connection_pool_enabled;
        let pool_max_idle_per_host = self.pool_max_idle_per_host;
        let relay_proxies = self.relay_proxies.clone();
        let proxy_username = self.proxy_username;
        let proxy_password = self.proxy_password;
        let tls_acceptor = if let Some(config) = tls_config {
            Some(TlsAcceptor::from(config))
        } else {
            None
        };

        let tcp_listener = TcpListener::bind(&addr).await
            .map_err(|e| ProxyError::Io(e))?;

        println!("HTTPS forward proxy listening on: https://{}", addr);
        if connection_pool_enabled {
            println!("Connection pool enabled (max idle per host: {})", pool_max_idle_per_host);
        } else {
            println!("Connection pool disabled (no-pool mode)");
        }

        loop {
            let (tcp_stream, _) = tcp_listener.accept().await
                .map_err(|e| ProxyError::Io(e))?;

            let connect_timeout = connect_timeout;
            let idle_timeout = idle_timeout;
            let max_connection_lifetime = max_connection_lifetime;
            let connection_pool_enabled = connection_pool_enabled;
            let pool_max_idle_per_host = pool_max_idle_per_host;
            let relay_proxies = relay_proxies.clone();
            let proxy_username = proxy_username.clone();
            let proxy_password = proxy_password.clone();
            let tls_acceptor = tls_acceptor.clone();

            tokio::spawn(async move {
                if let Some(acceptor) = tls_acceptor {
                    // HTTPS mode
                    match acceptor.accept(tcp_stream).await {
                        Ok(tls_stream) => {
                            let service = service_fn(move |req| {
                                let client = if connection_pool_enabled {
                                    Client::builder()
                                        .pool_max_idle_per_host(pool_max_idle_per_host)
                                        .pool_idle_timeout(idle_timeout)
                                        .build(HttpsConnector::new())
                                } else {
                                    Client::builder()
                                        .pool_max_idle_per_host(0)
                                        .build(HttpsConnector::new())
                                };
                                let proxy = ForwardProxy {
                                    client,
                                    connect_timeout,
                                    idle_timeout,
                                    max_connection_lifetime,
                                    connection_pool_enabled,
                                    pool_max_idle_per_host,
                                    relay_proxies: relay_proxies.clone(),
                                    proxy_username: proxy_username.clone(),
                                    proxy_password: proxy_password.clone(),
                                };
                                async move {
                                    proxy.handle_request(req).await
                                }
                            });

                            if let Err(e) = hyper::server::conn::Http::new()
                                .http1_keep_alive(true)
                                .serve_connection(tls_stream, service)
                                .await
                            {
                                eprintln!("Error serving HTTPS connection: {}", e);
                            }
                        }
                        Err(e) => {
                            eprintln!("Error establishing TLS connection: {}", e);
                        }
                    }
                }
            });
        }
    }

    async fn handle_request(&self, req: Request<Body>) -> Result<Response<Body>, Infallible> {
        match self.process_request(req).await {
            Ok(response) => Ok(response),
            Err(e) => {
                eprintln!("Proxy error: {}", e);
                // Return 401 Unauthorized for authentication errors
                let status = if matches!(e, ProxyError::Auth(_)) {
                    StatusCode::UNAUTHORIZED
                } else {
                    StatusCode::BAD_GATEWAY
                };

                let mut response_builder = Response::builder()
                    .status(status)
                    .body(Body::from(format!("Proxy Error: {}", e)))
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

    async fn process_request(&self, mut req: Request<Body>) -> Result<Response<Body>, ProxyError> {
        // Verify authentication credentials
        self.verify_authentication(&req)?;

        // Handle CONNECT method for HTTPS
        if *req.method() == Method::CONNECT {
            return self.handle_connect(req).await;
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
            println!("HTTP request to {}://{}:{}{} via relay proxy {} (matched domain rule)", 
                    scheme, host, port, 
                    target_uri.path_and_query().map(|pq| pq.as_str()).unwrap_or(""),
                    relay.url);
        } else {
            println!("HTTP request to {}://{}:{}{} (direct connection)", 
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

            println!("Sending HTTP request to relay proxy {} for target {}", relay.url, target_uri);

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
            let body_bytes = hyper::body::to_bytes(req.into_body()).await
                .map_err(|e| ProxyError::Http(format!("Failed to read request body: {}", e)))?;
            if !body_bytes.is_empty() {
                stream.write_all(&body_bytes).await
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
            
            response.body(Body::from(body_bytes))
                .map_err(|e| ProxyError::Http(format!("Failed to build response: {}", e)))?
        } else {
            // Direct connection: reconstruct request normally
            self.reconstruct_request(&mut req, &target_uri);

            // Send request through direct connection using client
            timeout(self.connect_timeout, self.client.request(req))
                .await
                .map_err(|_| ProxyError::Connection("Request timeout".to_string()))?
                .map_err(|e| ProxyError::Http(e.to_string()))?
        };

        // Log successful response
        println!("Successfully forwarded request to {}://{}:{} - Status: {}", scheme, host, port, response.status());

        Ok(response)
    }

    async fn handle_connect(&self, req: Request<Body>) -> Result<Response<Body>, ProxyError> {
        // Extract host and port from CONNECT request
        let authority = req.uri().authority()
            .ok_or_else(|| ProxyError::Config("Invalid CONNECT target".to_string()))?;

        let host = authority.host().to_string();
        let port = authority.port_u16().unwrap_or(443);

        // Find matching relay proxy for this domain
        let relay_proxy = self.find_relay_proxy_for_domain(&host);

        if let Some(relay) = &relay_proxy {
            println!("CONNECT request to {}:{} via relay proxy {} (matched domain rule)", host, port, relay.url);
        } else {
            println!("CONNECT request to {}:{} (direct connection)", host, port);
        }

        // Spawn the upgrade and tunnel handling
        tokio::spawn(async move {
            match hyper::upgrade::on(req).await {
                Ok(upgraded) => {
                    println!("Successfully upgraded connection for {}:{}", host, port);
                    
                    // Connect to the target server (directly or via relay proxy)
                    let target_stream = if let Some(relay) = relay_proxy {
                        // Connect via relay proxy
                        match Self::connect_via_relay(&relay.url, &relay.auth, &host, port).await {
                            Ok(stream) => stream,
                            Err(e) => {
                                eprintln!("Failed to connect via relay proxy to {}:{}: {}", host, port, e);
                                return;
                            }
                        }
                    } else {
                        // Direct connection
                        match TcpStream::connect(format!("{}:{}", host, port)).await {
                            Ok(stream) => stream,
                            Err(e) => {
                                eprintln!("Failed to connect to target {}:{}: {}", host, port, e);
                                return;
                            }
                        }
                    };

                    println!("Successfully connected to target {}:{}", host, port);
                    
                    // Setup bidirectional tunnel
                    let (mut client_read, mut client_write) = tokio::io::split(upgraded);
                    let (mut target_read, mut target_write) = target_stream.into_split();

                    let client_to_target = async {
                        match tokio::io::copy(&mut client_read, &mut target_write).await {
                            Ok(bytes) => println!("Client -> Target: {} bytes transferred for {}:{}", bytes, host, port),
                            Err(e) => eprintln!("Error in client->target tunnel for {}:{}: {}", host, port, e),
                        }
                    };

                    let target_to_client = async {
                        match tokio::io::copy(&mut target_read, &mut client_write).await {
                            Ok(bytes) => println!("Target -> Client: {} bytes transferred for {}:{}", bytes, host, port),
                            Err(e) => eprintln!("Error in target->client tunnel for {}:{}: {}", host, port, e),
                        }
                    };

                    // Run both directions concurrently
                    tokio::join!(client_to_target, target_to_client);
                    println!("TCP tunnel closed for {}:{}", host, port);
                }
                Err(e) => {
                    eprintln!("Failed to upgrade connection for {}:{}: {}", host, port, e);
                }
            }
        });

        // Return 200 Connection Established immediately
        // The upgrade will happen after this response is sent
        Ok(Response::builder()
            .status(StatusCode::OK)
            .body(Body::empty())
            .unwrap())
    }

    // Find the first matching relay proxy for the given domain
    // Returns None if no relay proxy matches (direct connection)
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

    // Check if a host matches any NO_PROXY format patterns
    // NO_PROXY format supports:
    // - "example.com" - matches example.com and *.example.com
    // - ".example.com" - matches *.example.com only
    // - "*.example.com" - matches *.example.com only
    // - "subdomain.example.com" - matches subdomain.example.com exactly
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

    // Helper function to connect via relay proxy
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

    fn extract_target_uri(&self, req: &Request<Body>) -> Result<Uri, ProxyError> {
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
    fn verify_authentication(&self, req: &Request<Body>) -> Result<(), ProxyError> {
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

    fn reconstruct_request(&self, req: &mut Request<Body>, target_uri: &Uri) {
        // Update request URI to target
        *req.uri_mut() = target_uri.clone();

        // Remove hop-by-hop headers
        let headers = req.headers_mut();
        headers.remove(CONNECTION);
        headers.remove("Proxy-Connection");
        headers.remove("Keep-Alive");
        headers.remove("Proxy-Authenticate");
        headers.remove("Proxy-Authorization");
        headers.remove("TE");
        headers.remove("Trailers");
        headers.remove("Transfer-Encoding");
        headers.remove("Upgrade");
    }
}

/// Create TLS server configuration from certificate and private key files
fn create_tls_config(private_key_path: &str, cert_path: &str) -> Result<ServerConfig, ProxyError> {
    let mut private_key_file = BufReader::new(
        File::open(private_key_path)
            .map_err(|e| ProxyError::Config(format!("Failed to open private key file: {}", e)))?
    );

    let mut cert_file = BufReader::new(
        File::open(cert_path)
            .map_err(|e| ProxyError::Config(format!("Failed to open certificate file: {}", e)))?
    );

    // Load certificate chain
    let certs = rustls_pemfile::certs(&mut cert_file)
        .map_err(|e| ProxyError::Config(format!("Failed to read certificate: {}", e)))?
        .into_iter()
        .map(rustls::Certificate)
        .collect();

    // Load private key
    let keys: Vec<_> = rustls_pemfile::pkcs8_private_keys(&mut private_key_file)
        .map_err(|e| ProxyError::Config(format!("Failed to read private key: {}", e)))?
        .into_iter()
        .map(rustls::PrivateKey)
        .collect();

    if keys.is_empty() {
        return Err(ProxyError::Config("No valid private key found".to_string()));
    }

    let config = ServerConfig::builder()
        .with_safe_defaults()
        .with_no_client_auth()
        .with_single_cert(certs, keys.into_iter().next().unwrap())
        .map_err(|e| ProxyError::Config(format!("Failed to create TLS config: {}", e)))?;

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hyper::{Method, Uri};

    #[test]
    fn test_target_uri_extraction() {
        let proxy = ForwardProxy::new(10, 90, 300);

        // Test absolute URI
        let absolute_uri: Uri = "http://example.com/path".parse().unwrap();
        let mut req = Request::builder()
            .method(Method::GET)
            .uri(absolute_uri.clone())
            .body(Body::empty())
            .unwrap();

        let extracted = proxy.extract_target_uri(&req).unwrap();
        assert_eq!(extracted, absolute_uri);
    }
}