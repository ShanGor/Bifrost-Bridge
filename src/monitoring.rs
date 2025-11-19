use crate::common::{HtmlTemplates, MetricsSummary, MonitoringHandles};
use crate::config::MonitoringConfig;
use crate::error::ProxyError;
use bytes::Bytes;
use http_body_util::Full;
use hyper::{Request, Response, StatusCode};
use hyper::body::Incoming;
use hyper::server::conn::http1::Builder as ServerBuilder;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use serde::Serialize;
use serde_json::json;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct MonitoringServer {
    config: MonitoringConfig,
    handles: MonitoringHandles,
}

impl MonitoringServer {
    pub fn new(config: MonitoringConfig, handles: MonitoringHandles) -> Self {
        Self { config, handles }
    }

    pub async fn run(self) -> Result<(), ProxyError> {
        let addr = self.config.listen_address
            .unwrap_or_else(|| "127.0.0.1:9900".parse().expect("default monitoring socket"));

        let listener = tokio::net::TcpListener::bind(&addr).await
            .map_err(|e| ProxyError::Io(e))?;

        log::info!("Monitoring server listening on http://{}", addr);

        let state = Arc::new(MonitoringState {
            config: self.config,
            handles: self.handles,
        });

        loop {
            let (stream, remote_addr) = listener.accept().await
                .map_err(|e| ProxyError::Io(e))?;
            let state = state.clone();

            tokio::spawn(async move {
                let io = TokioIo::new(stream);
                if let Err(err) = ServerBuilder::new()
                    .serve_connection(
                        io,
                        service_fn(move |req| {
                            let state = state.clone();
                            async move {
                                let response = state.route(req).await;
                                Ok::<_, Infallible>(response)
                            }
                        })
                    )
                    .await
                {
                    log::error!("Monitoring connection error from {}: {}", remote_addr, err);
                }
            });
        }
    }
}

struct MonitoringState {
    config: MonitoringConfig,
    handles: MonitoringHandles,
}

impl MonitoringState {
    async fn route(&self, req: Request<Incoming>) -> Response<Full<Bytes>> {
        match req.uri().path() {
            path if path == self.config.metrics_endpoint => self.handle_metrics(),
            path if path == self.config.health_endpoint => self.handle_health(),
            path if path == self.config.status_endpoint => self.handle_status(),
            _ => Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Full::new(Bytes::from("Monitoring endpoint not found")))
                .unwrap(),
        }
    }

    fn handle_metrics(&self) -> Response<Full<Bytes>> {
        match self.handles.registry().encode() {
            Ok(payload) => Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", "text/plain; version=0.0.4; charset=utf-8")
                .body(Full::new(Bytes::from(payload)))
                .unwrap(),
            Err(e) => {
                log::error!("Failed to encode Prometheus metrics: {}", e);
                Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .body(Full::new(Bytes::from("metrics unavailable")))
                    .unwrap()
            }
        }
    }

    fn handle_status(&self) -> Response<Full<Bytes>> {
        let summary = self.aggregate_summary();
        let html = HtmlTemplates::render_metrics_dashboard(&summary);
        Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "text/html; charset=utf-8")
            .body(Full::new(Bytes::from(html)))
            .unwrap()
    }

    fn handle_health(&self) -> Response<Full<Bytes>> {
        let proxies = self.collect_proxy_health();
        let mut status = "healthy";

        if proxies.iter().any(|p| p.connection_errors > 50 || p.average_response_time_ms > 1000) {
            status = "degraded";
        }
        if proxies.iter().any(|p| p.connection_errors > 500 || p.average_response_time_ms > 5_000) {
            status = "unhealthy";
        }

        let payload = json!({
            "status": status,
            "timestamp": current_timestamp(),
            "proxies": proxies,
        });

        Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "application/json")
            .body(Full::new(Bytes::from(payload.to_string())))
            .unwrap()
    }

    fn aggregate_summary(&self) -> MetricsSummary {
        let mut summary = MetricsSummary {
            requests_total: 0,
            response_bytes_total: 0,
            files_served: 0,
            files_streamed: 0,
            connections_active: 0,
            connection_errors: 0,
            average_response_time_ms: 0,
            timestamp: current_timestamp(),
        };

        let mut avg_samples = 0;

        for (_, metrics) in self.handles.all_metrics() {
            let proxy_summary = metrics.get_metrics_summary();
            summary.requests_total += proxy_summary.requests_total;
            summary.response_bytes_total += proxy_summary.response_bytes_total;
            summary.files_served += proxy_summary.files_served;
            summary.files_streamed += proxy_summary.files_streamed;
            summary.connections_active += proxy_summary.connections_active;
            summary.connection_errors += proxy_summary.connection_errors;
            if proxy_summary.average_response_time_ms > 0 {
                summary.average_response_time_ms += proxy_summary.average_response_time_ms;
                avg_samples += 1;
            }
        }

        if avg_samples > 0 {
            summary.average_response_time_ms /= avg_samples;
        }

        summary
    }

    fn collect_proxy_health(&self) -> Vec<ProxyHealth> {
        self.handles
            .all_metrics()
            .into_iter()
            .map(|(proxy_type, metrics)| {
                let summary = metrics.get_metrics_summary();
                ProxyHealth {
                    proxy_type: proxy_type.to_string(),
                    requests_total: summary.requests_total,
                    connections_active: summary.connections_active,
                    connection_errors: summary.connection_errors,
                    average_response_time_ms: summary.average_response_time_ms,
                }
            })
            .collect()
    }
}

#[derive(Serialize)]
struct ProxyHealth {
    proxy_type: String,
    requests_total: u64,
    connections_active: u64,
    connection_errors: u64,
    average_response_time_ms: u64,
}

fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
