use crate::error::ProxyError;
use hyper::{Body, Client, Method, Request, Response, Server, StatusCode, Uri};
use hyper::client::HttpConnector;
use hyper_tls::HttpsConnector;
use hyper::service::{make_service_fn, service_fn};
use hyper::header::{HOST, CONNECTION};
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

pub struct ForwardProxy {
    client: Client<HttpsConnector<HttpConnector>>,
    timeout_duration: Duration,
    connection_pool_enabled: bool,
    pool_max_idle_per_host: usize,
}

impl ForwardProxy {
    pub fn new(timeout_secs: u64) -> Self {
        Self {
            client: Client::builder()
                .pool_max_idle_per_host(10)
                .build(HttpsConnector::new()),
            timeout_duration: Duration::from_secs(timeout_secs),
            connection_pool_enabled: true,
            pool_max_idle_per_host: 10,
        }
    }

    pub fn new_with_pool_config(
        timeout_secs: u64,
        connection_pool_enabled: bool,
        pool_max_idle_per_host: usize,
    ) -> Self {
        let client = if connection_pool_enabled {
            Client::builder()
                .pool_max_idle_per_host(pool_max_idle_per_host)
                .build(HttpsConnector::new())
        } else {
            Client::builder()
                .pool_max_idle_per_host(0)
                .build(HttpsConnector::new())
        };

        Self {
            client,
            timeout_duration: Duration::from_secs(timeout_secs),
            connection_pool_enabled,
            pool_max_idle_per_host,
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
        let timeout_duration = self.timeout_duration;
        let connection_pool_enabled = self.connection_pool_enabled;
        let pool_max_idle_per_host = self.pool_max_idle_per_host;

        let make_svc = make_service_fn(move |_conn| {
            let timeout_duration = timeout_duration;
            let connection_pool_enabled = connection_pool_enabled;
            let pool_max_idle_per_host = pool_max_idle_per_host;
            async move {
                Ok::<_, Infallible>(service_fn(move |req| {
                    let client = if connection_pool_enabled {
                        Client::builder()
                            .pool_max_idle_per_host(pool_max_idle_per_host)
                            .build(HttpsConnector::new())
                    } else {
                        Client::builder()
                            .pool_max_idle_per_host(0)
                            .build(HttpsConnector::new())
                    };
                    let proxy = ForwardProxy {
                        client,
                        timeout_duration,
                        connection_pool_enabled,
                        pool_max_idle_per_host,
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
        let timeout_duration = self.timeout_duration;
        let connection_pool_enabled = self.connection_pool_enabled;
        let pool_max_idle_per_host = self.pool_max_idle_per_host;
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

            let timeout_duration = timeout_duration;
            let connection_pool_enabled = connection_pool_enabled;
            let pool_max_idle_per_host = pool_max_idle_per_host;
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
                                        .build(HttpsConnector::new())
                                } else {
                                    Client::builder()
                                        .pool_max_idle_per_host(0)
                                        .build(HttpsConnector::new())
                                };
                                let proxy = ForwardProxy {
                                    client,
                                    timeout_duration,
                                    connection_pool_enabled,
                                    pool_max_idle_per_host,
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
                let error_response = Response::builder()
                    .status(StatusCode::BAD_GATEWAY)
                    .body(Body::from(format!("Proxy Error: {}", e)))
                    .unwrap();
                Ok(error_response)
            }
        }
    }

    async fn process_request(&self, mut req: Request<Body>) -> Result<Response<Body>, ProxyError> {
        // Handle CONNECT method for HTTPS
        if *req.method() == Method::CONNECT {
            return self.handle_connect(req).await;
        }

        // Extract target URL from request
        let target_uri = self.extract_target_uri(&req)?;

        // Reconstruct request for target server
        self.reconstruct_request(&mut req, &target_uri);

        // Send request with timeout
        let response = timeout(self.timeout_duration, self.client.request(req))
            .await
            .map_err(|_| ProxyError::Connection("Request timeout".to_string()))?
            .map_err(|e| ProxyError::Http(e.to_string()))?;

        Ok(response)
    }

    async fn handle_connect(&self, req: Request<Body>) -> Result<Response<Body>, ProxyError> {
        // Extract host and port from CONNECT request
        let authority = req.uri().authority()
            .ok_or_else(|| ProxyError::Config("Invalid CONNECT target".to_string()))?;

        let host = authority.host().to_string();
        let port = authority.port_u16().unwrap_or(443);

        println!("CONNECT request to {}:{}", host, port);

        // Spawn the upgrade and tunnel handling
        tokio::spawn(async move {
            match hyper::upgrade::on(req).await {
                Ok(upgraded) => {
                    println!("Successfully upgraded connection for {}:{}", host, port);
                    
                    // Connect to the target server
                    match TcpStream::connect(format!("{}:{}", host, port)).await {
                        Ok(target_stream) => {
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
                            eprintln!("Failed to connect to target {}:{}: {}", host, port, e);
                        }
                    }
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
        let proxy = ForwardProxy::new(30);

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