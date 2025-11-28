//! Node identity, capabilities, and status types.
//!
//! This module defines types for representing a labman node's identity,
//! capabilities, and operational status for registration and heartbeat with
//! the control plane.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::endpoint::ModelDescriptor;

/// Node identity and capabilities.
///
/// This represents the node's information sent during registration and
/// capability sync with the control plane.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NodeInfo {
    /// Unique node identifier (provided by control plane)
    pub id: String,

    /// Optional region or datacenter identifier
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,

    /// Optional human-readable description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Node capabilities
    pub capabilities: NodeCapabilities,

    /// When this node was first registered
    #[serde(skip_serializing_if = "Option::is_none")]
    pub registered_at: Option<DateTime<Utc>>,

    /// labman version running on this node
    pub version: String,
}

impl NodeInfo {
    /// Create a new node info
    pub fn new<S: Into<String>>(id: S, capabilities: NodeCapabilities) -> Self {
        Self {
            id: id.into(),
            region: None,
            description: None,
            capabilities,
            registered_at: None,
            version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }

    /// Set the region
    pub fn with_region<S: Into<String>>(mut self, region: S) -> Self {
        self.region = Some(region.into());
        self
    }

    /// Set the description
    pub fn with_description<S: Into<String>>(mut self, description: S) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Set the registration timestamp
    pub fn with_registered_at(mut self, timestamp: DateTime<Utc>) -> Self {
        self.registered_at = Some(timestamp);
        self
    }
}

/// Node capabilities and available models.
///
/// Represents what models and features this node can provide.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NodeCapabilities {
    /// All models available across all endpoints
    pub models: Vec<ModelDescriptor>,

    /// Number of configured endpoints
    pub endpoint_count: usize,

    /// Estimated total concurrent request capacity
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_concurrent_requests: Option<usize>,

    /// Whether streaming is supported
    #[serde(default = "default_true")]
    pub supports_streaming: bool,

    /// Whether chat completions are supported
    #[serde(default = "default_true")]
    pub supports_chat: bool,

    /// Whether text completions are supported
    #[serde(default = "default_true")]
    pub supports_completions: bool,

    /// Additional metadata
    #[serde(flatten)]
    pub metadata: HashMap<String, serde_json::Value>,
}

fn default_true() -> bool {
    true
}

impl NodeCapabilities {
    /// Create new capabilities with models
    pub fn new(models: Vec<ModelDescriptor>, endpoint_count: usize) -> Self {
        Self {
            models,
            endpoint_count,
            max_concurrent_requests: None,
            supports_streaming: true,
            supports_chat: true,
            supports_completions: true,
            metadata: HashMap::new(),
        }
    }

    /// Set maximum concurrent requests
    pub fn with_max_concurrent(mut self, max: usize) -> Self {
        self.max_concurrent_requests = Some(max);
        self
    }

    /// Add custom metadata
    pub fn with_metadata<S: Into<String>>(mut self, key: S, value: serde_json::Value) -> Self {
        self.metadata.insert(key.into(), value);
        self
    }

    /// Get the number of unique models available
    pub fn model_count(&self) -> usize {
        self.models.len()
    }
}

/// Current operational status of a node.
///
/// Sent periodically to the control plane as heartbeat.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NodeStatus {
    /// Node identifier
    pub node_id: String,

    /// Current operational state
    pub state: NodeState,

    /// Number of healthy endpoints
    pub healthy_endpoints: usize,

    /// Total number of configured endpoints
    pub total_endpoints: usize,

    /// Number of currently active requests being proxied
    pub active_requests: usize,

    /// Total requests processed since startup
    pub total_requests: u64,

    /// Total errors encountered since startup
    pub total_errors: u64,

    /// System uptime in seconds
    pub uptime_seconds: u64,

    /// Timestamp of this status report
    pub timestamp: DateTime<Utc>,

    /// Optional error message if state is Error
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

impl NodeStatus {
    /// Create a new node status
    pub fn new<S: Into<String>>(node_id: S) -> Self {
        Self {
            node_id: node_id.into(),
            state: NodeState::Starting,
            healthy_endpoints: 0,
            total_endpoints: 0,
            active_requests: 0,
            total_requests: 0,
            total_errors: 0,
            uptime_seconds: 0,
            timestamp: Utc::now(),
            error_message: None,
        }
    }

    /// Create a running status
    pub fn running<S: Into<String>>(
        node_id: S,
        healthy_endpoints: usize,
        total_endpoints: usize,
    ) -> Self {
        Self {
            node_id: node_id.into(),
            state: NodeState::Running,
            healthy_endpoints,
            total_endpoints,
            active_requests: 0,
            total_requests: 0,
            total_errors: 0,
            uptime_seconds: 0,
            timestamp: Utc::now(),
            error_message: None,
        }
    }

    /// Update timestamp to now
    pub fn update_timestamp(&mut self) {
        self.timestamp = Utc::now();
    }

