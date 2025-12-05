use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    sync::{Arc, RwLock},
};

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        ConnectInfo, State,
    },
    response::IntoResponse,
    routing::get,
    Router,
};
use futures::{future::BoxFuture, Future, SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tracing::{error, info, warn};

/// Minimal error type alias for now; we’ll likely switch to `labman_core::LabmanError`
/// once this crate is wired into the rest of the system.
pub type Result<T> = std::result::Result<T, anyhow::Error>;

/// Direction of a protocol envelope, as seen on the wire.
///
/// Matches the `direction` field in `protocol.md`:
/// - "up" / "upstream"   — Agent/Portman → Conplane
/// - "down" / "downstream" — Conplane → Agent/Portman
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    #[serde(alias = "upstream")]
    Up,
    #[serde(alias = "downstream")]
    Down,
}

/// High-level kind of message as described in `protocol.md`.
///
/// For the first iteration we only model a subset and fall back to `Unknown`
/// for anything we don’t explicitly understand.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MessageKind {
    // Agent → Conplane
    RegisterAgent,
    Heartbeat,
    Metrics,
    OfferCapacity,
    DirectiveProgress,
    UsageReport,
    ResourceProfiles,
    AvailableModelCapacity,

    // Conplane → Agent
    PreloadModel,
    EvictModel,
    AssignWorkload,
    UpdateRegistry,
    Drain,
    RestartAgent,

    // Ack / Error
    Ack,
    Error,

    /// Any other kind we don't recognise yet.
    #[serde(other)]
    Unknown,
}

/// Minimal envelope type for messages between Portman and Conplane.
///
/// This mirrors the top-level structure in `protocol.md`. For now we keep the
/// payload as raw JSON so that we don’t need to model every variant up front.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub msg_id: String,
    #[serde(default)]
    pub site_id: Option<String>,
    #[serde(default)]
    pub agent_id: Option<String>,
    pub direction: Direction,
    pub kind: MessageKind,
    #[serde(default)]
    pub ts: Option<String>,
    #[serde(default)]
    pub payload: serde_json::Value,
}

/// Configuration for the Portman-facing WebSocket server.
///
/// In the first iteration we bind explicitly to a loopback address, and
/// expose a single `/agent` endpoint that Portman connects to, plus a
/// separate `/observe` endpoint for operator/observer clients.
#[derive(Clone, Debug)]
pub struct PortmanWsConfig {
    /// Address to bind the WS server to, e.g. `127.0.0.1:9100`.
    pub bind_addr: SocketAddr,
}

/// A very small record of a connected Portman subscriber.
///
/// This is intentionally minimal for the first iteration; we can extend it
/// later with protocol-level identity once we start decoding envelopes.
#[derive(Clone, Debug)]
pub struct PortmanSubscriber {
    /// Remote socket address as observed by the WS server.
    pub peer_addr: SocketAddr,
    /// Monotonic ID assigned by labman (not the protocol `agent_id`).
    pub connection_id: u64,
    /// Optional agent identity once a RegisterAgent envelope has been seen.
    pub agent_id: Option<String>,
}

/// In-memory registry of currently connected Portman subscribers.
///
/// This will be used later by an introspection interface so operators can
/// see which Portman daemons are connected and healthy.
///
/// NOTE: This is not yet aware of protocol-level agent identity; it simply
/// tracks active WS connections. Once we parse `RegisterAgent` envelopes,
/// we can map connections to `agent_id`s and enrich this registry.
#[derive(Debug, Default)]
pub struct PortmanSubscribers {
    inner: RwLock<InnerSubscribers>,
}

#[derive(Debug, Default)]
struct InnerSubscribers {
    next_id: u64,
    connections: HashMap<u64, PortmanSubscriber>,
}

