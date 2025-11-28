//! Endpoint types and health tracking.
//!
//! This module defines types for representing LLM endpoints, their health status,
//! and the models they provide.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

/// An LLM inference endpoint.
///
/// Represents a single OpenAI-compatible API endpoint (Ollama, vLLM, llama.cpp, etc.)
/// that labman can proxy requests to.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Endpoint {
    /// Unique name for this endpoint
    pub name: String,

    /// Base URL for the endpoint (e.g., "http://127.0.0.1:11434/v1")
    pub base_url: String,

    /// Current health status
    pub health: EndpointHealth,

    /// Models discovered from this endpoint's /v1/models API
    pub models: Vec<ModelDescriptor>,

    /// Last time this endpoint was checked
    pub last_checked: Option<DateTime<Utc>>,

    /// Last time this endpoint was successfully contacted
    pub last_success: Option<DateTime<Utc>>,

    /// Number of consecutive health check failures
    pub consecutive_failures: u32,
}

impl Endpoint {
    /// Create a new endpoint
    pub fn new<S: Into<String>>(name: S, base_url: S) -> Self {
        Self {
            name: name.into(),
            base_url: base_url.into(),
            health: EndpointHealth::Unknown,
            models: Vec::new(),
            last_checked: None,
            last_success: None,
            consecutive_failures: 0,
        }
    }

    /// Check if this endpoint is healthy
    pub fn is_healthy(&self) -> bool {
        matches!(self.health, EndpointHealth::Healthy)
    }

    /// Check if this endpoint provides a specific model
    pub fn has_model(&self, model_name: &str) -> bool {
        self.models.iter().any(|m| m.id == model_name)
    }

    /// Mark endpoint as healthy
    pub fn mark_healthy(&mut self) {
        self.health = EndpointHealth::Healthy;
        self.last_success = Some(Utc::now());
        self.consecutive_failures = 0;
    }

    /// Mark endpoint as unhealthy with a reason
    pub fn mark_unhealthy(&mut self, reason: String) {
        self.health = EndpointHealth::Unhealthy { reason };
        self.consecutive_failures += 1;
    }

    /// Update models from discovery
    pub fn update_models(&mut self, models: Vec<ModelDescriptor>) {
        self.models = models;
    }

    /// Get the number of models available on this endpoint
    pub fn model_count(&self) -> usize {
        self.models.len()
    }
}

/// Health status of an endpoint.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum EndpointHealth {
    /// Endpoint is healthy and responding
    Healthy,

    /// Endpoint is unhealthy or unreachable
    Unhealthy {
        /// Reason for unhealthy status
        reason: String,
    },

    /// Health status is unknown (not yet checked)
    Unknown,
}

impl fmt::Display for EndpointHealth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Healthy => write!(f, "healthy"),
            Self::Unhealthy { reason } => write!(f, "unhealthy: {}", reason),
            Self::Unknown => write!(f, "unknown"),
        }
    }
}

/// Descriptor for a model available on an endpoint.
///
/// This represents a model as returned by the OpenAI /v1/models API.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelDescriptor {
    /// Model identifier (e.g., "llama3.2:3b", "gpt-4")
    pub id: String,

    /// Unix timestamp when this model was created (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created: Option<i64>,

    /// Model owner/organization (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owned_by: Option<String>,

    /// Additional metadata about the model (optional)
    #[serde(flatten)]
    pub metadata: serde_json::Value,
}

impl ModelDescriptor {
    /// Create a new model descriptor
    pub fn new<S: Into<String>>(id: S) -> Self {
        Self {
            id: id.into(),
            created: None,
            owned_by: None,
            metadata: serde_json::Value::Null,
        }
    }

    /// Create a model descriptor with full information
    pub fn with_details<S: Into<String>>(
        id: S,
        created: Option<i64>,
        owned_by: Option<String>,
    ) -> Self {
        Self {
            id: id.into(),
            created,
            owned_by,
            metadata: serde_json::Value::Null,
        }
    }
}

/// OpenAI-compatible model list response.
///
/// This is the expected format from GET /v1/models endpoints.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelListResponse {
    /// Object type (should be "list")
    pub object: String,

    /// List of available models
    pub data: Vec<ModelDescriptor>,
}

impl ModelListResponse {
    /// Create a new model list response
    pub fn new(models: Vec<ModelDescriptor>) -> Self {
        Self {
            object: "list".to_string(),
            data: models,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_endpoint_creation() {
        let endpoint = Endpoint::new("test", "http://localhost:8000/v1");
        assert_eq!(endpoint.name, "test");
        assert_eq!(endpoint.base_url, "http://localhost:8000/v1");
        assert_eq!(endpoint.health, EndpointHealth::Unknown);
        assert_eq!(endpoint.models.len(), 0);
    }

    #[test]
    fn test_endpoint_health_tracking() {
        let mut endpoint = Endpoint::new("test", "http://localhost:8000/v1");

        // Initially unknown
        assert!(!endpoint.is_healthy());

        // Mark healthy
        endpoint.mark_healthy();
        assert!(endpoint.is_healthy());
        assert_eq!(endpoint.consecutive_failures, 0);
        assert!(endpoint.last_success.is_some());

        // Mark unhealthy
        endpoint.mark_unhealthy("connection failed".to_string());
        assert!(!endpoint.is_healthy());
        assert_eq!(endpoint.consecutive_failures, 1);

        // Multiple failures
        endpoint.mark_unhealthy("timeout".to_string());
        assert_eq!(endpoint.consecutive_failures, 2);

        // Recover
        endpoint.mark_healthy();
        assert_eq!(endpoint.consecutive_failures, 0);
    }

    #[test]
    fn test_endpoint_has_model() {
        let mut endpoint = Endpoint::new("test", "http://localhost:8000/v1");

        let models = vec![
            ModelDescriptor::new("llama3.2:3b"),
            ModelDescriptor::new("mixtral:8x7b"),
        ];

        endpoint.update_models(models);

        assert!(endpoint.has_model("llama3.2:3b"));
        assert!(endpoint.has_model("mixtral:8x7b"));
        assert!(!endpoint.has_model("gpt-4"));
        assert_eq!(endpoint.model_count(), 2);
    }

    #[test]
    fn test_endpoint_health_display() {
        let healthy = EndpointHealth::Healthy;
        assert_eq!(healthy.to_string(), "healthy");

        let unhealthy = EndpointHealth::Unhealthy {
            reason: "timeout".to_string(),
        };
        assert_eq!(unhealthy.to_string(), "unhealthy: timeout");

        let unknown = EndpointHealth::Unknown;
        assert_eq!(unknown.to_string(), "unknown");
    }

    #[test]
    fn test_model_descriptor_serialization() {
        let model = ModelDescriptor::new("test-model");
        let json = serde_json::to_string(&model).unwrap();
        assert!(json.contains("test-model"));

        let deserialized: ModelDescriptor = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.id, "test-model");
    }

    #[test]
    fn test_model_list_response() {
        let models = vec![
            ModelDescriptor::new("model-1"),
            ModelDescriptor::new("model-2"),
        ];

        let response = ModelListResponse::new(models);
        assert_eq!(response.object, "list");
        assert_eq!(response.data.len(), 2);

        // Test serialization
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("list"));
        assert!(json.contains("model-1"));
    }

    #[test]
    fn test_endpoint_serialization() {
        let endpoint = Endpoint::new("test", "http://localhost:8000/v1");
        let json = serde_json::to_string(&endpoint).unwrap();
        let deserialized: Endpoint = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.name, endpoint.name);
        assert_eq!(deserialized.base_url, endpoint.base_url);
    }
}
