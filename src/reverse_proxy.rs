use crate::common::{ConnectionTracker, PerformanceMetrics, RequestTimer, ResponseBuilder, is_websocket_upgrade};
use crate::error::ProxyError;
use crate::config::{ReverseProxyConfig, HealthCheckConfig, WebSocketConfig};
use crate::rate_limit::RateLimiter;
use hyper::{Request, Response, StatusCode, Uri, Method};
use hyper::body::{Body as _, Incoming};
use http_body_util::{BodyExt, Full, Empty};
use hyper::body::Bytes;
use hyper::server::conn::http1::Builder as ServerBuilder;
use hyper::header::{HOST, ORIGIN};
use hyper::header::HeaderName;
use log::{info, error, warn, debug};
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use hyper_util::client::legacy::{Client, connect::HttpConnector};
use hyper_util::rt::{TokioExecutor, TokioTimer};
use std::sync::Arc;
use tokio::io::copy_bidirectional;

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
pub struct RequestContext {
    pub client_ip: Option<String>,
}

pub struct ReverseProxy {
    target_url: Url,
    preserve_host: bool,
    // HTTP client with connection pooling
    http_client: Arc<Client<HttpConnector, Incoming>>,
    // Health check configuration
    health_check_config: Option<HealthCheckConfig>,
    metrics: Arc<PerformanceMetrics>,
    websocket_config: WebSocketConfig,
    rate_limiter: Arc<RateLimiter>,
}

impl ReverseProxy {
    /// Creates a new reverse proxy with default pooling configuration
    pub fn new(target_url: String, connect_timeout_secs: u64, idle_timeout_secs: u64, max_connection_lifetime_secs: u64) -> Result<Self, ProxyError> {
        Self::new_with_config(
            target_url,
            connect_timeout_secs,
            idle_timeout_secs,
            max_connection_lifetime_secs,
            None,
            None,
        )
    }

    /// Creates a new reverse proxy with custom pooling configuration
    pub fn new_with_config(
        target_url: String,
        connect_timeout_secs: u64,
        _idle_timeout_secs: u64,
        _max_connection_lifetime_secs: u64,
        reverse_proxy_config: Option<ReverseProxyConfig>,
        websocket_config: Option<WebSocketConfig>,
    ) -> Result<Self, ProxyError> {
        let url = Url::parse(&target_url)
            .map_err(|e| ProxyError::Url(e))?;

        // Get pool configuration
        let pool_config = reverse_proxy_config.unwrap_or_default();
        let health_check_config = pool_config.health_check.clone();

        // Build HTTP client with connection pooling
        let http_client = Self::build_http_client(
            connect_timeout_secs,
            pool_config.pool_max_idle_per_host,
            pool_config.pool_idle_timeout_secs,
        );

        info!("Reverse proxy configuration: pool_max_idle_per_host={}, pool_idle_timeout={}s",
              pool_config.pool_max_idle_per_host, pool_config.pool_idle_timeout_secs);

        if let Some(ref health_check) = health_check_config {
            info!("Health check enabled: interval={}s, timeout={}s, endpoint={:?}",
                  health_check.interval_secs, health_check.timeout_secs, health_check.endpoint);
        }

        Ok(Self {
            target_url: url,
            preserve_host: true,
            http_client: Arc::new(http_client),
            health_check_config,
            metrics: Arc::new(PerformanceMetrics::new()),
            websocket_config: websocket_config.unwrap_or_default(),
            rate_limiter: Arc::new(RateLimiter::new(None)),
        })
    }

