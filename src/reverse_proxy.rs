use crate::error::ProxyError;
use hyper::{Request, Response, StatusCode, Uri};
use hyper::body::Incoming;
use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use http::response::Parts;
use hyper::client::conn::http1::Builder as ClientBuilder;
use hyper::server::conn::http1::Builder as ServerBuilder;
use hyper::header::HOST;
use hyper::header::HeaderName;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;

// Custom header names for X-Forwarded-* headers
static X_FORWARDED_FOR: HeaderName = HeaderName::from_static("x-forwarded-for");
static X_FORWARDED_PROTO: HeaderName = HeaderName::from_static("x-forwarded-proto");
static X_FORWARDED_HOST: HeaderName = HeaderName::from_static("x-forwarded-host");
use std::convert::Infallible;
use std::net::SocketAddr;
use tokio::time::Duration;
use url::Url;

/// Wrapper to store request data including client IP
#[derive(Clone, Debug)]
struct RequestContext {
    client_ip: Option<String>,
}

pub struct ReverseProxy {
    target_url: Url,
    connect_timeout: Duration,
    idle_timeout: Duration,
    max_connection_lifetime: Duration,
    preserve_host: bool,
}

impl ReverseProxy {
    pub fn new(target_url: String, connect_timeout_secs: u64, idle_timeout_secs: u64, max_connection_lifetime_secs: u64) -> Result<Self, ProxyError> {
        let url = Url::parse(&target_url)
            .map_err(|e| ProxyError::Url(e))?;

        Ok(Self {
            target_url: url,
            connect_timeout: Duration::from_secs(connect_timeout_secs),
            idle_timeout: Duration::from_secs(idle_timeout_secs),
            max_connection_lifetime: Duration::from_secs(max_connection_lifetime_secs),
            preserve_host: true,
        })
    }

    pub fn with_preserve_host(mut self, preserve_host: bool) -> Self {
        self.preserve_host = preserve_host;
        self
    }

