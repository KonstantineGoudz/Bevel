//! Transport abstraction: matchbox/WebRTC in production, in-memory for tests.
//!
//! Replicon only sees [`ServerMessages`](bevy_replicon::prelude::ServerMessages)
//! / [`ClientMessages`](bevy_replicon::prelude::ClientMessages); this module
//! turns those into matchbox data channels and back. The enums here are the
//! single place where the socket types appear, so the handshake/channel-pump/
//! state code in `server.rs`/`client.rs` is transport-agnostic — and the
//! testkit runs exactly that code over an in-memory link.

#![allow(dead_code, unused_imports)]

use bevy::prelude::*;

use bevy_replicon::prelude::{Channel, RepliconChannels};
use matchbox_socket::{ChannelConfig, RtcIceServerConfig};

#[cfg(any(feature = "server", feature = "client"))]
use matchbox_socket::WebRtcSocket;

/// Stable backend peer key. The matchbox `PeerId` is mapped onto this so the
/// replicon `NetworkId` is `u64` (the testkit uses plain integers).
#[cfg(feature = "server")]
pub(crate) type PeerKey = u64;

// ---------------------------------------------------------------------------
// Channel creation (from replicon's channel list)
// ---------------------------------------------------------------------------

/// matchbox channel configs, index-aligned with the combined replicon channels:
/// index 0 is the reliable handshake control channel, then one matchbox channel
/// per replicon channel index `i` (server channel `i` for sends, client channel
/// `i` for receives).
///
/// matchbox needs a single reliability per index, so we take the stronger of
/// the two directions — replicon explicitly allows channels that are "at least
/// as reliable as requested".
pub(crate) fn channel_configs(channels: &RepliconChannels) -> Vec<ChannelConfig> {
    let server = channels.server_channels();
    let client = channels.client_channels();
    let count = server.len().max(client.len());

    let mut configs = Vec::with_capacity(count + 1);
    // Control channel: reliable.
    configs.push(ChannelConfig::reliable());
    for i in 0..count {
        let reliable = is_reliable(server.get(i)) || is_reliable(client.get(i));
        configs.push(if reliable {
            ChannelConfig::reliable()
        } else {
            ChannelConfig::unreliable()
        });
    }
    configs
}

fn is_reliable(channel: Option<&Channel>) -> bool {
    matches!(channel, Some(Channel::Ordered | Channel::Unordered))
}

// ---------------------------------------------------------------------------
// Message loop driver
// ---------------------------------------------------------------------------

/// Keeps the matchbox message loop alive for the lifetime of the transport.
#[derive(Debug)]
pub(crate) enum MessageLoopDriver {
    /// Own dedicated thread blocking on the future (no async runtime needed).
    #[cfg(not(target_arch = "wasm32"))]
    Thread(std::thread::JoinHandle<()>),
    /// Spawned on the host's Tokio runtime (`BackendRuntime`).
    #[cfg(not(target_arch = "wasm32"))]
    Tokio(tokio::task::JoinHandle<()>),
    /// Browser: `wasm_bindgen_futures::spawn_local`.
    #[cfg(target_arch = "wasm32")]
    Local,
}

/// Drive the matchbox message loop.
///
/// On native this spawns on `runtime` when given (the sim's Tokio handle),
/// otherwise on a dedicated thread blocking with `futures::executor`. In the
/// browser it must be `spawn_local` (the future is not `Send` there).
#[cfg(not(target_arch = "wasm32"))]
fn drive_message_loop(
    future: matchbox_socket::MessageLoopFuture,
    runtime: Option<tokio::runtime::Handle>,
) -> MessageLoopDriver {
    if let Some(handle) = runtime {
        return MessageLoopDriver::Tokio(handle.spawn(async move {
            if let Err(err) = future.await {
                log::warn!("matchbox message loop ended: {err}");
            }
        }));
    }
    let thread = std::thread::Builder::new()
        .name("repl-matchbox".to_string())
        .spawn(move || {
            if let Err(err) = futures::executor::block_on(future) {
                log::warn!("matchbox message loop ended: {err}");
            }
        })
        .expect("failed to spawn the matchbox message-loop thread");
    MessageLoopDriver::Thread(thread)
}

