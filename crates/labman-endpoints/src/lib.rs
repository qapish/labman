use std::collections::HashMap;
use std::sync::Arc;

use labman_config::{EndpointConfig, LabmanConfig};
use labman_core::endpoint::Endpoint;
use labman_core::{LabmanError, Result};
use labman_telemetry::MetricsRecorder;
use serde::{Deserialize, Serialize};
use thiserror::Error;

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
#[derive(Debug)]
pub struct EndpointRegistry {
    /// Endpoints keyed by logical name.
    endpoints: HashMap<String, EndpointEntry>,
}

/// A single entry in the registry.
#[derive(Debug)]
pub struct EndpointEntry {
    /// The core endpoint representation used throughout the system.
    pub endpoint: Endpoint,

    /// Static configuration metadata (concurrency limits, filters).
    pub meta: EndpointMeta,

    /// Current number of active requests (for scheduling).
    active_requests: usize,
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
            };

            endpoints.insert(ep_cfg.name.clone(), entry);
        }

        Ok(Self { endpoints })
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

    /// Iterate over all endpoint entries.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &EndpointEntry)> {
        self.endpoints.iter()
    }

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
    /// For now this simply delegates to `EndpointRegistry::from_config`. In
    /// future iterations this can:
    /// - Start health/model discovery tasks using the provided metrics.
    /// - Return a richer handle wrapping both the registry and its tasks.
    pub fn build(self) -> Result<EndpointRegistry> {
        let _maybe_metrics = self.metrics;
        EndpointRegistry::from_config(&self.config)
    }
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
}
