//! Core types, errors, and shared functionality for labman.
//!
//! This crate provides the foundational types used throughout the labman system:
//!
//! - **Error types**: Comprehensive error handling with [`LabmanError`] and [`Result`]
//! - **Endpoint types**: Representation of LLM endpoints, health tracking, and model discovery
//! - **Node types**: Node identity, capabilities, and status reporting for control plane communication
//!
//! # Overview
//!
//! labman-core is designed as a dependency-free foundation (aside from serialization and time handling)
//! that all other labman crates depend on. It defines the core domain model and error types without
//! implementing any business logic.
//!
//! # Examples
//!
//! ## Creating an endpoint
//!
//! ```rust
//! use labman_core::endpoint::Endpoint;
//!
//! let mut endpoint = Endpoint::new("ollama-local", "http://127.0.0.1:11434/v1");
//! endpoint.mark_healthy();
//! assert!(endpoint.is_healthy());
//! ```
//!
//! ## Working with node info
//!
//! ```rust
//! use labman_core::node::{NodeInfo, NodeCapabilities};
//!
//! let capabilities = NodeCapabilities::new(vec![], 2);
//! let node_info = NodeInfo::new("node-001", capabilities)
//!     .with_region("us-west")
//!     .with_description("Home GPU server");
//!
//! assert_eq!(node_info.id, "node-001");
//! ```
//!
//! ## Error handling
//!
//! ```rust
//! use labman_core::{Result, LabmanError};
//!
//! fn example_operation() -> Result<String> {
//!     Err(LabmanError::config("invalid configuration"))
//! }
//!
//! match example_operation() {
//!     Ok(val) => println!("Success: {}", val),
//!     Err(e) => println!("Error: {}", e),
//! }
//! ```

pub mod endpoint;
pub mod error;
pub mod node;

// Re-export commonly used types for convenience
pub use endpoint::{Endpoint, EndpointHealth, ModelDescriptor, ModelListResponse};
pub use error::{LabmanError, Result};
pub use node::{
    HeartbeatRequest, HeartbeatResponse, NodeCapabilities, NodeInfo, NodeState, NodeStatus,
    RegistrationRequest, RegistrationResponse,
};

/// Prelude module for convenient imports.
///
/// This module re-exports the most commonly used types so they can be
/// imported with a single glob import:
///
/// ```rust
/// use labman_core::prelude::*;
/// ```
pub mod prelude {
    pub use crate::endpoint::{Endpoint, EndpointHealth, ModelDescriptor};
    pub use crate::error::{LabmanError, Result};
    pub use crate::node::{NodeCapabilities, NodeInfo, NodeState, NodeStatus};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prelude_imports() {
        use crate::prelude::*;

        // Verify we can use types from prelude
        let _endpoint = Endpoint::new("test", "http://localhost");
        let _error = LabmanError::config("test");
        let _state = NodeState::Running;
    }

    #[test]
    fn test_result_type() {
        fn returns_result() -> Result<i32> {
            Ok(42)
        }

        assert_eq!(returns_result().unwrap(), 42);
    }
}