#[cfg(target_arch = "wasm32")]
fn drive_message_loop(future: matchbox_socket::MessageLoopFuture) -> MessageLoopDriver {
    wasm_bindgen_futures::spawn_local(async move {
        if let Err(err) = future.await {
            log::warn!("matchbox message loop ended: {err}");
        }
    });
    MessageLoopDriver::Local
}

/// Spawn the loop for the current target. `runtime` is ignored on wasm (it is
/// only ever passed by the native server).
#[cfg(not(target_arch = "wasm32"))]
fn spawn_loop(
    future: matchbox_socket::MessageLoopFuture,
    runtime: Option<tokio::runtime::Handle>,
) -> MessageLoopDriver {
    drive_message_loop(future, runtime)
}

#[cfg(target_arch = "wasm32")]
fn spawn_loop(future: matchbox_socket::MessageLoopFuture) -> MessageLoopDriver {
    drive_message_loop(future)
}

// ---------------------------------------------------------------------------
// Byte accounting (replicon `*_bps` stats)
// ---------------------------------------------------------------------------

/// Window used to turn the raw byte counters into bytes/second stats.
///
/// One second matches the game's 1 Hz stats log line and, more importantly,
/// averages over bursty replication: the server only sends mutations when a
/// replicated value actually changes, so a 250 ms window can easily cover an
/// idle gap and report `0.0` even though bytes flow every second. Cumulative
/// byte totals (never windowed) are exposed as well for exact measurements.
pub(crate) const STATS_WINDOW_SECS: f64 = 1.0;

/// Rolling byte window: accumulates bytes and publishes bytes/second once per
/// [`STATS_WINDOW_SECS`]. Used to fill replicon's `ClientStats` and
/// `ConnectedClientStats` (the matchbox transport exposes no counters itself).
#[derive(Clone, Debug, Default)]
pub(crate) struct ByteWindow {
    sent: u64,
    received: u64,
    /// Monotonic totals for the whole connection (not reset by a flush).
    sent_total: u64,
    received_total: u64,
    last_flush: Option<f64>,
    sent_bps: f64,
    received_bps: f64,
}

impl ByteWindow {
    pub(crate) fn add_sent(&mut self, bytes: u64) {
        self.sent += bytes;
        self.sent_total += bytes;
    }

    pub(crate) fn add_received(&mut self, bytes: u64) {
        self.received += bytes;
        self.received_total += bytes;
    }

    /// Publish new rates when the window elapsed. Returns whether it did.
    pub(crate) fn flush(&mut self, now: f64) -> bool {
        match self.last_flush {
            None => {
                self.last_flush = Some(now);
                false
            }
            Some(last) => {
                let delta = now - last;
                if delta < STATS_WINDOW_SECS {
                    return false;
                }
                self.sent_bps = self.sent as f64 / delta;
                self.received_bps = self.received as f64 / delta;
                self.sent = 0;
                self.received = 0;
                self.last_flush = Some(now);
                true
            }
        }
    }

    pub(crate) fn sent_bps(&self) -> f64 {
        self.sent_bps
    }

    pub(crate) fn received_bps(&self) -> f64 {
        self.received_bps
    }

    pub(crate) fn sent_total(&self) -> u64 {
        self.sent_total
    }

    pub(crate) fn received_total(&self) -> u64 {
        self.received_total
    }

    /// Forget the current sample window (e.g. on reconnect).
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }
}

// ---------------------------------------------------------------------------
// Server transport
// ---------------------------------------------------------------------------

/// Peers connecting/leaving, as reported by the transport.
#[cfg(all(feature = "server", not(target_arch = "wasm32")))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ServerPeerEvent {
    Connected(PeerKey),
    Disconnected(PeerKey),
}

/// Server-side transport: a matchbox host socket in production, an in-memory
/// link in tests.
// The matchbox variant is much larger than the in-memory one, but this enum is
// a single long-lived resource — boxing would only add an indirection.
#[allow(clippy::large_enum_variant)]
#[cfg(all(feature = "server", not(target_arch = "wasm32")))]
#[derive(Resource)]
pub(crate) enum ServerTransport {
    Matchbox(MatchboxHost),
    #[cfg(feature = "testkit")]
    Memory(crate::testkit::MemoryHost),
}

