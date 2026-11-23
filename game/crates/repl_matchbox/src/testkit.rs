//! In-memory transport + helpers so two Bevy apps can be linked in one process.
//!
//! This runs the **same** backend code as the WebRTC path: the postcard
//! handshake, the `ConnectedClient` lifecycle, the channel pump and the
//! `ClientState` transitions. Only the byte transport is replaced with shared
//! queues, so an app-pair test exercises real replicon replication and real
//! messages without sockets, threads, timing or a signaling server.
//!
//! Enable the `testkit` feature on the dependency (it implies `server` +
//! `client`) and call [`connect`] after both apps added their plugins:
//!
//! ```ignore
//! let mut link = party_dungeon_repl_matchbox::testkit::connect(&mut server, &mut client, "sub");
//! assert!(link.settle(&mut server, &mut client));
//! for _ in 0..10 { link.step(&mut server, &mut client); }
//! ```

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use bevy::prelude::*;

use crate::transport::{ClientLinkState, ClientTransport, PeerKey, ServerPeerEvent, ServerTransport};
use crate::{Authenticator, BackendAuthenticator, ClientBackendConfig, ClientSession, Session};

/// Accepts any non-empty token: `sub` and `name` are the token itself. Install
/// your own [`BackendAuthenticator`] before [`connect`] to test auth failures.
pub struct TestAuthenticator;

impl Authenticator for TestAuthenticator {
    fn authenticate(&self, token: &str) -> Result<Session, String> {
        if token.is_empty() {
            return Err("empty token".to_string());
        }
        Ok(Session {
            sub: token.to_string(),
            name: token.to_string(),
        })
    }
}

/// Shared state of one in-memory link.
#[derive(Debug)]
pub(crate) struct MemoryLink {
    peer: PeerKey,
    connected: bool,
    client_state: ClientLinkState,
    /// server -> client packets: `(matchbox channel, bytes)`
    to_client: VecDeque<(usize, Vec<u8>)>,
    /// client -> server packets: `(peer, matchbox channel, bytes)`
    to_server: VecDeque<(PeerKey, usize, Vec<u8>)>,
    /// server-side peer events not yet polled
    server_events: VecDeque<ServerPeerEvent>,
}

impl Default for MemoryLink {
    fn default() -> Self {
        Self {
            peer: 1,
            connected: true,
            client_state: ClientLinkState::Up,
            to_client: VecDeque::new(),
            to_server: VecDeque::new(),
            // Both apps start "connected": the caller wires them before the
            // first update, so the server sees the peer immediately.
            server_events: VecDeque::from([ServerPeerEvent::Connected(1)]),
        }
    }
}

type Shared = Arc<Mutex<MemoryLink>>;

fn lock(link: &Shared) -> std::sync::MutexGuard<'_, MemoryLink> {
    link.lock().expect("in-memory transport mutex")
}

/// Server end of an in-memory link.
#[derive(Debug)]
pub(crate) struct MemoryHost {
    link: Shared,
}

impl MemoryHost {
    pub(crate) fn poll(&mut self) -> Vec<ServerPeerEvent> {
        lock(&self.link).server_events.drain(..).collect()
    }

    pub(crate) fn recv(&mut self) -> Vec<(PeerKey, usize, Vec<u8>)> {
        lock(&self.link).to_server.drain(..).collect()
    }

    pub(crate) fn send(&mut self, _peer: PeerKey, channel: usize, bytes: Vec<u8>) {
        let mut link = lock(&self.link);
        if link.connected {
            link.to_client.push_back((channel, bytes));
        }
    }

    pub(crate) fn disconnect(&mut self, peer: PeerKey) {
        let mut link = lock(&self.link);
        if !link.connected {
            return;
        }
        link.connected = false;
        link.client_state = ClientLinkState::Closed("disconnected by server".to_string());
        link.to_client.clear();
        link.to_server.clear();
        link.server_events
            .push_back(ServerPeerEvent::Disconnected(peer));
    }
}

/// Client end of an in-memory link.
#[derive(Debug)]
pub(crate) struct MemoryClient {
    link: Shared,
    pub(crate) generation: u64,
}

impl MemoryClient {
    pub(crate) fn poll(&mut self) -> ClientLinkState {
        lock(&self.link).client_state.clone()
    }

    pub(crate) fn recv(&mut self) -> Vec<(usize, Vec<u8>)> {
        lock(&self.link).to_client.drain(..).collect()
    }

