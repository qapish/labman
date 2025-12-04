# labman Implementation Roadmap

This roadmap describes a staged path from the current repository state to a complete implementation of the functionality described in `readme.md` and `architecture.md`. Each stage builds on the previous one, with clear deliverables and integration points.

The goal is to reach:

- A single `labmand` daemon that:
  - Brings up a post-quantum WireGuard interface (via Rosenpass)
  - Registers the node with the control plane
  - Discovers and tracks local OpenAI-compatible endpoints
  - Exposes a unified OpenAI-compatible API (over the WG interface)
  - Sends periodic heartbeats and status to the control plane
- A `labman` CLI for operators
- A clear, auditable, crate-based architecture matching the design docs.

---

## Stage 0 — Baseline & Foundations (current)

**Status:** Partially done

**Existing pieces:**

- [x] Workspace set up with crates:
  - [x] `labman-core` – shared types (endpoint, node, error)
  - Placeholders for:
    - [x] `labman-config`
    - [ ] `labman-wireguard`
    - [ ] `labman-endpoints`
    - [ ] `labman-proxy`
    - [ ] `labman-client`
    - [x] `labman-telemetry`
    - [x] `labmand` (`bin/labman-daemon`)
    - [ ] `labman` (`bin/labman-cli`)
- [x] `labman-core` has:
  - [x] `LabmanError` and `Result<T>`
  - [x] Endpoint types: `Endpoint`, `EndpointHealth`, `ModelDescriptor`, `ModelListResponse`
  - [x] Node types: `NodeInfo`, `NodeCapabilities`, `NodeStatus`, `NodeState`, registration/heartbeat messages

**Goals for this stage:**

- [ ] Ensure the workspace builds successfully with `labman-core` as-is:
  - `cargo check -p labman-core`
  - `cargo test -p labman-core`

- [ ] Define minimal cross-crate dependency graph (no implementation yet):
  - [x] `labman-config` depends on `labman-core`
  - [ ] `labman-wireguard` depends on `labman-core`
  - [x] `labman-endpoints` depends on `labman-core`
  - [x] `labman-proxy` depends on `labman-core`, `labman-endpoints`
  - [ ] `labman-client` depends on `labman-core`
  - [x] `labman-telemetry` depends on `tracing`, `tracing-subscriber`
  - [x] `labmand` depends on all above crates
  - [ ] `labman` (CLI) depends on `labman-core`, `labman-config`, `labman-client` (eventually)

**Exit criteria:**

- [ ] Workspace compiles with stubbed crates (even if most functions are `todo!()`) and tests in `labman-core` pass.
- [x] Crate APIs are sketched enough to be called from the daemon in later stages.

---

## Stage 1 — Configuration Layer (`labman-config`)

**Objective:** Implement robust configuration loading and validation, so the daemon can be configured solely via a TOML file (e.g. `/etc/labman/labman.toml`) plus minimal environment variables.

### 1.1 Types & File Loading

- [x] Implement `labman-config` with:
  - [x] `LabmanConfig` root struct:
    - [x] `control_plane`:
      - [x] `base_url`
      - [x] `node_token`
      - [x] optional `region`, `description`
    - [x] `wireguard`:
      - [x] `interface_name` (default: `labman0`)
      - [x] `address` (optional; or derived from control plane registration)
      - [x] `private_key_path`, `public_key_path`
      - [x] `peer_endpoint` (control-plane WG endpoint)
      - [x] `allowed_ips`
      - [x] Rosenpass-related fields (key paths, peer pk, etc.)
    - [x] `proxy`:
      - [x] `listen_port` (default `8080`)
      - [x] optional `listen_addr` override (but constrained to WG address later)
    - [x] `endpoints`: `Vec<EndpointConfig>`
  - [x] `EndpointConfig`:
    - [x] `name: String`
    - [x] `base_url: String`
    - [x] `max_concurrent: Option<usize>`
    - [x] `models_include: Option<Vec<String>>`
    - [x] `models_exclude: Option<Vec<String>>`

- [x] Implement:
  - [x] `fn load_from_path<P: AsRef<Path>>(path: P) -> Result<LabmanConfig>` using `toml` and `serde`.
  - [x] `fn load_default() -> Result<LabmanConfig>`:
    - [x] Looks at common locations (`/etc/labman/labman.toml`, `./labman.toml`).
    - [x] Does not rely on labman-specific environment variables.

