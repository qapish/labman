use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use labman_config::{EndpointConfig, LabmanConfig};
use labman_core::endpoint::Endpoint;
use labman_core::{LabmanError, ModelDescriptor, ModelListResponse, NodeCapabilities, Result};
use labman_telemetry::MetricsRecorder;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing;

/// Errors specific to endpoint registry operations.
#[derive(Debug, Error)]
pub enum EndpointRegistryError {
    #[error("duplicate endpoint name: {0}")]
    DuplicateEndpointName(String),

    #[error("invalid endpoint base_url for '{name}': {reason}")]
    InvalidEndpointUrl { name: String, reason: String },
}

impl From<EndpointRegistryError> for LabmanError {
    fn from(err: EndpointRegistryError) -> Self {
        LabmanError::config(err.to_string())
    }
}

/// Metadata associated with a configured endpoint, beyond what is stored in
/// `labman_core::Endpoint`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EndpointMeta {
    /// Maximum number of concurrent requests allowed for this endpoint.
    pub max_concurrent: Option<usize>,

    /// Glob patterns for model inclusion.
    pub models_include: Option<Vec<String>>,

    /// Glob patterns for model exclusion.
    pub models_exclude: Option<Vec<String>>,
}

/// A registry of configured endpoints on this node.
///
/// This is the central in-process view of all OpenAI-compatible upstreams
/// (Ollama, vLLM, llama.cpp, etc.) that labman can proxy traffic to.
///
/// The registry is designed to be wrapped in an `Arc<tokio::sync::Mutex<_>>`
/// when used from async contexts so that background tasks (such as periodic
/// health checks and model discovery) can mutate shared state safely.
pub struct EndpointRegistry {
    /// Endpoints keyed by logical name.
    endpoints: HashMap<String, EndpointEntry>,

    /// Optional shared metrics recorder for emitting health and request metrics.
    ///
    /// This is provided via `EndpointRegistryBuilder::with_metrics` so that
    /// the registry can remain usable in environments where metrics are not
    /// desired while still allowing rich telemetry in production.
    metrics: Option<Arc<dyn MetricsRecorder>>,

    /// Index of model ID -> endpoint names that currently advertise that model.
    ///
    /// This is derived from `EndpointEntry.discovered_models` and is intended
    /// for use by routing and capability reporting logic.
    model_index: HashMap<String, Vec<String>>,
}

/// A single entry in the registry.
#[derive(Debug)]
pub struct EndpointEntry {
    /// The core endpoint representation used throughout the system.
    pub endpoint: Endpoint,

    /// Static configuration metadata (concurrency limits, filters).
    pub meta: EndpointMeta,

    /// Current number of active requests (for scheduling, not yet used).
    active_requests: usize,

    /// Whether this endpoint is currently considered healthy.
    ///
    /// For now this is managed purely by the registry's health check methods
    /// and not yet exposed externally.
    healthy: bool,

    /// Models discovered from this endpoint via `/v1/models`.
    ///
    /// This will be populated by model discovery logic and used for routing
    /// decisions and capability reporting.
    discovered_models: Vec<ModelDescriptor>,
}

impl EndpointRegistry {
    /// Construct an `EndpointRegistry` from the loaded configuration.
    ///
    /// This performs basic validation and normalisation of endpoint configs,
    /// but does not contact the upstreams (health checks and model discovery
    /// are handled by higher-level logic).
    pub fn from_config(cfg: &LabmanConfig) -> Result<Self> {
        let mut endpoints = HashMap::new();

        for ep_cfg in &cfg.endpoints {
            if endpoints.contains_key(&ep_cfg.name) {
                return Err(
                    EndpointRegistryError::DuplicateEndpointName(ep_cfg.name.clone()).into(),
                );
            }

            let endpoint = Self::build_core_endpoint(ep_cfg)?;
            let meta = EndpointMeta {
                max_concurrent: ep_cfg.max_concurrent,
                models_include: ep_cfg.models_include.clone(),
                models_exclude: ep_cfg.models_exclude.clone(),
            };

            let entry = EndpointEntry {
                endpoint,
                meta,
                active_requests: 0,
                healthy: false,
                discovered_models: Vec::new(),
            };

            endpoints.insert(ep_cfg.name.clone(), entry);
        }

        Ok(Self {
            endpoints,
            metrics: None,
            model_index: HashMap::new(),
        })
    }

