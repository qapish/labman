use std::process::Command;
use std::str;
use std::time::Duration;

use thiserror::Error;
use tracing::{debug, error, info, warn};

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, WireGuardError>;

/// Errors that can occur while managing WireGuard and Rosenpass integration.
#[derive(Debug, Error)]
pub enum WireGuardError {
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),

    #[error("wireguard operation failed: {0}")]
    WireGuard(String),

    #[error("rosenpass operation failed: {0}")]
    Rosenpass(String),

    #[error("io error: {0}")]
    Io(std::io::Error),
}

impl From<std::io::Error> for WireGuardError {
    fn from(err: std::io::Error) -> Self {
        WireGuardError::Io(err)
    }
}

/// High-level configuration for a WireGuard interface.
///
/// This is built from:
/// - `labman-config`'s `wireguard` section
/// - Control-plane registration response (for WG address, allowed IPs, etc.)
#[derive(Debug, Clone)]
pub struct WireGuardConfig {
    /// Interface name, e.g. `labman0`.
    pub interface_name: String,

    /// Local address, e.g. `10.90.0.2/32`.
    pub address: String,

    /// Peer endpoint (control-plane WG endpoint), e.g. `vpn.example.com:51820`.
    pub peer_endpoint: String,

    /// Allowed IPs as provided by the control-plane.
    pub allowed_ips: Vec<String>,

    /// Optional path to the private key file.
    pub private_key_path: Option<String>,

    /// Optional path to the public key file.
    pub public_key_path: Option<String>,
}

/// Runtime representation of a WireGuard interface managed by labman.
#[derive(Debug, Clone)]
pub struct WireGuardInterface {
    pub name: String,
    pub address: String,
    pub peer_endpoint: String,
    pub allowed_ips: Vec<String>,
}

/// Status of a WireGuard interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterfaceStatus {
    Up,
    Down,
    Unknown,
}

/// Configuration for Rosenpass integration.
///
/// For the initial version, this is intentionally minimal and geared towards
/// using Rosenpass as an external process. Future iterations can align this
/// more closely with Rosenpass's native configuration formats and APIs.
#[derive(Debug, Clone)]
pub struct RosenpassConfig {
    /// Path to a Rosenpass configuration file, if used.
    pub config_path: Option<String>,

    /// Directory where Rosenpass stores its persistent state and keys.
    pub state_dir: Option<String>,

    /// Optional Unix socket path if Rosenpass exposes a control socket.
    pub socket_path: Option<String>,
}

/// Abstraction over WireGuard operations.
///
/// Implementations can:
/// - Use shell commands (`ip`, `wg`, `wg-quick`) in a dev/backend,
/// - Use `wireguard-uapi` directly,
/// - Delegate to an external broker (e.g. Rosenpass's wireguard-broker).
pub trait WireGuardBackend: Send + Sync {
    /// Create a WireGuard interface based on the provided configuration.
    fn create_interface(&self, cfg: &WireGuardConfig) -> Result<WireGuardInterface>;

    /// Bring the given interface up (but not necessarily configure peers).
    fn bring_up(&self, iface: &WireGuardInterface) -> Result<()>;

    /// Bring the given interface down and delete it.
    fn bring_down(&self, iface: &WireGuardInterface) -> Result<()>;

    /// Query the interface status.
    fn status(&self, name: &str) -> Result<InterfaceStatus>;
}

/// Abstraction over Rosenpass PQ key exchange and key management.
///
/// For the first iteration, this trait is designed to support:
/// - Using Rosenpass as an external process,
/// - Ensuring that WireGuard keys are generated and available on disk.
pub trait RosenpassEngine: Send + Sync {
    /// Initialise Rosenpass for this node.
    ///
    /// This might:
    /// - Validate configuration,
    /// - Ensure state directories exist,
    /// - Optionally spawn a long-running Rosenpass process.
    fn init(&self, cfg: &RosenpassConfig) -> Result<()>;

    /// Ensure that WireGuard key material is available and return
    /// `(wg_private_key, wg_public_key)` as base64 or raw text.
    ///
    /// In an initial implementation, this may simply read key files that
    /// Rosenpass has written to disk.
    fn ensure_keys(&self) -> Result<(String, String)>;
}

/// A basic `WireGuardBackend` implementation that shells out to system
/// commands (`ip`, `wg`) to manage interfaces.
///
/// This is intended for development and simple deployments where:
/// - The host has standard WireGuard tooling installed,
/// - labman is running with sufficient privileges to manage interfaces.
pub struct ShellWireGuardBackend {
    /// Optional timeout for shell commands.
    pub command_timeout: Option<Duration>,
}

impl ShellWireGuardBackend {
    /// Create a new `ShellWireGuardBackend` with no explicit timeout.
    pub fn new() -> Self {
        Self {
            command_timeout: None,
        }
    }

    /// Create a new backend with a specific timeout for shell commands.
    pub fn with_timeout(timeout: Duration) -> Self {
        Self {
            command_timeout: Some(timeout),
        }
    }

    fn run_command(&self, program: &str, args: &[&str]) -> Result<()> {
        debug!("wireguard-shell: running {} {:?}", program, args);

        let mut cmd = Command::new(program);
        cmd.args(args);

        let output = cmd.output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            error!(
                "wireguard-shell: command {} {:?} failed: {}",
                program, args, stderr
            );
            return Err(WireGuardError::WireGuard(format!(
                "{} {:?} failed: {}",
                program, args, stderr
            )));
        }