### 1.2 Validation & Normalisation

- [x] Add methods on `LabmanConfig`:
  - [x] `fn validate(&self) -> Result<()>` that checks:
    - [x] Required fields present (node token, control_plane base URL).
    - [x] Endpoint URLs are valid HTTP/HTTPS and end in `/v1` or can be normalised.
    - [x] No duplicate endpoint names.
    - [ ] WireGuard keys paths exist (or are clearly marked as required outputs of onboarding).
    - [ ] `allowed_ips` does not include LAN ranges unless explicitly allowed (warn-level only, but never adds routes itself).
  - [x] `fn to_node_info(&self, capabilities: NodeCapabilities) -> NodeInfo`:
    - [x] Fill node description/region from config.
  - [ ] Helper for building `labman-core::Endpoint` from `EndpointConfig`.

### 1.3 Integration with Daemon (stub)

- [x] In `labmand`:
  - [x] Wire up main:
    - [x] Initialise telemetry/logging via `labman-telemetry`.
    - [x] Load config (with `--config`/`-c` argument + default search order).
    - [x] Log a summary (endpoints, WG iface name placeholder, control plane URL).
    - [x] Start HTTP server (`labman-server`) with `/metrics`.
    - [x] Build `EndpointRegistry` from config and perform initial health + model discovery passes.

**Exit criteria:**

- [x] `labman-config` has tests:
  - [x] Loads an example TOML config.
  - [x] Fails on malformed configs with `LabmanError::Config` / `LabmanError::InvalidConfig`.
- [x] `labmand` binary can be run with `--config` and:
  - [x] Prints parsed configuration summary.
  - [x] Validates configuration.
  - [x] Starts the HTTP server (including `/metrics`) using `labman-server`.

---

## Stage 2 — Telemetry & Observability (`labman-telemetry`)

**Objective:** Provide consistent logging/tracing used by all crates and ensure operator and control-plane observability.

### 2.1 Telemetry Crate

- [x] Implement `labman-telemetry` with:
  - [x] `fn init(level: Option<&str>) -> Result<()>`:
    - [x] Sets up `tracing_subscriber` with `env_filter`.
    - [x] Formats logs with timestamps, log level, and crate/module.
  - [ ] Optional features for JSON logging (for container environments).

### 2.2 Metrics Hooks (Future-proofing)

- [x] Define an internal interface for metrics export (Prometheus-backed):
  - [x] Traits for counters/gauges/histograms and a `MetricsRecorder` trait.
  - [x] Concrete `PrometheusMetricsRecorder` with shared `Registry`.
  - [x] HTTP helper `prometheus_http_response` for `/metrics`.

### 2.3 Integration

- [x] Use `labman-telemetry::init` at the start of `labmand::main`.
- [ ] Ensure `labman-core` uses `tracing` for notable events (model discovery issues, endpoint health changes) where appropriate, but keep `labman-core` mostly logging-free to remain generic.

**Exit criteria:**

- [x] `labmand` starts with telemetry initialised.
- [x] Logs show meaningful messages when configuration load succeeds/fails.
- [x] Unit tests for telemetry are minimal (ensure `init` doesn’t panic).

---

## Stage 3 — WireGuard + Rosenpass Layer (`labman-wireguard`)

**Objective:** Implement the secure networking foundation as per the security model and architecture.

### 3.1 WireGuard Interface Abstraction

- [ ] Implement `labman-wireguard` with a minimal, testable abstraction:

  - Core types:
    - `WireGuardConfig` (created from `LabmanConfig` + control-plane registration response).
    - `WireGuardInterface` struct with:
      - `name: String` (default `labman0`).
      - `address: String` (e.g. `10.90.x.y/32`).
      - `peer_endpoint: String`.
      - `allowed_ips: Vec<String>`.
    - `RosenpassConfig` for PQ key exchange.

  - Core operations:
    - `fn create_interface(config: &WireGuardConfig) -> Result<WireGuardInterface>`
    - `fn bring_up(interface: &WireGuardInterface) -> Result<()>`
    - `fn bring_down(interface: &WireGuardInterface) -> Result<()>`
    - `fn status(interface_name: &str) -> Result<InterfaceStatus>`

- These should use netlink / platform APIs directly where possible, or in a first iteration:
  - Implement a *mockable* trait (`WireGuardBackend`) so that initial versions can use shell commands in dev mode and later be replaced by a pure-Rust, Rosenpass-native backend.

