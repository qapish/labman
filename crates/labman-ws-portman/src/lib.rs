use std::{
    collections::HashMap,
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
use futures::{future::BoxFuture, Future};
use tokio::net::TcpListener;
use tracing::{error, info, warn};

/// Minimal error type alias for now; we’ll likely switch to `labman_core::LabmanError`
/// once this crate is wired into the rest of the system.
pub type Result<T> = std::result::Result<T, anyhow::Error>;

/// Configuration for the Portman-facing WebSocket server.
///
/// In the first iteration we bind explicitly to a loopback address, and
/// expose a single `/agent` endpoint that Portman connects to.
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
        };
        inner.connections.insert(id, sub.clone());
        sub
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

/// State shared with the WS handlers.
///
/// For now this is just the subscriber registry; in future we’ll also pass a
/// channel or handle to the upstream Conplane WS client to relay messages.
#[derive(Clone)]
struct AppState {
    subscribers: Arc<PortmanSubscribers>,
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
    };

    let app = Router::new()
        .route("/agent", get(handle_ws_upgrade))
        .with_state(state.clone())
        // Provide ConnectInfo&lt;SocketAddr&gt; so handlers using `ConnectInfo`
        // can extract the peer address.
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

/// HTTP handler that upgrades the connection to WebSocket.
async fn handle_ws_upgrade(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    info!(%addr, "Incoming Portman WS connection");
    ws.on_upgrade(move |socket| handle_ws_connection(socket, state, addr))
}

/// Handle a single WebSocket connection from a Portman daemon.
///
/// For now this:
/// - registers the connection in `PortmanSubscribers`
/// - logs text frames
/// - responds with a simple `"ok"` message
async fn handle_ws_connection(socket: WebSocket, state: AppState, peer: SocketAddr) {
    let subscriber = state.subscribers.add(peer);
    info!(
        connection_id = subscriber.connection_id,
        %peer,
        "Portman WS connected"
    );

    let connection_id = subscriber.connection_id;

    if let Err(e) = drive_ws_connection(socket, peer).await {
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
fn drive_ws_connection(mut socket: WebSocket, peer: SocketAddr) -> BoxFuture<'static, Result<()>> {
    Box::pin(async move {
        while let Some(msg_result) = socket.recv().await {
            match msg_result {
                Ok(Message::Text(text)) => {
                    info!(%peer, %text, "received WS text frame from Portman");

                    // For the first iteration, simply acknowledge receipt.
                    if let Err(e) = socket.send(Message::Text("ok".into())).await {
                        warn!(%peer, error = %e, "failed to send WS ack to Portman");
                        break;
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