        Ok(())
    }
}

impl WireGuardBackend for ShellWireGuardBackend {
    fn create_interface(&self, cfg: &WireGuardConfig) -> Result<WireGuardInterface> {
        if cfg.interface_name.trim().is_empty() {
            return Err(WireGuardError::InvalidConfig(
                "interface_name must not be empty".to_string(),
            ));
        }
        if cfg.address.trim().is_empty() {
            return Err(WireGuardError::InvalidConfig(
                "address must not be empty".to_string(),
            ));
        }

        // NOTE: This is a very minimal, Linux-oriented implementation. In
        // early iterations we only create the interface and assign the
        // address. Peer configuration and keys are expected to be managed
        // separately (e.g. via Rosenpass broker or additional commands).
        //
        // Equivalent commands:
        //   ip link add dev <name> type wireguard
        //   ip address add <address> dev <name>

        info!(
            "wireguard-shell: creating interface '{}' with address '{}'",
            cfg.interface_name, cfg.address
        );

        self.run_command(
            "ip",
            &[
                "link",
                "add",
                "dev",
                &cfg.interface_name,
                "type",
                "wireguard",
            ],
        )?;
        self.run_command(
            "ip",
            &["address", "add", &cfg.address, "dev", &cfg.interface_name],
        )?;

        Ok(WireGuardInterface {
            name: cfg.interface_name.clone(),
            address: cfg.address.clone(),
            peer_endpoint: cfg.peer_endpoint.clone(),
            allowed_ips: cfg.allowed_ips.clone(),
        })
    }

    fn bring_up(&self, iface: &WireGuardInterface) -> Result<()> {
        info!(
            "wireguard-shell: bringing up interface '{}' ({})",
            iface.name, iface.address
        );
        self.run_command("ip", &["link", "set", "up", "dev", &iface.name])?;
        Ok(())
    }

    fn bring_down(&self, iface: &WireGuardInterface) -> Result<()> {
        info!("wireguard-shell: deleting interface '{}'", iface.name);
        self.run_command("ip", &["link", "del", "dev", &iface.name])?;
        Ok(())
    }

    fn status(&self, name: &str) -> Result<InterfaceStatus> {
        debug!("wireguard-shell: querying status for '{}'", name);

        let mut cmd = Command::new("ip");
        cmd.args(["link", "show", "dev", name]);

        let output = cmd.output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            warn!(
                "wireguard-shell: failed to query status for '{}': {}",
                name, stderr
            );
            return Ok(InterfaceStatus::Unknown);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        if stdout.contains("state UP") {
            Ok(InterfaceStatus::Up)
        } else if stdout.contains("state DOWN") {
            Ok(InterfaceStatus::Down)
        } else {
            Ok(InterfaceStatus::Unknown)
        }
    }
}

/// A `RosenpassEngine` implementation that treats Rosenpass as an external
/// system dependency.
///
/// This is intentionally minimal. It is responsible for:
/// - Validating that Rosenpass appears to be available,
/// - Optionally bootstrapping configuration/state,
/// - Ensuring that WireGuard keys exist by reading them from disk.
///
/// Later iterations can:
/// - Spawn/monitor a Rosenpass daemon,
/// - Interact with a Rosenpass control socket,
/// - Use richer configuration semantics.
pub struct SystemRosenpassEngine;

impl SystemRosenpassEngine {
    pub fn new() -> Self {
        Self
    }

    fn check_rp_available(&self) -> Result<()> {
        let output = Command::new("which").arg("rp").output()?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(WireGuardError::Rosenpass(format!(
                "rosenpass binary 'rp' not found in PATH: {}",
                stderr
            )));
        }
        Ok(())
    }
}

impl RosenpassEngine for SystemRosenpassEngine {
    fn init(&self, cfg: &RosenpassConfig) -> Result<()> {
        // For now we simply ensure that `rp` is available and log basic
        // configuration. Future versions may start a long-running Rosenpass
        // process or perform additional validation.
        self.check_rp_available()?;

        info!("rosenpass-system: initialising Rosenpass integration");

        if let Some(ref config_path) = cfg.config_path {
            info!("rosenpass-system: config_path = {}", config_path);
        } else {
            info!("rosenpass-system: no explicit config_path provided");
        }

        if let Some(ref state_dir) = cfg.state_dir {
            info!("rosenpass-system: state_dir = {}", state_dir);
        }

        if let Some(ref socket_path) = cfg.socket_path {
            info!("rosenpass-system: socket_path = {}", socket_path);
        }

        Ok(())
    }

    fn ensure_keys(&self) -> Result<(String, String)> {
        // In an initial skeleton, we do not yet automate Rosenpass key
        // generation. Instead, this method can be wired to read existing
        // key files from disk (paths stored in `labman-config`) once that
        // configuration is plumbed through.
        //
        // For now, return an explicit error to make it clear that this
        // must be implemented before relying on it in production.
        Err(WireGuardError::Rosenpass(
            "SystemRosenpassEngine::ensure_keys is not yet implemented; \
             integrate with Rosenpass key material on disk or a control socket"
                .to_string(),
        ))
    }
}
