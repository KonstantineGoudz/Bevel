//! `bevy_replicon` messaging backend over the existing matchbox/WebRTC
//! transport (the same signaling room + TURN relays the legacy client uses).
//!
//! Replicon is transport-agnostic: a backend only has to
//! * manage [`ClientState`]/[`ServerState`],
//! * move bytes between `matchbox_socket` channels and
//!   `ServerMessages`/`ClientMessages`,
//! * spawn/despawn `ConnectedClient` entities and handle `DisconnectRequest`,
//! * optionally fill `ClientStats`/`ConnectedClientStats`.
//!
//! All messages are raw postcard bytes — no JSON, no protobuf. The handshake is
//! [`Handshake`] (postcard): the client sends its JWT + replicon protocol hash,
//! the server verifies both and answers `Welcome` or `Deny`.
//!
//! # Channel map
//!
//! matchbox channel **0** is a reliable control channel carrying the postcard
//! [`Handshake`]. Replicon channel `i` maps to matchbox channel `i + 1`, with
//! the *stronger* reliability of the two directions (replicon may register a
//! reliable channel on one side and an unreliable one on the other at the same
//! index; matchbox needs one data channel config per index):
//!
//! | matchbox | server channel        | client channel        | reliability |
//! |----------|-----------------------|-----------------------|-------------|
//! | 0        | handshake (control)   | handshake (control)   | reliable    |
//! | 1        | `ServerChannel::Updates` | `ClientChannel::MutationAcks` | reliable |
//! | 2        | `ServerChannel::Mutations` | `InputMsg`       | unreliable  |
//! | 3..      | server messages/events | client messages/events | merged     |
//!
//! # Server
//!
//! ```no_run
//! # use bevy::prelude::*;
//! # use bevy_replicon::prelude::*;
//! # use party_dungeon_repl_matchbox::{MatchboxServerPlugin, BackendAuthenticator, ServerBackendConfig};
//! # fn authenticator() -> BackendAuthenticator { todo!() }
//! let mut app = App::new();
//! app.add_plugins((
//!     MinimalPlugins,
//!     bevy::state::app::StatesPlugin,
//!     RepliconPlugins.set(RepliconSharedPlugin { auth_method: AuthMethod::Custom }),
//! ));
//! // …register the shared replication schema (`party_dungeon_replication::register`)…
//! app.insert_resource(ServerBackendConfig {
//!     signaling_url: "ws://127.0.0.1:6536/party-dungeon".to_string(),
//!     ..Default::default()
//! })
//! .insert_resource(authenticator())
//! .add_plugins(MatchboxServerPlugin);
//! ```
//!
//! After authentication the backend spawns, on the `ConnectedClient` entity:
//!
//! * replicon's `ConnectedClient { max_size }` and `NetworkId` (the matchbox
//!   peer id as `u64`),
//! * [`ClientSession`] (`sub`, `name`) — the authoritative identity,
//! * replicon's `AuthorizedClient` (with `AuthMethod::Custom` the backend must
//!   insert it itself; the plugin asserts the app configured `Custom`).
//!
//! and writes [`ClientAuthenticated`]. On disconnect the entity is despawned
//! and [`ClientDisconnected`] is written; this backend also honours replicon's
//! `DisconnectRequest` (control `Deny` + despawn + peer teardown).
//!
//! Each client entity's `ConnectedClientStats` is filled from the matchbox
//! pump: `sent_bps`/`received_bps` are bytes/second over the last ~250 ms
//! window. `rtt` and `packet_loss` stay 0 on the server — matchbox/SCTP exposes
//! neither (the client measures RTT with its own control-channel ping).
//!
//! # Client
//!
//! ```no_run
//! # use bevy::prelude::*;
//! # use bevy_replicon::prelude::*;
//! # use party_dungeon_repl_matchbox::{MatchboxClientPlugin, ClientBackendConfig};
//! let mut app = App::new();
//! app.add_plugins((
//!     MinimalPlugins,
//!     bevy::state::app::StatesPlugin,
//!     RepliconPlugins.set(RepliconSharedPlugin { auth_method: AuthMethod::Custom }),
//! ));
//! // …register the shared replication schema (`party_dungeon_replication::register`)…
//! app.insert_resource(ClientBackendConfig {
//!     signaling_url: "ws://127.0.0.1:6536/party-dungeon".to_string(),
//!     token: "<jwt>".to_string(),
//! })
//! .add_plugins(MatchboxClientPlugin);
//! ```
//!
//! The backend fills replicon's statistics so the game can log/measure the
//! connection: on the client the `ClientStats` resource (`rtt` in seconds from
//! a ~1 s control-channel ping, `sent_bps`/`received_bps` as bytes/second over
//! a 1 s window) and on the server each client entity's
//! `ConnectedClientStats` (`sent_bps`/`received_bps`; `rtt` and
//! `packet_loss` stay 0 because matchbox/SCTP exposes neither).
//!
//! Because replication only sends mutations when a value changes, a windowed
//! rate can legitimately be 0 during an idle second. For exact before/after
//! byte measurements read the cumulative counters instead: [`BackendTraffic`]
//! (client) and [`ClientTraffic`] (server, per authorized client).
//!
//! The client drives [`ClientState`] (`Disconnected` → `Connecting` →
//! `Connected`) and keeps [`BackendStatus`] up to date for the HUD. On
//! `Welcome` it publishes [`SessionInfo`] (resource) and [`SessionStarted`]
//! (message); a rejected handshake publishes [`SessionDenied`] and leaves
//! `ClientState::Disconnected`.
//!
//! Socket creation is **lazy**: nothing is created (and `BackendState` stays
//! `Idle`) until a [`ClientBackendConfig`] resource exists. Insert it (plus
//! [`BackendIceServer`]) once the in-game login produced a token; the first
//! update after the insert dials the configured URL and sends the handshake.
//! Changing the resource, or writing [`ReconnectRequest`], drops the socket and
//! reconnects with the new config.
//!
//! # Runtime / threading
//!
//! The matchbox message loop is driven on its own dedicated `std::thread` by
//! default, so a Bevy app running on a plain thread (e.g. inside
//! `ScheduleRunnerPlugin`) needs no async runtime. Insert
//! [`BackendRuntime`] (native) before the first update to spawn the loop on an
//! existing Tokio runtime instead. In the browser the loop is spawned with
//! `wasm_bindgen_futures::spawn_local`.
//!
//! # Tests
//!
//! With the `testkit` feature, [`testkit::connect`] wires two in-process apps
//! over an in-memory transport that runs the *same* handshake, channel pump and
//! state machine as the WebRTC path.

