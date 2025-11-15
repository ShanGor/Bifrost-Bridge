use crate::config::{Config, ProxyMode};
use crate::error::ProxyError;
use crate::forward_proxy::ForwardProxy;
use crate::reverse_proxy::ReverseProxy;
use crate::static_files::StaticFileHandler;
use hyper::{Body, Response, Server, StatusCode};
use hyper::service::{make_service_fn, service_fn};
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

pub struct ProxyFactory;

impl ProxyFactory {
    pub fn create_proxy(config: Config) -> Result<Box<dyn Proxy + Send>, ProxyError> {
        match config.mode {
            ProxyMode::Forward => {
                // Support backward compatibility with timeout_secs
                let connect_timeout_secs = config.connect_timeout_secs
                    .or(config.timeout_secs)
                    .unwrap_or(10);
                let idle_timeout_secs = config.idle_timeout_secs
                    .unwrap_or(90);
                let max_connection_lifetime_secs = config.max_connection_lifetime_secs
                    .unwrap_or(300);
                let connection_pool_enabled = config.connection_pool_enabled.unwrap_or(true);
                let pool_max_idle_per_host = config.pool_max_idle_per_host.unwrap_or(10);
                let proxy = ForwardProxy::new_with_pool_config(
                    connect_timeout_secs,
                    idle_timeout_secs,
                    max_connection_lifetime_secs,
                    connection_pool_enabled,
                    pool_max_idle_per_host,
                );
                Ok(Box::new(ForwardProxyAdapter {
                    proxy,
                    addr: config.listen_addr,
                    private_key: config.private_key,
                    certificate: config.certificate,
                }))
            }
            ProxyMode::Reverse => {
                if config.static_files.is_some() && config.reverse_proxy_target.is_none() {
                    // Static files only mode
                    let static_config = config.static_files.unwrap();
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
                    let tls_config = create_tls_config(&private_key_path, &cert_path)?;
                    let tls_config = Arc::new(tls_config);
                    let acceptor = TlsAcceptor::from(tls_config.clone());

                    let tcp_listener = tokio::net::TcpListener::bind(&addr).await
                        .map_err(|e| ProxyError::Io(e))?;

                    println!("HTTPS static file server listening on: https://{}", addr);
                    println!("Certificate: {}", cert_path);
                    println!("Private key: {}", private_key_path);

                    loop {
                        let (tcp_stream, _) = tcp_listener.accept().await
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
                                                        .body(Body::from("Internal Server Error"))
                                                        .unwrap())
                                                }
                                            }
                                        }
                                    });

                                    if let Err(e) = hyper::server::conn::Http::new()
                                        .http1_keep_alive(true)
                                        .serve_connection(tls_stream, service)
                                        .await
                                    {
                                        eprintln!("Error serving connection: {}", e);
                                    }
                                }
                                Err(e) => {
                                    eprintln!("Error establishing TLS connection: {}", e);
                                }
                            }
                        });
                    }
                }
                _ => {
                    // HTTP mode
                    let make_svc = make_service_fn(move |_conn| {
                        let handler = handler.clone();
                        async {
                            Ok::<_, Infallible>(service_fn(move |req| {
                                let handler = handler.clone();
                                async move {
                                    match handler.handle_request(&req).await {
                                        Ok(response) => Ok::<_, Infallible>(response),
                                        Err(_) => {
                                            Ok::<_, Infallible>(Response::builder()
                                                .status(StatusCode::INTERNAL_SERVER_ERROR)
                                                .body(Body::from("Internal Server Error"))
                                                .unwrap())
                                        }
                                    }
                                }
                            }))
                        }
                    });

                    let server = Server::bind(&addr).serve(make_svc);
                    println!("HTTP static file server listening on: http://{}", addr);

                    if let Err(e) = server.await {
                        eprintln!("Server error: {}", e);
                        return Err(ProxyError::Hyper(e.to_string()));
                    }
                }
            }
            Ok(())
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