### 3.2 Rosenpass Integration

- [ ] Introduce a trait or façade for Rosenpass integration:

  - `trait RosenpassEngine`:
    - `fn init(&self, cfg: &RosenpassConfig) -> Result<RosenpassSession>`
    - `fn derive_wireguard_keys(&self, session: &RosenpassSession) -> Result<(String, String)>` (private/public or session keys).
  - For initial implementation, stub this out while designing the interface and types.
  - Later replace with actual Rosenpass Rust library integration.

### 3.3 Security Invariants

- [ ] Implement safeguards in `labman-wireguard` and/or `labmand`:

  - Ensure:
    - Interface is created as `/32` address (no routing for LANs).
    - IP forwarding is not enabled by this daemon.
    - No NAT/iptables manipulation is performed.
    - `listen_addr` for proxy is bound only to the WG address (enforced later in proxy stage).

- Provide defensive checks:
  - `fn validate_control_plane_allowed_ips(allowed_ips: &[String]) -> Result<()>`:
    - Log warnings for suspicious entries (e.g., RFC1918 ranges).
    - Do **not** add any extra routes based on them.

### 3.4 Daemon Integration

- [ ] In `labmand`:
  - [ ] After config load, call into `labman-wireguard` to initialise the interface (with temporary keys from config).
  - [ ] Log resulting WG IP address.
  - [ ] Integrate WG lifecycle with daemon startup/shutdown.

**Exit criteria:**

- [ ] `labmand` can:
  - [x] Load config.
  - [ ] Bring up a WG interface (even if using a simple backend).
  - [ ] Tear it down cleanly on shutdown or error paths.
- [ ] Integration tests (VM/CI environment permitting) for WG creation and reachability from localhost.

---

## Stage 4 — Endpoint Management & Health (`labman-endpoints`)

**Objective:** Implement the in-LAN endpoint registry, health checks, and model discovery / filtering.

### 4.1 Endpoint Registry

- [x] Implement `labman-endpoints` with:
  - [x] `EndpointRegistry` struct:
    - [x] Stores a collection of `labman-core::Endpoint`.
    - [x] Indexes by model name for fast lookup (via a derived model index).
    - [x] Tracks per-endpoint concurrency limits and active request counts.
  - [x] Initialization:
    - [x] `fn from_config(config: &LabmanConfig) -> Result<EndpointRegistry>`:
      - [x] Convert `EndpointConfig` to `Endpoint` with initial health and metadata.
      - [x] Store `max_concurrent` and model filters in registry metadata.

### 4.2 Health Checks

- [x] Implement periodic health checking:

  - [x] `fn health_check_all(&mut self) -> Result<()>`:
    - [x] For each endpoint, mark as healthy (synchronous stub retained for simple callers).
  - [x] `async fn health_check_all_http(&mut self) -> Result<()>`:
    - [x] For each endpoint:
      - [x] Perform an HTTP request to `base_url`.
      - [x] On 2xx: mark healthy.
      - [x] On non-2xx or error: mark unhealthy and emit metrics/logs.

  - [x] Add a background task interface:
    - [x] `fn spawn_periodic_health_check(registry: Arc<tokio::sync::Mutex<EndpointRegistry>>, interval: Duration, shutdown: S)`.
    - [x] Uses `tokio` for async runtime and runs until shutdown.

### 4.3 Model Discovery & Filtering

- [x] Implement model discovery logic consistent with `architecture.md` (first pass):

  - [x] For each healthy endpoint:
    - [x] Call `GET {base_url}/models` or `/v1/models` (OpenAI format).
    - [x] Parse into `ModelListResponse` and update `EndpointEntry.discovered_models`.

  - [x] Apply filtering rules from config:
    - [x] `models.include` (glob-based allowlist).
    - [x] `models.exclude` (glob-based denylist).
    - [x] If both present: include first, then exclude.
  - [x] Maintain:
    - [x] Map `model_name -> Vec<EndpointName>` for scheduling.

### 4.4 Scheduling / Selection Algorithm

- [x] Implement model-aware routing (initial skeleton):

  - `fn select_endpoint_for_model(&self, model: &str) -> Option<(&String, &EndpointEntry)>`:
    - Filter endpoints:
      - Healthy only.
      - Support the model.
      - Respect `max_concurrent` (using current active requests).
    - Use a simple algorithm first:
      - Currently returns the first endpoint advertising the model.
      - Future work: round-robin or lowest active request count.
  - On selection:
    - Increment the active request count.
    - Provide a guard type (RAII) to decrement active count when request completes.