#![allow(unused)]

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

pub use bevy_replicon;
pub use bevy_replicon::prelude::{
    AuthMethod, ClientState, ProtocolHash, RepliconChannels, RepliconPlugins, RepliconSharedPlugin,
    ServerState,
};
pub use matchbox_socket::RtcIceServerConfig;

#[cfg(feature = "client")]
mod client;
mod transport;

#[cfg(all(feature = "server", target_arch = "wasm32"))]
compile_error!(
    "the `server` feature (and `testkit`) is native-only; the browser builds the client half only"
);

#[cfg(all(feature = "server", not(target_arch = "wasm32")))]
mod server;

#[cfg(all(not(target_arch = "wasm32"), feature = "testkit"))]
pub mod testkit;

#[cfg(test)]
mod tests;

/// The signaling room path. Kept as `/party-dungeon`: it is the matchbox room
/// id and the NPM location that proxies to the sim's signaling port.
pub const ROOM_PATH: &str = "/party-dungeon";

/// matchbox channel index of the reliable postcard [`Handshake`] control
/// channel. Replicon channel `i` lives on matchbox channel `i + 1`.
pub const CONTROL_CHANNEL: usize = 0;

/// Handshake sent by the client before replicon replication starts, on the
/// reliable control channel.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Handshake {
    Hello {
        token: String,
        /// Replicon's protocol hash (all replication/message registrations).
        /// `ProtocolHash` is a serializable newtype, so equality is exact.
        protocol: ProtocolHash,
    },
    Welcome {
        sub: String,
        name: String,
        /// Server tick the client's inputs should start from (replicon tick).
        tick: u32,
    },
    Deny {
        reason: String,
    },
    /// Latency probe sent by an authenticated client on the control channel
    /// (matchbox/SCTP does not expose an RTT). The server answers
    /// [`Handshake::Pong`] with the same nonce.
    Ping {
        nonce: u64,
    },
    /// Answer to [`Handshake::Ping`].
    Pong {
        nonce: u64,
    },
}