    /// Build HTTP client for reverse proxy with connection pooling
    ///
    /// Reverse proxy pooling strategy:
    /// - Connects to a single fixed backend server
    /// - Maintains persistent connection pool for better performance
    /// - pool_max_idle_per_host: 0-50 (user configurable, default: 10)
    /// - pool_idle_timeout: Long timeout (60-90s) to keep connections warm
    /// - Health checks ensure pooled connections are healthy
    fn build_http_client(
        connect_timeout_secs: u64,
        pool_max_idle_per_host: usize,
        pool_idle_timeout_secs: u64,
    ) -> Client<HttpConnector, Incoming> {
        let mut connector = HttpConnector::new();
        connector.set_connect_timeout(Some(Duration::from_secs(connect_timeout_secs)));
        connector.set_keepalive(Some(Duration::from_secs(pool_idle_timeout_secs)));
        connector.set_nodelay(true);

        let mut builder = Client::builder(TokioExecutor::new());

        if pool_max_idle_per_host == 0 {
            info!("Reverse proxy: connection pooling DISABLED (pool_max_idle_per_host=0)");
            builder.pool_max_idle_per_host(0);
        } else {
            info!("Reverse proxy: connection pooling ENABLED (pool_max_idle_per_host={}, idle_timeout={}s)",
                  pool_max_idle_per_host, pool_idle_timeout_secs);
            builder.pool_max_idle_per_host(pool_max_idle_per_host);
            builder.pool_idle_timeout(Duration::from_secs(pool_idle_timeout_secs));
            builder.pool_timer(TokioTimer::new());
        }

        builder
            .http2_only(false)
            .build(connector)
    }

    pub fn with_preserve_host(mut self, preserve_host: bool) -> Self {
        self.preserve_host = preserve_host;
        self
    }

    pub fn with_metrics(mut self, metrics: Arc<PerformanceMetrics>) -> Self {
        self.metrics = metrics;
        self
    }

    pub fn with_rate_limiter(mut self, rate_limiter: Arc<RateLimiter>) -> Self {
        self.rate_limiter = rate_limiter;
        self
    }

    /// Public method for handling individual requests (used by CombinedProxyAdapter)
    pub async fn handle_request_with_context(&self, req: Request<Incoming>, context: RequestContext) -> Result<Response<Full<Bytes>>, Infallible> {
        Self::handle_request_static(
            req,
            context,
            self.http_client.clone(),
            self.target_url.clone(),
            self.preserve_host,
            Arc::new(self.websocket_config.clone()),
            self.metrics.clone(),
            self.rate_limiter.clone(),
        ).await
    }

    pub async fn run(self, addr: SocketAddr) -> Result<(), ProxyError> {
        let listener = tokio::net::TcpListener::bind(addr).await
            .map_err(|e| ProxyError::Hyper(e.to_string()))?;

        info!("Reverse proxy listening on: {} -> {}", addr, self.target_url);

        // Start health check task if configured
        if let Some(health_check_config) = self.health_check_config.clone() {
            let target_url = self.target_url.clone();
            let http_client = self.http_client.clone();
            tokio::spawn(async move {
                Self::health_check_loop(http_client, target_url, health_check_config).await;
            });
        }

        let http_client = self.http_client.clone();
        let target_url = self.target_url.clone();
        let preserve_host = self.preserve_host;
        let websocket_config = Arc::new(self.websocket_config.clone());
        let metrics = self.metrics.clone();
        let rate_limiter = self.rate_limiter.clone();

        loop {
            let (stream, remote_addr) = listener.accept().await
                .map_err(|e| ProxyError::Hyper(e.to_string()))?;

            let http_client = http_client.clone();
            let target_url = target_url.clone();
            let metrics = metrics.clone();
            let websocket_config = websocket_config.clone();
            let rate_limiter = rate_limiter.clone();

            tokio::spawn(async move {
                let _connection = ConnectionTracker::new(metrics.clone());
                let io = TokioIo::new(stream);

                if let Err(err) = ServerBuilder::new()
                    .serve_connection(
                        io,
                        service_fn(move |req| {
                            let http_client = http_client.clone();
                            let target_url = target_url.clone();
                            let client_ip = Some(remote_addr.ip().to_string());
                            let metrics = metrics.clone();
                            let websocket_cfg = websocket_config.clone();
                            let rate_limiter = rate_limiter.clone();

                            let context = RequestContext {
                                client_ip: client_ip.clone(),
                            };

                            async move {
                                metrics.increment_requests();
                                let timer = RequestTimer::with_metrics(metrics.clone());
                                let result = Self::handle_request_static(
                                    req,
                                    context,
                                    http_client,
                                    target_url,
                                    preserve_host,
                                    websocket_cfg,
                                    metrics.clone(),
                                    rate_limiter.clone(),
                                ).await;

                                if let Some(len) = result.as_ref()
                                    .ok()
                                    .and_then(|response| response.body().size_hint().exact()) {
                                    metrics.record_response_bytes(len as u64);
                                }
                                timer.finish();
                                result
                            }
                        })
                    )
                    .await
                {
                    error!("Error serving reverse proxy connection: {}", err);
                }
            });
        }
    }