    /// Return the number of configured endpoints.
    pub fn len(&self) -> usize {
        self.endpoints.len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.endpoints.is_empty()
    }

    /// Get an endpoint entry by name.
    pub fn get(&self, name: &str) -> Option<&EndpointEntry> {
        self.endpoints.get(name)
    }

    /// Get a mutable endpoint entry by name.
    pub fn get_mut(&mut self, name: &str) -> Option<&mut EndpointEntry> {
        self.endpoints.get_mut(name)
    }

    /// Whether metrics recording is enabled for this registry.
    pub fn has_metrics(&self) -> bool {
        self.metrics.is_some()
    }

    /// Iterate over all endpoint entries.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &EndpointEntry)> {
        self.endpoints.iter()
    }

    /// Get the current model index snapshot.
    ///
    /// This returns a borrowed view of the internal index mapping
    /// `model_id -> Vec<endpoint_name>`. Callers should treat this as
    /// read-only and rebuild the index via discovery when needed.
    pub fn model_index(&self) -> &HashMap<String, Vec<String>> {
        &self.model_index
    }

    /// Build `NodeCapabilities` from the currently discovered models and
    /// endpoint configuration.
    ///
    /// This flattens all unique model IDs across endpoints and uses simple
    /// heuristics for capacity:
    /// - `endpoint_count`: total configured endpoints.
    /// - `max_concurrent_requests`: sum of per-endpoint `max_concurrent`
    ///   values, ignoring `None` entries.
    pub fn to_node_capabilities(&self) -> NodeCapabilities {
        use std::collections::HashSet;

        let mut unique_models: HashSet<String> = HashSet::new();
        let mut models: Vec<ModelDescriptor> = Vec::new();

        for entry in self.endpoints.values() {
            for model in &entry.discovered_models {
                if unique_models.insert(model.id.clone()) {
                    models.push(model.clone());
                }
            }
        }

        let endpoint_count = self.endpoints.len();

        let max_concurrent_requests = self
            .endpoints
            .values()
            .filter_map(|e| e.meta.max_concurrent)
            .reduce(|acc, v| acc.saturating_add(v));

        let mut caps = NodeCapabilities::new(models, endpoint_count);
        if let Some(max) = max_concurrent_requests {
            caps = caps.with_max_concurrent(max);
        }
        caps
    }

    /// Convert an `EndpointConfig` into a `labman_core::Endpoint`, performing

    /// Convert an `EndpointConfig` into a `labman_core::Endpoint`, performing
    /// minimal validation/normalisation on the base URL.
    fn build_core_endpoint(cfg: &EndpointConfig) -> Result<Endpoint> {
        let base_url = cfg.base_url.trim();

        if base_url.is_empty() {
            return Err(EndpointRegistryError::InvalidEndpointUrl {
                name: cfg.name.clone(),
                reason: "base_url must not be empty".to_string(),
            }
            .into());
        }

        if !(base_url.starts_with("http://") || base_url.starts_with("https://")) {
            return Err(EndpointRegistryError::InvalidEndpointUrl {
                name: cfg.name.clone(),
                reason: "base_url must start with http:// or https://".to_string(),
            }
            .into());
        }

        // For now we assume the caller has validated the `/v1` suffix. Later we
        // can normalise base URLs here if necessary.
        Ok(Endpoint::new(&cfg.name, &base_url.to_string()))
    }

    /// Perform a basic health check for all configured endpoints.
    ///
    /// This synchronous variant is intentionally simple and currently just
    /// marks all endpoints as healthy. It is retained for callers that don't
    /// require HTTP probing.
    pub fn health_check_all(&mut self) -> Result<()> {
        for (name, entry) in self.endpoints.iter_mut() {
            entry.healthy = true;

            if let Some(metrics) = &self.metrics {
                metrics.record_request_end(Some(name.as_str()), None, true, None);
            }
        }

        Ok(())
    }

    /// Perform an HTTP-based health check for all configured endpoints.
    ///
    /// This initial implementation:
    /// - Issues a GET request to `{base_url}` (typically `/v1`).
    /// - Considers 2xx responses as healthy.
    /// - Marks other responses or network errors as unhealthy.
    /// - Emits basic success/failure metrics when a `MetricsRecorder` is present.
    ///
    /// It is async so it can be used from Tokio-based code paths in `labmand`.
    pub async fn health_check_all_http(&mut self) -> Result<()> {
        let client = reqwest::Client::new();

        for (name, entry) in self.endpoints.iter_mut() {
            let url = &entry.endpoint.base_url;
            let resp = client.get(url).send().await;

            match resp {
                Ok(r) if r.status().is_success() => {
                    entry.healthy = true;

                    if let Some(metrics) = &self.metrics {
                        metrics.record_request_end(Some(name.as_str()), None, true, None);
                    }
                }
                Ok(r) => {
                    entry.healthy = false;
                    let status = r.status();
                    tracing::warn!(
                        "endpoint '{}' unhealthy: HTTP {}",
                        entry.endpoint.name,
                        status
                    );

                    if let Some(metrics) = &self.metrics {
                        metrics.record_error(Some(name.as_str()), "health_http_status");
                    }
                }
                Err(e) => {
                    entry.healthy = false;
                    tracing::warn!(
                        "endpoint '{}' unhealthy: request error: {}",
                        entry.endpoint.name,
                        e
                    );

                    if let Some(metrics) = &self.metrics {
                        metrics.record_error(Some(name.as_str()), "health_http_error");
                    }
                }
            }
        }

        Ok(())
    }

    /// Discover models from all healthy endpoints via `/v1/models`.
    ///
    /// For each endpoint:
    /// - Skips if `healthy == false`.
    /// - Issues a GET to `{base_url}/models` or `{base_url}/v1/models` depending
    ///   on whether `base_url` already ends with `/v1`.
    /// - Parses the response into `ModelListResponse`.
    /// - Applies `models_include` / `models_exclude` filters.
    /// - Populates `discovered_models` with the filtered list.
    pub async fn discover_models_all_http(&mut self) -> Result<()> {
        let client = reqwest::Client::new();

        for (name, entry) in self.endpoints.iter_mut() {
            if !entry.healthy {
                tracing::warn!(
                    "skipping model discovery for unhealthy endpoint '{}'",
                    entry.endpoint.name
                );
                continue;
            }

            let base_url = entry.endpoint.base_url.trim_end_matches('/');
            let models_url = if base_url.ends_with("/v1") {
                format!("{}/models", base_url)
            } else {
                format!("{}/v1/models", base_url)
            };

            let resp = client.get(&models_url).send().await;

            let list: ModelListResponse = match resp {
                Ok(r) if r.status().is_success() => match r.json().await {
                    Ok(json) => json,
                    Err(e) => {
                        tracing::warn!(
                            "endpoint '{}' model discovery JSON parse error: {}",
                            entry.endpoint.name,
                            e
                        );
                        if let Some(metrics) = &self.metrics {
                            metrics.record_error(Some(name.as_str()), "model_discovery_parse");
                        }
                        continue;
                    }
                },
                Ok(r) => {
                    tracing::warn!(
                        "endpoint '{}' model discovery HTTP {}",
                        entry.endpoint.name,
                        r.status()
                    );
                    if let Some(metrics) = &self.metrics {
                        metrics.record_error(Some(name.as_str()), "model_discovery_http_status");
                    }
                    continue;
                }
                Err(e) => {
                    tracing::warn!(
                        "endpoint '{}' model discovery request error: {}",
                        entry.endpoint.name,
                        e
                    );
                    if let Some(metrics) = &self.metrics {
                        metrics.record_error(Some(name.as_str()), "model_discovery_error");
                    }
                    continue;
                }
            };

            let mut models = list.data;

            // Apply include filter
            if let Some(include) = &entry.meta.models_include {
                models.retain(|m| {
                    include
                        .iter()
                        .any(|pat| glob_match(pat.as_str(), m.id.as_str()))
                });
            }

            // Apply exclude filter
            if let Some(exclude) = &entry.meta.models_exclude {
                models.retain(|m| {
                    !exclude
                        .iter()
                        .any(|pat| glob_match(pat.as_str(), m.id.as_str()))
                });
            }

            entry.discovered_models = models;

            if let Some(metrics) = &self.metrics {
                metrics.record_request_end(Some(name.as_str()), None, true, None);
            }
        }

        // Rebuild the model index from the newly discovered models.
        self.rebuild_model_index();

        Ok(())
    }

    /// Rebuild the `model_index` from the current `discovered_models` of each
    /// endpoint. This is called after a successful model discovery pass.
    fn rebuild_model_index(&mut self) {
        self.model_index.clear();

        for (endpoint_name, entry) in self.endpoints.iter() {
            for model in &entry.discovered_models {
                let id = model.id.clone();
                self.model_index
                    .entry(id)
                    .or_insert_with(Vec::new)
                    .push(endpoint_name.clone());
            }
        }
    }

    /// Select an endpoint for a given model.
    ///
    /// Current behaviour:
    /// - Looks up the model in `model_index`.
    /// - Filters to endpoints that are currently marked healthy.
    /// - Returns the first matching endpoint entry, if any.
    ///
    /// Future work:
    /// - Integrate `active_requests` and `max_concurrent`.
    /// - Implement better scheduling (round-robin, least-loaded, etc.).
    pub fn select_endpoint_for_model(&self, model_id: &str) -> Option<(&String, &EndpointEntry)> {
        let endpoint_names = self.model_index.get(model_id)?;
        for name in endpoint_names {
            if let Some(entry) = self.endpoints.get(name) {
                if entry.healthy {
                    return self.endpoints.get_key_value(name);
                }
            }
        }
        None
    }

    /// Spawn a periodic HTTP-based health checker and model discovery task.
    ///
    /// This is intended to be called from an async context with a shared
    /// `Arc<tokio::sync::Mutex<EndpointRegistry>>`. It will:
    ///
    /// - Run `health_check_all_http` on the given interval.
    /// - After each successful health pass, run `discover_models_all_http` so
    ///   that model information stays reasonably fresh.
    /// - Log any internal errors but keep the task alive.
    ///
    /// The task will run until the provided `shutdown` future resolves.
    ///
    /// Example usage:
    ///
    /// ```ignore
    /// let registry = Arc::new(tokio::sync::Mutex::new(registry));
    /// let shutdown = shutdown_signal(); // some Future that resolves on shutdown
    /// EndpointRegistry::spawn_periodic_health_check(registry.clone(), Duration::from_secs(30), shutdown);
    /// ```
    pub fn spawn_periodic_health_check<S>(
        registry: Arc<tokio::sync::Mutex<EndpointRegistry>>,
        interval: Duration,
        shutdown: S,
    ) where
        S: std::future::Future<Output = ()> + Send + 'static,
    {
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            tokio::pin!(shutdown);

            loop {
                tokio::select! {
                    _ = ticker.tick() => {
                        let mut guard = registry.lock().await;
                        if let Err(err) = guard.health_check_all_http().await {
                            tracing::warn!("periodic endpoint HTTP health check failed: {}", err);
                            continue;
                        }

                        if let Err(err) = guard.discover_models_all_http().await {
                            tracing::warn!("periodic endpoint model discovery failed: {}", err);
                        }
                    }
                    _ = &mut shutdown => {
                        tracing::info!("stopping periodic endpoint health checker");
                        break;
                    }
                }
            }
        });
    }
}

