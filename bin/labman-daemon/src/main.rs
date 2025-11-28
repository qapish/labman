use std::path::PathBuf;
use std::process;

use clap::{ArgAction, Parser};
use labman_config::{load_default, load_from_path, LabmanConfig};
use labman_core::LabmanError;

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

    /// Print loaded configuration summary and exit without starting the daemon.
    ///
    /// This is primarily useful for debugging configuration issues.
    #[arg(long = "print-config", action = ArgAction::SetTrue)]
    print_config: bool,
}

fn main() {
    let cli = Cli::parse();

    let config_result: Result<LabmanConfig, LabmanError> = if let Some(path) = cli.config {
        match load_from_path(&path) {
            Ok(cfg) => {
                eprintln!("labmand: loaded configuration from {}", path.display());
                Ok(cfg)
            }
            Err(err) => {
                eprintln!(
                    "labmand: failed to load configuration from {}: {}",
                    path.display(),
                    err
                );
                Err(err)
            }
        }
    } else {
        match load_default() {
            Ok(cfg) => {
                eprintln!("labmand: loaded configuration from default locations");
                Ok(cfg)
            }
            Err(err) => {
                eprintln!("labmand: failed to load configuration from default locations: {err}");
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

    if cli.print_config {
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