    /// Static method to handle requests (used in service_fn)
    async fn handle_request_static(
        req: Request<Incoming>,
        context: RequestContext,
        http_client: Arc<Client<HttpConnector, Incoming>>,
        target_url: Url,
        preserve_host: bool,
        websocket_config: Arc<WebSocketConfig>,
        metrics: Arc<PerformanceMetrics>,
        rate_limiter: Arc<RateLimiter>,
    ) -> Result<Response<Full<Bytes>>, Infallible> {
        if rate_limiter.is_enabled() {
            if let Some(client_ip) = context.client_ip.as_deref() {
                if let Err(hit) = rate_limiter
                    .check_request(
                        client_ip,
                        req.method(),
                        req.uri()
                            .path_and_query()
                            .map(|pq| pq.as_str())
                            .unwrap_or("/"),
                    )
                    .await
                {
                    warn!(
                        "Reverse proxy rate limit hit for {} via rule {}",
                        client_ip, hit.rule_id
                    );
                    return Ok(ResponseBuilder::too_many_requests(&hit.rule_id, hit.retry_after_secs));
                }
            }
        }

        if is_websocket_upgrade(req.headers()) {
            return Self::handle_websocket_request(req, context, http_client, target_url, preserve_host, websocket_config).await;
        }

        match Self::process_request_static(req, context, http_client, target_url, preserve_host).await {
            Ok(response) => Ok(response),
            Err(e) => {
                error!("Proxy error: {}", e);
                let error_response = Response::builder()
                    .status(StatusCode::BAD_GATEWAY)
                    .body(Full::new(Bytes::from(format!("Proxy Error: {}", e))))
                    .unwrap();
                metrics.increment_connection_errors();
                Ok(error_response)
            }
        }
    }

    /// Process request using HTTP client with connection pooling
    async fn process_request_static(
        req: Request<Incoming>,
        context: RequestContext,
        http_client: Arc<Client<HttpConnector, Incoming>>,
        target_url: Url,
        preserve_host: bool,
    ) -> Result<Response<Full<Bytes>>, ProxyError> {
        let prepared = Self::rewrite_backend_request(req, &context, &target_url, preserve_host, false)?;

        let response = http_client.request(prepared).await
            .map_err(|e| ProxyError::Http(format!("Failed to forward request: {}", e)))?;

        Self::finalize_backend_response(response, false).await
    }

    async fn handle_websocket_request(
        mut req: Request<Incoming>,
        context: RequestContext,
        http_client: Arc<Client<HttpConnector, Incoming>>,
        target_url: Url,
        preserve_host: bool,
        websocket_config: Arc<WebSocketConfig>,
    ) -> Result<Response<Full<Bytes>>, Infallible> {
        if let Err(reason) = Self::validate_websocket_headers(req.headers(), &websocket_config) {
            return Ok(ResponseBuilder::error(StatusCode::FORBIDDEN, &reason));
        }

        let client_upgrade = hyper::upgrade::on(&mut req);
        let prepared_request = match Self::rewrite_backend_request(req, &context, &target_url, preserve_host, true) {
            Ok(request) => request,
            Err(e) => {
                error!("WebSocket request rewrite failed: {}", e);
                return Ok(ResponseBuilder::error(StatusCode::BAD_GATEWAY, "Invalid WebSocket request"));
            }
        };

        let mut backend_response = match http_client.request(prepared_request).await {
            Ok(resp) => resp,
            Err(e) => {
                error!("WebSocket backend request failed: {}", e);
                return Ok(ResponseBuilder::error(StatusCode::BAD_GATEWAY, "WebSocket backend error"));
            }
        };

        if backend_response.status() != StatusCode::SWITCHING_PROTOCOLS {
            return match Self::finalize_backend_response(backend_response, false).await {
                Ok(resp) => Ok(resp),
                Err(e) => {
                    error!("Failed to finalize backend response: {}", e);
                    Ok(ResponseBuilder::error(StatusCode::BAD_GATEWAY, "WebSocket backend error"))
                }
            };
        }

        let backend_upgrade = hyper::upgrade::on(&mut backend_response);
        let (parts, _) = backend_response.into_parts();
        let switch_response = Response::from_parts(parts, Full::new(Bytes::new()));

        tokio::spawn(async move {
            match (client_upgrade.await, backend_upgrade.await) {
                (Ok(client_stream), Ok(backend_stream)) => {
                    let mut client_io = TokioIo::new(client_stream);
                    let mut backend_io = TokioIo::new(backend_stream);
                    if let Err(e) = copy_bidirectional(&mut client_io, &mut backend_io).await {
                        error!("WebSocket tunnel error: {}", e);
                    }
                }
                (Err(e), _) => error!("Client WebSocket upgrade failed: {}", e),
                (_, Err(e)) => error!("Backend WebSocket upgrade failed: {}", e),
            }
        });

        Ok(switch_response)
    }

