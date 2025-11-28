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

impl LabmanConfig {
    /// Perform basic structural validation of the configuration.
    ///
    /// This does not attempt to contact any external systems; it only checks
    /// for obviously invalid or inconsistent values. More advanced validation
    /// (e.g., control-plane reachability) belongs in higher-level crates.
    pub fn validate(&self) -> Result<()> {
        self.validate_control_plane()?;
        self.validate_endpoints()?;
        self.validate_wireguard()?;
        Ok(())
    }

    fn validate_control_plane(&self) -> Result<()> {
        if self.control_plane.base_url.trim().is_empty() {
            return Err(LabmanError::invalid_config(
                "control_plane.base_url",
                "control_plane.base_url must not be empty",
            ));
        }

        if self.control_plane.node_token.trim().is_empty() {
            return Err(LabmanError::invalid_config(
                "control_plane.node_token",
                "control_plane.node_token must not be empty",
            ));
        }

        // Very lightweight check: URL should look like http(s)://...
        let url = self.control_plane.base_url.trim();
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err(LabmanError::invalid_config(
                "control_plane.base_url",
                "control_plane.base_url must start with http:// or https://",
            ));
        }

        Ok(())
    }

    fn validate_endpoints(&self) -> Result<()> {
        // Check for duplicate endpoint names
        let mut seen = std::collections::HashSet::new();
        for ep in &self.endpoints {
            if ep.name.trim().is_empty() {
                return Err(LabmanError::invalid_config(
                    "endpoints.name",
                    "endpoint name must not be empty",
                ));
            }

            if !seen.insert(ep.name.clone()) {
                return Err(LabmanError::invalid_config(
                    "endpoints.name",
                    &format!("duplicate endpoint name: {}", ep.name),
                ));
            }

            let base_url = ep.base_url.trim();
            if base_url.is_empty() {
                return Err(LabmanError::invalid_config(
                    "endpoints.base_url",
                    &format!("endpoint '{}' has an empty base_url", ep.name),
                ));
            }

            if !(base_url.starts_with("http://") || base_url.starts_with("https://")) {
                return Err(LabmanError::invalid_config(
                    "endpoints.base_url",
                    &format!(
                        "endpoint '{}' base_url must start with http:// or https://",
                        ep.name
                    ),
                ));
            }

            // Allow base URLs that either end with `/v1` or can be normalized to it.
            if !(base_url.ends_with("/v1") || base_url.contains("/v1/")) {
                // We do not modify here, just warn via error message context.
                // Normalisation logic, if any, should live in a higher layer.
                return Err(LabmanError::invalid_config(
                    "endpoints.base_url",
                    &format!(
                        "endpoint '{}' base_url should typically end with /v1 (got '{}')",
                        ep.name, base_url
                    ),
                ));
            }
        }

        Ok(())
    }

    fn validate_wireguard(&self) -> Result<()> {
        // For now we only perform very basic checks; stronger invariants
        // (e.g., CIDR parsing, interface existence) are left to the
        // wireguard layer.
        if self.wireguard.interface_name.trim().is_empty() {
            return Err(LabmanError::invalid_config(
                "wireguard.interface_name",
                "wireguard.interface_name must not be empty",
            ));
        }

        // Sanity check allowed_ips for obviously bogus entries.
        for cidr in &self.wireguard.allowed_ips {
            if cidr.trim().is_empty() {
                return Err(LabmanError::invalid_config(
                    "wireguard.allowed_ips",
                    "wireguard.allowed_ips must not contain empty entries",
                ));
            }
        }

        Ok(())
    }
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
        let path = PathBuf::from("test_labman_config_minimal.toml");

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

    #[test]
    fn test_validate_rejects_empty_control_plane_url() {
        let cfg = LabmanConfig {
            control_plane: ControlPlaneConfig {
                base_url: "".to_string(),
                node_token: "token".to_string(),
                region: None,
                description: None,
            },
            wireguard: WireGuardConfig {
                interface_name: "labman0".to_string(),
                address: None,
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
            endpoints: Vec::new(),
        };

        let res = cfg.validate();
        assert!(res.is_err());
    }

    #[test]
    fn test_validate_rejects_duplicate_endpoint_names() {
        let cfg = LabmanConfig {
            control_plane: ControlPlaneConfig {
                base_url: "https://control.example.com/api/v1".to_string(),
                node_token: "token".to_string(),
                region: None,
                description: None,
            },
            wireguard: WireGuardConfig {
                interface_name: "labman0".to_string(),
                address: None,
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
            endpoints: vec![
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
            ],
        };

        let res = cfg.validate();
        assert!(res.is_err());
    }
}
