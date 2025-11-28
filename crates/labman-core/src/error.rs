//! Error types for labman.
//!
//! This module defines all error types that can occur throughout the labman system.

/// The main error type for labman operations.
#[derive(Debug, thiserror::Error)]
pub enum LabmanError {
    /// Configuration-related errors
    #[error("Configuration error: {0}")]
    Config(String),

    /// Invalid configuration value
    #[error("Invalid configuration for '{field}': {message}")]
    InvalidConfig { field: String, message: String },

    /// Configuration file not found
    #[error("Configuration file not found at path: {0}")]
    ConfigNotFound(String),

    /// WireGuard interface errors
    #[error("WireGuard error: {0}")]
    WireGuard(String),

    /// Rosenpass (post-quantum) errors
    #[error("Rosenpass error: {0}")]
    Rosenpass(String),

    /// Network interface errors
    #[error("Network interface error: {0}")]
    NetworkInterface(String),

    /// Endpoint communication errors
    #[error("Endpoint '{endpoint}' error: {message}")]
    Endpoint { endpoint: String, message: String },

    /// Endpoint not found
    #[error("Endpoint not found: {0}")]
    EndpointNotFound(String),

    /// Endpoint unhealthy
    #[error("Endpoint '{0}' is unhealthy")]
    EndpointUnhealthy(String),

    /// Model not found on any endpoint
    #[error("Model '{0}' not found on any healthy endpoint")]
    ModelNotFound(String),

    /// Model discovery failed
    #[error("Failed to discover models from endpoint '{endpoint}': {message}")]
    ModelDiscovery { endpoint: String, message: String },

    /// HTTP request errors
    #[error("HTTP request failed: {0}")]
    Http(String),

    /// HTTP client errors (wraps reqwest errors)
    #[error("HTTP client error: {0}")]
    HttpClient(#[from] reqwest::Error),

    /// Request timeout
    #[error("Request timed out after {0}s")]
    Timeout(u64),

    /// Invalid HTTP request
    #[error("Invalid request: {0}")]
    InvalidRequest(String),

    /// Invalid HTTP response from endpoint
    #[error("Invalid response from endpoint '{endpoint}': {message}")]
    InvalidResponse { endpoint: String, message: String },

    /// Proxy errors
    #[error("Proxy error: {0}")]
    Proxy(String),

    /// Streaming response error
    #[error("Streaming error: {0}")]
    Streaming(String),

    /// Control plane communication errors
    #[error("Control plane error: {0}")]
    ControlPlane(String),

    /// Control plane authentication failed
    #[error("Control plane authentication failed: {0}")]
    Authentication(String),

    /// Node registration failed
    #[error("Node registration failed: {0}")]
    Registration(String),

    /// Heartbeat failed
    #[error("Heartbeat failed: {0}")]
    Heartbeat(String),

    /// Serialization/deserialization errors
    #[error("Serialization error: {0}")]
    Serialization(String),

    /// JSON serialization errors (wraps serde_json errors)
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// TOML parsing errors (for when we parse TOML in config)
    #[error("TOML error: {0}")]
    Toml(String),

    /// I/O errors
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// File system errors
    #[error("File system error: {0}")]
    FileSystem(String),

    /// Permission denied
    #[error("Permission denied: {0}")]
    PermissionDenied(String),

    /// Resource not available
    #[error("Resource not available: {0}")]
    ResourceUnavailable(String),

    /// Operation not supported
    #[error("Operation not supported: {0}")]
    Unsupported(String),

    /// Invalid state
    #[error("Invalid state: {0}")]
    InvalidState(String),

    /// Concurrent operation limit reached
    #[error("Concurrent operation limit reached for endpoint '{0}'")]
    ConcurrencyLimitReached(String),

    /// Shutdown signal received
    #[error("Shutdown signal received")]
    Shutdown,

    /// Generic internal error
    #[error("Internal error: {0}")]
    Internal(String),
}

impl LabmanError {
    /// Create a config error with a message
    pub fn config<S: Into<String>>(message: S) -> Self {
        Self::Config(message.into())
    }

    /// Create an invalid config error
    pub fn invalid_config<S: Into<String>>(field: S, message: S) -> Self {
        Self::InvalidConfig {
            field: field.into(),
            message: message.into(),
        }
    }

    /// Create a WireGuard error
    pub fn wireguard<S: Into<String>>(message: S) -> Self {
        Self::WireGuard(message.into())
    }

    /// Create a Rosenpass error
    pub fn rosenpass<S: Into<String>>(message: S) -> Self {
        Self::Rosenpass(message.into())
    }

    /// Create an endpoint error
    pub fn endpoint<S: Into<String>>(endpoint: S, message: S) -> Self {
        Self::Endpoint {
            endpoint: endpoint.into(),
            message: message.into(),
        }
    }

    /// Create a model discovery error
    pub fn model_discovery<S: Into<String>>(endpoint: S, message: S) -> Self {
        Self::ModelDiscovery {
            endpoint: endpoint.into(),
            message: message.into(),
        }
    }

    /// Create an invalid response error
    pub fn invalid_response<S: Into<String>>(endpoint: S, message: S) -> Self {
        Self::InvalidResponse {
            endpoint: endpoint.into(),
            message: message.into(),
        }
    }

    /// Check if this error is transient (retryable)
    pub fn is_transient(&self) -> bool {
        matches!(
            self,
            Self::Http(_)
                | Self::HttpClient(_)
                | Self::Timeout(_)
                | Self::EndpointUnhealthy(_)
                | Self::Heartbeat(_)
                | Self::ResourceUnavailable(_)
        )
    }

    /// Check if this error is fatal (should stop the daemon)
    pub fn is_fatal(&self) -> bool {
        matches!(
            self,
            Self::ConfigNotFound(_)
                | Self::PermissionDenied(_)
                | Self::Shutdown
                | Self::Authentication(_)
        )
    }
}

/// Result type alias for labman operations
pub type Result<T> = std::result::Result<T, LabmanError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = LabmanError::config("test message");
        assert_eq!(err.to_string(), "Configuration error: test message");
    }

    #[test]
    fn test_invalid_config_error() {
        let err = LabmanError::invalid_config("test_field", "test message");
        assert_eq!(
            err.to_string(),
            "Invalid configuration for 'test_field': test message"
        );
    }

    #[test]
    fn test_endpoint_error() {
        let err = LabmanError::endpoint("test-endpoint", "connection failed");
        assert_eq!(
            err.to_string(),
            "Endpoint 'test-endpoint' error: connection failed"
        );
    }

    #[test]
    fn test_transient_errors() {
        assert!(LabmanError::Timeout(30).is_transient());
        assert!(LabmanError::Http("test".into()).is_transient());
        assert!(!LabmanError::ConfigNotFound("test".into()).is_transient());
    }

    #[test]
    fn test_fatal_errors() {
        assert!(LabmanError::Shutdown.is_fatal());
        assert!(LabmanError::PermissionDenied("test".into()).is_fatal());
        assert!(!LabmanError::Timeout(30).is_fatal());
    }
}