impl PortmanSubscribers {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a newly connected Portman WS client.
    pub fn add(&self, peer_addr: SocketAddr) -> PortmanSubscriber {
        let mut inner = self.inner.write().expect("PortmanSubscribers poisoned");
        let id = inner.next_id;
        inner.next_id = inner.next_id.wrapping_add(1);

        let sub = PortmanSubscriber {
            peer_addr,
            connection_id: id,
            agent_id: None,
        };
        inner.connections.insert(id, sub.clone());
        sub
    }

    /// Update the agent_id for a given connection, if present.
    pub fn set_agent_id(&self, connection_id: u64, agent_id: String) {
        let mut inner = self.inner.write().expect("PortmanSubscribers poisoned");
        if let Some(sub) = inner.connections.get_mut(&connection_id) {
            sub.agent_id = Some(agent_id);
        }
    }

    /// Remove a subscriber by connection id, returning it if present.
    pub fn remove(&self, connection_id: u64) -> Option<PortmanSubscriber> {
        let mut inner = self.inner.write().expect("PortmanSubscribers poisoned");
        inner.connections.remove(&connection_id)
    }

    /// Snapshot of all current subscribers.
    pub fn list(&self) -> Vec<PortmanSubscriber> {
        let inner = self.inner.read().expect("PortmanSubscribers poisoned");
        inner.connections.values().cloned().collect()
    }

    /// Current count of active subscribers.
    pub fn len(&self) -> usize {
        let inner = self.inner.read().expect("PortmanSubscribers poisoned");
        inner.connections.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Stream kinds that /observe clients can subscribe to.
///
/// This is intentionally minimal for now; we’ll add more variants as needed.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum StreamKind {
    /// Subscribe to all protocol envelopes regardless of `kind`.
    All,
    /// Subscribe only to specific protocol `kind` values (e.g. `register_agent`,
    /// `heartbeat`, etc.). For now we use the same string space as `MessageKind`.
    ByKind,
}

/// Observer client subscription state.
///
/// Each observer connection may subscribe to zero or more stream kinds and,
/// optionally, a set of concrete protocol `kind` strings when using the
/// `ByKind` stream.
#[derive(Debug, Default, Clone)]
pub struct ObserverState {
    /// High-level stream selectors (All / ByKind).
    pub subscribed_kinds: HashSet<StreamKind>,
    /// Optional set of protocol `kind` strings when subscribed with `ByKind`.
    ///
    /// For example: ["register_agent", "heartbeat", "metrics"].
    pub kinds_filter: Option<HashSet<String>>,
}

/// Registry of active observer connections.
#[derive(Debug, Default)]
pub struct Observers {
    /// Per-observer subscription state.
    inner: RwLock<HashMap<u64, ObserverState>>,
    /// Per-observer WebSocket send handles used for broadcasting events.
    senders: RwLock<HashMap<u64, tokio::sync::mpsc::UnboundedSender<Message>>>,
}

impl Observers {
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(HashMap::new()),
            senders: RwLock::new(HashMap::new()),
        }
    }

    pub fn add(&self, connection_id: u64) {
        let mut inner = self.inner.write().expect("Observers poisoned");
        inner.insert(connection_id, ObserverState::default());
    }

    pub fn remove(&self, connection_id: u64) {
        let mut inner = self.inner.write().expect("Observers poisoned");
        inner.remove(&connection_id);

        let mut senders = self.senders.write().expect("Observers senders poisoned");
        senders.remove(&connection_id);
    }

    pub fn set_subscription(&self, connection_id: u64, kinds: HashSet<StreamKind>) {
        let mut inner = self.inner.write().expect("Observers poisoned");
        if let Some(state) = inner.get_mut(&connection_id) {
            state.subscribed_kinds = kinds;
        }
    }

    pub fn list(&self) -> HashMap<u64, ObserverState> {
        let inner = self.inner.read().expect("Observers poisoned");
        inner.clone()
    }

    /// Register a sender for a given observer connection.
    pub fn register_sender(
        &self,
        connection_id: u64,
        sender: tokio::sync::mpsc::UnboundedSender<Message>,
    ) {
        let mut senders = self.senders.write().expect("Observers senders poisoned");
        senders.insert(connection_id, sender);
    }

    /// Snapshot of all active senders.
    pub fn sender_snapshot(&self) -> HashMap<u64, tokio::sync::mpsc::UnboundedSender<Message>> {
        let senders = self.senders.read().expect("Observers senders poisoned");
        senders.clone()
    }
}

