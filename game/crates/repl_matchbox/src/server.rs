//! Server half of the matchbox replicon backend: host socket, per-peer
//! postcard handshake, channel pump and `ConnectedClient` lifecycle.

use std::collections::HashMap;

use bevy::prelude::*;
use bevy_replicon::prelude::*;
use bevy_replicon::server::server_tick::ServerTick;
use bevy_replicon::shared::backend::connected_client::NetworkId;
use postcard::{from_bytes, to_stdvec};

use crate::transport::{channel_configs, ByteWindow, PeerKey, ServerPeerEvent, ServerTransport};
use crate::{
    BackendAuthenticator, BackendIceServer, ClientAuthenticated, ClientDisconnected, ClientSession,
    ClientTraffic, Handshake, ServerBackendConfig, CONTROL_CHANNEL,
};

#[cfg(not(target_arch = "wasm32"))]
use crate::transport::MatchboxHost;

/// Per-peer byte counters feeding replicon's `ConnectedClientStats`.
#[derive(Resource, Default)]
pub(crate) struct ServerByteCounters {
    per_peer: HashMap<PeerKey, ByteWindow>,
}
#[cfg(not(target_arch = "wasm32"))]
use crate::BackendRuntime;

/// Where a peer is in the backend handshake.
#[derive(Clone, Copy, Debug)]
enum PeerPhase {
    /// Connected, waiting for `Handshake::Hello` on the control channel.
    Awaiting,
    /// Handshake failed: `Deny` sent, ignoring the peer until it disconnects.
    Rejected,
    /// Authenticated; replicon traffic flows for `client`.
    Active(Entity),
}

/// Backend peer bookkeeping.
#[derive(Resource, Default)]
pub(crate) struct ServerPeers {
    phases: HashMap<PeerKey, PeerPhase>,
    peer_of_entity: HashMap<Entity, PeerKey>,
    /// When each still-unautenticated peer's data channel opened, plus whether
    /// we already warned about it stalling. Lets the sim log tell a "peer
    /// connected but never handshook" stall apart from a peer that never
    /// arrived at all.
    awaiting_since: HashMap<PeerKey, (std::time::Instant, bool)>,
}

impl ServerPeers {
    fn insert_awaiting(&mut self, peer: PeerKey) {
        self.phases.insert(peer, PeerPhase::Awaiting);
        self.awaiting_since
            .entry(peer)
            .or_insert_with(|| (std::time::Instant::now(), false));
    }

    fn phase(&self, peer: PeerKey) -> Option<PeerPhase> {
        self.phases.get(&peer).copied()
    }

    fn activate(&mut self, peer: PeerKey, client: Entity) {
        self.phases.insert(peer, PeerPhase::Active(client));
        self.peer_of_entity.insert(client, peer);
        self.awaiting_since.remove(&peer);
    }

    fn reject(&mut self, peer: PeerKey) {
        self.phases.insert(peer, PeerPhase::Rejected);
        self.awaiting_since.remove(&peer);
    }

    /// Forget a peer, returning its phase (the caller handles the client entity).
    fn remove_peer(&mut self, peer: PeerKey) -> Option<PeerPhase> {
        let phase = self.phases.remove(&peer)?;
        self.awaiting_since.remove(&peer);
        if let PeerPhase::Active(client) = phase {
            self.peer_of_entity.remove(&client);
        }
        Some(phase)
    }

    /// Forget the peer behind a client entity (`DisconnectRequest` / despawn).
    fn take_entity(&mut self, client: Entity) -> Option<PeerKey> {
        let peer = self.peer_of_entity.remove(&client)?;
        self.phases.remove(&peer);
        Some(peer)
    }

    fn peer_of(&self, client: Entity) -> Option<PeerKey> {
        self.peer_of_entity.get(&client).copied()
    }

    /// `(peer, client entity)` for every authenticated client.
    fn active_pairs(&self) -> Vec<(PeerKey, Entity)> {
        self.phases
            .iter()
            .filter_map(|(peer, phase)| match phase {
                PeerPhase::Active(client) => Some((*peer, *client)),
                _ => None,
            })
            .collect()
    }
}