/// Result of a successful authentication.
#[derive(Clone, Debug)]
pub struct Session {
    pub sub: String,
    pub name: String,
}

/// Server-side verifier: turns a client JWT into a session (or a deny reason).
///
/// The sim implements this with `party_dungeon_server::decode_claims_unverified`
/// (plus the same follow-up the legacy `Hello` did: persist one character per
/// subject and resolve its display name).
pub trait Authenticator: Send + Sync + 'static {
    fn authenticate(&self, token: &str) -> Result<Session, String>;
}

/// Resource with the server's [`Authenticator`]. Without it every handshake is
/// denied (`no authenticator`) — fail closed.
#[derive(Resource)]
pub struct BackendAuthenticator(pub Box<dyn Authenticator>);

/// Server transport configuration.
#[derive(Resource, Clone, Debug)]
pub struct ServerBackendConfig {
    /// Full matchbox room URL, e.g. `ws://127.0.0.1:6536/party-dungeon`.
    /// Keep the `/party-dungeon` room path — it is what NPM proxies.
    pub signaling_url: String,
    /// Value for replicon's `ConnectedClient::max_size` (splitting budget for
    /// unreliable mutation messages), in bytes.
    pub max_message_size: usize,
}

impl Default for ServerBackendConfig {
    fn default() -> Self {
        Self {
            signaling_url: format!("ws://127.0.0.1:6536{ROOM_PATH}"),
            max_message_size: 16 * 1024,
        }
    }
}

/// Client transport configuration.
#[derive(Resource, Clone, Debug, PartialEq)]
pub struct ClientBackendConfig {
    /// Full matchbox room URL, e.g. `ws://127.0.0.1:6536/party-dungeon`.
    pub signaling_url: String,
    /// JWT sent in [`Handshake::Hello`]; the server verifies it with its
    /// [`BackendAuthenticator`].
    pub token: String,
}

impl Default for ClientBackendConfig {
    fn default() -> Self {
        Self {
            signaling_url: format!("ws://127.0.0.1:6536{ROOM_PATH}"),
            token: String::new(),
        }
    }
}

/// Optional ICE server override for the socket. When absent matchbox's built-in
/// STUN defaults are used. `urls` empty disables STUN (useful for loopback
/// tests); a TURN relay goes here in one credential pair, exactly like the
/// legacy client's flattened `DUNGEON_ICE_SERVERS`.
#[derive(Resource, Clone, Debug)]
pub struct BackendIceServer(pub RtcIceServerConfig);

/// Optional native Tokio runtime handle used to drive the matchbox message
/// loop. When absent the backend spawns its own dedicated thread, which works
/// for a Bevy app that has no async context (the sim embeds its app on a plain
/// thread).
#[cfg(all(feature = "server", not(target_arch = "wasm32")))]
#[derive(Resource, Clone)]
pub struct BackendRuntime(pub tokio::runtime::Handle);

/// Why the last connection attempt failed / what the link is doing (HUD).
#[derive(Resource, Default, Clone, Debug)]
pub struct BackendStatus {
    pub state: BackendState,
    pub detail: String,
    /// Whether the WebRTC path currently goes through a TURN relay (`false`
    /// while unknown).
    pub relayed: bool,
}