#[cfg(all(feature = "server", not(target_arch = "wasm32")))]
impl ServerTransport {
    pub(crate) fn poll(&mut self) -> Vec<ServerPeerEvent> {
        match self {
            Self::Matchbox(host) => host.poll(),
            #[cfg(feature = "testkit")]
            Self::Memory(host) => host.poll(),
        }
    }

    /// All received packets since the last call: `(peer, matchbox channel, bytes)`.
    pub(crate) fn recv(&mut self) -> Vec<(PeerKey, usize, Vec<u8>)> {
        match self {
            Self::Matchbox(host) => host.recv(),
            #[cfg(feature = "testkit")]
            Self::Memory(host) => host.recv(),
        }
    }

    pub(crate) fn send(&mut self, peer: PeerKey, channel: usize, bytes: Vec<u8>) {
        match self {
            Self::Matchbox(host) => host.send(peer, channel, bytes),
            #[cfg(feature = "testkit")]
            Self::Memory(host) => host.send(peer, channel, bytes),
        }
    }

    /// Stop talking to `peer` (the caller sends a control `Deny` first so the
    /// client learns why). matchbox 0.15 has no peer-disconnect API, so this
    /// forgets the peer and drops its traffic; the client closes its socket on
    /// the `Deny`.
    pub(crate) fn disconnect(&mut self, peer: PeerKey) {
        match self {
            Self::Matchbox(host) => host.disconnect(peer),
            #[cfg(feature = "testkit")]
            Self::Memory(host) => host.disconnect(peer),
        }
    }
}

/// matchbox host socket wrapper. `WebRtcSocket` is not `Sync`, so it lives
/// behind a `Mutex` to be a Bevy resource.
#[cfg(all(feature = "server", not(target_arch = "wasm32")))]
pub(crate) struct MatchboxHost {
    socket: std::sync::Mutex<WebRtcSocket>,
    /// matchbox peer -> backend key.
    peers: std::collections::HashMap<matchbox_socket::PeerId, PeerKey>,
    /// backend key -> matchbox peer.
    by_key: std::collections::HashMap<PeerKey, matchbox_socket::PeerId>,
    next_key: PeerKey,
    channels: usize,
    closed: bool,
    _driver: MessageLoopDriver,
}

#[cfg(all(feature = "server", not(target_arch = "wasm32")))]
impl MatchboxHost {
    pub(crate) fn new(
        signaling_url: &str,
        channel_configs: Vec<ChannelConfig>,
        ice: Option<RtcIceServerConfig>,
        runtime: Option<tokio::runtime::Handle>,
    ) -> Self {
        use matchbox_socket::WebRtcSocketBuilder;

        let channels = channel_configs.len();
        let mut builder = WebRtcSocketBuilder::new(signaling_url);
        for config in channel_configs {
            builder = builder.add_channel(config);
        }
        if let Some(ice) = ice {
            builder = builder.ice_server(ice);
        }
        let (socket, future) = builder.build();
        let driver = spawn_loop(future, runtime);
        Self {
            socket: std::sync::Mutex::new(socket),
            peers: Default::default(),
            by_key: Default::default(),
            next_key: 1,
            channels,
            closed: false,
            _driver: driver,
        }
    }

    fn poll(&mut self) -> Vec<ServerPeerEvent> {
        if self.closed {
            return Vec::new();
        }
        let mut socket = self.socket.lock().expect("matchbox socket mutex");
        let changes = match socket.try_update_peers() {
            Ok(changes) => changes,
            Err(err) => {
                log::warn!("matchbox host socket closed: {err}");
                self.closed = true;
                let keys: Vec<PeerKey> = self.peers.values().copied().collect();
                self.peers.clear();
                self.by_key.clear();
                return keys.into_iter().map(ServerPeerEvent::Disconnected).collect();
            }
        };
        let mut events = Vec::new();
        for (peer_id, state) in changes {
            match state {
                matchbox_socket::PeerState::Connected => {
                    if let std::collections::hash_map::Entry::Vacant(entry) =
                        self.peers.entry(peer_id)
                    {
                        let key = self.next_key;
                        self.next_key += 1;
                        entry.insert(key);
                        self.by_key.insert(key, peer_id);
                        events.push(ServerPeerEvent::Connected(key));
                    }
                }
                matchbox_socket::PeerState::Disconnected => {
                    if let Some(key) = self.peers.remove(&peer_id) {
                        self.by_key.remove(&key);
                        events.push(ServerPeerEvent::Disconnected(key));
                    }
                }
            }
        }
        events
    }

