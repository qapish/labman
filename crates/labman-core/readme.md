# labman-core

Core types, errors, and shared functionality for labman.

## Overview

`labman-core` is the foundational crate for the labman distributed inference network daemon. It provides shared types, comprehensive error handling, and domain models used throughout the labman system.

This crate is designed to be a minimal, dependency-light foundation that all other labman crates depend on. It contains no business logic—only type definitions, serialization support, and error handling.

## What's Included

### Error Types (`error` module)

- **`LabmanError`**: Comprehensive error enum covering all failure modes in labman
- **`Result<T>`**: Type alias for `std::result::Result<T, LabmanError>`
- Helper methods for creating and classifying errors (transient, fatal, etc.)

### Endpoint Types (`endpoint` module)

- **`Endpoint`**: Represents an LLM inference endpoint (Ollama, vLLM, llama.cpp, etc.)
- **`EndpointHealth`**: Health status tracking (Healthy, Unhealthy, Unknown)
- **`ModelDescriptor`**: Model information from OpenAI `/v1/models` API
- **`ModelListResponse`**: OpenAI-compatible model list response format

### Node Types (`node` module)

- **`NodeInfo`**: Node identity and capabilities for registration
- **`NodeCapabilities`**: Models and features this node provides
- **`NodeStatus`**: Current operational status for heartbeats
- **`NodeState`**: State machine for node lifecycle
- **`RegistrationRequest/Response`**: Control plane registration protocol
- **`HeartbeatRequest/Response`**: Control plane heartbeat protocol

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
labman-core = { path = "../labman-core" }
```

Or use the prelude for convenient imports:

```rust
use labman_core::prelude::*;

fn main() -> Result<()> {
    let endpoint = Endpoint::new("ollama-local", "http://127.0.0.1:11434/v1");
    println!("Created endpoint: {}", endpoint.name);
    Ok(())
}
```

## Examples

### Error Handling

```rust
use labman_core::{Result, LabmanError};

fn connect_to_endpoint(url: &str) -> Result<String> {
    if url.is_empty() {
        return Err(LabmanError::InvalidRequest("URL cannot be empty".into()));
    }
    Ok("Connected".into())
}

match connect_to_endpoint("") {
    Ok(_) => println!("Success"),
    Err(e) if e.is_transient() => println!("Temporary error, retry: {}", e),
    Err(e) if e.is_fatal() => println!("Fatal error, shutdown: {}", e),
    Err(e) => println!("Error: {}", e),
}
```

### Endpoint Management

```rust
use labman_core::endpoint::{Endpoint, ModelDescriptor};

let mut endpoint = Endpoint::new("vllm-gpu", "http://192.168.1.42:8000/v1");

// Mark as healthy
endpoint.mark_healthy();
assert!(endpoint.is_healthy());

// Add discovered models
let models = vec![
    ModelDescriptor::new("llama-3.2-3b"),
    ModelDescriptor::new("mixtral-8x7b"),
];
endpoint.update_models(models);

// Check for specific model
if endpoint.has_model("llama-3.2-3b") {
    println!("Endpoint supports llama-3.2-3b");
}
```

### Node Registration

```rust
use labman_core::node::{NodeInfo, NodeCapabilities, RegistrationRequest};
use labman_core::endpoint::ModelDescriptor;

let models = vec![
    ModelDescriptor::new("llama-3.2-3b"),
    ModelDescriptor::new("mixtral-8x7b"),
];

let capabilities = NodeCapabilities::new(models, 2)
    .with_max_concurrent(16);

let node_info = NodeInfo::new("homelab-001", capabilities)
    .with_region("us-west")
    .with_description("Living room GPU server");

let registration = RegistrationRequest {
    token: "secret-token".into(),
    node_info,
    wireguard_public_key: "wg-pub-key".into(),
    rosenpass_public_key: "rp-pub-key".into(),
};

// Send to control plane...
```

### Status Reporting

```rust
use labman_core::node::{NodeStatus, NodeState};

let mut status = NodeStatus::running("homelab-001", 3, 4);
status.active_requests = 5;
status.total_requests = 1234;
status.uptime_seconds = 86400; // 1 day

if !status.is_healthy() {
    status.set_error("All endpoints unhealthy");
}
```

## Module Structure

```
labman-core/
├── error.rs          # Error types and Result alias
├── endpoint.rs       # Endpoint, health, and model types
├── node.rs           # Node identity, capabilities, and status
└── lib.rs            # Public API and prelude
```

## Design Principles

### 1. Zero Business Logic

This crate contains only data structures and type definitions. All business logic lives in other crates (`labman-endpoints`, `labman-proxy`, etc.).

### 2. Serialization-Ready

All types implement `Serialize` and `Deserialize` for communication with:
- Control plane APIs (JSON over HTTPS)
- Configuration files (TOML)
- Inter-process communication

### 3. Comprehensive Error Coverage

`LabmanError` covers all failure modes across the entire labman system, making it easy to propagate errors between crates without type conversion.

### 4. Self-Documenting

Types include extensive documentation and derive `Debug` for troubleshooting. All public APIs have doc comments with examples.

### 5. Testable

Every module includes comprehensive unit tests. Types are designed to be easy to construct for testing.

## Dependencies

Minimal dependencies for maximum portability:

- **thiserror**: Ergonomic error handling
- **serde**: Serialization framework
- **serde_json**: JSON support
- **chrono**: Time and date handling
- **reqwest**: Only for error type compatibility (no actual HTTP in this crate)

## Testing

Run tests:

```bash
cargo test -p labman-core
```

Run tests with coverage:

```bash
cargo test -p labman-core -- --nocapture
```

Check for warnings:

```bash
cargo clippy -p labman-core -- -D warnings
```

Generate documentation:

```bash
cargo doc -p labman-core --open
```

## Version

Current version: `0.0.1` (pre-alpha)

This crate follows semantic versioning. Until 1.0, breaking changes may occur between minor versions.

## License

MIT License - see repository root for details.

## Contributing

This crate is part of the labman project. See the main repository README for contribution guidelines.

When adding new types:
1. Add comprehensive doc comments
2. Implement `Serialize` and `Deserialize`
3. Add unit tests
4. Update this README if adding a new module