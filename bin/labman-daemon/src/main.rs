use std::net::SocketAddr;
use std::path::PathBuf;
use std::process;

use clap::{ArgAction, Parser};
use labman_config::{load_default, load_from_path, LabmanConfig};
use labman_core::LabmanError;
use labman_telemetry;
use tracing::warn;

/// labmand - labman daemon
///
/// At this stage, labmand is responsible only for:
/// - Parsing basic CLI arguments
/// - Loading configuration via `labman-config`
/// - Printing a short summary and exiting
///
/// Configuration discovery rules:
/// 1. If `--config PATH` (or `-c PATH`) is provided, that path is used.
/// 2. Otherwise, `labman_config::load_default()` is used, which probes:
///    - `/etc/labman/labman.toml`
///    - `./labman.toml`
///
/// No labman-specific environment variables are used for configuration.
#[derive(Debug, Parser)]
#[command(
    name = "labmand",
    version,
    about = "labman daemon",
    long_about = "labmand is the labman daemon responsible for managing secure control-plane connectivity and proxying to local/remote LLM endpoints.",
    disable_help_subcommand = true
)]
struct Cli {
    /// Path to configuration file (TOML).
    ///
    /// When provided, this path is used instead of the default search locations.
    /// Long form (`--config`) is preferred in docs and examples; `-c` is a
    /// short-form alias for interactive use.
    #[arg(long = "config", short = 'c', value_name = "PATH")]
    config: Option<PathBuf>,

    /// Log level for labmand (overrides RUST_LOG if set).
    ///
    /// Accepts standard tracing levels (trace, debug, info, warn, error) or a
    /// full filter expression (e.g. "info,labmand=debug").
    #[arg(long = "log-level", short = 'L', value_name = "LEVEL")]
    log_level: Option<String>,

    /// Print loaded configuration summary and exit without starting the daemon.
    ///
    /// This is primarily useful for debugging configuration issues.
    #[arg(long = "print-config", action = ArgAction::SetTrue)]
    print_config: bool,

    /// Start a minimal metrics HTTP server and then exit.
    ///
    /// This is an early stub used to verify that the `/metrics` endpoint can be
    /// reached by the control plane over WireGuard and by an operator's
    /// Prometheus/Grafana stack on their local network (subject to routing and
    /// firewall configuration).
    #[arg(long = "serve-metrics", value_name = "ADDR", requires = "print_config")]
    serve_metrics: Option<String>,

    /// Validate configuration and exit without starting the daemon.
    ///
    /// This is useful for CI and deployment pipelines to ensure configuration
    /// is structurally sound before rollout.
    #[arg(long = "check-config", action = ArgAction::SetTrue)]
    check_config: bool,
}

