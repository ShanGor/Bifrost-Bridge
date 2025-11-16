use crate::config::{Config, ProxyMode, RelayProxyConfig};
use crate::error::ProxyError;
use crate::forward_proxy::ForwardProxy;
use crate::reverse_proxy::ReverseProxy;
use crate::static_files::StaticFileHandler;
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
use std::fs::File;
use std::io::BufReader;
use std::sync::Arc;
use rustls::ServerConfig;
use tokio_rustls::TlsAcceptor;

pub trait Proxy {
    fn run(self: Box<Self>) -> Pin<Box<dyn Future<Output = Result<(), ProxyError>> + Send>>;
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
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| ProxyError::Config(format!("Failed to read certificate: {}", e)))?;

    if certs.is_empty() {
        return Err(ProxyError::Config("No valid certificate found".to_string()));
    }

    // Try to load private key in different formats
    let private_key = rustls_pemfile::private_key(&mut private_key_file)
        .map_err(|e| ProxyError::Config(format!("Failed to read private key: {}", e)))?
        .ok_or_else(|| ProxyError::Config("No valid private key found".to_string()))?;

    let config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, private_key)
        .map_err(|e| ProxyError::Config(format!("Failed to create TLS config: {}", e)))?;

    Ok(config)
}

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
                } else {
                    // Reverse proxy mode (with or without static files)
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
                    let proxy = ReverseProxy::new(target_url, connect_timeout_secs, idle_timeout_secs, max_connection_lifetime_secs)?;
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

                    let tls_config = create_tls_config(&private_key_path, &cert_path)?;
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
                                                    Ok::<_, Infallible>(Response::builder()
                                                        .status(StatusCode::INTERNAL_SERVER_ERROR)
                                                        .body(Full::new(Bytes::from("Internal Server Error")))
                                                        .unwrap())
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
                                                    Ok::<_, Infallible>(Response::builder()
                                                        .status(StatusCode::INTERNAL_SERVER_ERROR)
                                                        .body(Full::new(Bytes::from("Internal Server Error")))
                                                        .unwrap())
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