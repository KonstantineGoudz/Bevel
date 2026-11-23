//! Host-side WebRTC net: signaling server + WebRtcSocket as the dungeon's
//! authoritative peer.
//!
//! Direct clients (Bevy native + WASM) connect to the same signaling URL;
//! the first peer becomes the host (this sim). One reliable channel carries
//! `ClientMsg` / `ServerMsg`, one unreliable channel carries `Snapshot`.
//!
//! Interest management lives in [`crate::stream`] (`ChunkIndex` + `PeerView` +
//! `PlayerBuckets`); this module only owns the per-peer state and the
//! matchbox send/queue glue: `stream_world` drains each peer's chunk diff into
//! `WorldChunk` / `WorldUnload` messages and `broadcast` sends each peer its
//! own filtered `Snapshot`.

use std::collections::HashMap;
use std::net::SocketAddr;

use matchbox_signaling::SignalingServer;
use matchbox_socket::{PeerId, PeerState, WebRtcSocketBuilder};
use party_dungeon_net::{decode, encode, ClientMsg, Input, ServerMsg, WorldOp, CHUNK_SEND_BUDGET};

use crate::authority::{self, Authority};
use crate::sim::{Sim, SimEvent};
use crate::stream::{ChunkIndex, PeerView, PlayerBuckets};

/// Signaling + host socket state. Call `poll` each tick to drain inputs,
/// `stream_world` to stream level cells, and `broadcast` to fan out the
/// per-receiver snapshots.
pub struct HostNet {
    pub sim: Sim,
    pub socket: matchbox_socket::WebRtcSocket,
    /// PeerId -> sub (auth subject) after Hello is verified. Provider-agnostic:
    /// Firebase UID, Auth0 sub, etc. — whatever the JWT's `sub` claim carries.
    pub peers: HashMap<PeerId, String>,
    /// Latest input per peer.
    pub inputs: HashMap<String, Input>,
    /// Spatial index of the streamed world (rebuilt on dungeon version bumps)
    /// plus one interest view per peer (radii, resident cells, chunk queue,
    /// visible players). Torn down with its peer.
    pub stream: ChunkIndex,
    pub views: HashMap<PeerId, PeerView>,
    /// Last `dungeon_version` announced with `ServerMsg::DungeonChanged`. The
    /// changed cells themselves arrive as `WorldChunk`s from `stream_world`.
    pub notified_version: u64,
    /// Transport-agnostic session + world-edit persistence (shared with the
    /// replicon path): service-role player rows and cache, and the
    /// persist → apply → bump edit state machine.
    pub authority: Authority,
}

impl HostNet {
    pub fn new(
        sim: Sim,
        socket: matchbox_socket::WebRtcSocket,
        surreal: Option<party_dungeon_server::SurrealConfig>,
        service_user: String,
        service_pass: String,
    ) -> Self {
        let notified_version = sim.dungeon_version;
        Self {
            sim,
            socket,
            peers: HashMap::new(),
            inputs: HashMap::new(),
            stream: ChunkIndex::new(),
            views: HashMap::new(),
            notified_version,
            authority: Authority::new(surreal, service_user, service_pass),
        }
    }

    /// Drain reliable messages, verify Hellos, collect inputs, drop departed
    /// peers (and their streaming views).
    pub async fn poll(&mut self) {
        for (peer, state) in self.socket.update_peers() {
            // Peer visibility matters for ops: "the phone never appears" and
            // "it appears but never gets a data path" are different failures.
            eprintln!("sim: peer {peer:?} -> {state:?}");
            if matches!(state, PeerState::Disconnected) {
                self.views.remove(&peer);
                if let Some(sub) = self.peers.remove(&peer) {
                    self.inputs.remove(&sub);
                    self.sim.leave(&sub);
                    self.authority.leave(&sub).await;
                }
            }
        }
        let msgs: Vec<(PeerId, Vec<u8>)> = self
            .socket
            .get_channel_mut(0)
            .unwrap()
            .receive()
            .into_iter()
            .map(|(p, b)| (p, b.to_vec()))
            .collect();
        for (peer, bytes) in msgs {
            if let Ok(msg) = decode::<ClientMsg>(&bytes) {
                self.handle_client_msg(peer, msg).await;
            }
        }
    }