fn main() {
    let cli = Cli::parse();

    // Initialise telemetry as early as possible so subsequent logs use the
    // configured subscriber. CLI-provided log level, if any, takes precedence
    // over RUST_LOG.
    if let Err(err) = labman_telemetry::init(cli.log_level.as_deref()) {
        eprintln!("labmand: failed to initialise telemetry: {}", err);
        process::exit(1);
    }

    let config_result: Result<LabmanConfig, LabmanError> = if let Some(path) = cli.config {
        match load_from_path(&path) {
            Ok(cfg) => {
                tracing::info!("loaded configuration from {}", path.display());
                Ok(cfg)
            }
            Err(err) => {
                tracing::error!(
                    "failed to load configuration from {}: {}",
                    path.display(),
                    err
                );
                Err(err)
            }
        }
    } else {
        match load_default() {
            Ok(cfg) => {
                tracing::info!("loaded configuration from default locations");
                Ok(cfg)
            }
            Err(err) => {
                tracing::error!("failed to load configuration from default locations: {err}");
                Err(err)
            }
        }
    };

    let config = match config_result {
        Ok(cfg) => cfg,
        Err(_) => {
            // Error already printed above; exit with a non-zero status code.
            process::exit(1);
        }
    };

    // Perform structural validation before any further processing.
    if let Err(err) = config.validate() {
        tracing::error!("configuration validation failed: {}", err);
        process::exit(1);
    }

    if cli.check_config {
        // Configuration loaded and validated successfully; exit cleanly.
        tracing::info!("configuration is valid");
        return;
    }

    if let Some(addr_str) = cli.serve_metrics.as_deref() {
        // This is a stub implementation that only logs the intended bind
        // address for the metrics server. In a later stage, this will be
        // replaced by an async HTTP listener that serves `/metrics` backed by
        // Prometheus.
        match addr_str.parse::<SocketAddr>() {
            Ok(addr) => {
                tracing::info!(
                    "metrics server stub would bind on {} for control plane and operator scrapers",
                    addr
                );
            }
            Err(err) => {
                tracing::error!("invalid metrics bind address '{}': {}", addr_str, err);
                process::exit(1);
            }
        }

        // In stub mode, we do not actually start a server; we just exit after
        // confirming the address is syntactically valid.
        return;
    }

    if cli.print_config {
        tracing::info!("starting labmand with loaded configuration");
        print_config_summary(&config);
        // For now we just exit after printing.
        return;
    }

    // Placeholder for future startup sequence:
    // - Telemetry initialization
    // - WireGuard / Rosenpass setup
    // - Endpoint management & proxy startup
    //
    // For now, just print a brief summary and exit.
    print_config_summary(&config);
}

/// Print a concise summary of the loaded configuration.
///
/// This is intentionally minimal for now; future stages can expand it or
/// replace it with structured logging.
fn print_config_summary(cfg: &LabmanConfig) {
    println!("labmand configuration summary:");
    println!("  control_plane.base_url = {}", cfg.control_plane.base_url);
    println!(
        "  control_plane.region    = {}",
        cfg.control_plane.region.as_deref().unwrap_or("-")
    );
    println!(
        "  control_plane.description = {}",
        cfg.control_plane.description.as_deref().unwrap_or("-")
    );

    println!(
        "  wireguard.interface_name = {}",
        cfg.wireguard.interface_name
    );
    println!(
        "  wireguard.address        = {}",
        cfg.wireguard
            .address
            .as_deref()
            .unwrap_or("<not set; may be provided by control plane>")
    );
    println!(
        "  wireguard.peer_endpoint  = {}",
        cfg.wireguard
            .peer_endpoint
            .as_deref()
            .unwrap_or("<not set>")
    );
    println!(
        "  wireguard.allowed_ips    = [{}]",
        if cfg.wireguard.allowed_ips.is_empty() {
            String::from("<none>")
        } else {
            cfg.wireguard.allowed_ips.join(", ")
        }
    );

    println!("  proxy.listen_port        = {}", cfg.proxy.listen_port);
    println!(
        "  proxy.listen_addr        = {}",
        cfg.proxy
            .listen_addr
            .as_deref()
            .unwrap_or("<default (WG addr)>")
    );

    println!("  endpoints:");
    if cfg.endpoints.is_empty() {
        println!("    <none configured>");
    } else {
        for ep in &cfg.endpoints {
            println!("    - name        = {}", ep.name);
            println!("      base_url    = {}", ep.base_url);
            if let Some(max) = ep.max_concurrent {
                println!("      max_concurrent = {}", max);
            } else {
                println!("      max_concurrent = <unbounded>");
            }
            match &ep.models_include {
                Some(patterns) if !patterns.is_empty() => {
                    println!("      models_include = [{}]", patterns.join(", "));
                }
                _ => println!("      models_include = <none>"),
            }
            match &ep.models_exclude {
                Some(patterns) if !patterns.is_empty() => {
                    println!("      models_exclude = [{}]", patterns.join(", "));
                }
                _ => println!("      models_exclude = <none>"),
            }
        }
    }
}
