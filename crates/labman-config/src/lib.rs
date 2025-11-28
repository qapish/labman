//! Configuration loading and types for labman.
//!
//! This crate is responsible for:
//! - Defining the top-level configuration model used by the daemon and other crates
//! - Loading configuration from TOML files
//! - Providing a simple default search strategy (e.g. /etc/labman/labman.toml, ./labman.toml)
//!
//! The goal is to keep this crate focused on configuration concerns and to avoid
//! pulling in heavy runtime dependencies. Business logic and orchestration live
//! in higher-level crates.
//!
//! The configuration model here is intentionally more constrained than the
//! example `labman.example.toml` file: some fields in the example are
//! control‑plane details that will eventually be driven by registration
//! protocols. This crate focuses on the subset of configuration that should be
//! static and operator‑managed.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use labman_core::{LabmanError, Result};

/// Root configuration struct for labman.
///
/// This represents the operator‑supplied configuration that the daemon
/// and related crates consume.
#[derive(Debug, Clone, Deserialize)]
pub struct LabmanConfig {
    /// Control‑plane connectivity and identity configuration.
    pub control_plane: ControlPlaneConfig,

    /// WireGuard + Rosenpass configuration for the control‑plane tunnel.
    pub wireguard: WireGuardConfig,

    /// Proxy configuration for the local HTTP interface exposed over the tunnel.
    pub proxy: ProxyConfig,

    /// Logical LLM endpoints this node can use.
    #[serde(default)]
    pub endpoints: Vec<EndpointConfig>,
}

/// Control‑plane configuration section.
#[derive(Debug, Clone, Deserialize)]
pub struct ControlPlaneConfig {
    /// Base URL of the control‑plane API, e.g. `https://control.example.com/api/v1`.
    pub base_url: String,

    /// Node authentication token used when talking to the control plane.
    pub node_token: String,

    /// Optional region identifier (datacenter, cloud region, campus, etc.).
    #[serde(default)]
    pub region: Option<String>,

    /// Optional human‑readable description of this node.
    #[serde(default)]
    pub description: Option<String>,
}

/// WireGuard and Rosenpass configuration.
///
/// These fields describe how this node should establish a secure tunnel
/// towards the control plane. Some values may be refined or replaced
/// once control‑plane registration is implemented.
#[derive(Debug, Clone, Deserialize)]
pub struct WireGuardConfig {
    /// Interface name (default: `labman0`).
    ///
    /// In TOML this is `interface_name`.
    #[serde(default = "default_interface_name")]
    pub interface_name: String,

    /// Local WireGuard address (CIDR, e.g. `10.90.1.2/32`).
    ///
    /// This may eventually be provided by the control plane, but for now we
    /// allow it to be configured explicitly.
    #[serde(default)]
    pub address: Option<String>,

    /// Path to the node's WireGuard private key.
    ///
    /// The corresponding public key may be derived or stored separately.
    #[serde(default)]
    pub private_key_path: Option<String>,

    /// Path to the node's WireGuard public key.
    #[serde(default)]
    pub public_key_path: Option<String>,

    /// Control‑plane WireGuard peer endpoint, e.g. `control.example.com:51820`.
    #[serde(default)]
    pub peer_endpoint: Option<String>,

    /// Allowed IPs for the WireGuard peer (CIDR strings).
    ///
    /// Typically just the control‑plane address or a narrow range.
    #[serde(default)]
    pub allowed_ips: Vec<String>,

    /// Post‑quantum / Rosenpass configuration.
    #[serde(default)]
    pub rosenpass: Option<RosenpassConfig>,
}

/// Rosenpass‑related configuration for post‑quantum key exchange.
#[derive(Debug, Clone, Deserialize)]
pub struct RosenpassConfig {
    /// Path to this node's Rosenpass private key.
    #[serde(default)]
    pub private_key_path: Option<String>,

    /// Path to this node's Rosenpass public key (if stored separately).
    #[serde(default)]
    pub public_key_path: Option<String>,

    /// Path to the control‑plane Rosenpass public key.
    #[serde(default)]
    pub peer_public_key_path: Option<String>,
}

/// Proxy configuration for the local HTTP interface.
#[derive(Debug, Clone, Deserialize)]
pub struct ProxyConfig {
    /// Port to listen on (binds to WireGuard interface by default).
    ///
    /// Defaults to 8080.
    #[serde(default = "default_listen_port")]
    pub listen_port: u16,

