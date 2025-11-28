use std::env;
use std::str::FromStr;

use time::{format_description, UtcOffset};
use tracing::Level;
use tracing_subscriber::filter::EnvFilter;
use tracing_subscriber::fmt;
use tracing_subscriber::fmt::time::OffsetTime;
use tracing_subscriber::prelude::*;

/// Error type for telemetry initialisation failures.
///
/// This is intentionally lightweight so `labman-telemetry` can be used
/// without depending on `labman-core`. Callers can map this into their own
/// error types as needed.
#[derive(Debug)]
pub enum TelemetryError {
    /// Provided log level string could not be parsed.
    InvalidLevel(String),

    /// Failed to configure the subscriber (should be rare).
    SubscriberInit(String),
}

impl std::fmt::Display for TelemetryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TelemetryError::InvalidLevel(level) => {
                write!(f, "invalid log level: {}", level)
            }
            TelemetryError::SubscriberInit(msg) => write!(f, "failed to init telemetry: {}", msg),
        }
    }
}

impl std::error::Error for TelemetryError {}

/// Result alias for telemetry operations.
pub type Result<T> = std::result::Result<T, TelemetryError>;

/// Initialise the global telemetry / logging subscriber.
///
/// This sets up a `tracing_subscriber` using `EnvFilter` and a formatted
/// output layer. It is intended to be called once at process startup
/// (typically from `main` in the daemon or CLI).
///
/// # Parameters
///
/// - `level`: Optional log level string. If `None`, the function will:
///   - Respect `RUST_LOG` if it is set, or
///   - Default to `"info"` otherwise.
///   If `Some(level)` is provided, it takes precedence over `RUST_LOG`.
///
/// # Behavior
///
/// - Logs are formatted with timestamps, level, and target.
/// - A single global subscriber is installed. Calling `init` more than once
///   will be treated as a no-op by `tracing_subscriber::registry().init()`.
///
/// # Examples
///
/// Basic usage with default level:
///
/// ```ignore
/// labman_telemetry::init(None)?;
/// ```
///
/// Explicit level:
///
/// ```ignore
/// labman_telemetry::init(Some("debug"))?;
/// ```
///
/// Respect `RUST_LOG` (when `level` is `None`):
///
/// ```ignore
/// // RUST_LOG=labmand=trace labmand ...
/// labman_telemetry::init(None)?;
/// ```
pub fn init(level: Option<&str>) -> Result<()> {
    // Determine the effective filter string:
    //
    // - If an explicit level is provided, use that (e.g. "info", "debug").
    // - Otherwise:
    //   - If RUST_LOG is set, let EnvFilter parse it.
    //   - Else default to "info".
    let filter = if let Some(level_str) = level {
        parse_level_filter(level_str)?
    } else if env::var("RUST_LOG").is_ok() {
        EnvFilter::from_default_env()
    } else {
        EnvFilter::new("info")
    };

    // Build a text formatter with timestamps, level, and target (module path).
    let fmt_layer = fmt::layer()
        .with_target(true)
        .with_thread_ids(false)
        .with_thread_names(false)
        .with_level(true)
        .with_timer(OffsetTime::new(
            // Use local time with offset; falls back to UTC if offset cannot be determined.
            UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC),
            format_description::parse(
                "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z",
            )
            .unwrap_or_else(|_| {
                // Fallback to a very simple format if the description cannot be parsed.
                format_description::parse("[hour]:[minute]:[second]").unwrap()
            }),
        ));

    // Compose registry + filter + formatter.
    let subscriber = tracing_subscriber::registry().with(filter).with(fmt_layer);

    // Install as global subscriber.
    subscriber
        .try_init()
        .map_err(|e| TelemetryError::SubscriberInit(e.to_string()))?;

    Ok(())
}

/// Parse a simple level string into an `EnvFilter`.
///
/// Supports both plain levels ("info", "debug", etc.) and full `EnvFilter`
/// expressions (like "info,labmand=debug").
///
/// The heuristic is:
/// - If the string parses cleanly as a `Level`, we use it as a simple
///   global filter (`EnvFilter::new(level_str)`).
/// - Otherwise, we treat the string as an `EnvFilter` expression and let
///   `EnvFilter::builder()` handle it.
fn parse_level_filter(level_str: &str) -> Result<EnvFilter> {
    // First try to parse as a simple Level.
    if Level::from_str(level_str).is_ok() {
        return Ok(EnvFilter::new(level_str));
    }

    // Fallback: treat as a full EnvFilter expression, e.g. "info,labmand=debug".
    EnvFilter::builder()
        .parse(level_str)
        .map_err(|e| TelemetryError::InvalidLevel(format!("{} ({})", level_str, e)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_level() {
        let f = parse_level_filter("info").expect("should parse info level");
        // EnvFilter doesn't expose a simple way to inspect internal rules,
        // but successful construction is enough for this test.
        let _ = f;
    }

    #[test]
    fn parse_full_expression() {
        let f = parse_level_filter("info,labmand=debug").expect("should parse expression");
        let _ = f;
    }

    // Note: `EnvFilter` is intentionally permissive and accepts many strings as
    // valid filter expressions, so we do not assert on specific rejection
    // behavior here. The important cases are covered by the positive parsing
    // tests above.
}