/// Commands an /observe client can send to control its subscription or
/// request discovery data.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum ObserveCommand {
    /// Subscribe to a set of stream kinds (replaces any existing subscription).
    ///
    /// When using the `by_kind` stream, the `kinds_filter` field controls which
    /// protocol `kind` values should be echoed:
    ///
    /// {
    ///   "command": "subscribe",
    ///   "kinds": ["all"]
    /// }
    ///
    /// {
    ///   "command": "subscribe",
    ///   "kinds": ["by_kind"],
    ///   "kinds_filter": ["register_agent", "heartbeat"]
    /// }
    Subscribe {
        kinds: Vec<StreamKind>,
        #[serde(default)]
        kinds_filter: Option<Vec<String>>,
    },
    /// Request discovery information about the current deployment view.
    Discover {
        #[serde(default)]
        what: Option<String>,
    },
}

/// State shared with the WS handlers.
///
/// For now this includes the Portman subscriber registry and a registry of
/// observer clients connected via `/observe`.
#[derive(Clone)]
struct AppState {
    subscribers: Arc<PortmanSubscribers>,
    observers: Arc<Observers>,
}

/// Start the Portman-facing WebSocket server and run it until `shutdown` resolves.
///
/// This is the main entrypoint that `labmand` will call:
///
/// - Binds a TCP listener on `config.bind_addr`
/// - Exposes a single `/agent` WS endpoint
/// - Tracks connections in `PortmanSubscribers`
/// - Logs incoming text frames and replies `"ok"`
///
/// `shutdown` is typically a future that resolves when the daemon is shutting
/// down (e.g. signal handler in `labmand`).
pub async fn run_portman_ws_server(
    config: PortmanWsConfig,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<Arc<PortmanSubscribers>> {
    let state = AppState {
        subscribers: Arc::new(PortmanSubscribers::new()),
        observers: Arc::new(Observers::new()),
    };

    // We use into_make_service_with_connect_info so that handlers using
    // `ConnectInfo<SocketAddr>` can extract the peer address.
    let app = Router::new()
        .route("/agent", get(handle_ws_upgrade_agent))
        .route("/observe", get(handle_ws_upgrade_observe))
        .with_state(state.clone())
        .into_make_service_with_connect_info::<SocketAddr>();

    let listener = TcpListener::bind(config.bind_addr).await?;
    info!(addr = %config.bind_addr, "Portman WS server listening");

    let subscribers = state.subscribers.clone();

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await
        .map_err(|e| {
            error!(error = %e, "Portman WS server terminated with error");
            anyhow::anyhow!(e)
        })?;

    Ok(subscribers)
}

/// HTTP handler that upgrades the connection to WebSocket for Portman agents
/// connecting on `/agent`.
async fn handle_ws_upgrade_agent(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    info!(%addr, "Incoming Portman WS connection");
    ws.on_upgrade(move |socket| handle_ws_connection(socket, state, addr))
}

/// HTTP handler that upgrades the connection to WebSocket for observer
/// clients connecting on `/observe`.
async fn handle_ws_upgrade_observe(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    info!(%addr, "Incoming observer WS connection");
    ws.on_upgrade(move |socket| handle_observer_connection(socket, state, addr))
}

/// Handle a single WebSocket connection from a Portman daemon.
///
/// For now this:
/// - registers the connection in `PortmanSubscribers`
/// - logs text frames
/// - responds with a simple `"ok"` message or error envelopes
async fn handle_ws_connection(socket: WebSocket, state: AppState, peer: SocketAddr) {
    let subscriber = state.subscribers.add(peer);
    info!(
        connection_id = subscriber.connection_id,
        %peer,
        "Portman WS connected"
    );

    let connection_id = subscriber.connection_id;
    let subscribers = state.subscribers.clone();
    let observers = state.observers.clone();

    if let Err(e) = drive_ws_connection(socket, peer, connection_id, subscribers, observers).await {
        warn!(
            connection_id,
            %peer,
            error = %e,
            "Portman WS connection terminated with error"
        );
    }

    let removed = state.subscribers.remove(connection_id);
    match removed {
        Some(sub) => {
            info!(
                connection_id = sub.connection_id,
                %peer,
                remaining = state.subscribers.len(),
                "Portman WS disconnected and removed from subscribers"
            );
        }
        None => {
            // This should not normally happen, but avoid panicking in handler.
            warn!(
                connection_id,
                %peer,
                "Portman WS disconnected but subscriber was not found in registry"
            );
        }
    }
}

/// Drive the WS connection: read frames, log them, and send basic ACKs.
///
/// This is kept separate from `handle_ws_connection` so that later we can
/// swap the body for real protocol handling while preserving connection
/// registration/teardown behavior.
fn drive_ws_connection(
    mut socket: WebSocket,
    peer: SocketAddr,
    connection_id: u64,
    subscribers: Arc<PortmanSubscribers>,
    observers: Arc<Observers>,
) -> BoxFuture<'static, Result<()>> {
    Box::pin(async move {
        while let Some(msg_result) = socket.recv().await {
            match msg_result {
                Ok(Message::Text(text)) => {
                    info!(%peer, %text, "received WS text frame from Portman");

                    // Try to parse the incoming frame as a protocol `Envelope`.
                    match serde_json::from_str::<Envelope>(&text) {
                        Ok(env) => {
                            info!(
                                %peer,
                                msg_id = %env.msg_id,
                                agent_id = ?env.agent_id,
                                kind = ?env.kind,
                                direction = ?env.direction,
                                "parsed Portman protocol envelope"
                            );

                            // If this is a RegisterAgent from Portman, update the
                            // subscriber record with the agent_id for discovery.
                            if env.direction == Direction::Up
                                && env.kind == MessageKind::RegisterAgent
                            {
                                if let Some(agent_id) = env.agent_id.clone() {
                                    subscribers.set_agent_id(connection_id, agent_id);
                                }
                            }

                            // Broadcast this envelope to any observers that have
                            // subscribed to relevant streams (either `all` or a
                            // matching `by_kind` filter).
                            broadcast_to_observers(&env, &observers).await?;

                            // For now, simply acknowledge receipt with a generic "ok".
                            if let Err(e) = socket.send(Message::Text("ok".into())).await {
                                warn!(%peer, error = %e, "failed to send WS ack to Portman");
                                break;
                            }
                        }
                        Err(err) => {
                            warn!(
                                %peer,
                                error = %err,
                                "failed to parse WS text frame as protocol Envelope; sending error"
                            );

                            // Send a minimal error envelope back so Portman can see what went wrong.
                            let error_env = Envelope {
                                msg_id: "local-error".to_string(),
                                site_id: None,
                                agent_id: None,
                                direction: Direction::Down,
                                kind: MessageKind::Error,
                                ts: None,
                                payload: serde_json::json!({
                                    "code": "INVALID_ENVELOPE",
                                    "message": format!("failed to parse envelope: {}", err),
                                }),
                            };

                            let payload = match serde_json::to_string(&error_env) {
                                Ok(s) => s,
                                Err(ser_err) => {
                                    warn!(
                                        %peer,
                                        error = %ser_err,
                                        "failed to serialize error envelope; closing connection"
                                    );
                                    break;
                                }
                            };

                            if let Err(e) = socket.send(Message::Text(payload)).await {
                                warn!(%peer, error = %e, "failed to send error envelope to Portman");
                                break;
                            }
                        }
                    }
                }
                Ok(Message::Binary(_bin)) => {
                    // We expect JSON text frames; log and ignore binary for now.
                    warn!(%peer, "ignoring binary WS frame from Portman");
                }
                Ok(Message::Ping(_)) | Ok(Message::Pong(_)) => {
                    // Axum/tungstenite handle ping/pong internally; nothing special needed.
                }
                Ok(Message::Close(frame)) => {
                    info!(%peer, ?frame, "Portman WS close frame received");
                    break;
                }
                Err(e) => {
                    warn!(%peer, error = %e, "error reading WS frame from Portman");
                    break;
                }
            }
        }

        Ok(())
    })
}

