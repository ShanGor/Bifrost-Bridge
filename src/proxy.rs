use crate::config::{Config, ProxyMode, RelayProxyConfig};
use crate::error::{ProxyError, ErrorContext, ContextualError};
use crate::error_recovery::ErrorRecoveryManager;
use crate::forward_proxy::ForwardProxy;
use crate::reverse_proxy::ReverseProxy;
use crate::static_files::StaticFileHandler;
use crate::common::{ResponseBuilder, TlsConfig, FileBody, ProxyType, IsolatedWorker};
use log::{info, debug, warn, error};
use hyper::{Response, StatusCode};
use hyper::body::Bytes;
use hyper::service::service_fn;
use hyper::server::conn::http1::Builder as ServerBuilder;
use hyper_util::rt::TokioIo;
use http_body_util::Full;
use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio_rustls::TlsAcceptor;

pub trait Proxy {
    fn run(self: Box<Self>) -> Pin<Box<dyn Future<Output = Result<(), ProxyError>> + Send>>;
}

// TLS configuration is now handled by TlsConfig::create_config in common.rs

pub struct ProxyFactory;

impl ProxyFactory {
    pub fn create_proxy(config: Config) -> Result<Box<dyn Proxy + Send>, ProxyError> {
        info!("Creating proxy instance for mode: {:?}", config.mode);
        debug!("Proxy configuration - listen_addr: {}, max_connections: {:?}",
               config.listen_addr, config.max_connections);

        match config.mode {
            ProxyMode::Forward => {
                info!("Initializing Forward Proxy mode");
                debug!("Forward proxy configuration - connection_pool: {:?}",
                       config.connection_pool_enabled);
                // Support backward compatibility with timeout_secs
                let connect_timeout_secs = config.connect_timeout_secs
                    .or(config.timeout_secs)
                    .unwrap_or(10);
                let idle_timeout_secs = config.idle_timeout_secs
                    .unwrap_or(90);
                let max_connection_lifetime_secs = config.max_connection_lifetime_secs
                    .unwrap_or(300);
                let connection_pool_enabled = config.connection_pool_enabled.unwrap_or(true);
                
                // Support both new relay_proxies and legacy relay_proxy fields
                let relay_configs = if let Some(relay_proxies) = config.relay_proxies {
                    // Use new multi-relay configuration
                    relay_proxies
                } else if config.relay_proxy_url.is_some() {
                    // Convert legacy single relay proxy to new format
                    vec![RelayProxyConfig {
                        relay_proxy_url: config.relay_proxy_url.unwrap(),
                        relay_proxy_username: config.relay_proxy_username,
                        relay_proxy_password: config.relay_proxy_password,
                        relay_proxy_domains: config.relay_proxy_domain_suffixes.unwrap_or_default(),
                    }]
                } else {
                    Vec::new()
                };
                
                let proxy = ForwardProxy::new_with_relay_proxies(
                    connect_timeout_secs,
                    idle_timeout_secs,
                    max_connection_lifetime_secs,
                    connection_pool_enabled,
                    relay_configs,
                    config.proxy_username,
                    config.proxy_password,
                );
                
                Ok(Box::new(ForwardProxyAdapter {
                    proxy,
                    addr: config.listen_addr,
                    private_key: config.private_key,
                    certificate: config.certificate,
                }))
            }
            ProxyMode::Reverse => {
                info!("Initializing Reverse Proxy mode");

                if config.static_files.is_some() && config.reverse_proxy_target.is_none() {
                    info!("Static files only mode (no reverse proxy target)");
                    let static_config = config.static_files.unwrap();
                    debug!("Static files configuration - mounts: {}", static_config.mounts.len());
                    let handler = StaticFileHandler::new(static_config)?;
                    Ok(Box::new(StaticFileProxyAdapter {
                        handler,
                        addr: config.listen_addr,
                        private_key: config.private_key,
                        certificate: config.certificate,
                    }))
                } else if config.static_files.is_some() && config.reverse_proxy_target.is_some() {
                    // Combined mode: both reverse proxy and static files
                    info!("Combined reverse proxy + static files mode");
                    let static_config = config.static_files.unwrap();
                    debug!("Static files configuration - mounts: {}", static_config.mounts.len());
                    let handler = StaticFileHandler::new(static_config)?;

                    let target_url = config.reverse_proxy_target.unwrap();
                    info!("Reverse proxy target: {}", target_url);
                    // Support backward compatibility with timeout_secs
                    let connect_timeout_secs = config.connect_timeout_secs
                        .or(config.timeout_secs)
                        .unwrap_or(10);
                    let idle_timeout_secs = config.idle_timeout_secs
                        .unwrap_or(90);
                    let max_connection_lifetime_secs = config.max_connection_lifetime_secs
                        .unwrap_or(300);
                    let proxy = ReverseProxy::new_with_config(
                        target_url,
                        connect_timeout_secs,
                        idle_timeout_secs,
                        max_connection_lifetime_secs,
                        config.reverse_proxy_config.clone(),
                    )?;

                    Ok(Box::new(CombinedProxyAdapter {
                        reverse_proxy: proxy,
                        static_handler: handler,
                        addr: config.listen_addr,
                        private_key: config.private_key,
                        certificate: config.certificate,
                    }))
                } else {
                    // Reverse proxy only mode
                    let target_url = config.reverse_proxy_target
                        .ok_or_else(|| ProxyError::Config("Reverse proxy target URL is required for reverse proxy mode".to_string()))?;
                    info!("Reverse proxy target: {}", target_url);
                    // Support backward compatibility with timeout_secs
                    let connect_timeout_secs = config.connect_timeout_secs
                        .or(config.timeout_secs)
                        .unwrap_or(10);
                    let idle_timeout_secs = config.idle_timeout_secs
                        .unwrap_or(90);
                    let max_connection_lifetime_secs = config.max_connection_lifetime_secs
                        .unwrap_or(300);
                    let proxy = ReverseProxy::new_with_config(
                        target_url,
                        connect_timeout_secs,
                        idle_timeout_secs,
                        max_connection_lifetime_secs,
                        config.reverse_proxy_config.clone(),
                    )?;
                    Ok(Box::new(ReverseProxyAdapter {
                        proxy,
                        addr: config.listen_addr,
                        private_key: config.private_key,
                        certificate: config.certificate,
                    }))
                }
            }
        }
    }
}