    fn validate_websocket_headers(headers: &hyper::HeaderMap, config: &WebSocketConfig) -> Result<(), String> {
        if !config.enabled {
            return Err("WebSocket support is disabled".to_string());
        }

        if config.allowed_origins.iter().all(|o| o != "*") {
            let origin = headers.get(ORIGIN)
                .and_then(|v| v.to_str().ok())
                .ok_or_else(|| "Origin header is required for WebSocket requests".to_string())?;

            if !config.allowed_origins.iter().any(|allowed| allowed.eq_ignore_ascii_case(origin)) {
                return Err("Origin not allowed".to_string());
            }
        }

        if !config.supported_protocols.is_empty() {
            let offered = headers.get("Sec-WebSocket-Protocol")
                .and_then(|v| v.to_str().ok())
                .map(|raw| raw.split(',').map(|s| s.trim().to_string()).collect::<Vec<_>>())
                .unwrap_or_else(|| Vec::new());

            if offered.is_empty() {
                return Err("WebSocket subprotocol required".to_string());
            }

            let supported = config.supported_protocols.iter().map(|p| p.to_ascii_lowercase()).collect::<Vec<_>>();
            if !offered.iter().any(|offer| supported.iter().any(|allowed| allowed == &offer.to_ascii_lowercase())) {
                return Err("Unsupported WebSocket subprotocol".to_string());
            }
        }

        Ok(())
    }

    fn rewrite_backend_request(
        mut req: Request<Incoming>,
        context: &RequestContext,
        target_url: &Url,
        preserve_host: bool,
        keep_upgrade: bool,
    ) -> Result<Request<Incoming>, ProxyError> {
        let path_and_query = req.uri().path_and_query()
            .ok_or_else(|| ProxyError::Config("Invalid URI path".to_string()))?;

        let target_url_string = format!(
            "{}{}",
            target_url.as_str().trim_end_matches('/'),
            path_and_query.as_str()
        );

        let target_uri: Uri = target_url_string.parse()
            .map_err(|e: hyper::http::uri::InvalidUri| ProxyError::Uri(e.to_string()))?;

        let original_host = req.headers().get(HOST).cloned();
        *req.uri_mut() = target_uri.clone();

        let headers = req.headers_mut();

        if !preserve_host {
            if let Some(authority) = target_uri.authority() {
                headers.insert(HOST, authority.to_string().parse().unwrap());
            }
        }

        if let Some(client_ip) = &context.client_ip {
            headers.insert(X_FORWARDED_FOR.clone(), client_ip.parse().unwrap());
        }
        headers.insert(X_FORWARDED_PROTO.clone(), "https".parse().unwrap());
        if let Some(host) = original_host {
            headers.insert(X_FORWARDED_HOST.clone(), host);
        }

        Self::strip_request_headers(headers, keep_upgrade);
        Ok(req)
    }

    fn strip_request_headers(headers: &mut hyper::HeaderMap, keep_upgrade: bool) {
        if !keep_upgrade {
            headers.remove("Connection");
            headers.remove("Upgrade");
        }
        headers.remove("Keep-Alive");
        headers.remove("Proxy-Authenticate");
        headers.remove("Proxy-Authorization");
        headers.remove("TE");
        headers.remove("Trailers");
        headers.remove("Transfer-Encoding");
    }

