use std::env;
use std::str::FromStr;

use time::{format_description, UtcOffset};
use tracing::Level;
use tracing_subscriber::filter::EnvFilter;
use tracing_subscriber::fmt;
use tracing_subscriber::fmt::time::OffsetTime;
use tracing_subscriber::prelude::*;

/// Re-export the Prometheus-backed metrics recorder so that other crates can
/// depend on a concrete type without any feature gating.
pub use crate::prometheus_impl::PrometheusMetricsRecorder;

use prometheus::{
    Encoder, HistogramOpts, HistogramVec, IntCounterVec, IntGauge, Opts, Registry, TextEncoder,
};

use hyper::{body::Bytes, Response};

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

/// A simple counter metric.
///
/// Implementations are expected to be cheap and non-panicking. This trait is
/// intentionally minimal and does not prescribe any particular metrics backend.
pub trait Counter {
    /// Increment the counter by 1.
    fn inc(&self);

    /// Add an arbitrary value to the counter.
    fn add(&self, value: u64);
}

/// A simple gauge metric.
///
/// Gauges represent values that can go up and down (e.g. active requests).
pub trait Gauge {
    /// Set the gauge to an absolute value.
    fn set(&self, value: i64);

    /// Increment the gauge by 1.
    fn inc(&self);

    /// Decrement the gauge by 1.
    fn dec(&self);
}

/// A simple histogram metric.
///
/// This is used for recording distributions such as request latencies.
pub trait Histogram {
    /// Record a new sample value.
    fn observe(&self, value: f64);
}

/// Interface for recording proxy- and endpoint-level metrics.
///
/// This trait is meant to be implemented by whichever metrics backend we
/// choose in the future (Prometheus, OpenTelemetry, etc.). For now it
/// allows call sites to be wired without committing to a concrete
/// implementation.
pub trait MetricsRecorder: Send + Sync + 'static {
    /// Record that a request has started.
    ///
    /// - `endpoint`: logical endpoint name, if known.
    /// - `model`: logical model name, if known.
    fn record_request_start(&self, endpoint: Option<&str>, model: Option<&str>);

    /// Record that a request has completed.
    ///
    /// - `endpoint`: logical endpoint name, if known.
    /// - `model`: logical model name, if known.
    /// - `success`: whether the request was considered successful.
    /// - `latency_secs`: request latency in seconds (optional if not measured).
    fn record_request_end(
        &self,
        endpoint: Option<&str>,
        model: Option<&str>,
        success: bool,
        latency_secs: Option<f64>,
    );

    /// Record an error associated with an endpoint or the system as a whole.
    ///
    /// - `endpoint`: logical endpoint name, if applicable.
    /// - `kind`: a short, stable error kind string (e.g. "timeout",
    ///   "upstream_5xx", "config").
    fn record_error(&self, endpoint: Option<&str>, kind: &str);

    /// Record a change in the number of active proxied requests.
    ///
    /// This is typically mirrored by a gauge in the concrete implementation.
    fn set_active_requests(&self, count: u64);
}

/// A no-op metrics recorder that does nothing.
///
/// This is useful as a default implementation in environments where metrics
/// are not configured or desired.
#[derive(Debug, Clone, Default)]
pub struct NoopMetricsRecorder;

impl MetricsRecorder for NoopMetricsRecorder {
    fn record_request_start(&self, _endpoint: Option<&str>, _model: Option<&str>) {}

    fn record_request_end(
        &self,
        _endpoint: Option<&str>,
        _model: Option<&str>,
        _success: bool,
        _latency_secs: Option<f64>,
    ) {
    }

    fn record_error(&self, _endpoint: Option<&str>, _kind: &str) {}

    fn set_active_requests(&self, _count: u64) {}
}

pub mod prometheus_impl {
    use super::*;

    /// Prometheus HTTP metrics handler.
    ///
    /// This function encodes the given registry into Prometheus' text exposition
    /// format and returns an HTTP response suitable for serving on a `/metrics`
    /// endpoint.
    ///
    /// Typical usage in an HTTP server:
    ///
    /// - Expose this handler at `/metrics` on a listener that is reachable:
    ///   - Over the WireGuard tunnel for the control plane.
    ///   - From the operator's local network for their own Prometheus/Grafana stack,
    ///     if they configure routing/firewalling appropriately.
    ///
    /// The handler itself is agnostic to how the listener is exposed; that is the
    /// daemon's responsibility.
    pub fn prometheus_http_response(registry: &Registry) -> Response<Bytes> {
        let encoder = TextEncoder::new();
        let metric_families = registry.gather();

        let mut buffer = Vec::new();
        if let Err(err) = encoder.encode(&metric_families, &mut buffer) {
            // In case of encoding failure, return a 500 with a simple text body.
            let body = format!("failed to encode Prometheus metrics: {}", err);
            return Response::builder()
                .status(500)
                .header("Content-Type", "text/plain; charset=utf-8")
                .body(Bytes::from(body))
                .unwrap_or_else(|_| Response::new(Bytes::from_static(b"internal error")));
        }

        Response::builder()
            .status(200)
            .header(
                "Content-Type",
                encoder.format_type(), // e.g. "text/plain; version=0.0.4"
            )
            .body(Bytes::from(buffer))
            .unwrap_or_else(|_| Response::new(Bytes::from_static(b"internal error")))
    }