### 4.5 Control-Plane Capabilities View

- [x] Provide a function to convert the registry into `NodeCapabilities`:

  - `fn to_node_capabilities(&self) -> NodeCapabilities`:
    - [x] Flatten all unique models.
    - [x] `endpoint_count` = total endpoints.
    - [x] `max_concurrent_requests` = sum of `max_concurrent` (or heuristic).

### 4.6 Daemon Integration

- [x] In `labmand`:
  - [x] After config (WG pending):
    - [x] Instantiate `EndpointRegistry` from config via `EndpointRegistryBuilder` with shared metrics.
    - [x] Kick off:
      - [x] Initial HTTP-based health check & model discovery.
      - [x] Periodic health checker + model discovery loop in the main Tokio runtime.
    - [x] Keep daemon alive by running the HTTP server and background tasks in the same runtime.

**Exit criteria:**

- [ ] Unit tests for:
  - [ ] Model discovery and filtering.
  - [ ] Endpoint selection logic.
  - [ ] Health status transitions.
- [ ] Simple integration test using a mocked local HTTP server.

(Note: Core Stage 4 functionality (registry, health checks, model discovery, scheduling, and integration with `labmand` and `labman-proxy`) is implemented and exercised manually against real endpoints; automated tests remain to be added for full exit criteria.)

---

## Stage 5 — Proxy Layer (`labman-proxy`)

**Objective:** Expose a unified OpenAI-compatible API over the WireGuard interface, resolving opaque, control-plane provided model slugs to specific local endpoints and models.

### 5.1 HTTP Server Skeleton

- [x] Implement `labman-proxy` crate with initial HTTP server skeleton:

  - [x] Expose a `/v1/models` route backed by `EndpointRegistry::to_node_capabilities().models`.
  - [x] Wire proxy HTTP listener into `labmand` on a local address/port (currently 127.0.0.1 + `proxy.listen_port`; to be moved to the WG-bound address in Stage 3/5 integration).
  - [x] Add `POST /v1/chat/completions`
  - [ ] Add `POST /v1/completions`

- [ ] Ensure:
  - [ ] Binding is restricted to WG IP/port (address from `WireGuardInterface`).
  - [ ] No binding to `0.0.0.0` or LAN interfaces.

### 5.2 Request Handling

- [x] For `/v1/models`:
  - [x] Return aggregated model list from `EndpointRegistry::to_node_capabilities().models` in OpenAI `list` format for OpenAI compatibility, while providing richer per-endpoint/per-tenant model data to the control plane via a separate API.

- [x] For `/v1/chat/completions`:
  - [x] Parse incoming OpenAI-compatible request body:
    - [x] Extract `model` field (treated as an opaque control-plane provided slug).
    - [x] Handle both streaming (`stream: true`) and non-streaming.
  - [x] Use `EndpointRegistry`’s slug index to resolve the opaque `model` slug into a concrete `(tenant, endpoint_name, model_id)` triple selected by the control plane.
  - [x] Rewrite the upstream request so that the selected endpoint sees the concrete `model_id` it understands.
  - [x] Forward HTTP request to `endpoint.base_url`:
    - [x] Transform path if necessary (current implementation appends `/chat/completions` to the configured base URL, which is expected to include `/v1`).
    - [x] Stream response back to caller.
  - [x] Handle:
    - [x] Upstream connection and body-read errors mapped to appropriate HTTP status codes.
- [ ] For `/v1/completions`:
  - [ ] Implement similar request handling and forwarding as `chat/completions`.

- Provide clear error surfaces:
  - If no endpoint has the model: return `LabmanError::ModelNotFound`.
  - If all endpoints with the model are unhealthy or overloaded: 503 with relevant error.

### 5.3 Streaming Support

- [x] Implement streaming (SSE-style or chunked JSON lines as per OpenAI):

  - [x] Proxy streaming responses from local endpoints to upstream client by piping the upstream byte stream.
  - [ ] Carefully handle backpressure and cancellation:
    - [ ] Cancellation should decrement active count on endpoint.
    - [ ] Partially streamed responses should be logged appropriately.

### 5.4 Telemetry