#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackendState {
    /// Nothing happened yet.
    #[default]
    Idle,
    /// Signaling is up but no host peer answered yet.
    WaitingForHost,
    /// Link is up; the postcard handshake is in flight.
    Handshaking,
    /// Authenticated, replicon traffic flows.
    Connected,
    /// Denied / link lost / signaling failed — see `detail`.
    Failed,
}

// ---------------------------------------------------------------------------
// Server-side exports (sim)
// ---------------------------------------------------------------------------

/// Authoritative identity of a connected peer, on the replicon
/// `ConnectedClient` entity (spawned by the backend after a verified
/// [`Handshake::Hello`]). Also carries replicon's `NetworkId` (matchbox peer id
/// as `u64`).
#[cfg(feature = "server")]
#[derive(Component, Clone, Debug, PartialEq, Eq)]
pub struct ClientSession {
    pub sub: String,
    pub name: String,
}

/// A peer finished authentication and now has `AuthorizedClient`; read it to
/// hook game state (player entity, interest set) onto `client`.
#[cfg(feature = "server")]
#[derive(Message, Clone, Copy, Debug)]
pub struct ClientAuthenticated {
    pub client: Entity,
}

/// A previously authenticated peer went away (socket closed, deny, or
/// `DisconnectRequest`). Its `ConnectedClient` entity has been despawned.
#[cfg(feature = "server")]
#[derive(Message, Clone, Debug)]
pub struct ClientDisconnected {
    pub client: Entity,
    pub sub: Option<String>,
}

// ---------------------------------------------------------------------------
// Client-side exports (game)
// ---------------------------------------------------------------------------

/// Cumulative traffic counters + the last window's rates for the client
/// connection, updated by the backend. Use this for exact byte measurements
/// (the before/after bandwidth comparison): unlike `ClientStats`'s windowed
/// `*_bps`, the totals never reset, so an idle or bursty second cannot mask the
/// bytes that actually flowed.
#[cfg(feature = "client")]
#[derive(Resource, Default, Clone, Copy, Debug, PartialEq)]
pub struct BackendTraffic {
    /// Payload bytes handed to matchbox since the socket was created.
    pub sent_bytes: u64,
    /// Payload bytes received from matchbox since the socket was created.
    pub received_bytes: u64,
    /// Bytes/second over the last stats window (mirrors `ClientStats`).
    pub sent_bps: f64,
    pub received_bps: f64,
}

/// Server-side cumulative traffic counters for one authorized client, on the
/// same entity as [`ClientSession`]. The windowed rates are in replicon's
/// `ConnectedClientStats`; these totals cannot be hidden by an idle window.
#[cfg(feature = "server")]
#[derive(Component, Default, Clone, Copy, Debug, PartialEq)]
pub struct ClientTraffic {
    /// Payload bytes sent to this client since it connected.
    pub sent_bytes: u64,
    /// Payload bytes received from this client since it connected.
    pub received_bytes: u64,
}

/// Session published on a successful handshake (also written once as
/// [`SessionStarted`]).
#[cfg(feature = "client")]
#[derive(Resource, Clone, Debug)]
pub struct SessionInfo {
    pub sub: String,
    pub name: String,
    /// Server tick the session started at (input baseline).
    pub tick: u32,
}

/// Ask the backend to drop the current socket and open a fresh one from the
/// current [`ClientBackendConfig`] (e.g. after a token refresh or a manual
/// re-login). The new socket is created on the next tick, which runs the
/// handshake again.
///
/// Inserting a *changed* [`ClientBackendConfig`] does the same automatically,
/// so writing this message is only needed to force a reconnect when the config
/// did not change. Socket creation is lazy: nothing is created until a
/// [`ClientBackendConfig`] exists, so the in-game login flow can insert it
/// (and [`BackendIceServer`]) once the user is signed in.
#[cfg(feature = "client")]
#[derive(Message, Clone, Copy, Debug, Default)]
pub struct ReconnectRequest;