    /// Prometheus-backed metrics recorder and HTTP exporter.
    ///
    /// This is behind the `prometheus` feature flag so that deployments which do
    /// not require metrics do not have to pull in the Prometheus and async HTTP
    /// stacks.
    #[derive(Clone)]
    pub struct PrometheusMetricsRecorder {
        pub(crate) registry: Registry,
        requests_total: IntCounterVec,
        request_latency_seconds: HistogramVec,
        active_requests: IntGauge,
        errors_total: IntCounterVec,
    }

    impl PrometheusMetricsRecorder {
        /// Create a new Prometheus-backed recorder with a fresh registry.
        pub fn new() -> Self {
            let registry = Registry::new();

            let requests_total = IntCounterVec::new(
                Opts::new(
                    "labman_requests_total",
                    "Total number of requests processed",
                )
                .namespace("labman"),
                &["endpoint", "model", "success"],
            )
            .expect("failed to create labman_requests_total counter");
            registry
                .register(Box::new(requests_total.clone()))
                .expect("failed to register labman_requests_total");

            let request_latency_seconds = HistogramVec::new(
                HistogramOpts::new(
                    "labman_request_latency_seconds",
                    "Request latency in seconds",
                )
                .namespace("labman"),
                &["endpoint", "model"],
            )
            .expect("failed to create labman_request_latency_seconds histogram");
            registry
                .register(Box::new(request_latency_seconds.clone()))
                .expect("failed to register labman_request_latency_seconds");

            let active_requests = IntGauge::with_opts(
                Opts::new(
                    "labman_active_requests",
                    "Number of active proxied requests on this node",
                )
                .namespace("labman"),
            )
            .expect("failed to create labman_active_requests gauge");
            registry
                .register(Box::new(active_requests.clone()))
                .expect("failed to register labman_active_requests");

            let errors_total = IntCounterVec::new(
                Opts::new(
                    "labman_errors_total",
                    "Total number of errors encountered by this node",
                )
                .namespace("labman"),
                &["endpoint", "kind"],
            )
            .expect("failed to create labman_errors_total counter");
            registry
                .register(Box::new(errors_total.clone()))
                .expect("failed to register labman_errors_total");

            Self {
                registry,
                requests_total,
                request_latency_seconds,
                active_requests,
                errors_total,
            }
        }

        /// Access the underlying Prometheus registry, for use by HTTP exporters.
        pub fn registry(&self) -> &Registry {
            &self.registry
        }
    }

    impl MetricsRecorder for PrometheusMetricsRecorder {
        fn record_request_start(&self, _endpoint: Option<&str>, _model: Option<&str>) {
            // We don't change any counters here; active_requests is updated via
            // set_active_requests, which the caller should maintain.
        }

        fn record_request_end(
            &self,
            endpoint: Option<&str>,
            model: Option<&str>,
            success: bool,
            latency_secs: Option<f64>,
        ) {
            let endpoint_label = endpoint.unwrap_or("_unknown");
            let model_label = model.unwrap_or("_unknown");
            let success_label = if success { "true" } else { "false" };

            self.requests_total
                .with_label_values(&[endpoint_label, model_label, success_label])
                .inc();

            if let Some(lat) = latency_secs {
                self.request_latency_seconds
                    .with_label_values(&[endpoint_label, model_label])
                    .observe(lat);
            }
        }

        fn record_error(&self, endpoint: Option<&str>, kind: &str) {
            let endpoint_label = endpoint.unwrap_or("_unknown");
            self.errors_total
                .with_label_values(&[endpoint_label, kind])
                .inc();
        }

        fn set_active_requests(&self, count: u64) {
            self.active_requests.set(count as i64);
        }
    }
}

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

    #[test]
    fn noop_metrics_recorder_does_not_panic() {
        let recorder = NoopMetricsRecorder::default();

        recorder.record_request_start(Some("endpoint-1"), Some("model-A"));
        recorder.record_request_end(Some("endpoint-1"), Some("model-A"), true, Some(0.123));
        recorder.record_error(Some("endpoint-1"), "timeout");
        recorder.set_active_requests(5);
    }

    // Note: `EnvFilter` is intentionally permissive and accepts many strings as
    // valid filter expressions, so we do not assert on specific rejection
    // behavior here. The important cases are covered by the positive parsing
    // tests above.
}