- [ ] Use `tracing` to log:
  - [ ] Request start and end (per request ID).
  - [ ] Endpoint selection decision.
  - [x] Errors and upstream failures/timeouts in proxy handlers.

### 5.5 Daemon Integration

- [ ] In `labmand`:
  - [ ] After WG + endpoints:
    - [ ] Obtain WG IP from `WireGuardInterface`.
    - [ ] Derive `listen_addr = (wg_ip, config.proxy.listen_port)`.
    - [x] Start proxy server with graceful shutdown support, currently bound to `127.0.0.1:config.proxy.listen_port` (to be switched to the WG IP once Stage 3 is implemented).

**Exit criteria:**

- [ ] `labmand` can:
  - [ ] Bring up WG.
  - [ ] Register endpoints.
  - [ ] Serve `/v1/models` and `chat/completions` over the WG address.
- Manual test:
  - Simulate control-plane with `curl`/HTTP client to WG IP.
  - Observe requests routed to a local test endpoint.

---

## Stage 6 — Control-Plane Client (`labman-client`)

**Objective:** Implement node registration, capability sync, and heartbeat with the control plane.

### 6.1 Registration

- [ ] Implement `labman-client` with:

  - `ControlPlaneClient` struct:
    - Contains `base_url`, `node_token`, HTTP client.

  - `async fn register_node(&self, info: NodeInfo, wg_pub: String, rp_pub: String) -> Result<RegistrationResponse>`:
    - Sends `RegistrationRequest` to control-plane endpoint (e.g., `/api/nodes/register`).
    - Receives `RegistrationResponse` with:
      - `node_id`
      - `wireguard_address`
      - Possibly additional settings.

  - Integrate with `labman-wireguard`:
    - Use `wireguard_address` from registration response to finalise WG interface configuration.

### 6.2 Heartbeat

- [ ] Implement heartbeat loop:

  - `async fn send_heartbeat(&self, heartbeat: HeartbeatRequest) -> Result<HeartbeatResponse>`.

  - In `labmand`:
    - Construct `NodeStatus` periodically using:
      - Endpoint health.
      - Request counters from `labman-proxy`.
      - Uptime.
    - Use `EndpointRegistry::to_node_capabilities` when models or endpoints change, to update capabilities in heartbeat.

  - Handle requested state changes:
    - `NodeState::Maintenance`, `NodeState::Stopping`, etc.
    - For initial version:
      - Log state change instructions.
      - Optionally reduce scheduling if in maintenance mode.

### 6.3 Error Handling & Retries

- [ ] For control-plane errors:
  - Use retry/backoff for transient errors (`LabmanError::is_transient`).
  - On authentication failures or fatal errors:
    - Log and set node state to `Error`.
    - Optionally trigger graceful shutdown.

**Exit criteria:**

- [ ] `labmand`:
  - [ ] Registers node at startup with control-plane.
  - [ ] Receives WG address, configures interface.
  - [ ] Starts periodic heartbeats with up-to-date status.
- [ ] Tests with a mock control-plane HTTP server.

---

## Stage 7 — Daemon Orchestration (`labmand`)

**Objective:** Glue everything together into a single robust daemon process with proper lifecycle handling.

### 7.1 Startup Sequence

- [ ] In `labmand` main:

1. Initialise telemetry.
2. Parse CLI arguments (config path, log level, maybe `--foreground`).
3. Load configuration via `labman-config`.
4. Build initial `EndpointRegistry` from config (endpoints unknown health).
5. Prepare initial `NodeCapabilities` from endpoints (empty/models unknown is OK).
6. Build `NodeInfo` from config + capabilities.
7. Generate or load WG/Rosenpass keys if not present (future enhancement; for now assume present).
8. Register with control-plane using `labman-client`:
   - Receive `node_id` and `wireguard_address`.
9. Create WireGuard interface via `labman-wireguard` using registration data.
10. Start:
    - Endpoint health/model discovery loop.
    - Heartbeat loop.
    - Proxy server bound to WG IP and configured port.
11. Wait on shutdown signal (SIGINT/SIGTERM).

### 7.2 Shutdown Sequence

- [ ] On shutdown signal or fatal error:

  - Stop accepting new requests (proxy server stop).
  - Wait for in-flight requests to complete (within timeout).
  - Send final heartbeat with `NodeState::Stopping`.
  - Bring down WG interface.
  - Exit with appropriate status.