pub(crate) fn add(app: &mut App) {
    app.init_resource::<ServerPeers>()
        .init_resource::<ServerByteCounters>()
        .add_message::<ClientAuthenticated>()
        .add_message::<ClientDisconnected>()
        .add_systems(
            PreUpdate,
            (
                init_transport.run_if(|transport: Option<Res<ServerTransport>>| transport.is_none()),
                set_running,
                receive_packets,
                flush_stats,
            )
                .chain()
                .in_set(ServerSystems::ReceivePackets),
        )
        .add_systems(
            PostUpdate,
            send_packets.in_set(ServerSystems::SendPackets),
        )
        .add_systems(
            PostUpdate,
            disconnect_by_request.after(ServerSystems::SendPackets),
        )
        .add_observer(cleanup_despawned_client);
}

/// Create the matchbox host socket once `RepliconChannels` is complete (i.e.
/// after `party_dungeon_replication::register`). Runs on the first update.
fn init_transport(
    mut commands: Commands,
    channels: Res<RepliconChannels>,
    config: Option<Res<ServerBackendConfig>>,
    ice: Option<Res<BackendIceServer>>,
    runtime: Option<Res<BackendRuntime>>,
) {
    let config = config.as_deref().cloned().unwrap_or_default();
    let configs = channel_configs(&channels);
    let host = MatchboxHost::new(
        &config.signaling_url,
        configs,
        ice.as_ref().map(|ice| ice.0.clone()),
        runtime.as_ref().map(|runtime| runtime.0.clone()),
    );
    log::info!(
        "repl backend: hosting matchbox room {}",
        config.signaling_url
    );
    commands.insert_resource(ServerTransport::Matchbox(host));
}

/// Replicon only replicates while [`ServerState::Running`].
fn set_running(state: Option<Res<State<ServerState>>>, mut next: ResMut<NextState<ServerState>>) {
    if state.map(|state| *state.get()) != Some(ServerState::Running) {
        next.set(ServerState::Running);
    }
}

/// One tick of transport work: peer lifecycle, then the postcard handshake and
/// replicon packet forwarding.
#[allow(clippy::too_many_arguments)]
fn receive_packets(
    mut transport: ResMut<ServerTransport>,
    mut peers: ResMut<ServerPeers>,
    mut counters: ResMut<ServerByteCounters>,
    mut messages: ResMut<ServerMessages>,
    protocol: Res<ProtocolHash>,
    auth: Option<Res<BackendAuthenticator>>,
    config: Option<Res<ServerBackendConfig>>,
    tick: Res<ServerTick>,
    sessions: Query<&ClientSession>,
    mut commands: Commands,
    mut authenticated: MessageWriter<ClientAuthenticated>,
    mut disconnected: MessageWriter<ClientDisconnected>,
) {
    let max_size = config
        .as_deref()
        .map(|config| config.max_message_size)
        .unwrap_or_else(|| ServerBackendConfig::default().max_message_size);

    // 1. Peers joining/leaving the room.
    for event in transport.poll() {
        match event {
            ServerPeerEvent::Connected(peer) => {
                log::info!("repl backend: peer {peer:?} data channel open, awaiting Hello");
                peers.insert_awaiting(peer);
            }
            ServerPeerEvent::Disconnected(peer) => {
                if let Some(PeerPhase::Active(client)) = peers.remove_peer(peer) {
                    let sub = sessions.get(client).ok().map(|session| session.sub.clone());
                    log::debug!("repl backend: client {client} ({sub:?}) disconnected");
                    commands.entity(client).despawn();
                    disconnected.write(ClientDisconnected { client, sub });
                }
            }
        }
    }

    // 1b. Peers whose data channel is open but whose Hello never arrives.
    // (A phone that shows "waiting for the host peer" while the sim logs the
    // peer as connected is stuck here: WebRTC came up but the handshake did
    // not complete.) Warn once per peer so the sim log answers "did the
    // client ever send Hello?" without spamming every tick.
    for (peer, (since, warned)) in peers.awaiting_since.iter_mut() {
        if *warned || since.elapsed() < std::time::Duration::from_secs(20) {
            continue;
        }
        *warned = true;
        let bytes = counters
            .per_peer
            .get(peer)
            .map(|counter| counter.received_total())
            .unwrap_or(0);
        log::warn!(
            "repl backend: peer {peer:?} data channel open for {}s with no Hello ({} control bytes received)",
            since.elapsed().as_secs(),
            bytes,
        );
    }

    // 2. Packets: the control channel until authorized, then replicon traffic.
    for (peer, channel, bytes) in transport.recv() {
        counters
            .per_peer
            .entry(peer)
            .or_default()
            .add_received(bytes.len() as u64);
        let Some(phase) = peers.phase(peer) else {
            continue; // unknown peer (already forgotten)
        };
        match phase {
            PeerPhase::Awaiting => {
                if channel != CONTROL_CHANNEL {
                    log::debug!(
                        "repl backend: dropping pre-handshake traffic from {peer:?} on channel {channel}"
                    );
                    continue;
                }
                handle_hello(
                    &mut transport,
                    &mut peers,
                    peer,
                    &bytes,
                    &protocol,
                    auth.as_deref(),
                    tick.get(),
                    max_size,
                    &mut commands,
                    &mut authenticated,
                );
            }
            PeerPhase::Rejected => {}
            PeerPhase::Active(_client) => {
                if channel == CONTROL_CHANNEL {
                    // Post-handshake control traffic: answer latency probes.
                    // (A duplicate Hello is simply ignored.)
                    if let Ok(Handshake::Ping { nonce }) = from_bytes::<Handshake>(&bytes)
                        && let Ok(reply) = to_stdvec(&Handshake::Pong { nonce })
                    {
                        transport.send(peer, CONTROL_CHANNEL, reply);
                    }
                    continue;
                }
                let client = _client;
                messages.insert_received(client, channel - 1, bytes);
            }
        }
    }
}

