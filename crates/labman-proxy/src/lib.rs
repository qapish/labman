//! labman-proxy: OpenAI-compatible HTTP proxy layer.
//!
//! This crate owns the HTTP surface for OpenAI-style APIs (e.g. `/v1/models`,
//! `/v1/chat/completions`) and delegates endpoint discovery and scheduling to
//! `labman-endpoints`.
//!
//! For now, this module provides a minimal HTTP server skeleton with a single
//! `GET /v1/models` route backed by `EndpointRegistry::to_node_capabilities()`.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use labman_core::ModelDescriptor;
use labman_endpoints::EndpointRegistry;
use labman_telemetry::MetricsRecorder;
use serde::Serialize;
use tokio::task::JoinHandle;
use tracing::{error, info};

/// Error type for the proxy server.
#[derive(Debug)]
pub enum ProxyError {
    /// Failed to bind/serve HTTP.
    Http(String),
}

impl std::fmt::Display for ProxyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProxyError::Http(msg) => write!(f, "proxy HTTP error: {}", msg),
        }
    }
}

impl std::error::Error for ProxyError {}

/// Application state shared across HTTP handlers.
///
/// This holds:
/// - A shared `EndpointRegistry` for model discovery and (future) routing.
/// - A shared `MetricsRecorder` for request/response metrics.
#[derive(Clone)]
pub struct ProxyState {
    pub registry: Arc<tokio::sync::Mutex<EndpointRegistry>>,
    pub metrics: Arc<dyn MetricsRecorder>,
}

/// Configuration for the proxy HTTP server.
#[derive(Debug, Clone)]
pub struct ProxyConfig {
    /// Address to bind the proxy on, typically the WireGuard IP + proxy port.
    pub listen_addr: SocketAddr,
}

/// Handle to a running proxy server.
pub struct ProxyServer {
    cfg: ProxyConfig,
    state: ProxyState,
}

impl ProxyServer {
    /// Create a new proxy server with the given configuration and state.
    pub fn new(
        cfg: ProxyConfig,
        registry: EndpointRegistry,
        metrics: Arc<dyn MetricsRecorder>,
    ) -> Self {
        let state = ProxyState {
            registry: Arc::new(tokio::sync::Mutex::new(registry)),
            metrics,
        };

        Self { cfg, state }
    }

    /// Return the shared registry handle.
    pub fn registry(&self) -> Arc<tokio::sync::Mutex<EndpointRegistry>> {
        Arc::clone(&self.state.registry)
    }

    /// Return the shared `MetricsRecorder`.
    pub fn metrics(&self) -> Arc<dyn MetricsRecorder> {
        Arc::clone(&self.state.metrics)
    }

    /// Build the Axum `Router` for this proxy.
    fn router(&self) -> Router {
        Router::new()
            .route("/v1/models", get(get_models))
            .with_state(self.state.clone())
    }

    /// Spawn the HTTP server on the current Tokio runtime and return a handle.
    pub fn spawn(self) -> JoinHandle<Result<(), ProxyError>> {
        tokio::spawn(self.run())
    }

    /// Run the HTTP server until it exits.
    pub async fn run(self) -> Result<(), ProxyError> {
        let addr = self.cfg.listen_addr;
        let app = self.router();

        info!("labman-proxy: binding HTTP server on {}", addr);

        let listener = match tokio::net::TcpListener::bind(addr).await {
            Ok(l) => l,
            Err(e) => {
                return Err(ProxyError::Http(format!(
                    "failed to bind proxy listener on {}: {}",
                    addr, e
                )));
            }
        };

        info!("labman-proxy: listening on {}", addr);

        loop {
            let (stream, peer_addr) = match listener.accept().await {
                Ok(pair) => pair,
                Err(e) => {
                    error!("labman-proxy: accept error: {}", e);
                    return Err(ProxyError::Http(e.to_string()));
                }
            };

            let svc = app.clone();
            let io = hyper_util::rt::TokioIo::new(stream);
            let conn = hyper::server::conn::http1::Builder::new()
                .serve_connection(io, hyper_util::service::TowerToHyperService::new(svc))
                .with_upgrades();

            tokio::spawn(async move {
                if let Err(e) = conn.await {
                    error!("labman-proxy: error serving {}: {}", peer_addr, e);
                }
            });
        }
    }
}