    pub async fn run(self, addr: SocketAddr) -> Result<(), ProxyError> {
        let listener = tokio::net::TcpListener::bind(addr).await
            .map_err(|e| ProxyError::Hyper(e.to_string()))?;

        println!("Reverse proxy listening on: {} -> {}", addr, self.target_url);

        loop {
            let (stream, remote_addr) = listener.accept().await
                .map_err(|e| ProxyError::Hyper(e.to_string()))?;

            let target_url = self.target_url.clone();
            let connect_timeout = self.connect_timeout;
            let idle_timeout = self.idle_timeout;
            let max_connection_lifetime = self.max_connection_lifetime;
            let preserve_host = self.preserve_host;

            tokio::spawn(async move {
                let io = TokioIo::new(stream);

                if let Err(err) = ServerBuilder::new()
                    .serve_connection(
                        io,
                        service_fn(move |req| {
                            let target_url = target_url.clone();
                            let connect_timeout = connect_timeout;
                            let idle_timeout = idle_timeout;
                            let max_connection_lifetime = max_connection_lifetime;
                            let preserve_host = preserve_host;
                            let client_ip = Some(remote_addr.ip().to_string());

                            let context = RequestContext {
                                client_ip: client_ip.clone(),
                            };

                            async move {
                                let proxy = ReverseProxy {
                                    target_url: target_url.clone(),
                                    connect_timeout,
                                    idle_timeout,
                                    max_connection_lifetime,
                                    preserve_host,
                                };
                                proxy.handle_request_with_context(req, context).await
                            }
                        })
                    )
                    .await
                {
                    eprintln!("Error serving connection: {}", err);
                }
            });
        }
    }

  
    async fn handle_request_with_context(&self, req: Request<Incoming>, context: RequestContext) -> Result<Response<Full<Bytes>>, Infallible> {
        match self.process_request_with_context(req, context).await {
            Ok(response) => Ok(response),
            Err(e) => {
                eprintln!("Proxy error: {}", e);
                let error_response = Response::builder()
                    .status(StatusCode::BAD_GATEWAY)
                    .body(Full::new(Bytes::from(format!("Proxy Error: {}", e))))
                    .unwrap();
                Ok(error_response)
            }
        }
    }

  
    async fn process_request_with_context(&self, mut req: Request<Incoming>, context: RequestContext) -> Result<Response<Full<Bytes>>, ProxyError> {
        // Construct target URL
        let target_uri = self.build_target_uri(&req)?;

        // Modify request for reverse proxy with context
        self.modify_request_with_context(&mut req, &target_uri, context);

        // Extract host and port from target URI
        let authority = target_uri.authority()
            .ok_or_else(|| ProxyError::Config("Invalid target URI".to_string()))?;

        let host = authority.host();
        let port = authority.port_u16().unwrap_or(80);

        // Connect to target server
        let stream = tokio::time::timeout(
            self.connect_timeout,
            tokio::net::TcpStream::connect((host, port))
        )
        .await
        .map_err(|_| ProxyError::Connection("Request timeout".to_string()))?
        .map_err(|e| ProxyError::Connection(e.to_string()))?;

        let io = TokioIo::new(stream);

        // Send request using HTTP/1.1 client
        let (mut sender, conn) = ClientBuilder::new()
            .handshake(io)
            .await
            .map_err(|e| ProxyError::Http(e.to_string()))?;

        // Spawn the connection task
        tokio::spawn(async move {
            if let Err(err) = conn.await {
                eprintln!("Connection error: {}", err);
            }
        });

        // Send request
        let response = tokio::time::timeout(
            self.connect_timeout,
            sender.send_request(req)
        )
        .await
        .map_err(|_| ProxyError::Connection("Request timeout".to_string()))?
        .map_err(|e| ProxyError::Http(e.to_string()))?;

        // Modify response and collect body
        let (parts, body) = response.into_parts();
        let body_bytes = body.collect().await
            .map_err(|e| ProxyError::Http(format!("Failed to collect response body: {}", e)))?;

        // Apply response modifications to parts
        let modified_parts = self.modify_response_parts(parts);

        Ok(Response::from_parts(modified_parts, Full::new(body_bytes.to_bytes())))
    }

    fn build_target_uri<B>(&self, req: &Request<B>) -> Result<Uri, ProxyError> {
        let path_and_query = req.uri().path_and_query()
            .ok_or_else(|| ProxyError::Config("Invalid URI path".to_string()))?;

        let target_url_string = format!("{}{}",
            self.target_url.as_str().trim_end_matches('/'),
            path_and_query.as_str()
        );

        let target_uri: Uri = target_url_string.parse()
            .map_err(|e: hyper::http::uri::InvalidUri| ProxyError::Uri(e.to_string()))?;

        Ok(target_uri)
    }

    
    fn modify_request_with_context<B>(&self, req: &mut Request<B>, target_uri: &Uri, context: RequestContext) {
        // Update request URI to target
        // Collect needed headers before mutable borrow
        let original_host = req.headers().get(HOST).cloned();

        *req.uri_mut() = target_uri.clone();

        let headers = req.headers_mut();

        // Handle Host header
        if self.preserve_host {
            // Keep original Host header
        } else {
            // Set Host to target server
            if let Some(authority) = target_uri.authority() {
                headers.insert(HOST, authority.to_string().parse().unwrap());
            }
        }

        // Add X-Forwarded-* headers
        if let Some(client_ip) = &context.client_ip {
            headers.insert(X_FORWARDED_FOR.clone(), client_ip.parse().unwrap());
        }

        headers.insert(X_FORWARDED_PROTO.clone(), "https".parse().unwrap());

        if let Some(host) = original_host {
            headers.insert(X_FORWARDED_HOST.clone(), host);
        }

        // Remove hop-by-hop headers
        headers.remove("Connection");
        headers.remove("Keep-Alive");
        headers.remove("Proxy-Authenticate");
        headers.remove("Proxy-Authorization");
        headers.remove("TE");
        headers.remove("Trailers");
        headers.remove("Transfer-Encoding");
        headers.remove("Upgrade");
    }