struct ForwardProxyAdapter {
    proxy: ForwardProxy,
    addr: std::net::SocketAddr,
    private_key: Option<String>,
    certificate: Option<String>,
}

impl Proxy for ForwardProxyAdapter {
    fn run(self: Box<Self>) -> Pin<Box<dyn Future<Output = Result<(), ProxyError>> + Send>> {
        Box::pin(async move {
            let addr = self.addr;
            let private_key = self.private_key;
            let certificate = self.certificate;

            self.proxy.run_with_config(addr, private_key, certificate).await
        })
    }
}

struct ReverseProxyAdapter {
    proxy: ReverseProxy,
    addr: std::net::SocketAddr,
    #[allow(dead_code)]
    private_key: Option<String>,
    #[allow(dead_code)]
    certificate: Option<String>,
}

impl Proxy for ReverseProxyAdapter {
    fn run(self: Box<Self>) -> Pin<Box<dyn Future<Output = Result<(), ProxyError>> + Send>> {
        Box::pin(async move {
            let addr = self.addr;
            self.proxy.run(addr).await
        })
    }
}

struct StaticFileProxyAdapter {
    handler: StaticFileHandler,
    addr: SocketAddr,
    private_key: Option<String>,
    certificate: Option<String>,
}

impl Proxy for StaticFileProxyAdapter {
    fn run(self: Box<Self>) -> Pin<Box<dyn Future<Output = Result<(), ProxyError>> + Send>> {
        Box::pin(async move {
            let handler = Arc::new(self.handler);
            let addr = self.addr;
            let private_key = self.private_key;
            let certificate = self.certificate;

            match (private_key, certificate) {
                (Some(private_key_path), Some(cert_path)) => {
                    // HTTPS mode
                    info!("Enabling HTTPS/TLS mode");
                    debug!("Loading TLS certificate from: {}", cert_path);
                    debug!("Loading TLS private key from: {}", private_key_path);

                    let tls_config = TlsConfig::create_config(&private_key_path, &cert_path)?;
                    let tls_config = Arc::new(tls_config);
                    let acceptor = TlsAcceptor::from(tls_config.clone());

                    info!("Binding TCP listener to: {}", addr);
                    let tcp_listener = tokio::net::TcpListener::bind(&addr).await
                        .map_err(|e| ProxyError::Io(e))?;

                    info!("HTTPS static file server listening on: https://{}", addr);
                    debug!("TLS certificate file: {}", cert_path);
                    debug!("TLS private key file: {}", private_key_path);

                    loop {
                        let (tcp_stream, remote_addr) = tcp_listener.accept().await
                            .map_err(|e| ProxyError::Io(e))?;
                        let acceptor = acceptor.clone();
                        let handler_ref = handler.clone();

                        tokio::spawn(async move {
                            match acceptor.accept(tcp_stream).await {
                                Ok(tls_stream) => {
                                    let service = service_fn(move |req| {
                                        let handler = handler_ref.clone();
                                        async move {
                                            match handler.handle_request(&req).await {
                                                Ok(response) => Ok::<_, Infallible>(response),
                                                Err(_) => {
                                                    Ok::<_, Infallible>(ResponseBuilder::internal_server_error_file_body())
                                                }
                                            }
                                        }
                                    });

                                    if let Err(e) = ServerBuilder::new()
                                        .keep_alive(true)
                                        .serve_connection(TokioIo::new(tls_stream), service)
                                        .await
                                    {
                                        error!("Error serving TLS connection: {}", e);
                                    }
                                }
                                Err(e) => {
                                    warn!("Error establishing TLS connection from {}: {}",
                                          remote_addr, e);
                                }
                            }
                        });
                    }
                }
                _ => {
                    // HTTP mode
                    info!("Running in HTTP mode (no TLS)");
                    info!("Binding HTTP listener to: {}", addr);
                    let listener = tokio::net::TcpListener::bind(addr).await
                        .map_err(|e| ProxyError::Hyper(e.to_string()))?;
                    info!("HTTP static file server listening on: http://{}", addr);

                    loop {
                        let (stream, _) = listener.accept().await
                            .map_err(|e| ProxyError::Hyper(e.to_string()))?;

                        let handler = handler.clone();
                        tokio::spawn(async move {
                            let io = TokioIo::new(stream);

                            if let Err(err) = ServerBuilder::new()
                                .serve_connection(
                                    io,
                                    service_fn(move |req| {
                                        let handler = handler.clone();
                                        async move {
                                            match handler.handle_request(&req).await {
                                                Ok(response) => Ok::<_, Infallible>(response),
                                                Err(_) => {
                                                    Ok::<_, Infallible>(ResponseBuilder::internal_server_error_file_body())
                                                }
                                            }
                                        }
                                    })
                                )
                                .await
                            {
                                error!("Error serving HTTP connection: {}", err);
                            }
                        });
                    }
                }
            }
        })
    }
}