    fn recv(&mut self) -> Vec<(PeerKey, usize, Vec<u8>)> {
        let channels = self.channels;
        let mut socket = self.socket.lock().expect("matchbox socket mutex");
        let mut out = Vec::new();
        for index in 0..channels {
            let Ok(channel) = socket.get_channel_mut(index) else {
                continue;
            };
            for (peer_id, packet) in channel.receive() {
                if let Some(key) = self.peers.get(&peer_id) {
                    out.push((*key, index, packet.to_vec()));
                }
            }
        }
        out
    }

    fn send(&mut self, peer: PeerKey, channel: usize, bytes: Vec<u8>) {
        let Some(peer_id) = self.by_key.get(&peer).copied() else {
            return;
        };
        let mut socket = self.socket.lock().expect("matchbox socket mutex");
        if let Ok(channel) = socket.get_channel_mut(channel) {
            let _ = channel
                .try_send(bytes.into(), peer_id)
                .inspect_err(|err| log::trace!("matchbox send to {peer:?} failed: {err}"));
        }
    }

    fn disconnect(&mut self, peer: PeerKey) {
        if let Some(peer_id) = self.by_key.remove(&peer) {
            self.peers.remove(&peer_id);
        }
    }
}

// ---------------------------------------------------------------------------
// Client transport
// ---------------------------------------------------------------------------

/// Link state as the client sees it.
#[cfg(feature = "client")]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum ClientLinkState {
    /// No socket yet.
    #[default]
    Off,
    /// Signaling/handshake with the host in progress (no peer yet).
    Connecting,
    /// The WebRTC data channels to the host peer are up.
    Up,
    /// Socket closed or the host went away.
    Closed(String),
}

/// Client-side transport: a matchbox client socket in production, an in-memory
/// link in tests.
#[allow(clippy::large_enum_variant)]
#[cfg(feature = "client")]
#[derive(Resource)]
pub(crate) enum ClientTransport {
    Matchbox(MatchboxClient),
    #[cfg(all(feature = "testkit", not(target_arch = "wasm32")))]
    Memory(crate::testkit::MemoryClient),
}

/// Monotonic id for each socket instance, so reconnects are observable (tests,
/// diagnostics). Not part of the wire protocol.
fn next_generation() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

#[cfg(feature = "client")]
impl ClientTransport {
    /// Which socket instance this is (changes on reconnect).
    pub(crate) fn generation(&self) -> u64 {
        match self {
            Self::Matchbox(client) => client.generation,
            #[cfg(all(feature = "testkit", not(target_arch = "wasm32")))]
            Self::Memory(client) => client.generation,
        }
    }

    pub(crate) fn poll(&mut self) -> ClientLinkState {
        match self {
            Self::Matchbox(client) => client.poll(),
            #[cfg(all(feature = "testkit", not(target_arch = "wasm32")))]
            Self::Memory(client) => client.poll(),
        }
    }

    /// All received packets since the last call: `(matchbox channel, bytes)`.
    pub(crate) fn recv(&mut self) -> Vec<(usize, Vec<u8>)> {
        match self {
            Self::Matchbox(client) => client.recv(),
            #[cfg(all(feature = "testkit", not(target_arch = "wasm32")))]
            Self::Memory(client) => client.recv(),
        }
    }

    pub(crate) fn send(&mut self, channel: usize, bytes: Vec<u8>) {
        match self {
            Self::Matchbox(client) => client.send(channel, bytes),
            #[cfg(all(feature = "testkit", not(target_arch = "wasm32")))]
            Self::Memory(client) => client.send(channel, bytes),
        }
    }

    /// Close the socket (used after a deny, so the host sees us leave).
    pub(crate) fn close(&mut self) {
        match self {
            Self::Matchbox(client) => client.close(),
            #[cfg(all(feature = "testkit", not(target_arch = "wasm32")))]
            Self::Memory(client) => client.close(),
        }
    }