/// Response type for `/v1/models`.
///
/// This mirrors the OpenAI `list` response: a wrapper with `object = "list"`
/// and `data = Vec<ModelDescriptor>`.
#[derive(Serialize)]
struct ModelsResponse {
    object: String,
    data: Vec<ModelDescriptor>,
}

/// Handler for `GET /v1/models`.
///
/// This aggregates the models discovered by `EndpointRegistry` into a single
/// OpenAI-compatible `list` response.
///
/// Notes:
/// - Currently, this does not expose per-endpoint metadata; it simply returns
///   the union of unique model IDs discovered across endpoints.
/// - In future iterations we may want to:
///   - Include additional fields (e.g. which endpoints support which models).
///   - Attach metrics (e.g. per-model popularity).
async fn get_models(State(state): State<ProxyState>) -> Json<ModelsResponse> {
    let registry = state.registry.lock().await;
    let caps = registry.to_node_capabilities();

    let models = caps.models.clone();
    drop(registry);

    // Record a simple metric for the models listing request.
    state
        .metrics
        .record_request_end(Some("proxy"), Some("models_list"), true, None);

    Json(ModelsResponse {
        object: "list".to_string(),
        data: models,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;
    use tower::util::ServiceExt;

    fn empty_registry() -> EndpointRegistry {
        EndpointRegistry::from_config(&LabmanConfigBuilder::empty()).unwrap()
    }

    struct LabmanConfigBuilder;

    impl LabmanConfigBuilder {
        fn empty() -> labman_config::LabmanConfig {
            use labman_config::{
                ControlPlaneConfig, ProxyConfig, TelemetryConfig, WireGuardConfig,
            };

            labman_config::LabmanConfig {
                control_plane: ControlPlaneConfig {
                    base_url: "https://control.local/api/v1".to_string(),
                    node_token: "test-token".to_string(),
                    region: None,
                    description: None,
                },
                wireguard: WireGuardConfig {
                    interface_name: "labman0".to_string(),
                    address: Some("10.90.0.2/32".to_string()),
                    private_key_path: None,
                    public_key_path: None,
                    peer_endpoint: None,
                    allowed_ips: Vec::new(),
                    rosenpass: None,
                },
                proxy: ProxyConfig {
                    listen_port: 8080,
                    listen_addr: None,
                },
                telemetry: Some(TelemetryConfig {
                    log_level: Some("info".to_string()),
                    log_format: Some("text".to_string()),
                    disable_metrics: false,
                    metrics_port: 9090,
                }),
                endpoints: Vec::new(),
            }
        }
    }

    struct NoopMetrics;

    impl MetricsRecorder for NoopMetrics {
        fn record_request_start(&self, _endpoint: Option<&str>, _model: Option<&str>) {}
        fn record_request_end(
            &self,
            _endpoint: Option<&str>,
            _model: Option<&str>,
            _success: bool,
            _latency_secs: Option<f64>,
        ) {
        }
        fn record_error(&self, _endpoint: Option<&str>, _kind: &str) {}
        fn set_active_requests(&self, _count: u64) {}
    }

    #[tokio::test]
    async fn get_models_returns_empty_list_for_empty_registry() {
        let registry = empty_registry();
        let metrics: Arc<dyn MetricsRecorder> = Arc::new(NoopMetrics);
        let state = ProxyState {
            registry: Arc::new(tokio::sync::Mutex::new(registry)),
            metrics,
        };

        let app = Router::new()
            .route("/v1/models", get(get_models))
            .with_state(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/v1/models")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), axum::http::StatusCode::OK);
    }
}