/// Broadcast a given envelope to all observer connections that are subscribed
/// to streams covering this protocol message.
///
/// Semantics:
/// - If an observer subscribed to `all`, it receives every envelope.
/// - If an observer subscribed to `by_kind`, it receives envelopes whose
///   `kind` string matches one of its `kinds_filter` entries.
/// - Additional stream types can be layered on later if needed.
async fn broadcast_to_observers(env: &Envelope, observers: &Observers) -> Result<()> {
    let snapshot = observers.list();
    if snapshot.is_empty() {
        return Ok(());
    }

    let payload = serde_json::to_string(env)?;

    // Snapshot active senders so we can broadcast without holding locks while
    // performing IO.
    let sender_snapshot = observers.sender_snapshot();

    // Determine the canonical `kind` string for this envelope.
    let kind_str = serde_json::to_string(&env.kind)?;
    // `kind_str` will be a quoted JSON string (e.g. "\"register_agent\""); trim quotes.
    let kind_str = kind_str.trim_matches('"').to_string();

    for (id, state) in snapshot {
        let wants_all = state.subscribed_kinds.contains(&StreamKind::All);
        let wants_by_kind = state.subscribed_kinds.contains(&StreamKind::ByKind);

        if !wants_all && !wants_by_kind {
            continue;
        }

        if wants_by_kind {
            // If using a by_kind filter, ensure this envelope's kind is in the filter set.
            if let Some(filter) = &state.kinds_filter {
                if !filter.contains(&kind_str) {
                    continue;
                }
            } else {
                // No filter configured; treat as no interest in any specific kind.
                continue;
            }
        }

        if let Some(sender) = sender_snapshot.get(&id) {
            let _ = sender.send(Message::Text(payload.clone()));
        }
    }

    info!(
        kind = ?env.kind,
        subscriber_count = sender_snapshot.len(),
        "broadcasted envelope to subscribed observers"
    );

    Ok(())
}