    async fn finalize_backend_response(
        response: Response<Incoming>,
        keep_upgrade: bool,
    ) -> Result<Response<Full<Bytes>>, ProxyError> {
        let (mut parts, body) = response.into_parts();
        let body_bytes = body.collect().await
            .map_err(|e| ProxyError::Http(format!("Failed to collect response body: {}", e)))?;

        Self::strip_response_headers(&mut parts.headers, keep_upgrade);
        parts.headers.insert("X-Proxy-Server", "rust-reverse-proxy".parse().unwrap());

        Ok(Response::from_parts(parts, Full::new(body_bytes.to_bytes())))
    }

    fn strip_response_headers(headers: &mut hyper::HeaderMap, keep_upgrade: bool) {
        if !keep_upgrade {
            headers.remove("Connection");
            headers.remove("Upgrade");
        }
        headers.remove("Keep-Alive");
        headers.remove("Proxy-Authenticate");
        headers.remove("Proxy-Authorization");
        headers.remove("TE");
        headers.remove("Trailers");
        headers.remove("Transfer-Encoding");
    }

    /// Health check loop (runs in background)
    async fn health_check_loop(
        http_client: Arc<Client<HttpConnector, Incoming>>,
        target_url: Url,
        config: HealthCheckConfig,
    ) {
        let interval = Duration::from_secs(config.interval_secs);
        let timeout = Duration::from_secs(config.timeout_secs);
        let endpoint = config.endpoint;

        info!("Starting health check loop for {}", target_url);

        let mut interval_timer = tokio::time::interval(interval);
        loop {
            interval_timer.tick().await;

            let is_healthy = if let Some(ref endpoint) = endpoint {
                Self::http_health_check(&http_client, &target_url, endpoint, timeout).await
            } else {
                Self::tcp_health_check(&target_url, timeout).await
            };

            if is_healthy {
                debug!("Health check passed for {}", target_url);
            } else {
                warn!("Health check failed for {}", target_url);
            }
        }
    }

    /// TCP health check (default)
    async fn tcp_health_check(target_url: &Url, timeout: Duration) -> bool {
        let host = match target_url.host_str() {
            Some(h) => h,
            None => return false,
        };
        let port = target_url.port().unwrap_or(80);

        match tokio::time::timeout(
            timeout,
            tokio::net::TcpStream::connect((host, port))
        ).await {
            Ok(Ok(_)) => true,
            Ok(Err(e)) => {
                debug!("TCP health check failed: {}", e);
                false
            }
            Err(_) => {
                debug!("TCP health check timeout");
                false
            }
        }
    }

    /// HTTP endpoint health check
    async fn http_health_check(
        _http_client: &Client<HttpConnector, Incoming>,
        target_url: &Url,
        endpoint: &str,
        timeout: Duration,
    ) -> bool {
        let health_url = format!("{}{}",
            target_url.as_str().trim_end_matches('/'),
            endpoint
        );

        // Use a simple HTTP client for health check (not the pooled client)
        let connector = HttpConnector::new();
        let simple_client: Client<HttpConnector, Empty<Bytes>> = Client::builder(TokioExecutor::new())
            .build(connector);

        let request = match Request::builder()
            .method(Method::GET)
            .uri(health_url)
            .body(Empty::<Bytes>::new()) {
            Ok(req) => req,
            Err(e) => {
                debug!("Failed to build health check request: {}", e);
                return false;
            }
        };

        match tokio::time::timeout(timeout, simple_client.request(request)).await {
            Ok(Ok(response)) => {
                let status = response.status();
                status.is_success() || status.is_redirection()
            }
            Ok(Err(e)) => {
                debug!("HTTP health check failed: {}", e);
                false
            }
            Err(_) => {
                debug!("HTTP health check timeout");
                false
            }
        }
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reverse_proxy_creation() {
        let result = ReverseProxy::new("http://backend.example.com".to_string(), 10, 90, 300);
        assert!(result.is_ok());

        let invalid_url = ReverseProxy::new("not-a-url".to_string(), 10, 90, 300);
        assert!(invalid_url.is_err());
    }
}
