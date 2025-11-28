use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use axum::{response::IntoResponse, Router};
use hyper::server::conn::http1;
use hyper_util::rt::TokioIo;
use hyper_util::service::TowerToHyperService;
use labman_telemetry::{MetricsRecorder, NoopMetricsRecorder};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tracing::{error, info};

#[cfg(feature = "prometheus")]
use labman_telemetry::{prometheus_http_response, PrometheusMetricsRecorder};

/// Error type for the HTTP server.
///
/// This is intentionally lightweight; callers (typically `labmand`) can map it
/// into their own error types if desired.
#[derive(Debug)]
pub enum ServerError {
    /// Failed to bind on the requested address.
    BindFailed(String),
    /// The HTTP server encountered a runtime error.
    ServeFailed(String),
}

impl std::fmt::Display for ServerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServerError::BindFailed(msg) => write!(f, "failed to bind HTTP server: {}", msg),
            ServerError::ServeFailed(msg) => write!(f, "HTTP server error: {}", msg),
        }
    }
}

impl std::error::Error for ServerError {}

/// Configuration for the labman HTTP server.
///
/// This is a minimal configuration focused on the metrics endpoint. Future
/// iterations can extend this with additional bind addresses, TLS options,
/// separate public/control-plane listeners, etc.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// Address to bind the HTTP server on, e.g. `10.90.1.2:9090`.
    ///
    /// Operators can choose an address that is:
    /// - Within the WireGuard address space (for control-plane scraping).
    /// - On a LAN interface or 0.0.0.0 (for operator Prometheus/Grafana),
    ///   subject to routing and firewall configuration.
    pub bind_addr: SocketAddr,
}

/// Shared application state for the HTTP server.
///
/// For now this only exposes the metrics recorder. As the server grows, this
/// can be extended with additional shared services (proxy handle, endpoint
/// registry, etc.).
#[derive(Clone)]
struct AppState {
    #[cfg(feature = "prometheus")]
    prometheus: Arc<labman_telemetry::PrometheusMetricsRecorder>,

    #[allow(dead_code)]
    metrics: Arc<dyn MetricsRecorder>,
}

impl AppState {
    fn new(metrics: Arc<dyn MetricsRecorder>) -> Self {
        #[cfg(feature = "prometheus")]
        let prometheus = Arc::new(labman_telemetry::PrometheusMetricsRecorder::new());

        Self {
            #[cfg(feature = "prometheus")]
            prometheus,
            metrics,
        }
    }
}

/// Handle to a running labman HTTP server.
///
/// The main entrypoint (`run`) is `async` and will not return until the server
/// stops (e.g., due to shutdown or error). Callers that want finer-grained
/// control can spawn `run` onto a Tokio task and manage the `JoinHandle`.
pub struct LabmanServer {
    cfg: ServerConfig,
    metrics_recorder: Arc<dyn MetricsRecorder>,
}

impl LabmanServer {
    /// Create a new labman server with the given configuration.
    ///
    /// By default, this will:
    /// - Use a Prometheus-backed metrics recorder when the `prometheus` feature
    ///   is enabled.
    /// - Fall back to a no-op recorder otherwise.
    pub fn new(cfg: ServerConfig) -> Self {
        #[cfg(feature = "prometheus")]
        let recorder: Arc<dyn MetricsRecorder> =
            Arc::new(PrometheusMetricsRecorder::new()) as Arc<dyn MetricsRecorder>;

        #[cfg(not(feature = "prometheus"))]
        let recorder: Arc<dyn MetricsRecorder> =
            Arc::new(NoopMetricsRecorder::default()) as Arc<dyn MetricsRecorder>;

        Self {
            cfg,
            metrics_recorder: recorder,
        }
    }

    /// Get a shared reference to the underlying metrics recorder.
    ///
    /// This allows other components (e.g., proxy or endpoint layers) to record
    /// metrics without needing to know which concrete backend is in use.
    pub fn metrics_recorder(&self) -> Arc<dyn MetricsRecorder> {
        Arc::clone(&self.metrics_recorder)
    }

    /// Spawn the HTTP server onto the current Tokio runtime and return a handle.
    pub fn spawn(self) -> JoinHandle<Result<(), ServerError>> {
        tokio::spawn(self.run())
    }

    /// Run the HTTP server until shutdown.
    ///
    /// This starts an `axum` + `hyper` server bound on the configured
    /// `bind_addr` and exposes:
    ///
    /// - `GET /metrics` — Prometheus metrics when the `prometheus` feature is
    ///   enabled; otherwise a 501 (Not Implemented).
    ///
    /// All other paths currently return 404.
    pub async fn run(self) -> Result<(), ServerError> {
        let addr = self.cfg.bind_addr;

        info!("labman-server: binding HTTP server on {}", addr);

        let state = AppState::new(self.metrics_recorder.clone());

        let app = Router::new()
            .route("/metrics", get(metrics_handler))
            .with_state(state);

        let listener = TcpListener::bind(addr)
            .await
            .map_err(|e| ServerError::BindFailed(e.to_string()))?;

        info!("labman-server: listening on {}", addr);

        loop {
            let (stream, peer_addr) = match listener.accept().await {
                Ok(s) => s,
                Err(e) => {
                    error!("labman-server: accept error: {}", e);
                    return Err(ServerError::ServeFailed(e.to_string()));
                }
            };

            let svc = app.clone();
            let io = TokioIo::new(stream);
            let conn = http1::Builder::new()
                .serve_connection(io, TowerToHyperService::new(svc))
                .with_upgrades();

            tokio::spawn(async move {
                if let Err(e) = conn.await {
                    error!("labman-server: error serving {}: {}", peer_addr, e);
                }
            });
        }
    }
}

/// Handler for `GET /metrics`.
///
/// When the `prometheus` feature is enabled, this returns a Prometheus text
/// exposition payload backed by the internal registry. Otherwise, we return a
/// 501 to signal that metrics support is not compiled in.
async fn metrics_handler(State(_state): State<AppState>) -> impl IntoResponse {
    #[cfg(feature = "prometheus")]
    {
        // Use the shared Prometheus recorder from application state so that
        // metrics recorded elsewhere in the process are included in the
        // exported registry.
        let resp = prometheus_http_response(_state.prometheus.registry());

        let (parts, body_bytes) = resp.into_parts();
        let body = axum::body::Body::from(body_bytes);

        (parts.status, parts.headers, body).into_response()
    }

    #[cfg(not(feature = "prometheus"))]
    {
        (
            StatusCode::NOT_IMPLEMENTED,
            [("Content-Type", "text/plain; charset=utf-8")],
            Bytes::from_static(b"Prometheus metrics not enabled\n"),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;
    use tower::util::ServiceExt; // for `oneshot`

    #[tokio::test]
    async fn test_not_found_for_unknown_path() {
        let recorder: Arc<dyn MetricsRecorder> =
            Arc::new(NoopMetricsRecorder::default()) as Arc<dyn MetricsRecorder>;
        let state = AppState::new(recorder);

        let app = Router::new()
            .route("/metrics", get(metrics_handler))
            .with_state(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/unknown")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