    fn modify_response_parts(&self, mut parts: Parts) -> Parts {
        let headers = &mut parts.headers;

        // Remove hop-by-hop headers from response
        headers.remove("Connection");
        headers.remove("Keep-Alive");
        headers.remove("Proxy-Authenticate");
        headers.remove("Proxy-Authorization");
        headers.remove("TE");
        headers.remove("Trailers");
        headers.remove("Transfer-Encoding");
        headers.remove("Upgrade");

        // Add server identification header
        headers.insert("X-Proxy-Server", "rust-reverse-proxy".parse().unwrap());

        parts
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hyper::{Method, Uri};

    #[test]
    fn test_target_uri_building() {
        let proxy = ReverseProxy::new("http://backend.example.com".to_string(), 10, 90, 300).unwrap();

        let req = Request::builder()
            .method(Method::GET)
            .uri("/api/users")
            .body(Full::new(Bytes::new()))
            .unwrap();

        let target_uri = proxy.build_target_uri(&req).unwrap();
        assert_eq!(target_uri.to_string(), "http://backend.example.com/api/users");
    }

    #[test]
    fn test_reverse_proxy_creation() {
        let result = ReverseProxy::new("http://backend.example.com".to_string(), 10, 90, 300);
        assert!(result.is_ok());

        let invalid_url = ReverseProxy::new("not-a-url".to_string(), 10, 90, 300);
        assert!(invalid_url.is_err());
    }

    #[test]
    fn test_modify_request_with_client_ip() {
        let proxy = ReverseProxy::new("http://backend.example.com".to_string(), 10, 90, 300).unwrap();

        let mut req = Request::builder()
            .method(Method::GET)
            .uri("/api/test")
            .header("Host", "example.com")
            .body(Full::new(Bytes::new()))
            .unwrap();

        let target_uri: Uri = "http://backend.example.com/api/test".parse().unwrap();
        let client_ip = "192.168.1.100".to_string();
        let context = RequestContext {
            client_ip: Some(client_ip.clone()),
        };

        proxy.modify_request_with_context(&mut req, &target_uri, context);

        // Verify X-Forwarded-For header is set
        assert_eq!(
            req.headers().get("x-forwarded-for").unwrap().to_str().unwrap(),
            client_ip
        );

        // Verify X-Forwarded-Proto header is set
        assert_eq!(
            req.headers().get("x-forwarded-proto").unwrap().to_str().unwrap(),
            "https"
        );

        // Verify X-Forwarded-Host header is set
        assert_eq!(
            req.headers().get("x-forwarded-host").unwrap().to_str().unwrap(),
            "example.com"
        );
    }

    #[test]
    fn test_modify_request_without_client_ip() {
        let proxy = ReverseProxy::new("http://backend.example.com".to_string(), 10, 90, 300).unwrap();

        let mut req = Request::builder()
            .method(Method::GET)
            .uri("/api/test")
            .header("Host", "example.com")
            .body(Full::new(Bytes::new()))
            .unwrap();

        let target_uri: Uri = "http://backend.example.com/api/test".parse().unwrap();
        let context = RequestContext {
            client_ip: None,
        };

        proxy.modify_request_with_context(&mut req, &target_uri, context);

        // Verify X-Forwarded-For header is NOT set when client_ip is None
        assert!(req.headers().get("x-forwarded-for").is_none());

        // But other headers should still be set
        assert_eq!(
            req.headers().get("x-forwarded-proto").unwrap().to_str().unwrap(),
            "https"
        );

        assert_eq!(
            req.headers().get("x-forwarded-host").unwrap().to_str().unwrap(),
            "example.com"
        );
    }
}