    /// True when the current WebRTC path goes through a TURN relay; `None`
    /// while unknown.
    pub(crate) fn relayed(&self) -> Option<bool> {
        match self {
            Self::Matchbox(client) => client.relayed(),
            #[cfg(all(feature = "testkit", not(target_arch = "wasm32")))]
            Self::Memory(_) => Some(false),
        }
    }
}

/// matchbox client socket wrapper. The host is the single peer in the room
/// (matchbox `ClientServer` topology: the first peer to join is the host, i.e.
/// the sim).
#[cfg(feature = "client")]
pub(crate) struct MatchboxClient {
    socket: std::sync::Mutex<WebRtcSocket>,
    host: Option<matchbox_socket::PeerId>,
    channels: usize,
    state: ClientLinkState,
    closed: bool,
    generation: u64,
    _driver: MessageLoopDriver,
}

#[cfg(feature = "client")]
impl MatchboxClient {
    pub(crate) fn new(
        signaling_url: &str,
        channel_configs: Vec<ChannelConfig>,
        ice: Option<RtcIceServerConfig>,
    ) -> Self {
        use matchbox_socket::WebRtcSocketBuilder;

        let channels = channel_configs.len();
        let mut builder = WebRtcSocketBuilder::new(signaling_url);
        for config in channel_configs {
            builder = builder.add_channel(config);
        }
        if let Some(ice) = ice {
            builder = builder.ice_server(ice);
        }
        let (socket, future) = builder.build();
        // Native: dedicated thread by default; wasm: `spawn_local`.
        #[cfg(not(target_arch = "wasm32"))]
        let driver = spawn_loop(future, None);
        #[cfg(target_arch = "wasm32")]
        let driver = spawn_loop(future);
        Self {
            socket: std::sync::Mutex::new(socket),
            host: None,
            channels,
            state: ClientLinkState::Connecting,
            closed: false,
            generation: next_generation(),
            _driver: driver,
        }
    }

    fn poll(&mut self) -> ClientLinkState {
        if !self.closed {
            let mut socket = self.socket.lock().expect("matchbox socket mutex");
            match socket.try_update_peers() {
                Ok(changes) => {
                    for (peer_id, peer_state) in changes {
                        match peer_state {
                            matchbox_socket::PeerState::Connected => {
                                if self.host.is_none() {
                                    self.host = Some(peer_id);
                                }
                            }
                            matchbox_socket::PeerState::Disconnected => {
                                if self.host == Some(peer_id) {
                                    self.host = None;
                                    self.state = ClientLinkState::Closed(
                                        "host peer disconnected".to_string(),
                                    );
                                }
                            }
                        }
                    }
                    if self.host.is_some() {
                        self.state = ClientLinkState::Up;
                    } else if matches!(self.state, ClientLinkState::Up) {
                        self.state =
                            ClientLinkState::Closed("host peer disconnected".to_string());
                    }
                }
                Err(err) => {
                    self.closed = true;
                    self.state = ClientLinkState::Closed(format!("socket closed: {err}"));
                }
            }
        }
        self.state.clone()
    }

    fn recv(&mut self) -> Vec<(usize, Vec<u8>)> {
        let channels = self.channels;
        let mut socket = self.socket.lock().expect("matchbox socket mutex");
        let mut out = Vec::new();
        for index in 0..channels {
            let Ok(channel) = socket.get_channel_mut(index) else {
                continue;
            };
            for (peer_id, packet) in channel.receive() {
                // Only the host talks to us.
                if Some(peer_id) == self.host {
                    out.push((index, packet.to_vec()));
                }
            }
        }
        out
    }

    fn send(&mut self, channel: usize, bytes: Vec<u8>) {
        let Some(host) = self.host else {
            return;
        };
        let mut socket = self.socket.lock().expect("matchbox socket mutex");
        if let Ok(channel) = socket.get_channel_mut(channel) {
            let _ = channel
                .try_send(bytes.into(), host)
                .inspect_err(|err| log::trace!("matchbox send to host failed: {err}"));
        }
    }

    fn close(&mut self) {
        self.closed = true;
        self.host = None;
        self.state = ClientLinkState::Closed("closed by client".to_string());
        self.socket
            .lock()
            .expect("matchbox socket mutex")
            .close();
    }

    fn relayed(&self) -> Option<bool> {
        let host = self.host?;
        let socket = self.socket.lock().expect("matchbox socket mutex");
        socket.is_relayed(host)
    }
}