/// The handshake succeeded: `sub`/`name` are authoritative.
#[cfg(feature = "client")]
#[derive(Message, Clone, Debug)]
pub struct SessionStarted {
    pub sub: String,
    pub name: String,
    pub tick: u32,
}

/// The server denied the handshake (bad token, protocol mismatch, …). There is
/// no point retrying with the same credentials.
#[cfg(feature = "client")]
#[derive(Message, Clone, Debug)]
pub struct SessionDenied {
    pub reason: String,
}

// ---------------------------------------------------------------------------
// Test / diagnostic helpers
// ---------------------------------------------------------------------------

/// Whether the server has at least one authorized client entity (i.e. a peer
/// completed the handshake). Handy for tests and health checks.
#[cfg(feature = "server")]
pub fn server_has_client(server_app: &mut App) -> bool {
    let world = server_app.world_mut();
    let mut query = world.query_filtered::<Entity, With<ClientSession>>();
    query.iter(world).next().is_some()
}

/// Whether the client reached `ClientState::Connected` (handshake accepted).
#[cfg(feature = "client")]
pub fn client_reached_connected(client_app: &App) -> bool {
    client_app
        .world()
        .get_resource::<State<ClientState>>()
        .is_some_and(|state| *state.get() == ClientState::Connected)
}

// ---------------------------------------------------------------------------
// Plugins
// ---------------------------------------------------------------------------

/// Adds the server half of the backend (matchbox host + channel pump + handshake).
///
/// Requires `RepliconPlugins` configured with `AuthMethod::Custom` (asserted at
/// `Plugin::finish`), [`ServerBackendConfig`] and [`BackendAuthenticator`].
#[cfg(all(feature = "server", not(target_arch = "wasm32")))]
pub struct MatchboxServerPlugin;

#[cfg(all(feature = "server", not(target_arch = "wasm32")))]
impl Plugin for MatchboxServerPlugin {
    fn build(&self, app: &mut App) {
        server::add(app);
    }

    fn finish(&self, app: &mut App) {
        assert_auth_method(app, "MatchboxServerPlugin");
    }
}

/// Adds the client half of the backend (matchbox client + channel pump + handshake).
///
/// Requires `RepliconPlugins` configured with `AuthMethod::Custom` (asserted at
/// `Plugin::finish`) and [`ClientBackendConfig`].
#[cfg(feature = "client")]
pub struct MatchboxClientPlugin;

#[cfg(feature = "client")]
impl Plugin for MatchboxClientPlugin {
    fn build(&self, app: &mut App) {
        client::add(app);
    }

    fn finish(&self, app: &mut App) {
        assert_auth_method(app, "MatchboxClientPlugin");
    }
}

/// The handshake authenticates the peer itself, so replicon must not run its own
/// `ProtocolCheck` (which would also register extra channels and shift the
/// channel map). A mismatch here would only surface as confusing denials, so
/// fail loudly at app build.
#[cfg(any(feature = "server", feature = "client"))]
fn assert_auth_method(app: &App, plugin: &str) {
    match app.world().get_resource::<AuthMethod>() {
        Some(AuthMethod::Custom) => {}
        Some(other) => panic!(
            "{plugin} requires RepliconPlugins configured with AuthMethod::Custom, found {other:?}. \
             Build it as `RepliconPlugins.set(RepliconSharedPlugin {{ auth_method: AuthMethod::Custom }})` \
             on BOTH the server and the client, otherwise the protocol channel registration differs \
             and the channel map desynchronises."
        ),
        None => panic!(
            "{plugin} requires bevy_replicon: add RepliconPlugins with \
             `RepliconSharedPlugin {{ auth_method: AuthMethod::Custom }}` before this plugin."
        ),
    }
}
