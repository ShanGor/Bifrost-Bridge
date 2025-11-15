use crate::error::ProxyError;
use hyper::{Body, Client, Request, Response, Server, StatusCode, Uri};
use hyper::client::HttpConnector;
use hyper::header::HOST;
use hyper::header::HeaderName;
use hyper::service::{make_service_fn, service_fn};

// Custom header names for X-Forwarded-* headers
static X_FORWARDED_FOR: HeaderName = HeaderName::from_static("x-forwarded-for");
static X_FORWARDED_PROTO: HeaderName = HeaderName::from_static("x-forwarded-proto");
static X_FORWARDED_HOST: HeaderName = HeaderName::from_static("x-forwarded-host");
use std::convert::Infallible;
use std::net::SocketAddr;
use tokio::time::{timeout, Duration};
use url::Url;

pub struct ReverseProxy {
    client: Client<HttpConnector>,
    target_url: Url,
    timeout_duration: Duration,
    preserve_host: bool,
}

impl ReverseProxy {
    pub fn new(target_url: String, timeout_secs: u64) -> Result<Self, ProxyError> {
        let url = Url::parse(&target_url)
            .map_err(|e| ProxyError::Url(e))?;

        Ok(Self {
            client: Client::builder()
                .pool_max_idle_per_host(10)
                .build_http(),
            target_url: url,
            timeout_duration: Duration::from_secs(timeout_secs),
            preserve_host: true,
        })
    }

    pub fn with_preserve_host(mut self, preserve_host: bool) -> Self {
        self.preserve_host = preserve_host;
        self
    }

    pub async fn run(self, addr: SocketAddr) -> Result<(), ProxyError> {
        let target_url = self.target_url.clone();
        let timeout_duration = self.timeout_duration;
        let preserve_host = self.preserve_host;

        let make_svc = make_service_fn(move |_conn| {
            let target_url = target_url.clone();
            let timeout_duration = timeout_duration;
            let preserve_host = preserve_host;

            async move {
                Ok::<_, Infallible>(service_fn(move |req| {
                    let client = Client::builder()
                        .pool_max_idle_per_host(10)
                        .build_http();
                    let proxy = ReverseProxy {
                        client,
                        target_url: target_url.clone(),
                        timeout_duration,
                        preserve_host,
                    };
                    async move {
                        proxy.handle_request(req).await
                    }
                }))
            }
        });

        let server = Server::bind(&addr).serve(make_svc);
        println!("Reverse proxy listening on: {} -> {}", addr, self.target_url);

        if let Err(e) = server.await {
            eprintln!("Server error: {}", e);
            return Err(ProxyError::Hyper(e.to_string()));
        }

        Ok(())
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
        // Construct target URL
        let target_uri = self.build_target_uri(&req)?;

        // Modify request for reverse proxy
        self.modify_request(&mut req, &target_uri);

        // Send request with timeout
        let response = timeout(self.timeout_duration, self.client.request(req))
            .await
            .map_err(|_| ProxyError::Connection("Request timeout".to_string()))?
            .map_err(|e| ProxyError::Http(e.to_string()))?;

        // Modify response
        let modified_response = self.modify_response(response);

        Ok(modified_response)
    }

    fn build_target_uri(&self, req: &Request<Body>) -> Result<Uri, ProxyError> {
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

    fn modify_request(&self, req: &mut Request<Body>, target_uri: &Uri) {
        // Update request URI to target
        // Collect needed headers before mutable borrow
        let client_ip = self.get_client_ip(req);
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
        if let Some(client_ip) = client_ip {
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

    fn modify_response(&self, mut response: Response<Body>) -> Response<Body> {
        let headers = response.headers_mut();

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

        response
    }

    fn get_client_ip(&self, _req: &Request<Body>) -> Option<String> {
        // Try to get client IP from connection info
        // In a real implementation, you'd get this from the connection
        // For now, we'll return a placeholder
        Some("127.0.0.1".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hyper::{Method, Uri};

    #[test]
    fn test_target_uri_building() {
        let proxy = ReverseProxy::new("http://backend.example.com".to_string(), 30).unwrap();

        let mut req = Request::builder()
            .method(Method::GET)
            .uri("/api/users")
            .body(Body::empty())
            .unwrap();

        let target_uri = proxy.build_target_uri(&req).unwrap();
        assert_eq!(target_uri.to_string(), "http://backend.example.com/api/users");
    }

    #[test]
    fn test_reverse_proxy_creation() {
        let result = ReverseProxy::new("http://backend.example.com".to_string(), 30);
        assert!(result.is_ok());

        let invalid_url = ReverseProxy::new("not-a-url".to_string(), 30);
        assert!(invalid_url.is_err());
    }
}