    async fn handle_client_msg(&mut self, peer: PeerId, msg: ClientMsg) {
        match msg {
            ClientMsg::Hello { token, name } => {
                let sub = match authority::verify_token(&token) {
                    Ok(sub) => {
                        eprintln!("sim: hello accepted peer={peer:?} sub={sub}");
                        sub
                    }
                    Err(reason) => {
                        eprintln!("sim: hello denied peer={peer:?}: {reason}");
                        let deny = ServerMsg::Deny { reason };
                        let _ = self
                            .socket
                            .get_channel_mut(0)
                            .unwrap()
                            .try_send(encode(&deny).into(), peer);
                        return;
                    }
                };
                // Persist one character per subject (service role, best
                // effort — dev mode with no DB still joins in memory).
                let display = self.authority.join(&sub, name).await;
                let name = display.unwrap_or_else(|| sub.clone());
                self.sim.join(&sub, &name, 100.0, 100.0);
                self.peers.insert(peer, sub.clone());
                let welcome = ServerMsg::Welcome {
                    sub: sub.clone(),
                    tick: self.sim.tick as u32,
                    dungeon_version: self.sim.dungeon_version,
                };
                let _ = self
                    .socket
                    .get_channel_mut(0)
                    .unwrap()
                    .try_send(encode(&welcome).into(), peer);
                // Fresh interest state at the default radii. The client clears
                // its chunk store on Welcome, so the cells stream back in over
                // the next few ticks (`CHUNK_SEND_BUDGET` per tick) — never
                // inline in one full-world message.
                self.views.insert(peer, PeerView::new());
            }
            ClientMsg::Input(input) => {
                if let Some(sub) = self.peers.get(&peer).cloned() {
                    self.inputs.insert(sub, input);
                }
            }
            ClientMsg::SetInterest {
                world_radius,
                player_radius,
            } => {
                // Unknown peer (no Hello yet): ignore, the view is created on
                // Hello with the defaults.
                if let Some(view) = self.views.get_mut(&peer) {
                    view.set_interest(world_radius, player_radius);
                }
            }
            ClientMsg::WorldEdit { ops } => {
                // Async-friendly like `persist_join`: the DB write is awaited
                // in the message path, not spawned, so a batch is ordered
                // before the version bump and the chunk echo.
                self.handle_world_edit(peer, ops).await;
            }
        }
    }

    /// Handle one authored edit batch through the shared [`Authority`].
    ///
    /// The authority persists each op (or applies it in memory in dev mode),
    /// applies it to the sim and bumps `dungeon_version` once per batch. This
    /// path only maps the returned transport-neutral replies onto the legacy
    /// reliable channel. They are queued here — before `stream_world()` runs
    /// later in the same tick — so on the ordered channel the client always
    /// learns its rid before the streamed echo.
    async fn handle_world_edit(&mut self, peer: PeerId, ops: Vec<WorldOp>) {
        let Some(sub) = self.peers.get(&peer).cloned() else {
            eprintln!("sim: world edit from peer {peer:?} ignored: no Hello yet");
            return;
        };
        let replies = self
            .authority
            .apply_world_edit(&sub, &ops, &mut self.sim)
            .await;
        for reply in replies {
            self.send_reliable(peer, reply.into_server_msg());
        }
    }

    fn send_reliable(&mut self, peer: PeerId, msg: ServerMsg) {
        let _ = self
            .socket
            .get_channel_mut(0)
            .unwrap()
            .try_send(encode(&msg).into(), peer);
    }

    /// Step the sim one tick and return events.
    pub fn step(&mut self) -> Vec<SimEvent> {
        self.sim.step(&self.inputs)
    }

    /// One streaming tick for the whole dungeon: refresh the chunk index when
    /// the sim's `dungeon_version` moved, then give every peer its diff —
    /// queued changed chunks (bounded by `CHUNK_SEND_BUDGET`), one batched
    /// `WorldUnload` for cells that left its radius + margin, and nothing else.
    /// A new peer's initial cells stream in over the following ticks.
    pub fn stream_world(&mut self) {
        // Rebuild when the dungeon changed (edit / interact toggle / hot
        // reload). Content-diffing keeps untouched cells' versions stable.
        let _ = self.stream.sync(&self.sim);

        let peers: Vec<PeerId> = self.socket.connected_peers().collect();
        for peer in peers {
            let Some(sub) = self.peers.get(&peer).cloned() else {
                continue; // not past Hello yet
            };
            let origin = self
                .sim
                .players
                .get(&sub)
                .map(|p| [p.pos.x, p.pos.y])
                .unwrap_or([self.sim.spawn.x, self.sim.spawn.y]);
            let Some(view) = self.views.get_mut(&peer) else {
                continue;
            };
            let update = view.tick(&self.stream, origin, CHUNK_SEND_BUDGET);
            if update.chunks.is_empty() && update.unload.is_empty() {
                continue;
            }
            // Reliable channel 0: ordered, so a cell's unload never overtakes
            // the chunk that replaced it.
            let channel = self.socket.get_channel_mut(0).unwrap();
            for chunk in update.chunks {
                let msg = ServerMsg::WorldChunk(chunk);
                let _ = channel.try_send(encode(&msg).into(), peer);
            }
            if !update.unload.is_empty() {
                let msg = ServerMsg::WorldUnload { cells: update.unload };
                let _ = channel.try_send(encode(&msg).into(), peer);
            }
        }
    }