/// Factory for building an `EndpointRegistry` that is wired with telemetry.
///
/// This can be used by higher-level components (e.g. `labmand`) to create a
/// registry and share a `MetricsRecorder` with it.
pub struct EndpointRegistryBuilder {
    config: LabmanConfig,
    metrics: Option<Arc<dyn MetricsRecorder>>,
}

impl EndpointRegistryBuilder {
    /// Start building a registry from a given configuration.
    pub fn new(config: LabmanConfig) -> Self {
        Self {
            config,
            metrics: None,
        }
    }

    /// Attach a shared `MetricsRecorder` so that the registry and its
    /// background tasks (health checks, model discovery) can emit metrics.
    pub fn with_metrics(mut self, metrics: Arc<dyn MetricsRecorder>) -> Self {
        self.metrics = Some(metrics);
        self
    }

    /// Build the registry.
    ///
    /// For now this populates the metrics recorder (if provided) and delegates
    /// to `EndpointRegistry::from_config`. In future iterations this can:
    /// - Start health/model discovery tasks using the provided metrics.
    /// - Return a richer handle wrapping both the registry and its tasks.
    pub fn build(self) -> Result<EndpointRegistry> {
        let mut registry = EndpointRegistry::from_config(&self.config)?;
        registry.metrics = self.metrics;
        Ok(registry)
    }
}