    /// Optional listen address override. This will later be constrained so
    /// that it only binds on the WireGuard address.
    #[serde(default)]
    pub listen_addr: Option<String>,
}

/// Configuration for a single logical endpoint.
///
/// The scheduler and endpoint management layer will turn these into
/// concrete `labman_core::Endpoint` instances and perform health
/// checks and model discovery.
#[derive(Debug, Clone, Deserialize)]
pub struct EndpointConfig {
    /// Logical name for this endpoint (unique per config file).
    pub name: String,

    /// Base URL of the endpoint, typically ending in `/v1`,
    /// e.g. `http://127.0.0.1:11434/v1`.
    pub base_url: String,

    /// Optional concurrency limit for this endpoint.
    #[serde(default)]
    pub max_concurrent: Option<usize>,

    /// Optional list of glob patterns describing which models to expose.
    ///
    /// If provided, only models matching at least one pattern will be
    /// made available through the proxy.
    #[serde(default)]
    pub models_include: Option<Vec<String>>,

    /// Optional list of glob patterns describing which models to exclude.
    ///
    /// Applied after `models_include` (if any).
    #[serde(default)]
    pub models_exclude: Option<Vec<String>>,
}

/// Load configuration from a specific file path.
///
/// This function parses TOML into [`LabmanConfig`] and maps errors into
/// [`LabmanError::Config`] / [`LabmanError::InvalidConfig`] as appropriate.
pub fn load_from_path<P: AsRef<Path>>(path: P) -> Result<LabmanConfig> {
    let path_ref = path.as_ref();
    let contents = fs::read_to_string(path_ref).map_err(|err| {
        LabmanError::config(format!(
            "failed to read config file '{}': {}",
            path_ref.display(),
            err
        ))
    })?;

    let cfg: LabmanConfig = toml::from_str(&contents).map_err(|err| {
        LabmanError::invalid_config(
            path_ref.display().to_string(),
            format!("failed to parse config: {}", err),
        )
    })?;

    Ok(cfg)
}

/// Attempt to load configuration using the default search strategy.
///
/// Current strategy (in order):
/// 1. `/etc/labman/labman.toml`
/// 2. `./labman.toml` (in the current working directory)
pub fn load_default() -> Result<LabmanConfig> {
    let candidates = [
        PathBuf::from("/etc/labman/labman.toml"),
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join("labman.toml"),
    ];

    for candidate in &candidates {
        if candidate.exists() {
            return load_from_path(candidate);
        }
    }

    Err(LabmanError::config(
        "no configuration file found; provide a path explicitly or create /etc/labman/labman.toml or ./labman.toml".to_string(),
    ))
}

fn default_interface_name() -> String {
    "labman0".to_string()
}

fn default_listen_port() -> u16 {
    8080
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;
    use std::path::PathBuf;

    #[test]
    fn test_load_from_path_minimal() {
        // Create a temporary file path in the current directory without relying on
        // external tempfile utilities. This keeps the test self-contained and
        // avoids additional dev-only dependencies.
        let mut path = PathBuf::from("test_labman_config_minimal.toml");

        // Ensure we don't accidentally reuse an existing file from a previous run.
        let _ = fs::remove_file(&path);

        {
            let mut file = fs::File::create(&path).expect("create temp config file");
            writeln!(
                file,
                r#"
[control_plane]
base_url = "https://control.example.com/api/v1"
node_token = "test-token"

[wireguard]
interface_name = "labman0"

[proxy]
listen_port = 8080

[[endpoints]]
name = "local-endpoint"
base_url = "http://127.0.0.1:11434/v1"
"#
            )
            .expect("write config");
        }

        let cfg = load_from_path(&path).expect("load config");

        assert_eq!(
            cfg.control_plane.base_url,
            "https://control.example.com/api/v1"
        );
        assert_eq!(cfg.control_plane.node_token, "test-token");
        assert_eq!(cfg.wireguard.interface_name, "labman0");
        assert_eq!(cfg.proxy.listen_port, 8080);
        assert_eq!(cfg.endpoints.len(), 1);
        assert_eq!(cfg.endpoints[0].name, "local-endpoint");
        assert_eq!(cfg.endpoints[0].base_url, "http://127.0.0.1:11434/v1");

        // Best-effort cleanup; ignore errors if the file was already removed.
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_missing_file_errors() {
        let res = load_from_path("/this/definitely/does/not/exist.toml");
        assert!(res.is_err());
    }
}