### 7.3 Resilience

- Ensure:
  - Individual component failures are handled gracefully:
    - Endpoint health check failures do not crash daemon.
    - Temporary control-plane outages do not break proxying (just skip heartbeat until recovered).
  - Logging clearly explains any degraded state.

**Exit criteria:**

- `labmand` behaves like a real daemon:
  - Start / stop reliably.
  - Survives transient network issues.
  - Honors config updates (initially only on restart; hot-reload can be future work).

---

## Stage 8 — Protocol & WebSocket Integration (Portman ⇄ labman ⇄ Conplane)

**Objective:** Implement the messaging protocol defined in `protocol.md` over WebSockets, with labman acting as the proxy between Portman (downstream) and Conplane (upstream).

This stage turns labman from an HTTP-only control-plane client into a full **WS protocol relay** implementing:

- The common **envelope** format
- Portman → Conplane upstream messages
- Conplane → Portman downstream directives
- Robust connection management and routing behavior as described in `protocol.md` (esp. Section 6, “Labman Behaviour”).

### 8.1 Protocol Types / Crate

- [ ] Introduce a dedicated protocol module or crate (e.g. `labman-protocol` or `labman-core::protocol`) that defines:

  - [ ] `Envelope` type matching the spec in `protocol.md`:
    - `msg_id`
    - `site_id`
    - `agent_id`
    - `direction`
    - `kind`
    - `ts`
    - `payload` (generic or enum)

  - [ ] Strongly-typed payload structs for key message families (names following `protocol.md`):

    - Agent → Conplane (upstream):
      - [ ] `RegisterAgent`
      - [ ] `Heartbeat`
      - [ ] `Metrics`
      - [ ] `OfferCapacity`
      - [ ] `DirectiveProgress`
      - [ ] `UsageReport`
      - [ ] Per-model capacity hints (`ResourceProfiles` / `AvailableModelCapacity`)

    - Conplane → Agent (downstream):
      - [ ] `PreloadModel`
      - [ ] `EvictModel`
      - [ ] `AssignWorkload`
      - [ ] `UpdateRegistry`
      - [ ] Administrative directives:
        - [ ] `Drain`
        - [ ] `RestartAgent`

    - Shared response types:
      - [ ] `Ack`
      - [ ] `Error`

  - [ ] A `MessageKind` enum or similar that maps exactly to the `kind` strings in `protocol.md`.

  - [ ] Serialization/deserialization that is:
    - [ ] Strict about unknown fields and invalid enums where appropriate.
    - [ ] Forward-compatible enough to ignore optional extensions if `protocol.md` permits.

### 8.2 Downstream WS (Portman → labman)

- [ ] Implement a Portman-facing WebSocket server crate (or module), e.g. `labman-ws-portman`, which:

  - [ ] Listens on a local-only address/port (e.g. `127.0.0.1:<port>`), never on the WireGuard or public interfaces.
  - [ ] Exposes a single WS endpoint (e.g. `/agent`) that Portman connects to.
  - [ ] Accepts and validates incoming `Envelope` frames from Portman:
    - [ ] Decodes JSON into typed `Envelope<T>`.
    - [ ] Performs basic validation (e.g., `direction == "upstream"`, `agent_id` present).
  - [ ] Forwards accepted upstream messages into an internal routing channel in labman for delivery to Conplane.
  - [ ] Sends downstream messages from Conplane to Portman over the same WS connection.

- [ ] Connection lifecycle behavior (per `protocol.md` Section 6.2):

  - [ ] Support initial registration (`RegisterAgent`) on first connect.
  - [ ] Handle disconnects from Portman:
    - [ ] Mark agent as disconnected.
    - [ ] Optionally emit a synthetic “disconnected” notification to Conplane if specified in `protocol.md`.
  - [ ] Implement basic backpressure / buffering limits to avoid unbounded queues.

### 8.3 Upstream WS (labman → Conplane)

- [ ] Extend `labman-client` to support a persistent WebSocket connection to Conplane:

  - [ ] Connect to configured Conplane WS endpoint (over WireGuard).
  - [ ] Authenticate as required (e.g., bearer token, headers) per control-plane contract.
  - [ ] Send and receive `Envelope` frames using the same protocol types as above.
  - [ ] Implement reconnect behavior:
    - [ ] Exponential backoff with jitter.
    - [ ] Limits and logging for repeated failures.