/// Very small glob matcher for `*` wildcard on the model ID.
///
/// This is intentionally minimal and can be replaced with a more robust
/// implementation later if needed.
fn glob_match(pattern: &str, text: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    // Split on '*' and ensure the segments appear in order.
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == text;
    }

    // Match prefix
    if !text.starts_with(parts[0]) {
        return false;
    }

    let mut remainder = &text[parts[0].len()..];

    // Match middle segments
    for part in &parts[1..parts.len() - 1] {
        if let Some(idx) = remainder.find(part) {
            remainder = &remainder[idx + part.len()..];
        } else {
            return false;
        }
    }

    // Match suffix
    let last = parts.last().unwrap();
    if !last.is_empty() && !remainder.ends_with(last) {
        return false;
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use labman_config::{EndpointConfig, ProxyConfig, TelemetryConfig, WireGuardConfig};
    use labman_core::node::{NodeCapabilities, NodeInfo};

    fn minimal_config() -> LabmanConfig {
        LabmanConfig {
            control_plane: labman_config::ControlPlaneConfig {
                base_url: "https://control.local/api/v1".to_string(),
                node_token: "test-token".to_string(),
                region: Some("test-region".to_string()),
                description: Some("test node".to_string()),
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
            endpoints: vec![],
        }
    }

    #[test]
    fn registry_from_empty_config_is_empty() {
        let cfg = minimal_config();
        let registry = EndpointRegistry::from_config(&cfg).expect("build registry");
        assert_eq!(registry.len(), 0);
        assert!(registry.is_empty());
    }

    #[test]
    fn registry_rejects_duplicate_names() {
        let mut cfg = minimal_config();
        cfg.endpoints = vec![
            EndpointConfig {
                name: "dup".to_string(),
                base_url: "http://127.0.0.1:11434/v1".to_string(),
                max_concurrent: None,
                models_include: None,
                models_exclude: None,
            },
            EndpointConfig {
                name: "dup".to_string(),
                base_url: "http://127.0.0.1:11434/v1".to_string(),
                max_concurrent: None,
                models_include: None,
                models_exclude: None,
            },
        ];

        let res = EndpointRegistry::from_config(&cfg);
        assert!(res.is_err());
    }

    #[test]
    fn registry_builds_single_endpoint() {
        let mut cfg = minimal_config();
        cfg.endpoints = vec![EndpointConfig {
            name: "local-llm".to_string(),
            base_url: "http://127.0.0.1:11434/v1".to_string(),
            max_concurrent: Some(8),
            models_include: Some(vec!["llama*".to_string()]),
            models_exclude: Some(vec!["*test*".to_string()]),
        }];

        let registry = EndpointRegistry::from_config(&cfg).expect("build registry");
        assert_eq!(registry.len(), 1);

        let entry = registry.get("local-llm").expect("endpoint present");
        assert_eq!(entry.endpoint.name, "local-llm");
        assert_eq!(entry.endpoint.base_url, "http://127.0.0.1:11434/v1");
        assert_eq!(entry.meta.max_concurrent, Some(8));
        assert_eq!(
            entry.meta.models_include.as_ref().unwrap(),
            &vec!["llama*".to_string()]
        );
        assert_eq!(
            entry.meta.models_exclude.as_ref().unwrap(),
            &vec!["*test*".to_string()]
        );
    }

    #[test]
    fn labman_config_to_node_info_still_compiles_with_registry_present() {
        let cfg = minimal_config();

        let caps = NodeCapabilities::new(Vec::new(), 0);
        let info: NodeInfo = cfg.to_node_info(caps);

        assert_eq!(info.region.as_deref(), Some("test-region"));
        assert_eq!(info.description.as_deref(), Some("test node"));
    }

    #[test]
    fn glob_match_basic_cases() {
        assert!(glob_match("*", "gpt-4"));
        assert!(glob_match("gpt-4", "gpt-4"));
        assert!(glob_match("gpt-*", "gpt-4"));
        assert!(glob_match("gpt-*", "gpt-3.5"));
        assert!(glob_match("llama*7b", "llama3-7b"));
        assert!(!glob_match("gpt-4", "gpt-3.5"));
        assert!(!glob_match("llama*7b", "llama3-8b"));
    }

    #[test]
    fn to_node_capabilities_flattens_models_and_sums_capacity() {
        let mut cfg = minimal_config();
        cfg.endpoints = vec![
            EndpointConfig {
                name: "ep1".to_string(),
                base_url: "http://127.0.0.1:1111/v1".to_string(),
                max_concurrent: Some(2),
                models_include: None,
                models_exclude: None,
            },
            EndpointConfig {
                name: "ep2".to_string(),
                base_url: "http://127.0.0.1:2222/v1".to_string(),
                max_concurrent: Some(3),
                models_include: None,
                models_exclude: None,
            },
        ];

        let mut registry = EndpointRegistry::from_config(&cfg).expect("build registry");
        {
            let ep1 = registry.get_mut("ep1").unwrap();
            ep1.discovered_models = vec![
                ModelDescriptor::with_details("gpt-4", None, Some("openai".to_string())),
                ModelDescriptor::with_details("gpt-3.5", None, Some("openai".to_string())),
            ];

            let ep2 = registry.get_mut("ep2").unwrap();
            ep2.discovered_models = vec![
                ModelDescriptor::with_details("gpt-4", None, Some("openai".to_string())),
                ModelDescriptor::with_details("llama3", None, Some("meta".to_string())),
            ];
        }

        let caps = registry.to_node_capabilities();
        assert_eq!(caps.endpoint_count, 2);
        assert_eq!(caps.max_concurrent_requests, Some(5));
        // We should see unique models across endpoints.
        let model_ids: std::collections::HashSet<_> =
            caps.models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(model_ids.len(), 3);
        assert!(model_ids.contains("gpt-4"));
        assert!(model_ids.contains("gpt-3.5"));
        assert!(model_ids.contains("llama3"));
    }

    #[test]
    fn rebuild_model_index_and_select_endpoint_for_model_respects_health() {
        let mut cfg = minimal_config();
        cfg.endpoints = vec![
            EndpointConfig {
                name: "healthy-ep".to_string(),
                base_url: "http://127.0.0.1:1111/v1".to_string(),
                max_concurrent: Some(2),
                models_include: None,
                models_exclude: None,
            },
            EndpointConfig {
                name: "unhealthy-ep".to_string(),
                base_url: "http://127.0.0.1:2222/v1".to_string(),
                max_concurrent: Some(2),
                models_include: None,
                models_exclude: None,
            },
        ];

        let mut registry = EndpointRegistry::from_config(&cfg).expect("build registry");
        {
            let healthy = registry.get_mut("healthy-ep").unwrap();
            healthy.discovered_models = vec![ModelDescriptor::new("gpt-4")];
            healthy.healthy = true;

            let unhealthy = registry.get_mut("unhealthy-ep").unwrap();
            unhealthy.discovered_models = vec![ModelDescriptor::new("gpt-4")];
            unhealthy.healthy = false;
        }

        // Rebuild the index after manually adjusting discovered models.
        registry.rebuild_model_index();

        let selected = registry.select_endpoint_for_model("gpt-4");
        assert!(selected.is_some());
        let (name, entry) = selected.unwrap();
        assert_eq!(name.as_str(), "healthy-ep");
        assert!(entry.healthy);

        let none = registry.select_endpoint_for_model("non-existent-model");
        assert!(none.is_none());
    }
}