    pub(crate) fn send(&mut self, channel: usize, bytes: Vec<u8>) {
        let mut link = lock(&self.link);
        if link.connected {
            let peer = link.peer;
            link.to_server.push_back((peer, channel, bytes));
        }
    }

    pub(crate) fn close(&mut self) {
        let mut link = lock(&self.link);
        if !link.connected {
            return;
        }
        link.connected = false;
        link.client_state = ClientLinkState::Closed("closed by client".to_string());
        link.to_client.clear();
        link.to_server.clear();
        let peer = link.peer;
        link.server_events
            .push_back(ServerPeerEvent::Disconnected(peer));
    }
}

/// A live in-memory link between a server app and a client app. Keep it alive
/// for as long as the apps are stepped (it holds the shared transport state).
#[derive(Debug)]
pub struct TestLink {
    _link: Shared,
}

/// Wire a server app and a client app together over the in-memory transport.
///
/// Inserts the memory transports in both apps, and — only when absent — a
/// [`TestAuthenticator`] on the server and a [`ClientBackendConfig`] with
/// `token` on the client, so a caller's own auth/token setup always wins.
///
/// Both apps must already have `MinimalPlugins`, `StatesPlugin`,
/// `RepliconPlugins` (with `AuthMethod::Custom`), the shared replication
/// schema, and [`crate::MatchboxServerPlugin`] / [`crate::MatchboxClientPlugin`].
pub fn connect(
    server_app: &mut App,
    client_app: &mut App,
    token: impl Into<String>,
) -> TestLink {
    let link: Shared = Arc::new(Mutex::new(MemoryLink::default()));

    server_app.world_mut().insert_resource(ServerTransport::Memory(MemoryHost {
        link: link.clone(),
    }));
    client_app
        .world_mut()
        .insert_resource(ClientTransport::Memory(MemoryClient {
            link: link.clone(),
            generation: 1,
        }));

    if server_app
        .world()
        .get_resource::<BackendAuthenticator>()
        .is_none()
    {
        server_app
            .world_mut()
            .insert_resource(BackendAuthenticator(Box::new(TestAuthenticator)));
    }
    if client_app
        .world()
        .get_resource::<ClientBackendConfig>()
        .is_none()
    {
        client_app.world_mut().insert_resource(ClientBackendConfig {
            signaling_url: "memory://".to_string(),
            token: token.into(),
        });
    }

    TestLink { _link: link }
}

impl TestLink {
    /// One server update followed by one client update. Packets written during
    /// one update are visible to the other side's next update.
    ///
    /// Each step also forces exactly one fixed tick (replicon advances
    /// `ServerTick` in `FixedPostUpdate`), so a test does not depend on real
    /// wall-clock time accumulating between updates.
    pub fn step(&mut self, server_app: &mut App, client_app: &mut App) {
        force_fixed_step(server_app);
        force_fixed_step(client_app);
        server_app.update();
        client_app.update();
    }

    /// Step until `ready` returns true (checked before every step), up to
    /// `max_steps`. Returns whether it became true.
    pub fn run_until(
        &mut self,
        server_app: &mut App,
        client_app: &mut App,
        max_steps: usize,
        mut ready: impl FnMut(&mut App, &mut App) -> bool,
    ) -> bool {
        for _ in 0..max_steps {
            if ready(server_app, client_app) {
                return true;
            }
            self.step(server_app, client_app);
        }
        ready(server_app, client_app)
    }

    /// Pump until the handshake is done: the server authorized a client and the
    /// client reached `ClientState::Connected`.
    pub fn settle(&mut self, server_app: &mut App, client_app: &mut App) -> bool {
        self.run_until(server_app, client_app, 64, |server, client| {
            server_has_client(server) && client_reached_connected(client)
        })
    }

    /// The authorized server-side client entity (query `ClientSession` on it).
    pub fn client_entity(&self, server_app: &App) -> Option<Entity> {
        server_app
            .world()
            .iter_entities()
            .find(|entity| entity.contains::<ClientSession>())
            .map(|entity| entity.id())
    }
}

/// Make the next `App::update()` run exactly one `FixedUpdate` step.
fn force_fixed_step(app: &mut App) {
    if let Some(mut fixed) = app.world_mut().get_resource_mut::<Time<Fixed>>() {
        let timestep = fixed.timestep();
        fixed.accumulate_overstep(timestep);
    }
}

pub use crate::{client_reached_connected, server_has_client};