- [ ] Message routing responsibilities (per `protocol.md` Section 6.1 and 6.3):

  - [ ] Forward all valid Portman-originated upstream messages to Conplane:
    - `RegisterAgent`, `Heartbeat`, `Metrics`, `OfferCapacity`, `DirectiveProgress`, `UsageReport`, capacity hints, etc.
  - [ ] Receive directives and other downstream messages from Conplane and route them:
    - [ ] To Portman via the downstream WS server (if connected).
    - [ ] Handle missing or disconnected Portman:
      - [ ] Queue or reject with appropriate `Error` / `Ack` semantics per `protocol.md`.

### 8.4 Message Routing & Correlation

- [ ] Implement a routing layer inside `labmand` that:

  - [ ] Maintains mapping between:
    - `agent_id` (Portman identity)
    - Active WS connections (Portman side)
    - Registration / state derived from `RegisterAgent`/`Heartbeat`.

  - [ ] Correlates messages using:
    - `msg_id` and `directive_id` where applicable, so that:
      - [ ] Acks/Errors can be matched to originating directives.
      - [ ] Progress updates can be tied back to specific directives.

  - [ ] Enforces directionality:
    - [ ] Upstream-only kinds from Portman are never accepted from Conplane, and vice versa.

- [ ] Define internal error handling semantics:

  - [ ] Malformed envelopes:
    - [ ] Reject with an `Error` message where the protocol permits.
    - [ ] Optionally close the offending WS connection on repeated failures.
  - [ ] Unknown `kind`:
    - [ ] Log and ignore, or respond with a typed `Error`, according to `protocol.md` guidance.

### 8.5 Integration With Existing Components

- [ ] Connect protocol messages to existing labman functionality:

  - [ ] Map `RegisterAgent` payloads to labman’s view of node capabilities and endpoints where appropriate.
  - [ ] Use `Heartbeat` and `Metrics` messages to update internal telemetry / health data.
  - [ ] Use directives:
    - [ ] `PreloadModel` / `EvictModel` / `AssignWorkload` to drive interactions with Portman and influence local routing decisions or pre-warm behavior.
    - [ ] Administrative directives (`Drain`, `RestartAgent`) to coordinate shutdown / maintenance flows at the Portman layer (labman itself remains a pure proxy).

- [ ] Ensure the OpenAI HTTP proxy (`labman-proxy`) exports enough metrics/usage information for Portman to construct accurate `UsageReport` / capacity hints messages, or for labman to help aggregate that information if the protocol evolves in that direction.

**Exit criteria:**

- [ ] A dedicated protocol module/crate with strongly-typed message definitions that round-trip to/from the JSON described in `protocol.md`.
- [ ] A Portman-facing WS server that accepts and forwards upstream messages and delivers downstream directives.
- [ ] A Conplane-facing WS client that connects over WireGuard and exchanges protocol messages.
- [ ] A routing layer in `labmand` that cleanly connects Portman and Conplane per the protocol (including basic error and reconnect behavior).
- [ ] Manual test:
  - [ ] Portman connects to labman via WS, sends `RegisterAgent` and `Heartbeat`.
  - [ ] A test Conplane WS server receives them and responds with a directive (`PreloadModel`).
  - [ ] Portman receives the directive via labman and can acknowledge it.

---

## Stage 9 — CLI Tool (`labman`)

**Objective:** Provide a minimal, purpose-built CLI that integrates cleanly with `labmand` and systemd, focusing on installation and lifecycle hooks rather than a rich interactive UI.

The CLI will primarily be used in:

- Operator workflows to install/update labmand and systemd units.
- Systemd `ExecStartPre` and `ExecStopPost` (or similar) hooks to run pre/post steps.

### 9.1 Core Subcommands

Implement `labman` with `clap`, providing at least the following subcommands:

- `labman install`
  - [ ] Install or update the `labmand` binary into a target location (e.g., `/usr/local/bin` by default, configurable via flags).
  - [ ] Install or update systemd unit files for `labmand` (e.g., `/etc/systemd/system/labmand.service`).
  - [ ] Optionally perform basic validation (e.g., check that `labman-config` can find a config file).
  - [ ] Be idempotent where possible: re-running `install` should not break existing setups.