struct CombinedProxyAdapter {
    reverse_proxy: ReverseProxy,
    static_handler: StaticFileHandler,
    addr: std::net::SocketAddr,
    #[allow(dead_code)]
    private_key: Option<String>,
    #[allow(dead_code)]
    certificate: Option<String>,
}

impl Proxy for CombinedProxyAdapter {
    fn run(self: Box<Self>) -> Pin<Box<dyn Future<Output = Result<(), ProxyError>> + Send>> {
        Box::pin(async move {
            let addr = self.addr;
            let private_key = self.private_key;
            let certificate = self.certificate;
            let reverse_proxy = Arc::new(self.reverse_proxy);
            let static_handler = Arc::new(self.static_handler);

            match (private_key, certificate) {
                (Some(private_key_path), Some(cert_path)) => {
                    // HTTPS mode
                    info!("Enabling HTTPS/TLS mode for combined proxy");
                    debug!("Loading TLS certificate from: {}", cert_path);
                    debug!("Loading TLS private key from: {}", private_key_path);

                    let tls_config = TlsConfig::create_config(&private_key_path, &cert_path)?;
                    let tls_config = Arc::new(tls_config);
                    let acceptor = TlsAcceptor::from(tls_config.clone());

                    info!("Binding TCP listener to: {}", addr);
                    let tcp_listener = tokio::net::TcpListener::bind(&addr).await
                        .map_err(|e| ProxyError::Io(e))?;

                    info!("HTTPS combined proxy server listening on: https://{}", addr);
                    debug!("TLS certificate file: {}", cert_path);
                    debug!("TLS private key file: {}", private_key_path);

                    loop {
                        let (tcp_stream, remote_addr) = tcp_listener.accept().await
                            .map_err(|e| ProxyError::Io(e))?;
                        let acceptor = acceptor.clone();
                        let reverse_proxy_ref = reverse_proxy.clone();
                        let static_handler_ref = static_handler.clone();

                        tokio::spawn(async move {
                            match acceptor.accept(tcp_stream).await {
                                Ok(tls_stream) => {
                                    let service = service_fn(move |req| {
                                        let reverse_proxy = reverse_proxy_ref.clone();
                                        let static_handler = static_handler_ref.clone();
                                        async move {
                                            // Route request to appropriate handler
                                            let request_path = req.uri().path();

                                            // Check if request matches any static file mount
                                            if let Some((_mount_info, _relative_path)) = static_handler.find_mount_for_path(request_path) {
                                                // Serve static file
                                                match static_handler.handle_request(&req).await {
                                                    Ok(response) => Ok::<_, Infallible>(response),
                                                    Err(ProxyError::NotFound(_)) => {
                                                        // Fall back to reverse proxy if static file not found
                                                        let context = crate::reverse_proxy::RequestContext {
                                                            client_ip: Some(remote_addr.ip().to_string()),
                                                        };
                                                        match reverse_proxy.handle_request_with_context(req, context).await {
                                                            Ok(response) => {
                                                                // Convert Full<Bytes> to FileBody
                                                                let (parts, body) = response.into_parts();
                                                                let response_with_file_body = Response::from_parts(parts, FileBody::InMemory(body));
                                                                Ok::<_, Infallible>(response_with_file_body)
                                                            }
                                                            Err(_) => {
                                                                Ok::<_, Infallible>(Response::builder()
                                                                    .status(StatusCode::BAD_GATEWAY)
                                                                    .body(FileBody::InMemory(Full::new(Bytes::from("Proxy Error"))))
                                                                    .unwrap())
                                                            }
                                                        }
                                                    },
                                                    Err(_) => {
                                                        Ok::<_, Infallible>(Response::builder()
                                                            .status(StatusCode::INTERNAL_SERVER_ERROR)
                                                            .body(FileBody::InMemory(Full::new(Bytes::from("Internal Server Error"))))
                                                            .unwrap())
                                                    }
                                                }
                                            } else {
                                                // Forward to reverse proxy
                                                let context = crate::reverse_proxy::RequestContext {
                                                    client_ip: Some(remote_addr.ip().to_string()),
                                                };
                                                match reverse_proxy.handle_request_with_context(req, context).await {
                                                    Ok(response) => {
                                                        // Convert Full<Bytes> to FileBody
                                                        let (parts, body) = response.into_parts();
                                                        let response_with_file_body = Response::from_parts(parts, FileBody::InMemory(body));
                                                        Ok::<_, Infallible>(response_with_file_body)
                                                    }
                                                    Err(_) => {
                                                        Ok::<_, Infallible>(Response::builder()
                                                            .status(StatusCode::BAD_GATEWAY)
                                                            .body(FileBody::InMemory(Full::new(Bytes::from("Proxy Error"))))
                                                            .unwrap())
                                                    }
                                                }
                                            }

                                        }
                                    });

                                    if let Err(e) = ServerBuilder::new()
                                        .keep_alive(true)
                                        .serve_connection(TokioIo::new(tls_stream), service)
                                        .await
                                    {
                                        error!("Error serving TLS connection: {}", e);
                                    }
                                }
                                Err(e) => {
                                    warn!("Error establishing TLS connection from {}: {}",
                                          remote_addr, e);
                                }
                            }
                        });
                    }
                }
                _ => {
                    // HTTP mode
                    info!("Running in HTTP mode for combined proxy");
                    info!("Binding HTTP listener to: {}", addr);
                    let listener = tokio::net::TcpListener::bind(addr).await
                        .map_err(|e| ProxyError::Hyper(e.to_string()))?;
                    info!("HTTP combined proxy server listening on: http://{}", addr);

                    loop {
                        let (stream, remote_addr) = listener.accept().await
                            .map_err(|e| ProxyError::Hyper(e.to_string()))?;

                        let reverse_proxy = reverse_proxy.clone();
                        let static_handler = static_handler.clone();
                        tokio::spawn(async move {
                            let io = TokioIo::new(stream);

                            if let Err(err) = ServerBuilder::new()
                                .serve_connection(
                                    io,
                                    service_fn(move |req| {
                                        let reverse_proxy = reverse_proxy.clone();
                                        let static_handler = static_handler.clone();
                                        async move {
                                            // Route request to appropriate handler
                                            let request_path = req.uri().path();

                                            // Check if request matches any static file mount
                                            if let Some((_mount_info, _relative_path)) = static_handler.find_mount_for_path(request_path) {
                                                // Serve static file
                                                match static_handler.handle_request(&req).await {
                                                    Ok(response) => Ok::<_, Infallible>(response),
                                                    Err(ProxyError::NotFound(_)) => {
                                                        // Fall back to reverse proxy if static file not found
                                                        let context = crate::reverse_proxy::RequestContext {
                                                            client_ip: Some(remote_addr.ip().to_string()),
                                                        };
                                                        match reverse_proxy.handle_request_with_context(req, context).await {
                                                            Ok(response) => {
                                                                // Convert Full<Bytes> to FileBody
                                                                let (parts, body) = response.into_parts();
                                                                let response_with_file_body = Response::from_parts(parts, FileBody::InMemory(body));
                                                                Ok::<_, Infallible>(response_with_file_body)
                                                            }
                                                            Err(_) => {
                                                                Ok::<_, Infallible>(Response::builder()
                                                                    .status(StatusCode::BAD_GATEWAY)
                                                                    .body(FileBody::InMemory(Full::new(Bytes::from("Proxy Error"))))
                                                                    .unwrap())
                                                            }
                                                        }
                                                    },
                                                    Err(_) => {
                                                        Ok::<_, Infallible>(Response::builder()
                                                            .status(StatusCode::INTERNAL_SERVER_ERROR)
                                                            .body(FileBody::InMemory(Full::new(Bytes::from("Internal Server Error"))))
                                                            .unwrap())
                                                    }
                                                }
                                            } else {
                                                // Forward to reverse proxy
                                                let context = crate::reverse_proxy::RequestContext {
                                                    client_ip: Some(remote_addr.ip().to_string()),
                                                };
                                                match reverse_proxy.handle_request_with_context(req, context).await {
                                                    Ok(response) => {
                                                        // Convert Full<Bytes> to FileBody
                                                        let (parts, body) = response.into_parts();
                                                        let response_with_file_body = Response::from_parts(parts, FileBody::InMemory(body));
                                                        Ok::<_, Infallible>(response_with_file_body)
                                                    }
                                                    Err(_) => {
                                                        Ok::<_, Infallible>(Response::builder()
                                                            .status(StatusCode::BAD_GATEWAY)
                                                            .body(FileBody::InMemory(Full::new(Bytes::from("Proxy Error"))))
                                                            .unwrap())
                                                    }
                                                }
                                            }

                                        }
                                    })
                                )
                                .await
                            {
                                error!("Error serving HTTP connection: {}", err);
                            }
                        });
                    }
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ProxyMode;

    #[test]
    fn test_proxy_factory_forward() {
        let mut config = Config::default();
        config.mode = ProxyMode::Forward;
        config.listen_addr = "127.0.0.1:8080".parse().unwrap();

        let proxy = ProxyFactory::create_proxy(config);
        assert!(proxy.is_ok());
    }

    #[test]
    fn test_proxy_factory_reverse() {
        let mut config = Config::default();
        config.mode = ProxyMode::Reverse;
        config.listen_addr = "127.0.0.1:8080".parse().unwrap();
        config.reverse_proxy_target = Some("http://backend.example.com".to_string());

        let proxy = ProxyFactory::create_proxy(config);
        assert!(proxy.is_ok());
    }

    #[test]
    fn test_proxy_factory_reverse_no_target() {
        let mut config = Config::default();
        config.mode = ProxyMode::Reverse;
        config.listen_addr = "127.0.0.1:8080".parse().unwrap();
        config.reverse_proxy_target = None;

        let proxy = ProxyFactory::create_proxy(config);
        assert!(proxy.is_err());
    }
}

/// Isolated proxy adapter that uses dedicated workers with separate resources
pub struct IsolatedProxyAdapter {
    handler: Arc<dyn Proxy + Send + Sync>,
    worker: Arc<IsolatedWorker>,
    addr: SocketAddr,
    private_key: Option<String>,
    certificate: Option<String>,
    error_recovery: Arc<ErrorRecoveryManager>,
}

impl IsolatedProxyAdapter {
    pub fn new(
        handler: Arc<dyn Proxy + Send + Sync>,
        addr: String,
        private_key: Option<String>,
        certificate: Option<String>,
        worker: Arc<IsolatedWorker>,
    ) -> Result<Self, ProxyError> {
        let addr = addr.parse()
            .map_err(|e| ProxyError::Config(format!("Invalid bind address: {}", e)))?;

        let error_recovery = Arc::new(ErrorRecoveryManager::default());

        Ok(Self {
            handler,
            worker,
            addr,
            private_key,
            certificate,
            error_recovery,
        })
    }

    /// Handle an error with context and recovery
    async fn handle_error_with_recovery(&self, error: ProxyError, operation: &str) -> bool {
        let worker_id = format!("{:?}", self.worker.proxy_type);
        let context = ErrorContext::new("IsolatedProxyAdapter", operation)
            .with_worker_id(&worker_id)
            .with_proxy_type(&format!("{:?}", self.worker.proxy_type));

        let contextual_error = ContextualError::new(error, context);
        let should_retry = contextual_error.should_retry();

        // Handle error through recovery manager
        if let Err(recovery_error) = self.error_recovery.handle_error(contextual_error).await {
            error!("Recovery failed for operation {}: {}", operation, recovery_error);
            return false;
        }

        // Check if we should retry
        should_retry
    }

    /// Execute an operation with error recovery
    async fn execute_with_recovery<F, T, Fut>(&self, operation: &str, f: F) -> Option<T>
    where
        F: Fn() -> Fut + Send + Sync,
        Fut: std::future::Future<Output = Result<T, ProxyError>>,
    {
        let mut attempts = 0;
        let max_attempts = 3;

        loop {
            attempts += 1;

            let future = f();
            match future.await {
                Ok(result) => {
                    // Success - update worker health
                    let worker_id = format!("{:?}", self.worker.proxy_type);
                    self.error_recovery.update_worker_health(&worker_id, true).await;
                    return Some(result);
                }
                Err(error) => {
                    warn!("Error in operation {} (attempt {}): {}", operation, attempts, error);

                    if !self.handle_error_with_recovery(error, operation).await {
                        error!("Error recovery failed, not retrying operation: {}", operation);
                        return None;
                    }

                    if attempts >= max_attempts {
                        error!("Max retry attempts ({}) reached for operation: {}", max_attempts, operation);
                        return None;
                    }

                    // Wait before retry with exponential backoff
                    let delay = std::time::Duration::from_millis(100 * 2_u64.pow(attempts - 1));
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }

    /// Get the isolated worker for this adapter
    pub fn get_worker(&self) -> Arc<IsolatedWorker> {
        self.worker.clone()
    }

    /// Get the proxy type for this adapter
    pub fn get_proxy_type(&self) -> ProxyType {
        self.worker.proxy_type.clone()
    }

    /// Run the server with worker resource management and error recovery
    pub async fn run(&self) -> Result<(), ProxyError> {
        info!("Starting {} server on {}", self.get_proxy_type(), self.addr);
        info!("Worker configuration - max_connections: {}, max_memory: {}MB",
               self.worker.resource_limits.max_connections,
               self.worker.resource_limits.max_memory_mb);

        // Register worker with error recovery manager
        self.error_recovery.register_worker(&self.worker).await;

        let handler = Arc::clone(&self.handler);
        let worker = self.worker.clone();
        let addr = self.addr;
        let private_key = self.private_key.clone();
        let certificate = self.certificate.clone();

        // Start periodic health checks
        let error_recovery_clone = self.error_recovery.clone();
        let worker_id = format!("{:?}", self.worker.proxy_type);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
            loop {
                interval.tick().await;
                error_recovery_clone.update_worker_health(&worker_id, true).await;
            }
        });

        match (private_key, certificate) {
            (Some(private_key_path), Some(cert_path)) => {
                self.run_https_server(worker, handler, addr, &private_key_path, &cert_path).await
            }
            _ => {
                self.run_http_server(worker, handler, addr).await
            }
        }
    }

    async fn run_https_server(
        &self,
        worker: Arc<IsolatedWorker>,
        _handler: Arc<dyn Proxy + Send + Sync>,
        addr: SocketAddr,
        private_key_path: &str,
        cert_path: &str,
    ) -> Result<(), ProxyError>
    {
        info!("Enabling HTTPS/TLS mode for {}", worker.get_proxy_type());

        // Create TLS config with error recovery
        let tls_config = self.execute_with_recovery("tls_config_creation", || async {
            TlsConfig::create_config(private_key_path, cert_path)
                .map(|config| Arc::new(config))
        }).await.ok_or_else(|| ProxyError::WorkerCreationFailed("TLS config creation failed after retries".to_string()))?;

        let acceptor = TlsAcceptor::from(tls_config.clone());

        // Bind TCP listener with error recovery
        let tcp_listener = self.execute_with_recovery("tcp_listener_bind", || async {
            tokio::net::TcpListener::bind(&addr).await
                .map_err(|e| ProxyError::Io(e))
        }).await.ok_or_else(|| ProxyError::WorkerCreationFailed("TCP listener bind failed after retries".to_string()))?;

        info!("{} HTTPS server listening on: https://{}", worker.get_proxy_type(), addr);

        loop {
            if !worker.can_accept_connection() {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                continue;
            }

            let (tcp_stream, remote_addr) = tcp_listener.accept().await
                .map_err(|e| ProxyError::Io(e))?;

            let worker_ref = worker.clone();
            let acceptor_ref = acceptor.clone();

            tokio::spawn(async move {
                if !worker_ref.can_accept_connection() {
                    return;
                }

                worker_ref.connection_pool.increment_connections();
                worker_ref.metrics.increment_connections();

                let request_timer = crate::common::RequestTimer::with_metrics(worker_ref.metrics.clone());

                match acceptor_ref.accept(tcp_stream).await {
                    Ok(_tls_stream) => {
                        debug!("TLS connection established from {} to {}", remote_addr, worker_ref.get_proxy_type());
                        request_timer.finish();
                    }
                    Err(e) => {
                        error!("TLS handshake failed from {} to {}: {}", remote_addr, worker_ref.get_proxy_type(), e);
                        worker_ref.metrics.increment_connection_errors();
                    }
                }

                worker_ref.connection_pool.decrement_connections();
            });
        }
    }

    async fn run_http_server(
        &self,
        worker: Arc<IsolatedWorker>,
        _handler: Arc<dyn Proxy + Send + Sync>,
        addr: SocketAddr,
    ) -> Result<(), ProxyError>
    {
        info!("Binding TCP listener to: {}", addr);
        let tcp_listener = tokio::net::TcpListener::bind(&addr).await
            .map_err(|e| ProxyError::Io(e))?;

        info!("{} HTTP server listening on: http://{}", worker.get_proxy_type(), addr);

        loop {
            if !worker.can_accept_connection() {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                continue;
            }

            let (tcp_stream, remote_addr) = tcp_listener.accept().await
                .map_err(|e| ProxyError::Io(e))?;

            let worker_ref = worker.clone();

            tokio::spawn(async move {
                if !worker_ref.can_accept_connection() {
                    return;
                }

                worker_ref.connection_pool.increment_connections();
                worker_ref.metrics.increment_connections();

                let request_timer = crate::common::RequestTimer::with_metrics(worker_ref.metrics.clone());

                // For now, we just accept the connection and close it
                // In a real implementation, this would handle HTTP requests
                drop(tcp_stream);
                debug!("HTTP connection established from {} to {}", remote_addr, worker_ref.get_proxy_type());
                request_timer.finish();

                worker_ref.connection_pool.decrement_connections();
            });
        }
    }
}

// IsolatedProxyAdapter has its own implementation, focusing on worker separation
// The SharedServer trait is used by other adapters, but IsolatedProxyAdapter
// has specialized worker management that doesn't fit the shared pattern