    /// Set error state with message
    pub fn set_error<S: Into<String>>(&mut self, message: S) {
        self.state = NodeState::Error;
        self.error_message = Some(message.into());
    }

    /// Check if node is healthy
    pub fn is_healthy(&self) -> bool {
        matches!(self.state, NodeState::Running) && self.healthy_endpoints > 0
    }
}

/// Operational state of a node.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum NodeState {
    /// Node is starting up
    Starting,

    /// Node is running and healthy
    Running,

    /// Node is running but degraded (some endpoints unhealthy)
    Degraded,

    /// Node is in maintenance mode
    Maintenance,

    /// Node encountered an error
    Error,

    /// Node is shutting down
    Stopping,
}

impl std::fmt::Display for NodeState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Starting => write!(f, "starting"),
            Self::Running => write!(f, "running"),
            Self::Degraded => write!(f, "degraded"),
            Self::Maintenance => write!(f, "maintenance"),
            Self::Error => write!(f, "error"),
            Self::Stopping => write!(f, "stopping"),
        }
    }
}

/// Node registration request.
///
/// Sent to the control plane to register a new node or update its information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistrationRequest {
    /// Node authentication token
    pub token: String,

    /// Node information
    pub node_info: NodeInfo,

    /// WireGuard public key
    pub wireguard_public_key: String,

    /// Rosenpass public key (post-quantum)
    pub rosenpass_public_key: String,
}

/// Node registration response from control plane.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistrationResponse {
    /// Whether registration was successful
    pub success: bool,

    /// Node ID assigned by control plane
    pub node_id: String,

    /// Optional message
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,

    /// Assigned WireGuard IP address
    pub wireguard_address: String,
}

/// Heartbeat request sent to control plane.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatRequest {
    /// Node ID
    pub node_id: String,

    /// Current status
    pub status: NodeStatus,

    /// Updated capabilities (if changed)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<NodeCapabilities>,
}

/// Heartbeat response from control plane.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatResponse {
    /// Whether heartbeat was accepted
    pub success: bool,

    /// Optional message or instructions
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,

    /// Requested node state change (e.g., maintenance mode)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested_state: Option<NodeState>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_node_info_creation() {
        let capabilities = NodeCapabilities::new(vec![], 0);
        let info = NodeInfo::new("test-node", capabilities)
            .with_region("us-west")
            .with_description("Test node");

        assert_eq!(info.id, "test-node");
        assert_eq!(info.region, Some("us-west".to_string()));
        assert_eq!(info.description, Some("Test node".to_string()));
    }

    #[test]
    fn test_node_capabilities() {
        let models = vec![
            crate::endpoint::ModelDescriptor::new("llama3.2"),
            crate::endpoint::ModelDescriptor::new("mixtral"),
        ];

        let capabilities = NodeCapabilities::new(models, 2)
            .with_max_concurrent(16)
            .with_metadata("gpu_count", serde_json::json!(2));

        assert_eq!(capabilities.model_count(), 2);
        assert_eq!(capabilities.endpoint_count, 2);
        assert_eq!(capabilities.max_concurrent_requests, Some(16));
        assert!(capabilities.supports_streaming);
    }

    #[test]
    fn test_node_status() {
        let mut status = NodeStatus::new("test-node");
        assert_eq!(status.state, NodeState::Starting);
        assert!(!status.is_healthy());

        status.state = NodeState::Running;
        status.healthy_endpoints = 1;
        assert!(status.is_healthy());

        status.set_error("test error");
        assert_eq!(status.state, NodeState::Error);
        assert!(!status.is_healthy());
        assert_eq!(status.error_message, Some("test error".to_string()));
    }

    #[test]
    fn test_node_state_display() {
        assert_eq!(NodeState::Starting.to_string(), "starting");
        assert_eq!(NodeState::Running.to_string(), "running");
        assert_eq!(NodeState::Error.to_string(), "error");
    }

    #[test]
    fn test_node_status_running() {
        let status = NodeStatus::running("test-node", 3, 4);
        assert_eq!(status.state, NodeState::Running);
        assert_eq!(status.healthy_endpoints, 3);
        assert_eq!(status.total_endpoints, 4);
    }

    #[test]
    fn test_registration_request_serialization() {
        let capabilities = NodeCapabilities::new(vec![], 0);
        let info = NodeInfo::new("test-node", capabilities);
        let request = RegistrationRequest {
            token: "secret".to_string(),
            node_info: info,
            wireguard_public_key: "wg-pub-key".to_string(),
            rosenpass_public_key: "rp-pub-key".to_string(),
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("secret"));
        assert!(json.contains("wg-pub-key"));
    }

    #[test]
    fn test_heartbeat_request_serialization() {
        let status = NodeStatus::running("test-node", 2, 2);
        let request = HeartbeatRequest {
            node_id: "test-node".to_string(),
            status,
            capabilities: None,
        };

        let json = serde_json::to_string(&request).unwrap();
        let deserialized: HeartbeatRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.node_id, "test-node");
    }
}