/// Handle a single WebSocket connection from an observer client on `/observe`.
///
/// Observers send JSON commands to subscribe to streams or request discovery
/// data about the current deployment view.
async fn handle_observer_connection(socket: WebSocket, state: AppState, peer: SocketAddr) {
    // For now, assign a synthetic connection ID based on a simple counter
    // derived from the PortmanSubscribers size plus a large offset to keep
    // namespaces distinct.
    let base_id = state.subscribers.len() as u64 + 1_000_000;
    let connection_id = base_id;
    state.observers.add(connection_id);

    // Split the WebSocket into a sending half (driven via mpsc) and a
    // receiving half (handled in this task). We keep the sending side in
    // the observer registry so that broadcast_to_observers can push frames
    // without needing direct access to this task.
    let (ws_sender, mut ws_receiver) = socket.split();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Message>();
    state.observers.register_sender(connection_id, tx.clone());

    // Spawn a task to forward messages from the channel to the WebSocket.
    let send_peer = peer;
    tokio::spawn(async move {
        let mut ws_sender = ws_sender;
        while let Some(msg) = rx.recv().await {
            if let Err(e) = ws_sender.send(msg).await {
                warn!(connection_id, %send_peer, error = %e, "observer send loop error");
                break;
            }
        }
    });

    info!(
        connection_id,
        %peer,
        "Observer WS connected"
    );

    while let Some(msg_result) = ws_receiver.next().await {
        match msg_result {
            Ok(Message::Text(text)) => {
                info!(%peer, %text, "received WS text frame from observer client");

                match serde_json::from_str::<ObserveCommand>(&text) {
                    Ok(ObserveCommand::Subscribe {
                        kinds,
                        kinds_filter,
                    }) => {
                        let mut set = HashSet::new();
                        for k in kinds {
                            set.insert(k);
                        }

                        // Normalise the optional kinds_filter into a HashSet<String>.
                        let kinds_filter_set =
                            kinds_filter.map(|v| v.into_iter().collect::<HashSet<_>>());

                        state.observers.set_subscription(connection_id, set.clone());

                        // Update the filter on the ObserverState directly.
                        {
                            let mut inner =
                                state.observers.inner.write().expect("Observers poisoned");
                            if let Some(st) = inner.get_mut(&connection_id) {
                                st.kinds_filter = kinds_filter_set;
                            }
                        }

                        let response = serde_json::json!({
                            "status": "ok",
                            "message": "subscription updated",
                            "subscribed_kinds": set.into_iter().map(|sk| match sk {
                                StreamKind::All => "all",
                                StreamKind::ByKind => "by_kind",
                            }).collect::<Vec<_>>(),
                        });
                        if let Err(e) = tx.send(Message::Text(response.to_string())) {
                            warn!(connection_id, %peer, error = %e, "failed to enqueue subscription ack to observer");
                            break;
                        }
                    }
                    Ok(ObserveCommand::Discover { what }) => {
                        // For now we only support discovering connected Portman
                        // agents from the subscriber registry.
                        let subs = state.subscribers.list();
                        let agents: Vec<_> = subs
                            .into_iter()
                            .map(|s| {
                                serde_json::json!({
                                    "connection_id": s.connection_id,
                                    "peer_addr": s.peer_addr.to_string(),
                                    "agent_id": s.agent_id,
                                })
                            })
                            .collect();

                        let response = serde_json::json!({
                            "status": "ok",
                            "what": what.unwrap_or_else(|| "agents".to_string()),
                            "agents": agents,
                        });

                        if let Err(e) = tx.send(Message::Text(response.to_string())) {
                            warn!(connection_id, %peer, error = %e, "failed to enqueue discovery response to observer");
                            break;
                        }
                    }
                    Err(err) => {
                        warn!(
                            connection_id,
                            %peer,
                            error = %err,
                            "failed to decode observer command; sending help"
                        );

                        // Send a help response with valid stream kinds and commands.
                        let valid_kinds: Vec<&'static str> = vec!["all", "by_kind"];
                        let response = serde_json::json!({
                            "status": "error",
                            "code": "INVALID_OBSERVE_COMMAND",
                            "message": format!("failed to parse observe command: {}", err),
                            "valid_commands": ["subscribe", "discover"],
                            "valid_stream_kinds": valid_kinds,
                            "subscribe_examples": [
                                {
                                    "command": "subscribe",
                                    "kinds": ["all"]
                                },
                                {
                                    "command": "subscribe",
                                    "kinds": ["by_kind"],
                                    "kinds_filter": ["register_agent", "heartbeat"]
                                }
                            ],
                        });

                        if let Err(e) = tx.send(Message::Text(response.to_string())) {
                            warn!(connection_id, %peer, error = %e, "failed to enqueue error/help response to observer");
                            break;
                        }
                    }
                }
            }
            Ok(Message::Binary(_)) => {
                warn!(connection_id, %peer, "ignoring binary observer frame");
            }
            Ok(Message::Ping(_)) | Ok(Message::Pong(_)) => {}
            Ok(Message::Close(frame)) => {
                info!(connection_id, %peer, ?frame, "Observer WS close frame received");
                break;
            }
            Err(e) => {
                warn!(connection_id, %peer, error = %e, "error reading observer WS frame");
                break;
            }
        }
    }

    state.observers.remove(connection_id);
    info!(
        connection_id,
        %peer,
        "Observer WS disconnected and removed from registry"
    );
}