/// Verify the control-channel `Handshake::Hello` and authorize the peer.
#[allow(clippy::too_many_arguments)]
fn handle_hello(
    transport: &mut ServerTransport,
    peers: &mut ServerPeers,
    peer: PeerKey,
    bytes: &[u8],
    protocol: &ProtocolHash,
    auth: Option<&BackendAuthenticator>,
    tick: u32,
    max_size: usize,
    commands: &mut Commands,
    authenticated: &mut MessageWriter<ClientAuthenticated>,
) {
    let Ok(Handshake::Hello {
        token,
        protocol: client_protocol,
    }) = from_bytes::<Handshake>(bytes)
    else {
        return deny(transport, peers, peer, "bad handshake message".to_string());
    };

    if client_protocol != *protocol {
        return deny(transport, peers, peer, "protocol mismatch".to_string());
    }

    let Some(auth) = auth else {
        return deny(transport, peers, peer, "server has no authenticator".to_string());
    };

    let session = match auth.0.authenticate(&token) {
        Ok(session) => session,
        Err(reason) => return deny(transport, peers, peer, format!("auth failed: {reason}")),
    };

    let client = commands
        .spawn((
            ConnectedClient { max_size },
            NetworkId::new(peer),
            ClientSession {
                sub: session.sub.clone(),
                name: session.name.clone(),
            },
            ClientTraffic::default(),
            AuthorizedClient,
        ))
        .id();
    log::info!(
        "repl backend: peer {peer:?} authorized as {} ({client})",
        session.sub
    );

    let welcome = Handshake::Welcome {
        sub: session.sub,
        name: session.name,
        tick,
    };
    if let Ok(bytes) = to_stdvec(&welcome) {
        transport.send(peer, CONTROL_CHANNEL, bytes);
    }
    peers.activate(peer, client);
    authenticated.write(ClientAuthenticated { client });
}

/// Send a control `Deny` and stop processing the peer's traffic. The client
/// closes its socket on the deny, which then surfaces as a disconnect.
fn deny(transport: &mut ServerTransport, peers: &mut ServerPeers, peer: PeerKey, reason: String) {
    log::info!("repl backend: denying peer {peer:?}: {reason}");
    if let Ok(bytes) = to_stdvec(&Handshake::Deny { reason }) {
        transport.send(peer, CONTROL_CHANNEL, bytes);
    }
    peers.reject(peer);
}