- `labman update`
  - [ ] Update `labmand` and systemd units to the latest available version (from a release channel or local artifact, depending on distribution choice).
  - [ ] Optionally restart `labmand` (behind a `--restart` flag) once update is complete.
  - [ ] Log what changed (binary path, unit file diffs if any).

- `labman pre`
  - Designed to be invoked from `labmand`’s `ExecStartPre`.
  - [ ] Perform any necessary pre-start steps, for example:
    - [ ] Validate configuration file presence and correctness.
    - [ ] Ensure directories and permissions are correct (e.g., `/var/lib/labman`, `/var/log/labman`).
    - [ ] Optionally run lightweight checks on external dependencies (WireGuard tools availability, if relevant for current implementation).
  - [ ] Exit non-zero with a clear, actionable error message if preconditions fail so systemd will not start `labmand`.

- `labman post`
  - Designed to be invoked from `labmand`’s `ExecStopPost` or equivalent post-stop hook.
  - [ ] Perform teardown or cleanup steps after `labmand` stops, for example:
    - [ ] Remove temporary files, sockets, or stale state.
    - [ ] Optionally clean up transient artifacts related to WireGuard or Rosenpass that are not managed automatically.
  - [ ] Exit status should reflect whether cleanup completed successfully, while being conservative about not leaving the system in a worse state.

### 9.2 CLI Design Notes

- [ ] Follow the conventions in `agents.md`:
  - Long-form flags in docs (e.g., `--unit-dir`, `--bin-dir`).
  - No labman-specific environment variables for core behavior.
- [ ] Ensure subcommands are:
  - [ ] Non-interactive by default (suitable for automation and systemd hooks).
  - [ ] Verbose enough in logs and stdout/stderr to aid debugging.

**Exit criteria:**

- [ ] `labman` binary builds and provides at least:
  - [ ] `install` and `update` for managing `labmand` and systemd units.
  - [ ] `pre` and `post` suitable for use in `ExecStartPre`/`ExecStopPost` contexts.
- [ ] Example systemd unit files in the repo use:
  - [ ] `ExecStartPre=/usr/bin/labman pre`
  - [ ] `ExecStopPost=/usr/bin/labman post`
- [ ] Documentation in `readme.md` and/or a dedicated CLI section describes these subcommands and their usage clearly.

---

## Stage 9 — Hardening, Testing & Operator Experience

**Objective:** Move from “works” to “robust and operator-friendly”.

### 9.1 Testing & QA

- [ ] Add:
  - Unit tests across all crates.
  - Integration tests:
    - Fake control-plane.
    - Fake local endpoints.
    - Verify complete request lifecycle.
  - Property tests for model filtering and scheduling.

### 9.2 Security Review

- [ ] Validate the security invariants described in `architecture.md`:

  - No LAN exposure through WG interface.
  - Only proxy binds to WG address.
  - No remote execution, no filesystem access from control-plane.
  - Minimal metadata in registration/heartbeat by default.

- Document:
  - Threat model and how code enforces it.
  - Operator guidance on firewall rules around `labman0`.

### 9.3 Documentation & UX

- [ ] Update:
  - `readme.md` with accurate quick start steps.
  - `architecture.md` sections with references to code modules where applicable.
  - A sample `labman.toml` reflecting all config fields and examples.

- [ ] Ensure:
  - Error messages are actionable and clear (e.g., “cannot connect to endpoint `ollama-local` at `http://127.0.0.1:11434/v1`: connection refused — is Ollama running?”).

---

## Stage 10 — Extensibility & Future Enhancements

These are beyond “first complete implementation” but align with the architecture:

- [ ] Support for:
  - More endpoint types / protocols.
  - Benchmark-based scheduling (latency, VRAM, cost).
  - Node mode controls via control-plane (maintenance, drain).
  - Integration with homelab dashboards / metrics exporters.
- Advanced configuration:
  - Per-model routing preferences.
  - Resource-aware scheduling (GPU load, VRAM usage).

---

## Summary

By following these stages in order:

1. Config & telemetry foundation.
2. WireGuard + Rosenpass secure tunnel.
3. Endpoint registry, health checks, and model discovery.
4. OpenAI-compatible proxy over WG.
5. Control-plane registration and heartbeat.
6. Daemon orchestration and lifecycle.
7. Operator CLI and UX polish.
8. Hardening and extensibility.

…the project will evolve from its current partial state into a fully functional, secure, operator-friendly daemon that matches the design described in the existing `readme` and `architecture` documents.