    /// Coarse "the dungeon changed" notification, sent once per version bump.
    /// The changed cells themselves follow as `WorldChunk` messages from
    /// [`HostNet::stream_world`]; this is for the HUD / logging.
    pub fn notify_dungeon_changed(&mut self) {
        let version = self.sim.dungeon_version;
        if version == self.notified_version {
            return;
        }
        self.notified_version = version;
        self.broadcast_reliable(&ServerMsg::DungeonChanged {
            dungeon_version: version,
        });
    }

    /// Broadcast one filtered snapshot per peer on the unreliable channel.
    /// The spatial player buckets are built once per tick; each peer sees the
    /// players within its `player_radius` (plus the exit margin), always
    /// including itself, sorted by `sub`.
    pub fn broadcast(&mut self, server_ms: i64) {
        let peers: Vec<PeerId> = self.socket.connected_peers().collect();
        if peers.is_empty() {
            // No receivers: do not clone the player roster or build buckets.
            return;
        }

        let tick = self.sim.tick as u32;
        let dungeon_version = self.sim.dungeon_version;
        let buckets = PlayerBuckets::build(self.sim.player_states());
        let spawn = [self.sim.spawn.x, self.sim.spawn.y];

        let mut snaps = Vec::with_capacity(peers.len());
        for peer in peers {
            let Some(sub) = self.peers.get(&peer).cloned() else {
                continue;
            };
            let origin = self
                .sim
                .players
                .get(&sub)
                .map(|p| [p.pos.x, p.pos.y])
                .unwrap_or(spawn);
            let Some(view) = self.views.get_mut(&peer) else {
                continue;
            };
            let snap = buckets.snapshot(
                tick,
                server_ms,
                dungeon_version,
                &sub,
                origin,
                view.player_radius,
                &mut view.visible_players,
            );
            snaps.push((peer, snap));
        }

        let channel = self.socket.get_channel_mut(1).unwrap();
        for (peer, snap) in snaps {
            let _ = channel.try_send(encode(&snap).into(), peer);
        }
    }

    pub fn broadcast_reliable(&mut self, msg: &ServerMsg) {
        let bytes = encode(msg);
        for peer in self.socket.connected_peers().collect::<Vec<_>>() {
            let _ = self
                .socket
                .get_channel_mut(0)
                .unwrap()
                .try_send(bytes.clone().into(), peer);
        }
    }
}

/// Spawn a ClientServer signaling server on `addr`.
pub fn signaling_server(addr: SocketAddr) -> SignalingServer {
    SignalingServer::client_server_builder(addr).build()
}

/// ICE servers from `DUNGEON_ICE_SERVERS` (JSON in the browser `RTCIceServer`
/// shape), when set. The sim must be given the same relay configuration as its
/// clients: a TURN allocation is what lets a client behind a carrier-grade NAT
/// (mobile data) reach the host, and a relayed link is the fallback when both
/// ends are behind strict NATs.
pub fn configured_ice_server() -> Option<matchbox_socket::RtcIceServerConfig> {
    let raw = std::env::var("DUNGEON_ICE_SERVERS")
        .ok()
        .filter(|value| !value.trim().is_empty())?;
    let servers = match party_dungeon_net::parse_ice_servers(&raw) {
        Ok(servers) => servers,
        Err(error) => {
            eprintln!("sim: ignoring DUNGEON_ICE_SERVERS: {error}");
            return None;
        }
    };
    let (urls, username, credential) = party_dungeon_net::flatten_ice_servers(&servers);
    if urls.is_empty() {
        return None;
    }
    eprintln!(
        "sim: ICE: {} configured url(s){}",
        urls.len(),
        if username.is_some() {
            " (TURN credentials set)"
        } else {
            ""
        }
    );
    Some(matchbox_socket::RtcIceServerConfig {
        urls,
        username,
        credential,
    })
}

/// Build a host WebRtcSocket connected to `signaling_url`. The future must be
/// spawned (e.g., `tokio::spawn(future)`).
pub fn host_socket(
    signaling_url: String,
) -> (
    matchbox_socket::WebRtcSocket,
    impl std::future::Future<Output = Result<(), matchbox_socket::Error>> + Send,
) {
    let mut builder = WebRtcSocketBuilder::new(signaling_url)
        .add_channel(matchbox_socket::ChannelConfig::reliable())
        .add_channel(matchbox_socket::ChannelConfig::unreliable());
    if let Some(ice_server) = configured_ice_server() {
        builder = builder.ice_server(ice_server);
    }
    let (socket, fut) = builder.build();
    (socket, fut)
}