/// Forward replicon's outbound messages onto the matching matchbox channels.
fn send_packets(
    mut transport: ResMut<ServerTransport>,
    mut counters: ResMut<ServerByteCounters>,
    mut messages: ResMut<ServerMessages>,
    peers: Res<ServerPeers>,
) {
    for (client, channel, bytes) in messages.drain_sent() {
        let Some(peer) = peers.peer_of(client) else {
            log::trace!("repl backend: dropping message for unknown client {client}");
            continue;
        };
        counters
            .per_peer
            .entry(peer)
            .or_default()
            .add_sent(bytes.len() as u64);
        transport.send(peer, channel + 1, bytes.to_vec());
    }
}

/// Publish the byte counters onto replicon's `ConnectedClientStats` once per
/// stats window (see [`ByteWindow`]). `rtt`/`packet_loss` are left at 0:
/// matchbox/SCTP exposes no losses or round-trip time on the server side.
fn flush_stats(
    time: Option<Res<Time>>,
    mut counters: ResMut<ServerByteCounters>,
    peers: Res<ServerPeers>,
    mut stats: Query<&mut ConnectedClientStats>,
    mut traffic: Query<&mut ClientTraffic>,
) {
    let Some(time) = time else {
        return;
    };
    let now = time.elapsed_secs_f64();
    for (peer, entity) in peers.active_pairs() {
        let window = counters.per_peer.entry(peer).or_default();
        if window.flush(now)
            && let Ok(mut stats) = stats.get_mut(entity)
        {
            stats.0.sent_bps = window.sent_bps();
            stats.0.received_bps = window.received_bps();
        }
        // Cumulative totals (exact measurement; never zeroed by an idle window).
        if let Ok(mut traffic) = traffic.get_mut(entity) {
            traffic.sent_bytes = window.sent_total();
            traffic.received_bytes = window.received_total();
        }
    }
    counters.per_peer.retain(|peer, _| peers.phase(*peer).is_some());
}

/// Replicon asks the backend to drop a client (`DisconnectRequest`): tell the
/// client why, stop pumping it and despawn its entity.
fn disconnect_by_request(
    mut requests: MessageReader<DisconnectRequest>,
    mut peers: ResMut<ServerPeers>,
    mut transport: Option<ResMut<ServerTransport>>,
    sessions: Query<&ClientSession>,
    mut commands: Commands,
    mut disconnected: MessageWriter<ClientDisconnected>,
) {
    for request in requests.read() {
        let client = request.client;
        let Some(peer) = peers.take_entity(client) else {
            continue;
        };
        log::debug!("repl backend: disconnecting client {client} by request");
        if let Some(transport) = transport.as_mut() {
            if let Ok(bytes) = to_stdvec(&Handshake::Deny {
                reason: "disconnected by server".to_string(),
            }) {
                transport.send(peer, CONTROL_CHANNEL, bytes);
            }
            transport.disconnect(peer);
        }
        let sub = sessions.get(client).ok().map(|session| session.sub.clone());
        commands.entity(client).despawn();
        disconnected.write(ClientDisconnected { client, sub });
    }
}

/// A client entity despawned outside the backend (game logic) must not leave a
/// live matchbox peer behind.
fn cleanup_despawned_client(
    remove: On<Remove<ConnectedClient>>,
    peers: Option<ResMut<ServerPeers>>,
    transport: Option<ResMut<ServerTransport>>,
    sessions: Query<&ClientSession>,
    mut disconnected: MessageWriter<ClientDisconnected>,
) {
    let (Some(mut peers), Some(mut transport)) = (peers, transport) else {
        return;
    };
    let client = remove.entity;
    let Some(peer) = peers.take_entity(client) else {
        return; // already handled by a disconnect path
    };
    log::debug!("repl backend: client {client} despawned, closing peer {peer:?}");
    if let Ok(bytes) = to_stdvec(&Handshake::Deny {
        reason: "disconnected by server".to_string(),
    }) {
        transport.send(peer, CONTROL_CHANNEL, bytes);
    }
    transport.disconnect(peer);
    let sub = sessions.get(client).ok().map(|session| session.sub.clone());
    disconnected.write(ClientDisconnected { client, sub });
}
